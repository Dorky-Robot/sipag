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

    // Sanity check: verify the working tree has a reasonable number of files.
    // A branch created from a broken tree (e.g., API error dropping base_tree)
    // could have nearly zero files. Operating on such a branch would generate
    // a PR that deletes the entire codebase.
    let file_count = count_tracked_files();
    if file_count < 5 {
        bail!(
            "working tree sanity check failed: only {file_count} tracked files found. \
             Expected a full checkout. The branch may have been created incorrectly."
        );
    }

    // Read PR description as the assignment.
    let pr_body = get_pr_body(&repo, pr_num)?;

    // Read lessons from previous workers (if any).
    let lessons_section = read_lessons_file(&repo);

    // Phase: working.
    update_phase(&state_path, WorkerPhase::Working)?;

    // Build the prompt: PR description + lessons + worker disposition.
    // Replace placeholders in the worker prompt with actual values.
    let worker_prompt = WORKER_PROMPT
        .replace("{BRANCH}", &branch)
        .replace("{PR_NUM}", &pr_num.to_string())
        .replace("{REPO}", &repo);

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
         {worker_prompt}"
    );

    // Capture HEAD sha before Claude runs for push verification.
    let pre_claude_sha = get_head_sha().unwrap_or_default();

    // Write prompt to a file and pipe via stdin to avoid OS argument size limits.
    // Passing large prompts via .arg() hits ARG_MAX (~128KB on Linux), causing
    // silent E2BIG failures.
    let prompt_path = "/tmp/sipag-prompt.txt";
    std::fs::write(prompt_path, &prompt).context("failed to write prompt file")?;

    // Run Claude Code with full permissions from /work directory.
    // Pipe prompt via stdin to avoid ARG_MAX limits on the command line.
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new("claude")
        .args(["--dangerously-skip-permissions", "-p", "-"])
        .current_dir("/work")
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to spawn claude")?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(prompt.as_bytes())
        .context("failed to write prompt to claude stdin")?;
    let status = child.wait().context("failed to wait for claude")?;

    let exit_code = status.code().unwrap_or(1);

    // Post-run verification: check if commits were actually pushed.
    if exit_code == 0 {
        let pushed = verify_commits_pushed(&branch, &pre_claude_sha);
        if !pushed {
            eprintln!("sipag-worker: claude exited 0 but no commits were pushed to {branch}");
            let mut s = state::read_state(&state_path).context("failed to read state file")?;
            let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
            s.phase = WorkerPhase::Failed;
            s.exit_code = Some(1);
            s.ended = Some(now.clone());
            s.heartbeat = now;
            s.error =
                Some("no_changes_pushed: claude exited 0 but no commits were pushed".to_string());
            state::write_state(&s).context("failed to write state file")?;
            return Ok(1);
        }
    }

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

/// Count the number of tracked files in the /work checkout.
fn count_tracked_files() -> usize {
    Command::new("git")
        .args(["-C", "/work", "ls-files"])
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.is_empty())
                .count()
        })
        .unwrap_or(0)
}

/// Get the current HEAD SHA in /work.
fn get_head_sha() -> Option<String> {
    Command::new("git")
        .args(["-C", "/work", "rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// Check whether Claude made any commits by comparing HEAD before and after.
///
/// Also checks for unpushed local commits — if Claude committed but didn't push,
/// that's a failure we need to catch.
fn verify_commits_pushed(branch: &str, pre_sha: &str) -> bool {
    // If we couldn't get the pre-SHA, assume ok (don't block on verification failures).
    if pre_sha.is_empty() {
        return true;
    }

    let post_sha = get_head_sha().unwrap_or_default();
    if post_sha.is_empty() || post_sha == pre_sha {
        // HEAD didn't change — Claude made no commits.
        return false;
    }

    // HEAD changed, but check if commits were actually pushed.
    let unpushed = Command::new("git")
        .args([
            "-C",
            "/work",
            "log",
            &format!("origin/{branch}..HEAD"),
            "--oneline",
        ])
        .output();

    if let Ok(o) = unpushed {
        let out = String::from_utf8_lossy(&o.stdout);
        if !out.trim().is_empty() {
            eprintln!("sipag-worker: found unpushed commits:\n{}", out.trim());
            return false;
        }
    }

    true
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
