use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use super::state::WorkerState;

/// Information about a PR found for a branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrInfo {
    pub number: u64,
    pub url: String,
}

/// Information about a GitHub issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueInfo {
    pub title: String,
    pub body: String,
}

/// Configuration for running a worker container.
///
/// Value object describing everything needed to launch a container.
/// Credentials and dynamic env vars are added by the caller before
/// passing to `ContainerRuntime::run_container`.
#[derive(Debug, Clone)]
pub struct ContainerConfig {
    /// Container name (e.g. `sipag-issue-42`).
    pub name: String,
    /// Docker image to use.
    pub image: String,
    /// GitHub repo in `owner/repo` format.
    pub repo: String,
    /// Branch to check out inside the container.
    pub branch: String,
    /// Environment variables to inject into the container.
    pub env: HashMap<String, String>,
    /// Optional execution timeout.
    pub timeout: Option<Duration>,
    /// Path where the container's stdout/stderr should be written.
    pub log_path: PathBuf,
}

/// Result of running a worker container.
#[derive(Debug, Clone)]
pub struct ContainerResult {
    /// Process exit code (0 = success).
    pub exit_code: i32,
    /// Wall-clock duration in seconds.
    pub duration_secs: i64,
}

/// Port for querying Docker container status.
pub trait ContainerRuntime {
    /// Check if a container with the given name is currently running.
    fn is_running(&self, container_name: &str) -> Result<bool>;

    /// Run a container with the given configuration, blocking until it exits.
    ///
    /// Default implementation returns an error — must be overridden by
    /// implementations that support launching containers.
    fn run_container(&self, _config: &ContainerConfig) -> Result<ContainerResult> {
        anyhow::bail!("run_container not implemented for this ContainerRuntime")
    }
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

    /// Fetch issue title and body.
    ///
    /// Default implementation returns an error — must be overridden by
    /// implementations that support fetching issue details.
    fn get_issue(&self, _repo: &str, _issue_num: u64) -> Result<IssueInfo> {
        anyhow::bail!("get_issue not implemented for this GitHubGateway")
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
