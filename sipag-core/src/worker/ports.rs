use anyhow::Result;
use chrono::{DateTime, Utc};

use super::state::WorkerState;

// ── Value Objects ─────────────────────────────────────────────────────────────

/// State of a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrState {
    Open,
    Merged,
    Closed,
}

/// Information about a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrInfo {
    pub number: u64,
    pub url: String,
    pub state: PrState,
    pub branch: String,
}

/// State of a GitHub issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueState {
    Open,
    Closed,
}

/// Information about a GitHub issue.
#[derive(Debug, Clone)]
pub struct IssueInfo {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: IssueState,
}

/// State of a pull request review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewState {
    ChangesRequested,
    Approved,
    Commented,
    Dismissed,
    Other,
}

/// A review submitted on a pull request.
#[derive(Debug, Clone)]
pub struct Review {
    pub state: ReviewState,
    pub submitted_at: DateTime<Utc>,
}

/// A comment on a pull request.
#[derive(Debug, Clone)]
pub struct Comment {
    pub created_at: DateTime<Utc>,
}

/// A GitHub timeline event for an issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineEvent {
    /// A cross-reference from a PR that references (and potentially closes) this issue.
    CrossReferenced {
        /// The PR number that references this issue.
        pr_num: u64,
        /// Whether the referencing PR has been merged.
        merged: bool,
    },
    /// Any other event type (ignored by reconciliation logic).
    Other,
}

// ── Ports (Traits) ─────────────────────────────────────────────────────────────

/// Port for querying Docker container status.
pub trait ContainerRuntime {
    /// Check if a container with the given name is currently running.
    fn is_running(&self, container_name: &str) -> Result<bool>;
}

/// Port for querying and mutating GitHub state.
///
/// All methods must not panic on closed/missing issues or missing labels.
/// Network failures should be propagated as errors.
pub trait GitHubGateway {
    // ── PR queries by branch ─────────────────────────────────────────────

    /// Find a PR (open or merged) for a given branch.
    ///
    /// Used by recovery to detect whether a worker's container produced a PR.
    fn find_pr_for_branch(&self, repo: &str, branch: &str) -> Result<Option<PrInfo>>;

    // ── PR queries by issue ──────────────────────────────────────────────

    /// Find a PR (open or merged, not closed-without-merge) for an issue.
    ///
    /// Uses "closes/fixes/resolves #N" in the PR body as the link. Closed-
    /// without-merge PRs are excluded so issues with abandoned PRs can be
    /// re-dispatched after re-approval.
    ///
    /// Mirrors `worker_has_pr` from github.sh.
    fn find_pr_for_issue(&self, repo: &str, issue_num: u64) -> Result<Option<PrInfo>>;

    /// Find an open (not yet merged) PR for an issue.
    ///
    /// Mirrors `worker_has_open_pr` from github.sh.
    fn find_open_pr_for_issue(&self, repo: &str, issue_num: u64) -> Result<Option<PrInfo>>;

    // ── PR iteration ─────────────────────────────────────────────────────

    /// Find open PR numbers that need another worker pass.
    ///
    /// A PR needs iteration if a CHANGES_REQUESTED review or any comment
    /// was submitted after the most recent commit. Both conditions are
    /// anchored to the last commit date so addressed feedback doesn't
    /// re-trigger iteration.
    ///
    /// Mirrors `worker_find_prs_needing_iteration` from github.sh.
    fn find_prs_needing_iteration(&self, repo: &str) -> Result<Vec<u64>>;

    // ── Conflicted PRs ────────────────────────────────────────────────────

    /// Find open sipag/issue-* PRs with merge conflicts.
    ///
    /// Excludes PRs with UNKNOWN mergeability to avoid false positives.
    /// Mirrors `worker_find_conflicted_prs` from github.sh.
    fn find_conflicted_prs(&self, repo: &str) -> Result<Vec<PrInfo>>;

    // ── Issue queries ─────────────────────────────────────────────────────

    /// Check if an issue is currently open.
    ///
    /// Returns false for closed or missing issues.
    /// Mirrors `worker_issue_is_open` from github.sh.
    fn issue_is_open(&self, repo: &str, issue_num: u64) -> Result<bool>;

    /// Fetch issue metadata (title, body, state).
    fn get_issue_info(&self, repo: &str, issue_num: u64) -> Result<Option<IssueInfo>>;

    /// List open issue numbers with a specific label.
    fn list_issues_with_label(&self, repo: &str, label: &str) -> Result<Vec<u64>>;

    /// Get the GitHub timeline events for an issue.
    ///
    /// Used by `should_reconcile` to detect cross-references from merged PRs.
    fn get_issue_timeline(&self, repo: &str, issue_num: u64) -> Result<Vec<TimelineEvent>>;

    // ── Issue management ──────────────────────────────────────────────────

    /// Close an issue with a comment.
    fn close_issue(&self, repo: &str, issue_num: u64, comment: &str) -> Result<()>;

    // ── Label management ──────────────────────────────────────────────────

    /// Transition labels on an issue. Either label can be None to skip.
    ///
    /// Must not return an error when the issue is closed, missing, or the
    /// label to remove doesn't exist.
    /// Mirrors `worker_transition_label` from github.sh.
    fn transition_label(
        &self,
        repo: &str,
        issue_num: u64,
        remove: Option<&str>,
        add: Option<&str>,
    ) -> Result<()>;

    // ── PR metadata ───────────────────────────────────────────────────────

    /// Get full PR info by PR number.
    fn get_pr_info(&self, repo: &str, pr_num: u64) -> Result<Option<PrInfo>>;

    /// Get open PRs for a specific branch.
    fn get_open_prs_for_branch(&self, repo: &str, branch: &str) -> Result<Vec<PrInfo>>;

    /// Get merged PRs for a specific branch.
    fn get_merged_prs_for_branch(&self, repo: &str, branch: &str) -> Result<Vec<PrInfo>>;

    // ── Branch management ─────────────────────────────────────────────────

    /// List remote branch names matching a given prefix.
    fn list_branches_with_prefix(&self, repo: &str, prefix: &str) -> Result<Vec<String>>;

    /// How many commits ahead is `head` of `base`?
    ///
    /// Returns 0 if the comparison fails or the branch is not ahead.
    fn branch_ahead_by(&self, repo: &str, base: &str, head: &str) -> Result<u64>;

    /// Create a pull request.
    fn create_pr(&self, repo: &str, branch: &str, title: &str, body: &str) -> Result<()>;

    /// Delete a remote branch.
    fn delete_branch(&self, repo: &str, branch: &str) -> Result<()>;
}

/// Port for reading/writing worker state files.
pub trait StateStore {
    /// Load a worker state for a specific repo/issue.
    fn load(&self, repo_slug: &str, issue_num: u64) -> Result<Option<WorkerState>>;

    /// Save (create or overwrite) a worker state.
    fn save(&self, state: &WorkerState) -> Result<()>;

    /// List all workers with active (non-terminal) status.
    fn list_active(&self) -> Result<Vec<WorkerState>>;
}
