use anyhow::{bail, Context, Result};
use sipag_core::{
    config::{default_sipag_dir, WorkerConfig},
    docker,
    repo::{self, ResolvedRepo},
    worker::github,
};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

/// System prompt template (embedded at compile time).
const WORK_PROMPT: &str = include_str!("../../lib/prompts/work.md");

/// Run an interactive sipag work session.
///
/// Resolves each directory to a GitHub repo, fetches board state, builds a
/// system prompt, and execs into an interactive Claude session.
pub fn run_work(dirs: &[PathBuf]) -> Result<()> {
    let dirs = if dirs.is_empty() {
        vec![std::env::current_dir().context("failed to get current directory")?]
    } else {
        dirs.to_vec()
    };

    // Resolve each directory to a GitHub repo.
    let mut repos = Vec::new();
    for dir in &dirs {
        let resolved = repo::resolve_repo(dir)
            .with_context(|| format!("failed to resolve repo for {}", dir.display()))?;
        eprintln!("  {} → {}", dir.display(), resolved.full_name);
        repos.push(resolved);
    }

    // Preflight checks.
    let sipag_dir = default_sipag_dir();
    let cfg = WorkerConfig::load(&sipag_dir)?;
    github::preflight_gh_auth()?;
    docker::preflight_docker_running()?;
    docker::preflight_docker_image(&cfg.image)?;

    // Fetch board state per repo.
    eprintln!("Fetching board state...");
    let board_state = format_board_state(&repos)?;

    // Build system prompt.
    let system_prompt = WORK_PROMPT.replace("{BOARD_STATE}", &board_state);

    // Exec into claude.
    eprintln!("Launching Claude session...\n");
    exec_claude(&system_prompt)
}

/// Fetch and format board state for all repos.
fn format_board_state(repos: &[ResolvedRepo]) -> Result<String> {
    let mut sections = Vec::new();

    for repo in repos {
        let mut section = format!("### {} ({})\n", repo.full_name, repo.local_path.display());

        let issues = github::fetch_open_issues(&repo.full_name).unwrap_or_default();
        let prs = github::fetch_open_prs(&repo.full_name).unwrap_or_default();

        if issues.is_empty() && prs.is_empty() {
            section.push_str("\nNo open issues or PRs.\n");
        } else {
            if !issues.is_empty() {
                section.push_str("\n**Open issues:**\n");
                for issue in &issues {
                    let labels = if issue.labels.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", issue.labels.join(", "))
                    };
                    section.push_str(&format!("- #{} {}{}\n", issue.number, issue.title, labels));
                }
            }

            if !prs.is_empty() {
                section.push_str("\n**Open PRs:**\n");
                for pr in &prs {
                    let labels = if pr.labels.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", pr.labels.join(", "))
                    };
                    section.push_str(&format!("- PR #{} {}{}\n", pr.number, pr.title, labels));
                }
            }
        }

        sections.push(section);
    }

    Ok(sections.join("\n"))
}

/// Replace the current process with an interactive Claude session.
fn exec_claude(system_prompt: &str) -> Result<()> {
    let err = Command::new("claude")
        .arg("--append-system-prompt")
        .arg(system_prompt)
        .exec();

    // exec() only returns on error.
    bail!("failed to exec claude: {err}")
}
