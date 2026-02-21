use anyhow::Result;

use super::state::WorkerState;

/// Information about a PR found for a branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrInfo {
    pub number: u64,
    pub url: String,
}

/// GitHub mergeability status of a PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mergeable {
    Mergeable,
    Conflicting,
    Unknown,
}

/// GitHub merge state status of a PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeState {
    /// All checks pass, no conflicts, no blocking reviews.
    Clean,
    Dirty,
    Blocked,
    Unstable,
    Behind,
    Unknown,
}

/// GitHub review decision for a PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
    /// No review decision (no reviewers assigned / no reviews submitted).
    None,
}

/// A candidate PR for auto-merging, with all fields needed to decide
/// whether it is safe to merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrMergeCandidate {
    pub number: u64,
    pub title: String,
    pub branch: String,
    pub mergeable: Mergeable,
    pub merge_state: MergeState,
    pub is_draft: bool,
    pub review_decision: ReviewDecision,
}

/// Port for querying Docker container status.
pub trait ContainerRuntime {
    /// Check if a container with the given name is currently running.
    fn is_running(&self, container_name: &str) -> Result<bool>;
}

/// Port for querying GitHub API.
pub trait GitHubGateway {
    /// Find a PR (open or merged) for a given branch.
    fn find_pr_for_branch(&self, repo: &str, branch: &str) -> Result<Option<PrInfo>>;

    /// Transition labels on an issue. Either label can be None to skip.
    /// Must not fail on closed/missing issues.
    fn transition_label(
        &self,
        repo: &str,
        issue_num: u64,
        remove: Option<&str>,
        add: Option<&str>,
    ) -> Result<()>;

    /// List open PRs with the data needed for auto-merge decisions.
    fn list_mergeable_prs(&self, repo: &str) -> Result<Vec<PrMergeCandidate>>;

    /// Merge a PR using squash strategy, deleting the head branch.
    fn merge_pr(&self, repo: &str, pr_num: u64, title: &str) -> Result<()>;

    /// Fire a lifecycle hook with the given environment variables.
    ///
    /// Implementations may ignore this call (e.g. in tests). The default
    /// no-ops so that existing gateway implementations do not need to change.
    fn fire_hook(&self, _hook_name: &str, _env: &[(&str, &str)]) -> Result<()> {
        Ok(())
    }
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
