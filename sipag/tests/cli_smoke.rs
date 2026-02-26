//! Binary smoke tests for the `sipag` CLI.
//!
//! These tests use `assert_cmd` to run the actual compiled binary and verify
//! basic behavior for the CLI (8 commands: configure, dispatch, ps, logs, kill,
//! tui, doctor, version).

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[allow(deprecated)]
fn sipag() -> Command {
    Command::cargo_bin("sipag").unwrap()
}

/// Helper: create a temp SIPAG_DIR with the expected subdirectories.
fn temp_sipag_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    for sub in &["workers", "logs"] {
        fs::create_dir(dir.path().join(sub)).unwrap();
    }
    dir
}

// ── Binary builds and runs ──────────────────────────────────────────────────

#[test]
fn binary_exists() {
    sipag();
}

// ── Version ─────────────────────────────────────────────────────────────────

#[test]
fn version_subcommand() {
    sipag()
        .arg("version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("sipag "));
}

#[test]
fn version_flag() {
    sipag()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("sipag "));
}

#[test]
fn version_flag_short() {
    sipag()
        .arg("-v")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("sipag "));
}

// ── Help ────────────────────────────────────────────────────────────────────

#[test]
fn help_flag() {
    sipag()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "sipag spins up isolated Docker sandboxes",
        ));
}

#[test]
fn help_lists_subcommands() {
    let output = sipag().arg("--help").output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    for cmd in &[
        "configure",
        "dispatch",
        "ps",
        "logs",
        "kill",
        "tui",
        "doctor",
        "version",
    ] {
        assert!(
            stdout.contains(cmd),
            "Help text should mention '{cmd}' subcommand"
        );
    }
}

// ── Ps ──────────────────────────────────────────────────────────────────────

#[test]
fn ps_empty() {
    let dir = temp_sipag_dir();
    sipag()
        .arg("ps")
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("No workers found"));
}

#[test]
fn ps_shows_worker() {
    let dir = temp_sipag_dir();
    // Use a recent timestamp so the stale-filter doesn't hide it.
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    // Use a terminal phase because scan_workers reconciles non-terminal
    // workers against Docker liveness (no Docker in tests → reconciled to failed).
    let json = format!(
        r#"{{"repo":"test/repo","pr_num":42,"issues":[1],"branch":"sipag/pr-42","container_id":"abc123","phase":"finished","heartbeat":"{now}","started":"{now}"}}"#
    );
    fs::write(dir.path().join("workers/test--repo--pr-42.json"), &json).unwrap();

    sipag()
        .arg("ps")
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("#42"))
        .stdout(predicate::str::contains("test/repo"))
        .stdout(predicate::str::contains("finished"));
}

// ── Logs ────────────────────────────────────────────────────────────────────

#[test]
fn logs_missing_task() {
    let dir = temp_sipag_dir();
    sipag()
        .args(["logs", "nonexistent-task"])
        .env("SIPAG_DIR", dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("No logs found"));
}

// ── Kill ────────────────────────────────────────────────────────────────────

#[test]
fn kill_nonexistent_prints_message() {
    let dir = temp_sipag_dir();
    sipag()
        .args(["kill", "nonexistent-task"])
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Killed nonexistent-task"));
}

// ── Doctor ──────────────────────────────────────────────────────────────────

#[test]
fn doctor_outputs_checks() {
    let dir = temp_sipag_dir();
    sipag()
        .arg("doctor")
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("sipag doctor"))
        .stdout(predicate::str::contains("Docker daemon:"))
        .stdout(predicate::str::contains("GitHub CLI:"))
        .stdout(predicate::str::contains("sipag dir:"));
}

#[test]
fn doctor_shows_sipag_dir_ok() {
    let dir = temp_sipag_dir();
    sipag()
        .arg("doctor")
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("sipag dir:      OK"));
}

// ── Dispatch (validation errors) ────────────────────────────────────────────

#[test]
fn dispatch_requires_repo_and_pr() {
    sipag()
        .arg("dispatch")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--repo"));
}

// ── Ps (state verification) ─────────────────────────────────────────────────

#[test]
fn ps_multiple_workers_all_shown() {
    let dir = temp_sipag_dir();
    // Use terminal phases — non-terminal workers get reconciled by scan_workers
    // (no Docker in tests), and recent timestamps so they pass the stale filter.
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    for (pr, phase) in [(10, "finished"), (20, "failed")] {
        let json = format!(
            r#"{{"repo":"a/b","pr_num":{pr},"issues":[],"branch":"sipag/pr-{pr}","container_id":"c{pr}","phase":"{phase}","heartbeat":"{now}","started":"{now}"}}"#
        );
        fs::write(dir.path().join(format!("workers/a--b--pr-{pr}.json")), json).unwrap();
    }

    sipag()
        .arg("ps")
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("#10"))
        .stdout(predicate::str::contains("#20"));
}

#[test]
fn ps_shows_finished_and_failed() {
    let dir = temp_sipag_dir();
    // Use a recent timestamp so the stale-filter doesn't hide these.
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    for (pr, phase) in [(1, "finished"), (2, "failed")] {
        let json = format!(
            r#"{{"repo":"o/r","pr_num":{pr},"issues":[],"branch":"b","container_id":"c","phase":"{phase}","heartbeat":"{now}","started":"{now}"}}"#
        );
        fs::write(dir.path().join(format!("workers/o--r--pr-{pr}.json")), json).unwrap();
    }

    sipag()
        .arg("ps")
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("finished"))
        .stdout(predicate::str::contains("failed"));
}

// ── Kill (state mutation) ───────────────────────────────────────────────────

#[test]
fn kill_by_pr_number_updates_state() {
    let dir = temp_sipag_dir();
    // In test (no Docker), scan_workers reconciles non-terminal workers to failed.
    // So killing a "working" worker finds it already failed by reconciliation.
    // The kill command preserves the terminal state and reports accordingly.
    let json = r#"{"repo":"o/r","pr_num":42,"issues":[],"branch":"b","container_id":"fake","phase":"working","heartbeat":"2026-01-01T00:00:00Z","started":"2026-01-01T00:00:00Z"}"#;
    let state_path = dir.path().join("workers/o--r--pr-42.json");
    fs::write(&state_path, json).unwrap();

    sipag()
        .args(["kill", "#42"])
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("PR #42"));

    // Verify the state file shows failed (set by scan_workers reconciliation).
    let updated = fs::read_to_string(&state_path).unwrap();
    assert!(
        updated.contains("\"failed\""),
        "Phase should be 'failed' after kill"
    );
}

// ── Doctor (config entries) ─────────────────────────────────────────────────

#[test]
fn doctor_shows_config_entries() {
    let dir = temp_sipag_dir();
    fs::write(dir.path().join("config"), "image=custom:v1\ntimeout=300\n").unwrap();

    sipag()
        .arg("doctor")
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("image=custom:v1"))
        .stdout(predicate::str::contains("timeout=300"));
}

// ── Logs (file fallback) ────────────────────────────────────────────────────

#[test]
fn logs_falls_back_to_log_file() {
    let dir = temp_sipag_dir();
    let state_json = r#"{"repo":"o/r","pr_num":7,"issues":[],"branch":"b","container_id":"nonexistent-container","phase":"finished","heartbeat":"2026-01-01T00:00:00Z","started":"2026-01-01T00:00:00Z"}"#;
    fs::write(dir.path().join("workers/o--r--pr-7.json"), state_json).unwrap();
    fs::write(
        dir.path().join("logs/o--r--pr-7.log"),
        "Worker output line 1\nWorker output line 2\n",
    )
    .unwrap();

    sipag()
        .args(["logs", "#7"])
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Worker output line 1"));
}

// ── Configure ───────────────────────────────────────────────────────────────

#[test]
fn configure_static_creates_all_templates() {
    let dir = TempDir::new().unwrap();
    // Create a .git dir so the warning doesn't fire.
    fs::create_dir(dir.path().join(".git")).unwrap();

    sipag()
        .args(["configure", "--static", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Installed"));

    let claude_dir = dir.path().join(".claude");
    // Agents
    assert!(claude_dir.join("agents/security-reviewer.md").exists());
    assert!(claude_dir.join("agents/architecture-reviewer.md").exists());
    assert!(claude_dir.join("agents/correctness-reviewer.md").exists());
    // Commands
    assert!(claude_dir.join("commands/dispatch.md").exists());
    assert!(claude_dir.join("commands/review.md").exists());
    assert!(claude_dir.join("commands/triage.md").exists());
    assert!(claude_dir.join("commands/ship-it.md").exists());
    // No hooks or settings — sipag configure only creates agents and commands
    assert!(!claude_dir.join("hooks").exists());
}

#[test]
fn configure_static_overwrites_existing() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();

    // First run — creates files.
    sipag()
        .args(["configure", "--static", dir.path().to_str().unwrap()])
        .assert()
        .success();

    // Second run — overwrites existing (no skip behavior).
    sipag()
        .args(["configure", "--static", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("overwrite:"));
}

#[test]
fn configure_help_shows_static_flag() {
    sipag()
        .args(["configure", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--static"));
}

// ── Configure alias ─────────────────────────────────────────────────────────

#[test]
fn config_alias_works() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();

    sipag()
        .args(["config", "--static", dir.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("Installed"));

    assert!(dir
        .path()
        .join(".claude/agents/security-reviewer.md")
        .exists());
}

// ── Unknown subcommand ──────────────────────────────────────────────────────

#[test]
fn unknown_subcommand_fails() {
    sipag()
        .arg("nonexistent-command")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand"));
}
