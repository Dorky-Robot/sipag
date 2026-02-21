//! Worker lifecycle management.
//!
//! Domain model:
//!   - `WorkerStatus` — enum for worker lifecycle states
//!   - `WorkerState`  — entity representing a single worker's state
//!   - `decision`     — pure functions for issue dispatch and finalization logic
//!   - `ports`        — trait boundaries (ContainerRuntime, GitHubGateway, StateStore)
//!   - `prompt`       — pure prompt-building functions (PromptBuilder)
//!   - `container`    — ContainerConfig value object, WorkerOutcome, pure planning functions
//!   - `orchestrator` — WorkerOrchestrator: coordinates issue runs via ports
//!   - `recovery`     — orchestration: recover and finalize active workers
//!   - `store`        — filesystem adapter for state persistence

pub mod container;
pub mod decision;
pub mod orchestrator;
pub mod ports;
pub mod prompt;
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

// Re-export new orchestration types.
pub use container::{classify_exit, plan_issue_container, WorkerOutcome};
pub use orchestrator::{issue_branch, WorkerOrchestrator};
pub use ports::{ContainerConfig, ContainerResult, IssueInfo, PrInfo};
pub use prompt::{build_issue_prompt, build_iteration_prompt, IssuePrompt};
