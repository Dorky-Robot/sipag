//! Docker container dispatch for PR workers.

use anyhow::{Context, Result};
use std::fs::{self, File};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::{Credentials, WorkerConfig};
use crate::state::{self, WorkerPhase, WorkerState};

/// Launch a Docker container to implement a PR.
///
/// The worker clones the repo, checks out the PR branch, reads the PR
/// description as its assignment, and runs Claude Code.
///
/// Returns the Docker container ID on success.
pub fn dispatch_worker(
    repo: &str,
    pr_num: u64,
    branch: &str,
    issues: &[u64],
    cfg: &WorkerConfig,
    creds: &Credentials,
    session_file: Option<&Path>,
) -> Result<String> {
    let repo_slug = repo.replace('/', "--");
    let container_name = format!("sipag-{repo_slug}-pr-{pr_num}");
    let log_dir = cfg.sipag_dir.join("logs");
    fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join(format!("{repo_slug}--pr-{pr_num}.log"));

    // Clean up any stale container from a previous attempt.
    let _ = Command::new("docker")
        .args(["rm", "-f", &container_name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    // Write initial state file.
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let state_path = state::state_file_path(&cfg.sipag_dir, repo, pr_num);
    // Write container_id in the initial state (it's deterministic and known before
    // spawn). This eliminates a race where the container could overwrite the state
    // file between the host's initial write and the second write that sets container_id.
    let initial_state = WorkerState {
        repo: repo.to_string(),
        pr_num,
        issues: issues.to_vec(),
        branch: branch.to_string(),
        container_id: container_name.clone(),
        phase: WorkerPhase::Starting,
        heartbeat: now.clone(),
        started: now.clone(),
        ended: None,
        exit_code: None,
        error: None,
        file_path: state_path.clone(),
    };
    state::write_state(&initial_state)?;

    // Build docker run command.
    let log_out = File::create(&log_path)
        .with_context(|| format!("Failed to create log file: {}", log_path.display()))?;
    let log_err = log_out.try_clone()?;

    let workers_dir = cfg.sipag_dir.join("workers");
    let state_filename = format!("{repo_slug}--pr-{pr_num}.json");

    let timeout_bin = crate::docker::resolve_timeout_command();
    let mut cmd;
    if let Some(ref bin) = timeout_bin {
        cmd = Command::new(bin);
        cmd.arg(cfg.timeout.to_string()).arg("docker").arg("run");
    } else {
        cmd = Command::new("docker");
        cmd.arg("run");
    }

    cmd.arg("--rm")
        .arg("--name")
        .arg(&container_name)
        // Labels for debugging
        .arg("--label")
        .arg(format!("org.sipag.repo={repo}"))
        .arg("--label")
        .arg(format!("org.sipag.pr={pr_num}"))
        // Mount state directory for heartbeats
        .arg("-v")
        .arg(format!("{}:/sipag-state", workers_dir.display()))
        // Mount lessons directory (read-only) for cross-worker learning
        .arg("-v")
        .arg(format!(
            "{}:/sipag-lessons:ro",
            cfg.sipag_dir.join("lessons").display()
        ))
        .arg("-e")
        .arg(format!("STATE_FILE=/sipag-state/{state_filename}"))
        // Environment
        .arg("-e")
        .arg(format!("REPO={repo}"))
        .arg("-e")
        .arg(format!("PR_NUM={pr_num}"))
        .arg("-e")
        .arg(format!("BRANCH={branch}"))
        .arg("-e")
        .arg("CLAUDE_CODE_OAUTH_TOKEN")
        .arg("-e")
        .arg("ANTHROPIC_API_KEY")
        .arg("-e")
        .arg("GH_TOKEN");

    // Mount host session file for --resume (if available).
    if let Some(path) = session_file {
        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        cmd.arg("-v")
            .arg(format!(
                "{}:/sipag-session/session.jsonl:ro",
                path.display()
            ))
            .arg("-e")
            .arg(format!("SIPAG_SESSION_ID={session_id}"));
    }

    // Image and entrypoint
    cmd.arg(&cfg.image)
        .arg("/usr/local/bin/sipag-worker")
        .stdout(Stdio::from(log_out))
        .stderr(Stdio::from(log_err));

    // Set credentials.
    if let Some(ref token) = creds.oauth_token {
        cmd.env("CLAUDE_CODE_OAUTH_TOKEN", token);
    }
    if let Some(ref key) = creds.api_key {
        cmd.env("ANTHROPIC_API_KEY", key);
    }
    cmd.env("GH_TOKEN", &creds.gh_token);

    // Spawn the container.
    let _child = cmd.spawn().context("Failed to spawn Docker container")?;

    println!("[PR #{pr_num}] Worker dispatched: {container_name}");

    Ok(container_name)
}

/// Extract a failure reason from the last 50 lines of a log file.
pub fn extract_failure_reason(log_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(log_path).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    let tail = if lines.len() > 50 {
        &lines[lines.len() - 50..]
    } else {
        &lines
    };

    for line in tail.iter().rev() {
        let lower = line.to_lowercase();
        if lower.contains("repository")
            && (lower.contains("not found") || lower.contains("does not exist"))
        {
            return Some("git clone failed: repository not found".to_string());
        }
        if lower.contains("could not resolve host") {
            return Some("git clone failed: could not resolve host".to_string());
        }
        if lower.contains("authentication failed") {
            return Some("git clone failed: authentication failed".to_string());
        }
    }

    tail.iter()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_reason_repo_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("test.log");
        fs::write(&log, "fatal: repository not found\n").unwrap();
        let reason = extract_failure_reason(&log).unwrap();
        assert!(reason.contains("repository not found"));
    }

    #[test]
    fn failure_reason_fallback_to_last_line() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("test.log");
        fs::write(&log, "Some output\nclaude exited with code 1\n").unwrap();
        let reason = extract_failure_reason(&log).unwrap();
        assert_eq!(reason, "claude exited with code 1");
    }

    #[test]
    fn failure_reason_empty_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("test.log");
        fs::write(&log, "").unwrap();
        assert!(extract_failure_reason(&log).is_none());
    }

    #[test]
    fn failure_reason_auth_failed() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("test.log");
        fs::write(
            &log,
            "Cloning into '/work'...\nfatal: Authentication failed for 'https://github.com/o/r'\n",
        )
        .unwrap();
        let reason = extract_failure_reason(&log).unwrap();
        assert!(reason.contains("authentication failed"));
    }

    #[test]
    fn container_name_format() {
        // The naming convention in dispatch_worker is: sipag-{repo_slug}-pr-{pr_num}
        let repo_slug = "owner/repo".replace('/', "--");
        let name = format!("sipag-{repo_slug}-pr-{}", 42);
        assert_eq!(name, "sipag-owner--repo-pr-42");
        assert!(!name.contains('/'));
    }

    #[test]
    fn initial_state_has_starting_phase() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("workers")).unwrap();

        let state_path = state::state_file_path(dir.path(), "owner/repo", 7);
        let initial = WorkerState {
            repo: "owner/repo".to_string(),
            pr_num: 7,
            issues: vec![10, 20],
            branch: "sipag/pr-7".to_string(),
            container_id: String::new(),
            phase: WorkerPhase::Starting,
            heartbeat: "2026-01-01T00:00:00Z".to_string(),
            started: "2026-01-01T00:00:00Z".to_string(),
            ended: None,
            exit_code: None,
            error: None,
            file_path: state_path.clone(),
        };
        state::write_state(&initial).unwrap();

        let loaded = state::read_state(&state_path).unwrap();
        assert_eq!(loaded.phase, WorkerPhase::Starting);
        assert_eq!(loaded.repo, "owner/repo");
        assert_eq!(loaded.pr_num, 7);
        assert_eq!(loaded.issues, vec![10, 20]);
        assert!(loaded.container_id.is_empty());
    }
}
