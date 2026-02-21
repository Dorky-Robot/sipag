pub mod aggregate;
pub mod naming;
pub mod parser;
pub mod repository;
pub mod storage;

pub use aggregate::{Task, TaskId};
pub use naming::slugify;
pub use parser::parse_task_content;
pub use repository::{FileTaskRepository, TaskRepository};
pub use storage::{
    append_ended, default_sipag_dir, list_tasks, next_filename, read_task_file, write_task_file,
    write_tracking_file,
};

use std::path::PathBuf;

/// Status of a task based on which directory it lives in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Queue,
    Running,
    Done,
    Failed,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Queue => "queue",
            TaskStatus::Running => "running",
            TaskStatus::Done => "done",
            TaskStatus::Failed => "failed",
        }
    }

    /// Single-character icon for use in list views.
    pub fn icon(&self) -> &'static str {
        match self {
            TaskStatus::Queue => "·",
            TaskStatus::Running => "⧖",
            TaskStatus::Done => "✓",
            TaskStatus::Failed => "✗",
        }
    }

    /// Human-readable status name.
    pub fn name(&self) -> &'static str {
        match self {
            TaskStatus::Queue => "pending",
            TaskStatus::Running => "running",
            TaskStatus::Done => "done",
            TaskStatus::Failed => "failed",
        }
    }

    /// Single-character symbol (alias for icon()).
    pub fn symbol(&self) -> &'static str {
        self.icon()
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A task file with optional YAML frontmatter.
#[derive(Debug, Clone)]
pub struct TaskFile {
    pub name: String,
    pub repo: Option<String>,
    pub priority: String,
    pub source: Option<String>,
    pub added: Option<String>,
    pub started: Option<String>,
    pub ended: Option<String>,
    pub container: Option<String>,
    pub issue: Option<String>,
    pub title: String,
    pub body: String,
    pub status: TaskStatus,
    /// Absolute path to the task file on disk.
    pub file_path: PathBuf,
}
