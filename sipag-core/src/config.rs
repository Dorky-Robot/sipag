//! Runtime configuration for sipag workers.
//!
//! Resolution order: **env var > `~/.sipag/config` file > hardcoded default**.
//!
//! ```text
//! Field                   Env Var                      Config Key               Default
//! ─────────────────────── ──────────────────────────── ──────────────────────── ────────
//! batch_size              SIPAG_BATCH_SIZE             batch_size               1 (max 5)
//! poll_interval           SIPAG_POLL_INTERVAL          poll_interval            120s
//! work_label              SIPAG_WORK_LABEL             work_label               "ready"
//! image                   SIPAG_IMAGE                  image                    ghcr.io/dorky-robot/sipag-worker:latest
//! timeout                 SIPAG_TIMEOUT                timeout                  1800s
//! auto_merge              —                            auto_merge               false
//! doc_refresh_interval    SIPAG_DOC_REFRESH_INTERVAL   doc_refresh_interval     10
//! state_max_age_days      SIPAG_STATE_MAX_AGE_DAYS     state_max_age_days       7
//! once                    — (CLI --once flag only)     —                        false
//! sipag_dir               SIPAG_DIR                    —                        ~/.sipag
//! ```
//!
//! Credentials follow the same pattern — see [`Credentials`].

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{env, fs};

const BATCH_SIZE_MAX: usize = 5;

/// Default Docker image for worker containers.
pub const DEFAULT_IMAGE: &str = "ghcr.io/dorky-robot/sipag-worker:latest";

/// Runtime configuration for the sipag worker.
///
/// All fields follow the resolution order: env var > `~/.sipag/config` file > hardcoded default.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Base directory for sipag state (`~/.sipag` by default).
    pub sipag_dir: PathBuf,
    /// Maximum issues to group into a single worker container (`SIPAG_BATCH_SIZE`, capped at 5; default 1).
    /// When 1 (default), each issue gets its own container (legacy behavior).
    /// When > 1, multiple ready issues are dispatched to one container and Claude
    /// decides which to address together in a cohesive PR.
    pub batch_size: usize,
    /// Sleep duration between polling cycles (`SIPAG_POLL_INTERVAL` seconds; default 120).
    pub poll_interval: Duration,
    /// GitHub issue label that marks a task ready for dispatch (`SIPAG_WORK_LABEL`; default "ready").
    pub work_label: String,
    /// Docker image for worker containers (`SIPAG_IMAGE`).
    pub image: String,
    /// Per-container execution timeout (`SIPAG_TIMEOUT` seconds; default 1800).
    pub timeout: Duration,
    /// Stop after one polling cycle (set via `--once` flag; not loaded from env or file).
    pub once: bool,
    /// Automatically merge clean PRs (config file `auto_merge=true`; default false).
    pub auto_merge: bool,
    /// Polling cycles between documentation refresh runs (`SIPAG_DOC_REFRESH_INTERVAL`; default 10).
    pub doc_refresh_interval: u64,
    /// Age in days after which terminal state files are pruned on startup (`SIPAG_STATE_MAX_AGE_DAYS`; default 7).
    pub state_max_age_days: u64,
}

impl WorkerConfig {
    /// Load config from env vars, `~/.sipag/config` file, and hardcoded defaults.
    ///
    /// Resolution order: env var > config file > default.
    pub fn load(sipag_dir: &Path) -> Result<Self> {
        Self::load_with_env(sipag_dir, |k| env::var(k).ok())
    }

    fn load_with_env(sipag_dir: &Path, get_env: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let mut cfg = Self::defaults(sipag_dir);

        // 1. Apply config file overrides
        let config_file = sipag_dir.join("config");
        if config_file.exists() {
            parse_config_file(&config_file, |key, value| {
                cfg.apply_file_entry(key, value);
            })?;
        }

        // 2. Apply env var overrides (env wins over file)
        cfg.apply_env_overrides(get_env);

        Ok(cfg)
    }

    fn defaults(sipag_dir: &Path) -> Self {
        Self {
            sipag_dir: sipag_dir.to_path_buf(),
            batch_size: 1,
            poll_interval: Duration::from_secs(120),
            work_label: "ready".to_string(),
            image: DEFAULT_IMAGE.to_string(),
            timeout: Duration::from_secs(1800),
            once: false,
            auto_merge: false,
            doc_refresh_interval: 10,
            state_max_age_days: 7,
        }
    }

    fn apply_file_entry(&mut self, key: &str, value: &str) {
        match key {
            "batch_size" => {
                if let Ok(n) = value.parse::<usize>() {
                    self.batch_size = n.min(BATCH_SIZE_MAX);
                }
            }
            "image" => self.image = value.to_string(),
            "timeout" => {
                if let Ok(n) = value.parse::<u64>() {
                    self.timeout = Duration::from_secs(n);
                }
            }
            "poll_interval" => {
                if let Ok(n) = value.parse::<u64>() {
                    self.poll_interval = Duration::from_secs(n);
                }
            }
            "work_label" => self.work_label = value.to_string(),
            "auto_merge" => self.auto_merge = value == "true",
            "doc_refresh_interval" => {
                if let Ok(n) = value.parse::<u64>() {
                    self.doc_refresh_interval = n;
                }
            }
            "state_max_age_days" => {
                if let Ok(n) = value.parse::<u64>() {
                    self.state_max_age_days = n;
                }
            }
            _ => {}
        }
    }

    fn apply_env_overrides(&mut self, get_env: impl Fn(&str) -> Option<String>) {
        if let Some(v) = get_env("SIPAG_BATCH_SIZE") {
            if let Ok(n) = v.parse::<usize>() {
                self.batch_size = n.min(BATCH_SIZE_MAX);
            }
        }
        if let Some(v) = get_env("SIPAG_IMAGE") {
            self.image = v;
        }
        if let Some(v) = get_env("SIPAG_TIMEOUT") {
            if let Ok(n) = v.parse::<u64>() {
                self.timeout = Duration::from_secs(n);
            }
        }
        if let Some(v) = get_env("SIPAG_POLL_INTERVAL") {
            if let Ok(n) = v.parse::<u64>() {
                self.poll_interval = Duration::from_secs(n);
            }
        }
        if let Some(v) = get_env("SIPAG_WORK_LABEL") {
            self.work_label = v;
        }
        if let Some(v) = get_env("SIPAG_DOC_REFRESH_INTERVAL") {
            if let Ok(n) = v.parse::<u64>() {
                self.doc_refresh_interval = n;
            }
        }
        if let Some(v) = get_env("SIPAG_STATE_MAX_AGE_DAYS") {
            if let Ok(n) = v.parse::<u64>() {
                self.state_max_age_days = n;
            }
        }
    }
}

/// Credentials required by worker containers.
///
/// Resolution:
/// - `oauth_token`: `CLAUDE_CODE_OAUTH_TOKEN` env > `~/.sipag/token` file
/// - `api_key`: `ANTHROPIC_API_KEY` env
/// - `gh_token`: `GH_TOKEN` env > `gh auth token`
#[derive(Debug)]
pub struct Credentials {
    /// Claude OAuth token (primary authentication method).
    pub oauth_token: Option<String>,
    /// Anthropic API key (fallback when no OAuth token is available).
    pub api_key: Option<String>,
    /// GitHub token for API access.
    pub gh_token: String,
}

impl Credentials {
    /// Load credentials from environment variables and credential files.
    pub fn load(sipag_dir: &Path) -> Result<Self> {
        Self::load_with_env(sipag_dir, |k| env::var(k).ok())
    }

    fn load_with_env(sipag_dir: &Path, get_env: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let oauth_token = Self::resolve_oauth_token(sipag_dir, &get_env);
        let api_key = get_env("ANTHROPIC_API_KEY").filter(|s| !s.is_empty());
        let gh_token = Self::resolve_gh_token(&get_env)?;
        Ok(Self {
            oauth_token,
            api_key,
            gh_token,
        })
    }

    fn resolve_oauth_token(
        sipag_dir: &Path,
        get_env: &impl Fn(&str) -> Option<String>,
    ) -> Option<String> {
        if let Some(token) = get_env("CLAUDE_CODE_OAUTH_TOKEN") {
            if !token.is_empty() {
                return Some(token);
            }
        }
        let token_file = sipag_dir.join("token");
        if token_file.exists() {
            if let Ok(contents) = fs::read_to_string(&token_file) {
                let trimmed = contents.trim().to_string();
                if !trimmed.is_empty() {
                    return Some(trimmed);
                }
            }
        }
        None
    }

    fn resolve_gh_token(get_env: &impl Fn(&str) -> Option<String>) -> Result<String> {
        if let Some(token) = get_env("GH_TOKEN") {
            if !token.is_empty() {
                return Ok(token);
            }
        }
        // Fall back to `gh auth token`
        let output = std::process::Command::new("gh")
            .args(["auth", "token"])
            .output()
            .map_err(|e| anyhow::anyhow!("Failed to run `gh auth token`: {e}"))?;
        if !output.status.success() {
            anyhow::bail!("Failed to get GitHub token. Set GH_TOKEN or run `gh auth login`.");
        }
        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if token.is_empty() {
            anyhow::bail!("GH_TOKEN is empty. Set GH_TOKEN or run `gh auth login`.");
        }
        Ok(token)
    }
}

/// Parse a `key=value` config file, calling `f` for each entry.
///
/// Lines starting with `#` and empty lines are skipped.
fn parse_config_file(path: &Path, mut f: impl FnMut(&str, &str)) -> Result<()> {
    let content = fs::read_to_string(path)?;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            f(k.trim(), v.trim());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    // ── WorkerConfig tests ─────────────────────────────────────────────────

    #[test]
    fn worker_config_defaults() {
        let dir = TempDir::new().unwrap();
        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(cfg.batch_size, 1);
        assert_eq!(cfg.poll_interval, Duration::from_secs(120));
        assert_eq!(cfg.work_label, "ready");
        assert_eq!(cfg.image, DEFAULT_IMAGE);
        assert_eq!(cfg.timeout, Duration::from_secs(1800));
        assert!(!cfg.once);
        assert!(!cfg.auto_merge);
        assert_eq!(cfg.doc_refresh_interval, 10);
        assert_eq!(cfg.state_max_age_days, 7);
    }

    #[test]
    fn worker_config_file_override() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("config"),
            "batch_size=3\nimage=custom-image:v1\ntimeout=900\npoll_interval=60\nwork_label=ready\nauto_merge=true\ndoc_refresh_interval=5\nstate_max_age_days=3\n",
        )
        .unwrap();

        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(cfg.batch_size, 3);
        assert_eq!(cfg.image, "custom-image:v1");
        assert_eq!(cfg.timeout, Duration::from_secs(900));
        assert_eq!(cfg.poll_interval, Duration::from_secs(60));
        assert_eq!(cfg.work_label, "ready");
        assert!(cfg.auto_merge);
        assert_eq!(cfg.doc_refresh_interval, 5);
        assert_eq!(cfg.state_max_age_days, 3);
    }

    #[test]
    fn worker_config_env_overrides_file() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("config"),
            "image=file-image:latest\nbatch_size=2\n",
        )
        .unwrap();

        let cfg = WorkerConfig::load_with_env(dir.path(), |k| match k {
            "SIPAG_IMAGE" => Some("env-image:latest".to_string()),
            "SIPAG_BATCH_SIZE" => Some("4".to_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(cfg.image, "env-image:latest");
        assert_eq!(cfg.batch_size, 4);
    }

    #[test]
    fn worker_config_env_only() {
        let dir = TempDir::new().unwrap();
        let cfg = WorkerConfig::load_with_env(dir.path(), |k| match k {
            "SIPAG_WORK_LABEL" => Some("triaged".to_string()),
            "SIPAG_TIMEOUT" => Some("3600".to_string()),
            "SIPAG_POLL_INTERVAL" => Some("30".to_string()),
            "SIPAG_DOC_REFRESH_INTERVAL" => Some("20".to_string()),
            "SIPAG_STATE_MAX_AGE_DAYS" => Some("14".to_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(cfg.work_label, "triaged");
        assert_eq!(cfg.timeout, Duration::from_secs(3600));
        assert_eq!(cfg.poll_interval, Duration::from_secs(30));
        assert_eq!(cfg.doc_refresh_interval, 20);
        assert_eq!(cfg.state_max_age_days, 14);
    }

    #[test]
    fn worker_config_batch_size_clamped_from_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config"), "batch_size=10\n").unwrap();

        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(cfg.batch_size, BATCH_SIZE_MAX);
    }

    #[test]
    fn worker_config_batch_size_clamped_from_env() {
        let dir = TempDir::new().unwrap();
        let cfg = WorkerConfig::load_with_env(dir.path(), |k| {
            if k == "SIPAG_BATCH_SIZE" {
                Some("99".to_string())
            } else {
                None
            }
        })
        .unwrap();
        assert_eq!(cfg.batch_size, BATCH_SIZE_MAX);
    }

    #[test]
    fn worker_config_comments_and_blank_lines_ignored() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("config"),
            "# comment\n\n  # indented comment\nbatch_size=2\n",
        )
        .unwrap();

        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(cfg.batch_size, 2);
        assert_eq!(cfg.image, DEFAULT_IMAGE); // unchanged
    }

    #[test]
    fn worker_config_unknown_keys_ignored() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("config"),
            "unknown_key=some_value\nbatch_size=2\n",
        )
        .unwrap();

        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(cfg.batch_size, 2);
    }

    #[test]
    fn worker_config_missing_config_file_ok() {
        let dir = TempDir::new().unwrap();
        // No config file — should use defaults without error
        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(cfg.batch_size, 1);
    }

    #[test]
    fn worker_config_invalid_numeric_values_ignored() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("config"),
            "batch_size=not_a_number\ntimeout=also_bad\nbatch_size=2\n",
        )
        .unwrap();

        // The second valid batch_size=2 should win; invalid values are skipped
        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        // First bad parse is ignored, second entry sets it to 2
        assert_eq!(cfg.batch_size, 2);
        // timeout should still be the default since the only value was invalid
        assert_eq!(cfg.timeout, Duration::from_secs(1800));
    }

    #[test]
    fn worker_config_auto_merge_false_by_default() {
        let dir = TempDir::new().unwrap();
        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert!(!cfg.auto_merge);
    }

    #[test]
    fn worker_config_auto_merge_true_from_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config"), "auto_merge=true\n").unwrap();
        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert!(cfg.auto_merge);
    }

    #[test]
    fn worker_config_auto_merge_only_exact_true() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config"), "auto_merge=yes\n").unwrap();
        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert!(!cfg.auto_merge); // "yes" is not "true"
    }

    #[test]
    fn worker_config_sipag_dir_preserved() {
        let dir = TempDir::new().unwrap();
        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(cfg.sipag_dir, dir.path());
    }

    // ── Credentials tests ─────────────────────────────────────────────────

    #[test]
    fn credentials_oauth_from_env() {
        let dir = TempDir::new().unwrap();
        let creds = Credentials::load_with_env(dir.path(), |k| match k {
            "CLAUDE_CODE_OAUTH_TOKEN" => Some("env-oauth-token".to_string()),
            "GH_TOKEN" => Some("gh-token".to_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(creds.oauth_token, Some("env-oauth-token".to_string()));
    }

    #[test]
    fn credentials_oauth_from_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("token"), "file-oauth-token\n").unwrap();

        let creds = Credentials::load_with_env(dir.path(), |k| {
            if k == "GH_TOKEN" {
                Some("gh-token".to_string())
            } else {
                None
            }
        })
        .unwrap();
        assert_eq!(creds.oauth_token, Some("file-oauth-token".to_string()));
    }

    #[test]
    fn credentials_oauth_env_priority_over_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("token"), "file-token\n").unwrap();

        let creds = Credentials::load_with_env(dir.path(), |k| match k {
            "CLAUDE_CODE_OAUTH_TOKEN" => Some("env-token".to_string()),
            "GH_TOKEN" => Some("gh-token".to_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(creds.oauth_token, Some("env-token".to_string()));
    }

    #[test]
    fn credentials_oauth_none_when_absent() {
        let dir = TempDir::new().unwrap();
        let creds = Credentials::load_with_env(dir.path(), |k| {
            if k == "GH_TOKEN" {
                Some("gh-token".to_string())
            } else {
                None
            }
        })
        .unwrap();
        assert_eq!(creds.oauth_token, None);
    }

    #[test]
    fn credentials_oauth_empty_env_falls_through_to_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("token"), "file-token\n").unwrap();

        let creds = Credentials::load_with_env(dir.path(), |k| match k {
            "CLAUDE_CODE_OAUTH_TOKEN" => Some(String::new()), // empty string
            "GH_TOKEN" => Some("gh-token".to_string()),
            _ => None,
        })
        .unwrap();
        // Empty env value should fall through to file
        assert_eq!(creds.oauth_token, Some("file-token".to_string()));
    }

    #[test]
    fn credentials_api_key_from_env() {
        let dir = TempDir::new().unwrap();
        let creds = Credentials::load_with_env(dir.path(), |k| match k {
            "ANTHROPIC_API_KEY" => Some("sk-ant-key".to_string()),
            "GH_TOKEN" => Some("gh-token".to_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(creds.api_key, Some("sk-ant-key".to_string()));
    }

    #[test]
    fn credentials_api_key_none_when_absent() {
        let dir = TempDir::new().unwrap();
        let creds = Credentials::load_with_env(dir.path(), |k| {
            if k == "GH_TOKEN" {
                Some("gh-token".to_string())
            } else {
                None
            }
        })
        .unwrap();
        assert_eq!(creds.api_key, None);
    }

    #[test]
    fn credentials_gh_token_from_env() {
        let dir = TempDir::new().unwrap();
        let creds = Credentials::load_with_env(dir.path(), |k| {
            if k == "GH_TOKEN" {
                Some("my-gh-token".to_string())
            } else {
                None
            }
        })
        .unwrap();
        assert_eq!(creds.gh_token, "my-gh-token");
    }
}
