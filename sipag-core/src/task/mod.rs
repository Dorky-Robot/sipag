pub mod naming;
pub mod parser;
pub mod storage;

use anyhow::Result;
use chrono::{DateTime, Utc};

pub use naming::slugify;
pub use parser::parse_task_content;
pub use storage::{
    append_ended, default_sipag_dir, list_tasks, next_filename, read_task_file, write_task_file,
    write_tracking_file,
};

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

    /// Single-character symbol for display (used by TUI).
    pub fn symbol(&self) -> &'static str {
        match self {
            TaskStatus::Queue => "·",
            TaskStatus::Running => "⧖",
            TaskStatus::Done => "✓",
            TaskStatus::Failed => "✗",
        }
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
}

// ── Task domain aggregate ─────────────────────────────────────────────────────

/// A unique task identifier (the file stem, e.g. "001-fix-bug" or "20240101120000-fix-bug").
pub type TaskId = String;

/// Domain aggregate representing a task with enforced state transitions.
///
/// Valid transitions:
/// - `Queued  → Running` via [`Task::start`]
/// - `Running → Done`    via [`Task::complete`]
/// - `Running → Failed`  via [`Task::fail`]
/// - `Failed  → Queued`  via [`Task::retry`]
/// - `Queued  → Failed`  via [`Task::discard`] (administrative, e.g. parse errors)
#[derive(Debug, Clone)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub description: String,
    pub repo: Option<String>,
    pub priority: Option<String>,
    pub status: TaskStatus,
    pub created: Option<DateTime<Utc>>,
    pub started: Option<DateTime<Utc>>,
    pub ended: Option<DateTime<Utc>>,
    pub source: Option<String>,
    pub container: Option<String>,
    pub issue: Option<String>,
}

impl Task {
    /// Create a minimal task with a given id and status (useful for internal transitions).
    pub fn with_status(id: TaskId, status: TaskStatus) -> Self {
        Self {
            id,
            title: String::new(),
            description: String::new(),
            repo: None,
            priority: None,
            status,
            created: None,
            started: None,
            ended: None,
            source: None,
            container: None,
            issue: None,
        }
    }

    /// `Queued → Running`. Errors if the current status is not `Queued`.
    /// The caller supplies the timestamp so it can be injected in tests.
    pub fn start(&mut self, now: DateTime<Utc>) -> Result<()> {
        if self.status != TaskStatus::Queue {
            anyhow::bail!(
                "cannot start task '{}': expected Queued, got {}",
                self.id,
                self.status
            );
        }
        self.status = TaskStatus::Running;
        self.started = Some(now);
        Ok(())
    }

    /// `Running → Done`. Errors if the current status is not `Running`.
    pub fn complete(&mut self, now: DateTime<Utc>) -> Result<()> {
        if self.status != TaskStatus::Running {
            anyhow::bail!(
                "cannot complete task '{}': expected Running, got {}",
                self.id,
                self.status
            );
        }
        self.status = TaskStatus::Done;
        self.ended = Some(now);
        Ok(())
    }

    /// `Running → Failed`. Errors if the current status is not `Running`.
    pub fn fail(&mut self, now: DateTime<Utc>) -> Result<()> {
        if self.status != TaskStatus::Running {
            anyhow::bail!(
                "cannot fail task '{}': expected Running, got {}",
                self.id,
                self.status
            );
        }
        self.status = TaskStatus::Failed;
        self.ended = Some(now);
        Ok(())
    }

    /// `Failed → Queued`. Errors if the current status is not `Failed`.
    pub fn retry(&mut self, _now: DateTime<Utc>) -> Result<()> {
        if self.status != TaskStatus::Failed {
            anyhow::bail!(
                "cannot retry task '{}': expected Failed, got {}",
                self.id,
                self.status
            );
        }
        self.status = TaskStatus::Queue;
        self.started = None;
        self.ended = None;
        Ok(())
    }

    /// `Queued → Failed` (administrative). Used when a queued task cannot be started
    /// due to infrastructure failures (e.g. unparseable file, missing repo config).
    pub fn discard(&mut self, now: DateTime<Utc>) -> Result<()> {
        if self.status != TaskStatus::Queue {
            anyhow::bail!(
                "cannot discard task '{}': expected Queued, got {}",
                self.id,
                self.status
            );
        }
        self.status = TaskStatus::Failed;
        self.ended = Some(now);
        Ok(())
    }
}

impl From<TaskFile> for Task {
    fn from(f: TaskFile) -> Self {
        let parse_dt = |s: &str| {
            DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
        };
        Task {
            id: f.name,
            title: f.title,
            description: f.body,
            repo: f.repo,
            priority: Some(f.priority),
            status: f.status,
            created: f.added.as_deref().and_then(parse_dt),
            started: f.started.as_deref().and_then(parse_dt),
            ended: f.ended.as_deref().and_then(parse_dt),
            source: f.source,
            container: f.container,
            issue: f.issue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2024-01-01T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn test_task_start_queued_to_running() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Queue);
        task.start(now()).unwrap();
        assert_eq!(task.status, TaskStatus::Running);
        assert_eq!(task.started, Some(now()));
    }

    #[test]
    fn test_task_start_rejects_running() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Running);
        assert!(task.start(now()).is_err());
    }

    #[test]
    fn test_task_start_rejects_done() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Done);
        assert!(task.start(now()).is_err());
    }

    #[test]
    fn test_task_start_rejects_failed() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Failed);
        assert!(task.start(now()).is_err());
    }

    #[test]
    fn test_task_complete_running_to_done() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Running);
        task.complete(now()).unwrap();
        assert_eq!(task.status, TaskStatus::Done);
        assert_eq!(task.ended, Some(now()));
    }

    #[test]
    fn test_task_complete_rejects_queue() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Queue);
        assert!(task.complete(now()).is_err());
    }

    #[test]
    fn test_task_complete_rejects_done() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Done);
        assert!(task.complete(now()).is_err());
    }

    #[test]
    fn test_task_fail_running_to_failed() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Running);
        task.fail(now()).unwrap();
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(task.ended, Some(now()));
    }

    #[test]
    fn test_task_fail_rejects_queue() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Queue);
        assert!(task.fail(now()).is_err());
    }

    #[test]
    fn test_task_fail_rejects_done() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Done);
        assert!(task.fail(now()).is_err());
    }

    #[test]
    fn test_task_retry_failed_to_queue() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Failed);
        task.started = Some(now());
        task.ended = Some(now());
        task.retry(now()).unwrap();
        assert_eq!(task.status, TaskStatus::Queue);
        assert_eq!(task.started, None);
        assert_eq!(task.ended, None);
    }

    #[test]
    fn test_task_retry_rejects_queue() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Queue);
        assert!(task.retry(now()).is_err());
    }

    #[test]
    fn test_task_retry_rejects_running() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Running);
        assert!(task.retry(now()).is_err());
    }

    #[test]
    fn test_task_discard_queue_to_failed() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Queue);
        task.discard(now()).unwrap();
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(task.ended, Some(now()));
    }

    #[test]
    fn test_task_discard_rejects_running() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Running);
        assert!(task.discard(now()).is_err());
    }

    #[test]
    fn test_invalid_transition_queue_to_done() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Queue);
        assert!(task.complete(now()).is_err());
    }

    #[test]
    fn test_task_full_lifecycle() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Queue);
        assert_eq!(task.status, TaskStatus::Queue);

        task.start(now()).unwrap();
        assert_eq!(task.status, TaskStatus::Running);

        task.fail(now()).unwrap();
        assert_eq!(task.status, TaskStatus::Failed);

        task.retry(now()).unwrap();
        assert_eq!(task.status, TaskStatus::Queue);

        task.start(now()).unwrap();
        task.complete(now()).unwrap();
        assert_eq!(task.status, TaskStatus::Done);
    }

    #[test]
    fn test_task_symbol() {
        assert_eq!(TaskStatus::Queue.symbol(), "·");
        assert_eq!(TaskStatus::Running.symbol(), "⧖");
        assert_eq!(TaskStatus::Done.symbol(), "✓");
        assert_eq!(TaskStatus::Failed.symbol(), "✗");
    }
}
