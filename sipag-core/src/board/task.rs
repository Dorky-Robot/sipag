//! Task data model — individual TOML files per task.
//!
//! Stored at `~/.sipag/projects/{project}/tasks/{NNN}.toml`.

use anyhow::{Context, Result};
use std::fmt;
use std::path::{Path, PathBuf};

use super::atomic_write;

/// Task status values.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskStatus {
    Backlog,
    Todo,
    InProgress,
    Review,
    Done,
    /// Catch-all for custom statuses.
    #[serde(untagged)]
    Custom(String),
}

impl TaskStatus {
    pub fn parse(s: &str) -> Self {
        match s {
            "backlog" => Self::Backlog,
            "todo" => Self::Todo,
            "in-progress" => Self::InProgress,
            "review" => Self::Review,
            "done" => Self::Done,
            other => Self::Custom(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Backlog => "backlog",
            Self::Todo => "todo",
            Self::InProgress => "in-progress",
            Self::Review => "review",
            Self::Done => "done",
            Self::Custom(s) => s,
        }
    }
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single task on the board.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Task {
    pub id: u64,
    pub title: String,
    pub status: TaskStatus,
    #[serde(default = "default_role")]
    pub role: String,
    #[serde(default)]
    pub labels: Vec<String>,
    /// Key-result ids inside the same project that this task advances.
    /// Empty means "loose" — not laddered to any KR.
    #[serde(default)]
    pub key_results: Vec<u64>,
    pub created: String,
    pub updated: String,
}

fn default_role() -> String {
    "dev".to_string()
}

impl Task {
    /// Directory containing task files for a project.
    fn tasks_dir(sipag_dir: &Path, project: &str) -> PathBuf {
        sipag_dir.join("projects").join(project).join("tasks")
    }

    /// Path to a specific task file.
    fn file_path(sipag_dir: &Path, project: &str, id: u64) -> PathBuf {
        Self::tasks_dir(sipag_dir, project).join(format!("{:03}.toml", id))
    }

    /// Load a single task by ID.
    pub fn load(sipag_dir: &Path, project: &str, id: u64) -> Result<Self> {
        let path = Self::file_path(sipag_dir, project, id);
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("task #{id} not found in project {project}"))?;
        let task: Self = toml::from_str(&content)
            .with_context(|| format!("invalid TOML in {}", path.display()))?;
        Ok(task)
    }

    /// Save this task to disk (atomic write).
    pub fn save(&self, sipag_dir: &Path, project: &str) -> Result<()> {
        let path = Self::file_path(sipag_dir, project, self.id);
        let content = toml::to_string_pretty(self)?;
        atomic_write(&path, content.as_bytes())
    }

    /// List all tasks for a project.
    pub fn list(sipag_dir: &Path, project: &str) -> Result<Vec<Self>> {
        let dir = Self::tasks_dir(sipag_dir, project);
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut tasks = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                let content = std::fs::read_to_string(&path)?;
                match toml::from_str::<Self>(&content) {
                    Ok(task) => tasks.push(task),
                    Err(e) => {
                        // Log but skip corrupt files.
                        log::warn!("skipping {}: {e}", path.display());
                    }
                }
            }
        }
        Ok(tasks)
    }

    /// Compute the next task ID by scanning existing task files.
    pub fn next_id(sipag_dir: &Path, project: &str) -> Result<u64> {
        let tasks = Self::list(sipag_dir, project)?;
        let max_id = tasks.iter().map(|t| t.id).max().unwrap_or(0);
        Ok(max_id + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_project(dir: &Path) {
        let tasks_dir = dir.join("projects").join("test").join("tasks");
        std::fs::create_dir_all(&tasks_dir).unwrap();
        // Also create project.toml so it's a valid project.
        let project_dir = dir.join("projects").join("test");
        std::fs::write(
            project_dir.join("project.toml"),
            "name = \"test\"\nrepo = \"a/b\"\nstatuses = [\"backlog\", \"todo\", \"done\"]\n",
        )
        .unwrap();
    }

    #[test]
    fn task_status_parse_and_display() {
        assert_eq!(TaskStatus::parse("backlog"), TaskStatus::Backlog);
        assert_eq!(TaskStatus::parse("in-progress"), TaskStatus::InProgress);
        assert_eq!(TaskStatus::parse("done"), TaskStatus::Done);
        assert_eq!(
            TaskStatus::parse("custom-thing"),
            TaskStatus::Custom("custom-thing".to_string())
        );
        assert_eq!(TaskStatus::InProgress.to_string(), "in-progress");
    }

    #[test]
    fn task_round_trip() {
        let dir = TempDir::new().unwrap();
        setup_project(dir.path());

        let task = Task {
            id: 1,
            title: "Fix auth bug".to_string(),
            status: TaskStatus::Todo,
            role: "dev".to_string(),
            labels: vec!["bug".to_string()],
            key_results: vec![],
            created: "2026-04-01T12:00:00Z".to_string(),
            updated: "2026-04-01T12:00:00Z".to_string(),
        };
        task.save(dir.path(), "test").unwrap();

        let loaded = Task::load(dir.path(), "test", 1).unwrap();
        assert_eq!(loaded.id, 1);
        assert_eq!(loaded.title, "Fix auth bug");
        assert_eq!(loaded.status, TaskStatus::Todo);
        assert_eq!(loaded.role, "dev");
        assert_eq!(loaded.labels, vec!["bug"]);
    }

    #[test]
    fn task_list_empty() {
        let dir = TempDir::new().unwrap();
        setup_project(dir.path());
        let tasks = Task::list(dir.path(), "test").unwrap();
        assert!(tasks.is_empty());
    }

    #[test]
    fn task_next_id_starts_at_one() {
        let dir = TempDir::new().unwrap();
        setup_project(dir.path());
        assert_eq!(Task::next_id(dir.path(), "test").unwrap(), 1);
    }

    #[test]
    fn task_next_id_increments() {
        let dir = TempDir::new().unwrap();
        setup_project(dir.path());

        let task = Task {
            id: 5,
            title: "Existing".to_string(),
            status: TaskStatus::Done,
            role: "dev".to_string(),
            labels: vec![],
            key_results: vec![],
            created: "2026-01-01T00:00:00Z".to_string(),
            updated: "2026-01-01T00:00:00Z".to_string(),
        };
        task.save(dir.path(), "test").unwrap();

        assert_eq!(Task::next_id(dir.path(), "test").unwrap(), 6);
    }

    #[test]
    fn task_load_nonexistent_fails() {
        let dir = TempDir::new().unwrap();
        setup_project(dir.path());
        assert!(Task::load(dir.path(), "test", 999).is_err());
    }
}
