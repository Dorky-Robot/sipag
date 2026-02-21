use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
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

    /// Single-character symbol for display (used by TUI).
    pub fn symbol(&self) -> &'static str {
        match self {
            TaskStatus::Queue => "·",
            TaskStatus::Running => "⧖",
            TaskStatus::Done => "✓",
            TaskStatus::Failed => "✗",
        }
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Task domain aggregate ─────────────────────────────────────────────────────

/// A unique task identifier (the file stem, e.g. "001-fix-bug" or "20240101120000-fix-bug").
pub type TaskId = String;

/// Domain aggregate representing a task with enforced state transitions.
///
/// Valid transitions:
/// - `Queued  → Running` via [`Task::start`]
/// - `Running → Done`    via [`Task::complete`]
/// - `Running → Failed`  via [`Task::fail`]
/// - `Failed  → Queued`  via [`Task::retry`]
/// - `Queued  → Failed`  via [`Task::discard`] (administrative, e.g. parse errors)
#[derive(Debug, Clone)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub description: String,
    pub repo: Option<String>,
    pub priority: Option<String>,
    pub status: TaskStatus,
    pub created: Option<DateTime<Utc>>,
    pub started: Option<DateTime<Utc>>,
    pub ended: Option<DateTime<Utc>>,
    pub source: Option<String>,
    pub container: Option<String>,
    pub issue: Option<String>,
}

impl Task {
    /// Create a minimal task with a given id and status (useful for internal transitions).
    pub fn with_status(id: TaskId, status: TaskStatus) -> Self {
        Self {
            id,
            title: String::new(),
            description: String::new(),
            repo: None,
            priority: None,
            status,
            created: None,
            started: None,
            ended: None,
            source: None,
            container: None,
            issue: None,
        }
    }

    /// `Queued → Running`. Errors if the current status is not `Queued`.
    /// The caller supplies the timestamp so it can be injected in tests.
    pub fn start(&mut self, now: DateTime<Utc>) -> Result<()> {
        if self.status != TaskStatus::Queue {
            anyhow::bail!(
                "cannot start task '{}': expected Queued, got {}",
                self.id,
                self.status
            );
        }
        self.status = TaskStatus::Running;
        self.started = Some(now);
        Ok(())
    }

    /// `Running → Done`. Errors if the current status is not `Running`.
    pub fn complete(&mut self, now: DateTime<Utc>) -> Result<()> {
        if self.status != TaskStatus::Running {
            anyhow::bail!(
                "cannot complete task '{}': expected Running, got {}",
                self.id,
                self.status
            );
        }
        self.status = TaskStatus::Done;
        self.ended = Some(now);
        Ok(())
    }

    /// `Running → Failed`. Errors if the current status is not `Running`.
    pub fn fail(&mut self, now: DateTime<Utc>) -> Result<()> {
        if self.status != TaskStatus::Running {
            anyhow::bail!(
                "cannot fail task '{}': expected Running, got {}",
                self.id,
                self.status
            );
        }
        self.status = TaskStatus::Failed;
        self.ended = Some(now);
        Ok(())
    }

    /// `Failed → Queued`. Errors if the current status is not `Failed`.
    pub fn retry(&mut self, _now: DateTime<Utc>) -> Result<()> {
        if self.status != TaskStatus::Failed {
            anyhow::bail!(
                "cannot retry task '{}': expected Failed, got {}",
                self.id,
                self.status
            );
        }
        self.status = TaskStatus::Queue;
        self.started = None;
        self.ended = None;
        Ok(())
    }

    /// `Queued → Failed` (administrative). Used when a queued task cannot be started
    /// due to infrastructure failures (e.g. unparseable file, missing repo config).
    pub fn discard(&mut self, now: DateTime<Utc>) -> Result<()> {
        if self.status != TaskStatus::Queue {
            anyhow::bail!(
                "cannot discard task '{}': expected Queued, got {}",
                self.id,
                self.status
            );
        }
        self.status = TaskStatus::Failed;
        self.ended = Some(now);
        Ok(())
    }
}

impl From<TaskFile> for Task {
    fn from(f: TaskFile) -> Self {
        let parse_dt = |s: &str| {
            DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
        };
        Task {
            id: f.name,
            title: f.title,
            description: f.body,
            repo: f.repo,
            priority: Some(f.priority),
            status: f.status,
            created: f.added.as_deref().and_then(parse_dt),
            started: f.started.as_deref().and_then(parse_dt),
            ended: f.ended.as_deref().and_then(parse_dt),
            source: f.source,
            container: f.container,
            issue: f.issue,
        }
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
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

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
    let start = remaining.iter().position(|l| !l.is_empty()).unwrap_or(remaining.len());
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
    content.push_str(&format!("started: {started}\ncontainer: {container}\n---\n{description}\n"));
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

    // ── Task aggregate state transition tests ─────────────────────────────────

    fn now() -> chrono::DateTime<Utc> {
        chrono::DateTime::parse_from_rfc3339("2024-01-01T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn test_task_start_queued_to_running() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Queue);
        task.start(now()).unwrap();
        assert_eq!(task.status, TaskStatus::Running);
        assert_eq!(task.started, Some(now()));
    }

    #[test]
    fn test_task_start_rejects_running() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Running);
        assert!(task.start(now()).is_err());
    }

    #[test]
    fn test_task_start_rejects_done() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Done);
        assert!(task.start(now()).is_err());
    }

    #[test]
    fn test_task_start_rejects_failed() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Failed);
        assert!(task.start(now()).is_err());
    }

    #[test]
    fn test_task_complete_running_to_done() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Running);
        task.complete(now()).unwrap();
        assert_eq!(task.status, TaskStatus::Done);
        assert_eq!(task.ended, Some(now()));
    }

    #[test]
    fn test_task_complete_rejects_queue() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Queue);
        assert!(task.complete(now()).is_err());
    }

    #[test]
    fn test_task_complete_rejects_done() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Done);
        assert!(task.complete(now()).is_err());
    }

    #[test]
    fn test_task_fail_running_to_failed() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Running);
        task.fail(now()).unwrap();
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(task.ended, Some(now()));
    }

    #[test]
    fn test_task_fail_rejects_queue() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Queue);
        assert!(task.fail(now()).is_err());
    }

    #[test]
    fn test_task_fail_rejects_done() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Done);
        assert!(task.fail(now()).is_err());
    }

    #[test]
    fn test_task_retry_failed_to_queue() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Failed);
        task.started = Some(now());
        task.ended = Some(now());
        task.retry(now()).unwrap();
        assert_eq!(task.status, TaskStatus::Queue);
        assert_eq!(task.started, None);
        assert_eq!(task.ended, None);
    }

    #[test]
    fn test_task_retry_rejects_queue() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Queue);
        assert!(task.retry(now()).is_err());
    }

    #[test]
    fn test_task_retry_rejects_running() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Running);
        assert!(task.retry(now()).is_err());
    }

    #[test]
    fn test_task_discard_queue_to_failed() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Queue);
        task.discard(now()).unwrap();
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(task.ended, Some(now()));
    }

    #[test]
    fn test_task_discard_rejects_running() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Running);
        assert!(task.discard(now()).is_err());
    }

    #[test]
    fn test_invalid_transition_queue_to_done() {
        // Queue → Done is not a valid domain transition.
        // task.complete() requires Running.
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Queue);
        assert!(task.complete(now()).is_err());
    }

    #[test]
    fn test_task_full_lifecycle() {
        let mut task = Task::with_status("001-test".to_string(), TaskStatus::Queue);
        assert_eq!(task.status, TaskStatus::Queue);

        task.start(now()).unwrap();
        assert_eq!(task.status, TaskStatus::Running);

        task.fail(now()).unwrap();
        assert_eq!(task.status, TaskStatus::Failed);

        task.retry(now()).unwrap();
        assert_eq!(task.status, TaskStatus::Queue);

        task.start(now()).unwrap();
        task.complete(now()).unwrap();
        assert_eq!(task.status, TaskStatus::Done);
    }

    #[test]
    fn test_task_symbol() {
        assert_eq!(TaskStatus::Queue.symbol(), "·");
        assert_eq!(TaskStatus::Running.symbol(), "⧖");
        assert_eq!(TaskStatus::Done.symbol(), "✓");
        assert_eq!(TaskStatus::Failed.symbol(), "✗");
    }

    #[test]
    fn test_task_from_task_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("001-test.md");
        fs::write(
            &path,
            "---\nrepo: myrepo\npriority: high\nsource: github#42\nadded: 2024-01-01T00:00:00Z\n---\nFix the bug\nsome body\n",
        )
        .unwrap();
        let task_file = parse_task_file(&path, TaskStatus::Queue).unwrap();
        let task = Task::from(task_file);
        assert_eq!(task.id, "001-test");
        assert_eq!(task.title, "Fix the bug");
        assert_eq!(task.description, "some body");
        assert_eq!(task.repo, Some("myrepo".to_string()));
        assert_eq!(task.priority, Some("high".to_string()));
        assert_eq!(task.status, TaskStatus::Queue);
        assert_eq!(task.source, Some("github#42".to_string()));
        assert!(task.created.is_some());
    }
}
