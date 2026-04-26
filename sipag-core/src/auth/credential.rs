//! Stored passkey credential metadata.
//!
//! sipag-core treats the passkey as an opaque JSON blob — the binary
//! crate (which depends on `webauthn-rs`) is responsible for
//! serializing the typed `Passkey` into JSON before save and parsing
//! it back on load. This keeps sipag-core dependency-light.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::credentials_dir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    /// Base64URL-encoded WebAuthn credential id. Filename = `{id}.json`.
    pub id: String,
    /// Human label, e.g. "felix-iphone". Optional; "" if user skipped.
    #[serde(default)]
    pub label: String,
    /// ISO8601 timestamp.
    pub created: String,
    /// ISO8601 timestamp of last successful assertion. Updated by the
    /// binary crate after each login.
    #[serde(default)]
    pub last_used: Option<String>,
    /// Opaque passkey blob produced by `webauthn_rs::prelude::Passkey`.
    /// sipag-core never inspects this value.
    pub passkey: serde_json::Value,
}

impl Credential {
    /// Sanitize the credential id for filesystem use. Passkey credential
    /// ids are base64url which only contains `[A-Za-z0-9_-]`; that's
    /// already filename-safe, but we strip everything else as belt+suspenders.
    fn safe_filename(id: &str) -> String {
        id.chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect()
    }

    fn file_path(sipag_dir: &Path, id: &str) -> PathBuf {
        credentials_dir(sipag_dir).join(format!("{}.json", Self::safe_filename(id)))
    }

    pub fn load(sipag_dir: &Path, id: &str) -> Result<Self> {
        let path = Self::file_path(sipag_dir, id);
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("credential {id} not found"))?;
        let cred: Self = serde_json::from_str(&content)
            .with_context(|| format!("invalid JSON in {}", path.display()))?;
        Ok(cred)
    }

    pub fn save(&self, sipag_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(credentials_dir(sipag_dir))?;
        let path = Self::file_path(sipag_dir, &self.id);
        let content = serde_json::to_string_pretty(self)?;
        crate::board::atomic_write(&path, content.as_bytes())
    }

    /// List all stored credentials, sorted by created.
    pub fn list(sipag_dir: &Path) -> Result<Vec<Self>> {
        let dir = credentials_dir(sipag_dir);
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let content = std::fs::read_to_string(&path)?;
            match serde_json::from_str::<Self>(&content) {
                Ok(c) => out.push(c),
                Err(e) => log::warn!("skipping malformed credential {}: {}", path.display(), e),
            }
        }
        out.sort_by(|a, b| a.created.cmp(&b.created));
        Ok(out)
    }

    pub fn delete(sipag_dir: &Path, id: &str) -> Result<()> {
        let path = Self::file_path(sipag_dir, id);
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
    use tempfile::TempDir;

    #[test]
    fn save_load_round_trip() {
        let tmp = TempDir::new().unwrap();
        let cred = Credential {
            id: "abc-123_def".into(),
            label: "test".into(),
            created: "2026-04-26T00:00:00Z".into(),
            last_used: None,
            passkey: serde_json::json!({ "stub": true }),
        };
        cred.save(tmp.path()).unwrap();
        let loaded = Credential::load(tmp.path(), "abc-123_def").unwrap();
        assert_eq!(loaded.id, "abc-123_def");
        assert_eq!(loaded.label, "test");
        assert_eq!(loaded.passkey["stub"], serde_json::Value::Bool(true));
    }

    #[test]
    fn list_sorts_by_created() {
        let tmp = TempDir::new().unwrap();
        for (id, ts) in [("c", "2026-04-28T00:00:00Z"),
                         ("a", "2026-04-26T00:00:00Z"),
                         ("b", "2026-04-27T00:00:00Z")] {
            Credential {
                id: id.into(),
                label: "".into(),
                created: ts.into(),
                last_used: None,
                passkey: serde_json::json!({}),
            }
            .save(tmp.path())
            .unwrap();
        }
        let ids: Vec<_> = Credential::list(tmp.path())
            .unwrap()
            .iter()
            .map(|c| c.id.clone())
            .collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn delete_idempotent() {
        let tmp = TempDir::new().unwrap();
        Credential::delete(tmp.path(), "missing").unwrap();
    }

    #[test]
    fn safe_filename_strips_dangerous_chars() {
        assert_eq!(Credential::safe_filename("a/b"), "ab");
        assert_eq!(Credential::safe_filename("a..b"), "ab");
        assert_eq!(Credential::safe_filename("ok-id_42"), "ok-id_42");
    }
}
