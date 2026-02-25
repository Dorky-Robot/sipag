use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use sipag_core::{
    config::{default_sipag_dir, validate_config_file_for_doctor, ConfigEntryStatus, WorkerConfig},
    docker, init,
    state::{self, format_duration},
    worker::{dispatch, github, lifecycle},
};
use std::path::PathBuf;
use std::process::Command;

use crate::init_project;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const GIT_HASH: &str = env!("CARGO_GIT_SHA");

#[derive(Parser)]
#[command(
    name = "sipag",
    version,
    disable_version_flag = true,
    about = "Sandbox launcher for Claude Code",
    long_about = "sipag spins up isolated Docker sandboxes for PR implementation.\n\nRun with no arguments to launch the interactive TUI."
)]
pub struct Cli {
    #[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]
    version: (),

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Install agents, commands, and hooks into a project
    Init {
        /// Project directory (defaults to current directory)
        #[arg(default_value = ".")]
        dir: PathBuf,

        /// Overwrite existing files
        #[arg(long, default_value_t = false)]
        force: bool,

        /// Install generic templates without launching Claude
        #[arg(long, default_value_t = false)]
        r#static: bool,
    },

    /// Dispatch a Docker worker for a PR
    Dispatch {
        /// Repository in owner/repo format
        #[arg(long)]
        repo: String,

        /// PR number to implement
        #[arg(long)]
        pr: u64,
    },

    /// List active and recent workers
    Ps {
        /// Show all workers (not just active + recent)
        #[arg(long, default_value_t = false)]
        all: bool,
    },

    /// Show logs for a worker
    Logs {
        /// Worker identifier (PR number or container name)
        id: String,
    },

    /// Kill a running worker
    Kill {
        /// Worker identifier (PR number or container name)
        id: String,
    },

    /// Launch interactive TUI
    Tui,

    /// Check system prerequisites
    Doctor,

    /// Print version
    Version,
}

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        None => run_tui(),
        Some(Commands::Init {
            dir,
            force,
            r#static: static_only,
        }) => init_project::run_init(&dir, force, static_only),
        Some(Commands::Tui) => run_tui(),
        Some(Commands::Dispatch { repo, pr }) => run_dispatch(&repo, pr),
        Some(Commands::Ps { all }) => run_ps(all),
        Some(Commands::Logs { id }) => run_logs(&id),
        Some(Commands::Kill { id }) => run_kill(&id),
        Some(Commands::Doctor) => run_doctor(),
        Some(Commands::Version) => run_version(),
    }
}

fn run_dispatch(repo: &str, pr_num: u64) -> Result<()> {
    let sipag_dir = default_sipag_dir();
    init::init_dirs(&sipag_dir)?;

    // Clean up stale terminal state files older than 24 hours.
    lifecycle::cleanup_stale(&sipag_dir, 24);

    let cfg = WorkerConfig::load(&sipag_dir)?;

    // Preflight checks.
    github::preflight_gh_auth()?;
    docker::preflight_docker_running()?;
    docker::preflight_docker_image(&cfg.image)?;

    // Ensure the sipag label exists and is on this PR.
    github::ensure_sipag_label(repo);
    github::label_pr_sipag(repo, pr_num);

    // Back-pressure: count active workers (non-terminal state files).
    // This reconciles against Docker to detect dead containers, so zombie
    // workers don't inflate the count. Use the configured staleness threshold
    // rather than the hardcoded default so operator tuning is respected.
    let workers = lifecycle::scan_workers_with_stale_secs(&sipag_dir, cfg.heartbeat_stale_secs);
    if cfg.max_open_prs > 0 {
        let active = workers.iter().filter(|w| !w.phase.is_terminal()).count();
        if active >= cfg.max_open_prs {
            anyhow::bail!(
                "Back-pressure: {active} active workers (max: {}). Wait for workers to finish.",
                cfg.max_open_prs
            );
        }
    }

    // Check for existing worker for this PR.
    if workers
        .iter()
        .any(|w| w.pr_num == pr_num && !w.phase.is_terminal())
    {
        anyhow::bail!("A worker is already running for PR #{pr_num}");
    }

    // Fetch PR details to get branch name.
    let pr_json = Command::new("gh")
        .args([
            "pr",
            "view",
            &pr_num.to_string(),
            "--repo",
            repo,
            "--json",
            "headRefName,body",
        ])
        .output()
        .context("Failed to run gh pr view")?;

    if !pr_json.status.success() {
        anyhow::bail!("PR #{pr_num} not found in {repo}");
    }

    let parsed: serde_json::Value =
        serde_json::from_slice(&pr_json.stdout).unwrap_or(serde_json::json!({}));
    let branch = parsed["headRefName"].as_str().unwrap_or("").to_string();
    let body = parsed["body"].as_str().unwrap_or("").to_string();

    if branch.is_empty() {
        anyhow::bail!("Could not determine branch for PR #{pr_num}");
    }

    // Extract issue numbers from PR body.
    let issues = extract_issue_nums(&body);

    // Load credentials.
    let creds = sipag_core::config::Credentials::load(&sipag_dir)?;

    dispatch::dispatch_worker(repo, pr_num, &branch, &issues, &cfg, &creds)?;
    Ok(())
}

/// Maximum number of terminal workers to show by default (use --all for full list).
const PS_DEFAULT_TERMINAL_LIMIT: usize = 5;

fn run_ps(show_all: bool) -> Result<()> {
    let sipag_dir = default_sipag_dir();
    lifecycle::cleanup_stale(&sipag_dir, 24);
    let all_workers = lifecycle::scan_workers(&sipag_dir);

    let now = chrono::Utc::now();

    // Partition into active and terminal.
    let (active, terminal): (Vec<_>, Vec<_>) =
        all_workers.iter().partition(|w| !w.phase.is_terminal());

    // Filter terminal: drop workers older than 24h with unparsable timestamps.
    let mut terminal: Vec<_> = terminal
        .into_iter()
        .filter(|w| {
            let timestamp = w.ended.as_deref().unwrap_or(&w.started);
            match chrono::DateTime::parse_from_rfc3339(timestamp) {
                Ok(ts) => {
                    let age_hours =
                        (now - ts.with_timezone(&chrono::Utc)).num_hours().max(0) as u64;
                    age_hours < 24
                }
                Err(_) => false,
            }
        })
        .collect();

    // Sort terminal by ended/started time descending (most recent first).
    terminal.sort_by(|a, b| {
        let ts_a = a.ended.as_deref().unwrap_or(&a.started);
        let ts_b = b.ended.as_deref().unwrap_or(&b.started);
        ts_b.cmp(ts_a)
    });

    let hidden = if !show_all && terminal.len() > PS_DEFAULT_TERMINAL_LIMIT {
        let hidden = terminal.len() - PS_DEFAULT_TERMINAL_LIMIT;
        terminal.truncate(PS_DEFAULT_TERMINAL_LIMIT);
        hidden
    } else {
        0
    };

    if active.is_empty() && terminal.is_empty() {
        println!("No workers found.");
        return Ok(());
    }

    let print_worker = |w: &state::WorkerState| {
        let age = if let Ok(started) = chrono::DateTime::parse_from_rfc3339(&w.started) {
            let secs = (now - started.with_timezone(&chrono::Utc))
                .num_seconds()
                .max(0) as u64;
            format_duration(secs)
        } else {
            "?".to_string()
        };

        let container_short = w
            .container_id
            .rfind("pr-")
            .map(|i| &w.container_id[i..])
            .unwrap_or(&w.container_id);

        println!(
            "#{:<7} {:<30} {:<12} {:<8} {}",
            w.pr_num, w.repo, w.phase, age, container_short
        );
        if let Some(ref err) = w.error {
            let short = if err.len() > 60 { &err[..60] } else { err };
            println!("         \x1b[31m↳ {short}\x1b[0m");
        }
    };

    println!(
        "{:<8} {:<30} {:<12} {:<8} CONTAINER",
        "PR", "REPO", "PHASE", "AGE"
    );
    println!("{}", "-".repeat(78));

    for w in &active {
        print_worker(w);
    }
    for w in &terminal {
        print_worker(w);
    }

    if hidden > 0 {
        println!("         ... {hidden} older workers hidden (use --all to show)");
    }

    // Summary counts.
    let finished_count = all_workers
        .iter()
        .filter(|w| w.phase == state::WorkerPhase::Finished)
        .count();
    let failed_count = all_workers
        .iter()
        .filter(|w| w.phase == state::WorkerPhase::Failed)
        .count();
    println!(
        "\n{} active, {} finished, {} failed ({} total)",
        active.len(),
        finished_count,
        failed_count,
        all_workers.len()
    );

    Ok(())
}

fn run_logs(id: &str) -> Result<()> {
    let sipag_dir = default_sipag_dir();

    // Try to find worker by PR number.
    if let Ok(pr_num) = id.trim_start_matches('#').parse::<u64>() {
        let workers = lifecycle::scan_workers(&sipag_dir);
        if let Some(w) = workers.iter().find(|w| w.pr_num == pr_num) {
            // Prefer the log file — it's the authoritative source because
            // Docker stdout is piped directly to it (Docker's own journal
            // receives nothing).
            let log_path = sipag_dir
                .join("logs")
                .join(format!("{}--pr-{pr_num}.log", w.repo.replace('/', "--")));
            if log_path.exists() {
                let content = std::fs::read_to_string(&log_path)?;
                print!("{content}");
                return Ok(());
            }

            // Fallback: try docker logs by stored container name.
            let container_name = w.container_id.clone();
            let status = Command::new("docker")
                .args(["logs", "--tail", "100", &container_name])
                .status();
            return match status {
                Ok(s) if s.success() => Ok(()),
                _ => anyhow::bail!("No logs found for PR #{pr_num}"),
            };
        }
    }

    // Try as container name directly.
    let status = Command::new("docker")
        .args(["logs", "--tail", "100", id])
        .status();

    match status {
        Ok(s) if s.success() => Ok(()),
        _ => anyhow::bail!("No logs found for '{id}'"),
    }
}

fn run_kill(id: &str) -> Result<()> {
    let sipag_dir = default_sipag_dir();

    // Find worker by PR number.
    if let Ok(pr_num) = id.trim_start_matches('#').parse::<u64>() {
        let workers = lifecycle::scan_workers(&sipag_dir);
        if let Some(w) = workers.iter().find(|w| w.pr_num == pr_num) {
            // If the worker already reached a terminal phase, preserve its state.
            // This prevents overwriting a successful `finished` with `failed`.
            if w.phase.is_terminal() {
                println!(
                    "Worker for PR #{pr_num} already {} — state preserved.",
                    w.phase
                );
                return Ok(());
            }

            // Kill the Docker container by stored name.
            let container_name = w.container_id.clone();
            let _ = Command::new("docker")
                .args(["kill", &container_name])
                .status();

            // Update state to failed.
            let mut updated = w.clone();
            updated.phase = state::WorkerPhase::Failed;
            updated.ended = Some(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
            updated.error = Some("Killed by user".to_string());
            state::write_state(&updated)?;

            println!("Killed worker for PR #{pr_num}");
            return Ok(());
        }
    }

    // Try as container name directly.
    let _ = Command::new("docker").args(["kill", id]).status();
    println!("Killed {id}");
    Ok(())
}

fn run_doctor() -> Result<()> {
    let sipag_dir = default_sipag_dir();

    println!("sipag doctor");
    println!("============\n");

    // 1. Docker
    print!("Docker daemon:  ");
    match docker::preflight_docker_running() {
        Ok(_) => println!("OK"),
        Err(e) => println!("FAIL — {e}"),
    }

    // 2. Docker image
    let cfg = WorkerConfig::load(&sipag_dir)
        .unwrap_or_else(|_| WorkerConfig::load(std::path::Path::new("/tmp")).unwrap());
    print!("Docker image:   ");
    match docker::preflight_docker_image(&cfg.image) {
        Ok(_) => println!("OK ({})", cfg.image),
        Err(_) => println!("MISSING ({})", cfg.image),
    }

    // 3. gh auth
    print!("GitHub CLI:     ");
    match github::preflight_gh_auth() {
        Ok(_) => println!("OK"),
        Err(e) => println!("FAIL — {e}"),
    }

    // 4. sipag dir
    print!("sipag dir:      ");
    if sipag_dir.exists() {
        println!("OK ({})", sipag_dir.display());
    } else {
        println!("MISSING ({})", sipag_dir.display());
    }

    // 5. Config file
    if let Some(entries) = validate_config_file_for_doctor(&sipag_dir) {
        println!("\nConfig file ({}/config):", sipag_dir.display());
        for entry in &entries {
            let status_str = match &entry.status {
                ConfigEntryStatus::Valid => "OK".to_string(),
                ConfigEntryStatus::InvalidValue { clamped_to } => {
                    format!("WARN — using {clamped_to}")
                }
                ConfigEntryStatus::Unknown { suggestion } => {
                    if let Some(s) = suggestion {
                        format!("UNKNOWN — did you mean '{s}'?")
                    } else {
                        "UNKNOWN".to_string()
                    }
                }
            };
            println!("  {}={} — {}", entry.key, entry.value, status_str);
        }
    }

    println!();
    Ok(())
}

fn run_version() -> Result<()> {
    println!("sipag {VERSION} ({GIT_HASH})");
    Ok(())
}

fn run_tui() -> Result<()> {
    // Exec the TUI binary.
    let status = Command::new("sipag-tui").status();
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => anyhow::bail!("Failed to launch sipag-tui: {e}"),
    }
}

/// Extract issue numbers from "Closes/Fixes/Resolves #N" in text.
fn extract_issue_nums(body: &str) -> Vec<u64> {
    let mut nums = Vec::new();
    for line in body.lines() {
        let lower = line.to_lowercase();
        for keyword in &["closes #", "fixes #", "resolves #"] {
            let mut search_from = 0;
            while let Some(pos) = lower[search_from..].find(keyword) {
                let abs_pos = search_from + pos + keyword.len();
                let rest = &line[abs_pos..];
                let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(n) = num_str.parse::<u64>() {
                    if !nums.contains(&n) {
                        nums.push(n);
                    }
                }
                search_from = abs_pos;
            }
        }
    }
    nums
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_issue_nums_from_body() {
        assert_eq!(extract_issue_nums("Closes #42"), vec![42]);
        assert_eq!(
            extract_issue_nums("Closes #1\nFixes #2\nResolves #3"),
            vec![1, 2, 3]
        );
        assert!(extract_issue_nums("No refs here").is_empty());
    }

    #[test]
    fn extract_issue_nums_deduplicates() {
        assert_eq!(extract_issue_nums("Closes #5\nFixes #5"), vec![5]);
    }

    #[test]
    fn extract_issue_nums_case_insensitive() {
        assert_eq!(extract_issue_nums("closes #1"), vec![1]);
        assert_eq!(extract_issue_nums("FIXES #2"), vec![2]);
        assert_eq!(extract_issue_nums("Resolves #3"), vec![3]);
    }

    #[test]
    fn extract_issue_nums_multiple_per_line() {
        assert_eq!(extract_issue_nums("Closes #1, Closes #2"), vec![1, 2]);
    }

    #[test]
    fn extract_issue_nums_ignores_non_numeric() {
        assert!(extract_issue_nums("Closes #abc").is_empty());
        assert!(extract_issue_nums("Closes #").is_empty());
    }

    #[test]
    fn extract_issue_nums_large_numbers() {
        assert_eq!(extract_issue_nums("Closes #99999"), vec![99999]);
    }
}
