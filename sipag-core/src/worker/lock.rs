//! Per-repo process lock for `sipag work`.
//!
//! Prevents two `sipag work` instances from running against the same repo
//! simultaneously. Uses a PID file at `~/.sipag/locks/{repo_slug}.lock`.
//! Stale locks (from crashed processes) are detected by checking whether the
//! recorded PID is still alive.

use anyhow::{bail, Result};
use std::fs;
use std::path::PathBuf;

/// RAII guard that holds the per-repo lock file and removes it on drop.
pub struct WorkerLock {
    path: PathBuf,
}

impl WorkerLock {
    /// Acquire the lock for `repo` (e.g. `"Dorky-Robot/sipag"`).
    ///
    /// - If no lock exists, writes the current PID and returns the guard.
    /// - If a stale lock exists (PID no longer running), overwrites it.
    /// - If a live lock exists and `force` is false, returns an error with the
    ///   existing PID in the message so the operator knows what to kill.
    /// - If a live lock exists and `force` is true, kills the old process and
    ///   acquires the lock.
    pub fn acquire(sipag_dir: &std::path::Path, repo: &str, force: bool) -> Result<Self> {
        let locks_dir = sipag_dir.join("locks");
        fs::create_dir_all(&locks_dir)?;

        let repo_slug = repo.replace('/', "--");
        let lock_path = locks_dir.join(format!("{repo_slug}.lock"));

        if lock_path.exists() {
            if let Ok(contents) = fs::read_to_string(&lock_path) {
                let existing_pid: Option<u32> = contents.trim().parse().ok();
                if let Some(pid) = existing_pid {
                    if is_pid_alive(pid) {
                        if force {
                            eprintln!(
                                "sipag work: killing existing instance (PID {pid}) for {repo}"
                            );
                            kill_process(pid);
                            // Give it a moment to exit before overwriting the lock.
                            std::thread::sleep(std::time::Duration::from_millis(500));
                        } else {
                            bail!(
                                "Another sipag work process (PID {pid}) is already running for {repo}.\n\
                                 Use --force to override."
                            );
                        }
                    }
                    // else: PID is not alive — stale lock, overwrite below.
                }
            }
        }

        let current_pid = std::process::id();
        fs::write(&lock_path, format!("{current_pid}\n"))?;

        Ok(Self { path: lock_path })
    }
}

impl Drop for WorkerLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Returns true if the process with `pid` is currently running.
///
/// Sends signal 0 via `kill -0`: this checks process existence without
/// delivering an actual signal and works on all Unix systems.
fn is_pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Send SIGTERM to a process so it can shut down cleanly.
fn kill_process(pid: u32) {
    let _ = std::process::Command::new("kill")
        .args([&pid.to_string()])
        .status();
}
