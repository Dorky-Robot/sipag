//! Key Result data model — one TOML file per KR under a project.
//!
//! Stored at `~/.sipag/projects/{project}/key-results/{NNN}.toml`.
//!
//! KRs sit between `Project` (= the OKR objective, in OKR speak) and
//! `Task`. They are deliberately minimal: a free-text title plus a
//! traffic-light stance. No target/current numbers, no scoring math.
//! The user said: hypothesis-shaped is fine, force-numeric is theatre.

use anyhow::{Context, Result};
use std::fmt;
use std::path::{Path, PathBuf};

use super::atomic_write;

/// Traffic light for a key result.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum KrStance {
    #[default]
    Green,
    Yellow,
    Red,
    Done,
}

impl KrStance {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "green" => Some(Self::Green),
            "yellow" => Some(Self::Yellow),
            "red" => Some(Self::Red),
            "done" => Some(Self::Done),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Green => "green",
            Self::Yellow => "yellow",
            Self::Red => "red",
            Self::Done => "done",
        }
    }
}

impl fmt::Display for KrStance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single key result under a project.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct KeyResult {
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub stance: KrStance,
    pub created: String,
}

impl KeyResult {
    fn dir(sipag_dir: &Path, project: &str) -> PathBuf {
        sipag_dir.join("projects").join(project).join("key-results")
    }

    fn file_path(sipag_dir: &Path, project: &str, id: u64) -> PathBuf {
        Self::dir(sipag_dir, project).join(format!("{:03}.toml", id))
    }

    pub fn load(sipag_dir: &Path, project: &str, id: u64) -> Result<Self> {
        let path = Self::file_path(sipag_dir, project, id);
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("KR #{id} not found in project {project}"))?;
        let kr: Self = toml::from_str(&content)
            .with_context(|| format!("invalid TOML in {}", path.display()))?;
        Ok(kr)
    }

    pub fn save(&self, sipag_dir: &Path, project: &str) -> Result<()> {
        let dir = Self::dir(sipag_dir, project);
        std::fs::create_dir_all(&dir)?;
        let path = Self::file_path(sipag_dir, project, self.id);
        let content = toml::to_string_pretty(self)?;
        atomic_write(&path, content.as_bytes())
    }

    /// All KRs for a project, sorted by id.
    pub fn list(sipag_dir: &Path, project: &str) -> Result<Vec<Self>> {
        let dir = Self::dir(sipag_dir, project);
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let content = std::fs::read_to_string(&path)?;
            match toml::from_str::<Self>(&content) {
                Ok(kr) => out.push(kr),
                Err(e) => log::warn!("skipping malformed KR file {}: {}", path.display(), e),
            }
        }
        out.sort_by_key(|k| k.id);
        Ok(out)
    }

    /// Next free id for a new KR in this project.
    pub fn next_id(sipag_dir: &Path, project: &str) -> Result<u64> {
        let existing = Self::list(sipag_dir, project)?;
        Ok(existing.iter().map(|k| k.id).max().map(|m| m + 1).unwrap_or(1))
    }

    /// Delete a KR file. Returns Ok(()) when it's already gone.
    pub fn delete(sipag_dir: &Path, project: &str, id: u64) -> Result<()> {
        let path = Self::file_path(sipag_dir, project, id);
        if !path.exists() {
            return Ok(());
        }
        std::fs::remove_file(&path)
            .with_context(|| format!("failed to remove {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{create_project, projects_dir};
    use tempfile::TempDir;

    fn setup() -> TempDir {
        let dir = TempDir::new().unwrap();
        create_project(dir.path(), "test", "owner/repo", None).unwrap();
        // touch projects dir to make sure it exists
        std::fs::create_dir_all(projects_dir(dir.path())).unwrap();
        dir
    }

    #[test]
    fn stance_parses_and_displays() {
        assert_eq!(KrStance::parse("green"), Some(KrStance::Green));
        assert_eq!(KrStance::parse("done"), Some(KrStance::Done));
        assert_eq!(KrStance::parse("blue"), None);
        assert_eq!(KrStance::Yellow.to_string(), "yellow");
    }

    #[test]
    fn stance_defaults_to_green_when_missing() {
        let dir = setup();
        let path = dir.path().join("projects/test/key-results/001.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "id = 1\ntitle = \"legacy\"\ncreated = \"2026-01-01T00:00:00Z\"\n",
        )
        .unwrap();
        let kr = KeyResult::load(dir.path(), "test", 1).unwrap();
        assert_eq!(kr.stance, KrStance::Green);
    }

    #[test]
    fn round_trip() {
        let dir = setup();
        let kr = KeyResult {
            id: 1,
            title: "Cmd+/ launches sipag fast enough to feel native".to_string(),
            stance: KrStance::Yellow,
            created: "2026-04-25T00:00:00Z".to_string(),
        };
        kr.save(dir.path(), "test").unwrap();

        let loaded = KeyResult::load(dir.path(), "test", 1).unwrap();
        assert_eq!(loaded.id, 1);
        assert_eq!(loaded.stance, KrStance::Yellow);
        assert_eq!(
            loaded.title,
            "Cmd+/ launches sipag fast enough to feel native"
        );
    }

    #[test]
    fn list_sorts_and_handles_empty() {
        let dir = setup();
        assert!(KeyResult::list(dir.path(), "test").unwrap().is_empty());

        for (id, title) in [(2u64, "second"), (1, "first"), (3, "third")] {
            KeyResult {
                id,
                title: title.to_string(),
                stance: KrStance::Green,
                created: "2026-04-25T00:00:00Z".to_string(),
            }
            .save(dir.path(), "test")
            .unwrap();
        }

        let list = KeyResult::list(dir.path(), "test").unwrap();
        let ids: Vec<u64> = list.iter().map(|k| k.id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn next_id_starts_at_one_then_increments() {
        let dir = setup();
        assert_eq!(KeyResult::next_id(dir.path(), "test").unwrap(), 1);

        KeyResult {
            id: 1,
            title: "first".to_string(),
            stance: KrStance::Green,
            created: "2026-04-25T00:00:00Z".to_string(),
        }
        .save(dir.path(), "test")
        .unwrap();
        assert_eq!(KeyResult::next_id(dir.path(), "test").unwrap(), 2);
    }
}
