//! Active session cookie store.
//!
//! When a user completes WebAuthn assertion, the binary crate mints a
//! session token, writes a Session record, and sets a cookie with
//! that token. On every authenticated request, the middleware loads
//! the session by its token, checks expiry, and (if valid) refreshes
//! `last_active` for sliding expiry.
//!
//! One TOML file per session — easy to rm, easy to inspect.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::sessions_dir;

/// Default session lifetime — sliding expiry of 30 days.
pub const DEFAULT_SESSION_TTL_DAYS: i64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub token: String,
    /// Which credential authenticated this session.
    pub credential_id: String,
    pub created: String,
    pub last_active: String,
    pub expires: String,
}

impl Session {
    fn safe_filename(token: &str) -> String {
        token
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect()
    }

    fn file_path(sipag_dir: &Path, token: &str) -> PathBuf {
        sessions_dir(sipag_dir).join(format!("{}.toml", Self::safe_filename(token)))
    }

    pub fn new(token: String, credential_id: String) -> Self {
        let now = Utc::now();
        let expires = now + Duration::days(DEFAULT_SESSION_TTL_DAYS);
        Self {
            token,
            credential_id,
            created: now.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            last_active: now.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            expires: expires.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        }
    }

    pub fn save(&self, sipag_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(sessions_dir(sipag_dir))?;
        let path = Self::file_path(sipag_dir, &self.token);
        let content = toml::to_string_pretty(self)?;
        crate::board::atomic_write(&path, content.as_bytes())
    }

    pub fn load(sipag_dir: &Path, token: &str) -> Result<Self> {
        let path = Self::file_path(sipag_dir, token);
        let content = std::fs::read_to_string(&path)
            .with_context(|| "session not found".to_string())?;
        let session: Self = toml::from_str(&content)
            .with_context(|| format!("invalid TOML in {}", path.display()))?;
        Ok(session)
    }

    pub fn delete(sipag_dir: &Path, token: &str) -> Result<()> {
        let path = Self::file_path(sipag_dir, token);
        if !path.exists() {
            return Ok(());
        }
        std::fs::remove_file(&path)
            .with_context(|| format!("failed to remove {}", path.display()))?;
        Ok(())
    }

    /// True if `expires` is in the past.
    pub fn is_expired(&self) -> bool {
        DateTime::parse_from_rfc3339(&self.expires)
            .map(|t| t.with_timezone(&Utc) < Utc::now())
            .unwrap_or(true)
    }

    /// Bump `last_active` to now and slide `expires` forward.
    pub fn touch(&mut self) {
        let now = Utc::now();
        self.last_active = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();
        self.expires = (now + Duration::days(DEFAULT_SESSION_TTL_DAYS))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn save_load_round_trip() {
        let tmp = TempDir::new().unwrap();
        let s = Session::new("tok-abc".into(), "cred-1".into());
        s.save(tmp.path()).unwrap();
        let loaded = Session::load(tmp.path(), "tok-abc").unwrap();
        assert_eq!(loaded.token, "tok-abc");
        assert_eq!(loaded.credential_id, "cred-1");
        assert!(!loaded.is_expired());
    }

    #[test]
    fn delete_idempotent() {
        let tmp = TempDir::new().unwrap();
        Session::delete(tmp.path(), "no-such-token").unwrap();
    }

    #[test]
    fn touch_slides_expiry() {
        let mut s = Session::new("t".into(), "c".into());
        let original_expires = s.expires.clone();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        s.touch();
        assert_ne!(s.expires, original_expires);
    }
}
