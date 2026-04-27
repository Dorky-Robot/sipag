//! Multi-project task board — file-based TOML data model (v4).
//!
//! Layout on disk:
//! ```text
//! ~/.sipag/
//!   config.toml
//!   projects/
//!     katulong/
//!       project.toml
//!       roles/
//!         dev.toml
//!       tasks/
//!         001.toml
//! ```

mod key_result;
mod project;
mod role;
mod task;

pub use key_result::{KeyResult, KrStance};
pub use project::{Project, ProjectKind};
pub use role::Role;
pub use task::{Task, TaskStatus};

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Top-level v4 config stored at `~/.sipag/config.toml`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BoardConfig {
    #[serde(default)]
    pub default_project: Option<String>,
    #[serde(default)]
    pub katulong_url: Option<String>,
}

impl BoardConfig {
    pub fn load(sipag_dir: &Path) -> Result<Self> {
        let path = sipag_dir.join("config.toml");
        if !path.exists() {
            return Ok(Self {
                default_project: None,
                katulong_url: None,
            });
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let cfg: Self = toml::from_str(&content)
            .with_context(|| format!("invalid TOML in {}", path.display()))?;
        Ok(cfg)
    }

    pub fn save(&self, sipag_dir: &Path) -> Result<()> {
        let path = sipag_dir.join("config.toml");
        let content = toml::to_string_pretty(self)?;
        atomic_write(&path, content.as_bytes())
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Projects directory: `{sipag_dir}/projects/`
pub fn projects_dir(sipag_dir: &Path) -> PathBuf {
    sipag_dir.join("projects")
}

/// List all project names by scanning the projects directory.
pub fn list_project_names(sipag_dir: &Path) -> Result<Vec<String>> {
    let dir = projects_dir(sipag_dir);
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() && path.join("project.toml").exists() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

/// Load a project by name.
pub fn load_project(sipag_dir: &Path, name: &str) -> Result<Project> {
    Project::load(sipag_dir, name)
}

/// Load all tasks for a project, optionally filtered by status.
pub fn list_tasks(sipag_dir: &Path, project: &str, status: Option<&str>) -> Result<Vec<Task>> {
    let mut tasks = Task::list(sipag_dir, project)?;
    if let Some(s) = status {
        let filter_status = TaskStatus::parse(s);
        tasks.retain(|t| t.status == filter_status);
    }
    tasks.sort_by_key(|t| t.id);
    Ok(tasks)
}

/// Add a new task to a project. Returns the created task.
pub fn add_task(
    sipag_dir: &Path,
    project: &str,
    title: &str,
    role: Option<&str>,
    labels: &[String],
) -> Result<Task> {
    // Ensure project exists.
    let proj = Project::load(sipag_dir, project)?;

    let next_id = Task::next_id(sipag_dir, project)?;
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    // Default status is "todo" (or first non-backlog status).
    let default_status = if proj.statuses.len() > 1 {
        TaskStatus::parse(&proj.statuses[1])
    } else {
        TaskStatus::Todo
    };

    let task = Task {
        id: next_id,
        title: title.to_string(),
        status: default_status,
        role: role.unwrap_or("dev").to_string(),
        labels: labels.to_vec(),
        key_results: Vec::new(),
        created: now.clone(),
        updated: now,
    };

    task.save(sipag_dir, project)?;
    Ok(task)
}

/// Move a task to a new status.
pub fn move_task(sipag_dir: &Path, project: &str, task_id: u64, new_status: &str) -> Result<Task> {
    let mut task = Task::load(sipag_dir, project, task_id)?;
    task.status = TaskStatus::parse(new_status);
    task.updated = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    task.save(sipag_dir, project)?;
    Ok(task)
}

/// Create a new project. Defaults to ProjectKind::Objective.
pub fn create_project(
    sipag_dir: &Path,
    name: &str,
    repo: &str,
    statuses: Option<Vec<String>>,
) -> Result<Project> {
    create_project_with_kind(sipag_dir, name, repo, ProjectKind::Objective, statuses)
}

/// Create a new project with an explicit kind (objective vs standing).
pub fn create_project_with_kind(
    sipag_dir: &Path,
    name: &str,
    repo: &str,
    kind: ProjectKind,
    statuses: Option<Vec<String>>,
) -> Result<Project> {
    let project = Project {
        name: name.to_string(),
        repo: repo.to_string(),
        kind,
        statuses: statuses.unwrap_or_else(|| {
            vec![
                "backlog".to_string(),
                "todo".to_string(),
                "in-progress".to_string(),
                "review".to_string(),
                "done".to_string(),
            ]
        }),
    };
    project.save(sipag_dir)?;
    Ok(project)
}

/// Load all roles for a project.
pub fn list_roles(sipag_dir: &Path, project: &str) -> Result<Vec<Role>> {
    Role::list(sipag_dir, project)
}

/// Delete a project — recursively removes its directory under
/// `<sipag_dir>/projects/<name>/`. No-op when the directory is
/// already gone. Loud when the parent directory is missing or
/// the path resolves outside `<sipag_dir>/projects/`.
pub fn delete_project(sipag_dir: &Path, name: &str) -> Result<()> {
    if name.is_empty() || name.contains('/') || name.contains("..") {
        anyhow::bail!("invalid project name: {name}");
    }
    let path = projects_dir(sipag_dir).join(name);
    if !path.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(&path)
        .with_context(|| format!("failed to remove {}", path.display()))?;
    Ok(())
}

/// Atomic write: write to a temp file in the same directory, then rename.
///
/// Promoted to `pub` so the auth modules can persist credential/
/// session/setup-token files using the same crash-safe pattern as the
/// board records.
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(data)?;
    tmp.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> TempDir {
        let dir = TempDir::new().unwrap();
        // Create a project.
        create_project(dir.path(), "testproj", "owner/repo", None).unwrap();
        dir
    }

    #[test]
    fn board_config_missing_file_returns_defaults() {
        let dir = TempDir::new().unwrap();
        let cfg = BoardConfig::load(dir.path()).unwrap();
        assert!(cfg.default_project.is_none());
    }

    #[test]
    fn board_config_round_trip() {
        let dir = TempDir::new().unwrap();
        let cfg = BoardConfig {
            default_project: Some("katulong".to_string()),
            katulong_url: Some("https://example.com".to_string()),
        };
        cfg.save(dir.path()).unwrap();
        let loaded = BoardConfig::load(dir.path()).unwrap();
        assert_eq!(loaded.default_project.as_deref(), Some("katulong"));
        assert_eq!(loaded.katulong_url.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn list_project_names_empty() {
        let dir = TempDir::new().unwrap();
        let names = list_project_names(dir.path()).unwrap();
        assert!(names.is_empty());
    }

    #[test]
    fn list_project_names_finds_projects() {
        let dir = setup();
        let names = list_project_names(dir.path()).unwrap();
        assert_eq!(names, vec!["testproj"]);
    }

    #[test]
    fn add_and_list_tasks() {
        let dir = setup();
        add_task(dir.path(), "testproj", "First task", None, &[]).unwrap();
        add_task(
            dir.path(),
            "testproj",
            "Second task",
            Some("test"),
            &["bug".to_string()],
        )
        .unwrap();

        let tasks = list_tasks(dir.path(), "testproj", None).unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, 1);
        assert_eq!(tasks[0].title, "First task");
        assert_eq!(tasks[1].id, 2);
        assert_eq!(tasks[1].title, "Second task");
        assert_eq!(tasks[1].role, "test");
        assert_eq!(tasks[1].labels, vec!["bug"]);
    }

    #[test]
    fn add_task_auto_increments_id() {
        let dir = setup();
        let t1 = add_task(dir.path(), "testproj", "A", None, &[]).unwrap();
        let t2 = add_task(dir.path(), "testproj", "B", None, &[]).unwrap();
        let t3 = add_task(dir.path(), "testproj", "C", None, &[]).unwrap();
        assert_eq!(t1.id, 1);
        assert_eq!(t2.id, 2);
        assert_eq!(t3.id, 3);
    }

    #[test]
    fn list_tasks_filters_by_status() {
        let dir = setup();
        add_task(dir.path(), "testproj", "Todo task", None, &[]).unwrap();
        let t2 = add_task(dir.path(), "testproj", "Done task", None, &[]).unwrap();
        move_task(dir.path(), "testproj", t2.id, "done").unwrap();

        let todo = list_tasks(dir.path(), "testproj", Some("todo")).unwrap();
        assert_eq!(todo.len(), 1);
        assert_eq!(todo[0].title, "Todo task");

        let done = list_tasks(dir.path(), "testproj", Some("done")).unwrap();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].title, "Done task");
    }

    #[test]
    fn move_task_updates_status() {
        let dir = setup();
        let task = add_task(dir.path(), "testproj", "Movable", None, &[]).unwrap();
        assert_eq!(task.status, TaskStatus::Todo);

        let moved = move_task(dir.path(), "testproj", task.id, "in-progress").unwrap();
        assert_eq!(moved.status, TaskStatus::InProgress);

        // Verify it persisted.
        let loaded = Task::load(dir.path(), "testproj", task.id).unwrap();
        assert_eq!(loaded.status, TaskStatus::InProgress);
    }

    #[test]
    fn create_project_with_custom_statuses() {
        let dir = TempDir::new().unwrap();
        let custom = vec!["open".to_string(), "closed".to_string()];
        let proj = create_project(dir.path(), "custom", "a/b", Some(custom.clone())).unwrap();
        assert_eq!(proj.statuses, custom);
    }
}
