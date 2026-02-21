use chrono::{DateTime, Utc};
use sipag_core::task::TaskFile;
use sipag_core::worker::WorkerState;
use std::path::PathBuf;

/// Re-export `TaskStatus` from `sipag-core` as the canonical `Status` type for the TUI.
pub use sipag_core::task::TaskStatus as Status;

/// A task as represented in the TUI — derived from `sipag_core::task::TaskFile`
/// or a worker state JSON file.
#[derive(Debug, Clone)]
pub struct Task {
    /// Numeric ID extracted from the filename prefix (e.g. `003-fix.md` → 3)
    /// or the issue number for worker-JSON tasks.
    pub id: u32,
    pub title: String,
    pub repo: Option<String>,
    pub priority: Option<String>,
    pub source: Option<String>,
    pub added: Option<DateTime<Utc>>,
    pub body: String,
    pub status: Status,
    /// GitHub issue number (from frontmatter `issue:` field or worker JSON `issue_num`).
    pub issue: Option<u32>,
    /// Absolute path to the `.md` file on disk (used to locate the companion `.log`
    /// when `log_path` is not set). Empty for worker-JSON tasks.
    pub file_path: PathBuf,
    /// Docker container name for running tasks (used for attach).
    pub container: Option<String>,
    /// Explicit log file path (set for worker-JSON tasks; overrides `file_path`-based lookup).
    pub log_path: Option<PathBuf>,
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

        let issue = tf
            .issue
            .as_deref()
            .and_then(|s| s.trim_start_matches('#').parse::<u32>().ok());

        Task {
            id,
            title: tf.title,
            repo: tf.repo,
            priority: Some(tf.priority),
            source: tf.source,
            added,
            body: tf.body,
            status: tf.status,
            issue,
            file_path: tf.file_path,
            container: tf.container,
            log_path: None,
        }
    }
}

impl From<WorkerState> for Task {
    fn from(w: WorkerState) -> Self {
        let added = w.started_at.as_deref().and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
        });

        let status = match w.status {
            sipag_core::worker::WorkerStatus::Enqueued => Status::Queue,
            sipag_core::worker::WorkerStatus::Running
            | sipag_core::worker::WorkerStatus::Recovering => Status::Running,
            sipag_core::worker::WorkerStatus::Done => Status::Done,
            sipag_core::worker::WorkerStatus::Failed => Status::Failed,
        };

        let issue_num = u32::try_from(w.issue_num).unwrap_or(0);

        Task {
            id: issue_num,
            title: w.issue_title,
            repo: if w.repo.is_empty() {
                None
            } else {
                Some(w.repo)
            },
            priority: None,
            source: None,
            added,
            body: String::new(),
            status,
            issue: Some(issue_num),
            file_path: PathBuf::new(),
            container: if w.container_name.is_empty() {
                None
            } else {
                Some(w.container_name)
            },
            log_path: w.log_path,
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

    /// Returns the last 30 lines of the log file.
    ///
    /// For worker-JSON tasks, reads from `log_path`. For task-file tasks,
    /// derives the log path from `file_path` with `.log` extension.
    /// Returns an empty vec if the log file does not exist.
    pub fn log_lines(&self) -> Vec<String> {
        let log_path = if let Some(p) = &self.log_path {
            p.clone()
        } else {
            self.file_path.with_extension("log")
        };
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
            container: Some("sipag-container-123".to_string()),
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
        assert_eq!(task.issue, None);
        assert_eq!(task.container, Some("sipag-container-123".to_string()));
        assert_eq!(task.log_path, None);
    }

    #[test]
    fn from_worker_state_running() {
        use sipag_core::worker::WorkerState;
        let w = WorkerState {
            repo: "Dorky-Robot/sipag".to_string(),
            issue_num: 42,
            issue_title: "Fix the thing".to_string(),
            branch: "sipag/issue-42-fix-the-thing".to_string(),
            container_name: "sipag-issue-42".to_string(),
            pr_num: None,
            pr_url: None,
            status: sipag_core::worker::WorkerStatus::Running,
            started_at: Some("2024-01-15T10:30:00Z".to_string()),
            ended_at: None,
            duration_s: None,
            exit_code: None,
            log_path: Some(PathBuf::from(
                "/home/.sipag/logs/Dorky-Robot--sipag--42.log",
            )),
        };
        let task = Task::from(w);
        assert_eq!(task.id, 42);
        assert_eq!(task.issue, Some(42));
        assert_eq!(task.title, "Fix the thing");
        assert_eq!(task.repo, Some("Dorky-Robot/sipag".to_string()));
        assert_eq!(task.status, Status::Running);
        assert_eq!(task.container, Some("sipag-issue-42".to_string()));
        assert!(task.log_path.is_some());
    }

    #[test]
    fn from_worker_state_done() {
        use sipag_core::worker::WorkerState;
        let w = WorkerState {
            repo: "Dorky-Robot/sipag".to_string(),
            issue_num: 42,
            issue_title: "Fix the thing".to_string(),
            branch: "sipag/issue-42-fix-the-thing".to_string(),
            container_name: "sipag-issue-42".to_string(),
            pr_num: Some(163),
            pr_url: Some("https://github.com/Dorky-Robot/sipag/pull/163".to_string()),
            status: sipag_core::worker::WorkerStatus::Done,
            started_at: Some("2024-01-15T10:30:00Z".to_string()),
            ended_at: Some("2024-01-15T10:34:23Z".to_string()),
            duration_s: Some(263),
            exit_code: Some(0),
            log_path: None,
        };
        let task = Task::from(w);
        assert_eq!(task.status, Status::Done);
        assert_eq!(task.log_path, None);
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
            issue: None,
            file_path: std::path::PathBuf::new(),
            container: None,
            log_path: None,
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
            issue: None,
            file_path: std::path::PathBuf::from("/nonexistent/path.md"),
            container: None,
            log_path: None,
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
            issue: None,
            file_path: md_path,
            container: None,
            log_path: None,
        };
        let lines = task.log_lines();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "line 0");
    }

    #[test]
    fn log_lines_uses_explicit_log_path() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("worker.log");
        let mut f = std::fs::File::create(&log_path).unwrap();
        writeln!(f, "worker log line").unwrap();

        let task = Task {
            id: 0,
            title: "test".to_string(),
            repo: None,
            priority: None,
            source: None,
            added: None,
            body: String::new(),
            status: Status::Running,
            issue: None,
            file_path: PathBuf::new(), // no .md file
            container: None,
            log_path: Some(log_path),
        };
        let lines = task.log_lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "worker log line");
    }
}
