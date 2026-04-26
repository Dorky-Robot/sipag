//! Setup tokens — short-lived single-use bootstrap secrets.
//!
//! Used twice in this codebase:
//!   1. **First-passkey enrollment**: `sipag setup-token` mints one,
//!      the user opens the printed URL on a trusted device,
//!      registers a passkey, the token is consumed.
//!   2. **Katulong-app install handshake**: each side mints a setup
//!      token to prove human intent during the cross-origin install
//!      flow (see docs/katulong-app-protocol.md).
//!
//! Tokens default to a 10-minute TTL and are single-use. Consumed
//! tokens are removed from disk immediately so a stolen file can't
//! replay an already-used token.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::setup_tokens_dir;

/// Purpose lets one consumer reject tokens minted for another flow,
/// even though they share the same store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SetupPurpose {
    /// `sipag setup-token` — first-passkey enrollment.
    EnrollPasskey,
    /// Katulong-app install handshake (B.1 in the protocol spec).
    KatulongAppInstall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupToken {
    pub token: String,
    pub purpose: SetupPurpose,
    pub created: String,
    pub expires: String,
}

impl SetupToken {
    fn safe_filename(token: &str) -> String {
        token
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect()
    }

    fn file_path(sipag_dir: &Path, token: &str) -> PathBuf {
        setup_tokens_dir(sipag_dir).join(format!("{}.toml", Self::safe_filename(token)))
    }

    pub fn new(token: String, purpose: SetupPurpose, ttl_minutes: i64) -> Self {
        let now = Utc::now();
        let expires = now + Duration::minutes(ttl_minutes);
        Self {
            token,
            purpose,
            created: now.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            expires: expires.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        }
    }

    pub fn save(&self, sipag_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(setup_tokens_dir(sipag_dir))?;
        let path = Self::file_path(sipag_dir, &self.token);
        let content = toml::to_string_pretty(self)?;
        crate::board::atomic_write(&path, content.as_bytes())
    }

    pub fn load(sipag_dir: &Path, token: &str) -> Result<Self> {
        let path = Self::file_path(sipag_dir, token);
        let content = std::fs::read_to_string(&path)
            .with_context(|| "setup token not found".to_string())?;
        let tok: Self = toml::from_str(&content)
            .with_context(|| format!("invalid TOML in {}", path.display()))?;
        Ok(tok)
    }

    /// Atomically consume a token: load it, verify purpose + freshness,
    /// remove it from disk, return it. Returns Err if the token is
    /// missing, expired, or for a different purpose.
    pub fn consume(
        sipag_dir: &Path,
        token: &str,
        expected: SetupPurpose,
    ) -> Result<Self> {
        let tok = Self::load(sipag_dir, token)?;
        if tok.purpose != expected {
            anyhow::bail!("setup token purpose mismatch");
        }
        if tok.is_expired() {
            // Best-effort cleanup of the expired record.
            let _ = std::fs::remove_file(Self::file_path(sipag_dir, token));
            anyhow::bail!("setup token expired");
        }
        // Remove first so a panic later doesn't leave a replayable token.
        std::fs::remove_file(Self::file_path(sipag_dir, token))
            .with_context(|| "failed to remove consumed setup token".to_string())?;
        Ok(tok)
    }

    pub fn is_expired(&self) -> bool {
        DateTime::parse_from_rfc3339(&self.expires)
            .map(|t| t.with_timezone(&Utc) < Utc::now())
            .unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn save_load_consume_cycle() {
        let tmp = TempDir::new().unwrap();
        let t = SetupToken::new("tk-abc".into(), SetupPurpose::EnrollPasskey, 10);
        t.save(tmp.path()).unwrap();

        let consumed =
            SetupToken::consume(tmp.path(), "tk-abc", SetupPurpose::EnrollPasskey).unwrap();
        assert_eq!(consumed.token, "tk-abc");

        // Second consume should fail (token no longer on disk).
        let err = SetupToken::consume(tmp.path(), "tk-abc", SetupPurpose::EnrollPasskey);
        assert!(err.is_err());
    }

    #[test]
    fn purpose_mismatch_rejects() {
        let tmp = TempDir::new().unwrap();
        let t = SetupToken::new("p1".into(), SetupPurpose::EnrollPasskey, 10);
        t.save(tmp.path()).unwrap();
        let err =
            SetupToken::consume(tmp.path(), "p1", SetupPurpose::KatulongAppInstall);
        assert!(err.is_err());
    }

    #[test]
    fn expired_rejects_and_cleans() {
        let tmp = TempDir::new().unwrap();
        let t = SetupToken::new("expired".into(), SetupPurpose::EnrollPasskey, -5);
        t.save(tmp.path()).unwrap();
        assert!(t.is_expired());
        let err = SetupToken::consume(tmp.path(), "expired", SetupPurpose::EnrollPasskey);
        assert!(err.is_err());
        // File should have been cleaned up by the consume call.
        assert!(SetupToken::load(tmp.path(), "expired").is_err());
    }
}
