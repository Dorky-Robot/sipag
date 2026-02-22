//! Worker lifecycle management.
//!
//! Domain model:
//!   - `WorkerStatus` — enum for worker lifecycle states
//!   - `WorkerState`  — entity representing a single worker's state
//!   - `decision`     — pure functions for issue dispatch and finalization logic
//!   - `ports`        — trait boundaries (ContainerRuntime, GitHubGateway, StateStore)
//!   - `recovery`     — orchestration: recover and finalize active workers
//!   - `store`        — filesystem adapter for state persistence
//!
//! Worker loop (replaces `bin/sipag` + `lib/worker/*.sh`):
//!   - `github`   — GitHub operations via the `gh` CLI
//!   - `dispatch` — Docker container dispatch for issue and PR workers
//!   - `poll`     — main polling loop (`sipag work`)
//!
//! Configuration lives in `crate::config::WorkerConfig` (not in this module).

pub(crate) mod decision;
pub(crate) mod dispatch;
pub(crate) mod github;
pub mod poll;
pub(crate) mod ports;
pub(crate) mod recovery;
pub mod state;
pub(crate) mod status;
pub(crate) mod store;

// Re-export public API — only what external crates actually use.
pub use poll::run_worker_loop;
pub use state::{branch_display, format_duration as format_worker_duration, WorkerState};
pub use status::WorkerStatus;
pub use store::{
    list_all_workers as list_workers, mark_worker_failed_by_container as mark_worker_failed,
};

/// Check that `gh` CLI is authenticated and can reach the GitHub API.
///
/// Thin wrapper so external crates don't need to reach into the internal
/// `github` submodule.
pub fn preflight_gh_auth() -> anyhow::Result<()> {
    github::preflight_gh_auth()
}

/// List open issues with the given label for a repo.
///
/// Thin wrapper for use by `sipag work --dry-run`.
pub fn list_ready_issues(repo: &str, label: &str) -> anyhow::Result<Vec<u64>> {
    github::list_approved_issues(repo, label)
}
