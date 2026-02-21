//! Pure container planning functions and outcome types.
//!
//! All functions are pure (no I/O, no side effects). The orchestrator
//! adds credentials and dynamic values before launching the container.

use std::path::Path;

use super::ports::ContainerConfig;
use crate::task::naming::slugify;

/// Outcome of running a worker container.
///
/// Returned by `classify_exit` and used by `WorkerOrchestrator::run_issue`
/// to determine whether to mark the worker done or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerOutcome {
    /// Container exited with code 0 and a PR was found — work succeeded.
    Done {
        /// PR number, if one was located after the run.
        pr_num: Option<u64>,
        /// PR URL, if one was located after the run.
        pr_url: Option<String>,
    },
    /// Container exited with a non-zero code, or no PR exists — work failed.
    Failed {
        /// The raw exit code from the container process.
        exit_code: i32,
    },
}

/// Determine the outcome from a container's exit code and PR presence.
///
/// Pure function — no side effects.
///
/// Decision rules:
/// - `exit_code == 0` **and** `pr_exists` → `Done`
/// - `exit_code == 0` **and** `!pr_exists` → `Failed` (Claude succeeded but created no PR)
/// - `exit_code != 0` → `Failed` (Claude process failed)
pub fn classify_exit(exit_code: i32, pr_exists: bool) -> WorkerOutcome {
    if exit_code == 0 && pr_exists {
        WorkerOutcome::Done {
            pr_num: None,
            pr_url: None,
        }
    } else {
        WorkerOutcome::Failed { exit_code }
    }
}

/// Compute the `ContainerConfig` for a new issue worker.
///
/// Pure function — no I/O. Credentials and prompt env vars are added by the
/// orchestrator before the config is passed to `ContainerRuntime::run_container`.
///
/// Naming conventions:
/// - Container: `sipag-issue-{issue_num}`
/// - Branch: `sipag/issue-{issue_num}-{slug}` (slug truncated to 50 chars,
///   matching the bash `worker_slugify` 50-char truncation)
/// - Log path: `{log_dir}/{repo_slug}--{issue_num}.log`
pub fn plan_issue_container(
    repo: &str,
    issue_num: u64,
    title: &str,
    image: &str,
    log_dir: &Path,
) -> ContainerConfig {
    let slug: String = slugify(title).chars().take(50).collect();
    let branch = format!("sipag/issue-{issue_num}-{slug}");
    let container_name = format!("sipag-issue-{issue_num}");
    let repo_slug = repo.replace('/', "--");
    let log_path = log_dir.join(format!("{repo_slug}--{issue_num}.log"));

    ContainerConfig {
        name: container_name,
        image: image.to_string(),
        repo: repo.to_string(),
        branch,
        env: std::collections::HashMap::new(),
        timeout: None,
        log_path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ── classify_exit ─────────────────────────────────────────────────────────

    #[test]
    fn exit_0_with_pr_is_done() {
        assert_eq!(
            classify_exit(0, true),
            WorkerOutcome::Done {
                pr_num: None,
                pr_url: None
            }
        );
    }

    #[test]
    fn exit_0_without_pr_is_failed() {
        // Claude succeeded but created no PR — treat as failure so it retries.
        assert_eq!(
            classify_exit(0, false),
            WorkerOutcome::Failed { exit_code: 0 }
        );
    }

    #[test]
    fn nonzero_exit_with_pr_is_failed() {
        // Container failed even if a PR somehow exists (e.g. from a previous run).
        assert_eq!(
            classify_exit(1, true),
            WorkerOutcome::Failed { exit_code: 1 }
        );
    }

    #[test]
    fn nonzero_exit_without_pr_is_failed() {
        assert_eq!(
            classify_exit(1, false),
            WorkerOutcome::Failed { exit_code: 1 }
        );
    }

    #[test]
    fn exit_code_preserved_in_failure() {
        assert_eq!(
            classify_exit(137, false),
            WorkerOutcome::Failed { exit_code: 137 }
        );
    }

    #[test]
    fn all_exit_and_pr_combinations_handled() {
        // Exhaustiveness: all four combinations should not panic.
        let _ = classify_exit(0, true);
        let _ = classify_exit(0, false);
        let _ = classify_exit(1, true);
        let _ = classify_exit(1, false);
    }

    // ── plan_issue_container ──────────────────────────────────────────────────

    #[test]
    fn plan_produces_correct_container_name() {
        let dir = TempDir::new().unwrap();
        let config = plan_issue_container(
            "owner/repo",
            42,
            "Fix the bug",
            "sipag-worker:latest",
            dir.path(),
        );
        assert_eq!(config.name, "sipag-issue-42");
    }

    #[test]
    fn plan_produces_correct_branch() {
        let dir = TempDir::new().unwrap();
        let config = plan_issue_container(
            "owner/repo",
            42,
            "Fix the bug",
            "sipag-worker:latest",
            dir.path(),
        );
        assert_eq!(config.branch, "sipag/issue-42-fix-the-bug");
    }

    #[test]
    fn plan_branch_slug_truncated_to_50_chars() {
        let dir = TempDir::new().unwrap();
        let long_title = "This is a very long issue title that exceeds fifty characters easily";
        let config = plan_issue_container(
            "owner/repo",
            1,
            long_title,
            "sipag-worker:latest",
            dir.path(),
        );
        let slug_part = config.branch.strip_prefix("sipag/issue-1-").unwrap();
        assert!(
            slug_part.len() <= 50,
            "slug part is {} chars (expected ≤ 50)",
            slug_part.len()
        );
    }

    #[test]
    fn plan_sets_image_and_repo() {
        let dir = TempDir::new().unwrap();
        let config =
            plan_issue_container("owner/repo", 42, "Fix the bug", "my-image:v1", dir.path());
        assert_eq!(config.image, "my-image:v1");
        assert_eq!(config.repo, "owner/repo");
    }

    #[test]
    fn plan_log_path_uses_repo_slug_with_double_dashes() {
        let dir = TempDir::new().unwrap();
        let config = plan_issue_container(
            "owner/repo",
            42,
            "Fix the bug",
            "sipag-worker:latest",
            dir.path(),
        );
        let expected = dir.path().join("owner--repo--42.log");
        assert_eq!(config.log_path, expected);
    }

    #[test]
    fn plan_env_is_empty_by_default() {
        let dir = TempDir::new().unwrap();
        let config = plan_issue_container(
            "owner/repo",
            42,
            "Fix the bug",
            "sipag-worker:latest",
            dir.path(),
        );
        assert!(config.env.is_empty());
    }

    #[test]
    fn plan_timeout_is_none_by_default() {
        let dir = TempDir::new().unwrap();
        let config = plan_issue_container(
            "owner/repo",
            42,
            "Fix the bug",
            "sipag-worker:latest",
            dir.path(),
        );
        assert!(config.timeout.is_none());
    }

    #[test]
    fn plan_slugifies_special_chars_in_title() {
        let dir = TempDir::new().unwrap();
        let config = plan_issue_container(
            "owner/repo",
            10,
            "feat(worker): detect stale PRs",
            "sipag-worker:latest",
            dir.path(),
        );
        assert_eq!(config.branch, "sipag/issue-10-feat-worker-detect-stale-prs");
    }
}
