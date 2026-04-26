use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use sipag_core::auth::{random_token, SetupPurpose, SetupToken};
use sipag_core::{board, config::default_sipag_dir, feature, katulong, refine};
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
    about = "Work dispatcher for Claude Code crews",
    long_about = "sipag is a board-driven dispatcher that ships tasks to katulong sessions.\n\nRun with no arguments to launch the interactive TUI."
)]
// `version: ()` is a clap-only field that wires `-v`/`--version` to
// the version-printing action — it has no value and no public use,
// so clippy's manual-non-exhaustive lint mistakes the shape for an
// API constraint. It isn't.
#[allow(clippy::manual_non_exhaustive)]
pub struct Cli {
    #[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]
    version: (),

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Dispatch a task by ID to its configured role session
    Dispatch {
        /// Task ID (e.g. 42)
        #[arg(value_name = "TASK_ID")]
        task_id: u64,

        /// Project name (default: from config)
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

    /// Launch interactive TUI
    Tui,

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

    /// Manage dispatch features (raw ideas, refinement queue)
    Feature {
        #[command(subcommand)]
        action: FeatureAction,
    },

    /// Refine one or more raw features into actionable tickets
    Refine {
        /// Feature IDs to refine (one or more)
        #[arg(value_name = "FEATURE_ID", required = true, num_args = 1..)]
        feature_ids: Vec<String>,

        /// Project name (default: from config)
        #[arg(short, long)]
        project: Option<String>,
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

    /// Run the agent-manager web server (Week-1 spike)
    ///
    /// Serves the ClojureScript SPA and proxies the katulong mesh defined
    /// in ~/.sipag/hosts.toml. API keys stay server-side.
    Serve {
        /// Port to listen on
        #[arg(long, default_value_t = 7100)]
        port: u16,

        /// Directory holding the compiled cljs SPA (index.html + assets)
        #[arg(long, default_value = "web/public")]
        web_root: std::path::PathBuf,
    },

    /// Mint a single-use setup token (Track A passkey bootstrap)
    ///
    /// Prints a URL like `http://localhost:7100/setup?token=<hex>`.
    /// Open it on a trusted device to enroll your first passkey.
    /// The token expires after 10 minutes and can only be used once.
    /// Set `SIPAG_PUBLIC_URL` to override the printed URL prefix
    /// (e.g. `https://sipag.felixflor.es`).
    SetupToken,

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

#[derive(Debug, Subcommand)]
pub enum FeatureAction {
    /// Add a raw feature idea to the dispatch store
    Add {
        /// The raw idea text (body of the feature)
        text: String,

        /// Project name (default: from config)
        #[arg(short, long)]
        project: Option<String>,

        /// Comma-separated list of projects this feature should target
        #[arg(long, value_delimiter = ',')]
        projects: Vec<String>,
    },

    /// List features in the dispatch store
    List {
        /// Project name (default: from config)
        #[arg(short, long)]
        project: Option<String>,

        /// Filter by status (raw, grouped, refined, needs-info, active)
        #[arg(long)]
        status: Option<String>,
    },

    /// Show a single feature (frontmatter + body)
    Show {
        /// Feature id (e.g. f-...)
        id: String,

        /// Project name (default: from config)
        #[arg(short, long)]
        project: Option<String>,
    },
}

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        None => run_tui(),
        Some(Commands::Tui) => run_tui(),
        Some(Commands::Dispatch {
            task_id,
            project,
            role,
        }) => run_dispatch_task(task_id, project.as_deref(), role.as_deref()),
        Some(Commands::Up { project }) => run_up(project.as_deref()),
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
        Some(Commands::Feature { action }) => match action {
            FeatureAction::Add {
                text,
                project,
                projects,
            } => run_feature_add(&text, project.as_deref(), &projects),
            FeatureAction::List { project, status } => {
                run_feature_list(project.as_deref(), status.as_deref())
            }
            FeatureAction::Show { id, project } => run_feature_show(&id, project.as_deref()),
        },
        Some(Commands::Refine {
            feature_ids,
            project,
        }) => run_refine(&feature_ids, project.as_deref()),
        Some(Commands::Sub {
            topic,
            from_seq,
            json,
        }) => run_sub(&topic, from_seq, json),
        Some(Commands::Serve { port, web_root }) => crate::serve::run(port, web_root),
        Some(Commands::SetupToken) => run_setup_token(),
        Some(Commands::Version) => run_version(),
    }
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

// ── Feature store handlers ────────────────────────────────────────────────

fn run_feature_add(text: &str, project: Option<&str>, projects: &[String]) -> Result<()> {
    let sipag_dir = default_sipag_dir();
    let project_name = resolve_project(project)?;
    let projects_opt = if projects.is_empty() {
        None
    } else {
        Some(projects.to_vec())
    };
    let f = feature::Feature::add(&sipag_dir, &project_name, text, projects_opt)?;
    println!("{}", f.id);
    Ok(())
}

fn run_feature_list(project: Option<&str>, status: Option<&str>) -> Result<()> {
    let sipag_dir = default_sipag_dir();
    let project_name = resolve_project(project)?;
    let features = feature::Feature::list(&sipag_dir, &project_name, status)?;

    if features.is_empty() {
        if let Some(s) = status {
            println!("No {s} features in {project_name}.");
        } else {
            println!("No features in {project_name}.");
        }
        return Ok(());
    }

    println!("{:<40} {:<12} FIRST LINE", "ID", "STATUS");
    println!("{}", "-".repeat(72));
    for f in &features {
        let first_line = f.body.lines().next().unwrap_or("").trim();
        let display = if first_line.len() > 36 {
            format!("{}...", &first_line[..33])
        } else {
            first_line.to_string()
        };
        println!("{:<40} {:<12} {}", f.id, f.status, display);
    }
    println!("\n{} features in {project_name}", features.len());
    Ok(())
}

fn run_feature_show(id: &str, project: Option<&str>) -> Result<()> {
    let sipag_dir = default_sipag_dir();
    let project_name = resolve_project(project)?;
    // Validate it exists and parses cleanly first.
    feature::Feature::get(&sipag_dir, &project_name, id)?
        .with_context(|| format!("feature {id} not found in project {project_name}"))?;
    let path = feature::Feature::path(&sipag_dir, &project_name, id);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    println!("{content}");
    Ok(())
}

fn run_refine(feature_ids: &[String], project: Option<&str>) -> Result<()> {
    let sipag_dir = default_sipag_dir();
    let project_name = resolve_project(project)?;

    // Progress callback prints one bullet per line to stderr so refinement
    // activity is visible in a long-running terminal without polluting
    // stdout (which we reserve for the final ticket list).
    let mut opts = refine::RefineOptions {
        on_progress: Some(Box::new(|bullet: &str| {
            eprintln!("  - {bullet}");
        })),
        ..Default::default()
    };

    let refiner = refine::Refiner::new();
    let created = match refiner.refine_batch(&sipag_dir, &project_name, feature_ids, &mut opts) {
        Ok(c) => c,
        Err(e) => {
            // Never leak `e.detail` to user output — it can contain raw
            // subprocess stderr (internal paths, uncooked claude output).
            // Callers that need the detail can set RUST_LOG=debug in a
            // future commit; for now the detail is dropped at the CLI
            // layer by design.
            let _ = e.detail;
            eprintln!("error: {}", e.public);
            std::process::exit(1);
        }
    };

    println!(
        "Refined {} features into {} tickets:",
        feature_ids.len(),
        created.len()
    );
    for f in &created {
        let proj = f.project.as_deref().unwrap_or("-");
        let title = f.body.lines().next().unwrap_or("").trim();
        println!("  {} [{}] {}", f.id, proj, title);
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

fn run_version() -> Result<()> {
    println!("sipag {VERSION} ({GIT_HASH})");
    Ok(())
}

/// Mint a single-use setup token, persist it under
/// `<sipag_dir>/setup-tokens/`, and print a URL the user opens on a
/// trusted device to enroll their first passkey.
///
/// The URL prefix comes from `SIPAG_PUBLIC_URL` (so a tunneled
/// instance can mint `https://sipag.felixflor.es/...`) or falls back
/// to `http://localhost:7100`. TTL is 10 minutes — the same window
/// katulong's setup tokens use.
fn run_setup_token() -> Result<()> {
    const TTL_MINUTES: i64 = 10;
    let public_url = std::env::var("SIPAG_PUBLIC_URL")
        .unwrap_or_else(|_| "http://localhost:7100".to_string());
    // Strip any trailing slash so format!("{prefix}/setup?...") doesn't
    // produce a double-slash like `http://host//setup`.
    let prefix = public_url.trim_end_matches('/');

    let token = random_token(32);
    let record = SetupToken::new(token.clone(), SetupPurpose::EnrollPasskey, TTL_MINUTES);
    record
        .save(&default_sipag_dir())
        .context("failed to persist setup token")?;

    println!("{prefix}/setup?token={token}");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_project_explicit() {
        let result = resolve_project(Some("myproject"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "myproject");
    }
}
