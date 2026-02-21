use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Status of a task based on which directory it lives in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Queue,
    Running,
    Done,
    Failed,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Queue => "queue",
            TaskStatus::Running => "running",
            TaskStatus::Done => "done",
            TaskStatus::Failed => "failed",
        }
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A task file with optional YAML frontmatter.
#[derive(Debug, Clone)]
pub struct TaskFile {
    pub name: String,
    pub repo: Option<String>,
    pub priority: String,
    pub source: Option<String>,
    pub added: Option<String>,
    pub started: Option<String>,
    pub ended: Option<String>,
    pub container: Option<String>,
    pub issue: Option<String>,
    pub title: String,
    pub body: String,
    pub status: TaskStatus,
}

/// Parse a task file with optional YAML frontmatter.
pub fn parse_task_file(path: &Path, status: TaskStatus) -> Result<TaskFile> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;

    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

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
        name,
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
    })
}

/// Convert text to a URL-safe slug (lowercase, hyphens only).
pub fn slugify(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut slug = String::new();
    let mut prev_hyphen = false;

    for c in lower.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
            prev_hyphen = false;
        } else if !prev_hyphen {
            slug.push('-');
            prev_hyphen = true;
        }
    }

    slug.trim_matches('-').to_string()
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

/// Write a task file with YAML frontmatter.
pub fn write_task_file(
    path: &Path,
    title: &str,
    repo: &str,
    priority: &str,
    source: Option<&str>,
) -> Result<()> {
    let added = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    let mut content = format!("---\nrepo: {repo}\npriority: {priority}\n");
    if let Some(src) = source {
        content.push_str(&format!("source: {src}\n"));
    }
    content.push_str(&format!("added: {added}\n---\n{title}\n"));
    fs::write(path, content).with_context(|| format!("Failed to write {}", path.display()))
}

/// Write a tracking file for a running task (used by sipag run).
pub fn write_tracking_file(
    path: &Path,
    repo_url: &str,
    issue: Option<&str>,
    container: &str,
    description: &str,
) -> Result<()> {
    let started = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
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
pub fn append_ended(path: &Path) -> Result<()> {
    let ended = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(path)
        .with_context(|| format!("Failed to open {}", path.display()))?;
    writeln!(file, "ended: {ended}")?;
    Ok(())
}

/// Create the sipag directory structure (idempotent).
pub fn init_dirs(base: &Path) -> Result<()> {
    let mut created = false;
    for subdir in &["queue", "running", "done", "failed"] {
        let dir = base.join(subdir);
        if !dir.exists() {
            fs::create_dir_all(&dir)
                .with_context(|| format!("Failed to create {}", dir.display()))?;
            println!("Created: {}", dir.display());
            created = true;
        }
    }
    if created {
        println!("Initialized: {}", base.display());
    } else {
        println!("Already initialized: {}", base.display());
    }
    Ok(())
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
            if let Ok(task) = parse_task_file(&path, status.clone()) {
                tasks.push(task);
            }
        }
    }

    Ok(tasks)
}

/// A single item in a markdown checklist file.
#[derive(Debug, Clone)]
pub struct ChecklistItem {
    pub line_num: usize, // 1-indexed
    pub title: String,
    pub body: String,
    pub done: bool,
}

/// Parse all checklist items from a markdown file with `- [ ]` / `- [x]` format.
pub fn parse_checklist(path: &Path) -> Result<Vec<ChecklistItem>> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;

    let mut items = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let n = lines.len();
    let mut i = 0;

    while i < n {
        let line = lines[i];
        if let Some(stripped) = line.strip_prefix("- [ ] ") {
            let title = stripped.to_string();
            let line_num = i + 1;
            // Collect indented body lines (2+ spaces)
            let mut body_lines = Vec::new();
            i += 1;
            while i < n {
                let next = lines[i];
                if next.starts_with("  ") || next.starts_with('\t') {
                    body_lines.push(next.trim().to_string());
                    i += 1;
                } else {
                    break;
                }
            }
            let body = body_lines.join("\n");
            items.push(ChecklistItem {
                line_num,
                title,
                body,
                done: false,
            });
        } else if let Some(stripped) = line.strip_prefix("- [x] ") {
            items.push(ChecklistItem {
                line_num: i + 1,
                title: stripped.to_string(),
                body: String::new(),
                done: true,
            });
            i += 1;
        } else {
            i += 1;
        }
    }

    Ok(items)
}

/// Find the first pending (unchecked) checklist item in a file.
pub fn next_checklist_item(path: &Path) -> Result<Option<ChecklistItem>> {
    let items = parse_checklist(path)?;
    Ok(items.into_iter().find(|item| !item.done))
}

/// Mark a checklist item as done at the given 1-indexed line number.
pub fn mark_checklist_done(path: &Path, line_num: usize) -> Result<()> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();

    let idx = line_num.saturating_sub(1);
    if idx < lines.len() {
        lines[idx] = lines[idx].replacen("- [ ] ", "- [x] ", 1);
    }

    fs::write(path, lines.join("\n") + "\n")
        .with_context(|| format!("Failed to write {}", path.display()))
}

/// Append a new pending checklist item to a file (creates file if missing).
pub fn append_checklist_item(path: &Path, title: &str) -> Result<()> {
    let line = format!("- [ ] {title}\n");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("Failed to open {}", path.display()))?;
    file.write_all(line.as_bytes())?;
    Ok(())
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

/// Return the default task file path (./tasks.md or $SIPAG_FILE).
pub fn default_sipag_file() -> PathBuf {
    if let Ok(f) = std::env::var("SIPAG_FILE") {
        return PathBuf::from(f);
    }
    PathBuf::from("tasks.md")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_slugify_basic() {
        assert_eq!(slugify("Hello World"), "hello-world");
    }

    #[test]
    fn test_slugify_special_chars() {
        assert_eq!(slugify("Fix Bug #1!"), "fix-bug-1");
    }

    #[test]
    fn test_slugify_multiple_separators() {
        assert_eq!(slugify("hello   world"), "hello-world");
    }

    #[test]
    fn test_slugify_leading_trailing() {
        assert_eq!(slugify("  hello  "), "hello");
    }

    #[test]
    fn test_slugify_already_slug() {
        assert_eq!(slugify("fix-bug"), "fix-bug");
    }

    #[test]
    fn test_parse_task_file_with_frontmatter() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("001-test.md");
        fs::write(
            &path,
            "---\nrepo: myrepo\npriority: high\n---\nFix the bug\nsome body text\n",
        )
        .unwrap();
        let task = parse_task_file(&path, TaskStatus::Queue).unwrap();
        assert_eq!(task.name, "001-test");
        assert_eq!(task.repo, Some("myrepo".to_string()));
        assert_eq!(task.priority, "high");
        assert_eq!(task.title, "Fix the bug");
        assert_eq!(task.body, "some body text");
        assert_eq!(task.status, TaskStatus::Queue);
    }

    #[test]
    fn test_parse_task_file_without_frontmatter() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("plain.md");
        fs::write(&path, "Just a plain task\nwith some body\n").unwrap();
        let task = parse_task_file(&path, TaskStatus::Queue).unwrap();
        assert_eq!(task.title, "Just a plain task");
        assert_eq!(task.body, "with some body");
        assert_eq!(task.priority, "medium");
    }

    #[test]
    fn test_parse_task_file_all_frontmatter_fields() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("task.md");
        fs::write(
            &path,
            "---\nrepo: myrepo\npriority: high\nsource: github#42\nadded: 2024-01-01T00:00:00Z\n---\nTitle\n",
        )
        .unwrap();
        let task = parse_task_file(&path, TaskStatus::Queue).unwrap();
        assert_eq!(task.source, Some("github#42".to_string()));
        assert_eq!(task.added, Some("2024-01-01T00:00:00Z".to_string()));
    }

    #[test]
    fn test_parse_tracking_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("20240101120000-fix-bug.md");
        fs::write(
            &path,
            "---\nrepo: https://github.com/org/repo\nissue: 21\nstarted: 2024-01-01T12:00:00Z\ncontainer: sipag-20240101120000-fix-bug\n---\nFix the bug\n",
        )
        .unwrap();
        let task = parse_task_file(&path, TaskStatus::Running).unwrap();
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
    fn test_init_dirs() {
        let dir = TempDir::new().unwrap();
        init_dirs(dir.path()).unwrap();
        assert!(dir.path().join("queue").exists());
        assert!(dir.path().join("running").exists());
        assert!(dir.path().join("done").exists());
        assert!(dir.path().join("failed").exists());
    }

    #[test]
    fn test_init_dirs_idempotent() {
        let dir = TempDir::new().unwrap();
        init_dirs(dir.path()).unwrap();
        // Should not fail on second call
        init_dirs(dir.path()).unwrap();
    }

    #[test]
    fn test_write_and_parse_task_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("task.md");
        write_task_file(&path, "My Task", "myrepo", "high", None).unwrap();
        let task = parse_task_file(&path, TaskStatus::Queue).unwrap();
        assert_eq!(task.title, "My Task");
        assert_eq!(task.repo, Some("myrepo".to_string()));
        assert_eq!(task.priority, "high");
        assert!(task.added.is_some());
    }

    #[test]
    fn test_write_task_file_with_source() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("task.md");
        write_task_file(&path, "My Task", "myrepo", "medium", Some("github#42")).unwrap();
        let task = parse_task_file(&path, TaskStatus::Queue).unwrap();
        assert_eq!(task.source, Some("github#42".to_string()));
    }

    #[test]
    fn test_parse_checklist() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tasks.md");
        fs::write(
            &path,
            "- [x] Done task\n- [ ] Pending task\n  some body\n- [ ] Another task\n",
        )
        .unwrap();
        let items = parse_checklist(&path).unwrap();
        assert_eq!(items.len(), 3);
        assert!(items[0].done);
        assert_eq!(items[0].title, "Done task");
        assert!(!items[1].done);
        assert_eq!(items[1].title, "Pending task");
        assert_eq!(items[1].body, "some body");
        assert!(!items[2].done);
    }

    #[test]
    fn test_next_checklist_item() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tasks.md");
        fs::write(
            &path,
            "- [x] Done task\n- [ ] First pending\n- [ ] Second pending\n",
        )
        .unwrap();
        let item = next_checklist_item(&path).unwrap().unwrap();
        assert_eq!(item.title, "First pending");
        assert_eq!(item.line_num, 2);
    }

    #[test]
    fn test_mark_checklist_done() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tasks.md");
        fs::write(&path, "- [x] Done\n- [ ] Pending\n- [ ] Another\n").unwrap();
        mark_checklist_done(&path, 2).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("- [x] Pending"));
        assert!(content.contains("- [ ] Another"));
    }

    #[test]
    fn test_append_checklist_item() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tasks.md");
        append_checklist_item(&path, "New task").unwrap();
        append_checklist_item(&path, "Another task").unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("- [ ] New task"));
        assert!(content.contains("- [ ] Another task"));
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
        )
        .unwrap();
        write_task_file(
            &dir.path().join("queue/002-task.md"),
            "Task 2",
            "repo2",
            "high",
            None,
        )
        .unwrap();
        let tasks = list_tasks(dir.path()).unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].status, TaskStatus::Queue);
    }
}
