use chrono::{DateTime, Utc};

use super::ports::{Comment, Review, ReviewState, TimelineEvent};
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

/// Pure function: does this PR need another worker pass?
///
/// Returns true if:
/// - A CHANGES_REQUESTED review was submitted after `last_commit_date`, OR
/// - Any PR comment was created after `last_commit_date`.
///
/// Anchoring to `last_commit_date` prevents re-triggering on feedback that
/// was already addressed by a worker (which pushed new commits). This also
/// covers PR authors who cannot formally request changes on their own PR —
/// their feedback arrives as plain comments instead.
pub fn needs_iteration(
    reviews: &[Review],
    comments: &[Comment],
    last_commit_date: DateTime<Utc>,
) -> bool {
    let has_changes_requested = reviews
        .iter()
        .any(|r| r.state == ReviewState::ChangesRequested && r.submitted_at > last_commit_date);
    let has_new_comments = comments.iter().any(|c| c.created_at > last_commit_date);
    has_changes_requested || has_new_comments
}

/// Pure function: should this in-progress issue be reconciled as done?
///
/// Inspects GitHub timeline events for a cross-reference from a merged PR.
/// Returns the merged PR number if found, None otherwise.
///
/// This uses the timeline API rather than `gh pr list --search` to avoid
/// fuzzy matching false positives (e.g. searching for #66 returning PRs
/// that mention #6).
pub fn should_reconcile(timeline_events: &[TimelineEvent]) -> Option<u64> {
    for event in timeline_events {
        if let TimelineEvent::CrossReferenced {
            pr_num,
            merged: true,
        } = event
        {
            return Some(*pr_num);
        }
    }
    None
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

    // ── needs_iteration ──────────────────────────────────────────────────────

    fn date(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn review(state: ReviewState, submitted_at: &str) -> Review {
        Review {
            state,
            submitted_at: date(submitted_at),
        }
    }

    fn comment(created_at: &str) -> Comment {
        Comment {
            created_at: date(created_at),
        }
    }

    #[test]
    fn changes_requested_after_commit_needs_iteration() {
        let reviews = vec![review(
            ReviewState::ChangesRequested,
            "2024-01-15T11:00:00Z",
        )];
        assert!(needs_iteration(&reviews, &[], date("2024-01-15T10:00:00Z")));
    }

    #[test]
    fn changes_requested_before_commit_no_iteration() {
        let reviews = vec![review(
            ReviewState::ChangesRequested,
            "2024-01-15T09:00:00Z",
        )];
        assert!(!needs_iteration(
            &reviews,
            &[],
            date("2024-01-15T10:00:00Z")
        ));
    }

    #[test]
    fn approved_review_after_commit_no_iteration() {
        // APPROVED reviews don't trigger iteration
        let reviews = vec![review(ReviewState::Approved, "2024-01-15T11:00:00Z")];
        assert!(!needs_iteration(
            &reviews,
            &[],
            date("2024-01-15T10:00:00Z")
        ));
    }

    #[test]
    fn comment_after_commit_needs_iteration() {
        let comments = vec![comment("2024-01-15T11:00:00Z")];
        assert!(needs_iteration(
            &[],
            &comments,
            date("2024-01-15T10:00:00Z")
        ));
    }

    #[test]
    fn comment_before_commit_no_iteration() {
        let comments = vec![comment("2024-01-15T09:00:00Z")];
        assert!(!needs_iteration(
            &[],
            &comments,
            date("2024-01-15T10:00:00Z")
        ));
    }

    #[test]
    fn empty_reviews_and_comments_no_iteration() {
        assert!(!needs_iteration(&[], &[], date("2024-01-15T10:00:00Z")));
    }

    #[test]
    fn mixed_reviews_only_changes_requested_after_triggers() {
        let reviews = vec![
            review(ReviewState::Approved, "2024-01-15T11:00:00Z"),
            review(ReviewState::ChangesRequested, "2024-01-15T11:30:00Z"),
        ];
        assert!(needs_iteration(&reviews, &[], date("2024-01-15T10:00:00Z")));
    }

    #[test]
    fn changes_requested_at_same_time_as_commit_no_iteration() {
        // Strictly after — same timestamp does not trigger
        let reviews = vec![review(
            ReviewState::ChangesRequested,
            "2024-01-15T10:00:00Z",
        )];
        assert!(!needs_iteration(
            &reviews,
            &[],
            date("2024-01-15T10:00:00Z")
        ));
    }

    // ── should_reconcile ─────────────────────────────────────────────────────

    #[test]
    fn merged_cross_reference_returns_pr_num() {
        let events = vec![TimelineEvent::CrossReferenced {
            pr_num: 42,
            merged: true,
        }];
        assert_eq!(should_reconcile(&events), Some(42));
    }

    #[test]
    fn unmerged_cross_reference_returns_none() {
        let events = vec![TimelineEvent::CrossReferenced {
            pr_num: 42,
            merged: false,
        }];
        assert_eq!(should_reconcile(&events), None);
    }

    #[test]
    fn other_event_returns_none() {
        let events = vec![TimelineEvent::Other];
        assert_eq!(should_reconcile(&events), None);
    }

    #[test]
    fn empty_timeline_returns_none() {
        assert_eq!(should_reconcile(&[]), None);
    }

    #[test]
    fn first_merged_pr_returned_when_multiple() {
        let events = vec![
            TimelineEvent::Other,
            TimelineEvent::CrossReferenced {
                pr_num: 10,
                merged: false,
            },
            TimelineEvent::CrossReferenced {
                pr_num: 11,
                merged: true,
            },
            TimelineEvent::CrossReferenced {
                pr_num: 12,
                merged: true,
            },
        ];
        assert_eq!(should_reconcile(&events), Some(11));
    }
}
