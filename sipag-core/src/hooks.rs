//! Lifecycle hook execution for sipag worker events.
//!
//! Hooks are executable scripts placed in `~/.sipag/hooks/` that fire when
//! workers reach key milestones. They run asynchronously so they never block
//! the worker. Missing or non-executable hooks are silently skipped.
//!
//! # Hook scripts
//!
//! Place executable files in `~/.sipag/hooks/` named after the event:
//!
//! | Hook name              | Event                          |
//! |------------------------|--------------------------------|
//! | `on-worker-started`    | Worker picked up an issue      |
//! | `on-worker-completed`  | Worker finished, PR opened     |
//! | `on-worker-failed`     | Worker exited non-zero         |
//! | `on-pr-merged`         | PR was merged                  |
//! | `on-pr-iteration-started` | Worker iterating on PR feedback |
//! | `on-pr-iteration-done` | PR iteration complete          |
//!
//! # Event data
//!
//! Each hook receives event data as environment variables (e.g. `SIPAG_EVENT`,
//! `SIPAG_REPO`, `SIPAG_ISSUE`). See [`HookEvent::env_vars`] for the full list
//! emitted per event type.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use anyhow::Result;

/// A lifecycle event emitted by sipag at key worker milestones.
#[derive(Debug, Clone)]
pub enum HookEvent {
    /// A worker picked up an issue and is starting.
    WorkerStarted {
        repo: String,
        issue: u64,
        title: String,
        task_id: String,
    },
    /// A worker finished successfully and opened a PR.
    WorkerCompleted {
        repo: String,
        issue: u64,
        title: String,
        task_id: String,
        pr_num: Option<u64>,
        pr_url: Option<String>,
        duration: Duration,
    },
    /// A worker exited with a non-zero exit code.
    WorkerFailed {
        repo: String,
        issue: u64,
        title: String,
        task_id: String,
        exit_code: i32,
        log_path: String,
    },
    /// A PR was merged.
    PrMerged { repo: String, pr_num: u64 },
    /// A worker is starting an iteration on PR review feedback.
    PrIterationStarted {
        repo: String,
        pr_num: u64,
        issue: Option<u64>,
        issue_title: String,
    },
    /// A PR iteration worker has completed.
    PrIterationDone {
        repo: String,
        pr_num: u64,
        exit_code: i32,
    },
}

impl HookEvent {
    /// Returns the hook script name for this event (e.g. `on-worker-started`).
    pub fn hook_name(&self) -> &'static str {
        match self {
            HookEvent::WorkerStarted { .. } => "on-worker-started",
            HookEvent::WorkerCompleted { .. } => "on-worker-completed",
            HookEvent::WorkerFailed { .. } => "on-worker-failed",
            HookEvent::PrMerged { .. } => "on-pr-merged",
            HookEvent::PrIterationStarted { .. } => "on-pr-iteration-started",
            HookEvent::PrIterationDone { .. } => "on-pr-iteration-done",
        }
    }

    /// Returns the environment variables to set when running the hook script.
    pub fn env_vars(&self) -> Vec<(&'static str, String)> {
        match self {
            HookEvent::WorkerStarted {
                repo,
                issue,
                title,
                task_id,
            } => vec![
                ("SIPAG_EVENT", "worker.started".to_string()),
                ("SIPAG_REPO", repo.clone()),
                ("SIPAG_ISSUE", issue.to_string()),
                ("SIPAG_ISSUE_TITLE", title.clone()),
                ("SIPAG_TASK_ID", task_id.clone()),
            ],
            HookEvent::WorkerCompleted {
                repo,
                issue,
                title,
                task_id,
                pr_num,
                pr_url,
                duration,
            } => vec![
                ("SIPAG_EVENT", "worker.completed".to_string()),
                ("SIPAG_REPO", repo.clone()),
                ("SIPAG_ISSUE", issue.to_string()),
                ("SIPAG_ISSUE_TITLE", title.clone()),
                ("SIPAG_TASK_ID", task_id.clone()),
                (
                    "SIPAG_PR_NUM",
                    pr_num.map_or_else(String::new, |n| n.to_string()),
                ),
                ("SIPAG_PR_URL", pr_url.clone().unwrap_or_default()),
                ("SIPAG_DURATION", duration.as_secs().to_string()),
            ],
            HookEvent::WorkerFailed {
                repo,
                issue,
                title,
                task_id,
                exit_code,
                log_path,
            } => vec![
                ("SIPAG_EVENT", "worker.failed".to_string()),
                ("SIPAG_REPO", repo.clone()),
                ("SIPAG_ISSUE", issue.to_string()),
                ("SIPAG_ISSUE_TITLE", title.clone()),
                ("SIPAG_TASK_ID", task_id.clone()),
                ("SIPAG_EXIT_CODE", exit_code.to_string()),
                ("SIPAG_LOG_PATH", log_path.clone()),
            ],
            HookEvent::PrMerged { repo, pr_num } => vec![
                ("SIPAG_EVENT", "pr.merged".to_string()),
                ("SIPAG_REPO", repo.clone()),
                ("SIPAG_PR_NUM", pr_num.to_string()),
            ],
            HookEvent::PrIterationStarted {
                repo,
                pr_num,
                issue,
                issue_title,
            } => vec![
                ("SIPAG_EVENT", "pr-iteration.started".to_string()),
                ("SIPAG_REPO", repo.clone()),
                ("SIPAG_PR_NUM", pr_num.to_string()),
                (
                    "SIPAG_ISSUE",
                    issue.map_or_else(String::new, |n| n.to_string()),
                ),
                ("SIPAG_ISSUE_TITLE", issue_title.clone()),
            ],
            HookEvent::PrIterationDone {
                repo,
                pr_num,
                exit_code,
            } => vec![
                ("SIPAG_EVENT", "pr-iteration.done".to_string()),
                ("SIPAG_REPO", repo.clone()),
                ("SIPAG_PR_NUM", pr_num.to_string()),
                ("SIPAG_EXIT_CODE", exit_code.to_string()),
            ],
        }
    }
}

/// Trait for firing lifecycle hooks.
///
/// Implementations decide how hook events are dispatched. The primary
/// implementation is [`FileHookRunner`]; tests use a mock that records events.
pub trait HookRunner {
    /// Fire a lifecycle event.
    ///
    /// Implementations should run hooks asynchronously so they never block the
    /// caller. Missing or non-executable hooks must be silently skipped.
    fn fire(&self, event: HookEvent) -> Result<()>;
}

/// File-based hook runner that executes scripts from a hooks directory.
///
/// Scripts live at `<hooks_dir>/<event-hook-name>` (e.g.
/// `~/.sipag/hooks/on-worker-started`). Each script is spawned asynchronously
/// with the event's environment variables added to the child process
/// environment.
pub struct FileHookRunner {
    hooks_dir: PathBuf,
}

impl FileHookRunner {
    /// Create a new runner that looks for hook scripts in `hooks_dir`.
    pub fn new(hooks_dir: PathBuf) -> Self {
        Self { hooks_dir }
    }
}

impl HookRunner for FileHookRunner {
    fn fire(&self, event: HookEvent) -> Result<()> {
        let hook_path = self.hooks_dir.join(event.hook_name());

        // Silently skip missing or non-executable hooks (matches bash behaviour).
        if !is_executable(&hook_path) {
            return Ok(());
        }

        let mut cmd = Command::new(&hook_path);
        for (key, val) in event.env_vars() {
            cmd.env(key, val);
        }
        // Spawn and immediately detach — fire-and-forget, just like `"$hook_path" &`.
        cmd.spawn()?;

        Ok(())
    }
}

/// Returns true if `path` exists and has at least one executable bit set.
#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// On non-Unix targets just check for existence (best-effort).
#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    // ── MockHookRunner ────────────────────────────────────────────────────────

    /// A test double that records every event fired.
    pub struct MockHookRunner {
        events: RefCell<Vec<HookEvent>>,
    }

    impl MockHookRunner {
        pub fn new() -> Self {
            Self {
                events: RefCell::new(Vec::new()),
            }
        }

        pub fn fired_events(&self) -> std::cell::Ref<'_, Vec<HookEvent>> {
            self.events.borrow()
        }
    }

    impl HookRunner for MockHookRunner {
        fn fire(&self, event: HookEvent) -> Result<()> {
            self.events.borrow_mut().push(event);
            Ok(())
        }
    }

    // ── HookEvent::hook_name ─────────────────────────────────────────────────

    #[test]
    fn hook_name_worker_started() {
        let event = HookEvent::WorkerStarted {
            repo: "r".to_string(),
            issue: 1,
            title: "t".to_string(),
            task_id: "id".to_string(),
        };
        assert_eq!(event.hook_name(), "on-worker-started");
    }

    #[test]
    fn hook_name_worker_completed() {
        let event = HookEvent::WorkerCompleted {
            repo: "r".to_string(),
            issue: 1,
            title: "t".to_string(),
            task_id: "id".to_string(),
            pr_num: Some(7),
            pr_url: Some("https://example.com/pr/7".to_string()),
            duration: Duration::from_secs(300),
        };
        assert_eq!(event.hook_name(), "on-worker-completed");
    }

    #[test]
    fn hook_name_worker_failed() {
        let event = HookEvent::WorkerFailed {
            repo: "r".to_string(),
            issue: 1,
            title: "t".to_string(),
            task_id: "id".to_string(),
            exit_code: 1,
            log_path: "/tmp/foo.log".to_string(),
        };
        assert_eq!(event.hook_name(), "on-worker-failed");
    }

    #[test]
    fn hook_name_pr_merged() {
        let event = HookEvent::PrMerged {
            repo: "r".to_string(),
            pr_num: 5,
        };
        assert_eq!(event.hook_name(), "on-pr-merged");
    }

    #[test]
    fn hook_name_pr_iteration_started() {
        let event = HookEvent::PrIterationStarted {
            repo: "r".to_string(),
            pr_num: 5,
            issue: Some(3),
            issue_title: "t".to_string(),
        };
        assert_eq!(event.hook_name(), "on-pr-iteration-started");
    }

    #[test]
    fn hook_name_pr_iteration_done() {
        let event = HookEvent::PrIterationDone {
            repo: "r".to_string(),
            pr_num: 5,
            exit_code: 0,
        };
        assert_eq!(event.hook_name(), "on-pr-iteration-done");
    }

    // ── HookEvent::env_vars ──────────────────────────────────────────────────

    fn env_map(
        vars: Vec<(&'static str, String)>,
    ) -> std::collections::HashMap<&'static str, String> {
        vars.into_iter().collect()
    }

    #[test]
    fn env_vars_worker_started() {
        let event = HookEvent::WorkerStarted {
            repo: "Dorky-Robot/sipag".to_string(),
            issue: 42,
            title: "Fix auth".to_string(),
            task_id: "20260220-fix-auth".to_string(),
        };
        let vars = env_map(event.env_vars());
        assert_eq!(vars["SIPAG_EVENT"], "worker.started");
        assert_eq!(vars["SIPAG_REPO"], "Dorky-Robot/sipag");
        assert_eq!(vars["SIPAG_ISSUE"], "42");
        assert_eq!(vars["SIPAG_ISSUE_TITLE"], "Fix auth");
        assert_eq!(vars["SIPAG_TASK_ID"], "20260220-fix-auth");
    }

    #[test]
    fn env_vars_worker_completed_with_pr() {
        let event = HookEvent::WorkerCompleted {
            repo: "Dorky-Robot/sipag".to_string(),
            issue: 42,
            title: "Fix auth".to_string(),
            task_id: "20260220-fix-auth".to_string(),
            pr_num: Some(47),
            pr_url: Some("https://github.com/Dorky-Robot/sipag/pull/47".to_string()),
            duration: Duration::from_secs(503),
        };
        let vars = env_map(event.env_vars());
        assert_eq!(vars["SIPAG_EVENT"], "worker.completed");
        assert_eq!(vars["SIPAG_PR_NUM"], "47");
        assert_eq!(
            vars["SIPAG_PR_URL"],
            "https://github.com/Dorky-Robot/sipag/pull/47"
        );
        assert_eq!(vars["SIPAG_DURATION"], "503");
    }

    #[test]
    fn env_vars_worker_completed_no_pr() {
        let event = HookEvent::WorkerCompleted {
            repo: "r".to_string(),
            issue: 1,
            title: "t".to_string(),
            task_id: "id".to_string(),
            pr_num: None,
            pr_url: None,
            duration: Duration::from_secs(60),
        };
        let vars = env_map(event.env_vars());
        assert_eq!(vars["SIPAG_PR_NUM"], "");
        assert_eq!(vars["SIPAG_PR_URL"], "");
    }

    #[test]
    fn env_vars_worker_failed() {
        let event = HookEvent::WorkerFailed {
            repo: "Dorky-Robot/sipag".to_string(),
            issue: 42,
            title: "Fix auth".to_string(),
            task_id: "20260220-fix-auth".to_string(),
            exit_code: 1,
            log_path: "/tmp/sipag/issue-42.log".to_string(),
        };
        let vars = env_map(event.env_vars());
        assert_eq!(vars["SIPAG_EVENT"], "worker.failed");
        assert_eq!(vars["SIPAG_EXIT_CODE"], "1");
        assert_eq!(vars["SIPAG_LOG_PATH"], "/tmp/sipag/issue-42.log");
        assert_eq!(vars["SIPAG_TASK_ID"], "20260220-fix-auth");
    }

    #[test]
    fn env_vars_pr_merged() {
        let event = HookEvent::PrMerged {
            repo: "Dorky-Robot/sipag".to_string(),
            pr_num: 47,
        };
        let vars = env_map(event.env_vars());
        assert_eq!(vars["SIPAG_EVENT"], "pr.merged");
        assert_eq!(vars["SIPAG_REPO"], "Dorky-Robot/sipag");
        assert_eq!(vars["SIPAG_PR_NUM"], "47");
    }

    #[test]
    fn env_vars_pr_iteration_started_with_issue() {
        let event = HookEvent::PrIterationStarted {
            repo: "Dorky-Robot/sipag".to_string(),
            pr_num: 47,
            issue: Some(42),
            issue_title: "Fix auth".to_string(),
        };
        let vars = env_map(event.env_vars());
        assert_eq!(vars["SIPAG_EVENT"], "pr-iteration.started");
        assert_eq!(vars["SIPAG_PR_NUM"], "47");
        assert_eq!(vars["SIPAG_ISSUE"], "42");
        assert_eq!(vars["SIPAG_ISSUE_TITLE"], "Fix auth");
    }

    #[test]
    fn env_vars_pr_iteration_started_no_issue() {
        let event = HookEvent::PrIterationStarted {
            repo: "r".to_string(),
            pr_num: 5,
            issue: None,
            issue_title: "t".to_string(),
        };
        let vars = env_map(event.env_vars());
        assert_eq!(vars["SIPAG_ISSUE"], "");
    }

    #[test]
    fn env_vars_pr_iteration_done() {
        let event = HookEvent::PrIterationDone {
            repo: "Dorky-Robot/sipag".to_string(),
            pr_num: 47,
            exit_code: 0,
        };
        let vars = env_map(event.env_vars());
        assert_eq!(vars["SIPAG_EVENT"], "pr-iteration.done");
        assert_eq!(vars["SIPAG_EXIT_CODE"], "0");
        assert_eq!(vars["SIPAG_REPO"], "Dorky-Robot/sipag");
        assert_eq!(vars["SIPAG_PR_NUM"], "47");
    }

    // ── MockHookRunner verifies events are fired ─────────────────────────────

    #[test]
    fn mock_records_worker_started() {
        let runner = MockHookRunner::new();
        runner
            .fire(HookEvent::WorkerStarted {
                repo: "Dorky-Robot/sipag".to_string(),
                issue: 42,
                title: "Fix auth".to_string(),
                task_id: "id-1".to_string(),
            })
            .unwrap();
        assert_eq!(runner.fired_events().len(), 1);
        assert!(matches!(
            runner.fired_events()[0],
            HookEvent::WorkerStarted { issue: 42, .. }
        ));
    }

    #[test]
    fn mock_records_multiple_events() {
        let runner = MockHookRunner::new();
        runner
            .fire(HookEvent::WorkerStarted {
                repo: "r".to_string(),
                issue: 1,
                title: "t".to_string(),
                task_id: "id".to_string(),
            })
            .unwrap();
        runner
            .fire(HookEvent::WorkerCompleted {
                repo: "r".to_string(),
                issue: 1,
                title: "t".to_string(),
                task_id: "id".to_string(),
                pr_num: Some(2),
                pr_url: None,
                duration: Duration::from_secs(100),
            })
            .unwrap();
        assert_eq!(runner.fired_events().len(), 2);
        assert!(matches!(
            runner.fired_events()[1],
            HookEvent::WorkerCompleted {
                pr_num: Some(2),
                ..
            }
        ));
    }

    // ── FileHookRunner: missing hook is silently skipped ─────────────────────

    #[test]
    fn file_hook_runner_skips_missing_hook() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FileHookRunner::new(dir.path().to_path_buf());
        // No script exists — fire must return Ok without error.
        let result = runner.fire(HookEvent::PrMerged {
            repo: "r".to_string(),
            pr_num: 1,
        });
        assert!(result.is_ok());
    }

    // ── FileHookRunner: non-executable hook is silently skipped ──────────────

    #[cfg(unix)]
    #[test]
    fn file_hook_runner_skips_non_executable_hook() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let hook_path = dir.path().join("on-pr-merged");
        fs::write(&hook_path, "#!/usr/bin/env bash\necho hello\n").unwrap();
        // Remove execute bits — script exists but is not executable.
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o644)).unwrap();

        let runner = FileHookRunner::new(dir.path().to_path_buf());
        let result = runner.fire(HookEvent::PrMerged {
            repo: "r".to_string(),
            pr_num: 1,
        });
        assert!(result.is_ok());
    }

    // ── FileHookRunner: executable hook is spawned ────────────────────────────

    #[cfg(unix)]
    #[test]
    fn file_hook_runner_spawns_executable_hook() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let output_file = dir.path().join("hook_ran");
        let hook_path = dir.path().join("on-worker-started");

        // Write a hook that creates a sentinel file.
        fs::write(
            &hook_path,
            format!("#!/usr/bin/env bash\ntouch {}\n", output_file.display()),
        )
        .unwrap();
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();

        let runner = FileHookRunner::new(dir.path().to_path_buf());
        runner
            .fire(HookEvent::WorkerStarted {
                repo: "r".to_string(),
                issue: 1,
                title: "t".to_string(),
                task_id: "id".to_string(),
            })
            .unwrap();

        // Poll for the sentinel file (hooks are fire-and-forget / async).
        let mut found = false;
        for _ in 0..20 {
            if output_file.exists() {
                found = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(found, "hook script should have created sentinel file");
    }

    // ── FileHookRunner: env vars are passed to the hook script ────────────────

    #[cfg(unix)]
    #[test]
    fn file_hook_runner_passes_env_vars() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let output_file = dir.path().join("env_output");
        let hook_path = dir.path().join("on-worker-started");

        // Write a hook that captures SIPAG_ISSUE to a file.
        fs::write(
            &hook_path,
            format!(
                "#!/usr/bin/env bash\necho \"$SIPAG_ISSUE\" > {}\n",
                output_file.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();

        let runner = FileHookRunner::new(dir.path().to_path_buf());
        runner
            .fire(HookEvent::WorkerStarted {
                repo: "r".to_string(),
                issue: 99,
                title: "t".to_string(),
                task_id: "id".to_string(),
            })
            .unwrap();

        // Poll for the output file (hooks are fire-and-forget / async).
        let mut content = String::new();
        for _ in 0..20 {
            if let Ok(c) = fs::read_to_string(&output_file) {
                content = c;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert_eq!(content.trim(), "99");
    }
}
