use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Pending,
    Running,
    Done,
    Failed,
}

impl Status {
    pub fn icon(&self) -> &str {
        match self {
            Status::Pending => "·",
            Status::Running => "⧖",
            Status::Done => "✓",
            Status::Failed => "✗",
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Status::Pending => "pending",
            Status::Running => "running",
            Status::Done => "done",
            Status::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: u32,
    pub title: String,
    pub repo: Option<String>,
    pub priority: Option<String>,
    pub source: Option<String>,
    pub added: Option<DateTime<Utc>>,
    pub body: String,
    pub status: Status,
    pub file_path: PathBuf,
}

#[derive(Deserialize, Default)]
pub(crate) struct Frontmatter {
    pub(crate) repo: Option<String>,
    pub(crate) priority: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) added: Option<String>,
}

impl Task {
    /// Returns a human-readable age string like "2d", "3h", "15m", "30s".
    pub fn format_age(&self) -> String {
        let Some(added) = &self.added else {
            return "-".to_string();
        };
        let now = Utc::now();
        let dur = now.signed_duration_since(*added);
        let secs = dur.num_seconds().max(0);
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m", secs / 60)
        } else if secs < 86400 {
            format!("{}h", secs / 3600)
        } else {
            format!("{}d", secs / 86400)
        }
    }

    /// Returns the path to the companion `.log` file, if any.
    pub fn log_path(&self) -> PathBuf {
        self.file_path.with_extension("log")
    }

    /// Returns the last `n` lines of the companion `.log` file.
    /// Returns an empty vec if the file does not exist.
    pub fn log_lines(&self) -> Vec<String> {
        self.last_log_lines(30)
    }

    pub fn last_log_lines(&self, n: usize) -> Vec<String> {
        let log_path = self.log_path();
        if !log_path.exists() {
            return vec![];
        }
        let content = fs::read_to_string(&log_path).unwrap_or_default();
        let lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
        let total = lines.len();
        if total <= n {
            lines
        } else {
            lines[total - n..].to_vec()
        }
    }

    /// Parse a task `.md` file with YAML frontmatter.
    pub fn parse_file(path: &Path, status: Status) -> Result<Self> {
        let content = fs::read_to_string(path)?;
        let (frontmatter, rest) = parse_frontmatter(&content);

        // First non-empty line after frontmatter is the title.
        let mut lines = rest.lines();
        let title = lines
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_string();
        let body: String = lines.collect::<Vec<_>>().join("\n").trim().to_string();

        // Extract numeric ID from filename prefix (e.g. "003-fix-flaky-test" → 3).
        let filename = path.file_stem().unwrap_or_default().to_string_lossy();
        let id = filename
            .split('-')
            .next()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);

        // Parse the `added` timestamp.
        let added = frontmatter.added.as_deref().and_then(|s| {
            DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
        });

        // Fallback title: use the slug from the filename.
        let title = if title.is_empty() {
            filename.to_string()
        } else {
            title
        };

        Ok(Task {
            id,
            title,
            repo: frontmatter.repo,
            priority: frontmatter.priority,
            source: frontmatter.source,
            added,
            body,
            status,
            file_path: path.to_path_buf(),
        })
    }
}

/// Split `content` into (parsed frontmatter, remaining text).
///
/// If the content does not start with `---`, returns a default (empty) frontmatter
/// and the original content unchanged.
pub(crate) fn parse_frontmatter(content: &str) -> (Frontmatter, String) {
    let lines: Vec<&str> = content.lines().collect();

    if lines.is_empty() || lines[0].trim() != "---" {
        return (Frontmatter::default(), content.to_string());
    }

    // Find the closing `---` line.
    let close = lines[1..].iter().position(|l| l.trim() == "---");
    let Some(rel_close) = close else {
        return (Frontmatter::default(), content.to_string());
    };
    let close_idx = rel_close + 1; // absolute index in `lines`

    let yaml = lines[1..close_idx].join("\n");
    let rest = lines[close_idx + 1..].join("\n");

    let fm = serde_yaml::from_str(&yaml).unwrap_or_default();
    (fm, rest)
}

/// Load all tasks from `sipag_dir`, scanning queue/, running/, done/, and failed/.
pub fn load_tasks(sipag_dir: &Path) -> Vec<Task> {
    let dirs = [
        (sipag_dir.join("queue"), Status::Pending),
        (sipag_dir.join("running"), Status::Running),
        (sipag_dir.join("done"), Status::Done),
        (sipag_dir.join("failed"), Status::Failed),
    ];

    let mut tasks = Vec::new();
    for (dir, status) in dirs {
        if !dir.exists() {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };

        let mut md_entries: Vec<_> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
            .collect();
        md_entries.sort_by_key(|e| e.file_name());

        for entry in md_entries {
            if let Ok(task) = Task::parse_file(&entry.path(), status.clone()) {
                tasks.push(task);
            }
        }
    }

    tasks
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    #[test]
    fn test_parse_frontmatter_full() {
        let content =
            "---\nrepo: salita\npriority: medium\nsource: github#142\n---\nTask Title\n\nBody text.";
        let (fm, rest) = parse_frontmatter(content);
        assert_eq!(fm.repo.as_deref(), Some("salita"));
        assert_eq!(fm.priority.as_deref(), Some("medium"));
        assert_eq!(fm.source.as_deref(), Some("github#142"));
        assert_eq!(rest, "Task Title\n\nBody text.");
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        let content = "Task Title\n\nBody text.";
        let (fm, rest) = parse_frontmatter(content);
        assert!(fm.repo.is_none());
        assert_eq!(rest, "Task Title\n\nBody text.");
    }

    #[test]
    fn test_parse_frontmatter_empty_body() {
        let content = "---\nrepo: salita\n---\nJust a title";
        let (fm, rest) = parse_frontmatter(content);
        assert_eq!(fm.repo.as_deref(), Some("salita"));
        assert_eq!(rest, "Just a title");
    }

    #[test]
    fn test_parse_frontmatter_no_closing_delimiter() {
        let content = "---\nrepo: salita\nTitle";
        let (_fm, rest) = parse_frontmatter(content);
        // Falls back to full content when no closing --- is found.
        assert_eq!(rest, content);
    }

    #[test]
    fn test_format_age_days() {
        let two_days_ago = Utc::now() - chrono::Duration::days(2);
        let task = Task {
            id: 1,
            title: "Test".to_string(),
            repo: None,
            priority: None,
            source: None,
            added: Some(two_days_ago),
            body: String::new(),
            status: Status::Pending,
            file_path: PathBuf::new(),
        };
        assert_eq!(task.format_age(), "2d");
    }

    #[test]
    fn test_format_age_hours() {
        let three_hours_ago = Utc::now() - chrono::Duration::hours(3);
        let task = Task {
            id: 1,
            title: "Test".to_string(),
            repo: None,
            priority: None,
            source: None,
            added: Some(three_hours_ago),
            body: String::new(),
            status: Status::Pending,
            file_path: PathBuf::new(),
        };
        assert_eq!(task.format_age(), "3h");
    }

    #[test]
    fn test_format_age_no_timestamp() {
        let task = Task {
            id: 1,
            title: "Test".to_string(),
            repo: None,
            priority: None,
            source: None,
            added: None,
            body: String::new(),
            status: Status::Pending,
            file_path: PathBuf::new(),
        };
        assert_eq!(task.format_age(), "-");
    }

    #[test]
    fn test_log_lines_last_30() {
        let dir = std::env::temp_dir().join("sipag_test_log_lines");
        fs::create_dir_all(&dir).unwrap();
        let log_path = dir.join("test.log");

        // Write 50 lines to the log file.
        let mut f = fs::File::create(&log_path).unwrap();
        for i in 1..=50 {
            writeln!(f, "line {}", i).unwrap();
        }

        let task = Task {
            id: 1,
            title: "Test".to_string(),
            repo: None,
            priority: None,
            source: None,
            added: None,
            body: String::new(),
            status: Status::Failed,
            file_path: dir.join("test.md"),
        };

        let lines = task.log_lines();
        assert_eq!(lines.len(), 30);
        assert_eq!(lines[0], "line 21");
        assert_eq!(lines[29], "line 50");

        fs::remove_file(&log_path).unwrap();
    }

    #[test]
    fn test_log_lines_no_file() {
        let task = Task {
            id: 1,
            title: "Test".to_string(),
            repo: None,
            priority: None,
            source: None,
            added: None,
            body: String::new(),
            status: Status::Failed,
            file_path: PathBuf::from("/nonexistent/path/test.md"),
        };
        assert!(task.log_lines().is_empty());
    }

    #[test]
    fn test_parse_file() {
        let dir = std::env::temp_dir().join("sipag_test_parse_file");
        fs::create_dir_all(&dir).unwrap();
        let md_path = dir.join("003-fix-flaky-test.md");
        fs::write(
            &md_path,
            "---\nrepo: salita\npriority: medium\nadded: 2026-02-19T22:30:00Z\n---\nFix the flaky WebSocket test\n\nThe test fails intermittently.",
        )
        .unwrap();

        let task = Task::parse_file(&md_path, Status::Failed).unwrap();
        assert_eq!(task.id, 3);
        assert_eq!(task.title, "Fix the flaky WebSocket test");
        assert_eq!(task.repo.as_deref(), Some("salita"));
        assert_eq!(task.priority.as_deref(), Some("medium"));
        assert_eq!(task.body, "The test fails intermittently.");
        assert_eq!(task.status, Status::Failed);
        assert!(task.added.is_some());

        fs::remove_file(&md_path).unwrap();
    }
}
