use anyhow::Result;
use std::time::Duration;

use super::decision::{decide_finalization, FinalizationResult};
use super::ports::{ContainerRuntime, GitHubGateway, StateStore};
use super::status::WorkerStatus;

/// Workers with no heartbeat update for this long are considered stale.
const STALE_HEARTBEAT_THRESHOLD: Duration = Duration::from_secs(10 * 60); // 10 minutes

/// Result of processing one worker during recovery/finalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryOutcome {
    /// Container is still running — left as-is.
    StillRunning { issue_num: u64 },
    /// Container is running but heartbeat is stale — may be stuck.
    StaleHeartbeat { issue_num: u64 },
    /// Container exited — state updated to done or failed.
    Finalized {
        issue_num: u64,
        status: WorkerStatus,
    },
    /// Stale "recovering" status reset back to "running".
    ResetToRunning { issue_num: u64 },
}

/// Recover and finalize all active workers.
///
/// Scans for "running" and "recovering" state files, checks container status,
/// and finalizes any whose containers have exited. Pure orchestration — all
/// side effects go through the ports.
///
/// This function is called:
/// - Once on startup (recovery)
/// - At the top of each poll cycle (finalization of exited containers)
pub fn recover_and_finalize(
    containers: &dyn ContainerRuntime,
    github: &dyn GitHubGateway,
    store: &dyn StateStore,
    work_label: &str,
) -> Result<Vec<RecoveryOutcome>> {
    let active_workers = store.list_active()?;
    let mut outcomes = Vec::new();

    for worker in &active_workers {
        let container_alive = containers.is_running(&worker.container_name)?;
        let pr = github.find_pr_for_branch(&worker.repo, &worker.branch)?;

        let result = decide_finalization(container_alive, pr.is_some());

        match result {
            FinalizationResult::StillRunning => {
                if worker.status == WorkerStatus::Recovering {
                    let mut updated = worker.clone();
                    updated.status = WorkerStatus::Running;
                    store.save(&updated)?;
                    outcomes.push(RecoveryOutcome::ResetToRunning {
                        issue_num: worker.issue_num,
                    });
                } else if is_heartbeat_stale(&worker.last_heartbeat) {
                    outcomes.push(RecoveryOutcome::StaleHeartbeat {
                        issue_num: worker.issue_num,
                    });
                } else {
                    outcomes.push(RecoveryOutcome::StillRunning {
                        issue_num: worker.issue_num,
                    });
                }
            }
            FinalizationResult::Done => {
                let _ = github.transition_label(
                    &worker.repo,
                    worker.issue_num,
                    Some("in-progress"),
                    None,
                );
                let mut updated = worker.clone();
                updated.status = WorkerStatus::Done;
                if let Some(pr_info) = &pr {
                    updated.pr_num = Some(pr_info.number);
                    updated.pr_url = Some(pr_info.url.clone());
                }
                store.save(&updated)?;
                outcomes.push(RecoveryOutcome::Finalized {
                    issue_num: worker.issue_num,
                    status: WorkerStatus::Done,
                });
            }
            FinalizationResult::Failed => {
                let _ = github.transition_label(
                    &worker.repo,
                    worker.issue_num,
                    Some("in-progress"),
                    Some(work_label),
                );
                let mut updated = worker.clone();
                updated.status = WorkerStatus::Failed;
                store.save(&updated)?;
                outcomes.push(RecoveryOutcome::Finalized {
                    issue_num: worker.issue_num,
                    status: WorkerStatus::Failed,
                });
            }
        }
    }

    Ok(outcomes)
}

/// Check if a heartbeat timestamp is older than the stale threshold.
///
/// Returns `false` if the heartbeat is `None` (workers without heartbeat
/// support are not considered stale — they predate the feature).
fn is_heartbeat_stale(last_heartbeat: &Option<String>) -> bool {
    let Some(hb) = last_heartbeat else {
        return false;
    };

    // Parse ISO 8601 timestamp: "2024-01-15T10:30:00Z"
    let Ok(hb_time) = chrono::DateTime::parse_from_rfc3339(hb) else {
        // Also try the format without timezone offset: "2024-01-15T10:30:00Z" sometimes
        // parsed differently. Fall back to non-stale if unparsable.
        return false;
    };

    let now = chrono::Utc::now();
    let age = now.signed_duration_since(hb_time);

    age > chrono::Duration::from_std(STALE_HEARTBEAT_THRESHOLD).unwrap_or(chrono::Duration::MAX)
}

#[cfg(test)]
mod tests {
    use super::super::ports::PrInfo;
    use super::super::state::WorkerState;
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    // ── Mock: ContainerRuntime ───────────────────────────────────────────────

    struct MockContainers {
        running: Vec<String>,
    }

    impl ContainerRuntime for MockContainers {
        fn is_running(&self, name: &str) -> Result<bool> {
            Ok(self.running.contains(&name.to_string()))
        }
    }

    // ── Mock: GitHubGateway ──────────────────────────────────────────────────

    struct MockGitHub {
        prs: HashMap<String, PrInfo>,
        label_calls: RefCell<Vec<LabelCall>>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct LabelCall {
        repo: String,
        issue: u64,
        remove: Option<String>,
        add: Option<String>,
    }

    impl MockGitHub {
        fn new() -> Self {
            Self {
                prs: HashMap::new(),
                label_calls: RefCell::new(Vec::new()),
            }
        }

        fn with_pr(mut self, branch: &str, number: u64) -> Self {
            self.prs.insert(
                branch.to_string(),
                PrInfo {
                    number,
                    url: format!("https://github.com/test/repo/pull/{}", number),
                },
            );
            self
        }
    }

    impl GitHubGateway for MockGitHub {
        fn find_pr_for_branch(&self, _repo: &str, branch: &str) -> Result<Option<PrInfo>> {
            Ok(self.prs.get(branch).cloned())
        }

        fn transition_label(
            &self,
            repo: &str,
            issue: u64,
            remove: Option<&str>,
            add: Option<&str>,
        ) -> Result<()> {
            self.label_calls.borrow_mut().push(LabelCall {
                repo: repo.to_string(),
                issue,
                remove: remove.map(|s| s.to_string()),
                add: add.map(|s| s.to_string()),
            });
            Ok(())
        }
    }

    // ── Mock: StateStore ─────────────────────────────────────────────────────

    struct MockStore {
        workers: RefCell<Vec<WorkerState>>,
    }

    impl MockStore {
        fn new(workers: Vec<WorkerState>) -> Self {
            Self {
                workers: RefCell::new(workers),
            }
        }

        fn get_status(&self, issue_num: u64) -> Option<WorkerStatus> {
            self.workers
                .borrow()
                .iter()
                .find(|w| w.issue_num == issue_num)
                .map(|w| w.status)
        }

        fn get_pr_num(&self, issue_num: u64) -> Option<u64> {
            self.workers
                .borrow()
                .iter()
                .find(|w| w.issue_num == issue_num)
                .and_then(|w| w.pr_num)
        }

        fn get_pr_url(&self, issue_num: u64) -> Option<String> {
            self.workers
                .borrow()
                .iter()
                .find(|w| w.issue_num == issue_num)
                .and_then(|w| w.pr_url.clone())
        }
    }

    impl StateStore for MockStore {
        fn load(&self, _repo_slug: &str, issue_num: u64) -> Result<Option<WorkerState>> {
            Ok(self
                .workers
                .borrow()
                .iter()
                .find(|w| w.issue_num == issue_num)
                .cloned())
        }

        fn save(&self, state: &WorkerState) -> Result<()> {
            let mut workers = self.workers.borrow_mut();
            if let Some(existing) = workers.iter_mut().find(|w| w.issue_num == state.issue_num) {
                *existing = state.clone();
            } else {
                workers.push(state.clone());
            }
            Ok(())
        }

        fn list_active(&self) -> Result<Vec<WorkerState>> {
            Ok(self
                .workers
                .borrow()
                .iter()
                .filter(|w| w.status.is_active())
                .cloned()
                .collect())
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn make_worker(issue_num: u64, status: WorkerStatus) -> WorkerState {
        WorkerState {
            repo: "test/repo".to_string(),
            issue_num,
            issue_title: format!("Issue {}", issue_num),
            branch: format!("sipag/issue-{}-test", issue_num),
            container_name: format!("sipag-issue-{}", issue_num),
            pr_num: None,
            pr_url: None,
            status,
            started_at: Some("2024-01-01T00:00:00Z".to_string()),
            ended_at: None,
            duration_s: None,
            exit_code: None,
            log_path: None,
            last_heartbeat: None,
            phase: None,
        }
    }

    // ── Tests ────────────────────────────────────────────────────────────────

    #[test]
    fn running_container_stays_running() {
        let store = MockStore::new(vec![make_worker(42, WorkerStatus::Running)]);
        let containers = MockContainers {
            running: vec!["sipag-issue-42".to_string()],
        };
        let github = MockGitHub::new();

        let outcomes = recover_and_finalize(&containers, &github, &store, "approved").unwrap();

        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0], RecoveryOutcome::StillRunning { issue_num: 42 });
        assert_eq!(store.get_status(42), Some(WorkerStatus::Running));
    }

    #[test]
    fn recovering_container_alive_resets_to_running() {
        let store = MockStore::new(vec![make_worker(42, WorkerStatus::Recovering)]);
        let containers = MockContainers {
            running: vec!["sipag-issue-42".to_string()],
        };
        let github = MockGitHub::new();

        let outcomes = recover_and_finalize(&containers, &github, &store, "approved").unwrap();

        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            outcomes[0],
            RecoveryOutcome::ResetToRunning { issue_num: 42 }
        );
        assert_eq!(store.get_status(42), Some(WorkerStatus::Running));
    }

    #[test]
    fn gone_container_with_pr_finalized_as_done() {
        let store = MockStore::new(vec![make_worker(42, WorkerStatus::Running)]);
        let containers = MockContainers { running: vec![] };
        let github = MockGitHub::new().with_pr("sipag/issue-42-test", 100);

        let outcomes = recover_and_finalize(&containers, &github, &store, "approved").unwrap();

        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            outcomes[0],
            RecoveryOutcome::Finalized {
                issue_num: 42,
                status: WorkerStatus::Done,
            }
        );
        assert_eq!(store.get_status(42), Some(WorkerStatus::Done));
    }

    #[test]
    fn gone_container_without_pr_finalized_as_failed() {
        let store = MockStore::new(vec![make_worker(42, WorkerStatus::Running)]);
        let containers = MockContainers { running: vec![] };
        let github = MockGitHub::new();

        let outcomes = recover_and_finalize(&containers, &github, &store, "approved").unwrap();

        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            outcomes[0],
            RecoveryOutcome::Finalized {
                issue_num: 42,
                status: WorkerStatus::Failed,
            }
        );
        assert_eq!(store.get_status(42), Some(WorkerStatus::Failed));
    }

    #[test]
    fn terminal_workers_not_processed() {
        let store = MockStore::new(vec![
            make_worker(42, WorkerStatus::Done),
            make_worker(43, WorkerStatus::Failed),
        ]);
        let containers = MockContainers { running: vec![] };
        let github = MockGitHub::new();

        let outcomes = recover_and_finalize(&containers, &github, &store, "approved").unwrap();

        assert!(outcomes.is_empty());
    }

    #[test]
    fn multiple_workers_processed_independently() {
        let store = MockStore::new(vec![
            make_worker(42, WorkerStatus::Running),    // alive
            make_worker(43, WorkerStatus::Running),    // gone, has PR
            make_worker(44, WorkerStatus::Recovering), // gone, no PR
        ]);
        let containers = MockContainers {
            running: vec!["sipag-issue-42".to_string()],
        };
        let github = MockGitHub::new().with_pr("sipag/issue-43-test", 100);

        let outcomes = recover_and_finalize(&containers, &github, &store, "approved").unwrap();

        assert_eq!(outcomes.len(), 3);
        assert_eq!(outcomes[0], RecoveryOutcome::StillRunning { issue_num: 42 });
        assert_eq!(
            outcomes[1],
            RecoveryOutcome::Finalized {
                issue_num: 43,
                status: WorkerStatus::Done,
            }
        );
        assert_eq!(
            outcomes[2],
            RecoveryOutcome::Finalized {
                issue_num: 44,
                status: WorkerStatus::Failed,
            }
        );
    }

    #[test]
    fn done_finalization_removes_in_progress_label() {
        let store = MockStore::new(vec![make_worker(42, WorkerStatus::Running)]);
        let containers = MockContainers { running: vec![] };
        let github = MockGitHub::new().with_pr("sipag/issue-42-test", 100);

        recover_and_finalize(&containers, &github, &store, "approved").unwrap();

        let calls = github.label_calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0],
            LabelCall {
                repo: "test/repo".to_string(),
                issue: 42,
                remove: Some("in-progress".to_string()),
                add: None,
            }
        );
    }

    #[test]
    fn failed_finalization_restores_work_label() {
        let store = MockStore::new(vec![make_worker(42, WorkerStatus::Running)]);
        let containers = MockContainers { running: vec![] };
        let github = MockGitHub::new();

        recover_and_finalize(&containers, &github, &store, "approved").unwrap();

        let calls = github.label_calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0],
            LabelCall {
                repo: "test/repo".to_string(),
                issue: 42,
                remove: Some("in-progress".to_string()),
                add: Some("approved".to_string()),
            }
        );
    }

    #[test]
    fn pr_info_stored_on_done() {
        let store = MockStore::new(vec![make_worker(42, WorkerStatus::Running)]);
        let containers = MockContainers { running: vec![] };
        let github = MockGitHub::new().with_pr("sipag/issue-42-test", 100);

        recover_and_finalize(&containers, &github, &store, "approved").unwrap();

        assert_eq!(store.get_pr_num(42), Some(100));
        assert_eq!(
            store.get_pr_url(42),
            Some("https://github.com/test/repo/pull/100".to_string())
        );
    }

    #[test]
    fn recovering_gone_no_pr_finalized_as_failed() {
        let store = MockStore::new(vec![make_worker(42, WorkerStatus::Recovering)]);
        let containers = MockContainers { running: vec![] };
        let github = MockGitHub::new();

        let outcomes = recover_and_finalize(&containers, &github, &store, "approved").unwrap();

        assert_eq!(
            outcomes[0],
            RecoveryOutcome::Finalized {
                issue_num: 42,
                status: WorkerStatus::Failed,
            }
        );
    }

    #[test]
    fn enqueued_worker_finalized_as_failed() {
        // An enqueued worker has no container (it was never started).
        // Recovery should finalize it as failed so it gets re-dispatched.
        let store = MockStore::new(vec![make_worker(42, WorkerStatus::Enqueued)]);
        let containers = MockContainers { running: vec![] };
        let github = MockGitHub::new();

        let outcomes = recover_and_finalize(&containers, &github, &store, "approved").unwrap();

        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            outcomes[0],
            RecoveryOutcome::Finalized {
                issue_num: 42,
                status: WorkerStatus::Failed,
            }
        );
        assert_eq!(store.get_status(42), Some(WorkerStatus::Failed));
    }

    #[test]
    fn enqueued_worker_restores_work_label() {
        // Recovery of an enqueued worker should restore the approved label
        // (removing in-progress if it was set before the crash).
        let store = MockStore::new(vec![make_worker(42, WorkerStatus::Enqueued)]);
        let containers = MockContainers { running: vec![] };
        let github = MockGitHub::new();

        recover_and_finalize(&containers, &github, &store, "approved").unwrap();

        let calls = github.label_calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0],
            LabelCall {
                repo: "test/repo".to_string(),
                issue: 42,
                remove: Some("in-progress".to_string()),
                add: Some("approved".to_string()),
            }
        );
    }

    #[test]
    fn empty_store_returns_no_outcomes() {
        let store = MockStore::new(vec![]);
        let containers = MockContainers { running: vec![] };
        let github = MockGitHub::new();

        let outcomes = recover_and_finalize(&containers, &github, &store, "approved").unwrap();

        assert!(outcomes.is_empty());
    }

    #[test]
    fn custom_work_label_used_on_failure() {
        let store = MockStore::new(vec![make_worker(42, WorkerStatus::Running)]);
        let containers = MockContainers { running: vec![] };
        let github = MockGitHub::new();

        recover_and_finalize(&containers, &github, &store, "ready").unwrap();

        let calls = github.label_calls.borrow();
        assert_eq!(calls[0].add, Some("ready".to_string()));
    }

    #[test]
    fn running_with_fresh_heartbeat_stays_running() {
        let mut worker = make_worker(42, WorkerStatus::Running);
        worker.last_heartbeat = Some(chrono::Utc::now().to_rfc3339());
        let store = MockStore::new(vec![worker]);
        let containers = MockContainers {
            running: vec!["sipag-issue-42".to_string()],
        };
        let github = MockGitHub::new();

        let outcomes = recover_and_finalize(&containers, &github, &store, "approved").unwrap();

        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0], RecoveryOutcome::StillRunning { issue_num: 42 });
    }

    #[test]
    fn running_with_stale_heartbeat_flagged() {
        let mut worker = make_worker(42, WorkerStatus::Running);
        // 15 minutes ago — exceeds the 10-minute threshold.
        let stale_time = chrono::Utc::now() - chrono::Duration::minutes(15);
        worker.last_heartbeat = Some(stale_time.to_rfc3339());
        let store = MockStore::new(vec![worker]);
        let containers = MockContainers {
            running: vec!["sipag-issue-42".to_string()],
        };
        let github = MockGitHub::new();

        let outcomes = recover_and_finalize(&containers, &github, &store, "approved").unwrap();

        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            outcomes[0],
            RecoveryOutcome::StaleHeartbeat { issue_num: 42 }
        );
    }

    #[test]
    fn running_without_heartbeat_not_stale() {
        // Workers without heartbeat support (None) should not be flagged.
        let worker = make_worker(42, WorkerStatus::Running);
        assert!(worker.last_heartbeat.is_none());
        let store = MockStore::new(vec![worker]);
        let containers = MockContainers {
            running: vec!["sipag-issue-42".to_string()],
        };
        let github = MockGitHub::new();

        let outcomes = recover_and_finalize(&containers, &github, &store, "approved").unwrap();

        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0], RecoveryOutcome::StillRunning { issue_num: 42 });
    }
}
