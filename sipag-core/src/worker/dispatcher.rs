use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use super::gh_gateway::{GhCliGateway, WorkerPoller};
use super::ports::{GitHubGateway, StateStore};
use super::state::WorkerState;
use super::status::WorkerStatus;
use super::store::FileStateStore;
use crate::task::slugify;

// ── Embedded bash scripts (run inside Docker containers) ──────────────────────
//
// All host-side orchestration is in Rust. These scripts are the only bash that
// remains after the loop migration — they run INSIDE the worker Docker image.

/// Issue worker: clone repo, create branch, draft PR, run claude, ensure PR, mark ready.
const ISSUE_WORKER_SCRIPT: &str = r#"set -euo pipefail
git clone "https://github.com/${REPO}.git" /work && cd /work
git config user.name "sipag"
git config user.email "sipag@localhost"
git remote set-url origin "https://x-access-token:${GH_TOKEN}@github.com/${REPO}.git"
git checkout -b "${BRANCH}"
git push -u origin "${BRANCH}"
if gh pr create --repo "${REPO}" \
        --title "${ISSUE_TITLE}" \
        --body "${PR_BODY}" \
        --draft \
        --head "${BRANCH}" 2>/tmp/sipag-pr-err.log; then
    echo "[sipag] Draft PR opened: branch=${BRANCH}"
else
    echo "[sipag] Draft PR deferred: $(cat /tmp/sipag-pr-err.log)"
fi
tmux new-session -d -s claude \
    "claude --dangerously-skip-permissions -p \"${PROMPT}\"; echo \$? > /tmp/.claude-exit"
tmux set-option -t claude history-limit 50000
touch /tmp/claude.log
tmux pipe-pane -t claude -o "cat >> /tmp/claude.log"
tail -f /tmp/claude.log &
TAIL_PID=$!
while tmux has-session -t claude 2>/dev/null; do sleep 1; done
kill $TAIL_PID 2>/dev/null || true
wait $TAIL_PID 2>/dev/null || true
CLAUDE_EXIT=$(cat /tmp/.claude-exit 2>/dev/null || echo 1)
if [ "$CLAUDE_EXIT" -eq 0 ]; then
    existing_pr=$(gh pr list --repo "${REPO}" --head "${BRANCH}" \
        --state open --json number -q ".[0].number" 2>/dev/null || true)
    if [ -z "$existing_pr" ]; then
        echo "[sipag] Retrying PR creation after work completion"
        gh pr create --repo "${REPO}" \
                --title "${ISSUE_TITLE}" \
                --body "${PR_BODY}" \
                --head "${BRANCH}" 2>/dev/null || true
    fi
    gh pr ready "${BRANCH}" --repo "${REPO}" || true
    echo "[sipag] PR marked ready for review"
fi
exit "$CLAUDE_EXIT"
"#;

/// PR iteration: checkout existing branch, run claude to address feedback.
const ITERATION_WORKER_SCRIPT: &str = r#"set -euo pipefail
git clone "https://github.com/${REPO}.git" /work && cd /work
git config user.name "sipag"
git config user.email "sipag@localhost"
git remote set-url origin "https://x-access-token:${GH_TOKEN}@github.com/${REPO}.git"
git checkout "${BRANCH}"
tmux new-session -d -s claude \
    "claude --dangerously-skip-permissions -p \"${PROMPT}\"; echo \$? > /tmp/.claude-exit"
tmux set-option -t claude history-limit 50000
touch /tmp/claude.log
tmux pipe-pane -t claude -o "cat >> /tmp/claude.log"
tail -f /tmp/claude.log &
TAIL_PID=$!
while tmux has-session -t claude 2>/dev/null; do sleep 1; done
kill $TAIL_PID 2>/dev/null || true
wait $TAIL_PID 2>/dev/null || true
exit "$(cat /tmp/.claude-exit 2>/dev/null || echo 1)"
"#;

/// Conflict-fix: merge main forward, run claude only if conflicts arise.
const CONFLICT_FIX_SCRIPT: &str = r#"set -euo pipefail
git clone "https://github.com/${REPO}.git" /work && cd /work
git config user.name "sipag"
git config user.email "sipag@localhost"
git remote set-url origin "https://x-access-token:${GH_TOKEN}@github.com/${REPO}.git"
git checkout "${BRANCH}"
git fetch origin main
if git merge origin/main --no-edit; then
    git push origin "${BRANCH}"
    echo "[sipag] Merged main into ${BRANCH} (no conflicts)"
    exit 0
fi
echo "[sipag] Conflicts detected in ${BRANCH}, running Claude to resolve..."
tmux new-session -d -s claude \
    "claude --dangerously-skip-permissions -p \"${PROMPT}\"; echo \$? > /tmp/.claude-exit"
tmux set-option -t claude history-limit 50000
touch /tmp/claude.log
tmux pipe-pane -t claude -o "cat >> /tmp/claude.log"
tail -f /tmp/claude.log &
TAIL_PID=$!
while tmux has-session -t claude 2>/dev/null; do sleep 1; done
kill $TAIL_PID 2>/dev/null || true
wait $TAIL_PID 2>/dev/null || true
exit "$(cat /tmp/.claude-exit 2>/dev/null || echo 1)"
"#;

// ── Prompt builders ───────────────────────────────────────────────────────────

fn build_issue_prompt(title: &str, body: &str, branch: &str, issue_num: u64) -> String {
    format!(
        "You are working on the repository at /work.\n\nYour task:\n{title}\n\n{body}\n\n\
         Instructions:\n\
         - You are on branch {branch} — do NOT create a new branch\n\
         - A draft PR is already open for this branch — do not open another one\n\
         - Implement the changes\n\
         - Run `make dev` (fmt + clippy + test) before committing to validate your changes\n\
         - Run any existing tests and make sure they pass\n\
         - Commit your changes with a clear commit message and push to origin\n\
         - The PR will be marked ready for review automatically when you finish\n\
         - The PR should close issue #{issue_num}\n"
    )
}

fn build_iteration_prompt(
    pr_num: u64,
    repo: &str,
    issue_body: &str,
    pr_diff: &str,
    review_feedback: &str,
    branch: &str,
) -> String {
    format!(
        "You are iterating on PR #{pr_num} in {repo}.\n\nOriginal issue:\n{issue_body}\n\n\
         Current PR diff:\n{pr_diff}\n\nReview feedback:\n{review_feedback}\n\n\
         Instructions:\n\
         - You are on branch {branch} which already has work in progress\n\
         - Read the review feedback carefully and address every point raised\n\
         - Make targeted changes that address the feedback\n\
         - Do NOT rewrite the PR from scratch — make surgical fixes\n\
         - Run `make dev` (fmt + clippy + test) before committing to validate your changes\n\
         - Commit with a message that references the feedback (do NOT amend existing commits)\n\
         - Push to the same branch (git push origin {branch}) — do NOT force push\n"
    )
}

fn build_conflict_fix_prompt(pr_num: u64, pr_title: &str, branch: &str, pr_body: &str) -> String {
    format!(
        "You are resolving merge conflicts in a pull request branch.\n\nThe repository is at /work.\n\n\
         PR #{pr_num}: {pr_title}\nBranch: {branch}\n\n\
         A `git merge origin/main` was attempted on this branch and produced merge conflicts.\n\
         The conflict markers are already present in the working tree.\n\n\
         Your task:\n\
         1. Run `git status` to see which files have conflicts\n\
         2. For each conflicted file: read, understand both sides, keep both where possible\n\
         3. Stage all resolved files with `git add <file>`\n\
         4. Run `git merge --continue --no-edit` to create the merge commit\n\
         5. Run `git push origin {branch}` to update the pull request\n\n\
         Critical rules:\n\
         - NEVER run `git rebase`\n\
         - NEVER run `git push --force`\n\
         - NEVER amend existing commits\n\n\
         Context about this PR:\n{pr_body}\n"
    )
}

// ── WorkerDispatcher ──────────────────────────────────────────────────────────

/// Launches Docker containers for issue workers, PR iteration workers, and conflict-fix workers.
///
/// Implements the same protocol as `lib/worker/docker.sh` but in Rust:
/// - Writes enqueued state before starting containers (crash-safe)
/// - Transitions GitHub labels around container execution
/// - Updates worker state files on completion
///
/// Clone-safe: all fields are owned so instances can be sent to threads.
#[derive(Clone)]
pub struct WorkerDispatcher {
    sipag_dir: PathBuf,
    image: String,
    timeout: u64,
    work_label: String,
    oauth_token: Option<String>,
    gh_token: String,
    log_dir: PathBuf,
}

impl WorkerDispatcher {
    /// Create a dispatcher, resolving credentials from `sipag_dir` and environment.
    pub fn new(sipag_dir: &Path, image: &str, timeout: u64, work_label: &str) -> Result<Self> {
        let oauth_token = resolve_oauth_token(sipag_dir);
        let gh_token = get_gh_token();
        let log_dir = sipag_dir.join("logs");
        fs::create_dir_all(&log_dir)?;
        Ok(Self {
            sipag_dir: sipag_dir.to_path_buf(),
            image: image.to_string(),
            timeout,
            work_label: work_label.to_string(),
            oauth_token,
            gh_token,
            log_dir,
        })
    }

    // ── Dispatch: new issue ───────────────────────────────────────────────────

    /// Dispatch a new issue worker.
    pub fn dispatch_issue(&self, repo: &str, issue_num: u64) -> Result<()> {
        let store = FileStateStore::new(&self.sipag_dir);
        let github = GhCliGateway;
        let container_name = format!("sipag-issue-{issue_num}");
        let now = utc_now();

        // Write enqueued state immediately (crash-safe)
        store.save(&make_state(
            repo,
            issue_num,
            "",
            "",
            &container_name,
            WorkerStatus::Enqueued,
            Some(now.clone()),
            None,
        ))?;

        // Transition label: work_label → in-progress
        let _ =
            github.transition_label(repo, issue_num, Some(&self.work_label), Some("in-progress"));

        // Fetch issue
        let issue = match github.get_issue(repo, issue_num) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("[#{issue_num}] Failed to fetch issue: {e}");
                self.fail_issue(repo, issue_num)?;
                return Ok(());
            }
        };
        println!("[#{issue_num}] Starting: {}", issue.title);

        // Build branch name, prompt, and PR body
        let slug = slugify(&issue.title);
        let branch = format!("sipag/issue-{issue_num}-{}", &slug[..slug.len().min(50)]);
        let pr_body = format!(
            "Closes #{issue_num}\n\n{}\n\n---\n\
             *This PR was opened by a sipag worker. Commits will appear as work progresses.*",
            issue.body
        );
        let prompt = build_issue_prompt(&issue.title, &issue.body, &branch, issue_num);

        // Write running state
        let repo_slug = repo.replace('/', "--");
        let log_path = self.log_dir.join(format!("{repo_slug}--{issue_num}.log"));
        let running_state = make_state(
            repo,
            issue_num,
            &issue.title,
            &branch,
            &container_name,
            WorkerStatus::Running,
            Some(now),
            Some(log_path.clone()),
        );
        store.save(&running_state)?;

        // Run container
        let start = SystemTime::now();
        let success = self.run_docker(
            &container_name,
            repo,
            &branch,
            &issue.title,
            &pr_body,
            &prompt,
            &log_path,
            ISSUE_WORKER_SCRIPT,
        );
        let duration_s = elapsed_secs(start);
        let ended_at = utc_now();

        // Update state
        if success {
            let pr = github.find_pr_for_branch(repo, &branch).ok().flatten();
            let _ = github.transition_label(repo, issue_num, Some("in-progress"), None);
            store.save(&WorkerState {
                status: WorkerStatus::Done,
                ended_at: Some(ended_at),
                duration_s: Some(duration_s),
                exit_code: Some(0),
                pr_num: pr.as_ref().map(|p| p.number),
                pr_url: pr.as_ref().map(|p| p.url.clone()),
                ..running_state
            })?;
            println!("[#{issue_num}] DONE: {}", issue.title);
        } else {
            let _ = github.transition_label(
                repo,
                issue_num,
                Some("in-progress"),
                Some(&self.work_label),
            );
            store.save(&WorkerState {
                status: WorkerStatus::Failed,
                ended_at: Some(ended_at),
                duration_s: Some(duration_s),
                exit_code: Some(1),
                ..running_state
            })?;
            println!("[#{issue_num}] FAILED: {}", issue.title);
        }
        Ok(())
    }

    // ── Dispatch: PR iteration ────────────────────────────────────────────────

    /// Dispatch a PR iteration worker.
    pub fn dispatch_pr_iteration(&self, repo: &str, pr_num: u64) -> Result<()> {
        let repo_slug = repo.replace('/', "--");
        let title = gh_output(&[
            "pr",
            "view",
            &pr_num.to_string(),
            "--repo",
            repo,
            "--json",
            "title",
            "-q",
            ".title",
        ])?;
        let branch = gh_output(&[
            "pr",
            "view",
            &pr_num.to_string(),
            "--repo",
            repo,
            "--json",
            "headRefName",
            "-q",
            ".headRefName",
        ])?;
        println!("[PR #{pr_num}] Iterating: {title} (branch: {branch})");

        let pr_body_raw = gh_output(&[
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
        .unwrap_or_default();
        let issue_body = extract_closes_issue(&pr_body_raw)
            .and_then(|n| {
                gh_output(&[
                    "issue",
                    "view",
                    &n.to_string(),
                    "--repo",
                    repo,
                    "--json",
                    "body",
                    "-q",
                    ".body",
                ])
                .ok()
            })
            .unwrap_or_default();

        let review_jq = concat!(
            r#"([.reviews[] | select(.state == "CHANGES_REQUESTED") | "Review by \(.author.login):\n\(.body)"] "#,
            r#"+ [.comments[] | "Comment by \(.author.login):\n\(.body)"]) | join("\n---\n")"#
        );
        let review_feedback = gh_output(&[
            "pr",
            "view",
            &pr_num.to_string(),
            "--repo",
            repo,
            "--json",
            "reviews,comments",
            "--jq",
            review_jq,
        ])
        .unwrap_or_default();
        let inline_jq = r#"[.[] | "Inline comment on \(.path) line \(.line // "?") by \(.user.login):\n\(.body)"] | join("\n---\n")"#;
        let inline = gh_output(&[
            "api",
            &format!("repos/{repo}/pulls/{pr_num}/comments"),
            "--jq",
            inline_jq,
        ])
        .unwrap_or_default();

        let all_feedback = join_feedback(&review_feedback, &inline);
        let pr_diff = gh_output(&["pr", "diff", &pr_num.to_string(), "--repo", repo])
            .map(|d| d.chars().take(50_000).collect::<String>())
            .unwrap_or_default();

        let prompt =
            build_iteration_prompt(pr_num, repo, &issue_body, &pr_diff, &all_feedback, &branch);
        let container_name = format!("sipag-pr-{pr_num}");
        let log_path = self
            .log_dir
            .join(format!("{repo_slug}--pr-{pr_num}-iter.log"));

        let success = self.run_docker(
            &container_name,
            repo,
            &branch,
            "",
            "",
            &prompt,
            &log_path,
            ITERATION_WORKER_SCRIPT,
        );
        if success {
            println!("[PR #{pr_num}] DONE iterating: {title}");
        } else {
            println!("[PR #{pr_num}] FAILED iteration: {title}");
        }
        Ok(())
    }

    // ── Dispatch: conflict fix ────────────────────────────────────────────────

    /// Dispatch a conflict-fix worker.
    pub fn dispatch_conflict_fix(&self, repo: &str, pr_num: u64) -> Result<()> {
        let repo_slug = repo.replace('/', "--");
        let title = gh_output(&[
            "pr",
            "view",
            &pr_num.to_string(),
            "--repo",
            repo,
            "--json",
            "title",
            "-q",
            ".title",
        ])?;
        let branch = gh_output(&[
            "pr",
            "view",
            &pr_num.to_string(),
            "--repo",
            repo,
            "--json",
            "headRefName",
            "-q",
            ".headRefName",
        ])?;
        let pr_body = gh_output(&[
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
        .unwrap_or_default();
        println!("[PR #{pr_num}] Merging main forward: {title} (branch: {branch})");

        let prompt = build_conflict_fix_prompt(pr_num, &title, &branch, &pr_body);
        let container_name = format!("sipag-conflict-{pr_num}");
        let log_path = self
            .log_dir
            .join(format!("{repo_slug}--pr-{pr_num}-conflict-fix.log"));

        let success = self.run_docker(
            &container_name,
            repo,
            &branch,
            "",
            "",
            &prompt,
            &log_path,
            CONFLICT_FIX_SCRIPT,
        );
        if success {
            println!("[PR #{pr_num}] Conflict fix done: {title}");
        } else {
            println!("[PR #{pr_num}] Conflict fix FAILED: {title}");
        }
        Ok(())
    }

    // ── Internal ──────────────────────────────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    fn run_docker(
        &self,
        container_name: &str,
        repo: &str,
        branch: &str,
        issue_title: &str,
        pr_body: &str,
        prompt: &str,
        log_path: &Path,
        bash_script: &str,
    ) -> bool {
        let log_out = match fs::File::create(log_path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("sipag: log create error: {e}");
                return false;
            }
        };
        let log_err = match log_out.try_clone() {
            Ok(f) => f,
            Err(_) => return false,
        };

        let mut cmd = Command::new("timeout");
        cmd.arg(self.timeout.to_string())
            .arg("docker")
            .arg("run")
            .arg("--rm")
            .arg("--name")
            .arg(container_name)
            .arg("-e")
            .arg("CLAUDE_CODE_OAUTH_TOKEN")
            .arg("-e")
            .arg("ANTHROPIC_API_KEY")
            .arg("-e")
            .arg(format!("GH_TOKEN={}", self.gh_token))
            .arg("-e")
            .arg(format!("REPO={repo}"))
            .arg("-e")
            .arg(format!("BRANCH={branch}"))
            .arg("-e")
            .arg(format!("ISSUE_TITLE={issue_title}"))
            .arg("-e")
            .arg(format!("PR_BODY={pr_body}"))
            .arg("-e")
            .arg(format!("PROMPT={prompt}"))
            .arg(&self.image)
            .arg("bash")
            .arg("-c")
            .arg(bash_script)
            .stdout(Stdio::from(log_out))
            .stderr(Stdio::from(log_err));

        if let Some(token) = &self.oauth_token {
            cmd.env("CLAUDE_CODE_OAUTH_TOKEN", token);
        }
        cmd.status().map(|s| s.success()).unwrap_or(false)
    }

    fn fail_issue(&self, repo: &str, issue_num: u64) -> Result<()> {
        let store = FileStateStore::new(&self.sipag_dir);
        let github = GhCliGateway;
        let _ =
            github.transition_label(repo, issue_num, Some("in-progress"), Some(&self.work_label));
        store.save(&make_state(
            repo,
            issue_num,
            "",
            "",
            &format!("sipag-issue-{issue_num}"),
            WorkerStatus::Failed,
            None,
            None,
        ))?;
        Ok(())
    }
}

// ── Free helpers ──────────────────────────────────────────────────────────────

fn utc_now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn elapsed_secs(start: SystemTime) -> i64 {
    start.elapsed().map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn resolve_oauth_token(sipag_dir: &Path) -> Option<String> {
    if let Ok(tok) = std::env::var("CLAUDE_CODE_OAUTH_TOKEN") {
        if !tok.is_empty() {
            return Some(tok);
        }
    }
    if let Ok(tok) = fs::read_to_string(sipag_dir.join("token")) {
        let tok = tok.trim().to_string();
        if !tok.is_empty() {
            return Some(tok);
        }
    }
    None
}

fn get_gh_token() -> String {
    Command::new("gh")
        .args(["auth", "token"])
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn gh_output(args: &[&str]) -> Result<String> {
    let out = Command::new("gh")
        .args(args)
        .stderr(Stdio::null())
        .output()?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn join_feedback(a: &str, b: &str) -> String {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b.to_string(),
        (_, true) => a.to_string(),
        _ => format!("{a}\n---\n{b}"),
    }
}

/// Extract first issue number from "Closes #N" / "Fixes #N" / "Resolves #N".
fn extract_closes_issue(body: &str) -> Option<u64> {
    for keyword in ["closes", "fixes", "resolves"] {
        for line in body.lines() {
            let lower = line.to_lowercase();
            if let Some(pos) = lower.find(keyword) {
                let after = line[pos + keyword.len()..].trim().trim_start_matches('#');
                let num_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(n) = num_str.parse::<u64>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn make_state(
    repo: &str,
    issue_num: u64,
    issue_title: &str,
    branch: &str,
    container_name: &str,
    status: WorkerStatus,
    started_at: Option<String>,
    log_path: Option<PathBuf>,
) -> WorkerState {
    WorkerState {
        repo: repo.to_string(),
        issue_num,
        issue_title: issue_title.to_string(),
        branch: branch.to_string(),
        container_name: container_name.to_string(),
        pr_num: None,
        pr_url: None,
        status,
        started_at,
        ended_at: None,
        duration_s: None,
        exit_code: None,
        log_path,
    }
}
