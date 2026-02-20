use std::fs;
use std::process::Command;
use tempfile::TempDir;

/// Path to the compiled sipag binary.
fn sipag_bin() -> &'static str {
    env!("CARGO_BIN_EXE_sipag")
}

/// Run the sipag binary with a given SIPAG_DIR and arguments, returning its output.
fn run(sipag_dir: &str, args: &[&str]) -> std::process::Output {
    Command::new(sipag_bin())
        .env("SIPAG_DIR", sipag_dir)
        // Avoid accidentally picking up a real token in tests
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .args(args)
        .output()
        .expect("failed to run sipag")
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

// ── version ────────────────────────────────────────────────────────────────

#[test]
fn test_version_subcommand() {
    let tmp = TempDir::new().unwrap();
    let out = run(tmp.path().to_str().unwrap(), &["version"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("sipag"));
}

#[test]
fn test_version_flag() {
    let tmp = TempDir::new().unwrap();
    // clap --version flag
    let out = Command::new(sipag_bin())
        .env("SIPAG_DIR", tmp.path())
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(stdout(&out).contains("sipag"));
}

// ── help ───────────────────────────────────────────────────────────────────

#[test]
fn test_help_flag() {
    let tmp = TempDir::new().unwrap();
    let out = Command::new(sipag_bin())
        .env("SIPAG_DIR", tmp.path())
        .arg("--help")
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = stdout(&out);
    assert!(s.contains("sipag"), "expected 'sipag' in help: {}", s);
}

// ── init ───────────────────────────────────────────────────────────────────

#[test]
fn test_init_creates_directories() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    let out = run(dir, &["init"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    for subdir in &["queue", "running", "done", "failed"] {
        assert!(
            tmp.path().join(subdir).is_dir(),
            "missing directory: {}",
            subdir
        );
    }
}

#[test]
fn test_init_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(dir, &["init"]);
    let out = run(dir, &["init"]);

    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("Already initialized"),
        "expected 'Already initialized', got: {}",
        stdout(&out)
    );
}

// ── repo ───────────────────────────────────────────────────────────────────

#[test]
fn test_repo_add_and_list() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(dir, &["init"]);

    let out = run(
        dir,
        &["repo", "add", "myrepo", "https://github.com/org/repo"],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("Registered"));

    let out = run(dir, &["repo", "list"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("myrepo=https://github.com/org/repo"),
        "got: {}",
        stdout(&out)
    );
}

#[test]
fn test_repo_add_duplicate_fails() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(dir, &["init"]);
    run(dir, &["repo", "add", "myrepo", "https://github.com/org/repo"]);

    let out = run(
        dir,
        &["repo", "add", "myrepo", "https://github.com/org/repo"],
    );
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("already exists"),
        "got: {}",
        stderr(&out)
    );
}

#[test]
fn test_repo_list_empty() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(dir, &["init"]);

    let out = run(dir, &["repo", "list"]);
    assert!(out.status.success());
    assert!(stdout(&out).contains("No repos registered"));
}

// ── add ────────────────────────────────────────────────────────────────────

#[test]
fn test_add_with_repo_creates_queue_file() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    let out = run(dir, &["add", "Fix the authentication bug", "--repo", "myrepo"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("Added: Fix the authentication bug"));

    let queue_dir = tmp.path().join("queue");
    assert!(queue_dir.is_dir());

    let entries: Vec<_> = fs::read_dir(&queue_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1, "expected 1 queue file");

    let file_path = entries[0].path();
    let content = fs::read_to_string(&file_path).unwrap();
    assert!(content.contains("repo: myrepo"), "content: {}", content);
    assert!(
        content.contains("Fix the authentication bug"),
        "content: {}",
        content
    );
    assert!(content.contains("priority: medium"), "content: {}", content);
}

#[test]
fn test_add_with_priority() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(
        dir,
        &[
            "add",
            "High priority task",
            "--repo",
            "myrepo",
            "--priority",
            "high",
        ],
    );

    let queue_dir = tmp.path().join("queue");
    let entries: Vec<_> = fs::read_dir(&queue_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    let content = fs::read_to_string(entries[0].path()).unwrap();
    assert!(content.contains("priority: high"), "content: {}", content);
}

#[test]
fn test_add_sequential_filenames() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(dir, &["add", "First task", "--repo", "repo1"]);
    run(dir, &["add", "Second task", "--repo", "repo1"]);
    run(dir, &["add", "Third task", "--repo", "repo1"]);

    let queue_dir = tmp.path().join("queue");
    let mut entries: Vec<String> = fs::read_dir(&queue_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    entries.sort();

    assert_eq!(entries.len(), 3);
    assert!(entries[0].starts_with("001-"), "got: {}", entries[0]);
    assert!(entries[1].starts_with("002-"), "got: {}", entries[1]);
    assert!(entries[2].starts_with("003-"), "got: {}", entries[2]);
}

#[test]
fn test_add_without_repo_uses_task_file() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();
    let task_file = tmp.path().join("tasks.md");

    let out = Command::new(sipag_bin())
        .env("SIPAG_DIR", dir)
        .env("SIPAG_FILE", task_file.to_str().unwrap())
        .args(["add", "A simple task"])
        .output()
        .unwrap();

    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(task_file.exists(), "task file not created");

    let content = fs::read_to_string(&task_file).unwrap();
    assert!(
        content.contains("- [ ] A simple task"),
        "content: {}",
        content
    );
}

// ── status ─────────────────────────────────────────────────────────────────

#[test]
fn test_status_empty() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(dir, &["init"]);

    let out = run(dir, &["status"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    // Empty dirs are not printed, so output may be empty (that's OK)
}

#[test]
fn test_status_shows_queue_items() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(dir, &["init"]);
    run(dir, &["add", "My task", "--repo", "myrepo"]);

    let out = run(dir, &["status"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let s = stdout(&out);
    assert!(s.contains("Queue"), "expected 'Queue' in output: {}", s);
}

// ── show ───────────────────────────────────────────────────────────────────

#[test]
fn test_show_task_in_queue() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(dir, &["init"]);

    let content = "---\nrepo: myrepo\npriority: medium\nadded: 2026-02-20T00:00:00Z\n---\nFix the bug\n";
    fs::write(tmp.path().join("queue").join("001-fix-the-bug.md"), content).unwrap();

    let out = run(dir, &["show", "001-fix-the-bug"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let s = stdout(&out);
    assert!(s.contains("001-fix-the-bug"), "output: {}", s);
    assert!(s.contains("Fix the bug"), "output: {}", s);
    assert!(s.contains("queue"), "output: {}", s);
}

#[test]
fn test_show_includes_log() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(dir, &["init"]);

    let md_content =
        "---\nrepo: myrepo\npriority: medium\nadded: 2026-02-20T00:00:00Z\n---\nDone task\n";
    fs::write(
        tmp.path().join("done").join("001-done-task.md"),
        md_content,
    )
    .unwrap();
    fs::write(
        tmp.path().join("done").join("001-done-task.log"),
        "Task completed successfully\n",
    )
    .unwrap();

    let out = run(dir, &["show", "001-done-task"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let s = stdout(&out);
    assert!(s.contains("Task completed successfully"), "output: {}", s);
    assert!(s.contains("=== Log ==="), "output: {}", s);
}

#[test]
fn test_show_not_found() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(dir, &["init"]);

    let out = run(dir, &["show", "nonexistent"]);
    assert!(!out.status.success());
}

// ── retry ──────────────────────────────────────────────────────────────────

#[test]
fn test_retry_moves_failed_to_queue() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(dir, &["init"]);

    let content = "---\nrepo: myrepo\npriority: medium\nadded: 2026-02-20T00:00:00Z\n---\nFailed task\n";
    let failed_file = tmp.path().join("failed").join("001-failed-task.md");
    fs::write(&failed_file, content).unwrap();

    let out = run(dir, &["retry", "001-failed-task"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("Retrying"),
        "output: {}",
        stdout(&out)
    );

    assert!(!failed_file.exists(), "failed file should be gone");
    assert!(
        tmp.path().join("queue").join("001-failed-task.md").exists(),
        "queue file should exist"
    );
}

#[test]
fn test_retry_removes_log() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(dir, &["init"]);

    let content = "---\nrepo: myrepo\npriority: medium\n---\nTask\n";
    let failed_md = tmp.path().join("failed").join("001-task.md");
    let failed_log = tmp.path().join("failed").join("001-task.log");
    fs::write(&failed_md, content).unwrap();
    fs::write(&failed_log, "old log\n").unwrap();

    run(dir, &["retry", "001-task"]);

    assert!(!failed_log.exists(), "log should be removed after retry");
}

#[test]
fn test_retry_not_found() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(dir, &["init"]);

    let out = run(dir, &["retry", "nonexistent"]);
    assert!(!out.status.success());
}

// ── ps ─────────────────────────────────────────────────────────────────────

#[test]
fn test_ps_empty() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(dir, &["init"]);

    let out = run(dir, &["ps"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let s = stdout(&out);
    // Should print the header and "No tasks found"
    assert!(s.contains("ID"), "output: {}", s);
    assert!(s.contains("No tasks found"), "output: {}", s);
}

#[test]
fn test_ps_shows_done_task() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(dir, &["init"]);

    // Create a tracking file in done/
    let content = "---\nrepo: https://github.com/org/repo\nstarted: 2026-02-20T10:00:00Z\nended: 2026-02-20T10:05:00Z\ncontainer: sipag-test-task\n---\nMy task\n";
    fs::write(tmp.path().join("done").join("test-task.md"), content).unwrap();

    let out = run(dir, &["ps"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let s = stdout(&out);
    assert!(s.contains("test-task"), "output: {}", s);
    assert!(s.contains("done"), "output: {}", s);
    assert!(s.contains("5m0s"), "expected 5m0s duration, output: {}", s);
}

// ── logs ───────────────────────────────────────────────────────────────────

#[test]
fn test_logs_not_found() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(dir, &["init"]);

    let out = run(dir, &["logs", "nonexistent"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("no log found"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn test_logs_from_done() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(dir, &["init"]);

    let log_content = "Task completed successfully\nPR created at https://github.com/org/repo/pull/1\n";
    fs::write(
        tmp.path().join("done").join("my-task.log"),
        log_content,
    )
    .unwrap();

    let out = run(dir, &["logs", "my-task"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("Task completed successfully"),
        "output: {}",
        stdout(&out)
    );
}

#[test]
fn test_logs_from_running() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(dir, &["init"]);

    fs::write(
        tmp.path().join("running").join("active-task.log"),
        "Still running...\n",
    )
    .unwrap();

    let out = run(dir, &["logs", "active-task"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("Still running"),
        "output: {}",
        stdout(&out)
    );
}

// ── slugify ────────────────────────────────────────────────────────────────

#[test]
fn test_slugify_via_add() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    run(dir, &["add", "Fix User Authentication Bug!", "--repo", "r"]);

    let queue_dir = tmp.path().join("queue");
    let mut entries: Vec<String> = fs::read_dir(&queue_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    entries.sort();

    assert_eq!(entries.len(), 1);
    assert!(
        entries[0].contains("fix-user-authentication-bug"),
        "filename: {}",
        entries[0]
    );
}

// ── start (queue empty) ────────────────────────────────────────────────────

#[test]
fn test_start_empty_queue() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().to_str().unwrap();

    // start with empty queue should report no tasks without error
    let out = run(dir, &["start"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(
        s.contains("No tasks in queue"),
        "expected 'No tasks in queue', got: {}",
        s
    );
}
