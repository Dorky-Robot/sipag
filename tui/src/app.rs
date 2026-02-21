use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sipag_core::task::list_tasks;
use std::{fs, path::PathBuf};

use crate::task::{Status, Task};

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
}

impl App {
    pub fn new() -> Result<Self> {
        let sipag_dir = Self::resolve_sipag_dir();
        let mut app = Self {
            sipag_dir,
            tasks: vec![],
            selected: 0,
            view: View::List,
            log_lines: vec![],
            log_scroll: 0,
            attach_request: None,
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
        let task_files = list_tasks(&self.sipag_dir).unwrap_or_default();
        self.tasks = task_files.into_iter().map(Task::from).collect();
        // Clamp selection
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
    pub fn retry_task(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        let task = &self.tasks[self.selected];
        if task.status != Status::Failed {
            return;
        }

        let file_path = task.file_path.clone();

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
            KeyCode::Char('j') | KeyCode::Down => self.select_next(),
            KeyCode::Char('k') | KeyCode::Up => self.select_prev(),
            KeyCode::Enter => self.open_detail(),
            KeyCode::Char('a') => {
                if let Some(container) = self.selected_container_name() {
                    self.attach_request = Some(container);
                }
            }
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
        use crate::task::Task;
        use sipag_core::task::TaskStatus;

        let make_task = |n: u32| Task {
            id: n,
            title: format!("Task {}", n),
            repo: None,
            priority: None,
            source: None,
            added: None,
            body: String::new(),
            status: TaskStatus::Queue,
            issue: None,
            file_path: std::path::PathBuf::new(),
            container: None,
        };

        let mut app = App {
            sipag_dir: std::path::PathBuf::new(),
            tasks: vec![make_task(1), make_task(2), make_task(3)],
            selected: 0,
            view: View::List,
            log_lines: vec![],
            log_scroll: 0,
            attach_request: None,
        };

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
}
