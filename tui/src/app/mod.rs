mod input;
mod state;

pub use state::{parse_task_brief, Task, TaskStatus};

use anyhow::Result;
use ratatui::widgets::ListState;
use std::path::PathBuf;

use crate::executor::ExecutorState;

// ── Mode ──────────────────────────────────────────────────────────────────────

pub enum Mode {
    TaskList,
    Executor,
}

// ── App ───────────────────────────────────────────────────────────────────────

pub struct App {
    pub sipag_dir: PathBuf,
    pub sipag_bin: Option<PathBuf>,
    pub tasks: Vec<Task>,
    pub task_list_state: ListState,
    pub mode: Mode,
    pub executor: Option<ExecutorState>,
}

impl App {
    pub fn new() -> Result<Self> {
        let sipag_dir = Self::resolve_sipag_dir();
        let sipag_bin = Self::find_sipag_bin();
        let mut app = Self {
            sipag_dir,
            sipag_bin,
            tasks: Vec::new(),
            task_list_state: ListState::default(),
            mode: Mode::TaskList,
            executor: None,
        };
        app.refresh_tasks()?;
        if !app.tasks.is_empty() {
            app.task_list_state.select(Some(0));
        }
        Ok(app)
    }

    /// Drive the executor poll on each tick.
    pub fn on_tick(&mut self) -> Result<()> {
        if matches!(self.mode, Mode::Executor) || self.executor.is_some() {
            let sipag_dir = self.sipag_dir.clone();
            if let Some(ref mut exec) = self.executor {
                exec.poll(&sipag_dir);
            }
        }
        Ok(())
    }

    // ── Init helpers ──────────────────────────────────────────────────────────

    fn resolve_sipag_dir() -> PathBuf {
        if let Ok(dir) = std::env::var("SIPAG_DIR") {
            return PathBuf::from(dir);
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| String::from("."));
        PathBuf::from(home).join(".sipag")
    }

    fn find_sipag_bin() -> Option<PathBuf> {
        // 1. SIPAG_BIN env var.
        if let Ok(b) = std::env::var("SIPAG_BIN") {
            let p = PathBuf::from(b);
            if p.exists() {
                return Some(p);
            }
        }

        // 2. Relative to this binary: <exe>/../../../bin/sipag
        //    (tui/target/{profile}/sipag-tui → project/bin/sipag)
        if let Ok(exe) = std::env::current_exe() {
            let candidate = exe
                .parent() // profile dir (debug / release)
                .and_then(|p| p.parent()) // target/
                .and_then(|p| p.parent()) // tui/
                .and_then(|p| p.parent()) // project root
                .map(|p| p.join("bin").join("sipag"));
            if let Some(ref p) = candidate {
                if p.exists() {
                    return candidate;
                }
            }
        }

        // 3. Look for `sipag` in PATH.
        state::which_sipag()
    }
}
