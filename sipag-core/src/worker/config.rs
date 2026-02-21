//! Worker configuration — loaded from env vars and `~/.sipag/config`.
//!
//! Mirrors `lib/worker/config.sh` worker defaults and override logic.

use std::fs;
use std::path::Path;

const DEFAULT_BATCH_SIZE: usize = 1;
const MAX_BATCH_SIZE: usize = 5;
pub const DEFAULT_IMAGE: &str = "ghcr.io/dorky-robot/sipag-worker:latest";
const DEFAULT_TIMEOUT: u64 = 1800;
const DEFAULT_POLL_INTERVAL: u64 = 120;
pub const DEFAULT_WORK_LABEL: &str = "approved";

/// Runtime configuration for the worker polling loop.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Max parallel Docker containers (1–5).
    pub batch_size: usize,
    /// Docker image to run workers in.
    pub image: String,
    /// Per-container timeout in seconds.
    pub timeout: u64,
    /// Seconds between polling cycles.
    pub poll_interval: u64,
    /// GitHub label that gates dispatch (default: `approved`).
    pub work_label: String,
    /// When true: run one polling cycle and exit.
    pub once: bool,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            batch_size: DEFAULT_BATCH_SIZE,
            image: DEFAULT_IMAGE.to_string(),
            timeout: DEFAULT_TIMEOUT,
            poll_interval: DEFAULT_POLL_INTERVAL,
            work_label: DEFAULT_WORK_LABEL.to_string(),
            once: false,
        }
    }
}

impl WorkerConfig {
    /// Load config from `~/.sipag/config` file and environment overrides.
    ///
    /// Environment variables take priority over the config file.
    /// Config file keys: `batch_size`, `image`, `timeout`, `poll_interval`,
    /// `work_label`.
    pub fn load(sipag_dir: &Path) -> Self {
        let mut cfg = Self::default();

        // 1. Load from config file.
        let config_file = sipag_dir.join("config");
        if let Ok(contents) = fs::read_to_string(&config_file) {
            for line in contents.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim();
                    let value = value.trim();
                    match key {
                        "batch_size" => {
                            if let Ok(n) = value.parse::<usize>() {
                                cfg.batch_size = n.min(MAX_BATCH_SIZE);
                            }
                        }
                        "image" => cfg.image = value.to_string(),
                        "timeout" => {
                            if let Ok(n) = value.parse::<u64>() {
                                cfg.timeout = n;
                            }
                        }
                        "poll_interval" => {
                            if let Ok(n) = value.parse::<u64>() {
                                cfg.poll_interval = n;
                            }
                        }
                        "work_label" => cfg.work_label = value.to_string(),
                        _ => {}
                    }
                }
            }
        }

        // 2. Environment overrides.
        if let Ok(v) = std::env::var("SIPAG_BATCH_SIZE") {
            if let Ok(n) = v.parse::<usize>() {
                cfg.batch_size = n.min(MAX_BATCH_SIZE);
            }
        }
        if let Ok(v) = std::env::var("SIPAG_IMAGE") {
            cfg.image = v;
        }
        if let Ok(v) = std::env::var("SIPAG_TIMEOUT") {
            if let Ok(n) = v.parse::<u64>() {
                cfg.timeout = n;
            }
        }
        if let Ok(v) = std::env::var("SIPAG_POLL_INTERVAL") {
            if let Ok(n) = v.parse::<u64>() {
                cfg.poll_interval = n;
            }
        }
        if let Ok(v) = std::env::var("SIPAG_WORK_LABEL") {
            cfg.work_label = v;
        }

        cfg
    }
}
