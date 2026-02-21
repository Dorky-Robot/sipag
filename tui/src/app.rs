use anyhow::Result;
use chrono::Utc;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sipag_core::worker::{list_workers, mark_worker_failed};
use std::{fs, path::PathBuf};

use crate::task::{Status, Task};

// ── ListMode ──────────────────────────────────────────────────────────────────

/// Which subset of workers is shown in the list view.
#[derive(Debug, Clone, PartialEq)]
pub enum ListMode {
    /// Show only active workers: running and recovering (Queue/Running status).
    Active,
    /// Show completed workers: done and failed. Auto-hides old entries.
    Archive,
}

// ── View ──────────────────────────────────────────────────────────────────────

/// Which top-level view is currently active.
pub enum View {
    List,
    Detail,
}

// ── App ───────────────────────────────────────────────────────────────────────

pub struct App {
    pub sipag_dir: PathBuf,
    pub tasks: Vec<Task>,
    /// Currently highlighted row in the list view.
    pub selected: usize,
    pub view: View,
    /// Lines from the companion `.log` file (capped to last 30).
    pub log_lines: Vec<String>,
    /// Vertical scroll offset within the log section of the detail view.
    pub log_scroll: usize,
    /// Set when the user presses 'a' on a running task. The main loop
    /// reads this, suspends the TUI, and runs `docker exec` to attach.
    pub attach_request: Option<String>,
    /// True when `~/.sipag/drain` exists (workers won't pick up new issues).
    pub draining: bool,
    /// Whether the list is showing active (running) or archived (done/failed) workers.
    pub list_mode: ListMode,
    /// Archive entries older than this many days are automatically hidden (default: 7).
    pub archive_max_age_days: u64,
}

impl App {
    pub fn new() -> Result<Self> {
        let sipag_dir = Self::resolve_sipag_dir();
        let draining = sipag_dir.join("drain").exists();
        let archive_max_age_days = std::env::var("SIPAG_ARCHIVE_DAYS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(7);
        let mut app = Self {
            sipag_dir,
            tasks: vec![],
            selected: 0,
            view: View::List,
            log_lines: vec![],
            log_scroll: 0,
            attach_request: None,
            draining,
            list_mode: ListMode::Active,
            archive_max_age_days,
        };
        app.refresh_tasks()?;
        Ok(app)
    }

    fn resolve_sipag_dir() -> PathBuf {
        if let Ok(dir) = std::env::var("SIPAG_DIR") {
            return PathBuf::from(dir);
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| String::from("."));
        PathBuf::from(home).join(".sipag")
    }

    // ── Task list ─────────────────────────────────────────────────────────────

    pub fn refresh_tasks(&mut self) -> Result<()> {
        let workers = list_workers(&self.sipag_dir).unwrap_or_default();
        let all_tasks: Vec<Task> = workers.into_iter().map(Task::from).collect();

        let now = Utc::now();
        let max_age = chrono::Duration::days(self.archive_max_age_days as i64);

        self.tasks = match self.list_mode {
            ListMode::Active => all_tasks
                .into_iter()
                .filter(|t| t.status == Status::Queue || t.status == Status::Running)
                .collect(),
            ListMode::Archive => all_tasks
                .into_iter()
                .filter(|t| t.status == Status::Done || t.status == Status::Failed)
                .filter(|t| {
                    // Auto-hide entries older than max_age.
                    // If ended_at is not set, keep the entry visible.
                    t.ended_at
                        .is_none_or(|ended| now.signed_duration_since(ended) < max_age)
                })
                .collect(),
        };

        // Clamp selection
        if self.tasks.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.tasks.len() {
            self.selected = self.tasks.len() - 1;
        }
        // Re-check drain signal on every refresh
        self.draining = self.sipag_dir.join("drain").exists();
        Ok(())
    }

    // ── List-view navigation ──────────────────────────────────────────────────

    pub fn select_next(&mut self) {
        if !self.tasks.is_empty() {
            self.selected = (self.selected + 1).min(self.tasks.len() - 1);
        }
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    // ── View transitions ──────────────────────────────────────────────────────

    /// Toggle between the Active and Archive list modes.
    pub fn toggle_list_mode(&mut self) {
        self.list_mode = match self.list_mode {
            ListMode::Active => ListMode::Archive,
            ListMode::Archive => ListMode::Active,
        };
        self.selected = 0;
        let _ = self.refresh_tasks();
    }

    /// Open the detail view for the currently selected task.
    pub fn open_detail(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        self.log_lines = self.tasks[self.selected].log_lines();
        self.log_scroll = 0;
        self.view = View::Detail;
    }

    /// Return to the list view.
    pub fn close_detail(&mut self) {
        self.view = View::List;
    }

    // ── Detail-view log scrolling ─────────────────────────────────────────────

    pub fn scroll_log_down(&mut self) {
        if self.log_scroll + 1 < self.log_lines.len() {
            self.log_scroll += 1;
        }
    }

    pub fn scroll_log_up(&mut self) {
        self.log_scroll = self.log_scroll.saturating_sub(1);
    }

    // ── Actions ───────────────────────────────────────────────────────────────

    /// Move the selected failed task back to queue/ and return to the list view.
    ///
    /// No-op for worker-JSON tasks (identified by an empty `file_path`); those
    /// must be retried via `sipag retry` in the Rust CLI.
    pub fn retry_task(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        let task = &self.tasks[self.selected];
        if task.status != Status::Failed {
            return;
        }

        let file_path = task.file_path.clone();

        // Worker-JSON tasks have no backing .md file — skip the file rename.
        if file_path.as_os_str().is_empty() {
            return;
        }

        // file_path = ~/.sipag/failed/NNN-task.md
        // parent     = ~/.sipag/failed/
        // parent²    = ~/.sipag/
        let sipag_dir = file_path
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf());

        let Some(sipag_dir) = sipag_dir else {
            return;
        };
        let Some(filename) = file_path.file_name() else {
            return;
        };

        let queue_path = sipag_dir.join("queue").join(filename);
        if fs::rename(&file_path, &queue_path).is_ok() {
            // Move the companion log file too, if it exists.
            let log_src = file_path.with_extension("log");
            if log_src.exists() {
                let log_dst = queue_path.with_extension("log");
                let _ = fs::rename(log_src, log_dst);
            }
            // Update in-place so the user sees the change immediately.
            self.tasks[self.selected].file_path = queue_path;
            self.tasks[self.selected].status = Status::Queue;
        }

        self.close_detail();
    }

    /// Remove the worker state JSON file for the selected done/failed task,
    /// dismissing it from the archive.
    ///
    /// Only operates on worker-JSON tasks (where `file_path` is empty).
    pub fn dismiss_selected(&mut self) -> Result<()> {
        if self.tasks.is_empty() {
            return Ok(());
        }
        let task = &self.tasks[self.selected];
        if task.status != Status::Done && task.status != Status::Failed {
            return Ok(());
        }

        // Worker-JSON tasks are identified by an empty file_path.
        // Only those have a workers/*.json file to delete.
        if !task.file_path.as_os_str().is_empty() {
            return Ok(());
        }

        if let (Some(repo), Some(issue_num)) = (task.repo.clone(), task.issue) {
            let repo_slug = repo.replace('/', "--");
            let json_path = self
                .sipag_dir
                .join("workers")
                .join(format!("{}--{}.json", repo_slug, issue_num));
            let _ = fs::remove_file(&json_path);
        }

        // If dismissing from the detail view, go back to the list.
        if matches!(self.view, View::Detail) {
            self.view = View::List;
        }

        self.refresh_tasks()?;
        Ok(())
    }

    /// Create the drain signal file (`~/.sipag/drain`).
    ///
    /// Workers will finish their current batch and then stop polling for new issues.
    pub fn drain(&mut self) -> Result<()> {
        let drain_path = self.sipag_dir.join("drain");
        let _ = fs::write(&drain_path, "");
        self.draining = true;
        Ok(())
    }

    /// Remove the drain signal file (`~/.sipag/drain`), allowing workers to
    /// resume picking up new issues.
    pub fn resume(&mut self) -> Result<()> {
        let drain_path = self.sipag_dir.join("drain");
        if drain_path.exists() {
            let _ = fs::remove_file(&drain_path);
        }
        self.draining = false;
        Ok(())
    }

    /// Kill the Docker container for the currently selected running task and
    /// mark its worker state JSON as `"failed"`.
    pub fn kill_selected(&mut self) -> Result<()> {
        if self.tasks.is_empty() {
            return Ok(());
        }
        let task = &self.tasks[self.selected];
        if task.status != Status::Running {
            return Ok(());
        }
        let Some(container) = task.container.clone() else {
            return Ok(());
        };

        let _ = std::process::Command::new("docker")
            .args(["kill", &container])
            .output();

        let _ = mark_worker_failed(&self.sipag_dir, &container);
        self.refresh_tasks()?;
        Ok(())
    }

    /// Kill all running Docker containers tracked in the worker state files
    /// and mark each as `"failed"`.
    pub fn kill_all(&mut self) -> Result<()> {
        let containers: Vec<String> = self
            .tasks
            .iter()
            .filter(|t| t.status == Status::Running)
            .filter_map(|t| t.container.clone())
            .collect();

        for container in &containers {
            let _ = std::process::Command::new("docker")
                .args(["kill", container.as_str()])
                .output();
            let _ = mark_worker_failed(&self.sipag_dir, container);
        }

        self.refresh_tasks()?;
        Ok(())
    }

    // ── Attach ────────────────────────────────────────────────────────────────

    /// Get the container name for the selected running task.
    pub fn selected_container_name(&self) -> Option<String> {
        let task = self.tasks.get(self.selected)?;
        if task.status != Status::Running {
            return None;
        }
        task.container.clone()
    }

    // ── Key handling ──────────────────────────────────────────────────────────

    /// Returns true if the app should quit.
    pub fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        match self.view {
            View::List => self.handle_list_key(key),
            View::Detail => self.handle_detail_key(key),
        }
    }

    fn handle_list_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.modifiers != KeyModifiers::NONE && key.modifiers != KeyModifiers::SHIFT {
            return Ok(false);
        }
        match key.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Tab => self.toggle_list_mode(),
            KeyCode::Char('j') | KeyCode::Down => self.select_next(),
            KeyCode::Up => self.select_prev(),
            KeyCode::Enter => self.open_detail(),
            KeyCode::Char('a') => {
                if let Some(container) = self.selected_container_name() {
                    // Running task selected: attach to its container.
                    self.attach_request = Some(container);
                } else {
                    // No running task selected: toggle between active/archive.
                    self.toggle_list_mode();
                }
            }
            KeyCode::Char('x') | KeyCode::Delete => self.dismiss_selected()?,
            KeyCode::Char('d') => self.drain()?,
            KeyCode::Char('r') => self.resume()?,
            KeyCode::Char('k') => self.kill_selected()?,
            KeyCode::Char('K') => self.kill_all()?,
            _ => {}
        }
        Ok(false)
    }

    fn handle_detail_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Esc => self.close_detail(),
            KeyCode::Char('j') | KeyCode::Down => self.scroll_log_down(),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_log_up(),
            KeyCode::Char('r') => self.retry_task(),
            KeyCode::Char('a') => {
                if let Some(container) = self.selected_container_name() {
                    self.attach_request = Some(container);
                }
            }
            KeyCode::Char('x') | KeyCode::Delete => self.dismiss_selected()?,
            _ => {}
        }
        Ok(false)
    }

    // ── Tick ──────────────────────────────────────────────────────────────────

    pub fn on_tick(&mut self) -> Result<()> {
        Ok(())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use sipag_core::task::TaskStatus;

    fn make_task(n: u32, status: TaskStatus) -> Task {
        Task {
            id: n,
            title: format!("Task {}", n),
            repo: None,
            priority: None,
            source: None,
            added: None,
            ended_at: None,
            duration_s: None,
            exit_code: None,
            pr_num: None,
            pr_url: None,
            body: String::new(),
            status,
            issue: None,
            file_path: std::path::PathBuf::new(),
            container: None,
            log_path: None,
        }
    }

    fn make_app_with_tasks(tasks: Vec<Task>) -> App {
        App {
            sipag_dir: std::path::PathBuf::new(),
            tasks,
            selected: 0,
            view: View::List,
            log_lines: vec![],
            log_scroll: 0,
            attach_request: None,
            draining: false,
            list_mode: ListMode::Active,
            archive_max_age_days: 7,
        }
    }

    #[test]
    fn task_status_symbols() {
        assert_eq!(TaskStatus::Queue.symbol(), "·");
        assert_eq!(TaskStatus::Running.symbol(), "⧖");
        assert_eq!(TaskStatus::Done.symbol(), "✓");
        assert_eq!(TaskStatus::Failed.symbol(), "✗");
    }

    #[test]
    fn app_new_missing_dir_succeeds() {
        // App::new() with a non-existent sipag dir should not panic — it returns empty tasks.
        std::env::set_var("SIPAG_DIR", "/tmp/sipag-test-nonexistent-dir-xyz");
        let app = App::new().expect("App::new() should succeed even with missing sipag dir");
        assert!(app.tasks.is_empty());
        std::env::remove_var("SIPAG_DIR");
    }

    #[test]
    fn select_next_and_prev() {
        let mut app = make_app_with_tasks(vec![
            make_task(1, TaskStatus::Queue),
            make_task(2, TaskStatus::Queue),
            make_task(3, TaskStatus::Queue),
        ]);

        app.select_next();
        assert_eq!(app.selected, 1);
        app.select_next();
        assert_eq!(app.selected, 2);
        // Cannot go past last
        app.select_next();
        assert_eq!(app.selected, 2);
        app.select_prev();
        assert_eq!(app.selected, 1);
        app.select_prev();
        assert_eq!(app.selected, 0);
        // Cannot go before first
        app.select_prev();
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn refresh_tasks_reads_worker_json() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir(&workers_dir).unwrap();

        let json = r#"{
            "repo": "Dorky-Robot/sipag",
            "issue_num": 42,
            "issue_title": "Fix the thing",
            "branch": "sipag/issue-42-fix-the-thing",
            "container_name": "sipag-issue-42",
            "pr_num": null,
            "pr_url": null,
            "status": "running",
            "started_at": "2024-01-15T10:30:00Z",
            "ended_at": null,
            "duration_s": null,
            "exit_code": null,
            "log_path": null
        }"#;

        let mut f = std::fs::File::create(workers_dir.join("Dorky-Robot--sipag--42.json")).unwrap();
        writeln!(f, "{}", json).unwrap();

        std::env::set_var("SIPAG_DIR", dir.path().to_str().unwrap());
        let app = App::new().expect("App::new() should succeed");
        std::env::remove_var("SIPAG_DIR");

        assert_eq!(app.tasks.len(), 1);
        assert_eq!(app.tasks[0].issue, Some(42));
        assert_eq!(app.tasks[0].status, TaskStatus::Running);
    }

    #[test]
    fn refresh_tasks_active_mode_filters_terminal() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir(&workers_dir).unwrap();

        let running_json = r#"{"repo":"test/repo","issue_num":1,"issue_title":"Running","branch":"b","container_name":"c","pr_num":null,"pr_url":null,"status":"running","started_at":null,"ended_at":null,"duration_s":null,"exit_code":null,"log_path":null}"#;
        let done_json = r#"{"repo":"test/repo","issue_num":2,"issue_title":"Done","branch":"b","container_name":"c","pr_num":null,"pr_url":null,"status":"done","started_at":null,"ended_at":null,"duration_s":null,"exit_code":null,"log_path":null}"#;

        let mut f = std::fs::File::create(workers_dir.join("test--repo--1.json")).unwrap();
        writeln!(f, "{}", running_json).unwrap();
        let mut f = std::fs::File::create(workers_dir.join("test--repo--2.json")).unwrap();
        writeln!(f, "{}", done_json).unwrap();

        std::env::set_var("SIPAG_DIR", dir.path().to_str().unwrap());
        let app = App::new().expect("App::new() should succeed");
        std::env::remove_var("SIPAG_DIR");

        // Active mode (default): only running task visible
        assert_eq!(app.tasks.len(), 1);
        assert_eq!(app.tasks[0].status, TaskStatus::Running);
    }

    #[test]
    fn toggle_list_mode_switches_visible_tasks() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir(&workers_dir).unwrap();

        let running_json = r#"{"repo":"test/repo","issue_num":1,"issue_title":"Running","branch":"b","container_name":"c","pr_num":null,"pr_url":null,"status":"running","started_at":null,"ended_at":null,"duration_s":null,"exit_code":null,"log_path":null}"#;
        let done_json = r#"{"repo":"test/repo","issue_num":2,"issue_title":"Done","branch":"b","container_name":"c","pr_num":null,"pr_url":null,"status":"done","started_at":"2024-01-15T10:00:00Z","ended_at":"2024-01-15T10:05:00Z","duration_s":300,"exit_code":0,"log_path":null}"#;

        let mut f = std::fs::File::create(workers_dir.join("test--repo--1.json")).unwrap();
        writeln!(f, "{}", running_json).unwrap();
        let mut f = std::fs::File::create(workers_dir.join("test--repo--2.json")).unwrap();
        writeln!(f, "{}", done_json).unwrap();

        let mut app = App {
            sipag_dir: dir.path().to_path_buf(),
            tasks: vec![],
            selected: 0,
            view: View::List,
            log_lines: vec![],
            log_scroll: 0,
            attach_request: None,
            draining: false,
            list_mode: ListMode::Active,
            // Use a large max_age so the fixture's hardcoded 2024 date is not pruned.
            archive_max_age_days: 99999,
        };
        app.refresh_tasks().unwrap();

        // Active mode: only running
        assert_eq!(app.tasks.len(), 1);
        assert_eq!(app.tasks[0].status, TaskStatus::Running);

        app.toggle_list_mode();

        // Archive mode: only done (within age threshold)
        assert_eq!(app.list_mode, ListMode::Archive);
        assert_eq!(app.tasks.len(), 1);
        assert_eq!(app.tasks[0].status, TaskStatus::Done);
    }

    #[test]
    fn archive_auto_hides_old_entries() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir(&workers_dir).unwrap();

        // A done task that ended 10 days ago (older than default 7 days)
        let old_done_json = r#"{"repo":"test/repo","issue_num":3,"issue_title":"Old Done","branch":"b","container_name":"c","pr_num":null,"pr_url":null,"status":"done","started_at":"2000-01-01T00:00:00Z","ended_at":"2000-01-01T01:00:00Z","duration_s":3600,"exit_code":0,"log_path":null}"#;

        let mut f = std::fs::File::create(workers_dir.join("test--repo--3.json")).unwrap();
        writeln!(f, "{}", old_done_json).unwrap();

        let mut app = App {
            sipag_dir: dir.path().to_path_buf(),
            tasks: vec![],
            selected: 0,
            view: View::List,
            log_lines: vec![],
            log_scroll: 0,
            attach_request: None,
            draining: false,
            list_mode: ListMode::Archive,
            archive_max_age_days: 7,
        };
        app.refresh_tasks().unwrap();

        // Old entry should be hidden
        assert_eq!(app.tasks.len(), 0);
    }

    #[test]
    fn dismiss_selected_removes_worker_json() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let workers_dir = dir.path().join("workers");
        std::fs::create_dir(&workers_dir).unwrap();

        let done_json = r#"{"repo":"test/repo","issue_num":5,"issue_title":"Done task","branch":"b","container_name":"c","pr_num":null,"pr_url":null,"status":"done","started_at":"2024-01-15T10:00:00Z","ended_at":"2024-01-15T10:05:00Z","duration_s":300,"exit_code":0,"log_path":null}"#;
        let json_path = workers_dir.join("test--repo--5.json");
        let mut f = std::fs::File::create(&json_path).unwrap();
        writeln!(f, "{}", done_json).unwrap();

        let mut app = App {
            sipag_dir: dir.path().to_path_buf(),
            tasks: vec![],
            selected: 0,
            view: View::List,
            log_lines: vec![],
            log_scroll: 0,
            attach_request: None,
            draining: false,
            list_mode: ListMode::Archive,
            // Use a large max_age so the fixture's hardcoded 2024 date is not pruned.
            archive_max_age_days: 99999,
        };
        app.refresh_tasks().unwrap();
        assert_eq!(app.tasks.len(), 1);

        app.dismiss_selected().unwrap();

        // JSON file removed
        assert!(!json_path.exists());
        // Task list now empty
        assert_eq!(app.tasks.len(), 0);
    }

    #[test]
    fn drain_creates_file_and_resume_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App {
            sipag_dir: dir.path().to_path_buf(),
            tasks: vec![],
            selected: 0,
            view: View::List,
            log_lines: vec![],
            log_scroll: 0,
            attach_request: None,
            draining: false,
            list_mode: ListMode::Active,
            archive_max_age_days: 7,
        };

        assert!(!app.draining);
        app.drain().unwrap();
        assert!(app.draining);
        assert!(dir.path().join("drain").exists());

        app.resume().unwrap();
        assert!(!app.draining);
        assert!(!dir.path().join("drain").exists());
    }

    #[test]
    fn resume_is_noop_when_not_draining() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App {
            sipag_dir: dir.path().to_path_buf(),
            tasks: vec![],
            selected: 0,
            view: View::List,
            log_lines: vec![],
            log_scroll: 0,
            attach_request: None,
            draining: false,
            list_mode: ListMode::Active,
            archive_max_age_days: 7,
        };

        // Should not panic or error when drain file does not exist
        app.resume().unwrap();
        assert!(!app.draining);
    }

    #[test]
    fn kill_selected_noop_on_non_running() {
        let dir = tempfile::tempdir().unwrap();
        let task = make_task(1, Status::Queue);
        let mut app = App {
            sipag_dir: dir.path().to_path_buf(),
            tasks: vec![task],
            selected: 0,
            view: View::List,
            log_lines: vec![],
            log_scroll: 0,
            attach_request: None,
            draining: false,
            list_mode: ListMode::Active,
            archive_max_age_days: 7,
        };

        // Should not error — non-running task is ignored
        app.kill_selected().unwrap();
        assert_eq!(app.tasks[0].status, Status::Queue);
    }
}
