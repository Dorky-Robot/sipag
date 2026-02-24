//! sipag-worker — container-side binary that replaces worker.sh.
//!
//! Runs inside the Docker container, imports sipag-core, and uses the same
//! `WorkerState` struct + `write_state()` as the host. This eliminates
//! field-name mismatches and argument-order bugs by construction.

use anyhow::{bail, Context, Result};
use sipag_core::state::{self, WorkerPhase};
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Worker disposition prompt (embedded at compile time).
const WORKER_PROMPT: &str = include_str!("../../lib/prompts/worker.md");

/// How often the supervision loop ticks (seconds).
const TICK_SECS: u64 = 10;

/// How often to check PR state on GitHub (seconds).
const PR_CHECK_INTERVAL_SECS: u64 = 300;

/// Grace period after PR is merged/closed before killing Claude (seconds).
const GRACE_PERIOD_SECS: u64 = 120;

/// PR state as reported by GitHub.
#[derive(Debug, PartialEq)]
enum PrState {
    Open,
    Merged,
    Closed,
    Unknown,
}

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
fn write_heartbeat(state_path: &Path, repo: &str, pr_num: u64, sub_phase: &str) {
    let heartbeat_path = state_path.with_extension("heartbeat");
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let content = serde_json::json!({
        "repo": repo,
        "pr_num": pr_num,
        "sub_phase": sub_phase,
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

/// Check the PR state on GitHub via `gh pr view`.
fn check_pr_state(repo: &str, pr_num: u64) -> PrState {
    let output = Command::new("gh")
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
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let state = String::from_utf8_lossy(&o.stdout).trim().to_string();
            match state.as_str() {
                "OPEN" => PrState::Open,
                "MERGED" => PrState::Merged,
                "CLOSED" => PrState::Closed,
                _ => PrState::Unknown,
            }
        }
        _ => PrState::Unknown,
    }
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

    // Clone the repo using a credential file so the token never appears in
    // process args (visible in `ps aux`, /proc/PID/cmdline).
    let gh_token = env::var("GH_TOKEN").unwrap_or_default();
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open("/tmp/.git-credentials")
            .context("failed to create /tmp/.git-credentials")?;
        writeln!(f, "https://x-access-token:{gh_token}@github.com")
            .context("failed to write git credentials")?;
    }
    run_cmd(
        "git",
        &[
            "config",
            "--global",
            "credential.helper",
            "store --file /tmp/.git-credentials",
        ],
    )?;
    run_cmd(
        "git",
        &["clone", &format!("https://github.com/{repo}.git"), "/work"],
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

    // Heartbeat configuration.
    let heartbeat_interval: u64 = env::var("SIPAG_HEARTBEAT_INTERVAL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    write_heartbeat(&state_path, &repo, pr_num, "working"); // immediate first heartbeat

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

    // Start Claude Code with full permissions from /work directory.
    let child = start_claude(&prompt)?;

    // Supervise Claude: heartbeats, PR state checks, grace period on merge/close.
    let exit_code = supervise_claude(child, &state_path, &repo, pr_num, heartbeat_interval)?;

    // Dump Claude's output log to stderr so it flows to the host log file.
    if let Ok(content) = fs::read_to_string("/tmp/claude-output.log") {
        eprint!("{content}");
    }

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
        .stderr(Stdio::null())
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
    check_pr_state(repo, pr_num) == PrState::Merged
}

/// Max bytes of lessons to include in the worker prompt.
/// Matches sipag_core::lessons::DEFAULT_MAX_BYTES (8KB ~ 20 lessons).
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

/// Spawn Claude Code and return the Child handle without waiting.
///
/// Redirects Claude's stdout and stderr to `/tmp/claude-output.log` inside the
/// container so the output can be dumped to the host log after Claude exits.
fn start_claude(prompt: &str) -> Result<Child> {
    let log_file =
        File::create("/tmp/claude-output.log").context("failed to create claude output log")?;
    let log_err = log_file
        .try_clone()
        .context("failed to clone log file handle")?;

    let mut child = Command::new("claude")
        .args(["--dangerously-skip-permissions", "-p", "-"])
        .current_dir("/work")
        .stdin(Stdio::piped())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_err))
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

    Ok(child)
}

/// Supervise a running Claude process.
///
/// Single loop with 10-second ticks that:
/// - Calls `child.try_wait()` each tick to detect natural exit
/// - Writes heartbeat every `heartbeat_interval` seconds
/// - Checks PR state on GitHub every 5 minutes
/// - On Merged/Closed: starts a 120-second grace period, then kills Claude
///
/// Returns the exit code.
fn supervise_claude(
    mut child: Child,
    state_path: &Path,
    repo: &str,
    pr_num: u64,
    heartbeat_interval: u64,
) -> Result<i32> {
    let start = Instant::now();
    let mut last_heartbeat = Instant::now();
    let mut last_pr_check = Instant::now();
    let mut grace_deadline: Option<Instant> = None;

    loop {
        std::thread::sleep(Duration::from_secs(TICK_SECS));

        // Check if Claude exited naturally.
        if let Some(status) = child.try_wait().context("failed to check claude status")? {
            return Ok(status.code().unwrap_or(1));
        }

        let now = Instant::now();

        // Write heartbeat at the configured interval.
        if now.duration_since(last_heartbeat).as_secs() >= heartbeat_interval {
            let sub_phase = if grace_deadline.is_some() {
                "grace_period"
            } else {
                "working"
            };
            write_heartbeat(state_path, repo, pr_num, sub_phase);
            last_heartbeat = now;
        }

        // Check if we're past the grace deadline.
        if let Some(deadline) = grace_deadline {
            if now >= deadline {
                eprintln!(
                    "sipag-worker: grace period expired after {}s, killing claude",
                    GRACE_PERIOD_SECS
                );
                let _ = child.kill();
                let status = child.wait().context("failed to reap claude after kill")?;
                return Ok(status.code().unwrap_or(0));
            }
            // During grace period, skip PR state checks (already decided to wind down).
            continue;
        }

        // Check PR state periodically.
        if now.duration_since(last_pr_check).as_secs() >= PR_CHECK_INTERVAL_SECS {
            last_pr_check = now;
            let pr_state = check_pr_state(repo, pr_num);
            match pr_state {
                PrState::Merged => {
                    eprintln!(
                        "sipag-worker: PR #{pr_num} merged, starting {GRACE_PERIOD_SECS}s grace period (running {}s)",
                        start.elapsed().as_secs()
                    );
                    write_heartbeat(state_path, repo, pr_num, "pr_merged");
                    grace_deadline = Some(now + Duration::from_secs(GRACE_PERIOD_SECS));
                }
                PrState::Closed => {
                    eprintln!(
                        "sipag-worker: PR #{pr_num} closed, starting {GRACE_PERIOD_SECS}s grace period (running {}s)",
                        start.elapsed().as_secs()
                    );
                    write_heartbeat(state_path, repo, pr_num, "pr_closed");
                    grace_deadline = Some(now + Duration::from_secs(GRACE_PERIOD_SECS));
                }
                PrState::Open | PrState::Unknown => {}
            }
        }
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
        emit_event("worker-failed", &s.repo, s.pr_num, error_msg);
        remove_heartbeat(&state_path);
    }
}
