use anyhow::Result;

use super::state::WorkerState;

/// Information about a PR found for a branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrInfo {
    pub number: u64,
    pub url: String,
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
