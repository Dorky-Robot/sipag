//! Worker lifecycle management.
//!
//! Domain model:
//!   - `WorkerStatus`  — enum for worker lifecycle states
//!   - `WorkerState`   — entity representing a single worker's state
//!   - `decision`      — pure functions for issue dispatch and finalization logic
//!   - `ports`         — trait boundaries (ContainerRuntime, GitHubGateway, StateStore)
//!   - `recovery`      — orchestration: recover and finalize active workers
//!   - `store`         — filesystem adapter for state persistence
//!
//! Worker polling loop (Rust replacement for lib/worker/loop.sh):
//!   - `config`        — `WorkerConfig` value object (for the poll loop)
//!   - `cycle`         — pure `plan_cycle()` function (no I/O)
//!   - `dispatch`      — Docker container dispatch (issue, PR iteration, conflict-fix)
//!   - `drain`         — `DrainSignal` file-based protocol
//!   - `github`        — GitHub operations via `gh` CLI
//!   - `poll`          — `run_worker_loop()` entry point for `sipag work`

pub mod config;
pub mod cycle;
pub mod decision;
pub mod dispatch;
pub mod drain;
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
