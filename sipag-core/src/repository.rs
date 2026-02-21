use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::path::PathBuf;

use crate::task::{parse_task_file, Task, TaskId, TaskStatus};

// ── Trait ─────────────────────────────────────────────────────────────────────

/// Abstraction over task storage. Implementations handle persistence and file moves.
pub trait TaskRepository {
    /// Retrieve a task by id, searching all status directories.
    fn get(&self, id: &TaskId) -> Result<Task>;

    /// Persist a task's current state to disk (write/overwrite the task file).
    fn save(&self, task: &Task) -> Result<()>;

    /// List all tasks with a given status.
    fn list_by_status(&self, status: &TaskStatus) -> Result<Vec<Task>>;

    /// Apply a validated state transition and move the task's files on disk.
    ///
    /// Supported transitions:
    /// - `Queue   → Running` (calls [`Task::start`])
    /// - `Running → Done`    (calls [`Task::complete`])
    /// - `Running → Failed`  (calls [`Task::fail`])
    /// - `Failed  → Queue`   (calls [`Task::retry`], removes log)
    /// - `Queue   → Failed`  (calls [`Task::discard`], administrative)
    fn transition(&self, task: &mut Task, to: TaskStatus, now: DateTime<Utc>) -> Result<()>;
}

// ── FileTaskRepository ────────────────────────────────────────────────────────

/// File-system–backed repository. Maps [`TaskStatus`] to subdirectories of `sipag_dir`.
pub struct FileTaskRepository {
    sipag_dir: PathBuf,
}

impl FileTaskRepository {
    pub fn new(sipag_dir: PathBuf) -> Self {
        Self { sipag_dir }
    }

    fn dir_for_status(&self, status: &TaskStatus) -> PathBuf {
        self.sipag_dir.join(status.as_str())
    }

    fn find_task(&self, id: &TaskId) -> Option<(PathBuf, TaskStatus)> {
        for status in &[
            TaskStatus::Queue,
            TaskStatus::Running,
            TaskStatus::Done,
            TaskStatus::Failed,
        ] {
            let path = self.dir_for_status(status).join(format!("{id}.md"));
            if path.exists() {
                return Some((path, status.clone()));
            }
        }
        None
    }
}

impl TaskRepository for FileTaskRepository {
    fn get(&self, id: &TaskId) -> Result<Task> {
        let (path, status) = self
            .find_task(id)
            .ok_or_else(|| anyhow::anyhow!("task '{}' not found", id))?;
        let task_file = parse_task_file(&path, status)?;
        Ok(Task::from(task_file))
    }

    fn save(&self, task: &Task) -> Result<()> {
        let dir = self.dir_for_status(&task.status);
        let path = dir.join(format!("{}.md", task.id));
        write_task_to_file(task, &path)
    }

    fn list_by_status(&self, status: &TaskStatus) -> Result<Vec<Task>> {
        let dir = self.dir_for_status(status);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
            .with_context(|| format!("Failed to read {}", dir.display()))?
            .flatten()
            .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
            .map(|e| e.path())
            .collect();
        paths.sort();
        let tasks = paths
            .into_iter()
            .filter_map(|p| parse_task_file(&p, status.clone()).ok())
            .map(Task::from)
            .collect();
        Ok(tasks)
    }

    fn transition(&self, task: &mut Task, to: TaskStatus, now: DateTime<Utc>) -> Result<()> {
        let from_status = task.status.clone();

        // Apply and validate the domain transition.
        match (&from_status, &to) {
            (TaskStatus::Queue, TaskStatus::Running) => task.start(now)?,
            (TaskStatus::Running, TaskStatus::Done) => task.complete(now)?,
            (TaskStatus::Running, TaskStatus::Failed) => task.fail(now)?,
            (TaskStatus::Failed, TaskStatus::Queue) => task.retry(now)?,
            (TaskStatus::Queue, TaskStatus::Failed) => task.discard(now)?,
            (from, to) => {
                anyhow::bail!("invalid transition from {} to {}", from, to);
            }
        }

        // Move files on disk.
        let from_dir = self.dir_for_status(&from_status);
        let to_dir = self.dir_for_status(&task.status);

        let from_md = from_dir.join(format!("{}.md", task.id));
        let to_md = to_dir.join(format!("{}.md", task.id));
        let from_log = from_dir.join(format!("{}.log", task.id));
        let to_log = to_dir.join(format!("{}.log", task.id));

        if from_md.exists() {
            std::fs::rename(&from_md, &to_md)
                .with_context(|| format!("Failed to move {} to {}", from_md.display(), to_md.display()))?;
        }

        // For Failed → Queue retries, remove the old log so the retry starts fresh.
        // For all other transitions, move the log alongside the task file.
        if from_status == TaskStatus::Failed && task.status == TaskStatus::Queue {
            if from_log.exists() {
                let _ = std::fs::remove_file(&from_log);
            }
        } else if from_log.exists() {
            std::fs::rename(&from_log, &to_log)
                .with_context(|| format!("Failed to move {} to {}", from_log.display(), to_log.display()))?;
        }

        Ok(())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Write a [`Task`] to a file using a unified YAML-frontmatter format.
fn write_task_to_file(task: &Task, path: &std::path::Path) -> Result<()> {
    let mut content = String::from("---\n");
    if let Some(repo) = &task.repo {
        content.push_str(&format!("repo: {repo}\n"));
    }
    if let Some(priority) = &task.priority {
        content.push_str(&format!("priority: {priority}\n"));
    }
    if let Some(source) = &task.source {
        content.push_str(&format!("source: {source}\n"));
    }
    if let Some(issue) = &task.issue {
        content.push_str(&format!("issue: {issue}\n"));
    }
    if let Some(container) = &task.container {
        content.push_str(&format!("container: {container}\n"));
    }
    if let Some(created) = task.created {
        content.push_str(&format!("added: {}\n", created.format("%Y-%m-%dT%H:%M:%SZ")));
    }
    if let Some(started) = task.started {
        content.push_str(&format!("started: {}\n", started.format("%Y-%m-%dT%H:%M:%SZ")));
    }
    if let Some(ended) = task.ended {
        content.push_str(&format!("ended: {}\n", ended.format("%Y-%m-%dT%H:%M:%SZ")));
    }
    content.push_str("---\n");
    content.push_str(&task.title);
    content.push('\n');
    if !task.description.is_empty() {
        content.push('\n');
        content.push_str(&task.description);
        content.push('\n');
    }
    std::fs::write(path, content)
        .with_context(|| format!("Failed to write {}", path.display()))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{init_dirs, write_task_file};
    use tempfile::TempDir;

    fn setup() -> (TempDir, FileTaskRepository) {
        let dir = TempDir::new().unwrap();
        init_dirs(dir.path()).unwrap();
        let repo = FileTaskRepository::new(dir.path().to_path_buf());
        (dir, repo)
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2024-01-01T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn test_get_task_in_queue() {
        let (dir, repo) = setup();
        write_task_file(
            &dir.path().join("queue/001-fix-bug.md"),
            "Fix the bug",
            "myrepo",
            "high",
            None,
        )
        .unwrap();
        let task = repo.get(&"001-fix-bug".to_string()).unwrap();
        assert_eq!(task.id, "001-fix-bug");
        assert_eq!(task.title, "Fix the bug");
        assert_eq!(task.status, TaskStatus::Queue);
    }

    #[test]
    fn test_get_task_not_found() {
        let (_dir, repo) = setup();
        assert!(repo.get(&"nonexistent".to_string()).is_err());
    }

    #[test]
    fn test_transition_queue_to_running() {
        let (dir, repo) = setup();
        write_task_file(
            &dir.path().join("queue/001-fix-bug.md"),
            "Fix the bug",
            "myrepo",
            "high",
            None,
        )
        .unwrap();
        let mut task = repo.get(&"001-fix-bug".to_string()).unwrap();
        repo.transition(&mut task, TaskStatus::Running, now()).unwrap();

        assert_eq!(task.status, TaskStatus::Running);
        assert!(!dir.path().join("queue/001-fix-bug.md").exists());
        assert!(dir.path().join("running/001-fix-bug.md").exists());
    }

    #[test]
    fn test_transition_running_to_done() {
        let (dir, repo) = setup();
        // Write directly to running/ as if a task was started
        std::fs::write(
            dir.path().join("running/001-fix-bug.md"),
            "---\nrepo: myrepo\nstarted: 2024-01-01T12:00:00Z\ncontainer: sipag-001-fix-bug\n---\nFix the bug\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("running/001-fix-bug.log"), "some log output").unwrap();

        let mut task = repo.get(&"001-fix-bug".to_string()).unwrap();
        repo.transition(&mut task, TaskStatus::Done, now()).unwrap();

        assert_eq!(task.status, TaskStatus::Done);
        assert!(!dir.path().join("running/001-fix-bug.md").exists());
        assert!(dir.path().join("done/001-fix-bug.md").exists());
        assert!(!dir.path().join("running/001-fix-bug.log").exists());
        assert!(dir.path().join("done/001-fix-bug.log").exists());
    }

    #[test]
    fn test_transition_running_to_failed() {
        let (dir, repo) = setup();
        std::fs::write(
            dir.path().join("running/001-fix-bug.md"),
            "---\nrepo: myrepo\nstarted: 2024-01-01T12:00:00Z\ncontainer: sipag-001-fix-bug\n---\nFix the bug\n",
        )
        .unwrap();

        let mut task = repo.get(&"001-fix-bug".to_string()).unwrap();
        repo.transition(&mut task, TaskStatus::Failed, now()).unwrap();

        assert_eq!(task.status, TaskStatus::Failed);
        assert!(dir.path().join("failed/001-fix-bug.md").exists());
    }

    #[test]
    fn test_transition_failed_to_queue_removes_log() {
        let (dir, repo) = setup();
        std::fs::write(
            dir.path().join("failed/001-fix-bug.md"),
            "---\nrepo: myrepo\n---\nFix the bug\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("failed/001-fix-bug.log"), "old log").unwrap();

        let mut task = repo.get(&"001-fix-bug".to_string()).unwrap();
        repo.transition(&mut task, TaskStatus::Queue, now()).unwrap();

        assert_eq!(task.status, TaskStatus::Queue);
        assert!(dir.path().join("queue/001-fix-bug.md").exists());
        // Log should be removed, not moved to queue/
        assert!(!dir.path().join("failed/001-fix-bug.log").exists());
        assert!(!dir.path().join("queue/001-fix-bug.log").exists());
    }

    #[test]
    fn test_transition_queue_to_failed_discard() {
        let (dir, repo) = setup();
        write_task_file(
            &dir.path().join("queue/001-fix-bug.md"),
            "Fix the bug",
            "myrepo",
            "high",
            None,
        )
        .unwrap();

        let mut task = repo.get(&"001-fix-bug".to_string()).unwrap();
        repo.transition(&mut task, TaskStatus::Failed, now()).unwrap();

        assert_eq!(task.status, TaskStatus::Failed);
        assert!(!dir.path().join("queue/001-fix-bug.md").exists());
        assert!(dir.path().join("failed/001-fix-bug.md").exists());
    }

    #[test]
    fn test_transition_invalid_queue_to_done() {
        let (dir, repo) = setup();
        write_task_file(
            &dir.path().join("queue/001-fix-bug.md"),
            "Fix the bug",
            "myrepo",
            "high",
            None,
        )
        .unwrap();
        let mut task = repo.get(&"001-fix-bug".to_string()).unwrap();
        // Queue → Done is not a valid transition
        assert!(repo.transition(&mut task, TaskStatus::Done, now()).is_err());
    }

    #[test]
    fn test_list_by_status() {
        let (dir, repo) = setup();
        write_task_file(
            &dir.path().join("queue/001-task.md"),
            "Task 1",
            "repo1",
            "medium",
            None,
        )
        .unwrap();
        write_task_file(
            &dir.path().join("queue/002-task.md"),
            "Task 2",
            "repo2",
            "high",
            None,
        )
        .unwrap();
        let tasks = repo.list_by_status(&TaskStatus::Queue).unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().all(|t| t.status == TaskStatus::Queue));
    }

    #[test]
    fn test_save_task() {
        let (dir, repo) = setup();
        let task = Task {
            id: "001-fix-bug".to_string(),
            title: "Fix the bug".to_string(),
            description: "Detailed description".to_string(),
            repo: Some("myrepo".to_string()),
            priority: Some("high".to_string()),
            status: TaskStatus::Queue,
            created: Some(now()),
            started: None,
            ended: None,
            source: Some("github#42".to_string()),
            container: None,
            issue: None,
        };
        repo.save(&task).unwrap();
        let loaded = repo.get(&"001-fix-bug".to_string()).unwrap();
        assert_eq!(loaded.title, "Fix the bug");
        assert_eq!(loaded.repo, Some("myrepo".to_string()));
        assert_eq!(loaded.source, Some("github#42".to_string()));
    }
}
