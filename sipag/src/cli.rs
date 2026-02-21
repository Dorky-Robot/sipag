use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use sipag_core::{
    config::WorkerConfig,
    executor::{self, RunConfig},
    init,
    prompt::{format_duration, generate_task_id},
    repo,
    task::{self, default_sipag_dir, FileTaskRepository, TaskId, TaskRepository, TaskStatus},
    triage,
    worker::{github::preflight_gh_auth, poll::run_worker_loop},
};
use std::fs;
use std::path::{Path, PathBuf};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(
    name = "sipag",
    version,
    about = "Sandbox launcher for Claude Code",
    long_about = "sipag spins up isolated Docker sandboxes and makes progress visible.\n\nRun with no arguments to launch the interactive TUI."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Launch the interactive TUI (default when no args given)
    Tui,

    /// Poll GitHub for approved issues and dispatch Docker workers
    Work {
        /// Repository in owner/repo format (e.g. Dorky-Robot/sipag).
        /// May be specified multiple times. Defaults to repos.conf or current git remote.
        repos: Vec<String>,

        /// Process one polling cycle and exit
        #[arg(long)]
        once: bool,
    },

    /// Signal workers to finish current batch and exit
    Drain,

    /// Clear the drain signal so workers continue polling
    Resume,

    /// Configure sipag and Claude Code permissions
    Setup,

    /// Diagnose setup problems and print fix commands
    Doctor,

    /// Prime an agile session (interactive: triage, approve, then run `sipag work`)
    Start {
        /// Repository in owner/repo format (optional; uses repos.conf if omitted)
        repo: Option<String>,
    },

    /// Conversational PR merge session
    Merge {
        /// Repository in owner/repo format (optional; inferred from git remote)
        repo: Option<String>,
    },

    /// Generate or update ARCHITECTURE.md and VISION.md via Claude
    #[command(name = "refresh-docs")]
    RefreshDocs {
        /// Repository in owner/repo format
        repo: String,

        /// Only refresh if ARCHITECTURE.md is stale
        #[arg(long)]
        check: bool,
    },

    /// Launch a Docker sandbox for a task
    Run {
        /// Repository URL to clone inside the container (required)
        #[arg(long)]
        repo: String,

        /// GitHub issue number to associate with this task
        #[arg(long)]
        issue: Option<String>,

        /// Run in background; sipag returns immediately
        #[arg(short = 'b', long)]
        background: bool,

        /// Task description
        description: String,
    },

    /// List running and recent tasks
    Ps,

    /// Print the log for a task
    Logs {
        /// Task ID
        id: String,
    },

    /// Kill a running container and move task to failed/
    Kill {
        /// Task ID
        id: String,
    },

    /// Process queue/ serially (uses sipag run internally)
    #[command(name = "queue-run")]
    QueueRun,

    /// Create ~/.sipag/{queue,running,done,failed}
    Init,

    /// Queue a task (requires --repo)
    Add {
        /// Task title/description
        title: String,

        /// Repository name (writes YAML file to queue/)
        #[arg(long, required = true)]
        repo: String,

        /// Priority level
        #[arg(long, default_value = "medium")]
        priority: String,
    },

    /// Print task file and log (searches all dirs)
    Show {
        /// Task name (without .md extension)
        name: String,
    },

    /// Move a failed task back to queue/
    Retry {
        /// Task name (without .md extension)
        name: String,
    },

    /// Manage the repo registry
    Repo {
        #[command(subcommand)]
        subcommand: RepoCommands,
    },

    /// Show queue state across all directories
    Status,

    /// Review open issues against VISION.md and recommend CLOSE/ADJUST/KEEP/MERGE
    Triage {
        /// Repository in owner/repo format (e.g. Dorky-Robot/sipag)
        repo: String,

        /// Print report only — make no changes
        #[arg(long, conflicts_with = "apply")]
        dry_run: bool,

        /// Apply all recommendations without confirmation
        #[arg(long, conflicts_with = "dry_run")]
        apply: bool,
    },

    /// Print shell completion scripts for bash, zsh, or fish
    Completions {
        /// Shell type: bash, zsh, or fish
        shell: String,
    },

    /// Print version
    Version,

    /// Internal: run Docker task in background (do not use directly)
    #[command(name = "_bg-exec", hide = true)]
    BgExec {
        #[arg(long)]
        task_id: String,
        #[arg(long)]
        repo_url: String,
        #[arg(long)]
        description: String,
        #[arg(long)]
        image: String,
        #[arg(long)]
        timeout: u64,
        #[arg(long)]
        sipag_dir: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum RepoCommands {
    /// Register a repo name → URL mapping
    Add { name: String, url: String },
    /// List registered repos
    List,
}

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        None | Some(Commands::Tui) | Some(Commands::Status) => {
            let status = std::process::Command::new("sipag-tui")
                .status()
                .with_context(|| "Failed to exec sipag-tui — is it installed?")?;
            if !status.success() {
                bail!("sipag-tui exited with status: {}", status);
            }
            Ok(())
        }
        Some(Commands::Work { repos, once }) => cmd_work(repos, once),
        Some(Commands::Drain) => cmd_drain(),
        Some(Commands::Resume) => cmd_resume(),
        Some(Commands::Setup) => cmd_shell_script("setup", &[]),
        Some(Commands::Doctor) => cmd_shell_script("doctor", &[]),
        Some(Commands::Start { repo }) => {
            let args = repo
                .as_deref()
                .map(|r| vec![r.to_string()])
                .unwrap_or_default();
            cmd_shell_script("start", &args)
        }
        Some(Commands::Merge { repo }) => {
            let args = repo
                .as_deref()
                .map(|r| vec![r.to_string()])
                .unwrap_or_default();
            cmd_shell_script("merge", &args)
        }
        Some(Commands::RefreshDocs { repo, check }) => {
            let mut args = vec![repo];
            if check {
                args.push("--check".to_string());
            }
            cmd_shell_script("refresh-docs", &args)
        }
        Some(Commands::Triage {
            repo,
            dry_run,
            apply,
        }) => triage::run_triage(&repo, dry_run, apply),
        Some(Commands::Completions { shell }) => cmd_completions(&shell),
        Some(Commands::Version) => {
            println!("sipag {VERSION}");
            Ok(())
        }
        Some(Commands::Init) => cmd_init(),
        Some(Commands::QueueRun) => cmd_queue_run(),
        Some(Commands::Run {
            repo,
            issue,
            background,
            description,
        }) => cmd_run(&repo, issue.as_deref(), background, &description),
        Some(Commands::Ps) => cmd_ps(),
        Some(Commands::Logs { id }) => cmd_logs(&id),
        Some(Commands::Kill { id }) => cmd_kill(&id),
        Some(Commands::Add {
            title,
            repo,
            priority,
        }) => cmd_add(&title, &repo, &priority),
        Some(Commands::Show { name }) => cmd_show(&name),
        Some(Commands::Retry { name }) => cmd_retry(&name),
        Some(Commands::Repo { subcommand }) => cmd_repo(subcommand),
        Some(Commands::BgExec {
            task_id,
            repo_url,
            description,
            image,
            timeout,
            sipag_dir,
        }) => executor::run_bg_exec(
            &sipag_dir,
            &task_id,
            &repo_url,
            &description,
            &image,
            timeout,
        ),
    }
}

fn sipag_dir() -> PathBuf {
    default_sipag_dir()
}

// ── New commands (previously bash-only) ──────────────────────────────────────

fn cmd_work(mut repos: Vec<String>, once: bool) -> Result<()> {
    let dir = sipag_dir();
    init::init_dirs(&dir).ok();

    let mut cfg = WorkerConfig::load(&dir)?;
    cfg.once = once;

    // Preflight checks.
    sipag_core::auth::preflight_auth(&dir)?;
    sipag_core::docker::preflight_docker_running()?;

    // Auto-pull image if not present.
    let image_check = std::process::Command::new("docker")
        .args(["image", "inspect", &cfg.image])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if !image_check.map(|s| s.success()).unwrap_or(false) {
        println!("Worker image '{}' not found — pulling...", cfg.image);
        let pull = std::process::Command::new("docker")
            .args(["pull", &cfg.image])
            .status();
        if !pull.map(|s| s.success()).unwrap_or(false) {
            bail!(
                "Error: Could not pull '{}'. Run 'sipag setup' to configure.",
                cfg.image
            );
        }
    }

    preflight_gh_auth()?;

    // Clear stale drain signal.
    let drain_file = dir.join("drain");
    if drain_file.exists() {
        println!("Warning: stale drain signal found. Clearing it and starting normally.");
        println!("Use 'sipag drain' to signal a graceful shutdown.");
        fs::remove_file(&drain_file).ok();
    }

    // Resolve repos list.
    if repos.is_empty() {
        repos = resolve_repos(&dir)?;
    }

    run_worker_loop(&repos, &dir, cfg)
}

fn cmd_drain() -> Result<()> {
    let dir = sipag_dir();
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("drain"), "")?;
    println!("Drain signal sent. Running workers will finish their current batch and exit.");
    println!("Use 'sipag resume' to cancel.");
    Ok(())
}

fn cmd_resume() -> Result<()> {
    let drain_file = sipag_dir().join("drain");
    fs::remove_file(&drain_file).ok();
    println!("Drain signal cleared. Workers will continue polling.");
    Ok(())
}

/// Run a bash script from the sipag lib/ installation (for commands that still
/// shell out: setup, doctor, start, merge, refresh-docs).
///
/// Resolution order for the lib/ directory:
///   1. `SIPAG_ROOT` environment variable → `$SIPAG_ROOT/lib/`
///   2. `~/.sipag/share/lib/` (Makefile install location)
///   3. Next to the binary: `<exe-dir>/../lib/` etc.
fn cmd_shell_script(script_name: &str, args: &[String]) -> Result<()> {
    let lib_dir = find_lib_dir()?;

    // Map logical names to (file, entry-function).
    let (script_file, func_name) = match script_name {
        "setup" => ("setup.sh", "setup_run"),
        "doctor" => ("doctor.sh", "doctor_run"),
        "start" => ("start.sh", "start_run_wrapper"),
        "merge" => ("merge.sh", "merge_run"),
        "refresh-docs" => ("refresh-docs.sh", "refresh_docs_run"),
        _ => bail!("Unknown shell script: {script_name}"),
    };

    let script_path = lib_dir.join(script_file);
    if !script_path.exists() {
        bail!(
            "Script not found: {}\n\nRun 'make install' or set SIPAG_ROOT to the sipag checkout root.",
            script_path.display()
        );
    }

    // Source the script and call the entry function, forwarding any args.
    let inline = format!(
        "source {} && {func_name} \"$@\"",
        shell_quote(script_path.to_string_lossy().as_ref())
    );

    let mut bash_args = vec!["-c".to_string(), inline, "--".to_string()];
    bash_args.extend_from_slice(args);

    let status = std::process::Command::new("bash")
        .args(&bash_args)
        .status()
        .with_context(|| format!("Failed to run bash script: {script_file}"))?;

    if !status.success() {
        bail!("{script_name} exited with status: {status}");
    }
    Ok(())
}

fn find_lib_dir() -> Result<PathBuf> {
    // 1. SIPAG_ROOT env var.
    if let Ok(root) = std::env::var("SIPAG_ROOT") {
        let lib = PathBuf::from(root).join("lib");
        if lib.exists() {
            return Ok(lib);
        }
    }

    // 2. ~/.sipag/share/lib (Makefile install location).
    if let Some(home) = dirs_home() {
        let lib = home.join(".sipag").join("share").join("lib");
        if lib.exists() {
            return Ok(lib);
        }
    }

    // 3. Relative to the running binary — walk ancestors looking for lib/setup.sh.
    if let Ok(exe) = std::env::current_exe() {
        for ancestor in exe.ancestors().skip(1).take(5) {
            let lib = ancestor.join("lib");
            if lib.join("setup.sh").exists() {
                return Ok(lib);
            }
        }
    }

    bail!(
        "Could not find sipag lib/ directory.\n\n\
         Run 'make install' or set SIPAG_ROOT to the sipag checkout root."
    )
}

/// Minimal shell quoting: wraps path in single quotes, escaping any embedded
/// single quotes.  Sufficient for file-system paths.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

/// Resolve the list of repos from repos.conf or the current git remote.
fn resolve_repos(sipag_dir: &Path) -> Result<Vec<String>> {
    let conf = sipag_dir.join("repos.conf");
    if conf.exists() {
        if let Ok(contents) = fs::read_to_string(&conf) {
            let repos: Vec<String> = contents
                .lines()
                .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
                .filter_map(|l| l.split_once('=').map(|(_, v)| v.trim().to_string()))
                .map(|url| {
                    let url = url.trim_end_matches(".git").to_string();
                    url.strip_prefix("https://github.com/")
                        .unwrap_or(&url)
                        .to_string()
                })
                .filter(|u| !u.is_empty())
                .collect();
            if !repos.is_empty() {
                return Ok(repos);
            }
        }
    }

    // Fall back to current git remote.
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output();
    if let Ok(o) = output {
        if o.status.success() {
            let url = String::from_utf8_lossy(&o.stdout).trim().to_string();
            let url = url.trim_end_matches(".git").to_string();
            let repo = url
                .strip_prefix("https://github.com/")
                .or_else(|| url.strip_prefix("git@github.com:"))
                .unwrap_or(&url)
                .to_string();
            if !repo.is_empty() {
                return Ok(vec![repo]);
            }
        }
    }

    bail!(
        "Error: Not in a git repo and no repos registered.\n\
         Run from a git repo, or: sipag repo add <name> <url>"
    )
}

// ── Existing commands (unchanged from sipag-cli) ──────────────────────────────

fn cmd_init() -> Result<()> {
    init::init_dirs(&sipag_dir())
}

/// Process the queue/ directory serially.
///
/// Renamed from `start` to `queue-run` to avoid clashing with the
/// agile-session-primer `sipag start [<repo>]` (which shells out to
/// lib/start.sh).  The queue-based workflow is used in the offline / file-
/// based task runner, not the GitHub issue workflow.
fn cmd_queue_run() -> Result<()> {
    let dir = sipag_dir();
    init::init_dirs(&dir).ok();
    println!("sipag executor starting (queue: {}/queue)", dir.display());

    let queue_dir = dir.join("queue");
    let failed_dir = dir.join("failed");
    let worker_cfg = WorkerConfig::load(&dir)?;
    let timeout = worker_cfg.timeout.as_secs();

    let repo = FileTaskRepository::new(dir.clone());
    let mut processed = 0;

    loop {
        // Pick the first .md file from queue, sorted alphabetically
        let mut paths: Vec<_> = fs::read_dir(&queue_dir)
            .with_context(|| format!("Failed to read {}", queue_dir.display()))?
            .flatten()
            .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
            .map(|e| e.path())
            .collect();
        paths.sort();

        let task_file = match paths.into_iter().next() {
            Some(p) => p,
            None => {
                if processed == 0 {
                    println!("No tasks in queue — use 'sipag add' to queue a task");
                } else {
                    println!("Queue empty — processed {processed} task(s)");
                }
                break;
            }
        };

        let task_name = task_file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let task_file_data = match task::read_task_file(&task_file, TaskStatus::Queue) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("Error: failed to parse task file: {e}");
                let _ = fs::rename(&task_file, failed_dir.join(format!("{task_name}.md")));
                println!("==> Failed: {task_name}");
                processed += 1;
                continue;
            }
        };

        let repo_name = task_file_data.repo.as_deref().unwrap_or("");
        let url = match repo::get_repo_url(&dir, repo_name) {
            Ok(u) => u,
            Err(e) => {
                eprintln!("Error: {e}");
                let _ = fs::rename(&task_file, failed_dir.join(format!("{task_name}.md")));
                println!("==> Failed: {task_name}");
                processed += 1;
                continue;
            }
        };

        // Extract issue number from source (e.g. "github#142" → "142")
        let issue_num = task_file_data
            .source
            .as_deref()
            .and_then(|s| s.split('#').next_back())
            .filter(|s| s.chars().all(|c| c.is_ascii_digit()))
            .map(|s| s.to_string());

        // Transition Queue → Running via repository.
        let task_id = TaskId::new(&task_name);
        let mut domain_task = match repo.get(&task_id) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("Error: failed to load task: {e}");
                processed += 1;
                continue;
            }
        };
        if let Err(e) = repo.transition(&mut domain_task, TaskStatus::Running, chrono::Utc::now()) {
            eprintln!("Error: failed to start task: {e}");
            processed += 1;
            continue;
        }

        let _ = executor::run_impl(
            &dir,
            RunConfig {
                task_id: &task_name,
                repo_url: &url,
                description: &task_file_data.title,
                issue: issue_num.as_deref(),
                background: false,
                image: &worker_cfg.image,
                timeout_secs: timeout,
            },
        );

        processed += 1;
    }

    Ok(())
}

fn cmd_run(repo_url: &str, issue: Option<&str>, background: bool, description: &str) -> Result<()> {
    let dir = sipag_dir();
    init::init_dirs(&dir).ok();

    let task_id = generate_task_id(description, chrono::Utc::now());
    println!("Task ID: {task_id}");

    let worker_cfg = WorkerConfig::load(&dir)?;

    executor::run_impl(
        &dir,
        RunConfig {
            task_id: &task_id,
            repo_url,
            description,
            issue,
            background,
            image: &worker_cfg.image,
            timeout_secs: worker_cfg.timeout.as_secs(),
        },
    )
}

fn cmd_ps() -> Result<()> {
    let dir = sipag_dir();
    let now = chrono::Utc::now();

    println!("{:<44}  {:<8}  {:<10}  REPO", "ID", "STATUS", "DURATION");

    let mut found = false;
    for status_dir in &["running", "done", "failed"] {
        let d = dir.join(status_dir);
        if !d.exists() {
            continue;
        }
        let mut paths: Vec<_> = fs::read_dir(&d)
            .unwrap_or_else(|_| std::fs::read_dir("/dev/null").unwrap())
            .flatten()
            .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
            .map(|e| e.path())
            .collect();
        paths.sort_by(|a, b| b.cmp(a)); // newest first

        for path in paths {
            let task = match task::read_task_file(&path, TaskStatus::Queue) {
                Ok(t) => t,
                Err(_) => continue,
            };

            let duration = compute_duration(&task, &now);
            let repo_short = task
                .repo
                .as_deref()
                .and_then(|r| r.split('/').next_back())
                .unwrap_or("unknown");

            println!(
                "{:<44}  {:<8}  {:<10}  {}",
                &task.name[..task.name.len().min(44)],
                status_dir,
                duration,
                repo_short
            );
            found = true;
        }
    }

    if !found {
        println!("No tasks found.");
    }

    Ok(())
}

fn compute_duration(task: &task::TaskFile, now: &chrono::DateTime<chrono::Utc>) -> String {
    use chrono::DateTime;

    let started = task
        .started
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc));

    let ended = task
        .ended
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc));

    match started {
        None => "-".to_string(),
        Some(start) => {
            let end = ended.unwrap_or(*now);
            let secs = (end - start).num_seconds();
            format_duration(secs)
        }
    }
}

fn cmd_logs(task_id: &str) -> Result<()> {
    let dir = sipag_dir();
    for status_dir in &["running", "done", "failed"] {
        let log_file = dir.join(status_dir).join(format!("{task_id}.log"));
        if log_file.exists() {
            print!("{}", fs::read_to_string(&log_file)?);
            return Ok(());
        }
    }
    bail!("Error: no log found for task '{task_id}'")
}

fn cmd_kill(task_id: &str) -> Result<()> {
    let dir = sipag_dir();
    let tracking_file = dir.join("running").join(format!("{task_id}.md"));
    if !tracking_file.exists() {
        bail!("Error: task '{}' not found in running/", task_id);
    }

    let container_name = format!("sipag-{task_id}");
    // Kill the container (ignore errors if already stopped)
    let _ = std::process::Command::new("docker")
        .args(["kill", &container_name])
        .output();

    // Transition Running → Failed via repository.
    let repo = FileTaskRepository::new(dir.clone());
    let id = TaskId::new(task_id);
    let mut task = repo.get(&id)?;
    repo.transition(&mut task, TaskStatus::Failed, chrono::Utc::now())?;

    println!("Killed: {task_id}");
    Ok(())
}

fn cmd_add(title: &str, repo: &str, priority: &str) -> Result<()> {
    if title.is_empty() {
        bail!("Usage: sipag add \"task text\" --repo <name> [--priority <level>]");
    }

    let dir = sipag_dir();
    if !dir.join("queue").exists() {
        init::init_dirs(&dir).ok();
    }
    let filename = task::next_filename(&dir.join("queue"), title);
    let path = dir.join("queue").join(&filename);
    let added = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    task::write_task_file(&path, title, repo, priority, None, &added)?;
    println!("Added: {title}");

    Ok(())
}

fn cmd_show(name: &str) -> Result<()> {
    let dir = sipag_dir();
    let mut found_file = None;
    let mut found_status = "";

    for status in &["queue", "running", "done", "failed"] {
        let candidate = dir.join(status).join(format!("{name}.md"));
        if candidate.exists() {
            found_file = Some(candidate);
            found_status = status;
            break;
        }
    }

    let found = found_file.ok_or_else(|| anyhow::anyhow!("task '{}' not found", name))?;
    println!("=== Task: {name} ===");
    println!("Status: {found_status}");
    print!("{}", fs::read_to_string(&found)?);

    let log_file = dir.join(found_status).join(format!("{name}.log"));
    if log_file.exists() {
        println!("=== Log ===");
        print!("{}", fs::read_to_string(&log_file)?);
    }

    Ok(())
}

fn cmd_retry(name: &str) -> Result<()> {
    let dir = sipag_dir();
    let failed_file = dir.join("failed").join(format!("{name}.md"));
    let failed_log = dir.join("failed").join(format!("{name}.log"));

    if !failed_file.exists() {
        bail!("Error: task '{}' not found in failed/", name);
    }

    // Delete the log before transitioning so the retry starts clean.
    if failed_log.exists() {
        let _ = fs::remove_file(&failed_log);
    }

    // Transition Failed → Queued.
    let repo = FileTaskRepository::new(dir.clone());
    let id = TaskId::new(name);
    let mut task = repo.get(&id)?;
    repo.transition(&mut task, TaskStatus::Queue, chrono::Utc::now())?;

    println!("Retrying: {name} (moved to queue)");
    Ok(())
}

fn cmd_repo(subcommand: RepoCommands) -> Result<()> {
    let dir = sipag_dir();
    match subcommand {
        RepoCommands::Add { name, url } => {
            repo::add_repo(&dir, &name, &url)?;
            println!("Registered: {name}={url}");
            Ok(())
        }
        RepoCommands::List => {
            let repos = repo::list_repos(&dir)?;
            if repos.is_empty() {
                println!("No repos registered. Use: sipag repo add <name> <url>");
            } else {
                for (name, url) in repos {
                    println!("{name}={url}");
                }
            }
            Ok(())
        }
    }
}

fn cmd_completions(shell: &str) -> Result<()> {
    let script = match shell {
        "bash" => crate::completions::BASH,
        "zsh" => crate::completions::ZSH,
        "fish" => crate::completions::FISH,
        _ => bail!("Unknown shell '{shell}'. Use: bash, zsh, or fish"),
    };
    print!("{script}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    // ── shell_quote ──────────────────────────────────────────────────────────

    #[test]
    fn shell_quote_simple_path() {
        assert_eq!(shell_quote("/usr/bin/foo"), "'/usr/bin/foo'");
    }

    #[test]
    fn shell_quote_path_with_spaces() {
        assert_eq!(shell_quote("/home/user/my stuff"), "'/home/user/my stuff'");
    }

    #[test]
    fn shell_quote_path_with_single_quotes() {
        assert_eq!(
            shell_quote("/home/user/it's here"),
            "'/home/user/it'\\''s here'"
        );
    }

    // ── compute_duration ─────────────────────────────────────────────────────

    fn make_task_file(started: Option<&str>, ended: Option<&str>) -> sipag_core::task::TaskFile {
        sipag_core::task::TaskFile {
            name: "test".to_string(),
            title: "test".to_string(),
            body: String::new(),
            repo: None,
            priority: "medium".to_string(),
            source: None,
            added: None,
            started: started.map(String::from),
            ended: ended.map(String::from),
            container: None,
            issue: None,
            status: sipag_core::task::TaskStatus::Queue,
            file_path: std::path::PathBuf::new(),
        }
    }

    #[test]
    fn compute_duration_with_start_and_end() {
        let task = make_task_file(Some("2024-01-01T00:00:00Z"), Some("2024-01-01T00:01:30Z"));
        let now = chrono::Utc::now();
        assert_eq!(compute_duration(&task, &now), "1m30s");
    }

    #[test]
    fn compute_duration_no_start() {
        let task = make_task_file(None, None);
        let now = chrono::Utc::now();
        assert_eq!(compute_duration(&task, &now), "-");
    }

    // ── resolve_repos ────────────────────────────────────────────────────────

    #[test]
    fn resolve_repos_from_repos_conf() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("repos.conf"),
            "sipag=https://github.com/Dorky-Robot/sipag.git\n\
             other=https://github.com/Dorky-Robot/other\n",
        )
        .unwrap();

        let repos = resolve_repos(dir.path()).unwrap();
        assert_eq!(repos, vec!["Dorky-Robot/sipag", "Dorky-Robot/other"]);
    }

    #[test]
    fn resolve_repos_skips_comments_and_blanks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("repos.conf"),
            "# A comment\n\n  # another\nsipag=https://github.com/Dorky-Robot/sipag\n",
        )
        .unwrap();

        let repos = resolve_repos(dir.path()).unwrap();
        assert_eq!(repos, vec!["Dorky-Robot/sipag"]);
    }

    #[test]
    fn resolve_repos_empty_conf_falls_through_to_git() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("repos.conf"), "# nothing useful\n").unwrap();
        // repos.conf has no entries, so resolve_repos falls through to
        // `git remote get-url origin`. In CI or a git repo this may succeed
        // or fail — we just verify it doesn't panic.
        let _ = resolve_repos(dir.path());
    }

    // ── clap parsing ─────────────────────────────────────────────────────────
    // These tests verify that every subcommand parses correctly.
    // This is the kind of test that would have caught the `sipag status`
    // regression — if the route is wrong, the variant won't match.

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).unwrap()
    }

    #[test]
    fn parse_no_args_is_none() {
        let cli = parse(&["sipag"]);
        assert!(cli.command.is_none());
    }

    #[test]
    fn parse_tui() {
        let cli = parse(&["sipag", "tui"]);
        assert!(matches!(cli.command, Some(Commands::Tui)));
    }

    #[test]
    fn parse_status() {
        let cli = parse(&["sipag", "status"]);
        assert!(matches!(cli.command, Some(Commands::Status)));
    }

    #[test]
    fn parse_version() {
        let cli = parse(&["sipag", "version"]);
        assert!(matches!(cli.command, Some(Commands::Version)));
    }

    #[test]
    fn parse_init() {
        let cli = parse(&["sipag", "init"]);
        assert!(matches!(cli.command, Some(Commands::Init)));
    }

    #[test]
    fn parse_ps() {
        let cli = parse(&["sipag", "ps"]);
        assert!(matches!(cli.command, Some(Commands::Ps)));
    }

    #[test]
    fn parse_drain() {
        let cli = parse(&["sipag", "drain"]);
        assert!(matches!(cli.command, Some(Commands::Drain)));
    }

    #[test]
    fn parse_resume() {
        let cli = parse(&["sipag", "resume"]);
        assert!(matches!(cli.command, Some(Commands::Resume)));
    }

    #[test]
    fn parse_setup() {
        let cli = parse(&["sipag", "setup"]);
        assert!(matches!(cli.command, Some(Commands::Setup)));
    }

    #[test]
    fn parse_doctor() {
        let cli = parse(&["sipag", "doctor"]);
        assert!(matches!(cli.command, Some(Commands::Doctor)));
    }

    #[test]
    fn parse_work_with_repos() {
        let cli = parse(&["sipag", "work", "Dorky-Robot/sipag", "other/repo"]);
        match cli.command {
            Some(Commands::Work { repos, once }) => {
                assert_eq!(repos, vec!["Dorky-Robot/sipag", "other/repo"]);
                assert!(!once);
            }
            other => panic!("Expected Work, got {other:?}"),
        }
    }

    #[test]
    fn parse_work_once() {
        let cli = parse(&["sipag", "work", "--once", "Dorky-Robot/sipag"]);
        match cli.command {
            Some(Commands::Work { once, .. }) => assert!(once),
            other => panic!("Expected Work, got {other:?}"),
        }
    }

    #[test]
    fn parse_run() {
        let cli = parse(&[
            "sipag",
            "run",
            "--repo",
            "https://github.com/test/repo",
            "-b",
            "Fix the bug",
        ]);
        match cli.command {
            Some(Commands::Run {
                repo,
                background,
                description,
                issue,
            }) => {
                assert_eq!(repo, "https://github.com/test/repo");
                assert!(background);
                assert_eq!(description, "Fix the bug");
                assert!(issue.is_none());
            }
            other => panic!("Expected Run, got {other:?}"),
        }
    }

    #[test]
    fn parse_run_with_issue() {
        let cli = parse(&[
            "sipag",
            "run",
            "--repo",
            "https://github.com/test/repo",
            "--issue",
            "42",
            "Fix the bug",
        ]);
        match cli.command {
            Some(Commands::Run { issue, .. }) => {
                assert_eq!(issue, Some("42".to_string()));
            }
            other => panic!("Expected Run, got {other:?}"),
        }
    }

    #[test]
    fn parse_logs() {
        let cli = parse(&["sipag", "logs", "task-123"]);
        match cli.command {
            Some(Commands::Logs { id }) => assert_eq!(id, "task-123"),
            other => panic!("Expected Logs, got {other:?}"),
        }
    }

    #[test]
    fn parse_kill() {
        let cli = parse(&["sipag", "kill", "task-123"]);
        match cli.command {
            Some(Commands::Kill { id }) => assert_eq!(id, "task-123"),
            other => panic!("Expected Kill, got {other:?}"),
        }
    }

    #[test]
    fn parse_add() {
        let cli = parse(&[
            "sipag",
            "add",
            "my task",
            "--repo",
            "sipag",
            "--priority",
            "high",
        ]);
        match cli.command {
            Some(Commands::Add {
                title,
                repo,
                priority,
            }) => {
                assert_eq!(title, "my task");
                assert_eq!(repo, "sipag");
                assert_eq!(priority, "high");
            }
            other => panic!("Expected Add, got {other:?}"),
        }
    }

    #[test]
    fn parse_add_default_priority() {
        let cli = parse(&["sipag", "add", "my task", "--repo", "sipag"]);
        match cli.command {
            Some(Commands::Add { priority, .. }) => assert_eq!(priority, "medium"),
            other => panic!("Expected Add, got {other:?}"),
        }
    }

    #[test]
    fn parse_show() {
        let cli = parse(&["sipag", "show", "task-name"]);
        match cli.command {
            Some(Commands::Show { name }) => assert_eq!(name, "task-name"),
            other => panic!("Expected Show, got {other:?}"),
        }
    }

    #[test]
    fn parse_retry() {
        let cli = parse(&["sipag", "retry", "task-name"]);
        match cli.command {
            Some(Commands::Retry { name }) => assert_eq!(name, "task-name"),
            other => panic!("Expected Retry, got {other:?}"),
        }
    }

    #[test]
    fn parse_repo_add() {
        let cli = parse(&[
            "sipag",
            "repo",
            "add",
            "myrepo",
            "https://github.com/test/repo",
        ]);
        match cli.command {
            Some(Commands::Repo {
                subcommand: RepoCommands::Add { name, url },
            }) => {
                assert_eq!(name, "myrepo");
                assert_eq!(url, "https://github.com/test/repo");
            }
            other => panic!("Expected Repo Add, got {other:?}"),
        }
    }

    #[test]
    fn parse_repo_list() {
        let cli = parse(&["sipag", "repo", "list"]);
        assert!(matches!(
            cli.command,
            Some(Commands::Repo {
                subcommand: RepoCommands::List
            })
        ));
    }

    #[test]
    fn parse_start_with_repo() {
        let cli = parse(&["sipag", "start", "Dorky-Robot/sipag"]);
        match cli.command {
            Some(Commands::Start { repo }) => {
                assert_eq!(repo, Some("Dorky-Robot/sipag".to_string()));
            }
            other => panic!("Expected Start, got {other:?}"),
        }
    }

    #[test]
    fn parse_start_no_repo() {
        let cli = parse(&["sipag", "start"]);
        match cli.command {
            Some(Commands::Start { repo }) => assert!(repo.is_none()),
            other => panic!("Expected Start, got {other:?}"),
        }
    }

    #[test]
    fn parse_merge() {
        let cli = parse(&["sipag", "merge", "Dorky-Robot/sipag"]);
        match cli.command {
            Some(Commands::Merge { repo }) => {
                assert_eq!(repo, Some("Dorky-Robot/sipag".to_string()));
            }
            other => panic!("Expected Merge, got {other:?}"),
        }
    }

    #[test]
    fn parse_refresh_docs() {
        let cli = parse(&["sipag", "refresh-docs", "Dorky-Robot/sipag", "--check"]);
        match cli.command {
            Some(Commands::RefreshDocs { repo, check }) => {
                assert_eq!(repo, "Dorky-Robot/sipag");
                assert!(check);
            }
            other => panic!("Expected RefreshDocs, got {other:?}"),
        }
    }

    #[test]
    fn parse_triage() {
        let cli = parse(&["sipag", "triage", "Dorky-Robot/sipag", "--dry-run"]);
        match cli.command {
            Some(Commands::Triage {
                repo,
                dry_run,
                apply,
            }) => {
                assert_eq!(repo, "Dorky-Robot/sipag");
                assert!(dry_run);
                assert!(!apply);
            }
            other => panic!("Expected Triage, got {other:?}"),
        }
    }

    #[test]
    fn parse_completions() {
        let cli = parse(&["sipag", "completions", "zsh"]);
        match cli.command {
            Some(Commands::Completions { shell }) => assert_eq!(shell, "zsh"),
            other => panic!("Expected Completions, got {other:?}"),
        }
    }

    #[test]
    fn parse_queue_run() {
        let cli = parse(&["sipag", "queue-run"]);
        assert!(matches!(cli.command, Some(Commands::QueueRun)));
    }

    // ── routing tests ────────────────────────────────────────────────────────
    // Verify that the match arms in run() map to the expected commands.
    // We test routes that DON'T need Docker, GitHub, or filesystem side effects.

    #[test]
    fn route_version_prints_version() {
        let cli = parse(&["sipag", "version"]);
        // run() should succeed without side effects
        assert!(run(cli).is_ok());
    }

    #[test]
    fn route_init_creates_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("SIPAG_DIR", dir.path());
        let cli = parse(&["sipag", "init"]);
        let result = run(cli);
        std::env::remove_var("SIPAG_DIR");
        assert!(result.is_ok());
    }

    // Verify that None, Tui, and Status all route to the same TUI exec path.
    // We can't actually run sipag-tui in tests, but we verify the match hits
    // the same branch by checking all three produce the same error when
    // sipag-tui is absent from PATH.
    #[test]
    fn route_status_tui_and_none_all_exec_tui() {
        // Temporarily override PATH so sipag-tui is not found
        let original_path = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", "/nonexistent");

        let err_none = run(parse(&["sipag"])).unwrap_err();
        let err_tui = run(parse(&["sipag", "tui"])).unwrap_err();
        let err_status = run(parse(&["sipag", "status"])).unwrap_err();

        std::env::set_var("PATH", &original_path);

        // All three should fail with the same sipag-tui error
        let msg_none = format!("{err_none:#}");
        let msg_tui = format!("{err_tui:#}");
        let msg_status = format!("{err_status:#}");

        assert!(
            msg_none.contains("sipag-tui"),
            "None route should exec sipag-tui: {msg_none}"
        );
        assert!(
            msg_tui.contains("sipag-tui"),
            "Tui route should exec sipag-tui: {msg_tui}"
        );
        assert!(
            msg_status.contains("sipag-tui"),
            "Status route should exec sipag-tui: {msg_status}"
        );
    }
}
