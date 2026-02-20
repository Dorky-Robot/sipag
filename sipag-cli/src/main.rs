use anyhow::Result;
use clap::{Parser, Subcommand};
use sipag_core::{config::Config, executor, init, repo, task};

const VERSION: &str = "0.1.0";

#[derive(Parser)]
#[command(
    name = "sipag",
    about = "sandbox launcher for Claude Code",
    version = VERSION,
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Launch a Docker sandbox for a task
    Run {
        /// Repository URL to clone (required)
        #[arg(long)]
        repo: String,

        /// GitHub issue number to associate
        #[arg(long)]
        issue: Option<String>,

        /// Run in background
        #[arg(short = 'b', long)]
        background: bool,

        /// Task description
        task: String,
    },

    /// List running and recent tasks
    Ps,

    /// Print log for a task
    Logs {
        /// Task ID
        id: String,
    },

    /// Kill a running task container and move to failed/
    Kill {
        /// Task ID
        id: String,
    },

    /// Show queue state across all directories
    Status,

    /// Print task file and log (searches queue/running/done/failed)
    Show {
        /// Task name (without .md extension)
        name: String,
    },

    /// Move a task from failed/ back to queue/ for retry
    Retry {
        /// Task name (without .md extension)
        name: String,
    },

    /// Add a task to the queue
    Add {
        /// Task description
        text: String,

        /// Repository name (uses queue/ format with YAML frontmatter)
        #[arg(long)]
        repo: Option<String>,

        /// Priority level
        #[arg(long, default_value = "medium")]
        priority: String,
    },

    /// Manage registered repositories
    Repo {
        #[command(subcommand)]
        command: RepoCommands,
    },

    /// Create ~/.sipag/{queue,running,done,failed} directory structure
    Init,

    /// Process queue/ serially using Docker
    Start,

    /// Launch the TUI
    Tui,

    /// Print version
    Version,
}

#[derive(Subcommand)]
enum RepoCommands {
    /// Register a repository name → URL mapping
    Add {
        /// Repository name
        name: String,
        /// Repository URL
        url: String,
    },
    /// List all registered repositories
    List,
}

fn main() {
    let cli = Cli::parse();
    let config = Config::default();

    let result = match cli.command {
        None | Some(Commands::Version) => {
            println!("sipag {}", VERSION);
            Ok(())
        }
        Some(Commands::Init) => cmd_init(&config),
        Some(Commands::Start) => cmd_start(&config),
        Some(Commands::Run {
            repo,
            issue,
            background,
            task,
        }) => cmd_run(&repo, issue.as_deref(), background, &task, &config),
        Some(Commands::Ps) => executor::print_ps(&config.sipag_dir),
        Some(Commands::Logs { id }) => executor::print_logs(&id, &config.sipag_dir),
        Some(Commands::Kill { id }) => executor::kill_task(&id, &config),
        Some(Commands::Status) => executor::print_status(&config.sipag_dir),
        Some(Commands::Show { name }) => executor::show_task(&name, &config.sipag_dir),
        Some(Commands::Retry { name }) => executor::retry_task(&name, &config.sipag_dir),
        Some(Commands::Add {
            text,
            repo,
            priority,
        }) => cmd_add(&text, repo.as_deref(), &priority, &config),
        Some(Commands::Repo {
            command: RepoCommands::Add { name, url },
        }) => cmd_repo_add(&name, &url, &config),
        Some(Commands::Repo {
            command: RepoCommands::List,
        }) => cmd_repo_list(&config),
        Some(Commands::Tui) => cmd_tui(),
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn cmd_init(config: &Config) -> Result<()> {
    init::init_dirs(&config.sipag_dir)
}

fn cmd_start(config: &Config) -> Result<()> {
    init::init_dirs(&config.sipag_dir)?;
    println!("sipag executor starting (queue: {})", config.sipag_dir.join("queue").display());
    executor::run_queue(config)
}

fn cmd_run(
    repo_url: &str,
    issue: Option<&str>,
    background: bool,
    description: &str,
    config: &Config,
) -> Result<()> {
    // Auto-init if needed
    if !config.sipag_dir.join("running").exists() {
        init::init_dirs(&config.sipag_dir)?;
    }

    // Generate task ID: YYYYMMDDHHMMSS-slug (slug truncated to 30 chars)
    let slug = task::slugify(description);
    let slug_part = &slug[..slug.len().min(30)];
    let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();
    let task_id_raw = format!("{}-{}", timestamp, slug_part);
    // Strip trailing hyphen (matches bash: ${task_id%-})
    let task_id = task_id_raw.trim_end_matches('-').to_string();

    println!("Task ID: {}", task_id);

    executor::run_impl(&task_id, repo_url, description, issue, background, config)
}

fn cmd_add(text: &str, repo: Option<&str>, priority: &str, config: &Config) -> Result<()> {
    if let Some(repo_name) = repo {
        // Queue format: write YAML frontmatter file to queue/
        let queue_dir = config.sipag_dir.join("queue");
        if !queue_dir.exists() {
            init::init_dirs(&config.sipag_dir)?;
        }
        let filename = task::next_filename(&queue_dir, text)?;
        let path = queue_dir.join(&filename);
        task::write_task_file(&path, text, repo_name, priority, None)?;
        println!("Added: {}", text);
    } else {
        // Legacy format: append checklist item to SIPAG_FILE
        task::add_task(&config.sipag_file, text)?;
        println!("Added: {}", text);
    }
    Ok(())
}

fn cmd_repo_add(name: &str, url: &str, config: &Config) -> Result<()> {
    repo::repo_add(name, url, &config.sipag_dir)?;
    println!("Registered: {}={}", name, url);
    Ok(())
}

fn cmd_repo_list(config: &Config) -> Result<()> {
    let repos = repo::repo_list(&config.sipag_dir)?;
    if repos.is_empty() {
        println!("No repos registered. Use: sipag repo add <name> <url>");
    } else {
        for (name, url) in repos {
            println!("{}={}", name, url);
        }
    }
    Ok(())
}

fn cmd_tui() -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new("sipag-tui").exec();
        anyhow::bail!("failed to exec sipag-tui: {}", err);
    }

    #[cfg(not(unix))]
    {
        anyhow::bail!("the tui command is only supported on Unix systems");
    }
}
