use crate::task::{Status, Task};
use std::fs;

/// Which top-level view is currently active.
pub enum View {
    List,
    Detail,
}

pub struct App {
    pub tasks: Vec<Task>,
    /// Currently highlighted row in the list view.
    pub selected: usize,
    pub view: View,
    /// Lines from the companion `.log` file (capped to last 30).
    pub log_lines: Vec<String>,
    /// Vertical scroll offset within the log section of the detail view.
    pub log_scroll: usize,
}

impl App {
    pub fn new(tasks: Vec<Task>) -> Self {
        Self {
            tasks,
            selected: 0,
            view: View::List,
            log_lines: vec![],
            log_scroll: 0,
        }
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

        let Some(sipag_dir) = sipag_dir else { return };
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
            self.tasks[self.selected].status = Status::Pending;
        }

        self.close_detail();
    }
}
