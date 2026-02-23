use chrono::{DateTime, Utc};
use sipag_core::state::{WorkerPhase, WorkerState};
use std::path::PathBuf;

/// A task as represented in the TUI — derived from `sipag_core::state::WorkerState`.
#[derive(Debug, Clone)]
pub struct Task {
    pub repo: String,
    pub pr_num: u64,
    pub issues: Vec<u64>,
    pub branch: String,
    pub container_id: String,
    pub phase: WorkerPhase,
    pub started: Option<DateTime<Utc>>,
    pub ended: Option<DateTime<Utc>>,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    /// Path to the state file on disk (for dismissal).
    pub file_path: PathBuf,
}

impl From<WorkerState> for Task {
    fn from(w: WorkerState) -> Self {
        Task {
            started: parse_rfc3339(&w.started),
            ended: w.ended.as_deref().and_then(parse_rfc3339),
            repo: w.repo,
            pr_num: w.pr_num,
            issues: w.issues,
            branch: w.branch,
            container_id: w.container_id,
            phase: w.phase,
            exit_code: w.exit_code,
            error: w.error,
            file_path: w.file_path,
        }
    }
}

fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    if s.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn format_since(dt: &DateTime<Utc>) -> String {
    let secs = Utc::now().signed_duration_since(*dt).num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

impl Task {
    pub fn format_age(&self) -> String {
        self.started
            .as_ref()
            .map(format_since)
            .unwrap_or_else(|| "-".to_string())
    }

    pub fn format_ended_age(&self) -> String {
        self.ended
            .as_ref()
            .or(self.started.as_ref())
            .map(format_since)
            .unwrap_or_else(|| "-".to_string())
    }

    pub fn duration_secs(&self) -> Option<u64> {
        let started = self.started?;
        let ended = self.ended?;
        Some(ended.signed_duration_since(started).num_seconds().max(0) as u64)
    }

    pub fn log_lines(&self) -> Vec<String> {
        let log_path = self.log_path();
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

    fn log_path(&self) -> PathBuf {
        // State file: .../workers/{slug}--pr-{N}.json
        // Log file:   .../logs/{slug}--pr-{N}.log
        if let Some(stem) = self.file_path.file_stem().and_then(|s| s.to_str()) {
            if let Some(sipag_dir) = self.file_path.parent().and_then(|p| p.parent()) {
                return sipag_dir.join("logs").join(format!("{stem}.log"));
            }
        }
        PathBuf::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sample_worker_state() -> WorkerState {
        WorkerState {
            repo: "Dorky-Robot/sipag".to_string(),
            pr_num: 42,
            issues: vec![10, 11],
            branch: "sipag/pr-42".to_string(),
            container_id: "abc123def456".to_string(),
            phase: WorkerPhase::Working,
            heartbeat: "2026-01-15T10:30:00Z".to_string(),
            started: "2026-01-15T10:30:00Z".to_string(),
            ended: None,
            exit_code: None,
            error: None,
            file_path: PathBuf::from("/home/.sipag/workers/Dorky-Robot--sipag--pr-42.json"),
        }
    }

    #[test]
    fn from_worker_state_working() {
        let task = Task::from(sample_worker_state());
        assert_eq!(task.pr_num, 42);
        assert_eq!(task.repo, "Dorky-Robot/sipag");
        assert_eq!(task.phase, WorkerPhase::Working);
        assert_eq!(task.container_id, "abc123def456");
        assert!(task.started.is_some());
        assert!(task.ended.is_none());
        assert_eq!(task.issues, vec![10, 11]);
    }

    #[test]
    fn from_worker_state_finished() {
        let mut w = sample_worker_state();
        w.phase = WorkerPhase::Finished;
        w.ended = Some("2026-01-15T10:35:00Z".to_string());
        w.exit_code = Some(0);

        let task = Task::from(w);
        assert_eq!(task.phase, WorkerPhase::Finished);
        assert!(task.ended.is_some());
        assert_eq!(task.exit_code, Some(0));
        assert_eq!(task.duration_secs(), Some(300));
    }

    #[test]
    fn from_worker_state_failed() {
        let mut w = sample_worker_state();
        w.phase = WorkerPhase::Failed;
        w.ended = Some("2026-01-15T10:35:00Z".to_string());
        w.exit_code = Some(1);
        w.error = Some("Claude crashed".to_string());

        let task = Task::from(w);
        assert_eq!(task.phase, WorkerPhase::Failed);
        assert_eq!(task.exit_code, Some(1));
        assert_eq!(task.error, Some("Claude crashed".to_string()));
    }

    #[test]
    fn format_age_no_started() {
        let mut w = sample_worker_state();
        w.started = String::new();
        let task = Task::from(w);
        assert_eq!(task.format_age(), "-");
    }

    #[test]
    fn format_ended_age_uses_ended() {
        let ended = Utc::now() - chrono::Duration::hours(2);
        let mut w = sample_worker_state();
        w.ended = Some(ended.to_rfc3339());
        let task = Task::from(w);
        assert_eq!(task.format_ended_age(), "2h");
    }

    #[test]
    fn format_ended_age_falls_back_to_started() {
        let started = Utc::now() - chrono::Duration::minutes(5);
        let mut w = sample_worker_state();
        w.started = started.to_rfc3339();
        let task = Task::from(w);
        assert_eq!(task.format_ended_age(), "5m");
    }

    #[test]
    fn log_path_derived_from_state_path() {
        let task = Task::from(sample_worker_state());
        assert_eq!(
            task.log_path(),
            PathBuf::from("/home/.sipag/logs/Dorky-Robot--sipag--pr-42.log")
        );
    }

    #[test]
    fn log_lines_missing_file() {
        let task = Task::from(sample_worker_state());
        assert!(task.log_lines().is_empty());
    }

    #[test]
    fn log_lines_reads_file() {
        let dir = tempfile::tempdir().unwrap();
        let workers_dir = dir.path().join("workers");
        let logs_dir = dir.path().join("logs");
        std::fs::create_dir_all(&workers_dir).unwrap();
        std::fs::create_dir_all(&logs_dir).unwrap();

        let mut w = sample_worker_state();
        w.file_path = workers_dir.join("test--repo--pr-1.json");

        let task = Task::from(w);
        let log_path = logs_dir.join("test--repo--pr-1.log");
        let mut f = std::fs::File::create(&log_path).unwrap();
        for i in 0..5 {
            writeln!(f, "line {i}").unwrap();
        }

        let lines = task.log_lines();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "line 0");
    }

    #[test]
    fn duration_secs_none_when_incomplete() {
        let task = Task::from(sample_worker_state());
        assert_eq!(task.duration_secs(), None);
    }
}
