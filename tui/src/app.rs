use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;
use sipag_core::worker_state::{list_workers, WorkerState};
use std::{fs, path::PathBuf};

// ── View mode ─────────────────────────────────────────────────────────────────

pub enum View {
    List,
    Detail,
}

// ── App ───────────────────────────────────────────────────────────────────────

pub struct App {
    pub sipag_dir: PathBuf,
    /// All workers loaded from ~/.sipag/workers/*.json
    pub workers: Vec<WorkerState>,
    /// Table selection state
    pub table_state: TableState,
    pub view: View,
    /// Log lines for the currently selected worker (loaded on Enter)
    pub log_lines: Vec<String>,
    /// Scroll offset into log_lines
    pub log_scroll: usize,
    /// Viewport height for the log pane (updated by the renderer)
    pub log_viewport_height: u16,
}

impl App {
    pub fn new() -> Result<Self> {
        let sipag_dir = Self::resolve_sipag_dir();
        let mut app = Self {
            sipag_dir,
            workers: Vec::new(),
            table_state: TableState::default(),
            view: View::List,
            log_lines: Vec::new(),
            log_scroll: 0,
            log_viewport_height: 20,
        };
        app.refresh_workers()?;
        if !app.workers.is_empty() {
            app.table_state.select(Some(0));
        }
        Ok(app)
    }

    fn resolve_sipag_dir() -> PathBuf {
        if let Ok(dir) = std::env::var("SIPAG_DIR") {
            return PathBuf::from(dir);
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".sipag")
    }

    // ── Data ──────────────────────────────────────────────────────────────────

    pub fn refresh_workers(&mut self) -> Result<()> {
        let selected = self.table_state.selected();
        self.workers = list_workers(&self.sipag_dir)?;

        // Clamp selection so it stays valid
        if self.workers.is_empty() {
            self.table_state.select(None);
        } else if let Some(i) = selected {
            if i >= self.workers.len() {
                self.table_state.select(Some(self.workers.len() - 1));
            }
        }
        Ok(())
    }

    pub fn selected_worker(&self) -> Option<&WorkerState> {
        self.table_state.selected().and_then(|i| self.workers.get(i))
    }

    /// Load the log for the selected worker into `self.log_lines`.
    pub fn load_selected_log(&mut self) {
        self.log_lines.clear();
        self.log_scroll = 0;
        if let Some(w) = self.selected_worker() {
            let path = w.resolved_log_path();
            if let Ok(content) = fs::read_to_string(&path) {
                self.log_lines = content.lines().map(|l| l.to_string()).collect();
            }
        }
    }

    // ── Key handling ──────────────────────────────────────────────────────────

    /// Returns `true` when the app should quit.
    pub fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(true);
        }
        match self.view {
            View::List => self.handle_list_key(key),
            View::Detail => self.handle_detail_key(key),
        }
    }

    fn handle_list_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Char('j') | KeyCode::Down => self.select_next(),
            KeyCode::Char('k') | KeyCode::Up => self.select_prev(),
            KeyCode::Enter => {
                self.load_selected_log();
                self.view = View::Detail;
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_detail_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.view = View::List;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let max = self.log_lines.len().saturating_sub(self.log_viewport_height as usize);
                self.log_scroll = (self.log_scroll + 1).min(max);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.log_scroll = self.log_scroll.saturating_sub(1);
            }
            KeyCode::Char('G') => {
                let max = self.log_lines.len().saturating_sub(self.log_viewport_height as usize);
                self.log_scroll = max;
            }
            KeyCode::Char('g') => {
                self.log_scroll = 0;
            }
            // Reload log from disk (useful for running workers)
            KeyCode::Char('r') => {
                self.load_selected_log();
            }
            _ => {}
        }
        Ok(false)
    }

    fn select_next(&mut self) {
        let len = self.workers.len();
        if len == 0 {
            return;
        }
        let next = self
            .table_state
            .selected()
            .map(|i| (i + 1).min(len - 1))
            .unwrap_or(0);
        self.table_state.select(Some(next));
    }

    fn select_prev(&mut self) {
        if self.workers.is_empty() {
            return;
        }
        let prev = self
            .table_state
            .selected()
            .map(|i| i.saturating_sub(1))
            .unwrap_or(0);
        self.table_state.select(Some(prev));
    }

    // ── Stats ─────────────────────────────────────────────────────────────────

    /// Returns (running, done, failed) counts.
    pub fn worker_counts(&self) -> (usize, usize, usize) {
        let running = self.workers.iter().filter(|w| w.status == "running").count();
        let done = self.workers.iter().filter(|w| w.status == "done").count();
        let failed = self.workers.iter().filter(|w| w.status == "failed").count();
        (running, done, failed)
    }

    /// Unique repos with running workers.
    pub fn active_repos(&self) -> Vec<String> {
        let mut repos: Vec<String> = self
            .workers
            .iter()
            .filter(|w| w.status == "running")
            .map(|w| w.repo.clone())
            .collect();
        repos.sort();
        repos.dedup();
        repos
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use sipag_core::worker_state::WorkerState;

    fn make_worker(status: &str, issue_num: u64) -> WorkerState {
        WorkerState {
            repo: "Owner/repo".to_string(),
            issue_num,
            issue_title: format!("Issue {issue_num}"),
            branch: format!("feat/issue-{issue_num}"),
            pr_num: None,
            pr_url: None,
            status: status.to_string(),
            started_at: "2026-02-20T10:00:00Z".to_string(),
            ended_at: None,
            duration_s: None,
            exit_code: None,
            log_path: "/tmp/test.log".to_string(),
        }
    }

    #[test]
    fn worker_counts_empty() {
        let app = App {
            sipag_dir: PathBuf::from("/tmp"),
            workers: Vec::new(),
            table_state: TableState::default(),
            view: View::List,
            log_lines: Vec::new(),
            log_scroll: 0,
            log_viewport_height: 20,
        };
        assert_eq!(app.worker_counts(), (0, 0, 0));
    }

    #[test]
    fn worker_counts_mixed() {
        let app = App {
            sipag_dir: PathBuf::from("/tmp"),
            workers: vec![
                make_worker("running", 1),
                make_worker("running", 2),
                make_worker("done", 3),
                make_worker("failed", 4),
            ],
            table_state: TableState::default(),
            view: View::List,
            log_lines: Vec::new(),
            log_scroll: 0,
            log_viewport_height: 20,
        };
        assert_eq!(app.worker_counts(), (2, 1, 1));
    }
}
