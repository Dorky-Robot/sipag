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

    /// Print version
    Version,

    /// Print shell completion script (bash, zsh, or fish)
    Completions {
        /// Shell to generate completions for
        #[arg(value_name = "SHELL")]
        shell: String,
    },

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
        Some(Commands::Completions { shell }) => cmd_completions(&shell),
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
    let image = std::env::var("SIPAG_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/dorky-robot/sipag-worker:latest".to_string());
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

    let image = std::env::var("SIPAG_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/dorky-robot/sipag-worker:latest".to_string());
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

fn cmd_completions(shell: &str) -> Result<()> {
    match shell {
        "bash" => print!("{}", BASH_COMPLETION),
        "zsh" => print!("{}", ZSH_COMPLETION),
        "fish" => print!("{}", FISH_COMPLETION),
        other => bail!(
            "Unknown shell '{}'. Supported: bash, zsh, fish\n\nUsage:\n  sipag completions bash   # then: source <(sipag completions bash)\n  sipag completions zsh    # then: sipag completions zsh > ~/.zsh/completions/_sipag\n  sipag completions fish   # then: sipag completions fish > ~/.config/fish/completions/sipag.fish",
            other
        ),
    }
    Ok(())
}

const BASH_COMPLETION: &str = r#"_sipag() {
    local cur prev words cword
    _init_completion 2>/dev/null || {
        COMPREPLY=()
        cur="${COMP_WORDS[COMP_CWORD]}"
        prev="${COMP_WORDS[COMP_CWORD-1]}"
        words=("${COMP_WORDS[@]}")
        cword=$COMP_CWORD
    }

    local commands="tui run ps logs kill start init add list next show retry repo status version completions"
    local sipag_dir="${SIPAG_DIR:-$HOME/.sipag}"

    _sipag_task_ids() {
        local ids=()
        for dir in running done failed queue; do
            local d="$sipag_dir/$dir"
            if [[ -d "$d" ]]; then
                for f in "$d"/*.md; do
                    [[ -f "$f" ]] && ids+=("$(basename "$f" .md)")
                done
            fi
        done
        echo "${ids[@]}"
    }

    _sipag_repo_names() {
        local names=()
        local conf="$sipag_dir/repos.conf"
        if [[ -f "$conf" ]]; then
            while IFS='=' read -r name _; do
                [[ -n "$name" && "$name" != \#* ]] && names+=("$name")
            done < "$conf"
        fi
        echo "${names[@]}"
    }

    case "$prev" in
        logs|kill)
            COMPREPLY=($(compgen -W "$(_sipag_task_ids)" -- "$cur"))
            return ;;
        show|retry)
            COMPREPLY=($(compgen -W "$(_sipag_task_ids)" -- "$cur"))
            return ;;
        --repo)
            # Check which subcommand is active
            local i
            for ((i=1; i < ${#words[@]} - 1; i++)); do
                if [[ "${words[$i]}" == "add" ]]; then
                    COMPREPLY=($(compgen -W "$(_sipag_repo_names)" -- "$cur"))
                    return
                fi
            done ;;
        --priority)
            COMPREPLY=($(compgen -W "low medium high critical" -- "$cur"))
            return ;;
        repo)
            COMPREPLY=($(compgen -W "add list" -- "$cur"))
            return ;;
        completions)
            COMPREPLY=($(compgen -W "bash zsh fish" -- "$cur"))
            return ;;
    esac

    # Check active subcommand for flag completion
    local i
    for ((i=1; i < ${#words[@]} - 1; i++)); do
        case "${words[$i]}" in
            run)
                COMPREPLY=($(compgen -W "--repo --issue --background -b" -- "$cur"))
                return ;;
            add)
                COMPREPLY=($(compgen -W "--repo --priority" -- "$cur"))
                return ;;
            list|next)
                COMPREPLY=($(compgen -W "--file -f --continue -c --dry-run -n" -- "$cur"))
                return ;;
        esac
    done

    COMPREPLY=($(compgen -W "$commands" -- "$cur"))
}
complete -F _sipag sipag
"#;

const ZSH_COMPLETION: &str = r##"#compdef sipag

_sipag() {
    local state
    local -a commands

    commands=(
        'tui:Launch interactive TUI'
        'run:Launch a Docker sandbox for a task'
        'ps:List running and recent tasks'
        'logs:Print the log for a task'
        'kill:Kill a running container'
        'start:Process queue serially'
        'init:Create ~/.sipag directories'
        'add:Queue a task'
        'list:Print all tasks with status'
        'next:Find first pending task and run'
        'show:Print task file and log'
        'retry:Move a failed task back to queue'
        'repo:Manage the repo registry'
        'status:Show queue state across all directories'
        'version:Print version'
        'completions:Print shell completion script'
    )

    _sipag_task_ids() {
        local sipag_dir="${SIPAG_DIR:-$HOME/.sipag}"
        local -a ids
        local dir f
        for dir in running done failed queue; do
            local d="$sipag_dir/$dir"
            if [[ -d "$d" ]]; then
                for f in "$d"/*.md; do
                    [[ -f "$f" ]] && ids+=("${f:t:r}")
                done
            fi
        done
        compadd "$@" -- "${ids[@]}"
    }

    _sipag_repo_names() {
        local sipag_dir="${SIPAG_DIR:-$HOME/.sipag}"
        local conf="$sipag_dir/repos.conf"
        local -a names
        if [[ -f "$conf" ]]; then
            while IFS='=' read -r name _; do
                [[ -n "$name" && "$name" != \#* ]] && names+=("$name")
            done < "$conf"
        fi
        compadd "$@" -- "${names[@]}"
    }

    _arguments -C \
        '1: :->command' \
        '*: :->args' && return

    case "$state" in
        command)
            _describe 'command' commands ;;
        args)
            case "${words[2]}" in
                logs|kill|show|retry)
                    _sipag_task_ids ;;
                repo)
                    local -a repo_cmds
                    repo_cmds=(
                        'add:Register a repo name'
                        'list:List registered repos'
                    )
                    if (( CURRENT == 3 )); then
                        _describe 'repo command' repo_cmds
                    fi ;;
                run)
                    _arguments \
                        '--repo[Repository URL]:url:' \
                        '--issue[GitHub issue number]:number:' \
                        '(-b --background)'{-b,--background}'[Run in background]' \
                        ':description:' ;;
                add)
                    _arguments \
                        '--repo[Repository name]:name:_sipag_repo_names' \
                        '--priority[Priority level]:priority:(low medium high critical)' \
                        ':title:' ;;
                list|next)
                    _arguments \
                        '(-f --file)'{-f,--file}'[Task file path]:file:_files' \
                        '(-c --continue)'{-c,--continue}'[Loop to next task]' \
                        '(-n --dry-run)'{-n,--dry-run}'[Show what would run]' ;;
                completions)
                    compadd bash zsh fish ;;
            esac ;;
    esac
}

_sipag "$@"
"##;

const FISH_COMPLETION: &str = r#"# sipag completions for fish
# Install: sipag completions fish > ~/.config/fish/completions/sipag.fish

function __sipag_task_ids
    set sipag_dir (set -q SIPAG_DIR && echo $SIPAG_DIR || echo "$HOME/.sipag")
    for dir in running done failed queue
        if test -d "$sipag_dir/$dir"
            ls "$sipag_dir/$dir" 2>/dev/null | string replace -r '\.md$' ''
        end
    end
end

function __sipag_repo_names
    set sipag_dir (set -q SIPAG_DIR && echo $SIPAG_DIR || echo "$HOME/.sipag")
    set conf "$sipag_dir/repos.conf"
    if test -f "$conf"
        grep -v '^#' "$conf" 2>/dev/null | string replace -r '=.*$' ''
    end
end

function __sipag_no_subcommand
    for i in (commandline -opc)
        switch $i
            case tui run ps logs kill start init add list next show retry repo status version completions
                return 1
        end
    end
    return 0
end

# Main commands
complete -c sipag -n __sipag_no_subcommand -f -a tui         -d 'Launch interactive TUI'
complete -c sipag -n __sipag_no_subcommand -f -a run         -d 'Launch a Docker sandbox for a task'
complete -c sipag -n __sipag_no_subcommand -f -a ps          -d 'List running and recent tasks'
complete -c sipag -n __sipag_no_subcommand -f -a logs        -d 'Print the log for a task'
complete -c sipag -n __sipag_no_subcommand -f -a kill        -d 'Kill a running container'
complete -c sipag -n __sipag_no_subcommand -f -a start       -d 'Process queue serially'
complete -c sipag -n __sipag_no_subcommand -f -a init        -d 'Create ~/.sipag directories'
complete -c sipag -n __sipag_no_subcommand -f -a add         -d 'Queue a task'
complete -c sipag -n __sipag_no_subcommand -f -a list        -d 'Print all tasks with status'
complete -c sipag -n __sipag_no_subcommand -f -a next        -d 'Find first pending task and run'
complete -c sipag -n __sipag_no_subcommand -f -a show        -d 'Print task file and log'
complete -c sipag -n __sipag_no_subcommand -f -a retry       -d 'Move a failed task back to queue'
complete -c sipag -n __sipag_no_subcommand -f -a repo        -d 'Manage the repo registry'
complete -c sipag -n __sipag_no_subcommand -f -a status      -d 'Show queue state across all directories'
complete -c sipag -n __sipag_no_subcommand -f -a version     -d 'Print version'
complete -c sipag -n __sipag_no_subcommand -f -a completions -d 'Print shell completion script'

# Task ID completions for logs, kill, show, retry
complete -c sipag -n '__fish_seen_subcommand_from logs kill show retry' -f -a '(__sipag_task_ids)'

# repo subcommands
complete -c sipag -n '__fish_seen_subcommand_from repo' -f -a add  -d 'Register a repo name'
complete -c sipag -n '__fish_seen_subcommand_from repo' -f -a list -d 'List registered repos'

# run flags
complete -c sipag -n '__fish_seen_subcommand_from run' -l repo       -d 'Repository URL'
complete -c sipag -n '__fish_seen_subcommand_from run' -l issue      -d 'GitHub issue number'
complete -c sipag -n '__fish_seen_subcommand_from run' -s b -l background -d 'Run in background'

# add flags
complete -c sipag -n '__fish_seen_subcommand_from add' -l repo     -d 'Repository name' -f -a '(__sipag_repo_names)'
complete -c sipag -n '__fish_seen_subcommand_from add' -l priority -d 'Priority level' -f -a 'low medium high critical'

# list and next flags
complete -c sipag -n '__fish_seen_subcommand_from list next' -s f -l file     -d 'Task file path' -r
complete -c sipag -n '__fish_seen_subcommand_from next'      -s c -l continue -d 'Loop to next task'
complete -c sipag -n '__fish_seen_subcommand_from next'      -s n -l dry-run  -d 'Show what would run'

# completions argument
complete -c sipag -n '__fish_seen_subcommand_from completions' -f -a 'bash zsh fish'
"#;

fn cmd_status() -> Result<()> {
    let dir = sipag_dir();
    let labels = [
        ("Queue", "queue"),
        ("Running", "running"),
        ("Done", "done"),
        ("Failed", "failed"),
    ];

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
