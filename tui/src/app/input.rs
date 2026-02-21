/// Key event handling and mode-specific dispatch.
use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::process::Command;

use super::{App, Mode, TaskStatus};
use crate::executor::ExecutorState;

impl App {
    /// Dispatch a key event to the appropriate handler.
    /// Returns `true` if the app should quit.
    pub fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        match self.mode {
            Mode::TaskList => self.handle_task_list_key(key),
            Mode::Executor => self.handle_executor_key(key),
        }
    }

    fn handle_task_list_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.modifiers != KeyModifiers::NONE && key.modifiers != KeyModifiers::SHIFT {
            return Ok(false);
        }
        match key.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Char('j') | KeyCode::Down => self.select_next(),
            KeyCode::Char('k') | KeyCode::Up => self.select_prev(),
            KeyCode::Char('x') => self.start_executor()?,
            KeyCode::Char('r') => self.retry_selected()?,
            _ => {}
        }
        Ok(false)
    }

    fn handle_executor_key(&mut self, key: KeyEvent) -> Result<bool> {
        let Some(ref mut exec) = self.executor else {
            return Ok(false);
        };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                exec.auto_scroll = false;
                exec.scroll = exec.scroll.saturating_add(1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                exec.auto_scroll = false;
                exec.scroll = exec.scroll.saturating_sub(1);
            }
            KeyCode::Char('G') => {
                exec.auto_scroll = true;
            }
            KeyCode::Esc => {
                // Return to task list without stopping the executor.
                self.mode = Mode::TaskList;
            }
            _ => {}
        }
        Ok(false)
    }

    // ── Actions ───────────────────────────────────────────────────────────────

    fn retry_selected(&mut self) -> Result<()> {
        let Some(idx) = self.task_list_state.selected() else {
            return Ok(());
        };
        let Some(task) = self.tasks.get(idx) else {
            return Ok(());
        };
        if task.status != TaskStatus::Failed {
            return Ok(());
        }
        let task_id = task.id.clone();
        let sipag_dir = self.sipag_dir.clone();
        let bin = self.sipag_bin.clone();
        let mut cmd = if let Some(b) = bin {
            Command::new(b)
        } else {
            Command::new("sipag")
        };
        cmd.args(["retry", &task_id])
            .env("SIPAG_DIR", sipag_dir)
            .output()
            .ok();
        self.refresh_tasks()?;
        Ok(())
    }

    fn start_executor(&mut self) -> Result<()> {
        // If already running, just switch to the executor view.
        if self.executor.is_some() {
            self.mode = Mode::Executor;
            return Ok(());
        }

        let sipag_dir = self.sipag_dir.clone();
        let bin = self.sipag_bin.clone();
        let mut cmd = if let Some(b) = bin {
            Command::new(b)
        } else {
            Command::new("sipag")
        };
        cmd.arg("start")
            .env("SIPAG_DIR", &sipag_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        let child = cmd.spawn().ok(); // If spawn fails, we still open the view.
        self.executor = Some(ExecutorState::new(child));
        self.mode = Mode::Executor;
        Ok(())
    }
}
