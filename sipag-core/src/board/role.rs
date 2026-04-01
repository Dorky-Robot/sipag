//! Role template — stored at `~/.sipag/projects/{project}/roles/{name}.toml`.

use anyhow::{Context, Result};
use std::path::Path;

use super::atomic_write;

/// A role template defining how a session type behaves.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Role {
    pub name: String,
    /// "kubo", "host", or "local".
    #[serde(rename = "type")]
    pub role_type: String,
    #[serde(default)]
    pub container: Option<String>,
    #[serde(default)]
    pub worktree: bool,
    #[serde(default = "default_command")]
    pub command: String,
    #[serde(default = "default_memory_context")]
    pub memory_context: String,
}

fn default_command() -> String {
    "yolo".to_string()
}

fn default_memory_context() -> String {
    "shared".to_string()
}

impl Role {
    /// Load a role by name.
    pub fn load(sipag_dir: &Path, project: &str, name: &str) -> Result<Self> {
        let path = sipag_dir
            .join("projects")
            .join(project)
            .join("roles")
            .join(format!("{name}.toml"));
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("role '{name}' not found in project {project}"))?;
        let role: Self = toml::from_str(&content)
            .with_context(|| format!("invalid TOML in {}", path.display()))?;
        Ok(role)
    }

    /// Save this role to disk.
    pub fn save(&self, sipag_dir: &Path, project: &str) -> Result<()> {
        let path = sipag_dir
            .join("projects")
            .join(project)
            .join("roles")
            .join(format!("{}.toml", self.name));
        let content = toml::to_string_pretty(self)?;
        atomic_write(&path, content.as_bytes())
    }

    /// List all roles for a project.
    pub fn list(sipag_dir: &Path, project: &str) -> Result<Vec<Self>> {
        let dir = sipag_dir.join("projects").join(project).join("roles");
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut roles = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                let content = std::fs::read_to_string(&path)?;
                match toml::from_str::<Self>(&content) {
                    Ok(role) => roles.push(role),
                    Err(e) => {
                        log::warn!("skipping {}: {e}", path.display());
                    }
                }
            }
        }
        roles.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(roles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_project(dir: &Path) {
        let roles_dir = dir.join("projects").join("test").join("roles");
        std::fs::create_dir_all(&roles_dir).unwrap();
    }

    #[test]
    fn role_round_trip() {
        let dir = TempDir::new().unwrap();
        setup_project(dir.path());

        let role = Role {
            name: "dev".to_string(),
            role_type: "kubo".to_string(),
            container: Some("katulong".to_string()),
            worktree: true,
            command: "yolo".to_string(),
            memory_context: "shared".to_string(),
        };
        role.save(dir.path(), "test").unwrap();

        let loaded = Role::load(dir.path(), "test", "dev").unwrap();
        assert_eq!(loaded.name, "dev");
        assert_eq!(loaded.role_type, "kubo");
        assert_eq!(loaded.container.as_deref(), Some("katulong"));
        assert!(loaded.worktree);
    }

    #[test]
    fn role_list_empty() {
        let dir = TempDir::new().unwrap();
        setup_project(dir.path());
        let roles = Role::list(dir.path(), "test").unwrap();
        assert!(roles.is_empty());
    }

    #[test]
    fn role_list_finds_roles() {
        let dir = TempDir::new().unwrap();
        setup_project(dir.path());

        let dev = Role {
            name: "dev".to_string(),
            role_type: "kubo".to_string(),
            container: None,
            worktree: true,
            command: "yolo".to_string(),
            memory_context: "shared".to_string(),
        };
        let test_role = Role {
            name: "test".to_string(),
            role_type: "host".to_string(),
            container: None,
            worktree: false,
            command: "yolo -p test".to_string(),
            memory_context: "shared".to_string(),
        };
        dev.save(dir.path(), "test").unwrap();
        test_role.save(dir.path(), "test").unwrap();

        let roles = Role::list(dir.path(), "test").unwrap();
        assert_eq!(roles.len(), 2);
        assert_eq!(roles[0].name, "dev");
        assert_eq!(roles[1].name, "test");
    }
}
