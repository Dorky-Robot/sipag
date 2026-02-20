use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use sipag_core::{
    executor::{self, generate_task_id, RunConfig},
    repo,
    task::{self, default_sipag_dir, default_sipag_file, TaskStatus},
};
use std::fs;
use std::path::PathBuf;

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

#[derive(Subcommand)]
pub enum Commands {
    /// Launch the interactive TUI (default when no args given)
    Tui,

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
    Start,

    /// Create ~/.sipag/{queue,running,done,failed}
    Init,

    /// Queue a task
    Add {
        /// Task title/description
        title: String,

        /// Repository name (writes YAML file to queue/)
        #[arg(long)]
        repo: Option<String>,

        /// Priority level
        #[arg(long, default_value = "medium")]
        priority: String,
    },

    /// Print all tasks with status (markdown checklist file)
    List {
        /// Task file path (default: ./tasks.md or $SIPAG_FILE)
        #[arg(short = 'f', long)]
        file: Option<PathBuf>,
    },

    /// Find first pending task, run claude, mark done
    Next {
        /// After completing, loop to the next task
        #[arg(short = 'c', long)]
        r#continue: bool,

        /// Show what would run; don't invoke claude
        #[arg(short = 'n', long)]
        dry_run: bool,

        /// Task file path (default: ./tasks.md or $SIPAG_FILE)
        #[arg(short = 'f', long)]
        file: Option<PathBuf>,
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

    /// Pick up approved GitHub issues and dispatch Docker workers
    Work {
        /// GitHub repository slug (owner/repo)
        repo: String,

        /// Git clone URL (defaults to https://github.com/<owner/repo>)
        #[arg(long)]
        repo_url: Option<String>,

        /// Label to filter issues on (default: approved; overrides SIPAG_WORK_LABEL and ~/.sipag/config)
        #[arg(long)]
        label: Option<String>,
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

#[derive(Subcommand)]
pub enum RepoCommands {
    /// Register a repo name → URL mapping
    Add { name: String, url: String },
    /// List registered repos
    List,
}

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        None | Some(Commands::Tui) => {
            let sipag_dir = default_sipag_dir();
            crate::tui::run_tui(&sipag_dir)
        }
        Some(Commands::Version) => {
            println!("sipag {VERSION}");
            Ok(())
        }
        Some(Commands::Init) => cmd_init(),
        Some(Commands::Start) => cmd_start(),
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
        }) => cmd_add(&title, repo.as_deref(), &priority),
        Some(Commands::List { file }) => cmd_list(file.as_deref()),
        Some(Commands::Next {
            r#continue,
            dry_run,
            file,
        }) => cmd_next(r#continue, dry_run, file.as_deref()),
        Some(Commands::Show { name }) => cmd_show(&name),
        Some(Commands::Retry { name }) => cmd_retry(&name),
        Some(Commands::Repo { subcommand }) => cmd_repo(subcommand),
        Some(Commands::Status) => cmd_status(),
        Some(Commands::Work { repo, repo_url, label }) => cmd_work(&repo, repo_url.as_deref(), label.as_deref()),
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

fn cmd_init() -> Result<()> {
    task::init_dirs(&sipag_dir())
}

fn cmd_start() -> Result<()> {
    let dir = sipag_dir();
    task::init_dirs(&dir).ok();
    println!("sipag executor starting (queue: {}/queue)", dir.display());

    let queue_dir = dir.join("queue");
    let running_dir = dir.join("running");
    let failed_dir = dir.join("failed");
    let image = std::env::var("SIPAG_IMAGE").unwrap_or_else(|_| "sipag-worker:latest".to_string());
    let timeout = std::env::var("SIPAG_TIMEOUT")
        .unwrap_or_else(|_| "1800".to_string())
        .parse::<u64>()
        .unwrap_or(1800);

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

        let task = match task::parse_task_file(&task_file, TaskStatus::Queue) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("Error: failed to parse task file: {e}");
                let _ = fs::rename(&task_file, failed_dir.join(format!("{task_name}.md")));
                println!("==> Failed: {task_name}");
                processed += 1;
                continue;
            }
        };

        let repo_name = task.repo.as_deref().unwrap_or("");
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
        let issue_num = task
            .source
            .as_deref()
            .and_then(|s| s.split('#').next_back())
            .filter(|s| s.chars().all(|c| c.is_ascii_digit()))
            .map(|s| s.to_string());

        // Move task to running/ before execution
        let running_file = running_dir.join(format!("{task_name}.md"));
        let _ = fs::rename(&task_file, &running_file);

        let _ = executor::run_impl(
            &dir,
            RunConfig {
                task_id: &task_name,
                repo_url: &url,
                description: &task.title,
                issue: issue_num.as_deref(),
                background: false,
                image: &image,
                timeout_secs: timeout,
            },
        );

        processed += 1;
    }

    Ok(())
}

fn cmd_run(repo_url: &str, issue: Option<&str>, background: bool, description: &str) -> Result<()> {
    let dir = sipag_dir();
    task::init_dirs(&dir).ok();

    let task_id = generate_task_id(description);
    println!("Task ID: {task_id}");

    let image = std::env::var("SIPAG_IMAGE").unwrap_or_else(|_| "sipag-worker:latest".to_string());
    let timeout = std::env::var("SIPAG_TIMEOUT")
        .unwrap_or_else(|_| "1800".to_string())
        .parse::<u64>()
        .unwrap_or(1800);

    executor::run_impl(
        &dir,
        RunConfig {
            task_id: &task_id,
            repo_url,
            description,
            issue,
            background,
            image: &image,
            timeout_secs: timeout,
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
            let task = match task::parse_task_file(&path, TaskStatus::Queue) {
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
            executor::format_duration(secs)
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

    let log_file = dir.join("running").join(format!("{task_id}.log"));
    let failed_dir = dir.join("failed");
    fs::rename(&tracking_file, failed_dir.join(format!("{task_id}.md")))?;
    if log_file.exists() {
        fs::rename(&log_file, failed_dir.join(format!("{task_id}.log")))?;
    }

    println!("Killed: {task_id}");
    Ok(())
}

fn cmd_add(title: &str, repo: Option<&str>, priority: &str) -> Result<()> {
    if title.is_empty() {
        bail!("Usage: sipag add \"task text\" [--repo <name>] [--priority <level>]");
    }

    let dir = sipag_dir();
    if let Some(repo_name) = repo {
        // Queue mode: write YAML file to queue/
        if !dir.join("queue").exists() {
            task::init_dirs(&dir).ok();
        }
        let filename = task::next_filename(&dir.join("queue"), title);
        let path = dir.join("queue").join(&filename);
        task::write_task_file(&path, title, repo_name, priority, None)?;
        println!("Added: {title}");
    } else {
        // Legacy checklist mode: append to SIPAG_FILE
        let file = default_sipag_file();
        task::append_checklist_item(&file, title)?;
        println!("Added: {title}");
    }

    Ok(())
}

fn cmd_list(file: Option<&std::path::Path>) -> Result<()> {
    let file = file
        .map(|f| f.to_path_buf())
        .unwrap_or_else(default_sipag_file);

    if !file.exists() {
        bail!("No task file: {}", file.display());
    }

    let items = task::parse_checklist(&file)?;
    let done_count = items.iter().filter(|i| i.done).count();
    let total = items.len();

    for item in &items {
        if item.done {
            println!("  [x] {}", item.title);
        } else {
            println!("  [ ] {}", item.title);
        }
    }

    println!();
    println!("{done_count}/{total} done");
    Ok(())
}

fn cmd_next(cont: bool, dry_run: bool, file: Option<&std::path::Path>) -> Result<()> {
    let file = file
        .map(|f| f.to_path_buf())
        .unwrap_or_else(default_sipag_file);

    loop {
        let item = match task::next_checklist_item(&file)? {
            Some(i) => i,
            None => {
                println!("No pending tasks in {}", file.display());
                return Ok(());
            }
        };

        println!("==> Task {}: {}", item.line_num, item.title);

        if dry_run {
            if !item.body.is_empty() {
                println!();
                println!("{}", item.body);
            }
            println!();
            println!("(dry run — skipping claude)");
            return Ok(());
        }

        match executor::run_claude(&item.title, &item.body) {
            Ok(_) => {
                task::mark_checklist_done(&file, item.line_num)?;
                println!("==> Done: {}", item.title);
            }
            Err(e) => {
                println!("==> Failed: {}: {e}", item.title);
                return Err(e);
            }
        }

        if !cont {
            return Ok(());
        }

        println!();
    }
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

    fs::rename(&failed_file, dir.join("queue").join(format!("{name}.md")))?;
    if failed_log.exists() {
        let _ = fs::remove_file(&failed_log);
    }

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

/// Load work_label from ~/.sipag/config (key=value format).
/// Returns None if the key is not found or the file doesn't exist.
fn load_work_label_from_config() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let sipag_dir = std::env::var("SIPAG_DIR")
        .unwrap_or_else(|_| format!("{home}/.sipag"));
    let config_path = std::path::Path::new(&sipag_dir).join("config");

    let content = fs::read_to_string(&config_path).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == "work_label" {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

fn cmd_work(gh_repo: &str, repo_url: Option<&str>, label_flag: Option<&str>) -> Result<()> {
    // Resolve the label to filter by, in priority order:
    //   1. --label flag (highest priority)
    //   2. SIPAG_WORK_LABEL env var
    //   3. work_label key in ~/.sipag/config
    //   4. "approved" (default)
    let work_label = if let Some(l) = label_flag {
        l.to_string()
    } else if let Ok(env_label) = std::env::var("SIPAG_WORK_LABEL") {
        env_label
    } else if let Some(cfg_label) = load_work_label_from_config() {
        cfg_label
    } else {
        "approved".to_string()
    };

    // Derive clone URL from the GitHub slug if not provided
    let clone_url = repo_url
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("https://github.com/{gh_repo}"));

    if !work_label.is_empty() {
        println!("==> Fetching open issues labelled '{work_label}' from {gh_repo}…");
    } else {
        println!("==> Fetching ALL open issues from {gh_repo} (no label filter)…");
    }

    // Build gh issue list arguments
    let mut gh_args = vec![
        "issue".to_string(),
        "list".to_string(),
        "--repo".to_string(),
        gh_repo.to_string(),
        "--state".to_string(),
        "open".to_string(),
        "--json".to_string(),
        "number,title".to_string(),
        "--limit".to_string(),
        "50".to_string(),
    ];

    if !work_label.is_empty() {
        gh_args.push("--label".to_string());
        gh_args.push(work_label.clone());
    }

    let output = std::process::Command::new("gh")
        .args(&gh_args)
        .output()
        .context("Failed to run 'gh issue list' — is the GitHub CLI installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("gh issue list failed: {stderr}");
    }

    let issues_json = String::from_utf8_lossy(&output.stdout);
    let issues: serde_json::Value = serde_json::from_str(&issues_json)
        .context("Failed to parse gh issue list JSON output")?;

    let issues_arr = issues
        .as_array()
        .context("Expected JSON array from gh issue list")?;

    println!("==> Found {} issue(s)", issues_arr.len());

    if issues_arr.is_empty() {
        println!("==> Nothing to work on.");
        return Ok(());
    }

    let dir = sipag_dir();
    task::init_dirs(&dir).ok();

    let image = std::env::var("SIPAG_IMAGE").unwrap_or_else(|_| "sipag-worker:latest".to_string());
    let timeout = std::env::var("SIPAG_TIMEOUT")
        .unwrap_or_else(|_| "1800".to_string())
        .parse::<u64>()
        .unwrap_or(1800);

    let mut dispatched = 0usize;
    let mut failed = 0usize;

    for issue in issues_arr {
        let number = issue["number"]
            .as_u64()
            .context("Issue missing 'number' field")?;
        let title = issue["title"]
            .as_str()
            .unwrap_or("(no title)")
            .to_string();

        println!("==> Working on #{number}: {title}");

        let issue_str = number.to_string();
        let task_id = executor::generate_task_id(&title);

        match executor::run_impl(
            &dir,
            RunConfig {
                task_id: &task_id,
                repo_url: &clone_url,
                description: &title,
                issue: Some(&issue_str),
                background: true,
                image: &image,
                timeout_secs: timeout,
            },
        ) {
            Ok(_) => {
                println!("==> Dispatched #{number}");
                dispatched += 1;
            }
            Err(e) => {
                eprintln!("==> Failed to dispatch #{number}: {e}");
                failed += 1;
            }
        }
    }

    println!("==> work complete (dispatched={dispatched} failed={failed})");
    Ok(())
}

fn cmd_status() -> Result<()> {
    let dir = sipag_dir();
    let labels = [("Queue", "queue"), ("Running", "running"), ("Done", "done"), ("Failed", "failed")];

    for (label, subdir) in &labels {
        let d = dir.join(subdir);
        if !d.exists() {
            continue;
        }
        let mut items: Vec<String> = fs::read_dir(&d)
            .unwrap_or_else(|_| fs::read_dir("/dev/null").unwrap())
            .flatten()
            .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();

        if items.is_empty() {
            continue;
        }

        items.sort();
        println!("{} ({}):", label, items.len());
        for item in &items {
            println!("  {item}");
        }
    }

    Ok(())
}
