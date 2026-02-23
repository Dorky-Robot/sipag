//! sipag-worker — container-side binary that replaces worker.sh.
//!
//! Runs inside the Docker container, imports sipag-core, and uses the same
//! `WorkerState` struct + `write_state()` as the host. This eliminates
//! field-name mismatches and argument-order bugs by construction.

use anyhow::{bail, Context, Result};
use sipag_core::state::{self, WorkerPhase};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Worker disposition prompt (embedded at compile time).
const WORKER_PROMPT: &str = include_str!("../../lib/prompts/worker.md");

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("sipag-worker: {e:#}");
            try_mark_failed(&format!("{e:#}"));
            1
        }
    };
    std::process::exit(code);
}

fn run() -> Result<i32> {
    let repo = required_env("REPO")?;
    let pr_num: u64 = required_env("PR_NUM")?
        .parse()
        .context("PR_NUM must be a positive integer")?;
    let branch = required_env("BRANCH")?;
    let state_file = required_env("STATE_FILE")?;

    let state_path = PathBuf::from(&state_file);

    // Phase: starting (state file already created by host dispatch).
    update_phase(&state_path, WorkerPhase::Starting)?;

    // Clone the repo and check out the PR branch.
    let gh_token = env::var("GH_TOKEN").unwrap_or_default();
    run_cmd(
        "git",
        &[
            "clone",
            &format!("https://x-access-token:{gh_token}@github.com/{repo}.git"),
            "/work",
        ],
    )?;
    run_cmd("git", &["-C", "/work", "config", "user.name", "sipag"])?;
    run_cmd(
        "git",
        &["-C", "/work", "config", "user.email", "sipag@localhost"],
    )?;
    run_cmd("git", &["-C", "/work", "fetch", "origin", &branch])?;
    run_cmd("git", &["-C", "/work", "checkout", &branch])?;

    // Read PR description as the assignment.
    let pr_body = get_pr_body(&repo, pr_num)?;

    // Read lessons from previous workers (if any).
    let lessons_section = read_lessons_file(&repo);

    // Phase: working.
    update_phase(&state_path, WorkerPhase::Working)?;

    // Build the prompt: PR description + lessons + worker disposition.
    let prompt = format!(
        "You are a sipag worker implementing a PR. The PR description below is your\n\
         complete assignment — it contains the architectural insight, approach, affected\n\
         issues, and constraints.\n\
         \n\
         --- PR DESCRIPTION ---\n\
         \n\
         {pr_body}\n\
         \n\
         --- END PR DESCRIPTION ---\n\
         \n\
         {lessons_section}\
         {WORKER_PROMPT}"
    );

    // Write prompt to a temp file to avoid argument size limits.
    let prompt_path = "/tmp/sipag-prompt.txt";
    std::fs::write(prompt_path, &prompt).context("failed to write prompt file")?;

    // Run Claude Code with full permissions from /work directory.
    let status = Command::new("claude")
        .args(["--dangerously-skip-permissions", "-p"])
        .arg(&prompt)
        .current_dir("/work")
        .status()
        .context("failed to run claude")?;

    let exit_code = status.code().unwrap_or(1);

    // Report completion.
    finish_state(&state_path, exit_code)?;

    Ok(exit_code)
}

/// Read a required environment variable.
fn required_env(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("missing required environment variable: {name}"))
}

/// Update the phase field in the state file.
fn update_phase(state_path: &Path, phase: WorkerPhase) -> Result<()> {
    let mut s = state::read_state(state_path).context("failed to read state file")?;
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    s.phase = phase;
    s.heartbeat = now;
    state::write_state(&s).context("failed to write state file")
}

/// Mark the worker as finished or failed and record the exit code.
fn finish_state(state_path: &Path, exit_code: i32) -> Result<()> {
    let mut s = state::read_state(state_path).context("failed to read state file")?;
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    s.phase = if exit_code == 0 {
        WorkerPhase::Finished
    } else {
        WorkerPhase::Failed
    };
    s.exit_code = Some(exit_code);
    s.ended = Some(now.clone());
    s.heartbeat = now;
    state::write_state(&s).context("failed to write state file")
}

/// Get the PR body via `gh pr view`.
fn get_pr_body(repo: &str, pr_num: u64) -> Result<String> {
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            &pr_num.to_string(),
            "--repo",
            repo,
            "--json",
            "body",
            "-q",
            ".body",
        ])
        .output()
        .context("failed to run gh pr view")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("gh pr view failed: {stderr}");
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run a command and bail on failure.
fn run_cmd(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to run {program}"))?;

    if !status.success() {
        bail!("{program} exited with code {}", status.code().unwrap_or(-1));
    }
    Ok(())
}

/// Read the lessons file for a repo from the mounted lessons directory.
///
/// Returns a formatted section to include in the prompt, or an empty string
/// if no lessons exist.
fn read_lessons_file(repo: &str) -> String {
    let repo_slug = repo.replace('/', "--");
    let path = format!("/sipag-lessons/{repo_slug}.md");
    match std::fs::read_to_string(&path) {
        Ok(content) if !content.trim().is_empty() => {
            format!(
                "## Lessons from previous workers\n\n\
                 Previous workers for this repo recorded the following lessons.\n\
                 Avoid repeating their mistakes:\n\n\
                 {}\n\n",
                content.trim()
            )
        }
        _ => String::new(),
    }
}

/// Best-effort attempt to mark the state file as failed on error.
fn try_mark_failed(error_msg: &str) {
    let state_file = match env::var("STATE_FILE") {
        Ok(f) => f,
        Err(_) => return,
    };
    let state_path = PathBuf::from(&state_file);
    if let Ok(mut s) = state::read_state(&state_path) {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        s.phase = WorkerPhase::Failed;
        s.exit_code = Some(1);
        s.ended = Some(now.clone());
        s.heartbeat = now;
        s.error = Some(error_msg.to_string());
        let _ = state::write_state(&s);
    }
}
