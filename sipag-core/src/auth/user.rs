//! Single-user record for sipag.
//!
//! WebAuthn requires a stable `user_handle` (UUID) so re-registrations
//! land on the same user. We persist exactly one of these per sipag
//! instance — sipag is single-user by design.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config::default_sipag_dir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub display_name: String,
}

impl User {
    pub fn path() -> PathBuf {
        default_sipag_dir().join("user.json")
    }

    /// Load the user record, or create it if missing.
    /// `display_name` is only used on first creation.
    pub fn load_or_create(display_name: impl Into<String>) -> Result<Self> {
        let path = Self::path();
        if path.exists() {
            return Self::load_from(&path);
        }
        let user = Self {
            id: Uuid::new_v4(),
            display_name: display_name.into(),
        };
        user.save_to(&path)?;
        Ok(user)
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let user: Self = serde_json::from_str(&content)
            .with_context(|| format!("invalid JSON in {}", path.display()))?;
        Ok(user)
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        crate::board::atomic_write(path, content.as_bytes())
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_or_create_round_trip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("user.json");
        let user = User {
            id: Uuid::new_v4(),
            display_name: "Sipag Owner".into(),
        };
        user.save_to(&path).unwrap();
        let loaded = User::load_from(&path).unwrap();
        assert_eq!(loaded.id, user.id);
        assert_eq!(loaded.display_name, "Sipag Owner");
    }
}
