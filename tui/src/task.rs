use chrono::{DateTime, Utc};
use sipag_core::task::TaskFile;
use std::path::PathBuf;

/// Re-export `TaskStatus` from `sipag-core` as the canonical `Status` type for the TUI.
pub use sipag_core::task::TaskStatus as Status;

/// A task as represented in the TUI — derived from `sipag_core::task::TaskFile`.
#[derive(Debug, Clone)]
pub struct Task {
    /// Numeric ID extracted from the filename prefix (e.g. `003-fix.md` → 3).
    pub id: u32,
    pub title: String,
    pub repo: Option<String>,
    pub priority: Option<String>,
    pub source: Option<String>,
    pub added: Option<DateTime<Utc>>,
    pub body: String,
    pub status: Status,
    /// Absolute path to the `.md` file on disk (used to locate the companion `.log`).
    pub file_path: PathBuf,
}

impl From<TaskFile> for Task {
    fn from(tf: TaskFile) -> Self {
        let added = tf.added.as_deref().and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
        });

        let id = tf
            .name
            .split('-')
            .next()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);

        Task {
            id,
            title: tf.title,
            repo: tf.repo,
            priority: Some(tf.priority),
            source: tf.source,
            added,
            body: tf.body,
            status: tf.status,
            file_path: tf.file_path,
        }
    }
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

    /// Returns the last 30 lines of the companion `.log` file.
    /// Returns an empty vec if the log file does not exist.
    pub fn log_lines(&self) -> Vec<String> {
        let log_path = self.file_path.with_extension("log");
        if !log_path.exists() {
            return vec![];
        }
        let content = std::fs::read_to_string(&log_path).unwrap_or_default();
        let lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
        let n = 30;
        if lines.len() <= n {
            lines
        } else {
            lines[lines.len() - n..].to_vec()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sipag_core::task::TaskStatus;
    use std::io::Write;

    #[test]
    fn status_icon_and_name() {
        assert_eq!(TaskStatus::Queue.icon(), "·");
        assert_eq!(TaskStatus::Running.icon(), "⧖");
        assert_eq!(TaskStatus::Done.icon(), "✓");
        assert_eq!(TaskStatus::Failed.icon(), "✗");

        assert_eq!(TaskStatus::Queue.name(), "pending");
        assert_eq!(TaskStatus::Running.name(), "running");
        assert_eq!(TaskStatus::Done.name(), "done");
        assert_eq!(TaskStatus::Failed.name(), "failed");
    }

    #[test]
    fn from_task_file_basic() {
        use sipag_core::task::TaskFile;
        let tf = TaskFile {
            name: "003-fix-bug".to_string(),
            repo: Some("myrepo".to_string()),
            priority: "high".to_string(),
            source: None,
            added: Some("2024-01-01T00:00:00Z".to_string()),
            started: None,
            ended: None,
            container: None,
            issue: None,
            title: "Fix the bug".to_string(),
            body: "Some description".to_string(),
            status: TaskStatus::Queue,
            file_path: std::path::PathBuf::from("/tmp/003-fix-bug.md"),
        };
        let task = Task::from(tf);
        assert_eq!(task.id, 3);
        assert_eq!(task.title, "Fix the bug");
        assert_eq!(task.repo, Some("myrepo".to_string()));
        assert_eq!(task.priority, Some("high".to_string()));
        assert_eq!(task.body, "Some description");
        assert!(task.added.is_some());
    }

    #[test]
    fn format_age_no_added() {
        let task = Task {
            id: 1,
            title: "test".to_string(),
            repo: None,
            priority: None,
            source: None,
            added: None,
            body: String::new(),
            status: Status::Queue,
            file_path: std::path::PathBuf::new(),
        };
        assert_eq!(task.format_age(), "-");
    }

    #[test]
    fn log_lines_missing_file() {
        let task = Task {
            id: 1,
            title: "test".to_string(),
            repo: None,
            priority: None,
            source: None,
            added: None,
            body: String::new(),
            status: Status::Queue,
            file_path: std::path::PathBuf::from("/nonexistent/path.md"),
        };
        assert!(task.log_lines().is_empty());
    }

    #[test]
    fn log_lines_reads_file() {
        let dir = tempfile::tempdir().unwrap();
        let md_path = dir.path().join("task.md");
        let log_path = dir.path().join("task.log");
        std::fs::write(&md_path, "title\n").unwrap();
        let mut f = std::fs::File::create(&log_path).unwrap();
        for i in 0..5 {
            writeln!(f, "line {}", i).unwrap();
        }
        let task = Task {
            id: 0,
            title: "test".to_string(),
            repo: None,
            priority: None,
            source: None,
            added: None,
            body: String::new(),
            status: Status::Running,
            file_path: md_path,
        };
        let lines = task.log_lines();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "line 0");
    }
}
