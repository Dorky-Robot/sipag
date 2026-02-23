//! Worker lifecycle management — scan, check, cleanup.

use anyhow::Result;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::state::{self, WorkerState};

/// Read all worker state files and return current state of all workers.
///
/// For non-terminal workers, checks whether the Docker container is still alive.
/// If the container is gone, the worker is marked as failed so `sipag ps` and
/// back-pressure calculations reflect reality.
pub fn scan_workers(sipag_dir: &Path) -> Vec<WorkerState> {
    let mut workers = state::list_all(sipag_dir);
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    for w in &mut workers {
        if w.phase.is_terminal() {
            continue;
        }
        let container_name = format!("sipag-worker-pr-{}", w.pr_num);
        if !check_container_alive(&container_name) {
            w.phase = state::WorkerPhase::Failed;
            w.ended = Some(now.clone());
            w.error = Some("container exited without updating state".to_string());
            let _ = state::write_state(w);
        }
    }

    workers
}

/// Check whether a Docker container is still running by container name.
pub fn check_container_alive(container_name: &str) -> bool {
    Command::new("docker")
        .args([
            "ps",
            "--filter",
            &format!("name=^{container_name}$"),
            "--format",
            "{{.Names}}",
        ])
        .output()
        .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(false)
}

/// Clean up a finished worker — remove state file and stop container if still running.
pub fn cleanup_finished(worker: &WorkerState, _sipag_dir: &Path) -> Result<()> {
    // Kill container if somehow still running.
    if !worker.container_id.is_empty() {
        let _ = Command::new("docker")
            .args(["rm", "-f", &worker.container_id])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    // Remove state file.
    state::remove_state(&worker.file_path)?;
    Ok(())
}

/// Remove state files for terminal workers (finished/failed) older than `max_age_hours`.
///
/// Returns the number of files cleaned up.
pub fn cleanup_stale(sipag_dir: &Path, max_age_hours: u64) -> usize {
    let workers = scan_workers(sipag_dir);
    let now = chrono::Utc::now();
    let mut cleaned = 0;

    for w in &workers {
        if !w.phase.is_terminal() {
            continue;
        }

        // Use ended time if available, otherwise started time.
        let timestamp = w.ended.as_deref().unwrap_or(&w.started);
        if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(timestamp) {
            let age_hours = (now - ts.with_timezone(&chrono::Utc)).num_hours().max(0) as u64;
            if age_hours >= max_age_hours && state::remove_state(&w.file_path).is_ok() {
                cleaned += 1;
            }
        }
    }

    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{WorkerPhase, WorkerState};
    use tempfile::TempDir;

    #[test]
    fn scan_workers_empty_dir() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();
        let workers = scan_workers(dir.path());
        assert!(workers.is_empty());
    }

    #[test]
    fn scan_workers_finds_state_files() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();

        let state = WorkerState {
            repo: "owner/repo".to_string(),
            pr_num: 42,
            issues: vec![1],
            branch: "sipag/test".to_string(),
            container_id: "abc".to_string(),
            phase: WorkerPhase::Working,
            heartbeat: "2026-01-01T00:00:00Z".to_string(),
            started: "2026-01-01T00:00:00Z".to_string(),
            ended: None,
            exit_code: None,
            error: None,
            file_path: state::state_file_path(dir.path(), "owner/repo", 42),
        };
        state::write_state(&state).unwrap();

        let workers = scan_workers(dir.path());
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].pr_num, 42);
    }

    #[test]
    fn scan_workers_ignores_malformed_files() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir_all(&workers_dir).unwrap();

        // Write one valid and one malformed state file.
        let state = WorkerState {
            repo: "owner/repo".to_string(),
            pr_num: 1,
            issues: vec![],
            branch: "sipag/pr-1".to_string(),
            container_id: "abc".to_string(),
            phase: WorkerPhase::Working,
            heartbeat: "2026-01-01T00:00:00Z".to_string(),
            started: "2026-01-01T00:00:00Z".to_string(),
            ended: None,
            exit_code: None,
            error: None,
            file_path: state::state_file_path(dir.path(), "owner/repo", 1),
        };
        state::write_state(&state).unwrap();
        std::fs::write(workers_dir.join("bad--file--pr-99.json"), "not json{{{").unwrap();

        let workers = scan_workers(dir.path());
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].pr_num, 1);
    }
}
