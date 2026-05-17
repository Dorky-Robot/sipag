//! Binary smoke tests for the `sipag` CLI.
//!
//! These tests use `assert_cmd` to run the actual compiled binary and verify
//! basic behavior for the CLI subcommands (dispatch, up, tui, version, ...).

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[allow(deprecated)]
fn sipag() -> Command {
    Command::cargo_bin("sipag").unwrap()
}

/// Helper: create a temp SIPAG_DIR for tests.
fn temp_sipag_dir() -> TempDir {
    TempDir::new().unwrap()
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
        .stdout(predicate::str::contains("board-driven dispatcher"));
}

#[test]
fn help_lists_subcommands() {
    let output = sipag().arg("--help").output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // `feature` and `refine` subcommands were deprecated 2026-05-17
    // along with sipag_core::{feature, refine}. See docs/modules.md §3.
    for cmd in &["dispatch", "up", "tui", "add", "list", "move", "version"] {
        assert!(
            stdout.contains(cmd),
            "Help text should mention '{cmd}' subcommand"
        );
    }
}

// ── Dispatch (validation errors) ────────────────────────────────────────────

#[test]
fn dispatch_requires_target() {
    sipag()
        .arg("dispatch")
        .assert()
        .failure()
        .stderr(predicate::str::contains("TASK_ID"));
}

// ── Dispatch (task-based) ───────────────────────────────────────────────────

#[test]
fn dispatch_task_requires_project() {
    // Dispatching task #1 with no project configured should fail with a
    // helpful message.
    let dir = temp_sipag_dir();
    sipag()
        .args(["dispatch", "1"])
        .env("SIPAG_DIR", dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("No project specified"));
}

#[test]
fn dispatch_task_missing_task() {
    // Dispatching a non-existent task should fail.
    let dir = temp_sipag_dir();
    // Create a project so project resolution works.
    let project_dir = dir.path().join("projects/testproj");
    fs::create_dir_all(project_dir.join("tasks")).unwrap();
    fs::create_dir_all(project_dir.join("roles")).unwrap();
    fs::write(
        project_dir.join("project.toml"),
        "name = \"testproj\"\nrepo = \"a/b\"\nstatuses = [\"todo\", \"done\"]\n",
    )
    .unwrap();
    // Set as default.
    fs::write(
        dir.path().join("config.toml"),
        "default_project = \"testproj\"\n",
    )
    .unwrap();

    sipag()
        .args(["dispatch", "999"])
        .env("SIPAG_DIR", dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("task #999 not found"));
}

// ── Up ─────────────────────────────────────────────────────────────────────

#[test]
fn up_requires_project() {
    let dir = temp_sipag_dir();
    sipag()
        .arg("up")
        .env("SIPAG_DIR", dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("No project specified"));
}

#[test]
fn up_with_no_roles() {
    let dir = temp_sipag_dir();
    let project_dir = dir.path().join("projects/testproj");
    fs::create_dir_all(project_dir.join("tasks")).unwrap();
    fs::create_dir_all(project_dir.join("roles")).unwrap();
    fs::write(
        project_dir.join("project.toml"),
        "name = \"testproj\"\nrepo = \"a/b\"\nstatuses = [\"todo\", \"done\"]\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("config.toml"),
        "default_project = \"testproj\"\n",
    )
    .unwrap();

    sipag()
        .arg("up")
        .env("SIPAG_DIR", dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("No roles configured"));
}

// The `feature_*` and `refine_*` smoke tests (and their `setup_project`
// helper) were removed 2026-05-17 with the rest of the deprecated
// refinement wiring. See sipag_core::{feature, refine} module
// doc-comments and docs/modules.md §3.

// ── Unknown subcommand ──────────────────────────────────────────────────────

#[test]
fn unknown_subcommand_fails() {
    sipag()
        .arg("nonexistent-command")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand"));
}
