//! Role template — stored at `~/.sipag/projects/{project}/roles/{name}.toml`.
//!
//! A Role is the "how to launch this kind of agent session" recipe a
//! Task points at via its `role: String` field. Three fields today:
//!
//! - `name` — stable identifier (`dev`, `reviewer`, `ci-fixer`).
//! - `command` — the launch keystroke sipag types into the katulong
//!   pane (e.g. `claude`, `claude --resume`). Default `yolo`.
//! - `worktree` — when `true`, sipag runs `git worktree add` against
//!   the project repo before launching, so the role works on its own
//!   branch.
//!
//! **Removed 2026-05-25** in the dead-field trim: `type` /
//! `container` / `memory_context` were Docker-era categorization
//! fields from sipag's pre-katulong dispatch model. None of the
//! sipag-binary code read them after the v2/v3 Docker dispatch path
//! was deleted (April 2026). The struct does NOT use
//! `#[serde(deny_unknown_fields)]`, so operator TOMLs that still
//! carry the old keys continue to load — the keys are silently
//! ignored. A back-compat test below pins that contract.

use anyhow::{Context, Result};
use std::path::Path;

use super::atomic_write;

/// A role template defining how a session type behaves.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Role {
    pub name: String,
    #[serde(default)]
    pub worktree: bool,
    #[serde(default = "default_command")]
    pub command: String,
}

fn default_command() -> String {
    "yolo".to_string()
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
            worktree: true,
            command: "claude".to_string(),
        };
        role.save(dir.path(), "test").unwrap();

        let loaded = Role::load(dir.path(), "test", "dev").unwrap();
        assert_eq!(loaded.name, "dev");
        assert!(loaded.worktree);
        assert_eq!(loaded.command, "claude");
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
            worktree: true,
            command: "claude".to_string(),
        };
        let test_role = Role {
            name: "test".to_string(),
            worktree: false,
            command: "claude -p test".to_string(),
        };
        dev.save(dir.path(), "test").unwrap();
        test_role.save(dir.path(), "test").unwrap();

        let roles = Role::list(dir.path(), "test").unwrap();
        assert_eq!(roles.len(), 2);
        assert_eq!(roles[0].name, "dev");
        assert_eq!(roles[1].name, "test");
    }

    #[test]
    fn role_load_ignores_removed_legacy_docker_fields() {
        // Operator TOMLs from the pre-katulong dispatch model may
        // still have `type`, `container`, and `memory_context` keys.
        // The struct doesn't set `deny_unknown_fields`, so these
        // load fine with the legacy keys silently dropped. Pinning
        // this contract so a future `deny_unknown_fields` addition
        // would surface as a test failure rather than silently
        // breaking every existing operator's role files.
        let dir = TempDir::new().unwrap();
        let roles_dir = dir.path().join("projects").join("test").join("roles");
        std::fs::create_dir_all(&roles_dir).unwrap();
        let legacy_toml = r#"
name = "dev"
type = "kubo"
container = "katulong"
worktree = true
command = "claude"
memory_context = "shared"
"#;
        std::fs::write(roles_dir.join("dev.toml"), legacy_toml).unwrap();

        let loaded = Role::load(dir.path(), "test", "dev").unwrap();
        assert_eq!(loaded.name, "dev");
        assert_eq!(loaded.command, "claude");
        assert!(loaded.worktree);
    }
}
