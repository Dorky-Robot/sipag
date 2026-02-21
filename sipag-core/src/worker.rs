//! Worker lifecycle management.
//!
//! Domain model:
//!   - `WorkerStatus` — enum for worker lifecycle states
//!   - `WorkerState`  — entity representing a single worker's state
//!   - `decision`     — pure functions for issue dispatch and finalization logic
//!   - `dedup`        — typed replacements for lib/worker/dedup.sh
//!   - `ports`        — trait boundaries (ContainerRuntime, GitHubGateway, StateStore)
//!   - `recovery`     — orchestration: recover and finalize active workers
//!   - `store`        — filesystem adapter for state persistence
//!
//! Worker loop (replaces `bin/sipag` + `lib/worker/*.sh`):
//!   - `config`   — runtime config loaded from env vars and `~/.sipag/config`
//!   - `github`   — GitHub operations via the `gh` CLI
//!   - `dispatch` — Docker container dispatch for issue and PR workers
//!   - `poll`     — main polling loop (`sipag work`)

pub mod config;
pub mod decision;
pub mod dedup;
pub mod dispatch;
pub mod github;
pub mod poll;
pub mod ports;
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
