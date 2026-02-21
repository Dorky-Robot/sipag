//! Typed replacements for `lib/worker/dedup.sh`.
//!
//! Provides:
//! - Pure query functions that inspect [`WorkerState`]/[`WorkerStatus`] without I/O.
//! - [`PrIterationTracker`]: typed replacement for marker-file PR iteration tracking.
//! - [`ConflictFixTracker`]: typed replacement for conflict-fix marker files.
//! - [`mark_state_done`]: [`StateStore`]-backed replacement for `worker_mark_state_done`.
//!
//! All state queries use [`StateStore::load`] rather than `jq`. Slug generation
//! uses [`crate::task::naming::slugify`] (already in Rust).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::decision::decide_issue_action;
use super::ports::StateStore;
use super::state::WorkerState;
use super::status::WorkerStatus;

// ── Pure query functions ──────────────────────────────────────────────────────

/// Return `true` if a new worker should be dispatched for this issue.
///
/// Thin wrapper over [`decide_issue_action`] for call sites that need a bool.
/// Replaces the `worker_is_completed` + `worker_is_in_flight` + `worker_is_failed`
/// combination in `lib/worker/loop.sh`.
pub fn is_dispatchable(state: Option<&WorkerState>, has_pr: bool) -> bool {
    decide_issue_action(state.map(|s| s.status), has_pr).is_dispatch()
}

/// Return `true` if the worker has completed successfully.
///
/// Replaces `worker_is_completed` from `lib/worker/dedup.sh`.
pub fn is_completed(state: Option<&WorkerState>) -> bool {
    state.is_some_and(|s| s.status == WorkerStatus::Done)
}

/// Return `true` if the worker is actively processing (enqueued, running, or recovering).
///
/// Replaces `worker_is_in_flight` from `lib/worker/dedup.sh`.
pub fn is_in_flight(state: Option<&WorkerState>) -> bool {
    state.is_some_and(|s| s.status.is_active())
}

/// Return `true` if the worker's previous attempt failed.
///
/// Replaces `worker_is_failed` from `lib/worker/dedup.sh`.
pub fn is_failed(state: Option<&WorkerState>) -> bool {
    state.is_some_and(|s| s.status == WorkerStatus::Failed)
}

// ── mark_state_done ───────────────────────────────────────────────────────────

/// Mark a worker state as done, creating a minimal record if none exists yet.
///
/// Replaces `worker_mark_state_done` from `lib/worker/dedup.sh`.
///
/// - Existing state: transitions to `Done`, preserves an existing `ended_at`,
///   and only overwrites `pr_num`/`pr_url` when the caller supplies `Some` values.
/// - No existing state: writes a minimal `Done` record (empty title/branch/container).
pub fn mark_state_done(
    store: &dyn StateStore,
    repo: &str,
    issue_num: u64,
    pr_num: Option<u64>,
    pr_url: Option<String>,
) -> Result<()> {
    let repo_slug = repo.replace('/', "--");
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let state = match store.load(&repo_slug, issue_num)? {
        Some(mut existing) => {
            existing.status = WorkerStatus::Done;
            if existing.ended_at.is_none() {
                existing.ended_at = Some(now);
            }
            if pr_num.is_some() {
                existing.pr_num = pr_num;
            }
            if pr_url.is_some() {
                existing.pr_url = pr_url;
            }
            existing
        }
        None => WorkerState {
            repo: repo.to_string(),
            issue_num,
            issue_title: String::new(),
            branch: String::new(),
            container_name: String::new(),
            pr_num,
            pr_url,
            status: WorkerStatus::Done,
            started_at: None,
            ended_at: Some(now),
            duration_s: None,
            exit_code: None,
            log_path: None,
        },
    };

    store.save(&state)
}

// ── PrIterationTracker ────────────────────────────────────────────────────────

/// Tracks which PRs currently have an iteration worker running.
///
/// Replaces the `worker_pr_is_running` / `worker_pr_mark_running` /
/// `worker_pr_mark_done` marker files in `lib/worker/dedup.sh`.
///
/// Marker files are transient — they live in the logs directory and are not
/// preserved across process restarts, matching the bash behaviour.
pub struct PrIterationTracker {
    dir: PathBuf,
}

impl PrIterationTracker {
    /// Create a tracker backed by `log_dir` (typically `~/.sipag/logs`).
    pub fn new(log_dir: &Path) -> Self {
        Self {
            dir: log_dir.to_path_buf(),
        }
    }

    fn marker_path(&self, pr_num: u64) -> PathBuf {
        self.dir.join(format!("pr-{}-running", pr_num))
    }

    /// Return `true` if a PR iteration worker is currently running for this PR.
    pub fn is_running(&self, pr_num: u64) -> bool {
        self.marker_path(pr_num).exists()
    }

    /// Mark a PR iteration worker as running (creates the marker file).
    pub fn mark_running(&self, pr_num: u64) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        fs::write(self.marker_path(pr_num), "")?;
        Ok(())
    }

    /// Mark a PR iteration worker as done (removes the marker file).
    pub fn mark_done(&self, pr_num: u64) -> Result<()> {
        let path = self.marker_path(pr_num);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

// ── ConflictFixTracker ────────────────────────────────────────────────────────

/// Tracks which PRs currently have a conflict-fix worker running.
///
/// Replaces the `worker_conflict_fix_is_running` / `worker_conflict_fix_mark_running` /
/// `worker_conflict_fix_mark_done` marker files in `lib/worker/dedup.sh`.
pub struct ConflictFixTracker {
    dir: PathBuf,
}

impl ConflictFixTracker {
    /// Create a tracker backed by `log_dir` (typically `~/.sipag/logs`).
    pub fn new(log_dir: &Path) -> Self {
        Self {
            dir: log_dir.to_path_buf(),
        }
    }

    fn marker_path(&self, pr_num: u64) -> PathBuf {
        self.dir.join(format!("pr-{}-conflict-fix-running", pr_num))
    }

    /// Return `true` if a conflict-fix worker is currently running for this PR.
    pub fn is_running(&self, pr_num: u64) -> bool {
        self.marker_path(pr_num).exists()
    }

    /// Mark a conflict-fix worker as running (creates the marker file).
    pub fn mark_running(&self, pr_num: u64) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        fs::write(self.marker_path(pr_num), "")?;
        Ok(())
    }

    /// Mark a conflict-fix worker as done (removes the marker file).
    pub fn mark_done(&self, pr_num: u64) -> Result<()> {
        let path = self.marker_path(pr_num);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use tempfile::TempDir;

    // ── MockStore ────────────────────────────────────────────────────────────

    struct MockStore {
        workers: RefCell<Vec<WorkerState>>,
    }

    impl MockStore {
        fn new(workers: Vec<WorkerState>) -> Self {
            Self {
                workers: RefCell::new(workers),
            }
        }

        fn get_state(&self, issue_num: u64) -> Option<WorkerState> {
            self.workers
                .borrow()
                .iter()
                .find(|w| w.issue_num == issue_num)
                .cloned()
        }
    }

    impl StateStore for MockStore {
        fn load(&self, _repo_slug: &str, issue_num: u64) -> Result<Option<WorkerState>> {
            Ok(self.get_state(issue_num))
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
        }
    }

    // ── is_dispatchable ──────────────────────────────────────────────────────

    #[test]
    fn dispatchable_new_issue_no_pr() {
        assert!(is_dispatchable(None, false));
    }

    #[test]
    fn not_dispatchable_new_issue_with_existing_pr() {
        assert!(!is_dispatchable(None, true));
    }

    #[test]
    fn not_dispatchable_done_issue() {
        let w = make_worker(1, WorkerStatus::Done);
        assert!(!is_dispatchable(Some(&w), false));
        assert!(!is_dispatchable(Some(&w), true));
    }

    #[test]
    fn not_dispatchable_enqueued_issue() {
        let w = make_worker(1, WorkerStatus::Enqueued);
        assert!(!is_dispatchable(Some(&w), false));
    }

    #[test]
    fn not_dispatchable_running_issue() {
        let w = make_worker(1, WorkerStatus::Running);
        assert!(!is_dispatchable(Some(&w), false));
    }

    #[test]
    fn not_dispatchable_recovering_issue() {
        let w = make_worker(1, WorkerStatus::Recovering);
        assert!(!is_dispatchable(Some(&w), false));
    }

    #[test]
    fn dispatchable_failed_issue() {
        let w = make_worker(1, WorkerStatus::Failed);
        assert!(is_dispatchable(Some(&w), false));
        assert!(is_dispatchable(Some(&w), true));
    }

    // ── is_completed ─────────────────────────────────────────────────────────

    #[test]
    fn completed_when_done() {
        let w = make_worker(1, WorkerStatus::Done);
        assert!(is_completed(Some(&w)));
    }

    #[test]
    fn not_completed_when_running() {
        let w = make_worker(1, WorkerStatus::Running);
        assert!(!is_completed(Some(&w)));
    }

    #[test]
    fn not_completed_when_failed() {
        let w = make_worker(1, WorkerStatus::Failed);
        assert!(!is_completed(Some(&w)));
    }

    #[test]
    fn not_completed_when_none() {
        assert!(!is_completed(None));
    }

    // ── is_in_flight ─────────────────────────────────────────────────────────

    #[test]
    fn in_flight_when_enqueued() {
        let w = make_worker(1, WorkerStatus::Enqueued);
        assert!(is_in_flight(Some(&w)));
    }

    #[test]
    fn in_flight_when_running() {
        let w = make_worker(1, WorkerStatus::Running);
        assert!(is_in_flight(Some(&w)));
    }

    #[test]
    fn in_flight_when_recovering() {
        let w = make_worker(1, WorkerStatus::Recovering);
        assert!(is_in_flight(Some(&w)));
    }

    #[test]
    fn not_in_flight_when_done() {
        let w = make_worker(1, WorkerStatus::Done);
        assert!(!is_in_flight(Some(&w)));
    }

    #[test]
    fn not_in_flight_when_failed() {
        let w = make_worker(1, WorkerStatus::Failed);
        assert!(!is_in_flight(Some(&w)));
    }

    #[test]
    fn not_in_flight_when_none() {
        assert!(!is_in_flight(None));
    }

    // ── is_failed ────────────────────────────────────────────────────────────

    #[test]
    fn failed_when_failed() {
        let w = make_worker(1, WorkerStatus::Failed);
        assert!(is_failed(Some(&w)));
    }

    #[test]
    fn not_failed_when_done() {
        let w = make_worker(1, WorkerStatus::Done);
        assert!(!is_failed(Some(&w)));
    }

    #[test]
    fn not_failed_when_running() {
        let w = make_worker(1, WorkerStatus::Running);
        assert!(!is_failed(Some(&w)));
    }

    #[test]
    fn not_failed_when_none() {
        assert!(!is_failed(None));
    }

    // ── mark_state_done ──────────────────────────────────────────────────────

    #[test]
    fn mark_done_updates_existing_state() {
        let store = MockStore::new(vec![make_worker(42, WorkerStatus::Running)]);
        mark_state_done(
            &store,
            "test/repo",
            42,
            Some(100),
            Some("https://example.com/pr/100".to_string()),
        )
        .unwrap();

        let state = store.get_state(42).unwrap();
        assert_eq!(state.status, WorkerStatus::Done);
        assert_eq!(state.pr_num, Some(100));
        assert_eq!(state.pr_url, Some("https://example.com/pr/100".to_string()));
        assert!(state.ended_at.is_some());
    }

    #[test]
    fn mark_done_creates_minimal_state_when_none_exists() {
        let store = MockStore::new(vec![]);
        mark_state_done(&store, "test/repo", 99, None, None).unwrap();

        let state = store.get_state(99).unwrap();
        assert_eq!(state.status, WorkerStatus::Done);
        assert_eq!(state.repo, "test/repo");
        assert_eq!(state.issue_num, 99);
        assert!(state.ended_at.is_some());
        assert!(state.pr_num.is_none());
    }

    #[test]
    fn mark_done_preserves_existing_ended_at() {
        let mut w = make_worker(42, WorkerStatus::Running);
        w.ended_at = Some("2024-01-01T10:00:00Z".to_string());
        let store = MockStore::new(vec![w]);

        mark_state_done(&store, "test/repo", 42, None, None).unwrap();

        let state = store.get_state(42).unwrap();
        assert_eq!(state.ended_at, Some("2024-01-01T10:00:00Z".to_string()));
    }

    #[test]
    fn mark_done_does_not_overwrite_pr_when_none_given() {
        let mut w = make_worker(42, WorkerStatus::Running);
        w.pr_num = Some(77);
        w.pr_url = Some("https://example.com/77".to_string());
        let store = MockStore::new(vec![w]);

        mark_state_done(&store, "test/repo", 42, None, None).unwrap();

        let state = store.get_state(42).unwrap();
        assert_eq!(state.pr_num, Some(77));
        assert_eq!(state.pr_url, Some("https://example.com/77".to_string()));
    }

    #[test]
    fn mark_done_with_pr_num_only() {
        let store = MockStore::new(vec![make_worker(42, WorkerStatus::Running)]);
        mark_state_done(&store, "test/repo", 42, Some(55), None).unwrap();

        let state = store.get_state(42).unwrap();
        assert_eq!(state.pr_num, Some(55));
        assert!(state.pr_url.is_none());
    }

    // ── PrIterationTracker ───────────────────────────────────────────────────

    #[test]
    fn pr_tracker_new_pr_is_not_running() {
        let dir = TempDir::new().unwrap();
        let tracker = PrIterationTracker::new(dir.path());
        assert!(!tracker.is_running(163));
    }

    #[test]
    fn pr_tracker_mark_running_sets_marker() {
        let dir = TempDir::new().unwrap();
        let tracker = PrIterationTracker::new(dir.path());
        tracker.mark_running(163).unwrap();
        assert!(tracker.is_running(163));
    }

    #[test]
    fn pr_tracker_mark_done_clears_marker() {
        let dir = TempDir::new().unwrap();
        let tracker = PrIterationTracker::new(dir.path());
        tracker.mark_running(163).unwrap();
        tracker.mark_done(163).unwrap();
        assert!(!tracker.is_running(163));
    }

    #[test]
    fn pr_tracker_mark_done_noop_when_not_running() {
        let dir = TempDir::new().unwrap();
        let tracker = PrIterationTracker::new(dir.path());
        // Should not error when no marker exists
        tracker.mark_done(999).unwrap();
    }

    #[test]
    fn pr_tracker_independent_prs() {
        let dir = TempDir::new().unwrap();
        let tracker = PrIterationTracker::new(dir.path());
        tracker.mark_running(10).unwrap();
        assert!(tracker.is_running(10));
        assert!(!tracker.is_running(20));
    }

    // ── ConflictFixTracker ───────────────────────────────────────────────────

    #[test]
    fn conflict_fix_tracker_new_pr_is_not_running() {
        let dir = TempDir::new().unwrap();
        let tracker = ConflictFixTracker::new(dir.path());
        assert!(!tracker.is_running(163));
    }

    #[test]
    fn conflict_fix_tracker_mark_running_sets_marker() {
        let dir = TempDir::new().unwrap();
        let tracker = ConflictFixTracker::new(dir.path());
        tracker.mark_running(163).unwrap();
        assert!(tracker.is_running(163));
    }

    #[test]
    fn conflict_fix_tracker_mark_done_clears_marker() {
        let dir = TempDir::new().unwrap();
        let tracker = ConflictFixTracker::new(dir.path());
        tracker.mark_running(163).unwrap();
        tracker.mark_done(163).unwrap();
        assert!(!tracker.is_running(163));
    }

    #[test]
    fn conflict_fix_tracker_mark_done_noop_when_not_running() {
        let dir = TempDir::new().unwrap();
        let tracker = ConflictFixTracker::new(dir.path());
        tracker.mark_done(999).unwrap();
    }

    #[test]
    fn conflict_fix_tracker_independent_from_pr_tracker() {
        let dir = TempDir::new().unwrap();
        let pr_tracker = PrIterationTracker::new(dir.path());
        let cf_tracker = ConflictFixTracker::new(dir.path());

        pr_tracker.mark_running(42).unwrap();
        // conflict fix tracker should not see the PR marker
        assert!(!cf_tracker.is_running(42));
    }
}
