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

const WORKER_PROMPT: &str = include_str!("../../../lib/prompts/worker-grouped.md");
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

// ── Issue context (shared between brainstorm and dispatch) ───────────────────

/// Pre-fetched issue context, built once in the poll loop and shared between
/// the brainstorm phase and worker dispatch. Avoids double-fetching from GitHub.
pub(crate) struct IssueContext {
    /// Formatted section with all open issues (full project landscape).
    pub all_issues_section: String,
    /// Formatted section with ready issues (candidates for this cycle).
    pub ready_issues_section: String,
    /// Title of the first (anchor) issue — used for branch naming.
    pub first_title: String,
}

/// Fetch issue context from GitHub, suitable for both brainstorm and dispatch.
///
/// When `context_issue_nums` is provided, only those issues are fetched for the
/// all-issues section (funnel-narrowed set). When `None`, falls back to fetching
/// all open issues (current behavior).
pub(crate) fn fetch_issue_context(
    repo: &str,
    issue_nums: &[u64],
    context_issue_nums: Option<&[u64]>,
) -> IssueContext {
    assert!(!issue_nums.is_empty(), "issue_nums must not be empty");

    // Build all_issues_section from either the narrowed set or all open issues.
    let mut all_issues_section = String::new();
    match context_issue_nums {
        Some(nums) => {
            for &num in nums {
                let (title, body) = github::get_issue_details(repo, num)
                    .unwrap_or_else(|_| (format!("Issue #{num}"), String::new()));
                all_issues_section.push_str(&format!("### Issue #{num}: {title}\n\n{body}\n\n"));
            }
        }
        None => {
            let all_open = github::list_all_open_issues(repo).unwrap_or_default();
            for (num, title, body) in &all_open {
                all_issues_section.push_str(&format!("### Issue #{num}: {title}\n\n{body}\n\n"));
            }
        }
    }

    // Fetch details for ready issues (candidates for this cycle).
    let mut ready_issues_section = String::new();
    let mut first_title = String::new();
    for &issue_num in issue_nums {
        let (title, body) = github::get_issue_details(repo, issue_num)
            .unwrap_or_else(|_| (format!("Issue #{issue_num}"), String::new()));
        if first_title.is_empty() {
            first_title = title.clone();
        }
        ready_issues_section.push_str(&format!("### Issue #{issue_num}: {title}\n\n{body}\n\n"));
    }

    IssueContext {
        all_issues_section,
        ready_issues_section,
        first_title,
    }
}

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

/// Launch a single Docker container to address one or more GitHub issues.
///
/// The worker receives the full project landscape (all open issues) as context
/// and the ready issues as candidates. Claude decides which to address in one
/// cohesive PR. Issues not addressed (no `Closes #N` in the PR body) stay
/// `ready` for the next polling cycle.
///
/// When `brainstorm_plan` is provided, it's injected into the worker prompt
/// via the `{{BRAINSTORM_PLAN}}` placeholder.
#[allow(clippy::too_many_arguments)]
pub fn dispatch_worker(
    repo: &str,
    issue_nums: &[u64],
    cfg: &WorkerConfig,
    sipag_dir: &Path,
    gh_token: Option<&str>,
    oauth_token: Option<&str>,
    api_key: Option<&str>,
    issue_ctx: Option<&IssueContext>,
    brainstorm_plan: Option<&str>,
) -> Result<()> {
    assert!(!issue_nums.is_empty(), "issue_nums must not be empty");

    let repo_slug = repo.replace('/', "--");
    let log_dir = sipag_dir.join("logs");
    fs::create_dir_all(&log_dir)?;

    let anchor_num = issue_nums[0];
    let container_name = if issue_nums.len() == 1 {
        format!("sipag-issue-{anchor_num}")
    } else {
        format!("sipag-group-{anchor_num}")
    };
    let log_path = if issue_nums.len() == 1 {
        log_dir.join(format!("{repo_slug}--{anchor_num}.log"))
    } else {
        log_dir.join(format!("{repo_slug}--group-{anchor_num}.log"))
    };

    let store = FileStateStore::new(sipag_dir);

    // 1. Write enqueued state for each issue (crash-safe).
    let started_at = now_utc();
    for &issue_num in issue_nums {
        let enqueued_state = WorkerState {
            repo: repo.to_string(),
            issue_num,
            issue_title: String::new(),
            branch: String::new(),
            container_name: container_name.clone(),
            pr_num: None,
            pr_url: None,
            status: WorkerStatus::Enqueued,
            started_at: Some(started_at.clone()),
            ended_at: None,
            duration_s: None,
            exit_code: None,
            log_path: Some(log_path.clone()),
            last_heartbeat: None,
            phase: None,
        };
        store.save(&enqueued_state)?;
    }

    // 2. Fetch issue context (or reuse pre-fetched context from poll loop).
    let owned_ctx;
    let ctx = match issue_ctx {
        Some(c) => c,
        None => {
            owned_ctx = fetch_issue_context(repo, issue_nums, None);
            &owned_ctx
        }
    };

    let all_issues_section = &ctx.all_issues_section;
    let ready_issues_section = &ctx.ready_issues_section;
    let first_title = &ctx.first_title;

    // 3. Build branch name and prompt.
    let slug: String = slugify(first_title).chars().take(50).collect();
    let branch = if issue_nums.len() == 1 {
        format!("sipag/issue-{anchor_num}-{slug}")
    } else {
        format!("sipag/group-{anchor_num}-{slug}")
    };

    let issue_refs: Vec<String> = issue_nums.iter().map(|n| format!("#{n}")).collect();
    let pr_title = if issue_nums.len() == 1 {
        format!("sipag: {first_title}")
    } else {
        format!("sipag: address issues {}", issue_refs.join(", "))
    };
    let pr_body_closes: Vec<String> = issue_nums.iter().map(|n| format!("Closes #{n}")).collect();
    let pr_body = format!(
        "{}\n\n---\n*This PR was opened by a sipag worker. Claude will update `Closes` references based on which issues are actually addressed.*",
        pr_body_closes.join("\n")
    );

    let brainstorm_section = brainstorm_plan
        .map(super::brainstorm::format_brainstorm_section)
        .unwrap_or_default();

    let prompt = WORKER_PROMPT
        .replace("{{ALL_ISSUES}}", all_issues_section)
        .replace("{{READY_ISSUES}}", ready_issues_section)
        .replace("{{BRAINSTORM_PLAN}}", &brainstorm_section)
        .replace("{{BRANCH}}", &branch);

    let issue_nums_str: Vec<String> = issue_nums.iter().map(|n| n.to_string()).collect();
    let issue_nums_env = issue_nums_str.join(" ");

    println!(
        "[group] Starting worker for issues {}: anchor #{anchor_num}",
        issue_refs.join(", ")
    );

    // 4. Write running state for each issue.
    for &issue_num in issue_nums {
        let (title, _) = github::get_issue_details(repo, issue_num)
            .unwrap_or_else(|_| (format!("Issue #{issue_num}"), String::new()));
        let running_state = WorkerState {
            repo: repo.to_string(),
            issue_num,
            issue_title: title,
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
    }

    // 5. Run Docker container (blocking).
    let start = Instant::now();
    let workers_dir = sipag_dir.join("workers");
    // Use anchor issue's state file for container self-reporting.
    let state_filename = format!("{repo_slug}--{anchor_num}.json");
    let success = run_worker_container(
        &container_name,
        repo,
        &branch,
        &pr_title,
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
        &[
            ("ISSUE_NUMS", &issue_nums_env),
            ("WORK_LABEL", &cfg.work_label),
        ],
    );
    let duration_s = start.elapsed().as_secs() as i64;
    let ended_at = now_utc();

    // 6. Update state files. Label transitions are handled by the container.
    if success {
        let pr = github::find_pr_for_branch(repo, &branch).unwrap_or(None);

        // Parse the PR body to determine which issues were addressed.
        let addressed = if let Some(ref p) = pr {
            extract_all_issue_nums_from_pr(repo, p.number)
        } else {
            vec![]
        };

        for &issue_num in issue_nums {
            let (title, _) = github::get_issue_details(repo, issue_num)
                .unwrap_or_else(|_| (format!("Issue #{issue_num}"), String::new()));

            if addressed.contains(&issue_num) {
                let mut done_state = WorkerState {
                    repo: repo.to_string(),
                    issue_num,
                    issue_title: title.clone(),
                    branch: branch.clone(),
                    container_name: container_name.clone(),
                    pr_num: None,
                    pr_url: None,
                    status: WorkerStatus::Done,
                    started_at: Some(started_at.clone()),
                    ended_at: Some(ended_at.clone()),
                    duration_s: Some(duration_s),
                    exit_code: Some(0),
                    log_path: Some(log_path.clone()),
                    last_heartbeat: None,
                    phase: None,
                };
                if let Some(ref p) = pr {
                    done_state.pr_num = Some(p.number);
                    done_state.pr_url = Some(p.url.clone());
                }
                store.save(&done_state)?;
                println!("[#{issue_num}] DONE: {title}");
            } else {
                let fail_state = WorkerState {
                    repo: repo.to_string(),
                    issue_num,
                    issue_title: title.clone(),
                    branch: branch.clone(),
                    container_name: container_name.clone(),
                    pr_num: None,
                    pr_url: None,
                    status: WorkerStatus::Failed,
                    started_at: Some(started_at.clone()),
                    ended_at: Some(ended_at.clone()),
                    duration_s: Some(duration_s),
                    exit_code: Some(0),
                    log_path: Some(log_path.clone()),
                    last_heartbeat: None,
                    phase: Some("not addressed in grouped PR".to_string()),
                };
                store.save(&fail_state)?;
                println!("[#{issue_num}] Not addressed — will retry next cycle");
            }
        }
    } else {
        let failure_reason = extract_failure_reason(&log_path);

        // Surface clone failures prominently so they're visible without digging through logs.
        if let Some(ref reason) = failure_reason {
            println!("[group]   Hint: {reason}");
            if let Some(stem) = log_path.file_stem().and_then(|s| s.to_str()) {
                println!("[group]   Full log: sipag logs {stem}");
            }
        }

        for &issue_num in issue_nums {
            let (title, _) = github::get_issue_details(repo, issue_num)
                .unwrap_or_else(|_| (format!("Issue #{issue_num}"), String::new()));
            let fail_state = WorkerState {
                repo: repo.to_string(),
                issue_num,
                issue_title: title.clone(),
                branch: branch.clone(),
                container_name: container_name.clone(),
                pr_num: None,
                pr_url: None,
                status: WorkerStatus::Failed,
                started_at: Some(started_at.clone()),
                ended_at: Some(ended_at.clone()),
                duration_s: Some(duration_s),
                exit_code: Some(1),
                log_path: Some(log_path.clone()),
                last_heartbeat: None,
                phase: failure_reason.clone(),
            };
            store.save(&fail_state)?;
            println!(
                "[#{issue_num}] FAILED: {title} — returned to {}",
                cfg.work_label
            );
        }
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
        &[],
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
        &[],
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

/// Extract a concise failure reason from a worker log file.
///
/// Scans the last 50 lines for known error patterns (git clone failures,
/// network errors, auth failures) and returns a short diagnostic string.
/// Falls back to the last non-empty log line if no recognized pattern is found.
pub(crate) fn extract_failure_reason(log_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(log_path).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    let tail: &[&str] = if lines.len() > 50 {
        &lines[lines.len() - 50..]
    } else {
        &lines
    };

    for line in tail.iter().rev() {
        let lower = line.to_lowercase();
        if lower.contains("repository")
            && (lower.contains("not found") || lower.contains("does not exist"))
        {
            return Some(
                "git clone failed: repository not found — check repo URL and token".to_string(),
            );
        }
        if lower.contains("could not resolve host") || lower.contains("name or service not known") {
            return Some(
                "git clone failed: could not resolve host — check network connectivity".to_string(),
            );
        }
        if lower.contains("authentication failed")
            || (lower.contains("permission denied")
                && (lower.contains("git") || lower.contains("clone")))
        {
            return Some(
                "git clone failed: authentication failed — check token permissions".to_string(),
            );
        }
        if lower.starts_with("fatal:")
            && (lower.contains("repository")
                || lower.contains("remote")
                || lower.contains("clone")
                || lower.contains("unable to access"))
        {
            let msg = line.trim().trim_start_matches("fatal:").trim();
            return Some(format!("git fatal: {msg}"));
        }
    }

    // Fallback: last non-empty line
    tail.iter()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim().to_string())
}

/// Run the Docker container for a worker, streaming output to `log_path`.
///
/// When `state_mount` is `Some((host_workers_dir, state_filename))`, the host's
/// workers directory is bind-mounted into the container and `STATE_FILE` is set
/// so the container can self-report heartbeats, phases, and PR info.
///
/// `extra_env` passes additional environment variables to the container
/// (e.g. `ISSUE_NUM`, `WORK_LABEL` for label management).
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
    extra_env: &[(&str, &str)],
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

    // Docker labels for easier debugging with `docker ps --filter` and `docker inspect`.
    let dispatched_at = now_utc();
    cmd.arg("--label")
        .arg(format!("org.sipag.repo={repo}"))
        .arg("--label")
        .arg(format!("org.sipag.branch={branch}"))
        .arg("--label")
        .arg(format!("org.sipag.dispatched-at={dispatched_at}"));
    // Add org.sipag.issues from extra_env if ISSUE_NUM or ISSUE_NUMS is present.
    if let Some(issues) = extra_env
        .iter()
        .find_map(|(k, v)| (*k == "ISSUE_NUM" || *k == "ISSUE_NUMS").then_some(*v))
    {
        cmd.arg("--label").arg(format!("org.sipag.issues={issues}"));
    }

    // Mount workers directory for state self-reporting.
    if let Some((workers_dir, state_filename)) = state_mount {
        cmd.arg("-v")
            .arg(format!("{}:/sipag-state", workers_dir.display()))
            .arg("-e")
            .arg(format!("STATE_FILE=/sipag-state/{state_filename}"));
    }

    // Extra env vars (e.g. ISSUE_NUM, WORK_LABEL for container-side label management).
    for (key, value) in extra_env {
        cmd.arg("-e").arg(format!("{key}={value}"));
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
    extract_all_issue_nums_from_body(body).into_iter().next()
}

/// Extract all "Closes/Fixes/Resolves #N" issue numbers from text.
fn extract_all_issue_nums_from_body(body: &str) -> Vec<u64> {
    let mut nums = Vec::new();
    for line in body.lines() {
        let lower = line.to_lowercase();
        for keyword in &["closes #", "fixes #", "resolves #"] {
            let mut search_from = 0;
            while let Some(pos) = lower[search_from..].find(keyword) {
                let abs_pos = search_from + pos + keyword.len();
                let rest = &line[abs_pos..];
                let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(n) = num_str.parse::<u64>() {
                    if !nums.contains(&n) {
                        nums.push(n);
                    }
                }
                search_from = abs_pos;
            }
        }
    }
    nums
}

/// Fetch the PR body from GitHub and extract all addressed issue numbers.
fn extract_all_issue_nums_from_pr(repo: &str, pr_num: u64) -> Vec<u64> {
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            &pr_num.to_string(),
            "--repo",
            repo,
            "--json",
            "body",
            "--jq",
            ".body",
        ])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let body = String::from_utf8_lossy(&o.stdout);
            extract_all_issue_nums_from_body(&body)
        }
        _ => vec![],
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── extract_issue_num_from_body (single) ─────────────────────────────────

    #[test]
    fn extract_single_closes() {
        assert_eq!(extract_issue_num_from_body("Closes #42"), Some(42));
    }

    #[test]
    fn extract_single_fixes() {
        assert_eq!(extract_issue_num_from_body("Fixes #99"), Some(99));
    }

    #[test]
    fn extract_single_resolves() {
        assert_eq!(extract_issue_num_from_body("Resolves #7"), Some(7));
    }

    #[test]
    fn extract_case_insensitive() {
        assert_eq!(extract_issue_num_from_body("CLOSES #10"), Some(10));
        assert_eq!(extract_issue_num_from_body("closes #10"), Some(10));
        assert_eq!(extract_issue_num_from_body("ClOsEs #10"), Some(10));
    }

    #[test]
    fn extract_returns_first_when_multiple() {
        assert_eq!(
            extract_issue_num_from_body("Closes #1\nCloses #2\nCloses #3"),
            Some(1)
        );
    }

    #[test]
    fn extract_none_when_no_match() {
        assert_eq!(
            extract_issue_num_from_body("No issue references here"),
            None
        );
        assert_eq!(extract_issue_num_from_body(""), None);
        assert_eq!(extract_issue_num_from_body("Just a #42 reference"), None);
    }

    #[test]
    fn extract_ignores_invalid_number() {
        assert_eq!(extract_issue_num_from_body("Closes #abc"), None);
        assert_eq!(extract_issue_num_from_body("Closes #"), None);
    }

    // ── extract_all_issue_nums_from_body ─────────────────────────────────────

    #[test]
    fn extract_all_single_issue() {
        assert_eq!(extract_all_issue_nums_from_body("Closes #42"), vec![42]);
    }

    #[test]
    fn extract_all_multiple_issues_same_keyword() {
        assert_eq!(
            extract_all_issue_nums_from_body("Closes #1\nCloses #2\nCloses #3"),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn extract_all_mixed_keywords() {
        assert_eq!(
            extract_all_issue_nums_from_body("Closes #10\nFixes #20\nResolves #30"),
            vec![10, 20, 30]
        );
    }

    #[test]
    fn extract_all_deduplicates() {
        assert_eq!(
            extract_all_issue_nums_from_body("Closes #5\nFixes #5\nResolves #5"),
            vec![5]
        );
    }

    #[test]
    fn extract_all_preserves_order() {
        assert_eq!(
            extract_all_issue_nums_from_body("Closes #30\nCloses #10\nCloses #20"),
            vec![30, 10, 20]
        );
    }

    #[test]
    fn extract_all_empty_body() {
        assert!(extract_all_issue_nums_from_body("").is_empty());
    }

    #[test]
    fn extract_all_no_matches() {
        assert!(extract_all_issue_nums_from_body("No issue refs\nJust text").is_empty());
    }

    #[test]
    fn extract_all_mixed_with_prose() {
        let body = "## Summary\n\nThis PR addresses several issues.\n\nCloses #42\n\nAlso fixes #99 and the related problem.\n\nResolves #7\n\nSee also #100 (not addressed).";
        assert_eq!(extract_all_issue_nums_from_body(body), vec![42, 99, 7]);
    }

    #[test]
    fn extract_all_multiple_on_same_line() {
        // "Closes #1, Closes #2" on the same line
        assert_eq!(
            extract_all_issue_nums_from_body("Closes #1, Closes #2"),
            vec![1, 2]
        );
    }

    #[test]
    fn extract_all_case_insensitive() {
        assert_eq!(
            extract_all_issue_nums_from_body("CLOSES #1\nfixes #2\nResolves #3"),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn extract_all_ignores_plain_hash_refs() {
        // "#42" without a keyword should not be extracted.
        assert!(extract_all_issue_nums_from_body("See #42 and #99").is_empty());
    }

    #[test]
    fn extract_all_realistic_pr_body() {
        let body = "\
Closes #101
Closes #103

## Summary
- Refactored the config module to support grouped dispatch
- Updated worker prompt for multi-issue context

## Test plan
- [x] `make dev` passes
- [x] Manual test with multiple issues

---
*This PR was opened by a sipag worker.*";
        assert_eq!(extract_all_issue_nums_from_body(body), vec![101, 103]);
    }

    // ── Worker prompt template ──────────────────────────────────────────────

    #[test]
    fn worker_prompt_has_expected_placeholders() {
        assert!(
            WORKER_PROMPT.contains("{{ALL_ISSUES}}"),
            "Missing {{{{ALL_ISSUES}}}} placeholder"
        );
        assert!(
            WORKER_PROMPT.contains("{{READY_ISSUES}}"),
            "Missing {{{{READY_ISSUES}}}} placeholder"
        );
        assert!(
            WORKER_PROMPT.contains("{{BRAINSTORM_PLAN}}"),
            "Missing {{{{BRAINSTORM_PLAN}}}} placeholder"
        );
        assert!(
            WORKER_PROMPT.contains("{{BRANCH}}"),
            "Missing {{{{BRANCH}}}} placeholder"
        );
    }

    #[test]
    fn worker_prompt_substitution_produces_valid_output() {
        let all_issues = "### Issue #1: Fix bug\n\nBug description\n\n### Issue #2: Add feature\n\nFeature description\n\n### Issue #3: Background task\n\nNot ready yet\n\n";
        let ready_issues = "### Issue #1: Fix bug\n\nBug description\n\n### Issue #2: Add feature\n\nFeature description\n\n";
        let branch = "sipag/group-1-fix-bug";

        let prompt = WORKER_PROMPT
            .replace("{{ALL_ISSUES}}", all_issues)
            .replace("{{READY_ISSUES}}", ready_issues)
            .replace("{{BRAINSTORM_PLAN}}", "")
            .replace("{{BRANCH}}", branch);

        assert!(prompt.contains("### Issue #1: Fix bug"));
        assert!(prompt.contains("### Issue #3: Background task"));
        assert!(prompt.contains("sipag/group-1-fix-bug"));
        assert!(!prompt.contains("{{ALL_ISSUES}}"));
        assert!(!prompt.contains("{{READY_ISSUES}}"));
        assert!(!prompt.contains("{{BRAINSTORM_PLAN}}"));
        assert!(!prompt.contains("{{BRANCH}}"));
    }

    #[test]
    fn worker_prompt_substitution_with_brainstorm_plan() {
        let plan = "## Pre-analysis\n\nSome plan content.";
        let prompt = WORKER_PROMPT
            .replace("{{ALL_ISSUES}}", "issues")
            .replace("{{READY_ISSUES}}", "ready")
            .replace("{{BRAINSTORM_PLAN}}", plan)
            .replace("{{BRANCH}}", "test-branch");

        assert!(prompt.contains("Pre-analysis"));
        assert!(prompt.contains("Some plan content."));
        assert!(!prompt.contains("{{BRAINSTORM_PLAN}}"));
    }

    #[test]
    fn worker_prompt_mentions_closes_instruction() {
        let lower = WORKER_PROMPT.to_lowercase();
        assert!(
            lower.contains("closes #n") || lower.contains("closes"),
            "Worker prompt should mention 'Closes #N' for addressed issues"
        );
    }

    #[test]
    fn worker_prompt_mentions_boy_scout_rule() {
        let lower = WORKER_PROMPT.to_lowercase();
        assert!(
            lower.contains("boy scout"),
            "Worker prompt should mention Boy Scout Rule"
        );
    }

    #[test]
    fn worker_prompt_mentions_test_curation() {
        let lower = WORKER_PROMPT.to_lowercase();
        assert!(
            lower.contains("test suite") || lower.contains("curate"),
            "Worker prompt should mention test suite curation"
        );
    }

    // ── Container script embeds ──────────────────────────────────────────────

    #[test]
    fn issue_container_script_handles_issue_nums_env() {
        // The container script should reference ISSUE_NUMS for grouped workers.
        assert!(
            ISSUE_CONTAINER_SCRIPT.contains("ISSUE_NUMS"),
            "Container script should handle ISSUE_NUMS env var for grouped dispatch"
        );
    }

    #[test]
    fn issue_container_script_falls_back_to_issue_num() {
        // Backward compat: should still work with single ISSUE_NUM.
        assert!(
            ISSUE_CONTAINER_SCRIPT.contains("ISSUE_NUM"),
            "Container script should fall back to ISSUE_NUM for single-issue dispatch"
        );
    }

    #[test]
    fn issue_container_script_parses_addressed_issues_on_success() {
        // On success, the script should parse PR body for "Closes/Fixes/Resolves #N".
        assert!(
            ISSUE_CONTAINER_SCRIPT.contains("addressed_issues")
                || ISSUE_CONTAINER_SCRIPT.contains("closes\\|fixes\\|resolves")
                || ISSUE_CONTAINER_SCRIPT.contains("closes|fixes|resolves"),
            "Container script should parse PR body for addressed issues on success"
        );
    }

    // ── sipag-state contract tests ───────────────────────────────────────────
    //
    // These tests document and enforce the contract between container scripts
    // and the sipag-state helper binary. sipag-state must be present in the
    // Docker image (installed via Dockerfile COPY) and invoked by the scripts
    // for heartbeat, phase, PR, and finish reporting.
    //
    // If these tests fail, it means a container script was modified to stop
    // reporting its state, which would break the TUI, `sipag ps`, and
    // stale-heartbeat detection in the poll loop.

    #[test]
    fn issue_container_script_reports_heartbeat_via_sipag_state() {
        assert!(
            ISSUE_CONTAINER_SCRIPT.contains("sipag-state heartbeat"),
            "Container script must call 'sipag-state heartbeat' so the poll loop \
             can detect stale workers"
        );
    }

    #[test]
    fn issue_container_script_reports_phase_via_sipag_state() {
        assert!(
            ISSUE_CONTAINER_SCRIPT.contains("sipag-state phase"),
            "Container script must call 'sipag-state phase' so the TUI and \
             'sipag ps' can show current worker progress"
        );
    }

    #[test]
    fn issue_container_script_records_pr_via_sipag_state() {
        assert!(
            ISSUE_CONTAINER_SCRIPT.contains("sipag-state pr "),
            "Container script must call 'sipag-state pr' to record PR number/URL \
             in the state file while the container is still running"
        );
    }

    #[test]
    fn issue_container_script_finalizes_via_sipag_state_finish() {
        assert!(
            ISSUE_CONTAINER_SCRIPT.contains("sipag-state finish"),
            "Container script must call 'sipag-state finish' to write terminal \
             status (done/failed) and duration to the state file"
        );
    }

    // ── extract_failure_reason ───────────────────────────────────────────────

    fn write_log(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn failure_reason_repo_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let log = write_log(
            dir.path(),
            "test.log",
            "Cloning into '/work'...\nfatal: repository 'https://github.com/owner/bad-repo.git/' not found\n",
        );
        let reason = extract_failure_reason(&log).unwrap();
        assert!(
            reason.contains("repository not found"),
            "Expected repo not found message, got: {reason}"
        );
    }

    #[test]
    fn failure_reason_could_not_resolve_host() {
        let dir = tempfile::tempdir().unwrap();
        let log = write_log(
            dir.path(),
            "test.log",
            "Cloning into '/work'...\nfatal: unable to access 'https://github.com/owner/repo.git/': Could not resolve host: github.com\n",
        );
        let reason = extract_failure_reason(&log).unwrap();
        assert!(
            reason.contains("could not resolve host"),
            "Expected network error message, got: {reason}"
        );
    }

    #[test]
    fn failure_reason_authentication_failed() {
        let dir = tempfile::tempdir().unwrap();
        let log = write_log(
            dir.path(),
            "test.log",
            "Cloning into '/work'...\nremote: Invalid username or password.\nfatal: Authentication failed for 'https://github.com/owner/repo.git/'\n",
        );
        let reason = extract_failure_reason(&log).unwrap();
        assert!(
            reason.contains("authentication failed"),
            "Expected auth failure message, got: {reason}"
        );
    }

    #[test]
    fn failure_reason_fallback_to_last_line() {
        let dir = tempfile::tempdir().unwrap();
        let log = write_log(
            dir.path(),
            "test.log",
            "Some output\nAnother line\nclaude exited with code 1\n",
        );
        let reason = extract_failure_reason(&log).unwrap();
        assert_eq!(reason, "claude exited with code 1");
    }

    #[test]
    fn failure_reason_empty_log_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let log = write_log(dir.path(), "test.log", "");
        assert!(extract_failure_reason(&log).is_none());
    }

    #[test]
    fn failure_reason_whitespace_only_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let log = write_log(dir.path(), "test.log", "   \n   \n");
        assert!(extract_failure_reason(&log).is_none());
    }

    #[test]
    fn failure_reason_missing_log_returns_none() {
        let path = std::path::PathBuf::from("/nonexistent/log/file.log");
        assert!(extract_failure_reason(&path).is_none());
    }
}
