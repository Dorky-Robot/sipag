use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use sipag_core::{
    board,
    config::{default_sipag_dir, validate_config_file_for_doctor, ConfigEntryStatus, WorkerConfig},
    docker, init, katulong,
    state::{self, format_duration},
    worker::{dispatch, github, lifecycle},
};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Command;

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
    /// Dispatch a task (by ID) or a Docker worker (by PR URL)
    Dispatch {
        /// Task ID (e.g. 42) or PR URL (https://github.com/owner/repo/pull/42)
        #[arg(value_name = "TARGET")]
        target: String,

        /// Project name (for task dispatch, default: from config)
        #[arg(short, long)]
        project: Option<String>,

        /// Role override (default: from task or 'dev')
        #[arg(short, long)]
        role: Option<String>,
    },

    /// Spin up all configured role sessions for a project
    Up {
        /// Project name (default: from config)
        project: Option<String>,
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

    // ── v4 board commands ────────────────────────────────────────────────────
    /// Add a task to the board
    Add {
        /// Task title
        title: String,

        /// Project name (default: from config)
        #[arg(short, long)]
        project: Option<String>,

        /// Labels (comma-separated or repeated)
        #[arg(short, long, value_delimiter = ',')]
        label: Vec<String>,

        /// Role for dispatch (default: dev)
        #[arg(short, long)]
        role: Option<String>,
    },

    /// List tasks on the board
    List {
        /// Project name (default: from config)
        #[arg(short, long)]
        project: Option<String>,

        /// Filter by status
        #[arg(long)]
        status: Option<String>,
    },

    /// Move a task to a new status
    Move {
        /// Task ID
        id: u64,

        /// New status (e.g. in-progress, review, done)
        status: String,

        /// Project name (default: from config)
        #[arg(short, long)]
        project: Option<String>,
    },

    /// List all projects
    Projects,

    /// Manage projects
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },

    /// Subscribe to katulong pub/sub topic and print events
    Sub {
        /// Pub/sub topic (e.g. crew/katulong/dev/agent-done)
        topic: String,

        /// Replay from sequence number
        #[arg(long, default_value = "0")]
        from_seq: u64,

        /// Output as JSON (one event per line)
        #[arg(long, default_value_t = false)]
        json: bool,
    },

    /// Check system prerequisites
    Doctor,

    /// Print version
    Version,
}

#[derive(Debug, Subcommand)]
pub enum ProjectAction {
    /// Add a new project
    Add {
        /// Project name
        name: String,

        /// GitHub repo (owner/repo)
        #[arg(long)]
        repo: String,
    },
}

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        None => run_tui(),
        Some(Commands::Tui) => run_tui(),
        Some(Commands::Dispatch {
            target,
            project,
            role,
        }) => {
            // Detect: pure number = task ID, URL = legacy PR dispatch.
            if target.parse::<u64>().is_ok() {
                let task_id: u64 = target.parse().unwrap();
                run_dispatch_task(task_id, project.as_deref(), role.as_deref())
            } else {
                let (repo, pr) = parse_pr_url(&target)?;
                run_dispatch_pr(&repo, pr)
            }
        }
        Some(Commands::Up { project }) => run_up(project.as_deref()),
        Some(Commands::Ps { all }) => run_ps(all),
        Some(Commands::Logs { id }) => run_logs(&id),
        Some(Commands::Kill { id }) => run_kill(&id),
        Some(Commands::Add {
            title,
            project,
            label,
            role,
        }) => run_add(&title, project.as_deref(), &label, role.as_deref()),
        Some(Commands::List { project, status }) => run_list(project.as_deref(), status.as_deref()),
        Some(Commands::Move {
            id,
            status,
            project,
        }) => run_move(id, &status, project.as_deref()),
        Some(Commands::Projects) => run_projects(),
        Some(Commands::Project { action }) => match action {
            ProjectAction::Add { name, repo } => run_project_add(&name, &repo),
        },
        Some(Commands::Sub {
            topic,
            from_seq,
            json,
        }) => run_sub(&topic, from_seq, json),
        Some(Commands::Doctor) => run_doctor(),
        Some(Commands::Version) => run_version(),
    }
}

/// Parse a GitHub PR URL into (owner/repo, pr_number).
/// Accepts: https://github.com/owner/repo/pull/42
/// Also accepts extra path segments (e.g. /pull/42/files) so URLs copied
/// from GitHub's web UI tabs work without modification.
fn parse_pr_url(url: &str) -> Result<(String, u64)> {
    let url = url.trim().trim_end_matches('/');
    let parts: Vec<&str> = url.split('/').collect();
    // Expected: ["https:", "", "github.com", "owner", "repo", "pull", "42"]
    if parts.len() >= 7 && parts[5] == "pull" {
        let owner = parts[3];
        let repo = parts[4];
        if owner.is_empty() || repo.is_empty() {
            anyhow::bail!(
                "Not a valid PR URL: {url}\nExpected: https://github.com/owner/repo/pull/N"
            );
        }
        let pr_num: u64 = parts[6]
            .parse()
            .with_context(|| format!("invalid PR number in URL: {}", parts[6]))?;
        Ok((format!("{owner}/{repo}"), pr_num))
    } else {
        anyhow::bail!("Not a valid PR URL: {url}\nExpected: https://github.com/owner/repo/pull/N")
    }
}

fn run_dispatch_pr(repo: &str, pr_num: u64) -> Result<()> {
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

// ── v4 dispatch (task-based via katulong API) ───────────────────────────────

fn run_dispatch_task(
    task_id: u64,
    project: Option<&str>,
    role_override: Option<&str>,
) -> Result<()> {
    let sipag_dir = default_sipag_dir();
    let project_name = resolve_project(project)?;

    // Load task.
    let task = board::Task::load(&sipag_dir, &project_name, task_id)
        .with_context(|| format!("task #{task_id} not found in project {project_name}"))?;

    // Determine role: override > task.role > "dev".
    let role_name = role_override.unwrap_or(&task.role);

    // Load role template.
    let role = board::Role::load(&sipag_dir, &project_name, role_name)
        .with_context(|| {
            format!("role '{role_name}' not found in project {project_name}. Create it at ~/.sipag/projects/{project_name}/roles/{role_name}.toml")
        })?;

    // Connect to katulong.
    let client = katulong::KatulongClient::from_remote_json()
        .context("Cannot connect to katulong — is ~/.katulong/remote.json configured?")?;

    let session = katulong::session_name(&project_name, role_name);

    // 1. Create session (idempotent find-or-create).
    println!("Creating session {session}...");
    client.create_session(&session)?;

    // 2. If role uses worktrees, set one up for this task.
    if role.worktree {
        let wt_cmd = katulong::worktree_command(&project_name, task_id);
        println!("Creating worktree for task #{task_id}...");
        client.exec_session(&session, &wt_cmd)?;
    }

    // 3. Exec the agent command.
    let agent_cmd = katulong::agent_command(
        &project_name,
        task_id,
        &task.title,
        &role.command,
        role.worktree,
    );
    println!("Launching agent for task #{task_id}: {}", task.title);
    client.exec_session(&session, &agent_cmd)?;

    // 4. Move task to in-progress.
    board::move_task(&sipag_dir, &project_name, task_id, "in-progress")?;

    // 5. Confirmation.
    println!();
    println!("Dispatched task #{task_id} to session {session}");
    println!("  Project:  {project_name}");
    println!("  Role:     {role_name}");
    println!("  Command:  {}", role.command);
    if role.worktree {
        println!(
            "  Worktree: {}",
            katulong::worktree_path(&project_name, task_id)
        );
    }
    println!();
    println!("Monitor at: {} (session: {session})", client.url());

    Ok(())
}

fn run_up(project: Option<&str>) -> Result<()> {
    let sipag_dir = default_sipag_dir();
    let project_name = resolve_project(project)?;

    // Load roles.
    let roles = board::list_roles(&sipag_dir, &project_name)?;
    if roles.is_empty() {
        println!(
            "No roles configured for {project_name}. Add them at ~/.sipag/projects/{project_name}/roles/"
        );
        return Ok(());
    }

    // Connect to katulong.
    let client = katulong::KatulongClient::from_remote_json()
        .context("Cannot connect to katulong — is ~/.katulong/remote.json configured?")?;

    println!("Bringing up sessions for {project_name}...\n");

    for role in &roles {
        let session = katulong::session_name(&project_name, &role.name);
        match client.create_session(&session) {
            Ok(()) => println!("  {session} — created"),
            Err(e) => println!("  {session} — FAILED: {e}"),
        }
    }

    println!("\n{} sessions for {project_name}", roles.len());
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

// ── v4 board command handlers ─────────────────────────────────────────────

/// Resolve the project name: explicit flag > config default > error.
fn resolve_project(explicit: Option<&str>) -> Result<String> {
    if let Some(p) = explicit {
        return Ok(p.to_string());
    }
    let sipag_dir = default_sipag_dir();
    let cfg = board::BoardConfig::load(&sipag_dir)?;
    if let Some(p) = cfg.default_project {
        return Ok(p);
    }
    // If there's exactly one project, use it.
    let projects = board::list_project_names(&sipag_dir)?;
    if projects.len() == 1 {
        return Ok(projects.into_iter().next().unwrap());
    }
    anyhow::bail!("No project specified. Use -p <project> or set default_project in config.toml")
}

fn run_add(
    title: &str,
    project: Option<&str>,
    labels: &[String],
    role: Option<&str>,
) -> Result<()> {
    let sipag_dir = default_sipag_dir();
    let project = resolve_project(project)?;
    let task = board::add_task(&sipag_dir, &project, title, role, labels)?;
    println!("#{} added to {} [{}]", task.id, project, task.status);
    Ok(())
}

fn run_list(project: Option<&str>, status: Option<&str>) -> Result<()> {
    let sipag_dir = default_sipag_dir();
    let project = resolve_project(project)?;
    let tasks = board::list_tasks(&sipag_dir, &project, status)?;

    if tasks.is_empty() {
        if let Some(s) = status {
            println!("No {s} tasks in {project}.");
        } else {
            println!("No tasks in {project}.");
        }
        return Ok(());
    }

    println!("{:<6} {:<40} {:<14} {:<8}", "ID", "TITLE", "STATUS", "ROLE");
    println!("{}", "-".repeat(70));
    for t in &tasks {
        let title_display = if t.title.len() > 38 {
            format!("{}...", &t.title[..35])
        } else {
            t.title.clone()
        };
        println!(
            "#{:<5} {:<40} {:<14} {:<8}",
            t.id, title_display, t.status, t.role
        );
    }
    println!("\n{} tasks in {project}", tasks.len());
    Ok(())
}

fn run_move(task_id: u64, new_status: &str, project: Option<&str>) -> Result<()> {
    let sipag_dir = default_sipag_dir();
    let project = resolve_project(project)?;
    let task = board::move_task(&sipag_dir, &project, task_id, new_status)?;
    println!("#{} -> {} in {project}", task.id, task.status);
    Ok(())
}

fn run_projects() -> Result<()> {
    let sipag_dir = default_sipag_dir();
    let names = board::list_project_names(&sipag_dir)?;

    if names.is_empty() {
        println!("No projects. Create one with: sipag project add <name> --repo <owner/repo>");
        return Ok(());
    }

    let cfg = board::BoardConfig::load(&sipag_dir)?;

    for name in &names {
        let marker = if cfg.default_project.as_deref() == Some(name) {
            " (default)"
        } else {
            ""
        };
        match board::load_project(&sipag_dir, name) {
            Ok(proj) => println!("  {} — {}{}", proj.name, proj.repo, marker),
            Err(_) => println!("  {} — (invalid project.toml){}", name, marker),
        }
    }

    Ok(())
}

fn run_project_add(name: &str, repo: &str) -> Result<()> {
    let sipag_dir = default_sipag_dir();
    board::create_project(&sipag_dir, name, repo, None)?;
    println!("Project '{name}' created ({repo}).");

    // If this is the first project, set it as default.
    let names = board::list_project_names(&sipag_dir)?;
    if names.len() == 1 {
        let mut cfg = board::BoardConfig::load(&sipag_dir)?;
        cfg.default_project = Some(name.to_string());
        cfg.save(&sipag_dir)?;
        println!("Set as default project.");
    }

    Ok(())
}

/// Read katulong remote config from ~/.katulong/remote.json.
/// Returns (url, api_key) tuple.
fn read_katulong_remote() -> Result<(String, Option<String>)> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let config_path = Path::new(&home).join(".katulong/remote.json");
    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Cannot read {}", config_path.display()))?;
    let parsed: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("Invalid JSON in {}", config_path.display()))?;

    let url = parsed["url"]
        .as_str()
        .filter(|s| !s.is_empty())
        .context("Missing 'url' in ~/.katulong/remote.json")?
        .to_string();

    let api_key = parsed["apiKey"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    Ok((url, api_key))
}

fn run_sub(topic: &str, from_seq: u64, json_output: bool) -> Result<()> {
    let (katulong_url, api_key) = read_katulong_remote()?;

    // URL-encode the topic (slashes become path segments for the SSE endpoint).
    // katulong expects: GET /sub/:topic where topic uses / separators.
    let encoded_topic = topic.replace('/', "%2F");
    let url = format!(
        "{}/sub/{}?fromSeq={}",
        katulong_url.trim_end_matches('/'),
        encoded_topic,
        from_seq
    );

    eprintln!("Subscribing to: {topic}");
    eprintln!("Endpoint: {url}");

    // Use curl to connect to SSE endpoint and stream events.
    let mut cmd = Command::new("curl");
    cmd.args(["-sfN", "--no-buffer"]);

    if let Some(ref key) = api_key {
        cmd.args(["-H", &format!("Authorization: Bearer {key}")]);
    }

    cmd.arg(&url);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());

    let mut child = cmd
        .spawn()
        .context("Failed to start curl for SSE subscription")?;
    let stdout = child
        .stdout
        .take()
        .context("Failed to capture curl stdout")?;

    let reader = BufReader::new(stdout);
    let mut event_type = String::new();
    let mut data_buf = String::new();

    for line in reader.lines() {
        let line = line.context("Error reading SSE stream")?;

        if let Some(rest) = line.strip_prefix("event:") {
            event_type = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("data:") {
            let data = rest.trim();
            if !data_buf.is_empty() {
                data_buf.push('\n');
            }
            data_buf.push_str(data);
        } else if line.is_empty() && !data_buf.is_empty() {
            // End of SSE event — dispatch
            if json_output {
                println!("{data_buf}");
            } else {
                // Pretty-print: try to parse as JSON for display
                match serde_json::from_str::<serde_json::Value>(&data_buf) {
                    Ok(val) => {
                        let evt = val["event"].as_str().unwrap_or(&event_type);
                        let ts = val["timestamp"].as_str().unwrap_or("?");
                        let session = val["session"].as_str().unwrap_or("?");
                        println!("[{ts}] {evt} session={session}");
                        // Print extra fields based on event type
                        if let Some(task_id) = val["task_id"].as_str() {
                            println!("  task_id: {task_id}");
                        }
                        if let Some(msg) = val["message"].as_str() {
                            if !msg.is_empty() {
                                println!("  message: {msg}");
                            }
                        }
                    }
                    Err(_) => {
                        println!("[{event_type}] {data_buf}");
                    }
                }
            }
            event_type.clear();
            data_buf.clear();
        }
    }

    let _ = child.wait();
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

    #[test]
    fn parse_pr_url_valid() {
        let (repo, pr) = parse_pr_url("https://github.com/acme/my-app/pull/42").unwrap();
        assert_eq!(repo, "acme/my-app");
        assert_eq!(pr, 42);
    }

    #[test]
    fn parse_pr_url_trailing_slash() {
        let (repo, pr) = parse_pr_url("https://github.com/owner/repo/pull/7/").unwrap();
        assert_eq!(repo, "owner/repo");
        assert_eq!(pr, 7);
    }

    #[test]
    fn parse_pr_url_extra_path_segments() {
        let (repo, pr) = parse_pr_url("https://github.com/acme/my-app/pull/42/files").unwrap();
        assert_eq!(repo, "acme/my-app");
        assert_eq!(pr, 42);
    }

    #[test]
    fn parse_pr_url_invalid() {
        assert!(parse_pr_url("https://github.com/owner/repo").is_err());
        assert!(parse_pr_url("not-a-url").is_err());
    }

    #[test]
    fn parse_pr_url_empty_owner_or_repo() {
        assert!(parse_pr_url("https://github.com//repo/pull/1").is_err());
        assert!(parse_pr_url("https://github.com/owner//pull/1").is_err());
    }

    #[test]
    fn parse_pr_url_non_numeric_pr() {
        assert!(parse_pr_url("https://github.com/owner/repo/pull/abc").is_err());
    }

    #[test]
    fn dispatch_target_detection_task_id() {
        // Pure numbers should parse as task IDs.
        assert!("42".parse::<u64>().is_ok());
        assert!("1".parse::<u64>().is_ok());
        assert!("99999".parse::<u64>().is_ok());
    }

    #[test]
    fn dispatch_target_detection_url() {
        // URLs should NOT parse as task IDs.
        assert!("https://github.com/owner/repo/pull/42"
            .parse::<u64>()
            .is_err());
        assert!("not-a-number".parse::<u64>().is_err());
    }

    #[test]
    fn resolve_project_explicit() {
        let result = resolve_project(Some("myproject"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "myproject");
    }
}
