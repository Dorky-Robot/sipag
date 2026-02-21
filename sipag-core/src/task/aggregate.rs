use anyhow::{bail, Result};
use chrono::{DateTime, Utc};

use crate::task::TaskStatus;

/// Opaque identifier for a task (the file stem, e.g. `001-fix-bug`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskId(pub String);

impl TaskId {
    pub fn new(id: impl Into<String>) -> Self {
        TaskId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for TaskId {
    fn from(s: String) -> Self {
        TaskId(s)
    }
}

impl From<&str> for TaskId {
    fn from(s: &str) -> Self {
        TaskId(s.to_string())
    }
}

/// Domain aggregate representing a task with enforced state transitions.
///
/// State machine:
/// ```text
/// Queued ──start()──▶ Running ──complete()──▶ Done
///                              ──fail()──────▶ Failed ──retry()──▶ Queued
/// ```
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
}

impl Task {
    /// Transition from `Queued` to `Running`.
    ///
    /// Returns an error if the task is not currently `Queued`.
    pub fn start(&mut self, now: DateTime<Utc>) -> Result<()> {
        if self.status != TaskStatus::Queue {
            bail!(
                "invalid transition: {} → Running (task must be Queued, got {:?})",
                self.id,
                self.status
            );
        }
        self.status = TaskStatus::Running;
        self.started = Some(now);
        Ok(())
    }

    /// Transition from `Running` to `Done`.
    ///
    /// Returns an error if the task is not currently `Running`.
    pub fn complete(&mut self, now: DateTime<Utc>) -> Result<()> {
        if self.status != TaskStatus::Running {
            bail!(
                "invalid transition: {} → Done (task must be Running, got {:?})",
                self.id,
                self.status
            );
        }
        self.status = TaskStatus::Done;
        self.ended = Some(now);
        Ok(())
    }

    /// Transition from `Running` to `Failed`.
    ///
    /// Returns an error if the task is not currently `Running`.
    pub fn fail(&mut self, now: DateTime<Utc>) -> Result<()> {
        if self.status != TaskStatus::Running {
            bail!(
                "invalid transition: {} → Failed (task must be Running, got {:?})",
                self.id,
                self.status
            );
        }
        self.status = TaskStatus::Failed;
        self.ended = Some(now);
        Ok(())
    }

    /// Transition from `Failed` back to `Queued`.
    ///
    /// Returns an error if the task is not currently `Failed`.
    pub fn retry(&mut self, _now: DateTime<Utc>) -> Result<()> {
        if self.status != TaskStatus::Failed {
            bail!(
                "invalid transition: {} → Queued (task must be Failed, got {:?})",
                self.id,
                self.status
            );
        }
        self.status = TaskStatus::Queue;
        self.started = None;
        self.ended = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task(status: TaskStatus) -> Task {
        Task {
            id: TaskId::new("001-test"),
            title: "Test".to_string(),
            description: String::new(),
            repo: None,
            priority: None,
            status,
            created: None,
            started: None,
            ended: None,
        }
    }

    fn t0() -> DateTime<Utc> {
        DateTime::from_timestamp(0, 0).unwrap()
    }

    #[test]
    fn start_transitions_queued_to_running() {
        let mut task = make_task(TaskStatus::Queue);
        task.start(t0()).unwrap();
        assert_eq!(task.status, TaskStatus::Running);
        assert_eq!(task.started, Some(t0()));
    }

    #[test]
    fn start_fails_if_not_queued() {
        let mut task = make_task(TaskStatus::Running);
        assert!(task.start(t0()).is_err());

        let mut task = make_task(TaskStatus::Done);
        assert!(task.start(t0()).is_err());

        let mut task = make_task(TaskStatus::Failed);
        assert!(task.start(t0()).is_err());
    }

    #[test]
    fn complete_transitions_running_to_done() {
        let mut task = make_task(TaskStatus::Running);
        task.complete(t0()).unwrap();
        assert_eq!(task.status, TaskStatus::Done);
        assert_eq!(task.ended, Some(t0()));
    }

    #[test]
    fn complete_fails_if_not_running() {
        let mut task = make_task(TaskStatus::Queue);
        assert!(task.complete(t0()).is_err());

        let mut task = make_task(TaskStatus::Done);
        assert!(task.complete(t0()).is_err());

        let mut task = make_task(TaskStatus::Failed);
        assert!(task.complete(t0()).is_err());
    }

    #[test]
    fn fail_transitions_running_to_failed() {
        let mut task = make_task(TaskStatus::Running);
        task.fail(t0()).unwrap();
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(task.ended, Some(t0()));
    }

    #[test]
    fn fail_fails_if_not_running() {
        let mut task = make_task(TaskStatus::Queue);
        assert!(task.fail(t0()).is_err());

        let mut task = make_task(TaskStatus::Done);
        assert!(task.fail(t0()).is_err());

        let mut task = make_task(TaskStatus::Failed);
        assert!(task.fail(t0()).is_err());
    }

    #[test]
    fn retry_transitions_failed_to_queued() {
        let mut task = make_task(TaskStatus::Failed);
        task.started = Some(t0());
        task.ended = Some(t0());
        task.retry(t0()).unwrap();
        assert_eq!(task.status, TaskStatus::Queue);
        assert!(task.started.is_none());
        assert!(task.ended.is_none());
    }

    #[test]
    fn retry_fails_if_not_failed() {
        let mut task = make_task(TaskStatus::Queue);
        assert!(task.retry(t0()).is_err());

        let mut task = make_task(TaskStatus::Running);
        assert!(task.retry(t0()).is_err());

        let mut task = make_task(TaskStatus::Done);
        assert!(task.retry(t0()).is_err());
    }

    #[test]
    fn queue_to_done_is_invalid() {
        let mut task = make_task(TaskStatus::Queue);
        assert!(task.complete(t0()).is_err());
        // File should conceptually still be in Queue state
        assert_eq!(task.status, TaskStatus::Queue);
    }

    #[test]
    fn queue_to_failed_is_invalid() {
        let mut task = make_task(TaskStatus::Queue);
        assert!(task.fail(t0()).is_err());
        assert_eq!(task.status, TaskStatus::Queue);
    }
}
