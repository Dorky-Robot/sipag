use crate::dispatch_gate;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use sipag_board as board;
use sipag_core::{config::default_sipag_dir, katulong};
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

    // `Feature { action: FeatureAction }` and `Refine { ... }` subcommands
    // were deprecated 2026-05-17 and their wiring stripped. See
    // sipag_core::{feature, refine} module doc-comments and
    // docs/architecture.md §3 for the work-model reframe (Experimentation
    // replaces the kanban refinement pipeline).
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

    /// Run the agent-manager web server
    ///
    /// Server-rendered HTMX UI (maud templates) plus a JSON `/api/*`
    /// surface for programmatic clients. Proxies the katulong mesh
    /// defined in ~/.sipag/hosts.toml; API keys stay server-side.
    Serve {
        /// Port to listen on
        #[arg(long, default_value_t = 7100)]
        port: u16,

        /// Directory of static assets served as a fallback (style.css,
        /// htmx.min.js, favicons). The HTML for `/` is rendered by the
        /// server, not loaded from disk.
        #[arg(long, default_value = "web/public")]
        web_root: std::path::PathBuf,

        /// Enable autonomous workers (research, expand, …). When off
        /// (default) the server still ships the UI, pubsub, WS, and
        /// HTMX CRUD; only the label-driven dispatcher is gated.
        #[arg(long, default_value_t = false)]
        workers: bool,

        /// Enable the lens-worker scheduler (Phase 1 #3). Loads lens
        /// definitions from `~/.sipag/lenses/*.toml` and fires each
        /// on its `TriggerPolicy::Schedule` cadence. Requires
        /// `~/.ollama-bridge/remote.json` for the bridge URL + bearer.
        /// Default OFF — operator opts in once the lens registry has
        /// content and the bridge is reachable.
        #[arg(long, default_value_t = false)]
        lens_scheduler: bool,

        /// Enable the bridge lens-worker (§9 Phase 1 #3 remainder).
        /// Subscribes to katulong `claude/<uuid>` SSE topics for
        /// dispatched sessions and fires gemma against a sliding
        /// window of events to produce observations. Requires both
        /// `~/.ollama-bridge/remote.json` (gemma) and
        /// `~/.katulong/remote.json` (SSE).
        #[arg(long, default_value_t = false)]
        bridge_worker: bool,
    },

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

// `FeatureAction` enum was removed 2026-05-17 with the rest of the
// deprecated refinement wiring. See sipag_core::feature module doc.

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
        Some(Commands::Sub {
            topic,
            from_seq,
            json,
        }) => run_sub(&topic, from_seq, json),
        Some(Commands::Serve {
            port,
            web_root,
            workers,
            lens_scheduler,
            bridge_worker,
        }) => crate::serve::run(port, web_root, workers, lens_scheduler, bridge_worker),
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

    // Load project — the gate needs its statuses (each with a free-form
    // description) so gemma4 has a list of columns to pick from, and a
    // `dispatchable` flag so we know which one means "fire."
    let project_cfg = board::Project::load(&sipag_dir, &project_name)
        .with_context(|| format!("failed to load project '{project_name}'"))?;
    let dispatchable = project_cfg.dispatchable_status().with_context(|| {
        format!(
            "cannot dispatch from project '{project_name}' — its project.toml needs one status \
             marked `dispatchable = true`"
        )
    })?;
    let dispatchable_name = dispatchable.name.clone();

    // Connect to katulong. Load the remote config once — used to
    // build the sync client (for gate output read + session create)
    // AND passed to `sipag_dispatch::dispatch` (which builds its own
    // async clients internally — see `sipag-dispatch` crate docs).
    let remote = katulong::RemoteConfig::load()
        .context("Cannot connect to katulong — is ~/.katulong/remote.json configured?")?;
    let client = katulong::KatulongClient::new(remote.url.clone(), remote.api_key.clone());

    let session_name = katulong::session_name(&project_name, role_name);

    // 1. Create session (idempotent find-or-create). The returned id is
    //    the stable handle for subsequent /sessions/by-id/... calls.
    println!("Creating session {session_name}...");
    let session = client.create_session(&session_name)?;

    // 2. Gate — ask gemma4 to classify the session's current state
    //    against the project's declared statuses. Programmatic
    //    detection lost to the long tail of pane states (login banner,
    //    permission prompt, mid-compaction, stuck-paste, etc.), so the
    //    classifier is the dispatcher now. Fail closed: any error
    //    from the classifier aborts dispatch.
    println!("Classifying session via gemma4...");
    let session_output = client
        .session_output_lines(&session.id, 80)
        .context("failed to fetch session output for gate classify")?;
    let decision = gate_classify(&task, &role, &project_cfg, &session_output)?;

    if decision.status_name != dispatchable_name {
        park_task_at(&sipag_dir, &project_name, task_id, &decision, &session_name)?;
        return Ok(());
    }

    // 3 + 4. Run the dispatch action via the sipag-dispatch crate.
    //    The crate handles worktree setup (HTTP `/exec`) and the WS
    //    attach + launch + paste + submit + processing-wait flow.
    //    Wrapped in a one-shot tokio runtime — same pattern as
    //    `gate_classify` — so the CLI stays sync-shaped at the top
    //    level. The CLI gets the same WS-attach orchestration the
    //    web UI uses; previously this path was raw HTTP `/exec` and
    //    couldn't see when the agent's TUI was actually ready.
    let prompt = format!("Work on task #{task_id}: {}", task.title);
    let worktree = if role.worktree {
        Some(sipag_dispatch::WorktreeSpec {
            setup_command: katulong::worktree_command(&project_name, task_id),
            path: katulong::worktree_path(&project_name, task_id),
        })
    } else {
        None
    };
    let input = sipag_dispatch::DispatchInput {
        task_id,
        project_name: project_name.clone(),
        task_title: task.title.clone(),
        role_command: role.command.clone(),
        prompt,
        worktree,
    };
    println!("Launching agent for task #{task_id}: {}", task.title);
    run_sipag_dispatch_cli(remote, session.clone(), input)?;

    // 5. Move task to in-progress, clearing any prior reason /
    //    human_action (e.g. from a previous parked dispatch that the
    //    operator just unblocked).
    advance_task_to_in_progress(&sipag_dir, &project_name, task_id)?;

    // 6. Confirmation.
    println!();
    println!("Dispatched task #{task_id} to session {session_name}");
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
    println!("Monitor at: {} (session: {session_name})", client.url());

    Ok(())
}

/// Run `sipag_dispatch::dispatch` inside a one-shot tokio runtime.
/// Same pattern as [`gate_classify`] — the rest of the CLI is sync;
/// only the WS attach flow needs async. Building a current-thread
/// runtime per dispatch is cheap relative to the agent launch latency
/// and keeps the rest of the codepath synchronous.
fn run_sipag_dispatch_cli(
    remote: katulong::RemoteConfig,
    session: katulong::TmuxSession,
    input: sipag_dispatch::DispatchInput,
) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime for sipag-dispatch")?;
    rt.block_on(async {
        sipag_dispatch::dispatch(remote, &session, input, |step| {
            // CLI step observer: a single line per phase transition
            // so the user sees forward progress. Matches the spirit
            // of the previous CLI's "Creating worktree..." /
            // "Launching agent..." prints.
            let label: &str = match step {
                sipag_dispatch::DispatchStep::WorktreeSetup => "Creating worktree",
                sipag_dispatch::DispatchStep::Attach => "Attaching to session",
                sipag_dispatch::DispatchStep::Launch => "Launching agent",
                sipag_dispatch::DispatchStep::WaitTuiReady => "Waiting for TUI",
                sipag_dispatch::DispatchStep::PastePrompt => "Pasting prompt",
                sipag_dispatch::DispatchStep::WaitEcho => "Confirming paste",
                sipag_dispatch::DispatchStep::Submit => "Submitting",
                sipag_dispatch::DispatchStep::WaitProcessing => "Waiting for response",
            };
            println!("  ▸ {label}...");
        })
        .await
    })
    .context("dispatch action failed")?;
    Ok(())
}

/// Run `dispatch_gate::classify` inside a one-shot tokio runtime. The
/// rest of the CLI is sync; only the bridge call (and therefore the
/// gate) needs to be async. Building a current-thread runtime per
/// dispatch is cheap relative to the model call itself and keeps the
/// rest of the codepath synchronous.
///
/// Loads `~/.ollama-bridge/remote.json` per call (the CLI doesn't
/// share an AppState). If missing/malformed, returns Err with a
/// clear "configure the bridge" message so dispatch fails closed.
fn gate_classify(
    task: &board::Task,
    role: &board::Role,
    project_cfg: &board::Project,
    session_output: &str,
) -> Result<dispatch_gate::GateDecision> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime for gate classify")?;
    rt.block_on(async {
        // Use the shared bridge-friendly client (Mozilla UA + 600s
        // timeout). `reqwest::Client::new()` would 403 against a
        // Cloudflare-fronted bridge tunnel and would hang
        // indefinitely on transport failure.
        let http = crate::bridge::default_http_client()?;
        let wiring = crate::bridge::build_bridge_wiring(http).context(
            "dispatch gate requires the ollama bridge; set up ~/.ollama-bridge/remote.json",
        )?;
        dispatch_gate::classify(
            &wiring.chat,
            dispatch_gate::GateInput {
                task_title: &task.title,
                task_role: &role.command,
                statuses: &project_cfg.statuses,
                session_output,
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("gate classify failed: {e}"))
    })
}

/// Park the task at the status gemma4 chose, persisting the reason
/// and (when present) the human action. The operator sees this in
/// the TUI / web UI and unblocks it (e.g., by `/login`-ing the
/// stuck katulong tile) before re-dispatching.
fn park_task_at(
    sipag_dir: &std::path::Path,
    project_name: &str,
    task_id: u64,
    decision: &dispatch_gate::GateDecision,
    session_name: &str,
) -> Result<()> {
    let mut task = board::Task::load(sipag_dir, project_name, task_id)?;
    task.status = board::TaskStatus::parse(&decision.status_name);
    task.reason = if decision.reason.trim().is_empty() {
        None
    } else {
        Some(decision.reason.clone())
    };
    task.human_action = decision.human_action.clone();
    task.updated = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    task.save(sipag_dir, project_name)?;

    println!();
    println!(
        "Task #{task_id} NOT dispatched — gemma4 classified session {session_name} as '{}'",
        decision.status_name
    );
    if let Some(reason) = &task.reason {
        println!("  Reason: {reason}");
    }
    if let Some(action) = &task.human_action {
        println!("  Action: {action}");
    }
    Ok(())
}

/// Advance the task to `in-progress`, clearing any reason /
/// human_action that may have been left over from an earlier parked
/// dispatch. Using a single load/save keeps the three mutations
/// atomic and avoids `move_task`'s status-only update path.
fn advance_task_to_in_progress(
    sipag_dir: &std::path::Path,
    project_name: &str,
    task_id: u64,
) -> Result<()> {
    let mut task = board::Task::load(sipag_dir, project_name, task_id)?;
    task.status = board::TaskStatus::InProgress;
    task.reason = None;
    task.human_action = None;
    task.updated = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    task.save(sipag_dir, project_name)?;
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
        let session_name = katulong::session_name(&project_name, &role.name);
        // create_session is idempotent (409 → list lookup), so the
        // session may have already existed — say "ready" rather than
        // "created" to avoid implying we made a new one each time.
        match client.create_session(&session_name) {
            Ok(_) => println!("  {session_name} — ready"),
            Err(e) => println!("  {session_name} — FAILED: {e}"),
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

// `run_feature_add`, `run_feature_list`, `run_feature_show`, and
// `run_refine` were removed 2026-05-17 along with the deprecated
// refinement pipeline wiring. See sipag_core::{feature, refine}.

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

fn run_sub(topic: &str, from_seq: u64, json_output: bool) -> Result<()> {
    let cfg = katulong::RemoteConfig::load()
        .context("Cannot load ~/.katulong/remote.json — is it set up?")?;

    eprintln!("Subscribing to: {topic}");
    eprintln!("From seq: {from_seq}");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime for sub")?;

    rt.block_on(async {
        use futures::StreamExt;

        let http = reqwest::Client::builder()
            .user_agent(concat!("sipag/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to build HTTP client")?;

        let mut stream = katulong::subscribe(http, &cfg.url, &cfg.api_key, topic, from_seq)
            .await
            .context("SSE connect failed")?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(evt) => {
                    if json_output {
                        let mut map = evt.extra.clone();
                        map.insert("seq".into(), evt.seq.into());
                        map.insert("event".into(), evt.event.clone().into());
                        map.insert("timestamp".into(), evt.timestamp.clone().into());
                        if let Some(s) = &evt.session {
                            map.insert("session".into(), s.clone().into());
                        }
                        if let Ok(json) = serde_json::to_string(&map) {
                            println!("{json}");
                        }
                    } else {
                        let session = evt.session.as_deref().unwrap_or("?");
                        println!("[{}] {} session={session}", evt.timestamp, evt.event);
                        if let Some(serde_json::Value::String(task_id)) = evt.extra.get("task_id") {
                            println!("  task_id: {task_id}");
                        }
                        if let Some(serde_json::Value::String(msg)) = evt.extra.get("message") {
                            if !msg.is_empty() {
                                println!("  message: {msg}");
                            }
                        }
                    }
                }
                Err(katulong::SseError::BadEvent(msg)) => {
                    eprintln!("malformed event: {msg}");
                }
                Err(e) => {
                    anyhow::bail!("SSE stream error: {e}");
                }
            }
        }
        eprintln!("stream ended");
        Ok(())
    })
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
