//! Runtime configuration for sipag.
//!
//! Resolution order: **env var > `~/.sipag/config` file > hardcoded default**.
//!
//! ```text
//! Field          Env Var             Config Key      Default
//! ────────────── ─────────────────── ─────────────── ─────────────────────────────────────
//! sipag_dir      SIPAG_DIR           —               ~/.sipag
//! image          SIPAG_IMAGE         image           ghcr.io/dorky-robot/sipag-worker:latest
//! timeout        SIPAG_TIMEOUT       timeout         7200s
//! work_label     SIPAG_WORK_LABEL    work_label      "ready"
//! max_open_prs   SIPAG_MAX_OPEN_PRS  max_open_prs    3 (0 = disabled)
//! poll_interval  SIPAG_POLL_INTERVAL poll_interval   120s
//! ```

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::{env, fs};

const TIMEOUT_MIN_SECS: u64 = 1;

/// Default Docker image for worker containers.
pub const DEFAULT_IMAGE: &str = "ghcr.io/dorky-robot/sipag-worker:latest";

/// All known keys in the `~/.sipag/config` file.
const KNOWN_KEYS: &[&str] = &[
    "image",
    "timeout",
    "work_label",
    "max_open_prs",
    "poll_interval",
];

/// Runtime configuration for sipag.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Base directory for sipag state (`~/.sipag` by default).
    pub sipag_dir: PathBuf,
    /// Docker image for worker containers.
    pub image: String,
    /// Per-container execution timeout in seconds (default 7200).
    pub timeout: u64,
    /// GitHub issue label that marks a task ready for dispatch (default "ready").
    pub work_label: String,
    /// Maximum open sipag PRs before dispatch is paused (default 3; 0 = disabled).
    pub max_open_prs: usize,
    /// Seconds between polling cycles (default 120).
    pub poll_interval: u64,
}

impl WorkerConfig {
    /// Load config from env vars, `~/.sipag/config` file, and hardcoded defaults.
    pub fn load(sipag_dir: &Path) -> Result<Self> {
        let (cfg, warnings) = Self::load_with_env_inner(sipag_dir, |k| env::var(k).ok())?;
        for w in &warnings {
            eprintln!("sipag warning: {w}");
        }
        Ok(cfg)
    }

    #[cfg(test)]
    fn load_with_env(sipag_dir: &Path, get_env: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let (cfg, _warnings) = Self::load_with_env_inner(sipag_dir, get_env)?;
        Ok(cfg)
    }

    fn load_with_env_inner(
        sipag_dir: &Path,
        get_env: impl Fn(&str) -> Option<String>,
    ) -> Result<(Self, Vec<String>)> {
        let mut cfg = Self::defaults(sipag_dir);
        let mut warnings: Vec<String> = Vec::new();

        let config_file = sipag_dir.join("config");
        if config_file.exists() {
            parse_config_file(&config_file, |key, value| {
                if let Some(w) = cfg.apply_file_entry(key, value) {
                    warnings.push(w);
                }
            })?;
        }

        let env_warnings = cfg.apply_env_overrides(get_env);
        warnings.extend(env_warnings);

        Ok((cfg, warnings))
    }

    fn defaults(sipag_dir: &Path) -> Self {
        Self {
            sipag_dir: sipag_dir.to_path_buf(),
            image: DEFAULT_IMAGE.to_string(),
            timeout: 7200,
            work_label: "ready".to_string(),
            max_open_prs: 3,
            poll_interval: 120,
        }
    }

    fn apply_file_entry(&mut self, key: &str, value: &str) -> Option<String> {
        match key {
            "image" => self.image = value.to_string(),
            "timeout" => match value.parse::<u64>() {
                Ok(n) if n < TIMEOUT_MIN_SECS => {
                    self.timeout = TIMEOUT_MIN_SECS;
                    return Some(format!(
                        "config: timeout={n} is invalid (minimum {TIMEOUT_MIN_SECS}s); using {TIMEOUT_MIN_SECS}s"
                    ));
                }
                Ok(n) => self.timeout = n,
                Err(_) => {
                    return Some(format!(
                        "config: timeout={value} is not a valid number; using default 7200s"
                    ));
                }
            },
            "work_label" => self.work_label = value.to_string(),
            "max_open_prs" => match value.parse::<usize>() {
                Ok(n) => self.max_open_prs = n,
                Err(_) => {
                    return Some(format!(
                        "config: max_open_prs={value} is not a valid number; using default 3"
                    ));
                }
            },
            "poll_interval" => match value.parse::<u64>() {
                Ok(n) if n < 10 => {
                    self.poll_interval = 10;
                    return Some(format!(
                        "config: poll_interval={n} is too low (minimum 10s); using 10s"
                    ));
                }
                Ok(n) => self.poll_interval = n,
                Err(_) => {
                    return Some(format!(
                        "config: poll_interval={value} is not a valid number; using default 120s"
                    ));
                }
            },
            _ => {}
        }
        None
    }

    fn apply_env_overrides(&mut self, get_env: impl Fn(&str) -> Option<String>) -> Vec<String> {
        let mut warnings = Vec::new();

        if let Some(v) = get_env("SIPAG_IMAGE") {
            self.image = v;
        }
        if let Some(v) = get_env("SIPAG_TIMEOUT") {
            match v.parse::<u64>() {
                Ok(n) if n < TIMEOUT_MIN_SECS => {
                    self.timeout = TIMEOUT_MIN_SECS;
                    warnings.push(format!(
                        "SIPAG_TIMEOUT={n} is invalid (minimum {TIMEOUT_MIN_SECS}s); using {TIMEOUT_MIN_SECS}s"
                    ));
                }
                Ok(n) => self.timeout = n,
                Err(_) => warnings.push(format!(
                    "SIPAG_TIMEOUT={v} is not a valid number; using default 7200s"
                )),
            }
        }
        if let Some(v) = get_env("SIPAG_WORK_LABEL") {
            self.work_label = v;
        }
        if let Some(v) = get_env("SIPAG_MAX_OPEN_PRS") {
            match v.parse::<usize>() {
                Ok(n) => self.max_open_prs = n,
                Err(_) => warnings.push(format!(
                    "SIPAG_MAX_OPEN_PRS={v} is not a valid number; using default 3"
                )),
            }
        }
        if let Some(v) = get_env("SIPAG_POLL_INTERVAL") {
            match v.parse::<u64>() {
                Ok(n) if n < 10 => {
                    self.poll_interval = 10;
                    warnings.push(format!(
                        "SIPAG_POLL_INTERVAL={n} is too low (minimum 10s); using 10s"
                    ));
                }
                Ok(n) => self.poll_interval = n,
                Err(_) => warnings.push(format!(
                    "SIPAG_POLL_INTERVAL={v} is not a valid number; using default 120s"
                )),
            }
        }
        warnings
    }
}

// ── Config file validation for `sipag doctor` ─────────────────────────────────

/// Validation status of a single config file entry.
#[derive(Debug, PartialEq)]
pub enum ConfigEntryStatus {
    Valid,
    InvalidValue { clamped_to: String },
    Unknown { suggestion: Option<String> },
}

/// A single validated config file entry, for display by `sipag doctor`.
#[derive(Debug)]
pub struct ConfigFileEntry {
    pub key: String,
    pub value: String,
    pub status: ConfigEntryStatus,
}

/// Parse and validate `~/.sipag/config`, returning entries for `sipag doctor` display.
pub fn validate_config_file_for_doctor(sipag_dir: &Path) -> Option<Vec<ConfigFileEntry>> {
    let path = sipag_dir.join("config");
    if !path.exists() {
        return None;
    }
    let mut entries = Vec::new();
    let _ = parse_config_file(&path, |key, value| {
        let status = validate_entry_status(key, value);
        entries.push(ConfigFileEntry {
            key: key.to_string(),
            value: value.to_string(),
            status,
        });
    });
    Some(entries)
}

fn validate_entry_status(key: &str, value: &str) -> ConfigEntryStatus {
    match key {
        "timeout" => match value.parse::<u64>() {
            Ok(n) if n < TIMEOUT_MIN_SECS => ConfigEntryStatus::InvalidValue {
                clamped_to: TIMEOUT_MIN_SECS.to_string(),
            },
            Ok(_) => ConfigEntryStatus::Valid,
            Err(_) => ConfigEntryStatus::InvalidValue {
                clamped_to: "7200 (default)".to_string(),
            },
        },
        "max_open_prs" => match value.parse::<usize>() {
            Ok(_) => ConfigEntryStatus::Valid,
            Err(_) => ConfigEntryStatus::InvalidValue {
                clamped_to: "3 (default)".to_string(),
            },
        },
        "poll_interval" => match value.parse::<u64>() {
            Ok(n) if n < 10 => ConfigEntryStatus::InvalidValue {
                clamped_to: "10".to_string(),
            },
            Ok(_) => ConfigEntryStatus::Valid,
            Err(_) => ConfigEntryStatus::InvalidValue {
                clamped_to: "120 (default)".to_string(),
            },
        },
        "image" | "work_label" => ConfigEntryStatus::Valid,
        _ => ConfigEntryStatus::Unknown {
            suggestion: closest_known_key(key),
        },
    }
}

fn closest_known_key(unknown: &str) -> Option<String> {
    KNOWN_KEYS
        .iter()
        .map(|k| (*k, levenshtein(unknown, k)))
        .filter(|(_, d)| *d <= 3)
        .min_by_key(|(_, d)| *d)
        .map(|(k, _)| k.to_string())
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let n = b.len();
    let mut row: Vec<usize> = (0..=n).collect();
    for (i, &ca) in a.iter().enumerate() {
        let mut prev = i;
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let temp = row[j + 1];
            row[j + 1] = if ca == cb {
                prev
            } else {
                1 + prev.min(temp).min(row[j])
            };
            prev = temp;
        }
    }
    row[n]
}

/// Credentials required by worker containers.
#[derive(Debug)]
pub struct Credentials {
    pub oauth_token: Option<String>,
    pub api_key: Option<String>,
    pub gh_token: String,
}

impl Credentials {
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
        crate::auth::read_token_file(sipag_dir)
    }

    fn resolve_gh_token(get_env: &impl Fn(&str) -> Option<String>) -> Result<String> {
        if let Some(token) = get_env("GH_TOKEN") {
            if !token.is_empty() {
                return Ok(token);
            }
        }
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

/// Return the default sipag directory (`~/.sipag`).
pub fn default_sipag_dir() -> PathBuf {
    env::var("SIPAG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            env::var("HOME")
                .map(|h| PathBuf::from(h).join(".sipag"))
                .unwrap_or_else(|_| PathBuf::from(".sipag"))
        })
}

/// Parse a `key=value` config file, calling `f` for each entry.
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

    #[test]
    fn worker_config_defaults() {
        let dir = TempDir::new().unwrap();
        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(cfg.image, DEFAULT_IMAGE);
        assert_eq!(cfg.timeout, 7200);
        assert_eq!(cfg.work_label, "ready");
        assert_eq!(cfg.max_open_prs, 3);
    }

    #[test]
    fn worker_config_file_override() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("config"),
            "image=custom:v1\ntimeout=900\nwork_label=approved\nmax_open_prs=5\n",
        )
        .unwrap();

        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(cfg.image, "custom:v1");
        assert_eq!(cfg.timeout, 900);
        assert_eq!(cfg.work_label, "approved");
        assert_eq!(cfg.max_open_prs, 5);
    }

    #[test]
    fn worker_config_env_overrides_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config"), "image=file-image:latest\n").unwrap();

        let cfg = WorkerConfig::load_with_env(dir.path(), |k| match k {
            "SIPAG_IMAGE" => Some("env-image:latest".to_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(cfg.image, "env-image:latest");
    }

    #[test]
    fn worker_config_timeout_zero_clamped() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config"), "timeout=0\n").unwrap();

        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(cfg.timeout, TIMEOUT_MIN_SECS);
    }

    #[test]
    fn worker_config_missing_config_file_ok() {
        let dir = TempDir::new().unwrap();
        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(cfg.timeout, 7200);
    }

    #[test]
    fn worker_config_invalid_numeric_values_use_defaults() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config"), "timeout=bad\n").unwrap();

        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(cfg.timeout, 7200);
    }

    #[test]
    fn doctor_no_config_file_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(validate_config_file_for_doctor(dir.path()).is_none());
    }

    #[test]
    fn doctor_unknown_key_with_suggestion() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config"), "imge=foo\n").unwrap();

        let entries = validate_config_file_for_doctor(dir.path()).unwrap();
        assert_eq!(entries.len(), 1);
        match &entries[0].status {
            ConfigEntryStatus::Unknown { suggestion } => {
                assert_eq!(suggestion.as_deref(), Some("image"));
            }
            other => panic!("Expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn levenshtein_same_string_is_zero() {
        assert_eq!(levenshtein("image", "image"), 0);
    }

    #[test]
    fn levenshtein_one_edit() {
        assert_eq!(levenshtein("imge", "image"), 1);
    }

    #[test]
    fn credentials_oauth_from_env() {
        let dir = TempDir::new().unwrap();
        let creds = Credentials::load_with_env(dir.path(), |k| match k {
            "CLAUDE_CODE_OAUTH_TOKEN" => Some("token".to_string()),
            "GH_TOKEN" => Some("gh".to_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(creds.oauth_token, Some("token".to_string()));
    }

    #[test]
    fn credentials_oauth_from_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("token"), "file-token\n").unwrap();

        let creds = Credentials::load_with_env(dir.path(), |k| {
            if k == "GH_TOKEN" {
                Some("gh".to_string())
            } else {
                None
            }
        })
        .unwrap();
        assert_eq!(creds.oauth_token, Some("file-token".to_string()));
    }

    #[test]
    fn credentials_gh_token_from_env() {
        let dir = TempDir::new().unwrap();
        let creds = Credentials::load_with_env(dir.path(), |k| {
            if k == "GH_TOKEN" {
                Some("my-gh".to_string())
            } else {
                None
            }
        })
        .unwrap();
        assert_eq!(creds.gh_token, "my-gh");
    }
}
