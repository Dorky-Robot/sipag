//! Worker lifecycle management — scan, check, cleanup.
//!
//! Liveness detection uses a three-tier approach:
//! 1. **Heartbeat file** (fast path) — a single `stat()` call per worker
//! 2. **Grace period** — workers started less than 60s ago are assumed alive
//! 3. **Docker ps** (fallback) — for old workers without heartbeat files

use anyhow::Result;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::SystemTime;

use crate::state::{self, WorkerState};

/// Default staleness threshold if no config is provided.
const DEFAULT_HEARTBEAT_STALE_SECS: u64 = 90;

/// Grace period for workers that just started (no heartbeat file yet).
const STARTUP_GRACE_SECS: u64 = 60;

/// Check if a heartbeat file is fresh (mtime within staleness threshold).
///
/// Returns:
/// - `Some(true)` if heartbeat exists and is fresh (worker alive)
/// - `Some(false)` if heartbeat exists but is stale (worker likely dead)
/// - `None` if no heartbeat file exists
fn check_heartbeat(state_path: &Path, stale_secs: u64) -> Option<bool> {
    let heartbeat_path = state_path.with_extension("heartbeat");
    let metadata = std::fs::metadata(&heartbeat_path).ok()?;
    let modified = metadata.modified().ok()?;
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default();
    Some(age.as_secs() < stale_secs)
}

/// Check if a worker was started recently enough to be in its grace period.
fn in_grace_period(started: &str) -> bool {
    if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(started) {
        let age = chrono::Utc::now() - ts.with_timezone(&chrono::Utc);
        age.num_seconds() < STARTUP_GRACE_SECS as i64
    } else {
        false
    }
}

/// Read all worker state files and return current state of all workers.
///
/// For non-terminal workers, checks liveness via heartbeat files (fast path),
/// grace period, or Docker ps (fallback for old workers without heartbeats).
/// Dead workers are marked as failed so `sipag ps` and back-pressure
/// calculations reflect reality.
pub fn scan_workers(sipag_dir: &Path) -> Vec<WorkerState> {
    scan_workers_with_stale_secs(sipag_dir, DEFAULT_HEARTBEAT_STALE_SECS)
}

/// Like `scan_workers` but with a configurable staleness threshold.
pub fn scan_workers_with_stale_secs(sipag_dir: &Path, stale_secs: u64) -> Vec<WorkerState> {
    let mut workers = state::list_all(sipag_dir);
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    for w in &mut workers {
        if w.phase.is_terminal() {
            continue;
        }

        // Tier 1: Check heartbeat file (one stat() call — no subprocess).
        match check_heartbeat(&w.file_path, stale_secs) {
            Some(true) => continue, // fresh heartbeat → alive
            Some(false) => {
                // Stale heartbeat — re-read state in case it finished between reads.
                if let Ok(fresh) = state::read_state(&w.file_path) {
                    if fresh.phase.is_terminal() {
                        *w = fresh;
                        continue;
                    }
                }
                mark_worker_failed(w, sipag_dir, &now, "heartbeat stale — worker presumed dead");
                continue;
            }
            None => {} // no heartbeat file — fall through
        }

        // Tier 2: Grace period for recently-started workers (no heartbeat yet).
        if in_grace_period(&w.started) {
            continue;
        }

        // Tier 3: Fallback to docker ps (backward compat for old workers).
        let container_name =
            if w.container_id.is_empty() || w.container_id.chars().all(|c| c.is_ascii_digit()) {
                let repo_slug = w.repo.replace('/', "--");
                format!("sipag-{repo_slug}-pr-{}", w.pr_num)
            } else {
                w.container_id.clone()
            };
        if !crate::docker::is_container_running(&container_name) {
            // Re-read to avoid race: container may have written terminal state
            // between our initial read and this check.
            if let Ok(fresh) = state::read_state(&w.file_path) {
                if fresh.phase.is_terminal() {
                    *w = fresh;
                    continue;
                }
            }
            mark_worker_failed(
                w,
                sipag_dir,
                &now,
                "container exited without updating state",
            );
        }
    }

    workers
}

/// Mark a worker as failed and emit a lifecycle event + lesson.
fn mark_worker_failed(w: &mut WorkerState, sipag_dir: &Path, now: &str, reason: &str) {
    w.phase = state::WorkerPhase::Failed;
    w.ended = Some(now.to_string());
    w.error = Some(reason.to_string());
    let _ = state::write_state(w);

    // Emit lifecycle event.
    let _ = crate::events::write_event(
        sipag_dir,
        "worker-orphaned",
        &w.repo,
        &format!("worker-orphaned: PR #{} in {}", w.pr_num, w.repo),
        reason,
    );

    // Extract failure reason from logs and record as lesson.
    let repo_slug = w.repo.replace('/', "--");
    let log_path = sipag_dir
        .join("logs")
        .join(format!("{repo_slug}--pr-{}.log", w.pr_num));
    let lesson_detail = crate::worker::dispatch::extract_failure_reason(&log_path)
        .unwrap_or_else(|| reason.to_string());
    let _ = crate::lessons::append_lesson(
        sipag_dir,
        &w.repo,
        &format!("## PR #{} failed ({})\n\n{}", w.pr_num, now, lesson_detail),
    );
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
/// Also cleans up orphaned heartbeat files alongside removed state files.
/// Falls back to file mtime when timestamps are unparsable (handles old-format
/// state files that would otherwise persist forever).
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
        let age_hours = if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(timestamp) {
            (now - ts.with_timezone(&chrono::Utc)).num_hours().max(0) as u64
        } else {
            // Unparsable timestamp (old-format state file) — fall back to file mtime.
            std::fs::metadata(&w.file_path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|mtime| {
                    SystemTime::now()
                        .duration_since(mtime)
                        .ok()
                        .map(|d| d.as_secs() / 3600)
                })
                .unwrap_or(0)
        };

        if age_hours >= max_age_hours && state::remove_state(&w.file_path).is_ok() {
            // Also remove any orphaned heartbeat file.
            let heartbeat_path = w.file_path.with_extension("heartbeat");
            let _ = std::fs::remove_file(&heartbeat_path);
            cleaned += 1;
        }
    }

    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{WorkerPhase, WorkerState};
    use tempfile::TempDir;

    fn make_worker(dir: &Path, pr_num: u64, phase: WorkerPhase, started: &str) -> WorkerState {
        let state = WorkerState {
            repo: "owner/repo".to_string(),
            pr_num,
            issues: vec![1],
            branch: format!("sipag/pr-{pr_num}"),
            container_id: "abc".to_string(),
            phase,
            heartbeat: started.to_string(),
            started: started.to_string(),
            ended: None,
            exit_code: None,
            error: None,
            file_path: state::state_file_path(dir, "owner/repo", pr_num),
        };
        state::write_state(&state).unwrap();
        state
    }

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
        make_worker(
            dir.path(),
            42,
            WorkerPhase::Finished,
            "2026-01-01T00:00:00Z",
        );

        let workers = scan_workers(dir.path());
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].pr_num, 42);
    }

    #[test]
    fn scan_workers_ignores_malformed_files() {
        let dir = TempDir::new().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir_all(&workers_dir).unwrap();
        make_worker(dir.path(), 1, WorkerPhase::Finished, "2026-01-01T00:00:00Z");
        std::fs::write(workers_dir.join("bad--file--pr-99.json"), "not json{{{").unwrap();

        let workers = scan_workers(dir.path());
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].pr_num, 1);
    }

    #[test]
    fn heartbeat_fresh_means_alive() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();
        let w = make_worker(dir.path(), 10, WorkerPhase::Working, "2020-01-01T00:00:00Z");

        // Write a fresh heartbeat file.
        let heartbeat_path = w.file_path.with_extension("heartbeat");
        std::fs::write(&heartbeat_path, "{}").unwrap();

        let workers = scan_workers(dir.path());
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].phase, WorkerPhase::Working); // still alive
    }

    #[test]
    fn check_heartbeat_returns_none_when_missing() {
        let dir = TempDir::new().unwrap();
        let fake_path = dir.path().join("nonexistent.json");
        assert!(check_heartbeat(&fake_path, 90).is_none());
    }

    #[test]
    fn check_heartbeat_fresh_file() {
        let dir = TempDir::new().unwrap();
        let state_path = dir.path().join("test.json");
        let heartbeat_path = dir.path().join("test.heartbeat");
        std::fs::write(&heartbeat_path, "{}").unwrap();
        assert_eq!(check_heartbeat(&state_path, 90), Some(true));
    }

    #[test]
    fn grace_period_for_recently_started() {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        assert!(in_grace_period(&now));
    }

    #[test]
    fn no_grace_period_for_old_workers() {
        assert!(!in_grace_period("2020-01-01T00:00:00Z"));
    }

    #[test]
    fn terminal_workers_skip_liveness_check() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();
        make_worker(dir.path(), 5, WorkerPhase::Finished, "2020-01-01T00:00:00Z");

        let workers = scan_workers(dir.path());
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].phase, WorkerPhase::Finished);
    }

    #[test]
    fn cleanup_stale_removes_heartbeat_files() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();

        let mut w = make_worker(dir.path(), 20, WorkerPhase::Failed, "2020-01-01T00:00:00Z");
        w.ended = Some("2020-01-01T01:00:00Z".to_string());
        state::write_state(&w).unwrap();

        let heartbeat_path = w.file_path.with_extension("heartbeat");
        std::fs::write(&heartbeat_path, "{}").unwrap();
        assert!(heartbeat_path.exists());

        let cleaned = cleanup_stale(dir.path(), 1);
        assert_eq!(cleaned, 1);
        assert!(!heartbeat_path.exists());
    }
}
