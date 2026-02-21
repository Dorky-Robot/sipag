use anyhow::Result;
use std::path::PathBuf;

use crate::task::{TaskFile, TaskStatus};

/// Parse task file content (pure — no I/O).
///
/// `name` is the logical task name (typically the file stem).
pub fn parse_task_content(content: &str, name: &str, status: TaskStatus) -> Result<TaskFile> {
    let mut repo: Option<String> = None;
    let mut priority = "medium".to_string();
    let mut source: Option<String> = None;
    let mut added: Option<String> = None;
    let mut started: Option<String> = None;
    let mut ended: Option<String> = None;
    let mut container: Option<String> = None;
    let mut issue: Option<String> = None;

    let lines: Vec<&str> = content.lines().collect();
    let n = lines.len();
    let mut i = 0;

    // Parse optional YAML frontmatter (--- ... ---)
    if n > 0 && lines[0] == "---" {
        i = 1;
        while i < n {
            if lines[i] == "---" {
                i += 1;
                break;
            }
            if let Some((key, val)) = lines[i].split_once(": ") {
                match key.trim() {
                    "repo" => repo = Some(val.to_string()),
                    "priority" => priority = val.to_string(),
                    "source" => source = Some(val.to_string()),
                    "added" => added = Some(val.to_string()),
                    "started" => started = Some(val.to_string()),
                    "ended" => ended = Some(val.to_string()),
                    "container" => container = Some(val.to_string()),
                    "issue" => issue = Some(val.to_string()),
                    _ => {}
                }
            }
            i += 1;
        }
    }

    // Find title: first non-empty line after frontmatter
    let mut title = String::new();
    while i < n {
        if !lines[i].is_empty() {
            title = lines[i].to_string();
            i += 1;
            break;
        }
        i += 1;
    }

    // Collect remaining lines as body, trimming leading/trailing blank lines
    let remaining = &lines[i..];
    let start = remaining
        .iter()
        .position(|l| !l.is_empty())
        .unwrap_or(remaining.len());
    let end = remaining
        .iter()
        .rposition(|l| !l.is_empty())
        .map(|p| p + 1)
        .unwrap_or(0);
    let body = if start < end {
        remaining[start..end].join("\n")
    } else {
        String::new()
    };

    Ok(TaskFile {
        name: name.to_string(),
        repo,
        priority,
        source,
        added,
        started,
        ended,
        container,
        issue,
        title,
        body,
        status,
        file_path: PathBuf::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_content_with_frontmatter() {
        let content = "---\nrepo: myrepo\npriority: high\n---\nFix the bug\nsome body text\n";
        let task = parse_task_content(content, "001-test", TaskStatus::Queue).unwrap();
        assert_eq!(task.name, "001-test");
        assert_eq!(task.repo, Some("myrepo".to_string()));
        assert_eq!(task.priority, "high");
        assert_eq!(task.title, "Fix the bug");
        assert_eq!(task.body, "some body text");
        assert_eq!(task.status, TaskStatus::Queue);
    }

    #[test]
    fn test_parse_content_without_frontmatter() {
        let content = "Just a plain task\nwith some body\n";
        let task = parse_task_content(content, "plain", TaskStatus::Queue).unwrap();
        assert_eq!(task.title, "Just a plain task");
        assert_eq!(task.body, "with some body");
        assert_eq!(task.priority, "medium");
    }

    #[test]
    fn test_parse_content_all_frontmatter_fields() {
        let content = "---\nrepo: myrepo\npriority: high\nsource: github#42\nadded: 2024-01-01T00:00:00Z\n---\nTitle\n";
        let task = parse_task_content(content, "task", TaskStatus::Queue).unwrap();
        assert_eq!(task.source, Some("github#42".to_string()));
        assert_eq!(task.added, Some("2024-01-01T00:00:00Z".to_string()));
    }

    #[test]
    fn test_parse_tracking_content() {
        let content = "---\nrepo: https://github.com/org/repo\nissue: 21\nstarted: 2024-01-01T12:00:00Z\ncontainer: sipag-20240101120000-fix-bug\n---\nFix the bug\n";
        let task =
            parse_task_content(content, "20240101120000-fix-bug", TaskStatus::Running).unwrap();
        assert_eq!(task.repo, Some("https://github.com/org/repo".to_string()));
        assert_eq!(task.issue, Some("21".to_string()));
        assert_eq!(task.started, Some("2024-01-01T12:00:00Z".to_string()));
        assert_eq!(task.status, TaskStatus::Running);
    }

    #[test]
    fn test_parse_content_empty_body() {
        let content = "---\nrepo: repo\npriority: medium\n---\nTitle only\n";
        let task = parse_task_content(content, "task", TaskStatus::Queue).unwrap();
        assert_eq!(task.title, "Title only");
        assert_eq!(task.body, "");
    }

    #[test]
    fn test_parse_content_body_trimmed() {
        let content = "Title\n\nBody line\n\n";
        let task = parse_task_content(content, "task", TaskStatus::Queue).unwrap();
        assert_eq!(task.title, "Title");
        assert_eq!(task.body, "Body line");
    }
}
