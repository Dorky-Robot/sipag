use std::collections::HashMap;

use super::decision::{decide_issue_action, IssueAction, SkipReason};
use super::status::WorkerStatus;

/// A snapshot of a single approved issue for planning purposes.
///
/// Created by reading GitHub issue state and local worker state files,
/// then passed into `plan_cycle()` as a pure-function input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueSnapshot {
    /// GitHub issue number.
    pub issue_num: u64,
    /// Whether an open or merged PR already exists for this issue.
    pub has_existing_pr: bool,
}

/// What the worker loop should do in a single polling cycle.
///
/// Returned by [`plan_cycle`] — a pure function with no side effects.
///
/// The WorkerLoop executes the plan in priority order:
///   1. Conflict fixes (dispatch_conflict_fixes)
///   2. PR iterations (dispatch_iterations)
///   3. New issues (dispatch_issues)
///
/// Each list is dispatched in batches of `batch_size` parallel workers.
#[derive(Debug, Clone, Default)]
pub struct CyclePlan {
    /// New issues to dispatch workers for.
    pub dispatch_issues: Vec<u64>,
    /// PR iterations to dispatch (takes priority over new issues).
    pub dispatch_iterations: Vec<u64>,
    /// Conflicted PRs to dispatch conflict-fix workers for.
    pub dispatch_conflict_fixes: Vec<u64>,
    /// Issues that have existing PRs but no state file — should be recorded as done.
    pub record_as_done: Vec<u64>,
}

impl CyclePlan {
    /// Returns true if there is any active work to dispatch this cycle.
    pub fn has_dispatch(&self) -> bool {
        !self.dispatch_issues.is_empty()
            || !self.dispatch_iterations.is_empty()
            || !self.dispatch_conflict_fixes.is_empty()
    }
}

/// Pure function: given the current state snapshot, determine what to do this cycle.
///
/// # Inputs
/// - `issue_snapshots`: approved issues with their PR status
/// - `worker_statuses`: map from issue number → current WorkerStatus (from state files)
/// - `prs_needing_iteration`: PR numbers with review feedback since last push
/// - `conflicted_prs`: PR numbers with merge conflicts
/// - `running_iteration_pr_nums`: PRs currently being iterated (in-memory tracking)
/// - `running_conflict_fix_pr_nums`: PRs currently being conflict-fixed (in-memory tracking)
/// - `batch_size`: max parallel workers per category (informational; WorkerLoop batches execution)
/// - `draining`: if true, no new work is dispatched
///
/// # Returns
/// A [`CyclePlan`] describing exactly what to dispatch and record.
///
/// No I/O, no side effects. All filtering and decision logic is here.
#[allow(clippy::too_many_arguments)]
pub fn plan_cycle(
    issue_snapshots: &[IssueSnapshot],
    worker_statuses: &HashMap<u64, WorkerStatus>,
    prs_needing_iteration: &[u64],
    conflicted_prs: &[u64],
    running_iteration_pr_nums: &[u64],
    running_conflict_fix_pr_nums: &[u64],
    _batch_size: usize,
    draining: bool,
) -> CyclePlan {
    if draining {
        return CyclePlan::default();
    }

    let mut dispatch_issues = Vec::new();
    let mut record_as_done = Vec::new();

    for snapshot in issue_snapshots {
        let status = worker_statuses.get(&snapshot.issue_num).copied();
        match decide_issue_action(status, snapshot.has_existing_pr) {
            IssueAction::Dispatch => {
                dispatch_issues.push(snapshot.issue_num);
            }
            IssueAction::Skip(SkipReason::ExistingPr) => {
                // No state file but a PR already exists — record as done to skip next cycle.
                record_as_done.push(snapshot.issue_num);
            }
            IssueAction::Skip(_) => {
                // Already done or in-flight — skip silently.
            }
        }
    }

    // PR iterations: skip those that already have a running worker.
    let dispatch_iterations: Vec<u64> = prs_needing_iteration
        .iter()
        .copied()
        .filter(|pr| {
            !running_iteration_pr_nums.contains(pr) && !running_conflict_fix_pr_nums.contains(pr)
        })
        .collect();

    // Conflict fixes: skip those with running workers.
    let dispatch_conflict_fixes: Vec<u64> = conflicted_prs
        .iter()
        .copied()
        .filter(|pr| {
            !running_conflict_fix_pr_nums.contains(pr) && !running_iteration_pr_nums.contains(pr)
        })
        .collect();

    CyclePlan {
        dispatch_issues,
        dispatch_iterations,
        dispatch_conflict_fixes,
        record_as_done,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn snapshot(issue_num: u64, has_pr: bool) -> IssueSnapshot {
        IssueSnapshot {
            issue_num,
            has_existing_pr: has_pr,
        }
    }

    fn statuses(pairs: &[(u64, WorkerStatus)]) -> HashMap<u64, WorkerStatus> {
        pairs.iter().cloned().collect()
    }

    fn no_running() -> &'static [u64] {
        &[]
    }

    // ── Drain ─────────────────────────────────────────────────────────────────

    #[test]
    fn drain_returns_empty_plan() {
        let plan = plan_cycle(
            &[snapshot(1, false)],
            &statuses(&[]),
            &[],
            &[],
            no_running(),
            no_running(),
            1,
            true, // draining
        );
        assert!(!plan.has_dispatch());
        assert!(plan.dispatch_issues.is_empty());
        assert!(plan.dispatch_iterations.is_empty());
        assert!(plan.dispatch_conflict_fixes.is_empty());
        assert!(plan.record_as_done.is_empty());
    }

    #[test]
    fn drain_suppresses_all_categories() {
        let plan = plan_cycle(
            &[snapshot(1, false)],
            &statuses(&[]),
            &[10, 11], // PRs needing iteration
            &[20],     // conflicted PRs
            no_running(),
            no_running(),
            1,
            true, // draining
        );
        assert!(!plan.has_dispatch());
    }

    // ── Issue filtering ───────────────────────────────────────────────────────

    #[test]
    fn no_approved_issues_returns_empty_dispatch() {
        let plan = plan_cycle(
            &[],
            &statuses(&[]),
            &[],
            &[],
            no_running(),
            no_running(),
            1,
            false,
        );
        assert!(plan.dispatch_issues.is_empty());
    }

    #[test]
    fn issue_with_no_state_no_pr_is_dispatched() {
        let plan = plan_cycle(
            &[snapshot(42, false)],
            &statuses(&[]),
            &[],
            &[],
            no_running(),
            no_running(),
            1,
            false,
        );
        assert_eq!(plan.dispatch_issues, vec![42]);
        assert!(plan.record_as_done.is_empty());
    }

    #[test]
    fn issue_with_no_state_existing_pr_is_recorded_done() {
        let plan = plan_cycle(
            &[snapshot(42, true)],
            &statuses(&[]),
            &[],
            &[],
            no_running(),
            no_running(),
            1,
            false,
        );
        assert!(plan.dispatch_issues.is_empty());
        assert_eq!(plan.record_as_done, vec![42]);
    }

    #[test]
    fn done_issue_is_skipped() {
        let plan = plan_cycle(
            &[snapshot(42, false)],
            &statuses(&[(42, WorkerStatus::Done)]),
            &[],
            &[],
            no_running(),
            no_running(),
            1,
            false,
        );
        assert!(plan.dispatch_issues.is_empty());
        assert!(plan.record_as_done.is_empty());
    }

    #[test]
    fn running_issue_is_skipped() {
        let plan = plan_cycle(
            &[snapshot(42, false)],
            &statuses(&[(42, WorkerStatus::Running)]),
            &[],
            &[],
            no_running(),
            no_running(),
            1,
            false,
        );
        assert!(plan.dispatch_issues.is_empty());
    }

    #[test]
    fn enqueued_issue_is_skipped() {
        let plan = plan_cycle(
            &[snapshot(42, false)],
            &statuses(&[(42, WorkerStatus::Enqueued)]),
            &[],
            &[],
            no_running(),
            no_running(),
            1,
            false,
        );
        assert!(plan.dispatch_issues.is_empty());
    }

    #[test]
    fn recovering_issue_is_skipped() {
        let plan = plan_cycle(
            &[snapshot(42, false)],
            &statuses(&[(42, WorkerStatus::Recovering)]),
            &[],
            &[],
            no_running(),
            no_running(),
            1,
            false,
        );
        assert!(plan.dispatch_issues.is_empty());
    }

    #[test]
    fn failed_issue_is_re_dispatched() {
        let plan = plan_cycle(
            &[snapshot(42, false)],
            &statuses(&[(42, WorkerStatus::Failed)]),
            &[],
            &[],
            no_running(),
            no_running(),
            1,
            false,
        );
        assert_eq!(plan.dispatch_issues, vec![42]);
    }

    #[test]
    fn mixed_issue_states_correctly_filtered() {
        let plan = plan_cycle(
            &[
                snapshot(1, false), // no state → dispatch
                snapshot(2, false), // done → skip
                snapshot(3, false), // running → skip
                snapshot(4, false), // failed → dispatch
                snapshot(5, true),  // no state + PR → record done
            ],
            &statuses(&[
                (2, WorkerStatus::Done),
                (3, WorkerStatus::Running),
                (4, WorkerStatus::Failed),
            ]),
            &[],
            &[],
            no_running(),
            no_running(),
            10,
            false,
        );
        let mut issues = plan.dispatch_issues.clone();
        issues.sort();
        assert_eq!(issues, vec![1, 4]);
        assert_eq!(plan.record_as_done, vec![5]);
    }

    // ── PR iterations ─────────────────────────────────────────────────────────

    #[test]
    fn pr_iterations_dispatched_when_not_running() {
        let plan = plan_cycle(
            &[],
            &statuses(&[]),
            &[10, 11],
            &[],
            no_running(),
            no_running(),
            1,
            false,
        );
        assert_eq!(plan.dispatch_iterations, vec![10, 11]);
    }

    #[test]
    fn running_pr_iteration_is_skipped() {
        let plan = plan_cycle(
            &[],
            &statuses(&[]),
            &[10, 11],
            &[],
            &[10], // PR 10 already running
            no_running(),
            1,
            false,
        );
        assert_eq!(plan.dispatch_iterations, vec![11]);
    }

    #[test]
    fn conflict_fix_running_blocks_iteration_for_same_pr() {
        let plan = plan_cycle(
            &[],
            &statuses(&[]),
            &[10],
            &[],
            no_running(),
            &[10], // PR 10 has conflict fix running
            1,
            false,
        );
        assert!(plan.dispatch_iterations.is_empty());
    }

    #[test]
    fn all_iterations_running_returns_empty() {
        let plan = plan_cycle(
            &[],
            &statuses(&[]),
            &[10, 11],
            &[],
            &[10, 11],
            no_running(),
            1,
            false,
        );
        assert!(plan.dispatch_iterations.is_empty());
    }

    // ── Conflict fixes ────────────────────────────────────────────────────────

    #[test]
    fn conflicted_prs_dispatched_when_not_running() {
        let plan = plan_cycle(
            &[],
            &statuses(&[]),
            &[],
            &[20, 21],
            no_running(),
            no_running(),
            1,
            false,
        );
        assert_eq!(plan.dispatch_conflict_fixes, vec![20, 21]);
    }

    #[test]
    fn running_conflict_fix_is_skipped() {
        let plan = plan_cycle(
            &[],
            &statuses(&[]),
            &[],
            &[20, 21],
            no_running(),
            &[20], // PR 20 has conflict fix running
            1,
            false,
        );
        assert_eq!(plan.dispatch_conflict_fixes, vec![21]);
    }

    #[test]
    fn iteration_running_blocks_conflict_fix_for_same_pr() {
        let plan = plan_cycle(
            &[],
            &statuses(&[]),
            &[],
            &[20],
            &[20], // PR 20 has iteration running
            no_running(),
            1,
            false,
        );
        assert!(plan.dispatch_conflict_fixes.is_empty());
    }

    // ── Priority: iterations before issues ────────────────────────────────────

    #[test]
    fn plan_includes_both_iterations_and_new_issues() {
        let plan = plan_cycle(
            &[snapshot(1, false)],
            &statuses(&[]),
            &[10],
            &[],
            no_running(),
            no_running(),
            5,
            false,
        );
        assert_eq!(plan.dispatch_issues, vec![1]);
        assert_eq!(plan.dispatch_iterations, vec![10]);
    }

    // ── has_dispatch ──────────────────────────────────────────────────────────

    #[test]
    fn has_dispatch_false_when_nothing_to_do() {
        let plan = plan_cycle(
            &[snapshot(1, false)],
            &statuses(&[(1, WorkerStatus::Done)]),
            &[],
            &[],
            no_running(),
            no_running(),
            1,
            false,
        );
        assert!(!plan.has_dispatch());
    }

    #[test]
    fn has_dispatch_true_when_new_issue() {
        let plan = plan_cycle(
            &[snapshot(1, false)],
            &statuses(&[]),
            &[],
            &[],
            no_running(),
            no_running(),
            1,
            false,
        );
        assert!(plan.has_dispatch());
    }

    #[test]
    fn has_dispatch_true_when_iteration() {
        let plan = plan_cycle(
            &[],
            &statuses(&[]),
            &[10],
            &[],
            no_running(),
            no_running(),
            1,
            false,
        );
        assert!(plan.has_dispatch());
    }

    #[test]
    fn has_dispatch_true_when_conflict_fix() {
        let plan = plan_cycle(
            &[],
            &statuses(&[]),
            &[],
            &[20],
            no_running(),
            no_running(),
            1,
            false,
        );
        assert!(plan.has_dispatch());
    }

    // ── Batch size informational ───────────────────────────────────────────────

    #[test]
    fn multiple_issues_all_returned_regardless_of_batch_size() {
        // plan_cycle returns ALL applicable items; WorkerLoop handles batching.
        let plan = plan_cycle(
            &[snapshot(1, false), snapshot(2, false), snapshot(3, false)],
            &statuses(&[]),
            &[],
            &[],
            no_running(),
            no_running(),
            1, // batch_size = 1, but all 3 should be returned
            false,
        );
        assert_eq!(plan.dispatch_issues.len(), 3);
    }

    // ── Exhaustiveness ────────────────────────────────────────────────────────

    #[test]
    fn all_worker_status_values_handled() {
        for status in [
            WorkerStatus::Enqueued,
            WorkerStatus::Running,
            WorkerStatus::Recovering,
            WorkerStatus::Done,
            WorkerStatus::Failed,
        ] {
            // Should not panic for any status
            let _ = plan_cycle(
                &[snapshot(42, false)],
                &statuses(&[(42, status)]),
                &[],
                &[],
                no_running(),
                no_running(),
                1,
                false,
            );
        }
    }

    #[test]
    fn draining_with_all_categories_returns_empty() {
        let plan = plan_cycle(
            &[snapshot(1, false), snapshot(2, false)],
            &statuses(&[(2, WorkerStatus::Failed)]),
            &[10, 11],
            &[20, 21],
            no_running(),
            no_running(),
            5,
            true, // draining
        );
        assert!(plan.dispatch_issues.is_empty());
        assert!(plan.dispatch_iterations.is_empty());
        assert!(plan.dispatch_conflict_fixes.is_empty());
        assert!(plan.record_as_done.is_empty());
    }
}
