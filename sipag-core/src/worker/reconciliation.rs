//! Reconciliation services: close in-progress issues and recover orphaned branches.
//!
//! These orchestration functions coordinate multiple GitHub API calls using the
//! GitHubGateway port. All decision logic lives in decision.rs as pure functions.

use anyhow::Result;

use super::decision::should_reconcile;
use super::ports::{GitHubGateway, IssueInfo, StateStore};
use super::status::WorkerStatus;

// ── Outcomes ──────────────────────────────────────────────────────────────────

/// Outcome of reconciling a single in-progress issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileOutcome {
    /// Issue closed because its PR was merged.
    Closed { issue_num: u64, pr_num: u64 },
    /// No merged PR found for this issue — left as-is.
    NoMergedPr { issue_num: u64 },
}

/// Outcome of processing a single branch during orphaned branch recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrphanedBranchOutcome {
    /// Recovery PR created for this branch.
    RecoveryPrCreated { branch: String, issue_num: u64 },
    /// A merged PR already existed; stale branch deleted.
    MergedBranchDeleted { branch: String, pr_num: u64 },
    /// Branch has no commits ahead of main — nothing to do.
    NothingAhead { branch: String },
    /// Branch already has an open PR — no recovery needed.
    AlreadyHasPr { branch: String },
}

// ── Reconcile ─────────────────────────────────────────────────────────────────

/// Close in-progress issues whose worker-created PR has been merged.
///
/// For each issue labeled "in-progress", queries the GitHub timeline API to
/// find a cross-reference from a merged PR (using `should_reconcile`). When
/// found:
/// - Closes the issue with a comment.
/// - Updates worker state to Done in the state store.
/// - Deletes the merged branch to prevent stale branch accumulation.
///
/// Also triggers `recover_orphaned_branches` at the start of each cycle.
///
/// Mirrors `worker_reconcile` from github.sh.
pub fn reconcile(
    gateway: &dyn GitHubGateway,
    store: &dyn StateStore,
    repo: &str,
) -> Result<Vec<ReconcileOutcome>> {
    // Orphaned branch recovery runs every cycle regardless of in-progress count
    recover_orphaned_branches(gateway, repo)?;

    let in_progress = gateway.list_issues_with_label(repo, "in-progress")?;
    let mut outcomes = Vec::new();

    for issue_num in in_progress {
        let timeline = gateway.get_issue_timeline(repo, issue_num)?;

        match should_reconcile(&timeline) {
            None => {
                outcomes.push(ReconcileOutcome::NoMergedPr { issue_num });
            }
            Some(merged_pr_num) => {
                // Fetch PR metadata for the comment, branch name, and URL
                let pr_info = gateway.get_pr_info(repo, merged_pr_num)?;
                let pr_url = pr_info.as_ref().map(|p| p.url.clone()).unwrap_or_default();
                let branch_name = pr_info
                    .as_ref()
                    .map(|p| p.branch.clone())
                    .unwrap_or_default();

                let comment = format!("Closed by merged PR #{}", merged_pr_num);
                gateway.close_issue(repo, issue_num, &comment)?;

                // Update worker state to Done
                let repo_slug = repo.replace('/', "--");
                if let Ok(Some(mut state)) = store.load(&repo_slug, issue_num) {
                    state.status = WorkerStatus::Done;
                    state.pr_num = Some(merged_pr_num);
                    if !pr_url.is_empty() {
                        state.pr_url = Some(pr_url);
                    }
                    let _ = store.save(&state);
                }

                // Delete the merged branch to prevent stale branch accumulation
                if !branch_name.is_empty() {
                    gateway.delete_branch(repo, &branch_name)?;
                }

                outcomes.push(ReconcileOutcome::Closed {
                    issue_num,
                    pr_num: merged_pr_num,
                });
            }
        }
    }

    Ok(outcomes)
}

// ── Orphaned branch recovery ──────────────────────────────────────────────────

/// Recover sipag/issue-* branches that have commits but no open PR.
///
/// For each matching branch:
/// - If an open PR already exists: skip.
/// - If a merged PR exists: delete the stale branch.
/// - If ahead of main but no PR: create a recovery draft PR.
/// - If not ahead of main: skip.
///
/// Mirrors `worker_reconcile_orphaned_branches` from github.sh.
pub fn recover_orphaned_branches(
    gateway: &dyn GitHubGateway,
    repo: &str,
) -> Result<Vec<OrphanedBranchOutcome>> {
    let branches = gateway.list_branches_with_prefix(repo, "sipag/issue-")?;
    let mut outcomes = Vec::new();

    for branch in branches {
        // Skip if an open PR already exists for this branch
        let open_prs = gateway.get_open_prs_for_branch(repo, &branch)?;
        if !open_prs.is_empty() {
            outcomes.push(OrphanedBranchOutcome::AlreadyHasPr { branch });
            continue;
        }

        // If a merged PR exists, delete the now-stale branch
        let merged_prs = gateway.get_merged_prs_for_branch(repo, &branch)?;
        if let Some(merged_pr) = merged_prs.first() {
            let pr_num = merged_pr.number;
            eprintln!(
                "[worker] Branch {} already merged via PR #{} — deleting stale branch",
                branch, pr_num
            );
            gateway.delete_branch(repo, &branch)?;
            outcomes.push(OrphanedBranchOutcome::MergedBranchDeleted { branch, pr_num });
            continue;
        }

        // Skip if branch has no commits ahead of main
        let ahead_by = gateway.branch_ahead_by(repo, "main", &branch)?;
        if ahead_by == 0 {
            outcomes.push(OrphanedBranchOutcome::NothingAhead { branch });
            continue;
        }

        // Extract issue number from "sipag/issue-NNN-slug"
        let issue_num = match extract_issue_num(&branch) {
            Some(n) => n,
            None => continue,
        };

        // Fetch issue details to build the recovery PR title and body
        let (pr_title, issue_body) = match gateway.get_issue_info(repo, issue_num)? {
            Some(IssueInfo { title, body, .. }) => (title, body),
            None => (branch.clone(), String::new()),
        };

        let recovery_body = format!(
            "Closes #{}\n\n{}\n\n---\n*This PR was created by sipag worker reconciliation (recovered orphaned branch).*",
            issue_num, issue_body
        );

        eprintln!(
            "[worker] Orphaned branch detected: {} (issue #{}: {})",
            branch, issue_num, pr_title
        );

        match gateway.create_pr(repo, &branch, &pr_title, &recovery_body) {
            Ok(_) => {
                eprintln!("[worker] Recovery PR created for branch {}", branch);
                outcomes.push(OrphanedBranchOutcome::RecoveryPrCreated { branch, issue_num });
            }
            Err(e) => {
                eprintln!(
                    "[worker] WARNING: Could not create recovery PR for {}: {}",
                    branch, e
                );
            }
        }
    }

    Ok(outcomes)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract the issue number from a branch name like "sipag/issue-123-some-slug".
pub fn extract_issue_num(branch: &str) -> Option<u64> {
    let after_prefix = branch.strip_prefix("sipag/issue-")?;
    let num_str: String = after_prefix
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if num_str.is_empty() {
        return None;
    }
    num_str.parse().ok()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::ports::{IssueInfo, IssueState, PrInfo, PrState, TimelineEvent};
    use super::super::state::WorkerState;
    use super::super::status::WorkerStatus;
    use super::*;
    use anyhow::Result;
    use std::cell::RefCell;
    use std::collections::HashMap;

    // ── MockGitHub ────────────────────────────────────────────────────────────

    #[derive(Default)]
    struct MockGitHub {
        // Configured responses
        in_progress_issues: Vec<u64>,
        timelines: HashMap<u64, Vec<TimelineEvent>>,
        pr_infos: HashMap<u64, PrInfo>,
        issue_infos: HashMap<u64, IssueInfo>,
        branches: Vec<String>,
        open_prs_by_branch: HashMap<String, Vec<PrInfo>>,
        merged_prs_by_branch: HashMap<String, Vec<PrInfo>>,
        ahead_by: HashMap<String, u64>,

        // Recorded calls
        closed_issues: RefCell<Vec<(u64, String)>>,
        deleted_branches: RefCell<Vec<String>>,
        created_prs: RefCell<Vec<(String, String, String)>>, // (branch, title, body)
    }

    impl MockGitHub {
        fn new() -> Self {
            Self::default()
        }

        fn with_in_progress(mut self, issues: &[u64]) -> Self {
            self.in_progress_issues = issues.to_vec();
            self
        }

        fn with_timeline(mut self, issue_num: u64, events: Vec<TimelineEvent>) -> Self {
            self.timelines.insert(issue_num, events);
            self
        }

        fn with_pr_info(mut self, pr_num: u64, info: PrInfo) -> Self {
            self.pr_infos.insert(pr_num, info);
            self
        }

        fn with_issue_info(mut self, issue_num: u64, info: IssueInfo) -> Self {
            self.issue_infos.insert(issue_num, info);
            self
        }

        fn with_branches(mut self, branches: &[&str]) -> Self {
            self.branches = branches.iter().map(|s| s.to_string()).collect();
            self
        }

        fn with_open_prs_for_branch(mut self, branch: &str, prs: Vec<PrInfo>) -> Self {
            self.open_prs_by_branch.insert(branch.to_string(), prs);
            self
        }

        fn with_merged_prs_for_branch(mut self, branch: &str, prs: Vec<PrInfo>) -> Self {
            self.merged_prs_by_branch.insert(branch.to_string(), prs);
            self
        }

        fn with_ahead_by(mut self, branch: &str, ahead: u64) -> Self {
            self.ahead_by.insert(branch.to_string(), ahead);
            self
        }

        fn get_closed_issues(&self) -> Vec<(u64, String)> {
            self.closed_issues.borrow().clone()
        }

        fn get_deleted_branches(&self) -> Vec<String> {
            self.deleted_branches.borrow().clone()
        }

        fn get_created_prs(&self) -> Vec<(String, String, String)> {
            self.created_prs.borrow().clone()
        }
    }

    impl super::super::ports::GitHubGateway for MockGitHub {
        fn find_pr_for_branch(&self, _repo: &str, _branch: &str) -> Result<Option<PrInfo>> {
            Ok(None)
        }

        fn find_pr_for_issue(&self, _repo: &str, _issue_num: u64) -> Result<Option<PrInfo>> {
            Ok(None)
        }

        fn find_open_pr_for_issue(&self, _repo: &str, _issue_num: u64) -> Result<Option<PrInfo>> {
            Ok(None)
        }

        fn find_prs_needing_iteration(&self, _repo: &str) -> Result<Vec<u64>> {
            Ok(vec![])
        }

        fn find_conflicted_prs(&self, _repo: &str) -> Result<Vec<PrInfo>> {
            Ok(vec![])
        }

        fn issue_is_open(&self, _repo: &str, _issue_num: u64) -> Result<bool> {
            Ok(true)
        }

        fn get_issue_info(&self, _repo: &str, issue_num: u64) -> Result<Option<IssueInfo>> {
            Ok(self.issue_infos.get(&issue_num).cloned())
        }

        fn list_issues_with_label(&self, _repo: &str, _label: &str) -> Result<Vec<u64>> {
            Ok(self.in_progress_issues.clone())
        }

        fn get_issue_timeline(&self, _repo: &str, issue_num: u64) -> Result<Vec<TimelineEvent>> {
            Ok(self.timelines.get(&issue_num).cloned().unwrap_or_default())
        }

        fn close_issue(&self, _repo: &str, issue_num: u64, comment: &str) -> Result<()> {
            self.closed_issues
                .borrow_mut()
                .push((issue_num, comment.to_string()));
            Ok(())
        }

        fn transition_label(
            &self,
            _repo: &str,
            _issue_num: u64,
            _remove: Option<&str>,
            _add: Option<&str>,
        ) -> Result<()> {
            Ok(())
        }

        fn get_pr_info(&self, _repo: &str, pr_num: u64) -> Result<Option<PrInfo>> {
            Ok(self.pr_infos.get(&pr_num).cloned())
        }

        fn get_open_prs_for_branch(&self, _repo: &str, branch: &str) -> Result<Vec<PrInfo>> {
            Ok(self
                .open_prs_by_branch
                .get(branch)
                .cloned()
                .unwrap_or_default())
        }

        fn get_merged_prs_for_branch(&self, _repo: &str, branch: &str) -> Result<Vec<PrInfo>> {
            Ok(self
                .merged_prs_by_branch
                .get(branch)
                .cloned()
                .unwrap_or_default())
        }

        fn list_branches_with_prefix(&self, _repo: &str, _prefix: &str) -> Result<Vec<String>> {
            Ok(self.branches.clone())
        }

        fn branch_ahead_by(&self, _repo: &str, _base: &str, head: &str) -> Result<u64> {
            Ok(self.ahead_by.get(head).copied().unwrap_or(0))
        }

        fn create_pr(&self, _repo: &str, branch: &str, title: &str, body: &str) -> Result<()> {
            self.created_prs.borrow_mut().push((
                branch.to_string(),
                title.to_string(),
                body.to_string(),
            ));
            Ok(())
        }

        fn delete_branch(&self, _repo: &str, branch: &str) -> Result<()> {
            self.deleted_branches.borrow_mut().push(branch.to_string());
            Ok(())
        }
    }

    // ── MockStore ─────────────────────────────────────────────────────────────

    #[derive(Default)]
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
    }

    impl super::super::ports::StateStore for MockStore {
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

    fn make_running_worker(issue_num: u64) -> WorkerState {
        WorkerState {
            repo: "test/repo".to_string(),
            issue_num,
            issue_title: format!("Issue {}", issue_num),
            branch: format!("sipag/issue-{}-test", issue_num),
            container_name: format!("sipag-issue-{}", issue_num),
            pr_num: None,
            pr_url: None,
            status: WorkerStatus::Running,
            started_at: Some("2024-01-01T00:00:00Z".to_string()),
            ended_at: None,
            duration_s: None,
            exit_code: None,
            log_path: None,
        }
    }

    fn merged_cross_ref(pr_num: u64) -> TimelineEvent {
        TimelineEvent::CrossReferenced {
            pr_num,
            merged: true,
        }
    }

    fn open_pr(number: u64, branch: &str) -> PrInfo {
        PrInfo {
            number,
            url: format!("https://github.com/test/repo/pull/{}", number),
            state: PrState::Open,
            branch: branch.to_string(),
        }
    }

    fn merged_pr(number: u64, branch: &str) -> PrInfo {
        PrInfo {
            number,
            url: format!("https://github.com/test/repo/pull/{}", number),
            state: PrState::Merged,
            branch: branch.to_string(),
        }
    }

    // ── reconcile tests ───────────────────────────────────────────────────────

    #[test]
    fn reconcile_closes_issue_with_merged_pr() {
        let store = MockStore::new(vec![make_running_worker(42)]);
        let github = MockGitHub::new()
            .with_in_progress(&[42])
            .with_timeline(42, vec![merged_cross_ref(100)])
            .with_pr_info(
                100,
                PrInfo {
                    number: 100,
                    url: "https://github.com/test/repo/pull/100".to_string(),
                    state: PrState::Merged,
                    branch: "sipag/issue-42-test".to_string(),
                },
            );

        let outcomes = reconcile(&github, &store, "test/repo").unwrap();

        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            outcomes[0],
            ReconcileOutcome::Closed {
                issue_num: 42,
                pr_num: 100,
            }
        );
    }

    #[test]
    fn reconcile_closes_issue_with_comment() {
        let store = MockStore::new(vec![]);
        let github = MockGitHub::new()
            .with_in_progress(&[42])
            .with_timeline(42, vec![merged_cross_ref(100)]);

        reconcile(&github, &store, "test/repo").unwrap();

        let closed = github.get_closed_issues();
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].0, 42);
        assert!(closed[0].1.contains("100"));
    }

    #[test]
    fn reconcile_updates_worker_state_to_done() {
        let store = MockStore::new(vec![make_running_worker(42)]);
        let github = MockGitHub::new()
            .with_in_progress(&[42])
            .with_timeline(42, vec![merged_cross_ref(100)])
            .with_pr_info(100, open_pr(100, "sipag/issue-42-test"));

        reconcile(&github, &store, "test/repo").unwrap();

        assert_eq!(store.get_status(42), Some(WorkerStatus::Done));
        assert_eq!(store.get_pr_num(42), Some(100));
    }

    #[test]
    fn reconcile_deletes_merged_branch() {
        let store = MockStore::new(vec![]);
        let branch = "sipag/issue-42-test";
        let github = MockGitHub::new()
            .with_in_progress(&[42])
            .with_timeline(42, vec![merged_cross_ref(100)])
            .with_pr_info(100, merged_pr(100, branch));

        reconcile(&github, &store, "test/repo").unwrap();

        assert!(github.get_deleted_branches().contains(&branch.to_string()));
    }

    #[test]
    fn reconcile_no_merged_pr_leaves_issue() {
        let store = MockStore::new(vec![]);
        let github = MockGitHub::new()
            .with_in_progress(&[42])
            .with_timeline(42, vec![TimelineEvent::Other]);

        let outcomes = reconcile(&github, &store, "test/repo").unwrap();

        assert_eq!(outcomes[0], ReconcileOutcome::NoMergedPr { issue_num: 42 });
        assert!(github.get_closed_issues().is_empty());
    }

    #[test]
    fn reconcile_empty_in_progress_returns_empty() {
        let store = MockStore::new(vec![]);
        let github = MockGitHub::new().with_in_progress(&[]);

        let outcomes = reconcile(&github, &store, "test/repo").unwrap();

        assert!(outcomes.is_empty());
    }

    #[test]
    fn reconcile_multiple_issues() {
        let store = MockStore::new(vec![]);
        let github = MockGitHub::new()
            .with_in_progress(&[10, 20])
            .with_timeline(10, vec![merged_cross_ref(100)])
            .with_timeline(20, vec![TimelineEvent::Other]);

        let outcomes = reconcile(&github, &store, "test/repo").unwrap();

        assert_eq!(outcomes.len(), 2);
        assert_eq!(
            outcomes[0],
            ReconcileOutcome::Closed {
                issue_num: 10,
                pr_num: 100,
            }
        );
        assert_eq!(outcomes[1], ReconcileOutcome::NoMergedPr { issue_num: 20 });
    }

    // ── recover_orphaned_branches tests ───────────────────────────────────────

    #[test]
    fn branch_with_open_pr_skipped() {
        let branch = "sipag/issue-42-test";
        let github = MockGitHub::new()
            .with_branches(&[branch])
            .with_open_prs_for_branch(branch, vec![open_pr(99, branch)]);

        let outcomes = recover_orphaned_branches(&github, "test/repo").unwrap();

        assert_eq!(
            outcomes[0],
            OrphanedBranchOutcome::AlreadyHasPr {
                branch: branch.to_string(),
            }
        );
        assert!(github.get_created_prs().is_empty());
    }

    #[test]
    fn branch_with_merged_pr_gets_deleted() {
        let branch = "sipag/issue-42-test";
        let github = MockGitHub::new()
            .with_branches(&[branch])
            .with_merged_prs_for_branch(branch, vec![merged_pr(99, branch)]);

        let outcomes = recover_orphaned_branches(&github, "test/repo").unwrap();

        assert_eq!(
            outcomes[0],
            OrphanedBranchOutcome::MergedBranchDeleted {
                branch: branch.to_string(),
                pr_num: 99,
            }
        );
        assert!(github.get_deleted_branches().contains(&branch.to_string()));
    }

    #[test]
    fn branch_not_ahead_skipped() {
        let branch = "sipag/issue-42-test";
        let github = MockGitHub::new()
            .with_branches(&[branch])
            .with_ahead_by(branch, 0);

        let outcomes = recover_orphaned_branches(&github, "test/repo").unwrap();

        assert_eq!(
            outcomes[0],
            OrphanedBranchOutcome::NothingAhead {
                branch: branch.to_string(),
            }
        );
    }

    #[test]
    fn orphaned_branch_gets_recovery_pr() {
        let branch = "sipag/issue-42-some-feature";
        let github = MockGitHub::new()
            .with_branches(&[branch])
            .with_ahead_by(branch, 3)
            .with_issue_info(
                42,
                IssueInfo {
                    number: 42,
                    title: "Some Feature".to_string(),
                    body: "Feature description".to_string(),
                    state: IssueState::Open,
                },
            );

        let outcomes = recover_orphaned_branches(&github, "test/repo").unwrap();

        assert_eq!(
            outcomes[0],
            OrphanedBranchOutcome::RecoveryPrCreated {
                branch: branch.to_string(),
                issue_num: 42,
            }
        );
        let created = github.get_created_prs();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].0, branch);
        assert_eq!(created[0].1, "Some Feature");
        assert!(created[0].2.contains("Closes #42"));
    }

    #[test]
    fn recovery_pr_body_mentions_orphaned_note() {
        let branch = "sipag/issue-42-test";
        let github = MockGitHub::new()
            .with_branches(&[branch])
            .with_ahead_by(branch, 1);

        recover_orphaned_branches(&github, "test/repo").unwrap();

        let created = github.get_created_prs();
        assert!(created[0].2.contains("reconciliation"));
    }

    #[test]
    fn no_branches_returns_empty() {
        let github = MockGitHub::new().with_branches(&[]);

        let outcomes = recover_orphaned_branches(&github, "test/repo").unwrap();

        assert!(outcomes.is_empty());
    }

    // ── extract_issue_num tests ───────────────────────────────────────────────

    #[test]
    fn extract_issue_num_basic() {
        assert_eq!(extract_issue_num("sipag/issue-42-some-slug"), Some(42));
    }

    #[test]
    fn extract_issue_num_large() {
        assert_eq!(
            extract_issue_num("sipag/issue-307-rustify-gateway"),
            Some(307)
        );
    }

    #[test]
    fn extract_issue_num_no_slug() {
        // Edge case: branch with no trailing slug
        assert_eq!(extract_issue_num("sipag/issue-42"), Some(42));
    }

    #[test]
    fn extract_issue_num_wrong_prefix() {
        assert_eq!(extract_issue_num("feature/issue-42"), None);
    }

    #[test]
    fn extract_issue_num_no_digits() {
        assert_eq!(extract_issue_num("sipag/issue-abc"), None);
    }
}
