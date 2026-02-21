//! Docker container dispatch for issue and PR workers.
//!
//! Implements `worker_run_issue`, `worker_run_pr_iteration`, and
//! `worker_run_conflict_fix` from `lib/worker/docker.sh` in Rust.

use anyhow::{Context, Result};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use super::github;
use super::ports::StateStore;
use super::state::WorkerState;
use super::status::WorkerStatus;
use super::store::FileStateStore;
use crate::config::WorkerConfig;
use crate::task::slugify;

// ── Prompt templates (embedded at compile time) ───────────────────────────────

const WORKER_ISSUE_PROMPT: &str = include_str!("../../../lib/prompts/worker-issue.md");
const WORKER_ITERATION_PROMPT: &str = include_str!("../../../lib/prompts/worker-iteration.md");
const WORKER_CONFLICT_FIX_PROMPT: &str =
    include_str!("../../../lib/prompts/worker-conflict-fix.md");

// ── Container bash scripts (run inside the Docker container) ─────────────────
//
// Each script lives in its own file under lib/container/ and is embedded at
// compile time via include_str!(). This is the single source of truth — the
// bash docker.sh scripts are a parallel legacy implementation that will be
// removed in Phase 2 of the Raptor refactor.
//
// All scripts expect these environment variables from `docker run -e`:
//   REPO, BRANCH, PROMPT, GH_TOKEN, CLAUDE_CODE_OAUTH_TOKEN, ANTHROPIC_API_KEY
// Issue worker also needs: ISSUE_TITLE, PR_BODY

const ISSUE_CONTAINER_SCRIPT: &str = include_str!("../../../lib/container/issue-worker.sh");
const ITERATION_CONTAINER_SCRIPT: &str = include_str!("../../../lib/container/iteration-worker.sh");
const CONFLICT_FIX_CONTAINER_SCRIPT: &str =
    include_str!("../../../lib/container/conflict-fix-worker.sh");

// ── Public helpers ────────────────────────────────────────────────────────────

/// Check whether a Docker container with the given name is currently running.
pub fn is_container_running(container_name: &str) -> bool {
    Command::new("docker")
        .args([
            "ps",
            "--filter",
            &format!("name=^{container_name}$"),
            "--format",
            "{{.Names}}",
        ])
        .output()
        .map(|o| {
            let text = String::from_utf8_lossy(&o.stdout);
            !text.trim().is_empty()
        })
        .unwrap_or(false)
}

// ── Issue worker ──────────────────────────────────────────────────────────────

/// Launch a Docker container to implement a GitHub issue.
///
/// Mirrors `worker_run_issue` in `lib/worker/docker.sh`:
/// 1. Write enqueued state (crash-safe).
/// 2. Transition label: `work_label` → `in-progress`.
/// 3. Fetch issue details.
/// 4. Build prompt from template.
/// 5. Run Docker container (blocking).
/// 6. Update state to `done` or `failed`.
/// 7. Transition label on completion.
pub fn dispatch_issue_worker(
    repo: &str,
    issue_num: u64,
    cfg: &WorkerConfig,
    sipag_dir: &Path,
    gh_token: Option<&str>,
    oauth_token: Option<&str>,
    api_key: Option<&str>,
) -> Result<()> {
    let repo_slug = repo.replace('/', "--");
    let log_dir = sipag_dir.join("logs");
    fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join(format!("{repo_slug}--{issue_num}.log"));

    let store = FileStateStore::new(sipag_dir);

    // 1. Write enqueued state immediately (crash-safe).
    let container_name = format!("sipag-issue-{issue_num}");
    let enqueued_state = WorkerState {
        repo: repo.to_string(),
        issue_num,
        issue_title: String::new(),
        branch: String::new(),
        container_name: container_name.clone(),
        pr_num: None,
        pr_url: None,
        status: WorkerStatus::Enqueued,
        started_at: Some(now_utc()),
        ended_at: None,
        duration_s: None,
        exit_code: None,
        log_path: Some(log_path.clone()),
        last_heartbeat: None,
        phase: None,
    };
    store.save(&enqueued_state)?;

    // 2. Transition label: work_label → in-progress.
    let _ = github::transition_label(repo, issue_num, Some(&cfg.work_label), Some("in-progress"));

    // 3. Fetch issue details.
    let (title, body) = github::get_issue_details(repo, issue_num)
        .unwrap_or_else(|_| (format!("Issue #{issue_num}"), String::new()));

    println!("[#{issue_num}] Starting: {title}");

    // 4. Build prompt and branch name.
    let slug: String = slugify(&title).chars().take(50).collect();
    let branch = format!("sipag/issue-{issue_num}-{slug}");

    let pr_body = format!(
        "Closes #{issue_num}\n\n{body}\n\n---\n*This PR was opened by a sipag worker. Commits will appear as work progresses.*"
    );

    let prompt = WORKER_ISSUE_PROMPT
        .replace("{{TITLE}}", &title)
        .replace("{{BODY}}", &body)
        .replace("{{BRANCH}}", &branch)
        .replace("{{ISSUE_NUM}}", &issue_num.to_string());

    // 5. Write running state.
    let started_at = now_utc();
    let running_state = WorkerState {
        repo: repo.to_string(),
        issue_num,
        issue_title: title.clone(),
        branch: branch.clone(),
        container_name: container_name.clone(),
        pr_num: None,
        pr_url: None,
        status: WorkerStatus::Running,
        started_at: Some(started_at.clone()),
        ended_at: None,
        duration_s: None,
        exit_code: None,
        log_path: Some(log_path.clone()),
        last_heartbeat: Some(started_at.clone()),
        phase: Some("starting container".to_string()),
    };
    store.save(&running_state)?;

    // 6. Run Docker container (blocking).
    let start = Instant::now();
    let workers_dir = sipag_dir.join("workers");
    let state_filename = format!("{repo_slug}--{issue_num}.json");
    let success = run_worker_container(
        &container_name,
        repo,
        &branch,
        &title,
        &pr_body,
        &prompt,
        &cfg.image,
        cfg.timeout.as_secs(),
        gh_token,
        oauth_token,
        api_key,
        ISSUE_CONTAINER_SCRIPT,
        &log_path,
        Some((&workers_dir, &state_filename)),
    );
    let duration_s = start.elapsed().as_secs() as i64;
    let ended_at = now_utc();

    // 7. Update state and manage labels.
    if success {
        // Find the PR that was created.
        let pr = github::find_pr_for_branch(repo, &branch).unwrap_or(None);

        // Remove in-progress label.
        let _ = github::transition_label(repo, issue_num, Some("in-progress"), None);

        let mut done_state = running_state.clone();
        done_state.status = WorkerStatus::Done;
        done_state.ended_at = Some(ended_at);
        done_state.duration_s = Some(duration_s);
        done_state.exit_code = Some(0);
        if let Some(ref p) = pr {
            done_state.pr_num = Some(p.number);
            done_state.pr_url = Some(p.url.clone());
        }
        store.save(&done_state)?;
        println!("[#{issue_num}] DONE: {title}");
    } else {
        // Return issue to work_label for retry.
        let _ =
            github::transition_label(repo, issue_num, Some("in-progress"), Some(&cfg.work_label));

        let mut fail_state = running_state.clone();
        fail_state.status = WorkerStatus::Failed;
        fail_state.ended_at = Some(ended_at);
        fail_state.duration_s = Some(duration_s);
        fail_state.exit_code = Some(1);
        store.save(&fail_state)?;
        println!(
            "[#{issue_num}] FAILED: {title} — returned to {}",
            cfg.work_label
        );
    }

    Ok(())
}

// ── PR iteration worker ───────────────────────────────────────────────────────

/// Launch a Docker container to iterate on a PR that needs changes.
///
/// Mirrors `worker_run_pr_iteration` in `lib/worker/docker.sh`.
pub fn dispatch_pr_iteration(
    repo: &str,
    pr_num: u64,
    cfg: &WorkerConfig,
    sipag_dir: &Path,
    gh_token: Option<&str>,
    oauth_token: Option<&str>,
    api_key: Option<&str>,
) -> Result<()> {
    let repo_slug = repo.replace('/', "--");
    let log_dir = sipag_dir.join("logs");
    fs::create_dir_all(&log_dir)?;

    // Fetch PR details.
    let pr_view = Command::new("gh")
        .args([
            "pr",
            "view",
            &pr_num.to_string(),
            "--repo",
            repo,
            "--json",
            "title,headRefName,body",
        ])
        .output()
        .context("Failed to run gh pr view")?;

    let pr_json: serde_json::Value =
        serde_json::from_slice(&pr_view.stdout).unwrap_or(serde_json::json!({}));
    let title = pr_json["title"].as_str().unwrap_or("").to_string();
    let branch = pr_json["headRefName"].as_str().unwrap_or("").to_string();
    let pr_body_text = pr_json["body"].as_str().unwrap_or("").to_string();

    if branch.is_empty() {
        anyhow::bail!("Could not determine branch for PR #{pr_num}");
    }

    println!("[PR #{pr_num}] Iterating: {title} (branch: {branch})");

    // Extract linked issue number from "Closes #N" in PR body.
    let issue_num = extract_issue_num_from_body(&pr_body_text);

    // Get original issue body if linked.
    let issue_body = if let Some(n) = issue_num {
        github::get_issue_details(repo, n)
            .ok()
            .map(|(_, body)| body)
            .unwrap_or_default()
    } else {
        String::new()
    };

    // Collect review feedback (CHANGES_REQUESTED + all comments).
    let review_feedback = collect_review_feedback(repo, pr_num);

    // Inline review comments.
    let inline_comments = collect_inline_comments(repo, pr_num);
    let full_feedback = if inline_comments.is_empty() {
        review_feedback
    } else if review_feedback.is_empty() {
        inline_comments
    } else {
        format!("{review_feedback}\n---\n{inline_comments}")
    };

    // Capture diff (capped to 50 KB).
    let pr_diff = get_pr_diff(repo, pr_num);

    let issue_body_display = if issue_body.is_empty() {
        "<not found>".to_string()
    } else {
        issue_body.clone()
    };

    let prompt = WORKER_ITERATION_PROMPT
        .replace("{{PR_NUM}}", &pr_num.to_string())
        .replace("{{REPO}}", repo)
        .replace("{{ISSUE_BODY}}", &issue_body_display)
        .replace("{{PR_DIFF}}", &pr_diff)
        .replace("{{REVIEW_FEEDBACK}}", &full_feedback)
        .replace("{{BRANCH}}", &branch);

    let log_path = log_dir.join(format!("{repo_slug}--pr-{pr_num}-iter.log"));
    let container_name = format!("sipag-pr-{pr_num}");

    let success = run_worker_container(
        &container_name,
        repo,
        &branch,
        &title,
        "",
        &prompt,
        &cfg.image,
        cfg.timeout.as_secs(),
        gh_token,
        oauth_token,
        api_key,
        ITERATION_CONTAINER_SCRIPT,
        &log_path,
        None,
    );

    if success {
        println!("[PR #{pr_num}] DONE iterating: {title}");
    } else {
        println!("[PR #{pr_num}] FAILED iteration: {title}");
    }

    Ok(())
}

// ── Conflict-fix worker ───────────────────────────────────────────────────────

/// Launch a Docker container to fix merge conflicts in a PR.
///
/// Merges `main` forward into the branch. If clean, pushes without Claude.
/// If conflicts, runs Claude to resolve. Mirrors `worker_run_conflict_fix`.
pub fn dispatch_conflict_fix(
    repo: &str,
    pr_num: u64,
    cfg: &WorkerConfig,
    sipag_dir: &Path,
    gh_token: Option<&str>,
    oauth_token: Option<&str>,
    api_key: Option<&str>,
) -> Result<()> {
    let repo_slug = repo.replace('/', "--");
    let log_dir = sipag_dir.join("logs");
    fs::create_dir_all(&log_dir)?;

    let pr_view = Command::new("gh")
        .args([
            "pr",
            "view",
            &pr_num.to_string(),
            "--repo",
            repo,
            "--json",
            "title,headRefName,body",
        ])
        .output()
        .context("Failed to run gh pr view")?;

    let pr_json: serde_json::Value =
        serde_json::from_slice(&pr_view.stdout).unwrap_or(serde_json::json!({}));
    let title = pr_json["title"].as_str().unwrap_or("").to_string();
    let branch = pr_json["headRefName"].as_str().unwrap_or("").to_string();
    let pr_body_text = pr_json["body"].as_str().unwrap_or("").to_string();

    if branch.is_empty() {
        anyhow::bail!("Could not determine branch for PR #{pr_num}");
    }

    println!("[PR #{pr_num}] Merging main forward: {title} (branch: {branch})");

    let prompt = WORKER_CONFLICT_FIX_PROMPT
        .replace("{{PR_NUM}}", &pr_num.to_string())
        .replace("{{PR_TITLE}}", &title)
        .replace("{{BRANCH}}", &branch)
        .replace("{{PR_BODY}}", &pr_body_text);

    let log_path = log_dir.join(format!("{repo_slug}--pr-{pr_num}-conflict-fix.log"));
    let container_name = format!("sipag-conflict-{pr_num}");

    let success = run_worker_container(
        &container_name,
        repo,
        &branch,
        &title,
        "",
        &prompt,
        &cfg.image,
        cfg.timeout.as_secs(),
        gh_token,
        oauth_token,
        api_key,
        CONFLICT_FIX_CONTAINER_SCRIPT,
        &log_path,
        None,
    );

    if success {
        println!("[PR #{pr_num}] Conflict fix done: {title}");
    } else {
        println!("[PR #{pr_num}] Conflict fix FAILED: {title}");
    }

    Ok(())
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn now_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Run the Docker container for a worker, streaming output to `log_path`.
///
/// When `state_mount` is `Some((host_workers_dir, state_filename))`, the host's
/// workers directory is bind-mounted into the container and `STATE_FILE` is set
/// so the container can self-report heartbeats, phases, and PR info.
///
/// Returns `true` on success (exit 0), `false` otherwise.
#[allow(clippy::too_many_arguments)]
fn run_worker_container(
    container_name: &str,
    repo: &str,
    branch: &str,
    issue_title: &str,
    pr_body: &str,
    prompt: &str,
    image: &str,
    timeout_secs: u64,
    gh_token: Option<&str>,
    oauth_token: Option<&str>,
    api_key: Option<&str>,
    script: &str,
    log_path: &PathBuf,
    state_mount: Option<(&Path, &str)>,
) -> bool {
    let log_out = match File::create(log_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("sipag: failed to create log {}: {e}", log_path.display());
            return false;
        }
    };
    let log_err = match log_out.try_clone() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("sipag: failed to clone log handle: {e}");
            return false;
        }
    };

    let timeout_bin = resolve_timeout_command();
    let mut cmd;
    if let Some(ref bin) = timeout_bin {
        cmd = Command::new(bin);
        cmd.arg(timeout_secs.to_string()).arg("docker").arg("run");
    } else {
        cmd = Command::new("docker");
        cmd.arg("run");
    }
    cmd.arg("--rm").arg("--name").arg(container_name);

    // Mount workers directory for state self-reporting.
    if let Some((workers_dir, state_filename)) = state_mount {
        cmd.arg("-v")
            .arg(format!("{}:/sipag-state", workers_dir.display()))
            .arg("-e")
            .arg(format!("STATE_FILE=/sipag-state/{state_filename}"));
    }

    cmd // Repository identity
        .arg("-e")
        .arg(format!("REPO={repo}"))
        // Branch and PR metadata (for the issue worker script)
        .arg("-e")
        .arg(format!("BRANCH={branch}"))
        .arg("-e")
        .arg(format!("ISSUE_TITLE={issue_title}"))
        .arg("-e")
        .arg(format!("PR_BODY={pr_body}"))
        // The Claude prompt
        .arg("-e")
        .arg(format!("PROMPT={prompt}"))
        // Pass credential env vars (values set below or inherited)
        .arg("-e")
        .arg("CLAUDE_CODE_OAUTH_TOKEN")
        .arg("-e")
        .arg("ANTHROPIC_API_KEY")
        .arg("-e")
        .arg("GH_TOKEN")
        .arg(image)
        .arg("bash")
        .arg("-c")
        .arg(script)
        .stdout(Stdio::from(log_out))
        .stderr(Stdio::from(log_err));

    // Set credentials as env vars on the child process.
    if let Some(token) = oauth_token {
        cmd.env("CLAUDE_CODE_OAUTH_TOKEN", token);
    }
    if let Some(key) = api_key {
        cmd.env("ANTHROPIC_API_KEY", key);
    }
    if let Some(token) = gh_token {
        cmd.env("GH_TOKEN", token);
    }

    cmd.status().map(|s| s.success()).unwrap_or(false)
}

/// Extract the first "Closes/Fixes/Resolves #N" issue number from text.
fn extract_issue_num_from_body(body: &str) -> Option<u64> {
    // Simple regex-free approach: scan for "closes #", "fixes #", "resolves #"
    for line in body.lines() {
        let lower = line.to_lowercase();
        for keyword in &["closes #", "fixes #", "resolves #"] {
            if let Some(pos) = lower.find(keyword) {
                let rest = &line[pos + keyword.len()..];
                let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(n) = num.parse::<u64>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

fn collect_review_feedback(repo: &str, pr_num: u64) -> String {
    let output = Command::new("gh")
        .args([
            "pr", "view", &pr_num.to_string(),
            "--repo", repo,
            "--json", "reviews,comments",
            "--jq",
            r#"([.reviews[] | select(.state == "CHANGES_REQUESTED") | "Review by \(.author.login):\n\(.body)"] +
               [.comments[] | "Comment by \(.author.login):\n\(.body)"]) | join("\n---\n")"#,
        ])
        .output();

    output
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn collect_inline_comments(repo: &str, pr_num: u64) -> String {
    let output = Command::new("gh")
        .args([
            "api",
            &format!("repos/{repo}/pulls/{pr_num}/comments"),
            "--jq",
            r#"[.[] | "Inline comment on \(.path) line \(.line // "?") by \(.user.login):\n\(.body)"] | join("\n---\n")"#,
        ])
        .output();

    output
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Find a working timeout command: `timeout` (Linux/coreutils) or `gtimeout` (macOS Homebrew).
/// Returns `None` if neither is available — the caller should run Docker without a timeout wrapper.
fn resolve_timeout_command() -> Option<String> {
    for bin in ["timeout", "gtimeout"] {
        if Command::new(bin)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
        {
            return Some(bin.to_string());
        }
    }
    eprintln!("sipag: warning: neither `timeout` nor `gtimeout` found — running without timeout");
    None
}

fn get_pr_diff(repo: &str, pr_num: u64) -> String {
    let output = Command::new("gh")
        .args(["pr", "diff", &pr_num.to_string(), "--repo", repo])
        .output();

    output
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            let text = String::from_utf8_lossy(&o.stdout);
            // Cap at 50 KB to avoid overwhelming the prompt.
            if text.len() > 50_000 {
                text[..50_000].to_string()
            } else {
                text.into_owned()
            }
        })
        .unwrap_or_default()
}
