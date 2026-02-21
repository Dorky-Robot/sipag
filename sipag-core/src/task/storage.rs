use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::task::{TaskFile, TaskStatus};
use crate::task::naming::slugify;
use crate::task::parser::parse_task_content;

/// Read a task file from disk and parse its content.
pub fn read_task_file(path: &Path, status: TaskStatus) -> Result<TaskFile> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    parse_task_content(&content, &name, status)
}

/// Write a task file with YAML frontmatter.
///
/// `added` is the ISO-8601 timestamp string (caller supplies via `chrono::Utc::now()`).
pub fn write_task_file(
    path: &Path,
    title: &str,
    repo: &str,
    priority: &str,
    source: Option<&str>,
    added: &str,
) -> Result<()> {
    let mut content = format!("---\nrepo: {repo}\npriority: {priority}\n");
    if let Some(src) = source {
        content.push_str(&format!("source: {src}\n"));
    }
    content.push_str(&format!("added: {added}\n---\n{title}\n"));
    fs::write(path, content).with_context(|| format!("Failed to write {}", path.display()))
}

/// Write a tracking file for a running task (used by sipag run).
///
/// `started` is the ISO-8601 timestamp string (caller supplies via `chrono::Utc::now()`).
pub fn write_tracking_file(
    path: &Path,
    repo_url: &str,
    issue: Option<&str>,
    container: &str,
    description: &str,
    started: &str,
) -> Result<()> {
    let mut content = format!("---\nrepo: {repo_url}\n");
    if let Some(iss) = issue {
        content.push_str(&format!("issue: {iss}\n"));
    }
    content.push_str(&format!(
        "started: {started}\ncontainer: {container}\n---\n{description}\n"
    ));
    fs::write(path, content).with_context(|| format!("Failed to write {}", path.display()))
}

/// Append "ended: <timestamp>" to an existing tracking file.
///
/// `ended` is the ISO-8601 timestamp string (caller supplies via `chrono::Utc::now()`).
pub fn append_ended(path: &Path, ended: &str) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(path)
        .with_context(|| format!("Failed to open {}", path.display()))?;
    writeln!(file, "ended: {ended}")?;
    Ok(())
}

/// Generate next sequential filename for a queue directory.
/// Pattern: NNN-slug.md (e.g. 001-fix-bug.md)
pub fn next_filename(queue_dir: &Path, title: &str) -> String {
    let slug = slugify(title);
    let mut max_num: u32 = 0;

    if let Ok(entries) = fs::read_dir(queue_dir) {
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let s = file_name.to_string_lossy();
            if s.ends_with(".md") {
                // Extract leading digits before the first '-'
                if let Some(num_str) = s.split('-').next() {
                    if let Ok(num) = num_str.parse::<u32>() {
                        if num > max_num {
                            max_num = num;
                        }
                    }
                }
            }
        }
    }

    format!("{:03}-{}.md", max_num + 1, slug)
}

/// List all task files across queue/, running/, done/, failed/ directories.
pub fn list_tasks(sipag_dir: &Path) -> Result<Vec<TaskFile>> {
    let mut tasks = Vec::new();
    let statuses = [
        (TaskStatus::Queue, "queue"),
        (TaskStatus::Running, "running"),
        (TaskStatus::Done, "done"),
        (TaskStatus::Failed, "failed"),
    ];

    for (status, subdir) in &statuses {
        let dir = sipag_dir.join(subdir);
        if !dir.exists() {
            continue;
        }
        let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
            .with_context(|| format!("Failed to read {}", dir.display()))?
            .flatten()
            .filter(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
            .map(|e| e.path())
            .collect();
        paths.sort();
        for path in paths {
            if let Ok(task) = read_task_file(&path, status.clone()) {
                tasks.push(task);
            }
        }
    }

    Ok(tasks)
}

/// Return the default sipag data directory (~/.sipag or $SIPAG_DIR).
pub fn default_sipag_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("SIPAG_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".sipag");
    }
    PathBuf::from(".sipag")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::init_dirs;
    use tempfile::TempDir;

    #[test]
    fn test_read_task_file_with_frontmatter() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("001-test.md");
        fs::write(
            &path,
            "---\nrepo: myrepo\npriority: high\n---\nFix the bug\nsome body text\n",
        )
        .unwrap();
        let task = read_task_file(&path, TaskStatus::Queue).unwrap();
        assert_eq!(task.name, "001-test");
        assert_eq!(task.repo, Some("myrepo".to_string()));
        assert_eq!(task.priority, "high");
        assert_eq!(task.title, "Fix the bug");
        assert_eq!(task.body, "some body text");
        assert_eq!(task.status, TaskStatus::Queue);
    }

    #[test]
    fn test_read_task_file_without_frontmatter() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("plain.md");
        fs::write(&path, "Just a plain task\nwith some body\n").unwrap();
        let task = read_task_file(&path, TaskStatus::Queue).unwrap();
        assert_eq!(task.title, "Just a plain task");
        assert_eq!(task.body, "with some body");
        assert_eq!(task.priority, "medium");
    }

    #[test]
    fn test_read_task_file_all_frontmatter_fields() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("task.md");
        fs::write(
            &path,
            "---\nrepo: myrepo\npriority: high\nsource: github#42\nadded: 2024-01-01T00:00:00Z\n---\nTitle\n",
        )
        .unwrap();
        let task = read_task_file(&path, TaskStatus::Queue).unwrap();
        assert_eq!(task.source, Some("github#42".to_string()));
        assert_eq!(task.added, Some("2024-01-01T00:00:00Z".to_string()));
    }

    #[test]
    fn test_read_tracking_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("20240101120000-fix-bug.md");
        fs::write(
            &path,
            "---\nrepo: https://github.com/org/repo\nissue: 21\nstarted: 2024-01-01T12:00:00Z\ncontainer: sipag-20240101120000-fix-bug\n---\nFix the bug\n",
        )
        .unwrap();
        let task = read_task_file(&path, TaskStatus::Running).unwrap();
        assert_eq!(task.repo, Some("https://github.com/org/repo".to_string()));
        assert_eq!(task.issue, Some("21".to_string()));
        assert_eq!(task.started, Some("2024-01-01T12:00:00Z".to_string()));
        assert_eq!(task.status, TaskStatus::Running);
    }

    #[test]
    fn test_next_filename_empty_dir() {
        let dir = TempDir::new().unwrap();
        let filename = next_filename(dir.path(), "Fix the bug");
        assert_eq!(filename, "001-fix-the-bug.md");
    }

    #[test]
    fn test_next_filename_with_existing() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("001-first.md"), "").unwrap();
        fs::write(dir.path().join("002-second.md"), "").unwrap();
        let filename = next_filename(dir.path(), "Third task");
        assert_eq!(filename, "003-third-task.md");
    }

    #[test]
    fn test_next_filename_nonsequential() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("005-jump.md"), "").unwrap();
        let filename = next_filename(dir.path(), "Next task");
        assert_eq!(filename, "006-next-task.md");
    }

    #[test]
    fn test_write_and_read_task_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("task.md");
        write_task_file(&path, "My Task", "myrepo", "high", None, "2024-01-01T00:00:00Z")
            .unwrap();
        let task = read_task_file(&path, TaskStatus::Queue).unwrap();
        assert_eq!(task.title, "My Task");
        assert_eq!(task.repo, Some("myrepo".to_string()));
        assert_eq!(task.priority, "high");
        assert_eq!(task.added, Some("2024-01-01T00:00:00Z".to_string()));
    }

    #[test]
    fn test_write_task_file_with_source() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("task.md");
        write_task_file(
            &path,
            "My Task",
            "myrepo",
            "medium",
            Some("github#42"),
            "2024-01-01T00:00:00Z",
        )
        .unwrap();
        let task = read_task_file(&path, TaskStatus::Queue).unwrap();
        assert_eq!(task.source, Some("github#42".to_string()));
    }

    #[test]
    fn test_list_tasks() {
        let dir = TempDir::new().unwrap();
        init_dirs(dir.path()).unwrap();
        write_task_file(
            &dir.path().join("queue/001-task.md"),
            "Task 1",
            "repo1",
            "medium",
            None,
            "2024-01-01T00:00:00Z",
        )
        .unwrap();
        write_task_file(
            &dir.path().join("queue/002-task.md"),
            "Task 2",
            "repo2",
            "high",
            None,
            "2024-01-01T00:00:00Z",
        )
        .unwrap();
        let tasks = list_tasks(dir.path()).unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].status, TaskStatus::Queue);
    }
}
