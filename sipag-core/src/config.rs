use std::path::PathBuf;

/// Runtime configuration derived from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    /// Base directory for sipag state (~/.sipag by default).
    pub sipag_dir: PathBuf,
    /// Legacy task file (./tasks.md by default).
    pub sipag_file: PathBuf,
    /// Docker image to use for task execution.
    pub image: String,
    /// Per-task timeout in seconds.
    pub timeout: u64,
    /// Path to OAuth token file.
    pub token_file: PathBuf,
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

impl Default for Config {
    fn default() -> Self {
        let sipag_dir = std::env::var("SIPAG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_dir().join(".sipag"));

        let token_file = std::env::var("SIPAG_TOKEN_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| sipag_dir.join("token"));

        Self {
            sipag_dir: sipag_dir.clone(),
            sipag_file: std::env::var("SIPAG_FILE")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("./tasks.md")),
            image: std::env::var("SIPAG_IMAGE")
                .unwrap_or_else(|_| "ghcr.io/dorky-robot/sipag-worker:latest".to_string()),
            timeout: std::env::var("SIPAG_TIMEOUT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1800),
            token_file,
        }
    }
}
