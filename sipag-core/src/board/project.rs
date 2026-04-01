//! Project data model — stored at `~/.sipag/projects/{name}/project.toml`.

use anyhow::{Context, Result};
use std::path::Path;

use super::atomic_write;

/// A project on the board.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Project {
    pub name: String,
    pub repo: String,
    #[serde(default = "default_statuses")]
    pub statuses: Vec<String>,
}

fn default_statuses() -> Vec<String> {
    vec![
        "backlog".to_string(),
        "todo".to_string(),
        "in-progress".to_string(),
        "review".to_string(),
        "done".to_string(),
    ]
}

impl Project {
    /// Load a project by name.
    pub fn load(sipag_dir: &Path, name: &str) -> Result<Self> {
        let path = sipag_dir.join("projects").join(name).join("project.toml");
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("project '{name}' not found"))?;
        let project: Self = toml::from_str(&content)
            .with_context(|| format!("invalid TOML in {}", path.display()))?;
        Ok(project)
    }

    /// Save this project to disk.
    pub fn save(&self, sipag_dir: &Path) -> Result<()> {
        let project_dir = sipag_dir.join("projects").join(&self.name);
        std::fs::create_dir_all(project_dir.join("tasks"))?;
        std::fs::create_dir_all(project_dir.join("roles"))?;

        let path = project_dir.join("project.toml");
        let content = toml::to_string_pretty(self)?;
        atomic_write(&path, content.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn project_round_trip() {
        let dir = TempDir::new().unwrap();
        let project = Project {
            name: "katulong".to_string(),
            repo: "dorky-robot/katulong".to_string(),
            statuses: default_statuses(),
        };
        project.save(dir.path()).unwrap();

        let loaded = Project::load(dir.path(), "katulong").unwrap();
        assert_eq!(loaded.name, "katulong");
        assert_eq!(loaded.repo, "dorky-robot/katulong");
        assert_eq!(loaded.statuses.len(), 5);
    }

    #[test]
    fn project_save_creates_directories() {
        let dir = TempDir::new().unwrap();
        let project = Project {
            name: "newproj".to_string(),
            repo: "a/b".to_string(),
            statuses: vec!["open".to_string(), "closed".to_string()],
        };
        project.save(dir.path()).unwrap();

        assert!(dir.path().join("projects/newproj/tasks").is_dir());
        assert!(dir.path().join("projects/newproj/roles").is_dir());
        assert!(dir.path().join("projects/newproj/project.toml").exists());
    }

    #[test]
    fn project_load_nonexistent_fails() {
        let dir = TempDir::new().unwrap();
        assert!(Project::load(dir.path(), "nope").is_err());
    }
}
