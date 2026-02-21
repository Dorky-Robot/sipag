use anyhow::Result;
use chrono::{DateTime, Utc};
use std::fs;
use std::path::{Path, PathBuf};

use crate::task::aggregate::{Task, TaskId};
use crate::task::storage::{read_task_file, write_task_file};
use crate::task::{TaskFile, TaskStatus};

/// Repository trait for task persistence.
///
/// Implementations map task state to a storage medium (e.g. directories on disk).
pub trait TaskRepository {
    /// Load a task by ID, searching all status directories.
    fn get(&self, id: &TaskId) -> Result<Task>;

    /// Persist a new task in its current status directory.
    fn save(&self, task: &Task) -> Result<()>;

    /// List all tasks with the given status.
    fn list_by_status(&self, status: TaskStatus) -> Result<Vec<Task>>;

    /// Apply a state transition, enforce the domain aggregate rules, and persist
    /// the resulting file move.
    ///
    /// Delegates to the corresponding `Task` method (`start`, `complete`, `fail`,
    /// or `retry`) so invalid transitions always return an error.
    fn transition(&self, task: &mut Task, to: TaskStatus, now: DateTime<Utc>) -> Result<()>;
}

/// File-system backed `TaskRepository`.
///
/// Task status is encoded in the subdirectory under `sipag_dir`:
/// - `queue/`   → `TaskStatus::Queue`
/// - `running/` → `TaskStatus::Running`
/// - `done/`    → `TaskStatus::Done`
/// - `failed/`  → `TaskStatus::Failed`
pub struct FileTaskRepository {
    sipag_dir: PathBuf,
}

impl FileTaskRepository {
    pub fn new(sipag_dir: PathBuf) -> Self {
        FileTaskRepository { sipag_dir }
    }

    fn dir_for_status(&self, status: &TaskStatus) -> PathBuf {
        self.sipag_dir.join(status.as_str())
    }
}

fn parse_dt(s: &str) -> Option<DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn task_from_file(tf: TaskFile) -> Task {
    Task {
        id: TaskId::new(&tf.name),
        title: tf.title,
        description: tf.body,
        repo: tf.repo,
        priority: Some(tf.priority),
        status: tf.status,
        created: tf.added.as_deref().and_then(parse_dt),
        started: tf.started.as_deref().and_then(parse_dt),
        ended: tf.ended.as_deref().and_then(parse_dt),
    }
}

impl TaskRepository for FileTaskRepository {
    fn get(&self, id: &TaskId) -> Result<Task> {
        for status in &[
            TaskStatus::Queue,
            TaskStatus::Running,
            TaskStatus::Done,
            TaskStatus::Failed,
        ] {
            let path = self
                .dir_for_status(status)
                .join(format!("{}.md", id.as_str()));
            if path.exists() {
                let tf = read_task_file(&path, status.clone())?;
                return Ok(task_from_file(tf));
            }
        }
        anyhow::bail!("task '{}' not found in any status directory", id.as_str())
    }

    fn save(&self, task: &Task) -> Result<()> {
        let dir = self.dir_for_status(&task.status);
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.md", task.id.as_str()));
        let added = task
            .created
            .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
            .unwrap_or_default();
        write_task_file(
            &path,
            &task.title,
            task.repo.as_deref().unwrap_or(""),
            task.priority.as_deref().unwrap_or("medium"),
            None,
            &added,
        )
    }

    fn list_by_status(&self, status: TaskStatus) -> Result<Vec<Task>> {
        let dir = self.dir_for_status(&status);
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", dir.display(), e))?
            .flatten()
            .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
            .map(|e| e.path())
            .collect();
        paths.sort();
        Ok(paths
            .into_iter()
            .filter_map(|p| read_task_file(&p, status.clone()).ok().map(task_from_file))
            .collect())
    }

    fn transition(&self, task: &mut Task, to: TaskStatus, now: DateTime<Utc>) -> Result<()> {
        let from_dir = self.dir_for_status(&task.status);
        let filename = format!("{}.md", task.id.as_str());
        let from_path = from_dir.join(&filename);

        // Enforce the transition via the domain aggregate (errors on invalid paths).
        match &to {
            TaskStatus::Running => task.start(now)?,
            TaskStatus::Done => task.complete(now)?,
            TaskStatus::Failed => task.fail(now)?,
            TaskStatus::Queue => task.retry(now)?,
        }

        let to_dir = self.dir_for_status(&task.status);
        let to_path = to_dir.join(&filename);

        if from_path.exists() {
            fs::rename(&from_path, &to_path)?;
        }

        // Move the companion log file if present.
        move_companion_log(&from_dir, &to_dir, task.id.as_str());

        Ok(())
    }
}

fn move_companion_log(from_dir: &Path, to_dir: &Path, task_id: &str) {
    let log_from = from_dir.join(format!("{task_id}.log"));
    if log_from.exists() {
        let log_to = to_dir.join(format!("{task_id}.log"));
        let _ = fs::rename(log_from, log_to);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::init_dirs;
    use tempfile::TempDir;

    fn setup() -> (TempDir, FileTaskRepository) {
        let dir = TempDir::new().unwrap();
        init_dirs(dir.path()).unwrap();
        let repo = FileTaskRepository::new(dir.path().to_path_buf());
        (dir, repo)
    }

    fn write_task_in(dir: &Path, subdir: &str, name: &str) {
        let path = dir.join(subdir).join(format!("{name}.md"));
        write_task_file(
            &path,
            "Test Task",
            "myrepo",
            "medium",
            None,
            "2024-01-01T00:00:00Z",
        )
        .unwrap();
    }

    fn t0() -> DateTime<Utc> {
        DateTime::from_timestamp(0, 0).unwrap()
    }

    #[test]
    fn get_finds_task_in_queue() {
        let (dir, repo) = setup();
        write_task_in(dir.path(), "queue", "001-test");
        let task = repo.get(&TaskId::new("001-test")).unwrap();
        assert_eq!(task.id.as_str(), "001-test");
        assert_eq!(task.status, TaskStatus::Queue);
    }

    #[test]
    fn get_finds_task_in_running() {
        let (dir, repo) = setup();
        write_task_in(dir.path(), "running", "001-test");
        let task = repo.get(&TaskId::new("001-test")).unwrap();
        assert_eq!(task.status, TaskStatus::Running);
    }

    #[test]
    fn get_returns_error_if_not_found() {
        let (_dir, repo) = setup();
        assert!(repo.get(&TaskId::new("nonexistent")).is_err());
    }

    #[test]
    fn list_by_status_returns_tasks() {
        let (dir, repo) = setup();
        write_task_in(dir.path(), "queue", "001-a");
        write_task_in(dir.path(), "queue", "002-b");
        let tasks = repo.list_by_status(TaskStatus::Queue).unwrap();
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn list_by_status_empty_dir() {
        let (_dir, repo) = setup();
        let tasks = repo.list_by_status(TaskStatus::Running).unwrap();
        assert!(tasks.is_empty());
    }

    #[test]
    fn save_writes_task_to_correct_directory() {
        let (dir, repo) = setup();
        let task = Task {
            id: TaskId::new("001-test"),
            title: "Test Task".to_string(),
            description: "body".to_string(),
            repo: Some("myrepo".to_string()),
            priority: Some("medium".to_string()),
            status: TaskStatus::Queue,
            created: Some(t0()),
            started: None,
            ended: None,
        };
        repo.save(&task).unwrap();
        assert!(dir.path().join("queue/001-test.md").exists());
    }

    #[test]
    fn transition_queue_to_running() {
        let (dir, repo) = setup();
        write_task_in(dir.path(), "queue", "001-test");
        let mut task = repo.get(&TaskId::new("001-test")).unwrap();

        repo.transition(&mut task, TaskStatus::Running, t0())
            .unwrap();

        assert_eq!(task.status, TaskStatus::Running);
        assert!(dir.path().join("running/001-test.md").exists());
        assert!(!dir.path().join("queue/001-test.md").exists());
    }

    #[test]
    fn transition_running_to_done() {
        let (dir, repo) = setup();
        write_task_in(dir.path(), "running", "001-test");
        let mut task = repo.get(&TaskId::new("001-test")).unwrap();

        repo.transition(&mut task, TaskStatus::Done, t0()).unwrap();

        assert_eq!(task.status, TaskStatus::Done);
        assert!(dir.path().join("done/001-test.md").exists());
        assert!(!dir.path().join("running/001-test.md").exists());
    }

    #[test]
    fn transition_running_to_failed() {
        let (dir, repo) = setup();
        write_task_in(dir.path(), "running", "001-test");
        let mut task = repo.get(&TaskId::new("001-test")).unwrap();

        repo.transition(&mut task, TaskStatus::Failed, t0())
            .unwrap();

        assert_eq!(task.status, TaskStatus::Failed);
        assert!(dir.path().join("failed/001-test.md").exists());
        assert!(!dir.path().join("running/001-test.md").exists());
    }

    #[test]
    fn transition_failed_to_queue() {
        let (dir, repo) = setup();
        write_task_in(dir.path(), "failed", "001-test");
        let mut task = repo.get(&TaskId::new("001-test")).unwrap();

        repo.transition(&mut task, TaskStatus::Queue, t0()).unwrap();

        assert_eq!(task.status, TaskStatus::Queue);
        assert!(dir.path().join("queue/001-test.md").exists());
        assert!(!dir.path().join("failed/001-test.md").exists());
    }

    #[test]
    fn transition_enforces_queue_to_done_is_invalid() {
        let (dir, repo) = setup();
        write_task_in(dir.path(), "queue", "001-test");
        let mut task = repo.get(&TaskId::new("001-test")).unwrap();

        let err = repo.transition(&mut task, TaskStatus::Done, t0());
        assert!(err.is_err());
        // Task status unchanged; file still in queue
        assert_eq!(task.status, TaskStatus::Queue);
        assert!(dir.path().join("queue/001-test.md").exists());
    }

    #[test]
    fn transition_enforces_queue_to_failed_is_invalid() {
        let (dir, repo) = setup();
        write_task_in(dir.path(), "queue", "001-test");
        let mut task = repo.get(&TaskId::new("001-test")).unwrap();

        let err = repo.transition(&mut task, TaskStatus::Failed, t0());
        assert!(err.is_err());
        assert_eq!(task.status, TaskStatus::Queue);
        assert!(dir.path().join("queue/001-test.md").exists());
    }

    #[test]
    fn transition_moves_companion_log() {
        let (dir, repo) = setup();
        write_task_in(dir.path(), "running", "001-test");
        fs::write(dir.path().join("running/001-test.log"), "log content").unwrap();

        let mut task = repo.get(&TaskId::new("001-test")).unwrap();
        repo.transition(&mut task, TaskStatus::Done, t0()).unwrap();

        assert!(dir.path().join("done/001-test.log").exists());
        assert!(!dir.path().join("running/001-test.log").exists());
    }
}
