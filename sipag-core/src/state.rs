//! PR-keyed worker state — the single source of truth for worker lifecycle.
//!
//! Each worker gets a JSON state file at `~/.sipag/workers/{owner}--{repo}--pr-{N}.json`.
//! Both the host (sipag) and container (sipag-worker) use this module, ensuring
//! field names and serialization are always consistent.

use anyhow::Result;
use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Lifecycle phase of a worker container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerPhase {
    Starting,
    Working,
    Finished,
    Failed,
}

impl fmt::Display for WorkerPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Starting => write!(f, "starting"),
            Self::Working => write!(f, "working"),
            Self::Finished => write!(f, "finished"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl WorkerPhase {
    pub fn parse(s: &str) -> Self {
        match s {
            "starting" => Self::Starting,
            "working" => Self::Working,
            "finished" => Self::Finished,
            "failed" => Self::Failed,
            _ => Self::Failed,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Finished | Self::Failed)
    }
}

/// State of a single worker, read from a JSON file.
#[derive(Debug, Clone)]
pub struct WorkerState {
    pub repo: String,
    pub pr_num: u64,
    pub issues: Vec<u64>,
    pub branch: String,
    pub container_id: String,
    pub phase: WorkerPhase,
    pub heartbeat: String,
    pub started: String,
    pub ended: Option<String>,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    /// Path to the state file on disk.
    pub file_path: PathBuf,
}

/// Compute the state file path for a given repo and PR number.
pub fn state_file_path(sipag_dir: &Path, repo: &str, pr_num: u64) -> PathBuf {
    let slug = repo.replace('/', "--");
    sipag_dir
        .join("workers")
        .join(format!("{slug}--pr-{pr_num}.json"))
}

/// Read a single worker state file.
pub fn read_state(path: &Path) -> Result<WorkerState> {
    let content = std::fs::read_to_string(path)?;
    let v: serde_json::Value = serde_json::from_str(&content)?;

    Ok(WorkerState {
        repo: v["repo"].as_str().unwrap_or_default().to_string(),
        pr_num: v["pr_num"].as_u64().unwrap_or(0),
        issues: v["issues"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_u64()).collect())
            .unwrap_or_default(),
        branch: v["branch"].as_str().unwrap_or_default().to_string(),
        container_id: v["container_id"].as_str().unwrap_or_default().to_string(),
        phase: WorkerPhase::parse(v["phase"].as_str().unwrap_or("failed")),
        heartbeat: v["heartbeat"].as_str().unwrap_or_default().to_string(),
        started: v["started"].as_str().unwrap_or_default().to_string(),
        ended: v["ended"].as_str().map(|s| s.to_string()),
        exit_code: v["exit_code"].as_i64().map(|n| n as i32),
        error: v["error"].as_str().map(|s| s.to_string()),
        file_path: path.to_path_buf(),
    })
}

/// Write a worker state file as JSON.
pub fn write_state(state: &WorkerState) -> Result<()> {
    if let Some(parent) = state.file_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let issues: Vec<serde_json::Value> = state
        .issues
        .iter()
        .map(|&n| serde_json::Value::Number(n.into()))
        .collect();

    let mut obj = serde_json::Map::new();
    obj.insert("repo".into(), state.repo.clone().into());
    obj.insert("pr_num".into(), state.pr_num.into());
    obj.insert("issues".into(), serde_json::Value::Array(issues));
    obj.insert("branch".into(), state.branch.clone().into());
    obj.insert("container_id".into(), state.container_id.clone().into());
    obj.insert("phase".into(), state.phase.to_string().into());
    obj.insert("heartbeat".into(), state.heartbeat.clone().into());
    obj.insert("started".into(), state.started.clone().into());

    if let Some(ref ended) = state.ended {
        obj.insert("ended".into(), ended.clone().into());
    }
    if let Some(code) = state.exit_code {
        obj.insert("exit_code".into(), code.into());
    }
    if let Some(ref error) = state.error {
        obj.insert("error".into(), error.clone().into());
    }

    let json = serde_json::to_string_pretty(&obj)?;

    // Atomic write: write to temp file in same directory, then rename.
    // rename(2) is atomic on POSIX when src and dst are on the same filesystem.
    let parent = state.file_path.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(json.as_bytes())?;
    tmp.persist(&state.file_path)?;
    Ok(())
}

/// List all worker state files in `sipag_dir/workers/`.
pub fn list_all(sipag_dir: &Path) -> Vec<WorkerState> {
    let workers_dir = sipag_dir.join("workers");
    let mut states = Vec::new();

    let entries = match std::fs::read_dir(&workers_dir) {
        Ok(entries) => entries,
        Err(_) => return states,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            match read_state(&path) {
                Ok(state) => states.push(state),
                Err(e) => eprintln!("sipag: failed to read state file {}: {e}", path.display()),
            }
        }
    }

    states.sort_by(|a, b| b.started.cmp(&a.started));
    states
}

/// Remove a worker state file.
pub fn remove_state(path: &Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Format a duration in seconds as a human-readable string.
pub fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_state(dir: &Path, pr_num: u64) -> WorkerState {
        WorkerState {
            repo: "owner/repo".to_string(),
            pr_num,
            issues: vec![1, 2],
            branch: "sipag/pr-branch".to_string(),
            container_id: "abc123".to_string(),
            phase: WorkerPhase::Working,
            heartbeat: "2026-01-01T00:00:00Z".to_string(),
            started: "2026-01-01T00:00:00Z".to_string(),
            ended: None,
            exit_code: None,
            error: None,
            file_path: state_file_path(dir, "owner/repo", pr_num),
        }
    }

    #[test]
    fn round_trip_state() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();

        let state = sample_state(dir.path(), 42);
        write_state(&state).unwrap();

        let loaded = read_state(&state.file_path).unwrap();
        assert_eq!(loaded.repo, "owner/repo");
        assert_eq!(loaded.pr_num, 42);
        assert_eq!(loaded.issues, vec![1, 2]);
        assert_eq!(loaded.phase, WorkerPhase::Working);
        assert_eq!(loaded.container_id, "abc123");
    }

    #[test]
    fn list_all_returns_states() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();

        let s1 = sample_state(dir.path(), 1);
        let s2 = sample_state(dir.path(), 2);
        write_state(&s1).unwrap();
        write_state(&s2).unwrap();

        let all = list_all(dir.path());
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn state_file_path_format() {
        let path = state_file_path(Path::new("/tmp/.sipag"), "Dorky-Robot/sipag", 501);
        assert_eq!(
            path.to_str().unwrap(),
            "/tmp/.sipag/workers/Dorky-Robot--sipag--pr-501.json"
        );
    }

    #[test]
    fn phase_display() {
        assert_eq!(WorkerPhase::Starting.to_string(), "starting");
        assert_eq!(WorkerPhase::Working.to_string(), "working");
        assert_eq!(WorkerPhase::Finished.to_string(), "finished");
        assert_eq!(WorkerPhase::Failed.to_string(), "failed");
    }

    #[test]
    fn phase_terminal() {
        assert!(!WorkerPhase::Starting.is_terminal());
        assert!(!WorkerPhase::Working.is_terminal());
        assert!(WorkerPhase::Finished.is_terminal());
        assert!(WorkerPhase::Failed.is_terminal());
    }

    #[test]
    fn format_duration_variants() {
        assert_eq!(format_duration(30), "30s");
        assert_eq!(format_duration(120), "2m");
        assert_eq!(format_duration(3661), "1h1m");
    }

    #[test]
    fn remove_state_file() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();

        let state = sample_state(dir.path(), 99);
        write_state(&state).unwrap();
        assert!(state.file_path.exists());

        remove_state(&state.file_path).unwrap();
        assert!(!state.file_path.exists());
    }

    #[test]
    fn remove_nonexistent_file_is_ok() {
        let path = Path::new("/tmp/nonexistent-sipag-state.json");
        assert!(remove_state(path).is_ok());
    }

    #[test]
    fn read_state_malformed_json_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "not json at all{{{").unwrap();
        assert!(read_state(&path).is_err());
    }

    #[test]
    fn read_state_missing_fields_defaults_gracefully() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("minimal.json");
        std::fs::write(&path, r#"{"repo": "a/b"}"#).unwrap();
        let state = read_state(&path).unwrap();
        assert_eq!(state.repo, "a/b");
        assert_eq!(state.pr_num, 0);
        assert_eq!(state.phase, WorkerPhase::Failed); // unknown phase → Failed
    }

    #[test]
    fn phase_parse_unknown_defaults_to_failed() {
        assert_eq!(WorkerPhase::parse("bogus"), WorkerPhase::Failed);
        assert_eq!(WorkerPhase::parse(""), WorkerPhase::Failed);
    }
}
