use std::path::{Path, PathBuf};
use std::time::Duration;

const DEFAULT_IMAGE: &str = "ghcr.io/dorky-robot/sipag-worker:latest";
const DEFAULT_TIMEOUT: u64 = 1800;
const DEFAULT_BATCH_SIZE: usize = 1;
const MAX_BATCH_SIZE: usize = 5;
const DEFAULT_POLL_INTERVAL_SECS: u64 = 120;
const DEFAULT_WORK_LABEL: &str = "approved";
const DEFAULT_DOC_REFRESH_INTERVAL: u64 = 10;

/// Configuration for the worker polling loop.
///
/// Value object: all fields are immutable once loaded. Parsed from env vars
/// and the `~/.sipag/config` key=value file, with env vars taking precedence.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Base sipag directory (default: `~/.sipag`).
    pub sipag_dir: PathBuf,
    /// Docker image for worker containers.
    pub image: String,
    /// Per-container timeout in seconds.
    pub timeout: u64,
    /// Max parallel Docker containers per dispatch batch.
    pub batch_size: usize,
    /// Sleep duration between poll cycles.
    pub poll_interval: Duration,
    /// GitHub label that marks issues ready for dispatch.
    pub work_label: String,
    /// If true, exit after one polling cycle.
    pub once: bool,
    /// Doc refresh interval in cycles (0 = disabled).
    pub doc_refresh_interval: u64,
}

impl WorkerConfig {
    /// Load config from `~/.sipag/config` file and environment variables.
    ///
    /// Resolution order (later wins):
    ///   1. Built-in defaults
    ///   2. `~/.sipag/config` key=value file
    ///   3. Environment variables (`SIPAG_*`)
    ///
    /// The `once` flag is set by the caller (from `--once` CLI flag), not config.
    pub fn load(sipag_dir: &Path, once: bool) -> Self {
        let mut image = DEFAULT_IMAGE.to_string();
        let mut timeout = DEFAULT_TIMEOUT;
        let mut batch_size = DEFAULT_BATCH_SIZE;
        let mut poll_interval_secs = DEFAULT_POLL_INTERVAL_SECS;
        let mut work_label = DEFAULT_WORK_LABEL.to_string();
        let mut doc_refresh_interval = DEFAULT_DOC_REFRESH_INTERVAL;

        // Load from config file (~/.sipag/config)
        let config_path = sipag_dir.join("config");
        if let Ok(content) = std::fs::read_to_string(&config_path) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim();
                    let value = value.trim();
                    match key {
                        "image" => image = value.to_string(),
                        "timeout" => {
                            if let Ok(v) = value.parse() {
                                timeout = v;
                            }
                        }
                        "batch_size" => {
                            if let Ok(v) = value.parse::<usize>() {
                                batch_size = v.min(MAX_BATCH_SIZE);
                            }
                        }
                        "poll_interval" => {
                            if let Ok(v) = value.parse() {
                                poll_interval_secs = v;
                            }
                        }
                        "work_label" => work_label = value.to_string(),
                        "doc_refresh_interval" => {
                            if let Ok(v) = value.parse() {
                                doc_refresh_interval = v;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Environment variables override config file
        if let Ok(v) = std::env::var("SIPAG_IMAGE") {
            if !v.is_empty() {
                image = v;
            }
        }
        if let Ok(v) = std::env::var("SIPAG_TIMEOUT") {
            if let Ok(n) = v.parse() {
                timeout = n;
            }
        }
        if let Ok(v) = std::env::var("SIPAG_BATCH_SIZE") {
            if let Ok(n) = v.parse::<usize>() {
                batch_size = n.min(MAX_BATCH_SIZE);
            }
        }
        if let Ok(v) = std::env::var("SIPAG_POLL_INTERVAL") {
            if let Ok(n) = v.parse() {
                poll_interval_secs = n;
            }
        }
        if let Ok(v) = std::env::var("SIPAG_WORK_LABEL") {
            if !v.is_empty() {
                work_label = v;
            }
        }
        if let Ok(v) = std::env::var("SIPAG_DOC_REFRESH_INTERVAL") {
            if let Ok(n) = v.parse() {
                doc_refresh_interval = n;
            }
        }

        Self {
            sipag_dir: sipag_dir.to_path_buf(),
            image,
            timeout,
            batch_size,
            poll_interval: Duration::from_secs(poll_interval_secs),
            work_label,
            once,
            doc_refresh_interval,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as IoWrite;
    use tempfile::TempDir;

    fn make_dir() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn defaults_when_no_config() {
        let dir = make_dir();
        let cfg = WorkerConfig::load(dir.path(), false);
        assert_eq!(cfg.image, DEFAULT_IMAGE);
        assert_eq!(cfg.timeout, DEFAULT_TIMEOUT);
        assert_eq!(cfg.batch_size, DEFAULT_BATCH_SIZE);
        assert_eq!(
            cfg.poll_interval,
            Duration::from_secs(DEFAULT_POLL_INTERVAL_SECS)
        );
        assert_eq!(cfg.work_label, DEFAULT_WORK_LABEL);
        assert!(!cfg.once);
    }

    #[test]
    fn once_flag_respected() {
        let dir = make_dir();
        let cfg = WorkerConfig::load(dir.path(), true);
        assert!(cfg.once);
    }

    #[test]
    fn config_file_overrides_defaults() {
        let dir = make_dir();
        let mut f = std::fs::File::create(dir.path().join("config")).unwrap();
        writeln!(f, "batch_size=3").unwrap();
        writeln!(f, "poll_interval=60").unwrap();
        writeln!(f, "work_label=ready").unwrap();
        writeln!(f, "image=myimage:latest").unwrap();
        writeln!(f, "timeout=900").unwrap();
        writeln!(f, "doc_refresh_interval=5").unwrap();
        drop(f);

        let cfg = WorkerConfig::load(dir.path(), false);
        assert_eq!(cfg.batch_size, 3);
        assert_eq!(cfg.poll_interval, Duration::from_secs(60));
        assert_eq!(cfg.work_label, "ready");
        assert_eq!(cfg.image, "myimage:latest");
        assert_eq!(cfg.timeout, 900);
        assert_eq!(cfg.doc_refresh_interval, 5);
    }

    #[test]
    fn batch_size_clamped_to_max() {
        let dir = make_dir();
        let mut f = std::fs::File::create(dir.path().join("config")).unwrap();
        writeln!(f, "batch_size=99").unwrap();
        drop(f);

        let cfg = WorkerConfig::load(dir.path(), false);
        assert_eq!(cfg.batch_size, MAX_BATCH_SIZE);
    }

    #[test]
    fn comments_and_blanks_ignored_in_config() {
        let dir = make_dir();
        let mut f = std::fs::File::create(dir.path().join("config")).unwrap();
        writeln!(f, "# this is a comment").unwrap();
        writeln!(f).unwrap();
        writeln!(f, "batch_size=2").unwrap();
        drop(f);

        let cfg = WorkerConfig::load(dir.path(), false);
        assert_eq!(cfg.batch_size, 2);
    }
}
