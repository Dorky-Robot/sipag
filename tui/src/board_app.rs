//! Kanban board app state — v4 multi-project task board.

use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sipag_core::board::{self, Task, TaskStatus};
use sipag_core::katulong;
use std::path::PathBuf;

/// The kanban board application state.
pub struct BoardApp {
    pub sipag_dir: PathBuf,

    // ── Projects ─────────────────────────────────────────────────────────────
    pub project_names: Vec<String>,
    pub active_project_idx: usize,

    // ── Board data ───────────────────────────────────────────────────────────
    pub statuses: Vec<String>,
    pub columns: Vec<Vec<Task>>,

    // ── Selection ────────────────────────────────────────────────────────────
    /// Which column is focused.
    pub col_idx: usize,
    /// Which task within the column is focused (by task ID for stability).
    pub selected_task_id: Option<u64>,
    /// Row offset within current column (used when ID not found).
    pub row_idx: usize,

    // ── Roles ────────────────────────────────────────────────────────────────
    pub role_names: Vec<String>,

    // ── Input mode ───────────────────────────────────────────────────────────
    pub input_mode: InputMode,
    pub input_buffer: String,

    // ── Status bar ───────────────────────────────────────────────────────────
    /// Transient status message shown in the footer (clears after a few ticks).
    pub status_message: Option<String>,
    status_message_ttl: u8,

    /// Tick counter for periodic refresh.
    tick_count: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputMode {
    Normal,
    /// Adding a new task — typing the title.
    AddTask,
    /// Moving a task — picking new status.
    MoveTask,
}

impl BoardApp {
    pub fn new() -> Result<Self> {
        Self::with_dir(sipag_core::config::default_sipag_dir())
    }

    pub fn with_dir(sipag_dir: PathBuf) -> Result<Self> {
        let mut app = Self {
            sipag_dir,
            project_names: vec![],
            active_project_idx: 0,
            statuses: vec![],
            columns: vec![],
            col_idx: 0,
            selected_task_id: None,
            row_idx: 0,
            role_names: vec![],
            input_mode: InputMode::Normal,
            input_buffer: String::new(),
            status_message: None,
            status_message_ttl: 0,
            tick_count: 0,
        };
        app.load_projects()?;
        app.load_board()?;
        Ok(app)
    }

    // ── Data loading ─────────────────────────────────────────────────────────

    fn load_projects(&mut self) -> Result<()> {
        self.project_names = board::list_project_names(&self.sipag_dir)?;

        // Try to set active project from config default.
        if let Ok(cfg) = board::BoardConfig::load(&self.sipag_dir) {
            if let Some(ref default) = cfg.default_project {
                if let Some(pos) = self.project_names.iter().position(|n| n == default) {
                    self.active_project_idx = pos;
                }
            }
        }
        Ok(())
    }

    pub fn load_board(&mut self) -> Result<()> {
        let Some(project_name) = self.project_names.get(self.active_project_idx).cloned() else {
            self.statuses.clear();
            self.columns.clear();
            self.role_names.clear();
            return Ok(());
        };

        // Load project config for statuses.
        match board::load_project(&self.sipag_dir, &project_name) {
            Ok(proj) => self.statuses = proj.statuses,
            Err(_) => {
                self.statuses = vec![
                    "backlog".to_string(),
                    "todo".to_string(),
                    "in-progress".to_string(),
                    "review".to_string(),
                    "done".to_string(),
                ];
            }
        }

        // Load all tasks.
        let tasks = board::list_tasks(&self.sipag_dir, &project_name, None).unwrap_or_default();

        // Distribute into columns.
        self.columns = self
            .statuses
            .iter()
            .map(|status| {
                let status_val = TaskStatus::parse(status);
                let mut col: Vec<Task> = tasks
                    .iter()
                    .filter(|t| t.status == status_val)
                    .cloned()
                    .collect();
                col.sort_by_key(|t| t.id);
                col
            })
            .collect();

        // Load roles.
        self.role_names = board::list_roles(&self.sipag_dir, &project_name)
            .unwrap_or_default()
            .into_iter()
            .map(|r| r.name)
            .collect();

        // Clamp column index.
        if self.col_idx >= self.statuses.len() {
            self.col_idx = 0;
        }

        // Re-anchor selection by task ID.
        self.anchor_selection();

        Ok(())
    }

    /// Find the selected task by ID, updating row_idx to match.
    fn anchor_selection(&mut self) {
        if let Some(id) = self.selected_task_id {
            if let Some(col) = self.columns.get(self.col_idx) {
                if let Some(pos) = col.iter().position(|t| t.id == id) {
                    self.row_idx = pos;
                    return;
                }
            }
            // Task not found in current column — search all columns.
            for (ci, col) in self.columns.iter().enumerate() {
                if let Some(pos) = col.iter().position(|t| t.id == id) {
                    self.col_idx = ci;
                    self.row_idx = pos;
                    return;
                }
            }
            // Task gone entirely.
            self.selected_task_id = None;
        }

        // Clamp row_idx.
        if let Some(col) = self.columns.get(self.col_idx) {
            if self.row_idx >= col.len() {
                self.row_idx = col.len().saturating_sub(1);
            }
            // Set selected_task_id from position.
            if let Some(task) = col.get(self.row_idx) {
                self.selected_task_id = Some(task.id);
            }
        }
    }

    pub fn active_project_name(&self) -> Option<&str> {
        self.project_names
            .get(self.active_project_idx)
            .map(|s| s.as_str())
    }

    pub fn selected_task(&self) -> Option<&Task> {
        self.columns
            .get(self.col_idx)
            .and_then(|col| col.get(self.row_idx))
    }

    #[cfg(test)]
    pub fn total_tasks(&self) -> usize {
        self.columns.iter().map(|c| c.len()).sum()
    }

    // ── Navigation ───────────────────────────────────────────────────────────

    pub fn nav_down(&mut self) {
        if let Some(col) = self.columns.get(self.col_idx) {
            if !col.is_empty() {
                self.row_idx = (self.row_idx + 1).min(col.len() - 1);
                self.selected_task_id = col.get(self.row_idx).map(|t| t.id);
            }
        }
    }

    pub fn nav_up(&mut self) {
        self.row_idx = self.row_idx.saturating_sub(1);
        if let Some(col) = self.columns.get(self.col_idx) {
            self.selected_task_id = col.get(self.row_idx).map(|t| t.id);
        }
    }

    pub fn nav_right(&mut self) {
        if !self.statuses.is_empty() {
            self.col_idx = (self.col_idx + 1).min(self.statuses.len() - 1);
            self.row_idx = 0;
            if let Some(col) = self.columns.get(self.col_idx) {
                self.selected_task_id = col.first().map(|t| t.id);
            }
        }
    }

    pub fn nav_left(&mut self) {
        self.col_idx = self.col_idx.saturating_sub(1);
        self.row_idx = 0;
        if let Some(col) = self.columns.get(self.col_idx) {
            self.selected_task_id = col.first().map(|t| t.id);
        }
    }

    pub fn next_project(&mut self) {
        if !self.project_names.is_empty() {
            self.active_project_idx = (self.active_project_idx + 1) % self.project_names.len();
            self.col_idx = 0;
            self.row_idx = 0;
            self.selected_task_id = None;
            let _ = self.load_board();
        }
    }

    // ── Actions ──────────────────────────────────────────────────────────────

    pub fn start_add_task(&mut self) {
        self.input_mode = InputMode::AddTask;
        self.input_buffer.clear();
    }

    pub fn confirm_add_task(&mut self) -> Result<()> {
        let title = self.input_buffer.trim().to_string();
        self.input_mode = InputMode::Normal;
        self.input_buffer.clear();

        if title.is_empty() {
            return Ok(());
        }

        let Some(project) = self.active_project_name().map(|s| s.to_string()) else {
            return Ok(());
        };

        let task = board::add_task(&self.sipag_dir, &project, &title, None, &[])?;
        self.selected_task_id = Some(task.id);
        self.load_board()?;
        Ok(())
    }

    pub fn cancel_input(&mut self) {
        self.input_mode = InputMode::Normal;
        self.input_buffer.clear();
    }

    pub fn start_move_task(&mut self) {
        if self.selected_task().is_some() {
            self.input_mode = InputMode::MoveTask;
            self.input_buffer.clear();
        }
    }

    pub fn confirm_move_task(&mut self) -> Result<()> {
        let status_input = self.input_buffer.trim().to_string();
        self.input_mode = InputMode::Normal;
        self.input_buffer.clear();

        if status_input.is_empty() {
            return Ok(());
        }

        let Some(project) = self.active_project_name().map(|s| s.to_string()) else {
            return Ok(());
        };

        let Some(task) = self.selected_task() else {
            return Ok(());
        };
        let task_id = task.id;

        // Match status against known statuses (prefix match).
        let target_status = self
            .statuses
            .iter()
            .find(|s| s.starts_with(&status_input) || *s == &status_input)
            .cloned()
            .unwrap_or(status_input);

        board::move_task(&self.sipag_dir, &project, task_id, &target_status)?;
        self.selected_task_id = Some(task_id);
        self.load_board()?;
        Ok(())
    }

    /// Set a transient status message (shown for ~3 seconds).
    fn set_status(&mut self, msg: String) {
        self.status_message = Some(msg);
        self.status_message_ttl = 15; // ~3s at 200ms tick rate
    }

    /// Dispatch the selected task via katulong crew API.
    pub fn dispatch_task(&mut self) -> Result<()> {
        let Some(project_name) = self.active_project_name().map(|s| s.to_string()) else {
            self.set_status("No project selected".to_string());
            return Ok(());
        };

        let Some(task) = self.selected_task() else {
            self.set_status("No task selected".to_string());
            return Ok(());
        };
        let task_id = task.id;
        let task_title = task.title.clone();
        let role_name = task.role.clone();

        // Load role template.
        let role = match board::Role::load(&self.sipag_dir, &project_name, &role_name) {
            Ok(r) => r,
            Err(_) => {
                self.set_status(format!("Role '{role_name}' not found"));
                return Ok(());
            }
        };

        // Connect to katulong.
        let client = match katulong::KatulongClient::from_remote_json() {
            Ok(c) => c,
            Err(_) => {
                self.set_status("Cannot connect to katulong".to_string());
                return Ok(());
            }
        };

        let session = katulong::session_name(&project_name, &role_name);

        // Create session.
        if let Err(e) = client.create_session(&session) {
            self.set_status(format!("Session create failed: {e}"));
            return Ok(());
        }

        // Worktree setup.
        if role.worktree {
            let wt_cmd = katulong::worktree_command(&project_name, task_id);
            let _ = client.exec_session(&session, &wt_cmd);
        }

        // Launch agent.
        let agent_cmd = katulong::agent_command(
            &project_name,
            task_id,
            &task_title,
            &role.command,
            role.worktree,
        );
        if let Err(e) = client.exec_session(&session, &agent_cmd) {
            self.set_status(format!("Agent launch failed: {e}"));
            return Ok(());
        }

        // Move task to in-progress.
        board::move_task(&self.sipag_dir, &project_name, task_id, "in-progress")?;
        self.selected_task_id = Some(task_id);
        self.load_board()?;

        self.set_status(format!("Dispatched #{task_id} to {session}"));
        Ok(())
    }

    /// Quick-move: advance the selected task to the next status column.
    pub fn move_task_forward(&mut self) -> Result<()> {
        let Some(project) = self.active_project_name().map(|s| s.to_string()) else {
            return Ok(());
        };
        let Some(task) = self.selected_task() else {
            return Ok(());
        };
        let task_id = task.id;

        // Find next status.
        if self.col_idx + 1 < self.statuses.len() {
            let next_status = self.statuses[self.col_idx + 1].clone();
            board::move_task(&self.sipag_dir, &project, task_id, &next_status)?;
            self.selected_task_id = Some(task_id);
            self.load_board()?;
        }
        Ok(())
    }

    // ── Key handling ─────────────────────────────────────────────────────────

    /// Returns true if the app should quit.
    pub fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        match self.input_mode {
            InputMode::Normal => self.handle_normal_key(key),
            InputMode::AddTask => self.handle_input_key(key, false),
            InputMode::MoveTask => self.handle_input_key(key, true),
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.modifiers != KeyModifiers::NONE && key.modifiers != KeyModifiers::SHIFT {
            return Ok(false);
        }
        match key.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Char('j') | KeyCode::Down => self.nav_down(),
            KeyCode::Char('k') | KeyCode::Up => self.nav_up(),
            KeyCode::Char('l') | KeyCode::Right => self.nav_right(),
            KeyCode::Char('h') | KeyCode::Left => self.nav_left(),
            KeyCode::Tab => self.next_project(),
            KeyCode::Char('a') => self.start_add_task(),
            KeyCode::Char('d') => self.dispatch_task()?,
            KeyCode::Char('m') => self.start_move_task(),
            KeyCode::Enter => self.move_task_forward()?,
            _ => {}
        }
        Ok(false)
    }

    fn handle_input_key(&mut self, key: KeyEvent, is_move: bool) -> Result<bool> {
        match key.code {
            KeyCode::Esc => self.cancel_input(),
            KeyCode::Enter => {
                if is_move {
                    self.confirm_move_task()?;
                } else {
                    self.confirm_add_task()?;
                }
            }
            KeyCode::Backspace => {
                self.input_buffer.pop();
            }
            KeyCode::Char(c) => {
                self.input_buffer.push(c);
            }
            _ => {}
        }
        Ok(false)
    }

    // ── Tick ──────────────────────────────────────────────────────────────────

    pub fn on_tick(&mut self) -> Result<()> {
        self.tick_count = self.tick_count.wrapping_add(1);

        // Decay status message.
        if self.status_message_ttl > 0 {
            self.status_message_ttl -= 1;
            if self.status_message_ttl == 0 {
                self.status_message = None;
            }
        }

        // Refresh board every 5 ticks (~1s at 200ms tick rate).
        if self.tick_count.is_multiple_of(5) && self.input_mode == InputMode::Normal {
            self.load_board()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        board::create_project(dir.path(), "testproj", "a/b", None).unwrap();
        // Set as default.
        let cfg = board::BoardConfig {
            default_project: Some("testproj".to_string()),
            katulong_url: None,
        };
        cfg.save(dir.path()).unwrap();
        dir
    }

    #[test]
    fn board_app_loads_empty_project() {
        let dir = setup_dir();
        let app = BoardApp::with_dir(dir.path().to_path_buf()).unwrap();
        assert_eq!(app.project_names, vec!["testproj"]);
        assert_eq!(app.statuses.len(), 5);
        assert_eq!(app.total_tasks(), 0);
    }

    #[test]
    fn board_app_loads_tasks_into_columns() {
        let dir = setup_dir();
        board::add_task(dir.path(), "testproj", "Task A", None, &[]).unwrap();
        board::add_task(dir.path(), "testproj", "Task B", None, &[]).unwrap();
        let t3 = board::add_task(dir.path(), "testproj", "Task C", None, &[]).unwrap();
        board::move_task(dir.path(), "testproj", t3.id, "in-progress").unwrap();

        let app = BoardApp::with_dir(dir.path().to_path_buf()).unwrap();
        assert_eq!(app.total_tasks(), 3);
        // todo column (index 1) should have 2 tasks.
        assert_eq!(app.columns[1].len(), 2);
        // in-progress column (index 2) should have 1 task.
        assert_eq!(app.columns[2].len(), 1);
    }

    #[test]
    fn board_app_nav_down_up() {
        let dir = setup_dir();
        board::add_task(dir.path(), "testproj", "A", None, &[]).unwrap();
        board::add_task(dir.path(), "testproj", "B", None, &[]).unwrap();

        let mut app = BoardApp::with_dir(dir.path().to_path_buf()).unwrap();
        // Move to todo column (index 1).
        app.nav_right();
        assert_eq!(app.col_idx, 1);
        assert_eq!(app.row_idx, 0);

        app.nav_down();
        assert_eq!(app.row_idx, 1);

        app.nav_up();
        assert_eq!(app.row_idx, 0);
    }

    #[test]
    fn board_app_next_project() {
        let dir = tempfile::tempdir().unwrap();
        board::create_project(dir.path(), "alpha", "a/a", None).unwrap();
        board::create_project(dir.path(), "beta", "b/b", None).unwrap();

        let mut app = BoardApp::with_dir(dir.path().to_path_buf()).unwrap();
        assert_eq!(app.active_project_name(), Some("alpha"));

        app.next_project();
        assert_eq!(app.active_project_name(), Some("beta"));

        app.next_project();
        assert_eq!(app.active_project_name(), Some("alpha"));
    }

    #[test]
    fn board_app_add_task_via_input() {
        let dir = setup_dir();
        let mut app = BoardApp::with_dir(dir.path().to_path_buf()).unwrap();

        app.start_add_task();
        assert_eq!(app.input_mode, InputMode::AddTask);

        app.input_buffer = "New task".to_string();
        app.confirm_add_task().unwrap();

        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.total_tasks(), 1);
    }

    #[test]
    fn board_app_move_task_forward() {
        let dir = setup_dir();
        board::add_task(dir.path(), "testproj", "A", None, &[]).unwrap();

        let mut app = BoardApp::with_dir(dir.path().to_path_buf()).unwrap();
        // Navigate to todo column where the task is.
        app.nav_right(); // col 1 = todo
        assert_eq!(app.col_idx, 1);
        assert!(app.selected_task().is_some());

        app.move_task_forward().unwrap();

        // Task should now be in in-progress (col 2).
        assert_eq!(app.columns[2].len(), 1);
        assert_eq!(app.columns[1].len(), 0);
    }

    #[test]
    fn board_app_empty_state() {
        let dir = tempfile::tempdir().unwrap();
        let app = BoardApp::with_dir(dir.path().to_path_buf()).unwrap();
        assert!(app.project_names.is_empty());
        assert!(app.statuses.is_empty());
        assert_eq!(app.total_tasks(), 0);
    }
}
