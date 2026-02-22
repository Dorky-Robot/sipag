//! Runtime configuration for sipag workers.
//!
//! Resolution order: **env var > `~/.sipag/config` file > hardcoded default**.
//!
//! ```text
//! Field                   Env Var                      Config Key               Default
//! ─────────────────────── ──────────────────────────── ──────────────────────── ────────
//! poll_interval           SIPAG_POLL_INTERVAL          poll_interval            120s
//! work_label              SIPAG_WORK_LABEL             work_label               "ready"
//! image                   SIPAG_IMAGE                  image                    ghcr.io/dorky-robot/sipag-worker:latest
//! timeout                 SIPAG_TIMEOUT                timeout                  1800s
//! auto_merge              —                            auto_merge               false
//! doc_refresh_interval    SIPAG_DOC_REFRESH_INTERVAL   doc_refresh_interval     10
//! state_max_age_days      SIPAG_STATE_MAX_AGE_DAYS     state_max_age_days       7
//! max_open_prs            SIPAG_MAX_OPEN_PRS           max_open_prs             5 (0 = disabled)
//! once                    — (CLI --once flag only)     —                        false
//! sipag_dir               SIPAG_DIR                    —                        ~/.sipag
//! ```
//!
//! Credentials follow the same pattern — see [`Credentials`].

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{env, fs};

const TIMEOUT_MIN_SECS: u64 = 1;
const POLL_INTERVAL_MIN_SECS: u64 = 1;

/// Default Docker image for worker containers.
pub const DEFAULT_IMAGE: &str = "ghcr.io/dorky-robot/sipag-worker:latest";

/// All known keys in the `~/.sipag/config` file.
const KNOWN_KEYS: &[&str] = &[
    "batch_size", // Ignored but accepted for backward compat
    "max_open_prs",
    "poll_interval",
    "work_label",
    "image",
    "timeout",
    "auto_merge",
    "doc_refresh_interval",
    "state_max_age_days",
];

/// Runtime configuration for the sipag worker.
///
/// All fields follow the resolution order: env var > `~/.sipag/config` file > hardcoded default.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Base directory for sipag state (`~/.sipag` by default).
    pub sipag_dir: PathBuf,
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
    /// Maximum open sipag PRs before new-issue dispatch is paused (`SIPAG_MAX_OPEN_PRS`; default 5; 0 = disabled).
    ///
    /// When the repo has >= `max_open_prs` open sipag PRs (branches matching `sipag/*`), the
    /// worker skips new issue dispatch and logs a message. Dispatch resumes automatically once
    /// the count drops below the threshold. Set to 0 to disable back-pressure entirely.
    pub max_open_prs: usize,
}

impl WorkerConfig {
    /// Load config from env vars, `~/.sipag/config` file, and hardcoded defaults.
    ///
    /// Resolution order: env var > config file > default.
    ///
    /// Prints warnings to stderr if any config values are invalid (e.g., `timeout=0`).
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

        // 1. Apply config file overrides
        let config_file = sipag_dir.join("config");
        if config_file.exists() {
            parse_config_file(&config_file, |key, value| {
                if let Some(w) = cfg.apply_file_entry(key, value) {
                    warnings.push(w);
                }
            })?;
        }

        // 2. Apply env var overrides (env wins over file)
        let env_warnings = cfg.apply_env_overrides(get_env);
        warnings.extend(env_warnings);

        Ok((cfg, warnings))
    }

    fn defaults(sipag_dir: &Path) -> Self {
        Self {
            sipag_dir: sipag_dir.to_path_buf(),
            poll_interval: Duration::from_secs(120),
            work_label: "ready".to_string(),
            image: DEFAULT_IMAGE.to_string(),
            timeout: Duration::from_secs(1800),
            once: false,
            auto_merge: false,
            doc_refresh_interval: 10,
            state_max_age_days: 7,
            max_open_prs: 5,
        }
    }

    /// Apply a single key=value entry from the config file, returning a warning string if the
    /// value was invalid and was clamped to the minimum.
    fn apply_file_entry(&mut self, key: &str, value: &str) -> Option<String> {
        match key {
            "batch_size" => {
                // Ignored — batch_size was removed. All ready issues are always
                // dispatched to a single worker.
            }
            "image" => self.image = value.to_string(),
            "timeout" => {
                if let Ok(n) = value.parse::<u64>() {
                    if n < TIMEOUT_MIN_SECS {
                        self.timeout = Duration::from_secs(TIMEOUT_MIN_SECS);
                        return Some(format!(
                            "config: timeout={n} is invalid (minimum {TIMEOUT_MIN_SECS}s); using {TIMEOUT_MIN_SECS}s"
                        ));
                    }
                    self.timeout = Duration::from_secs(n);
                }
            }
            "poll_interval" => {
                if let Ok(n) = value.parse::<u64>() {
                    if n < POLL_INTERVAL_MIN_SECS {
                        self.poll_interval = Duration::from_secs(POLL_INTERVAL_MIN_SECS);
                        return Some(format!(
                            "config: poll_interval={n} is invalid (minimum {POLL_INTERVAL_MIN_SECS}s); using {POLL_INTERVAL_MIN_SECS}s"
                        ));
                    }
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
            "max_open_prs" => {
                if let Ok(n) = value.parse::<usize>() {
                    self.max_open_prs = n;
                }
            }
            _ => {}
        }
        None
    }

    fn apply_env_overrides(&mut self, get_env: impl Fn(&str) -> Option<String>) -> Vec<String> {
        let mut warnings = Vec::new();

        // SIPAG_BATCH_SIZE is ignored — all ready issues go to one worker.
        if let Some(v) = get_env("SIPAG_IMAGE") {
            self.image = v;
        }
        if let Some(v) = get_env("SIPAG_TIMEOUT") {
            if let Ok(n) = v.parse::<u64>() {
                if n < TIMEOUT_MIN_SECS {
                    self.timeout = Duration::from_secs(TIMEOUT_MIN_SECS);
                    warnings.push(format!(
                        "SIPAG_TIMEOUT={n} is invalid (minimum {TIMEOUT_MIN_SECS}s); using {TIMEOUT_MIN_SECS}s"
                    ));
                } else {
                    self.timeout = Duration::from_secs(n);
                }
            }
        }
        if let Some(v) = get_env("SIPAG_POLL_INTERVAL") {
            if let Ok(n) = v.parse::<u64>() {
                if n < POLL_INTERVAL_MIN_SECS {
                    self.poll_interval = Duration::from_secs(POLL_INTERVAL_MIN_SECS);
                    warnings.push(format!(
                        "SIPAG_POLL_INTERVAL={n} is invalid (minimum {POLL_INTERVAL_MIN_SECS}s); using {POLL_INTERVAL_MIN_SECS}s"
                    ));
                } else {
                    self.poll_interval = Duration::from_secs(n);
                }
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
        if let Some(v) = get_env("SIPAG_MAX_OPEN_PRS") {
            if let Ok(n) = v.parse::<usize>() {
                self.max_open_prs = n;
            }
        }

        warnings
    }
}

// ── Config file validation for `sipag doctor` ─────────────────────────────────

/// Validation status of a single config file entry.
#[derive(Debug, PartialEq)]
pub enum ConfigEntryStatus {
    /// Key is known and value is valid.
    Valid,
    /// Key is known but value is out of range or unparsable; shows effective value.
    InvalidValue { clamped_to: String },
    /// Key is not recognized; may include a suggestion for a nearby known key.
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
///
/// Returns `None` if the config file does not exist.
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
        "batch_size" => ConfigEntryStatus::Valid, // Ignored, kept for backward compat
        "timeout" => match value.parse::<u64>() {
            Ok(n) if n < TIMEOUT_MIN_SECS => ConfigEntryStatus::InvalidValue {
                clamped_to: TIMEOUT_MIN_SECS.to_string(),
            },
            Ok(_) => ConfigEntryStatus::Valid,
            Err(_) => ConfigEntryStatus::InvalidValue {
                clamped_to: "1800 (default)".to_string(),
            },
        },
        "poll_interval" => match value.parse::<u64>() {
            Ok(n) if n < POLL_INTERVAL_MIN_SECS => ConfigEntryStatus::InvalidValue {
                clamped_to: POLL_INTERVAL_MIN_SECS.to_string(),
            },
            Ok(_) => ConfigEntryStatus::Valid,
            Err(_) => ConfigEntryStatus::InvalidValue {
                clamped_to: "120 (default)".to_string(),
            },
        },
        "doc_refresh_interval" | "state_max_age_days" | "max_open_prs" => {
            match value.parse::<u64>() {
                Ok(_) => ConfigEntryStatus::Valid,
                Err(_) => ConfigEntryStatus::InvalidValue {
                    clamped_to: "default".to_string(),
                },
            }
        }
        "image" | "work_label" => ConfigEntryStatus::Valid,
        "auto_merge" => {
            if value == "true" || value == "false" {
                ConfigEntryStatus::Valid
            } else {
                ConfigEntryStatus::InvalidValue {
                    clamped_to: "false (default)".to_string(),
                }
            }
        }
        _ => ConfigEntryStatus::Unknown {
            suggestion: closest_known_key(key),
        },
    }
}

/// Return the closest known config key to `unknown`, if within edit distance 3.
fn closest_known_key(unknown: &str) -> Option<String> {
    KNOWN_KEYS
        .iter()
        .map(|k| (*k, levenshtein(unknown, k)))
        .filter(|(_, d)| *d <= 3)
        .min_by_key(|(_, d)| *d)
        .map(|(k, _)| k.to_string())
}

/// Compute the Levenshtein edit distance between two strings.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let n = b.len();
    // 1-D rolling row: row[j] = edit distance for a[0..i] vs b[0..j].
    let mut row: Vec<usize> = (0..=n).collect();
    for (i, &ca) in a.iter().enumerate() {
        let mut prev = i; // row[0] before this iteration = i
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
        assert_eq!(cfg.poll_interval, Duration::from_secs(120));
        assert_eq!(cfg.work_label, "ready");
        assert_eq!(cfg.image, DEFAULT_IMAGE);
        assert_eq!(cfg.timeout, Duration::from_secs(1800));
        assert!(!cfg.once);
        assert!(!cfg.auto_merge);
        assert_eq!(cfg.doc_refresh_interval, 10);
        assert_eq!(cfg.state_max_age_days, 7);
        assert_eq!(cfg.max_open_prs, 5);
    }

    #[test]
    fn worker_config_file_override() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("config"),
            "image=custom-image:v1\ntimeout=900\npoll_interval=60\nwork_label=ready\nauto_merge=true\ndoc_refresh_interval=5\nstate_max_age_days=3\n",
        )
        .unwrap();

        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
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
        fs::write(dir.path().join("config"), "image=file-image:latest\n").unwrap();

        let cfg = WorkerConfig::load_with_env(dir.path(), |k| match k {
            "SIPAG_IMAGE" => Some("env-image:latest".to_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(cfg.image, "env-image:latest");
    }

    #[test]
    fn worker_config_env_timeout_overrides_file_timeout() {
        // Regression test for issue #329: SIPAG_TIMEOUT env must beat config file timeout.
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config"), "timeout=1800\n").unwrap();

        let cfg = WorkerConfig::load_with_env(dir.path(), |k| {
            if k == "SIPAG_TIMEOUT" {
                Some("3600".to_string())
            } else {
                None
            }
        })
        .unwrap();
        // Env var (3600) must win over config file (1800).
        assert_eq!(cfg.timeout, Duration::from_secs(3600));
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
    fn worker_config_batch_size_in_config_file_is_ignored() {
        // batch_size was removed — all ready issues go to one worker.
        // Config files with batch_size should still parse without error.
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config"), "batch_size=5\n").unwrap();
        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        // No batch_size field on config; just verify it loads without error
        assert_eq!(cfg.poll_interval, Duration::from_secs(120)); // default
    }

    #[test]
    fn worker_config_timeout_zero_clamped_to_min() {
        // Issue #330: timeout=0 must be clamped to 1s, not cause immediate container termination.
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config"), "timeout=0\n").unwrap();

        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(cfg.timeout, Duration::from_secs(TIMEOUT_MIN_SECS));
    }

    #[test]
    fn worker_config_poll_interval_zero_clamped_to_min() {
        // Issue #330: poll_interval=0 must be clamped to 1s to avoid a tight busy-loop.
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config"), "poll_interval=0\n").unwrap();

        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(
            cfg.poll_interval,
            Duration::from_secs(POLL_INTERVAL_MIN_SECS)
        );
    }

    #[test]
    fn worker_config_comments_and_blank_lines_ignored() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("config"),
            "# comment\n\n  # indented comment\ntimeout=900\n",
        )
        .unwrap();

        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(cfg.timeout, Duration::from_secs(900));
        assert_eq!(cfg.image, DEFAULT_IMAGE); // unchanged
    }

    #[test]
    fn worker_config_unknown_keys_ignored() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("config"),
            "unknown_key=some_value\ntimeout=900\n",
        )
        .unwrap();

        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(cfg.timeout, Duration::from_secs(900));
    }

    #[test]
    fn worker_config_missing_config_file_ok() {
        let dir = TempDir::new().unwrap();
        // No config file — should use defaults without error
        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(cfg.poll_interval, Duration::from_secs(120)); // default
    }

    #[test]
    fn worker_config_invalid_numeric_values_ignored() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("config"),
            "timeout=also_bad\npoll_interval=not_valid\n",
        )
        .unwrap();

        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        // Invalid values are skipped, defaults remain
        assert_eq!(cfg.timeout, Duration::from_secs(1800));
        assert_eq!(cfg.poll_interval, Duration::from_secs(120));
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

    #[test]
    fn worker_config_max_open_prs_default_is_five() {
        let dir = TempDir::new().unwrap();
        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(cfg.max_open_prs, 5);
    }

    #[test]
    fn worker_config_max_open_prs_from_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config"), "max_open_prs=10\n").unwrap();
        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(cfg.max_open_prs, 10);
    }

    #[test]
    fn worker_config_max_open_prs_zero_disables_back_pressure() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config"), "max_open_prs=0\n").unwrap();
        let cfg = WorkerConfig::load_with_env(dir.path(), no_env).unwrap();
        assert_eq!(cfg.max_open_prs, 0);
    }

    #[test]
    fn worker_config_max_open_prs_from_env() {
        let dir = TempDir::new().unwrap();
        let cfg = WorkerConfig::load_with_env(dir.path(), |k| {
            if k == "SIPAG_MAX_OPEN_PRS" {
                Some("8".to_string())
            } else {
                None
            }
        })
        .unwrap();
        assert_eq!(cfg.max_open_prs, 8);
    }

    #[test]
    fn worker_config_max_open_prs_env_overrides_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config"), "max_open_prs=3\n").unwrap();
        let cfg = WorkerConfig::load_with_env(dir.path(), |k| {
            if k == "SIPAG_MAX_OPEN_PRS" {
                Some("7".to_string())
            } else {
                None
            }
        })
        .unwrap();
        assert_eq!(cfg.max_open_prs, 7);
    }

    // ── validate_config_file_for_doctor tests ─────────────────────────────

    #[test]
    fn doctor_no_config_file_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(validate_config_file_for_doctor(dir.path()).is_none());
    }

    #[test]
    fn doctor_valid_config_entries() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("config"),
            "batch_size=3\ntimeout=900\nwork_label=approved\n",
        )
        .unwrap();

        let entries = validate_config_file_for_doctor(dir.path()).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].key, "batch_size");
        assert_eq!(entries[0].status, ConfigEntryStatus::Valid);
        assert_eq!(entries[1].key, "timeout");
        assert_eq!(entries[1].status, ConfigEntryStatus::Valid);
        assert_eq!(entries[2].key, "work_label");
        assert_eq!(entries[2].status, ConfigEntryStatus::Valid);
    }

    #[test]
    fn doctor_batch_size_accepted_as_valid() {
        // batch_size is ignored but accepted for backward compat.
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config"), "batch_size=0\n").unwrap();

        let entries = validate_config_file_for_doctor(dir.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, ConfigEntryStatus::Valid);
    }

    #[test]
    fn doctor_unknown_key_with_suggestion() {
        let dir = TempDir::new().unwrap();
        // "bathc_size" is a typo of "batch_size"
        fs::write(dir.path().join("config"), "bathc_size=4\n").unwrap();

        let entries = validate_config_file_for_doctor(dir.path()).unwrap();
        assert_eq!(entries.len(), 1);
        match &entries[0].status {
            ConfigEntryStatus::Unknown { suggestion } => {
                assert_eq!(suggestion.as_deref(), Some("batch_size"));
            }
            other => panic!("Expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn doctor_unknown_key_no_suggestion_for_gibberish() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config"), "zzzzzzzzz=1\n").unwrap();

        let entries = validate_config_file_for_doctor(dir.path()).unwrap();
        assert_eq!(entries.len(), 1);
        match &entries[0].status {
            ConfigEntryStatus::Unknown { suggestion } => {
                assert!(suggestion.is_none());
            }
            other => panic!("Expected Unknown, got {other:?}"),
        }
    }

    // ── levenshtein tests ─────────────────────────────────────────────────

    #[test]
    fn levenshtein_same_string_is_zero() {
        assert_eq!(levenshtein("batch_size", "batch_size"), 0);
    }

    #[test]
    fn levenshtein_one_typo() {
        assert_eq!(levenshtein("bathc_size", "batch_size"), 2);
    }

    #[test]
    fn levenshtein_empty_strings() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("", "abc"), 3);
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
