use std::collections::HashMap;
use std::path::PathBuf;

/// Runtime configuration derived from environment variables and ~/.sipag/config.
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
    /// Label used to filter GitHub issues in `sipag work`.
    /// Resolution order: SIPAG_WORK_LABEL env var → work_label in config file → "approved".
    /// Set to empty string to disable label filtering (picks up ALL open issues).
    pub work_label: String,
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Parse a `key=value` config file and return a map of key → value pairs.
/// Lines beginning with `#` and blank lines are ignored.
/// Leading/trailing whitespace around keys and values is stripped.
pub fn load_config_file(path: &std::path::Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(content) = std::fs::read_to_string(path) else {
        return map;
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            map.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    map
}

impl Default for Config {
    fn default() -> Self {
        let sipag_dir = std::env::var("SIPAG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_dir().join(".sipag"));

        let token_file = std::env::var("SIPAG_TOKEN_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| sipag_dir.join("token"));

        // Load optional file-based config; env vars take precedence.
        let file_cfg = load_config_file(&sipag_dir.join("config"));

        // Resolve work_label:
        //   1. SIPAG_WORK_LABEL env var (highest priority; empty string disables filter)
        //   2. work_label key in ~/.sipag/config
        //   3. "approved" (default)
        let work_label = std::env::var("SIPAG_WORK_LABEL").unwrap_or_else(|_| {
            file_cfg
                .get("work_label")
                .cloned()
                .unwrap_or_else(|| "approved".to_string())
        });

        Self {
            sipag_dir: sipag_dir.clone(),
            sipag_file: std::env::var("SIPAG_FILE")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("./tasks.md")),
            image: std::env::var("SIPAG_IMAGE")
                .unwrap_or_else(|_| "sipag-worker:latest".to_string()),
            timeout: std::env::var("SIPAG_TIMEOUT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1800),
            token_file,
            work_label,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    // ── load_config_file ──────────────────────────────────────────────────────

    #[test]
    fn test_load_config_file_returns_empty_when_missing() {
        let dir = tempdir().unwrap();
        let cfg = load_config_file(&dir.path().join("nonexistent"));
        assert!(cfg.is_empty());
    }

    #[test]
    fn test_load_config_file_parses_key_value() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "work_label=my-label").unwrap();

        let cfg = load_config_file(&path);
        assert_eq!(cfg.get("work_label").map(|s| s.as_str()), Some("my-label"));
    }

    #[test]
    fn test_load_config_file_ignores_comments() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "# this is a comment").unwrap();
        writeln!(f, "work_label=ci-approved").unwrap();

        let cfg = load_config_file(&path);
        assert_eq!(
            cfg.get("work_label").map(|s| s.as_str()),
            Some("ci-approved")
        );
        assert!(!cfg.contains_key("# this is a comment"));
    }

    #[test]
    fn test_load_config_file_ignores_blank_lines() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "").unwrap();
        writeln!(f, "work_label=approved").unwrap();

        let cfg = load_config_file(&path);
        assert_eq!(
            cfg.get("work_label").map(|s| s.as_str()),
            Some("approved")
        );
    }

    #[test]
    fn test_load_config_file_strips_whitespace() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "  work_label  =  spaced  ").unwrap();

        let cfg = load_config_file(&path);
        assert_eq!(cfg.get("work_label").map(|s| s.as_str()), Some("spaced"));
    }

    #[test]
    fn test_load_config_file_empty_value_preserved() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "work_label=").unwrap();

        let cfg = load_config_file(&path);
        assert_eq!(cfg.get("work_label").map(|s| s.as_str()), Some(""));
    }

    #[test]
    fn test_load_config_file_unknown_keys_present() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "unknown_key=value").unwrap();
        writeln!(f, "work_label=ok").unwrap();

        let cfg = load_config_file(&path);
        assert_eq!(cfg.get("work_label").map(|s| s.as_str()), Some("ok"));
        assert_eq!(cfg.get("unknown_key").map(|s| s.as_str()), Some("value"));
    }
}
