//! Worker lifecycle management — scan, check, cleanup.

use anyhow::Result;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::state::{self, WorkerState};

/// Read all worker state files and return current state of all workers.
pub fn scan_workers(sipag_dir: &Path) -> Vec<WorkerState> {
    state::list_all(sipag_dir)
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
}
