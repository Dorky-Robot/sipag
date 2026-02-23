//! Binary smoke tests for the `sipag` CLI.
//!
//! These tests use `assert_cmd` to run the actual compiled binary and verify
//! basic behavior for the v3 CLI (7 commands: dispatch, ps, logs, kill,
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

    for cmd in &["dispatch", "ps", "logs", "kill", "tui", "doctor", "version"] {
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
    let json = r#"{
        "repo": "test/repo",
        "pr_num": 42,
        "issues": [1],
        "branch": "sipag/pr-42",
        "container_id": "abc123",
        "phase": "working",
        "heartbeat": "2026-01-15T10:30:00Z",
        "started": "2026-01-15T10:30:00Z"
    }"#;
    fs::write(dir.path().join("workers/test--repo--pr-42.json"), json).unwrap();

    sipag()
        .arg("ps")
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("#42"))
        .stdout(predicate::str::contains("test/repo"))
        .stdout(predicate::str::contains("working"));
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

// ── Unknown subcommand ──────────────────────────────────────────────────────

#[test]
fn unknown_subcommand_fails() {
    sipag()
        .arg("nonexistent-command")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand"));
}
