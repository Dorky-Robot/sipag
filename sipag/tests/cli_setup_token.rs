//! Integration tests for the `sipag setup-token` CLI subcommand.
//!
//! The CLI mints a single-use token, persists it under
//! `<sipag_dir>/setup-tokens/`, and prints a URL the user can open
//! on a trusted device to enroll their first passkey.

use assert_cmd::Command;
use std::fs;
use tempfile::TempDir;

#[allow(deprecated)] // matches the project's existing cli_smoke.rs pattern; cargo_bin still works.
fn sipag_cmd(sipag_dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("sipag").unwrap();
    cmd.env("SIPAG_DIR", sipag_dir.path());
    cmd
}

#[test]
fn setup_token_prints_url_with_token_param() {
    let dir = TempDir::new().unwrap();
    let assert = sipag_cmd(&dir)
        .args(["setup-token"])
        .env("SIPAG_PUBLIC_URL", "http://localhost:7100")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("/setup?token="),
        "expected /setup?token=… in stdout; got:\n{stdout}"
    );
}

#[test]
fn setup_token_url_uses_sipag_public_url_env() {
    let dir = TempDir::new().unwrap();
    let assert = sipag_cmd(&dir)
        .args(["setup-token"])
        .env("SIPAG_PUBLIC_URL", "https://sipag.felixflor.es")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("https://sipag.felixflor.es/setup?token="),
        "expected env var to set URL prefix; got:\n{stdout}"
    );
}

#[test]
fn setup_token_falls_back_to_localhost_default_when_env_missing() {
    let dir = TempDir::new().unwrap();
    let mut cmd = sipag_cmd(&dir);
    cmd.env_remove("SIPAG_PUBLIC_URL");
    let assert = cmd.args(["setup-token"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("http://localhost:7100/setup?token="),
        "expected localhost default; got:\n{stdout}"
    );
}

#[test]
fn setup_token_persists_one_file_to_disk() {
    let dir = TempDir::new().unwrap();
    sipag_cmd(&dir)
        .args(["setup-token"])
        .env("SIPAG_PUBLIC_URL", "http://localhost:7100")
        .assert()
        .success();

    let setup_dir = dir.path().join("setup-tokens");
    assert!(
        setup_dir.exists(),
        "setup-tokens/ should be created"
    );
    let entries: Vec<_> = fs::read_dir(&setup_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s == "toml")
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "expected exactly one .toml file under setup-tokens/"
    );
}

#[test]
fn setup_token_each_invocation_mints_a_distinct_token() {
    let dir = TempDir::new().unwrap();
    let assert1 = sipag_cmd(&dir)
        .args(["setup-token"])
        .assert()
        .success();
    let assert2 = sipag_cmd(&dir)
        .args(["setup-token"])
        .assert()
        .success();

    let out1 = String::from_utf8(assert1.get_output().stdout.clone()).unwrap();
    let out2 = String::from_utf8(assert2.get_output().stdout.clone()).unwrap();
    assert_ne!(
        out1, out2,
        "two consecutive invocations should mint distinct tokens"
    );

    // Both should land on disk.
    let setup_dir = dir.path().join("setup-tokens");
    let count = fs::read_dir(&setup_dir).unwrap().count();
    assert_eq!(count, 2, "expected 2 token files after 2 invocations");
}
