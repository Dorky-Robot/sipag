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
//!   - `cycle`         — pure `plan_cycle()` function (no I/O)
//!   - `work_config`   — `WorkerConfig` value object
//!   - `drain`         — `DrainSignal` file-based protocol
//!   - `gh_gateway`    — `GhCliGateway` + `WorkerPoller` trait
//!   - `docker_runtime`— `DockerCliRuntime` adapter
//!   - `dispatcher`    — `WorkerDispatcher` container launcher
//!   - `loop_runner`   — `WorkerLoop` state machine + `run_worker_loop()` entry point

pub mod cycle;
pub mod decision;
pub mod dispatcher;
pub mod docker_runtime;
pub mod drain;
pub mod gh_gateway;
pub mod loop_runner;
pub mod ports;
pub mod recovery;
pub mod state;
pub mod status;
pub mod store;
pub mod work_config;

// Re-export core types for backward compatibility.
pub use state::{branch_display, format_duration as format_worker_duration, WorkerState};
pub use status::WorkerStatus;
pub use store::{
    list_all_workers as list_workers, mark_worker_failed_by_container as mark_worker_failed,
};
// Entry point for the `sipag work` subcommand.
pub use loop_runner::run_worker_loop;
