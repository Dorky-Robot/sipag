use super::status::WorkerStatus;

/// What the system should do with a discovered approved issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueAction {
    /// Do not dispatch a worker for this issue.
    Skip(SkipReason),
    /// Dispatch a new worker for this issue.
    Dispatch,
}

/// Why an issue was skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    AlreadyCompleted,
    InFlight,
    ExistingPr,
}

/// Result of checking whether an exited container should be finalized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalizationResult {
    /// Container is still running — nothing to do yet.
    StillRunning,
    /// Container exited and a PR was found — mark done.
    Done,
    /// Container exited with no PR — mark failed.
    Failed,
}

/// Pure function: given the current worker state for an issue (if any)
/// and whether a PR already exists, decide what action to take.
///
/// Encodes the decision tree:
///   1. done → skip (already completed)
///   2. enqueued/running/recovering → skip (in flight)
///   3. failed → dispatch (re-try)
///   4. no state + has PR → skip (record as done separately)
///   5. no state + no PR → dispatch (new work)
pub fn decide_issue_action(
    worker_status: Option<WorkerStatus>,
    has_existing_pr: bool,
) -> IssueAction {
    match worker_status {
        Some(WorkerStatus::Done) => IssueAction::Skip(SkipReason::AlreadyCompleted),
        Some(WorkerStatus::Enqueued | WorkerStatus::Running | WorkerStatus::Recovering) => {
            IssueAction::Skip(SkipReason::InFlight)
        }
        Some(WorkerStatus::Failed) => IssueAction::Dispatch,
        None if has_existing_pr => IssueAction::Skip(SkipReason::ExistingPr),
        None => IssueAction::Dispatch,
    }
}

/// Pure function: given whether a container is alive and whether a PR exists
/// for its branch, decide the finalization outcome.
///
/// Used by both startup recovery and per-cycle finalization.
pub fn decide_finalization(container_alive: bool, pr_exists: bool) -> FinalizationResult {
    if container_alive {
        FinalizationResult::StillRunning
    } else if pr_exists {
        FinalizationResult::Done
    } else {
        FinalizationResult::Failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── decide_issue_action ──────────────────────────────────────────────────

    #[test]
    fn completed_issue_is_skipped() {
        assert_eq!(
            decide_issue_action(Some(WorkerStatus::Done), false),
            IssueAction::Skip(SkipReason::AlreadyCompleted)
        );
    }

    #[test]
    fn completed_issue_skipped_regardless_of_pr() {
        assert_eq!(
            decide_issue_action(Some(WorkerStatus::Done), true),
            IssueAction::Skip(SkipReason::AlreadyCompleted)
        );
    }

    #[test]
    fn enqueued_issue_is_skipped() {
        assert_eq!(
            decide_issue_action(Some(WorkerStatus::Enqueued), false),
            IssueAction::Skip(SkipReason::InFlight)
        );
    }

    #[test]
    fn running_issue_is_skipped() {
        assert_eq!(
            decide_issue_action(Some(WorkerStatus::Running), false),
            IssueAction::Skip(SkipReason::InFlight)
        );
    }

    #[test]
    fn recovering_issue_is_skipped() {
        assert_eq!(
            decide_issue_action(Some(WorkerStatus::Recovering), false),
            IssueAction::Skip(SkipReason::InFlight)
        );
    }

    #[test]
    fn failed_issue_is_dispatched() {
        assert_eq!(
            decide_issue_action(Some(WorkerStatus::Failed), false),
            IssueAction::Dispatch
        );
    }

    #[test]
    fn failed_issue_dispatched_regardless_of_pr() {
        // A failed worker should be retried even if it left a PR behind.
        assert_eq!(
            decide_issue_action(Some(WorkerStatus::Failed), true),
            IssueAction::Dispatch
        );
    }

    #[test]
    fn no_state_with_existing_pr_is_skipped() {
        assert_eq!(
            decide_issue_action(None, true),
            IssueAction::Skip(SkipReason::ExistingPr)
        );
    }

    #[test]
    fn no_state_no_pr_is_dispatched() {
        assert_eq!(decide_issue_action(None, false), IssueAction::Dispatch);
    }

    // ── decide_finalization ──────────────────────────────────────────────────

    #[test]
    fn alive_container_stays_running() {
        assert_eq!(
            decide_finalization(true, false),
            FinalizationResult::StillRunning
        );
    }

    #[test]
    fn alive_container_stays_running_even_with_pr() {
        assert_eq!(
            decide_finalization(true, true),
            FinalizationResult::StillRunning
        );
    }

    #[test]
    fn gone_container_with_pr_is_done() {
        assert_eq!(decide_finalization(false, true), FinalizationResult::Done);
    }

    #[test]
    fn gone_container_without_pr_is_failed() {
        assert_eq!(
            decide_finalization(false, false),
            FinalizationResult::Failed
        );
    }

    // ── Exhaustiveness: all (status, pr) combinations ────────────────────────

    #[test]
    fn all_issue_action_combinations() {
        let statuses = [
            Some(WorkerStatus::Enqueued),
            Some(WorkerStatus::Running),
            Some(WorkerStatus::Recovering),
            Some(WorkerStatus::Done),
            Some(WorkerStatus::Failed),
            None,
        ];
        let pr_flags = [true, false];

        for status in &statuses {
            for &has_pr in &pr_flags {
                // Should not panic — all combinations are handled
                let _ = decide_issue_action(*status, has_pr);
            }
        }
    }

    #[test]
    fn all_finalization_combinations() {
        for &alive in &[true, false] {
            for &has_pr in &[true, false] {
                let _ = decide_finalization(alive, has_pr);
            }
        }
    }
}
