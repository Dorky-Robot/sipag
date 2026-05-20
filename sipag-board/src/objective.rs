//! Objective data model — the asymptotic, aspirational thing we're
//! optimizing for. Lives ABOVE projects; projects are the current
//! best means of approaching an objective. When the means change
//! (donkeys → trucks), the project gets retired but the objective
//! endures.
//!
//! Stored at `~/.sipag/objectives/{id}/objective.toml`, with KRs at
//! `~/.sipag/objectives/{id}/key-results/{NNN}.toml`.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::atomic_write;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Objective {
    /// Stable filesystem id (slug). Used as the directory name and as
    /// the cross-reference target from `Project.serves` and
    /// `Observation.kr_refs`.
    pub id: String,
    /// Short codename for terse references in tooling. Often matches
    /// `id` initially but can drift.
    #[serde(default)]
    pub name: String,
    /// The asymptotic sentence — the place we never finish reaching.
    /// No metric, no end date, no deliverable. Just the direction.
    pub aspiration: String,
    pub created: String,
    /// Free-form labels.
    #[serde(default)]
    pub labels: Vec<String>,
}

impl Objective {
    fn dir(sipag_dir: &Path, id: &str) -> PathBuf {
        sipag_dir.join("objectives").join(id)
    }

    pub fn path(sipag_dir: &Path, id: &str) -> PathBuf {
        Self::dir(sipag_dir, id).join("objective.toml")
    }

    pub fn load(sipag_dir: &Path, id: &str) -> Result<Self> {
        let path = Self::path(sipag_dir, id);
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("objective '{id}' not found at {}", path.display()))?;
        let obj: Self = toml::from_str(&content)
            .with_context(|| format!("invalid TOML in {}", path.display()))?;
        Ok(obj)
    }

    pub fn save(&self, sipag_dir: &Path) -> Result<()> {
        let dir = Self::dir(sipag_dir, &self.id);
        std::fs::create_dir_all(dir.join("key-results"))
            .with_context(|| format!("create dir {}", dir.display()))?;
        let path = dir.join("objective.toml");
        let content = toml::to_string_pretty(self).context("serialize objective")?;
        atomic_write(&path, content.as_bytes())
    }

    pub fn list(sipag_dir: &Path) -> Result<Vec<Self>> {
        let dir = sipag_dir.join("objectives");
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir)
            .with_context(|| format!("read objectives dir {}", dir.display()))?
            .flatten()
        {
            if !entry.path().is_dir() {
                continue;
            }
            let name = match entry.file_name().to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            if let Ok(obj) = Self::load(sipag_dir, &name) {
                out.push(obj);
            }
        }
        out.sort_by(|a, b| a.created.cmp(&b.created));
        Ok(out)
    }
}

/// Reference to a Key Result owned by a specific Objective. Used by
/// Tasks (work units) and Observations (live activity) to indicate
/// which KRs they advance — possibly across multiple objectives.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KrRef {
    pub objective: String,
    pub kr: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn round_trip_objective() {
        let dir = TempDir::new().unwrap();
        let obj = Objective {
            id: "device-flow".into(),
            name: "device-flow".into(),
            aspiration: "Going from idea to running across every device feels effortless".into(),
            created: "2026-05-05T00:00:00Z".into(),
            labels: vec!["north-star".into()],
        };
        obj.save(dir.path()).unwrap();
        let loaded = Objective::load(dir.path(), "device-flow").unwrap();
        assert_eq!(loaded.id, obj.id);
        assert_eq!(loaded.aspiration, obj.aspiration);
        assert_eq!(loaded.labels, obj.labels);
    }

    #[test]
    fn list_returns_all_objectives() {
        let dir = TempDir::new().unwrap();
        for (id, asp) in [
            ("device-flow", "Going from idea to running…"),
            ("files-durable", "Files outlive any one tool…"),
        ] {
            Objective {
                id: id.into(),
                name: id.into(),
                aspiration: asp.into(),
                created: "2026-05-05T00:00:00Z".into(),
                labels: vec![],
            }
            .save(dir.path())
            .unwrap();
        }
        let all = Objective::list(dir.path()).unwrap();
        assert_eq!(all.len(), 2);
    }
}
