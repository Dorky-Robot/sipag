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
//!   - `config`          — runtime config loaded from env vars and `~/.sipag/config`
//!   - `github`          — GitHub utility functions and minimal gateway adapter
//!   - `github_gateway`  — full `GhCliGateway` implementing `GitHubGateway`
//!   - `hook_runner`     — lifecycle hook execution
//!   - `reconciliation`  — reconcile merged PRs and recover orphaned branches
//!   - `dispatch`        — Docker container dispatch for issue and PR workers
//!   - `poll`            — main polling loop (`sipag work`)

pub mod config;
pub mod decision;
pub mod dispatch;
pub mod github;
pub mod github_gateway;
pub mod hook_runner;
pub mod poll;
pub mod ports;
pub mod reconciliation;
pub mod recovery;
pub mod state;
pub mod status;
pub mod store;

// Re-export core types for backward compatibility.
pub use state::{branch_display, format_duration as format_worker_duration, WorkerState};
pub use status::WorkerStatus;
pub use store::{
    list_all_workers as list_workers, mark_worker_failed_by_container as mark_worker_failed,
};
