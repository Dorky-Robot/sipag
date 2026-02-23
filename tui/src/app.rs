use anyhow::Result;
use chrono::Utc;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sipag_core::state;
use std::path::PathBuf;

use crate::task::Task;

// ── ListMode ──────────────────────────────────────────────────────────────────

/// Which subset of workers is shown in the list view.
#[derive(Debug, Clone, PartialEq)]
pub enum ListMode {
    Active,
    Archive,
}

// ── View ──────────────────────────────────────────────────────────────────────

pub enum View {
    List,
    Detail,
}

// ── App ───────────────────────────────────────────────────────────────────────

pub struct App {
    pub sipag_dir: PathBuf,
    pub tasks: Vec<Task>,
    pub selected: usize,
    pub view: View,
    pub log_lines: Vec<String>,
    pub log_scroll: usize,
    pub attach_request: Option<String>,
    pub list_mode: ListMode,
    pub archive_max_age_days: u64,
}

impl App {
    pub fn new() -> Result<Self> {
        let sipag_dir = sipag_core::config::default_sipag_dir();
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
            list_mode: ListMode::Active,
            archive_max_age_days,
        };
        app.refresh_tasks()?;
        Ok(app)
    }

    // ── Task list ─────────────────────────────────────────────────────────────

    pub fn refresh_tasks(&mut self) -> Result<()> {
        // Use scan_workers (not list_all) to detect dead containers and
        // reconcile non-terminal workers against Docker liveness.
        let workers = sipag_core::worker::lifecycle::scan_workers(&self.sipag_dir);
        let all_tasks: Vec<Task> = workers.into_iter().map(Task::from).collect();

        let now = Utc::now();
        let max_age = chrono::Duration::days(self.archive_max_age_days as i64);

        self.tasks = match self.list_mode {
            ListMode::Active => all_tasks
                .into_iter()
                .filter(|t| !t.phase.is_terminal())
                .collect(),
            ListMode::Archive => all_tasks
                .into_iter()
                .filter(|t| t.phase.is_terminal())
                .filter(|t| {
                    t.ended
                        .is_none_or(|ended| now.signed_duration_since(ended) < max_age)
                })
                .collect(),
        };

        if self.tasks.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.tasks.len() {
            self.selected = self.tasks.len() - 1;
        }
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

    pub fn toggle_list_mode(&mut self) {
        self.list_mode = match self.list_mode {
            ListMode::Active => ListMode::Archive,
            ListMode::Archive => ListMode::Active,
        };
        self.selected = 0;
        let _ = self.refresh_tasks();
    }

    pub fn open_detail(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        self.log_lines = self.tasks[self.selected].log_lines();
        self.log_scroll = 0;
        self.view = View::Detail;
    }

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

    /// Remove the worker state file for the selected finished/failed task.
    pub fn dismiss_selected(&mut self) -> Result<()> {
        if self.tasks.is_empty() {
            return Ok(());
        }
        let task = &self.tasks[self.selected];
        if !task.phase.is_terminal() {
            return Ok(());
        }

        state::remove_state(&task.file_path)?;

        if matches!(self.view, View::Detail) {
            self.view = View::List;
        }
        self.refresh_tasks()?;
        Ok(())
    }

    /// Kill the Docker container for the currently selected active task.
    pub fn kill_selected(&mut self) -> Result<()> {
        if self.tasks.is_empty() {
            return Ok(());
        }
        let task = &self.tasks[self.selected];
        if task.phase.is_terminal() {
            return Ok(());
        }

        // Kill by stored container name.
        let container_name = task.container_id.clone();
        let _ = std::process::Command::new("docker")
            .args(["kill", &container_name])
            .output();

        let mut worker = state::read_state(&task.file_path)?;
        worker.phase = state::WorkerPhase::Failed;
        worker.ended = Some(Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
        worker.error = Some("Killed by user".to_string());
        state::write_state(&worker)?;

        self.refresh_tasks()?;
        Ok(())
    }

    /// Kill all active Docker containers.
    pub fn kill_all(&mut self) -> Result<()> {
        let active: Vec<(String, PathBuf)> = self
            .tasks
            .iter()
            .filter(|t| !t.phase.is_terminal())
            .map(|t| (t.container_id.clone(), t.file_path.clone()))
            .collect();

        for (container_id, file_path) in &active {
            let container_name = container_id.clone();
            let _ = std::process::Command::new("docker")
                .args(["kill", &container_name])
                .output();
            if let Ok(mut worker) = state::read_state(file_path) {
                worker.phase = state::WorkerPhase::Failed;
                worker.ended = Some(Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
                worker.error = Some("Killed by user".to_string());
                let _ = state::write_state(&worker);
            }
        }

        self.refresh_tasks()?;
        Ok(())
    }

    // ── Attach ────────────────────────────────────────────────────────────────

    pub fn selected_container_name(&self) -> Option<String> {
        let task = self.tasks.get(self.selected)?;
        if task.phase.is_terminal() {
            return None;
        }
        Some(task.container_id.clone())
    }

    // ── Key handling ──────────────────────────────────────────────────────────

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
                    self.attach_request = Some(container);
                } else {
                    self.toggle_list_mode();
                }
            }
            KeyCode::Char('x') | KeyCode::Delete => self.dismiss_selected()?,
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
    use sipag_core::state::WorkerPhase;

    fn make_task(pr_num: u64, phase: WorkerPhase) -> Task {
        Task {
            repo: "test/repo".to_string(),
            pr_num,
            issues: vec![],
            branch: format!("sipag/pr-{pr_num}"),
            container_id: String::new(),
            phase,
            started: None,
            ended: None,
            exit_code: None,
            error: None,
            file_path: PathBuf::new(),
        }
    }

    fn make_app_with_tasks(tasks: Vec<Task>) -> App {
        App {
            sipag_dir: PathBuf::new(),
            tasks,
            selected: 0,
            view: View::List,
            log_lines: vec![],
            log_scroll: 0,
            attach_request: None,
            list_mode: ListMode::Active,
            archive_max_age_days: 7,
        }
    }

    #[test]
    fn app_new_missing_dir_succeeds() {
        std::env::set_var("SIPAG_DIR", "/tmp/sipag-test-nonexistent-dir-xyz");
        let app = App::new().expect("App::new() should succeed even with missing sipag dir");
        assert!(app.tasks.is_empty());
        std::env::remove_var("SIPAG_DIR");
    }

    #[test]
    fn select_next_and_prev() {
        let mut app = make_app_with_tasks(vec![
            make_task(1, WorkerPhase::Working),
            make_task(2, WorkerPhase::Working),
            make_task(3, WorkerPhase::Working),
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
    fn refresh_tasks_reads_state_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();

        // Use a terminal phase because scan_workers reconciles non-terminal
        // workers against Docker liveness (no Docker in tests → reconciled to failed).
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let s = state::WorkerState {
            repo: "test/repo".to_string(),
            pr_num: 42,
            issues: vec![1],
            branch: "sipag/pr-42".to_string(),
            container_id: "abc".to_string(),
            phase: WorkerPhase::Finished,
            heartbeat: now.clone(),
            started: now.clone(),
            ended: Some(now),
            exit_code: Some(0),
            error: None,
            file_path: state::state_file_path(dir.path(), "test/repo", 42),
        };
        state::write_state(&s).unwrap();

        let mut app = App {
            sipag_dir: dir.path().to_path_buf(),
            tasks: vec![],
            selected: 0,
            view: View::List,
            log_lines: vec![],
            log_scroll: 0,
            attach_request: None,
            list_mode: ListMode::Archive,
            archive_max_age_days: 7,
        };
        app.refresh_tasks().unwrap();

        assert_eq!(app.tasks.len(), 1);
        assert_eq!(app.tasks[0].pr_num, 42);
        assert_eq!(app.tasks[0].phase, WorkerPhase::Finished);
    }

    #[test]
    fn active_mode_filters_terminal() {
        // With scan_workers reconciliation (no Docker in tests), non-terminal
        // workers get reconciled to failed. So active mode shows 0 tasks when
        // all workers are terminal. We test that active mode correctly shows
        // nothing and archive mode shows both terminal workers.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();

        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let finished = state::WorkerState {
            repo: "test/repo".to_string(),
            pr_num: 1,
            issues: vec![],
            branch: "b".to_string(),
            container_id: "c".to_string(),
            phase: WorkerPhase::Finished,
            heartbeat: now.clone(),
            started: now.clone(),
            ended: Some(now.clone()),
            exit_code: Some(0),
            error: None,
            file_path: state::state_file_path(dir.path(), "test/repo", 1),
        };
        let failed = state::WorkerState {
            repo: "test/repo".to_string(),
            pr_num: 2,
            issues: vec![],
            branch: "b".to_string(),
            container_id: "c".to_string(),
            phase: WorkerPhase::Failed,
            heartbeat: now.clone(),
            started: now.clone(),
            ended: Some(now),
            exit_code: Some(1),
            error: None,
            file_path: state::state_file_path(dir.path(), "test/repo", 2),
        };
        state::write_state(&finished).unwrap();
        state::write_state(&failed).unwrap();

        let mut app = App {
            sipag_dir: dir.path().to_path_buf(),
            tasks: vec![],
            selected: 0,
            view: View::List,
            log_lines: vec![],
            log_scroll: 0,
            attach_request: None,
            list_mode: ListMode::Active,
            archive_max_age_days: 7,
        };
        app.refresh_tasks().unwrap();

        // Active mode shows no terminal workers.
        assert_eq!(app.tasks.len(), 0);

        // Archive mode shows both.
        app.list_mode = ListMode::Archive;
        app.refresh_tasks().unwrap();
        assert_eq!(app.tasks.len(), 2);
    }

    #[test]
    fn toggle_list_mode_switches_visible_tasks() {
        // Use terminal phases since scan_workers reconciles non-terminal workers
        // against Docker (no Docker in tests → reconciled to failed).
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();

        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let finished = state::WorkerState {
            repo: "test/repo".to_string(),
            pr_num: 1,
            issues: vec![],
            branch: "b".to_string(),
            container_id: "c".to_string(),
            phase: WorkerPhase::Finished,
            heartbeat: now.clone(),
            started: now.clone(),
            ended: Some(now.clone()),
            exit_code: Some(0),
            error: None,
            file_path: state::state_file_path(dir.path(), "test/repo", 1),
        };
        state::write_state(&finished).unwrap();

        let mut app = App {
            sipag_dir: dir.path().to_path_buf(),
            tasks: vec![],
            selected: 0,
            view: View::List,
            log_lines: vec![],
            log_scroll: 0,
            attach_request: None,
            list_mode: ListMode::Active,
            archive_max_age_days: 99999,
        };
        app.refresh_tasks().unwrap();

        // Active mode: no terminal workers shown.
        assert_eq!(app.tasks.len(), 0);

        app.toggle_list_mode();

        // Archive mode: terminal workers shown.
        assert_eq!(app.list_mode, ListMode::Archive);
        assert_eq!(app.tasks.len(), 1);
        assert_eq!(app.tasks[0].phase, WorkerPhase::Finished);
    }

    #[test]
    fn archive_auto_hides_old_entries() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();

        let old = state::WorkerState {
            repo: "test/repo".to_string(),
            pr_num: 3,
            issues: vec![],
            branch: "b".to_string(),
            container_id: "c".to_string(),
            phase: WorkerPhase::Finished,
            heartbeat: String::new(),
            started: "2000-01-01T00:00:00Z".to_string(),
            ended: Some("2000-01-01T01:00:00Z".to_string()),
            exit_code: Some(0),
            error: None,
            file_path: state::state_file_path(dir.path(), "test/repo", 3),
        };
        state::write_state(&old).unwrap();

        let mut app = App {
            sipag_dir: dir.path().to_path_buf(),
            tasks: vec![],
            selected: 0,
            view: View::List,
            log_lines: vec![],
            log_scroll: 0,
            attach_request: None,
            list_mode: ListMode::Archive,
            archive_max_age_days: 7,
        };
        app.refresh_tasks().unwrap();

        assert_eq!(app.tasks.len(), 0);
    }

    #[test]
    fn dismiss_selected_removes_state_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("workers")).unwrap();

        let file_path = state::state_file_path(dir.path(), "test/repo", 5);
        let s = state::WorkerState {
            repo: "test/repo".to_string(),
            pr_num: 5,
            issues: vec![],
            branch: "b".to_string(),
            container_id: "c".to_string(),
            phase: WorkerPhase::Finished,
            heartbeat: String::new(),
            started: "2026-01-15T10:00:00Z".to_string(),
            ended: Some("2026-01-15T10:05:00Z".to_string()),
            exit_code: Some(0),
            error: None,
            file_path: file_path.clone(),
        };
        state::write_state(&s).unwrap();

        let mut app = App {
            sipag_dir: dir.path().to_path_buf(),
            tasks: vec![],
            selected: 0,
            view: View::List,
            log_lines: vec![],
            log_scroll: 0,
            attach_request: None,
            list_mode: ListMode::Archive,
            archive_max_age_days: 99999,
        };
        app.refresh_tasks().unwrap();
        assert_eq!(app.tasks.len(), 1);

        app.dismiss_selected().unwrap();

        assert!(!file_path.exists());
        assert_eq!(app.tasks.len(), 0);
    }

    #[test]
    fn kill_selected_noop_on_terminal() {
        let task = make_task(1, WorkerPhase::Finished);
        let mut app = make_app_with_tasks(vec![task]);
        app.kill_selected().unwrap();
        assert_eq!(app.tasks[0].phase, WorkerPhase::Finished);
    }
}
