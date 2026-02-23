//! sipag-worker — container-side binary that replaces worker.sh.
//!
//! Runs inside the Docker container, imports sipag-core, and uses the same
//! `WorkerState` struct + `write_state()` as the host. This eliminates
//! field-name mismatches and argument-order bugs by construction.

use anyhow::{bail, Context, Result};
use sipag_core::state::{self, WorkerPhase};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

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

/// Emit a lifecycle event to the mounted events directory (best-effort).
fn emit_event(event_type: &str, repo: &str, pr_num: u64, detail: &str) {
    let events_dir = env::var("EVENTS_DIR").unwrap_or_default();
    if events_dir.is_empty() {
        return;
    }
    let _ = sipag_core::events::write_event_to(
        Path::new(&events_dir),
        event_type,
        repo,
        &format!("{event_type}: PR #{pr_num} in {repo}"),
        detail,
    );
}

/// Write a heartbeat file alongside the state file.
///
/// The file's mtime is the primary liveness signal. Contents are JSON for debugging.
fn write_heartbeat(state_path: &Path, repo: &str, pr_num: u64) {
    let heartbeat_path = state_path.with_extension("heartbeat");
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let content = serde_json::json!({
        "repo": repo,
        "pr_num": pr_num,
        "sub_phase": "working",
        "timestamp": timestamp,
        "pid": std::process::id(),
    });
    let _ = fs::write(&heartbeat_path, content.to_string());
}

/// Remove the heartbeat file on exit.
fn remove_heartbeat(state_path: &Path) {
    let heartbeat_path = state_path.with_extension("heartbeat");
    let _ = fs::remove_file(&heartbeat_path);
}

/// Spawn a background thread that writes the heartbeat file at a fixed interval.
///
/// Returns the shutdown flag — set it to `true` to stop the thread.
fn spawn_heartbeat_thread(
    state_path: PathBuf,
    repo: String,
    pr_num: u64,
    interval_secs: u64,
) -> Arc<AtomicBool> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();

    thread::spawn(move || {
        while !shutdown_clone.load(Ordering::Relaxed) {
            write_heartbeat(&state_path, &repo, pr_num);
            // Sleep in small increments so we notice shutdown quickly.
            for _ in 0..interval_secs {
                if shutdown_clone.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_secs(1));
            }
        }
    });

    shutdown
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
    // Scrub the token from the stored remote URL so it's not visible in
    // `git remote -v`, /proc/PID/cmdline, or Claude's output.
    run_cmd(
        "git",
        &[
            "-C",
            "/work",
            "remote",
            "set-url",
            "origin",
            &format!("https://github.com/{repo}.git"),
        ],
    )?;
    // Configure credential helper so push still works without the token in the URL.
    let credential_helper = format!(
        "!f() {{ echo \"protocol=https\nhost=github.com\nusername=x-access-token\npassword={gh_token}\"; }}; f"
    );
    run_cmd(
        "git",
        &[
            "-C",
            "/work",
            "config",
            "credential.helper",
            &credential_helper,
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
    emit_event(
        "worker-started",
        &repo,
        pr_num,
        "Worker entered working phase",
    );

    // Start heartbeat thread.
    let heartbeat_interval: u64 = env::var("SIPAG_HEARTBEAT_INTERVAL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    write_heartbeat(&state_path, &repo, pr_num); // immediate first heartbeat
    let heartbeat_shutdown =
        spawn_heartbeat_thread(state_path.clone(), repo.clone(), pr_num, heartbeat_interval);

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
    let pre_claude_sha =
        get_head_sha().context("failed to get HEAD SHA — git state may be corrupt")?;

    // Check if a host session file was mounted for --resume.
    let session_id = setup_session_resume();

    // Run Claude Code with full permissions from /work directory.
    let exit_code = spawn_claude(&session_id, &prompt)?;

    // If --resume failed, fall back to a fresh session.
    let exit_code = if exit_code != 0 && session_id.is_some() {
        eprintln!("sipag-worker: --resume failed (exit {exit_code}), retrying without resume");
        spawn_claude(&None, &prompt)?
    } else {
        exit_code
    };

    // Stop heartbeat thread before writing final state.
    heartbeat_shutdown.store(true, Ordering::Relaxed);

    // Post-run verification: check if commits were actually pushed.
    // If the worker self-merged the PR, the branch is deleted on remote,
    // so we skip push verification (the merge is proof enough).
    if exit_code == 0 {
        let pushed = if is_pr_merged(&repo, pr_num) {
            true
        } else {
            verify_commits_pushed(&branch, &pre_claude_sha)
        };
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
            emit_event(
                "worker-failed",
                &repo,
                pr_num,
                "claude exited 0 but no commits were pushed",
            );
            remove_heartbeat(&state_path);
            return Ok(1);
        }
    }

    // Report completion.
    finish_state(&state_path, exit_code)?;
    if exit_code == 0 {
        emit_event(
            "worker-finished",
            &repo,
            pr_num,
            "Worker completed successfully",
        );
    } else {
        emit_event(
            "worker-failed",
            &repo,
            pr_num,
            &format!("claude exited with code {exit_code}"),
        );
    }
    remove_heartbeat(&state_path);

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
    if exit_code != 0 {
        s.error = Some(format!("claude exited with code {exit_code}"));
    }
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

    let body = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if body.is_empty() {
        bail!("PR #{pr_num} has an empty body — cannot proceed without an assignment");
    }
    Ok(body)
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
    let post_sha = get_head_sha().unwrap_or_default();
    if post_sha.is_empty() || post_sha == pre_sha {
        // HEAD didn't change — Claude made no commits.
        return false;
    }

    // Refresh remote tracking refs before checking for unpushed commits.
    // Without this, origin/{branch} may be stale from the initial clone.
    let _ = Command::new("git")
        .args(["-C", "/work", "fetch", "origin", branch])
        .stderr(std::process::Stdio::null())
        .output();

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

    match unpushed {
        Ok(o) if o.status.success() => {
            let out = String::from_utf8_lossy(&o.stdout);
            if !out.trim().is_empty() {
                eprintln!("sipag-worker: found unpushed commits:\n{}", out.trim());
                return false;
            }
            true
        }
        _ => {
            // Can't determine push status — fail safe rather than optimistic.
            eprintln!("sipag-worker: could not verify push status for origin/{branch}");
            false
        }
    }
}

/// Check if a PR has been merged on GitHub.
fn is_pr_merged(repo: &str, pr_num: u64) -> bool {
    Command::new("gh")
        .args([
            "pr",
            "view",
            &pr_num.to_string(),
            "--repo",
            repo,
            "--json",
            "state",
            "-q",
            ".state",
        ])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .is_some_and(|state| state == "MERGED")
}

/// Max bytes of lessons to include in the worker prompt.
/// Matches sipag_core::lessons::DEFAULT_MAX_BYTES (8KB ≈ 20 lessons).
const LESSONS_MAX_BYTES: usize = 8 * 1024;

/// Read the lessons file for a repo from the mounted lessons directory.
///
/// Truncates from the front if the file exceeds LESSONS_MAX_BYTES, cutting at
/// the nearest `## ` heading boundary so entries stay intact. This prevents old
/// lessons from bloating the prompt.
fn read_lessons_file(repo: &str) -> String {
    let repo_slug = repo.replace('/', "--");
    let path = format!("/sipag-lessons/{repo_slug}.md");
    match std::fs::read_to_string(&path) {
        Ok(content) if !content.trim().is_empty() => {
            let trimmed = if content.len() <= LESSONS_MAX_BYTES {
                content.trim().to_string()
            } else {
                // Truncate from front, keeping last LESSONS_MAX_BYTES at a heading boundary.
                let start = content.len() - LESSONS_MAX_BYTES;
                let tail = &content[start..];
                if let Some(pos) = tail.find("\n## ") {
                    tail[pos + 1..].trim().to_string()
                } else {
                    tail.trim().to_string()
                }
            };
            format!(
                "## Lessons from previous workers\n\n\
                 Previous workers for this repo recorded the following lessons.\n\
                 Avoid repeating their mistakes:\n\n\
                 {trimmed}\n\n",
            )
        }
        _ => String::new(),
    }
}

/// Spawn Claude Code with the given args and pipe the prompt via stdin.
///
/// Returns the exit code.
fn spawn_claude(session_id: &Option<String>, prompt: &str) -> Result<i32> {
    use std::io::Write;
    use std::process::Stdio;

    let mut claude_args = vec!["--dangerously-skip-permissions"];
    if let Some(ref id) = session_id {
        claude_args.extend(["--resume", id]);
    }
    claude_args.extend(["-p", "-"]);

    let mut child = Command::new("claude")
        .args(&claude_args)
        .current_dir("/work")
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to spawn claude")?;

    {
        let mut stdin = child.stdin.take().unwrap();
        if let Err(e) = stdin.write_all(prompt.as_bytes()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e).context("failed to write prompt to claude stdin");
        }
    }

    let status = child.wait().context("failed to wait for claude")?;
    Ok(status.code().unwrap_or(1))
}

/// Copy the host's session file into Claude's expected location so `--resume` works.
///
/// Returns the session ID if a session file was mounted, or None to fall back
/// to a fresh session.
fn setup_session_resume() -> Option<String> {
    let session_id = env::var("SIPAG_SESSION_ID").ok()?;
    let src = Path::new("/sipag-session/session.jsonl");
    if !src.exists() {
        return None;
    }

    let dest_dir = PathBuf::from("/home/sipag/.claude/projects/sipag-session");
    fs::create_dir_all(&dest_dir).ok()?;
    let dest = dest_dir.join(format!("{session_id}.jsonl"));
    fs::copy(src, &dest).ok()?;

    eprintln!("sipag-worker: resuming host session {session_id}");
    Some(session_id)
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
        emit_event("worker-failed", &s.repo, s.pr_num, error_msg);
        remove_heartbeat(&state_path);
    }
}
