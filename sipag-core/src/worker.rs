//! Worker lifecycle management.
//!
//! Domain model:
//!   - `WorkerStatus` — enum for worker lifecycle states
//!   - `WorkerState`  — entity representing a single worker's state
//!   - `auto_merge`   — typed auto-merge service (replaces lib/worker/merge.sh)
//!   - `decision`     — pure functions for issue dispatch and finalization logic
//!   - `ports`        — trait boundaries (ContainerRuntime, GitHubGateway, StateStore)
//!   - `recovery`     — orchestration: recover and finalize active workers
//!   - `store`        — filesystem adapter for state persistence

pub mod auto_merge;
pub mod decision;
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
