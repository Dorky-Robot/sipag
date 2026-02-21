//! Binary smoke tests for the `sipag` CLI.
//!
//! These tests use `assert_cmd` to run the actual compiled binary and verify
//! basic behavior. They would have caught regressions like "sipag status
//! outputs text instead of launching TUI" because the binary must build
//! and respond correctly to each subcommand.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[allow(deprecated)] // cargo_bin works fine for our use case
fn sipag() -> Command {
    Command::cargo_bin("sipag").unwrap()
}

/// Helper: create a temp SIPAG_DIR with the expected subdirectories.
fn temp_sipag_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    for sub in &["queue", "running", "done", "failed"] {
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

    // Verify key subcommands appear in help text
    for cmd in &[
        "tui",
        "work",
        "status",
        "run",
        "ps",
        "logs",
        "kill",
        "add",
        "show",
        "retry",
        "repo",
        "init",
        "version",
        "drain",
        "resume",
        "setup",
        "doctor",
        "triage",
        "completions",
    ] {
        assert!(
            stdout.contains(cmd),
            "Help text should mention '{cmd}' subcommand"
        );
    }
}

// ── Init ────────────────────────────────────────────────────────────────────

#[test]
fn init_creates_directories() {
    let dir = TempDir::new().unwrap();
    sipag()
        .arg("init")
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success();

    for sub in &["queue", "running", "done", "failed"] {
        assert!(
            dir.path().join(sub).exists(),
            "init should create {sub}/ directory"
        );
    }
}

#[test]
fn init_idempotent() {
    let dir = TempDir::new().unwrap();
    sipag()
        .arg("init")
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success();
    // Running again should also succeed
    sipag()
        .arg("init")
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success();
}

// ── Completions ─────────────────────────────────────────────────────────────

#[test]
fn completions_bash() {
    sipag()
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("complete -F _sipag sipag"));
}

#[test]
fn completions_zsh() {
    sipag()
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("#compdef sipag"));
}

#[test]
fn completions_fish() {
    sipag()
        .args(["completions", "fish"])
        .assert()
        .success()
        .stdout(predicate::str::contains("complete -c sipag"));
}

#[test]
fn completions_unknown_shell_fails() {
    sipag()
        .args(["completions", "powershell"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Unknown shell"));
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
        .stdout(predicate::str::contains("No tasks found"));
}

// ── Add ─────────────────────────────────────────────────────────────────────

#[test]
fn add_creates_task_file() {
    let dir = temp_sipag_dir();
    // Write a repos.conf so the repo lookup works
    fs::write(
        dir.path().join("repos.conf"),
        "myrepo=https://github.com/test/repo\n",
    )
    .unwrap();

    sipag()
        .args(["add", "Fix the bug", "--repo", "myrepo"])
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Added: Fix the bug"));

    // Verify a .md file was created in queue/
    let queue_entries: Vec<_> = fs::read_dir(dir.path().join("queue"))
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .collect();
    assert_eq!(queue_entries.len(), 1, "Should have one task in queue/");
}

// ── Show ────────────────────────────────────────────────────────────────────

#[test]
fn show_missing_task() {
    let dir = temp_sipag_dir();
    sipag()
        .args(["show", "nonexistent-task"])
        .env("SIPAG_DIR", dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
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
        .stderr(predicate::str::contains("no log found"));
}

// ── Kill ────────────────────────────────────────────────────────────────────

#[test]
fn kill_missing_task() {
    let dir = temp_sipag_dir();
    sipag()
        .args(["kill", "nonexistent-task"])
        .env("SIPAG_DIR", dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found in running"));
}

// ── Retry ───────────────────────────────────────────────────────────────────

#[test]
fn retry_missing_task() {
    let dir = temp_sipag_dir();
    sipag()
        .args(["retry", "nonexistent-task"])
        .env("SIPAG_DIR", dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found in failed"));
}

// ── Drain / Resume ──────────────────────────────────────────────────────────

#[test]
fn drain_creates_signal_file() {
    let dir = temp_sipag_dir();
    sipag()
        .arg("drain")
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Drain signal sent"));

    assert!(dir.path().join("drain").exists());
}

#[test]
fn resume_clears_drain_signal() {
    let dir = temp_sipag_dir();
    fs::write(dir.path().join("drain"), "").unwrap();

    sipag()
        .arg("resume")
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Drain signal cleared"));

    assert!(!dir.path().join("drain").exists());
}

// ── Repo ────────────────────────────────────────────────────────────────────

#[test]
fn repo_add_and_list() {
    let dir = temp_sipag_dir();

    // Add a repo
    sipag()
        .args(["repo", "add", "myrepo", "https://github.com/test/repo"])
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Registered"));

    // List repos
    sipag()
        .args(["repo", "list"])
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("myrepo"));
}

#[test]
fn repo_list_empty() {
    let dir = temp_sipag_dir();
    sipag()
        .args(["repo", "list"])
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("No repos registered"));
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
