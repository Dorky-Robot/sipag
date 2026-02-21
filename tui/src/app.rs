use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;
pub use sipag_core::task::TaskStatus;
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    process::{Child, Command},
    time::Instant,
};

#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub repo: String,
    pub status: TaskStatus,
}

// ── Log lines ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum LogKind {
    Normal,
    Commit,
    Test,
    Pr,
    Error,
    Summary(bool), // true = success
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub text: String,
    pub kind: LogKind,
}

impl LogLine {
    /// Classify a raw log line by scanning for key event patterns.
    pub fn classify(text: &str) -> Self {
        let lower = text.to_lowercase();
        let kind = if Self::is_commit(&lower) {
            LogKind::Commit
        } else if Self::is_test(&lower) {
            LogKind::Test
        } else if Self::is_pr(&lower) {
            LogKind::Pr
        } else if Self::is_error(&lower) {
            LogKind::Error
        } else {
            LogKind::Normal
        };
        LogLine {
            text: text.to_string(),
            kind,
        }
    }

    fn is_commit(lower: &str) -> bool {
        lower.contains("git commit")
            || lower.contains("committed")
            || (lower.contains("commit") && lower.contains("sha"))
            || lower.contains("[new branch]")
            || (lower.contains("pushing") && lower.contains("branch"))
            || lower.contains("git push")
    }

    fn is_test(lower: &str) -> bool {
        lower.contains("cargo test")
            || lower.contains("npm test")
            || lower.contains("running tests")
            || lower.contains("test result")
            || lower.contains("tests passed")
            || lower.contains("tests failed")
            || (lower.contains("running") && lower.contains("test"))
    }

    fn is_pr(lower: &str) -> bool {
        lower.contains("pull request")
            || lower.contains("draft pr")
            || lower.contains("gh pr")
            || lower.contains("ready for review")
            || lower.contains("pr created")
            || lower.contains("pr #")
    }

    fn is_error(lower: &str) -> bool {
        lower.starts_with("error")
            || lower.contains("error:")
            || lower.contains("error[e")
            || lower.contains("panicked")
            || lower.contains("fatal:")
    }
}

// ── Executor state ────────────────────────────────────────────────────────────

/// Tracks the log file currently being tailed.
pub struct CurrentLog {
    pub task_title: String,
    pub log_path: PathBuf,
    pub file: fs::File,
    pub pos: u64,
    pub started: Instant,
}

pub struct ExecutorState {
    /// The sipag-start child process.
    pub process: Option<Child>,
    /// Log file being tailed right now (None between tasks).
    pub current: Option<CurrentLog>,
    /// All accumulated log lines (across all tasks in this run).
    pub log_lines: Vec<LogLine>,
    /// Raw scroll offset (line index of first visible line).
    pub scroll: usize,
    /// If true, always show the bottom of the log.
    pub auto_scroll: bool,
    /// Height of the log pane viewport (updated by ui::render).
    pub viewport_height: u16,
    /// True once the process has exited.
    pub finished: bool,
}

impl ExecutorState {
    fn new(process: Option<Child>) -> Self {
        Self {
            process,
            current: None,
            log_lines: Vec::new(),
            scroll: 0,
            auto_scroll: true,
            viewport_height: 20,
            finished: false,
        }
    }
}

// ── App mode ──────────────────────────────────────────────────────────────────

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
    /// Set when the user presses 'a' on a running task. The main loop
    /// reads this, suspends the TUI, and runs `docker exec` to attach.
    pub attach_request: Option<String>,
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
            attach_request: None,
        };
        app.refresh_tasks()?;
        if !app.tasks.is_empty() {
            app.task_list_state.select(Some(0));
        }
        Ok(app)
    }

    fn resolve_sipag_dir() -> PathBuf {
        if let Ok(dir) = std::env::var("SIPAG_DIR") {
            return PathBuf::from(dir);
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| String::from("."));
        PathBuf::from(home).join(".sipag")
    }

    fn find_sipag_bin() -> Option<PathBuf> {
        // 1. SIPAG_BIN env var
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

        // 3. Look for `sipag` in PATH
        which_sipag()
    }

    // ── Task list ─────────────────────────────────────────────────────────────

    pub fn refresh_tasks(&mut self) -> Result<()> {
        let mut tasks = Vec::new();
        for (status, subdir) in &[
            (TaskStatus::Queue, "queue"),
            (TaskStatus::Running, "running"),
            (TaskStatus::Done, "done"),
            (TaskStatus::Failed, "failed"),
        ] {
            let dir = self.sipag_dir.join(subdir);
            if !dir.exists() {
                continue;
            }
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            let mut paths: Vec<PathBuf> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|ext| ext == "md"))
                .collect();
            paths.sort();
            for path in paths {
                let id = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let (title, repo) = parse_task_brief(&path);
                tasks.push(Task {
                    id,
                    title,
                    repo,
                    status: status.clone(),
                });
            }
        }
        self.tasks = tasks;

        // Clamp selection
        let selected = self.task_list_state.selected().unwrap_or(0);
        if self.tasks.is_empty() {
            self.task_list_state.select(None);
        } else if selected >= self.tasks.len() {
            self.task_list_state.select(Some(self.tasks.len() - 1));
        }
        Ok(())
    }

    // ── Key handling ──────────────────────────────────────────────────────────

    /// Returns true if the app should quit.
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
            KeyCode::Char('a') => {
                if let Some(container) = self.selected_container_name() {
                    self.attach_request = Some(container);
                }
            }
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
                // Return to task list without stopping the executor
                self.mode = Mode::TaskList;
            }
            _ => {}
        }
        Ok(false)
    }

    fn select_next(&mut self) {
        let len = self.tasks.len();
        if len == 0 {
            return;
        }
        let next = self
            .task_list_state
            .selected()
            .map(|i| (i + 1).min(len - 1))
            .unwrap_or(0);
        self.task_list_state.select(Some(next));
    }

    fn select_prev(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        let prev = self
            .task_list_state
            .selected()
            .map(|i| i.saturating_sub(1))
            .unwrap_or(0);
        self.task_list_state.select(Some(prev));
    }

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

    // ── Executor start ────────────────────────────────────────────────────────

    fn start_executor(&mut self) -> Result<()> {
        // If already in executor view, just switch back to it
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

        let child = cmd.spawn().ok(); // If spawn fails, we still show the view

        self.executor = Some(ExecutorState::new(child));
        self.mode = Mode::Executor;
        Ok(())
    }

    // ── Tick: poll executor ───────────────────────────────────────────────────

    pub fn on_tick(&mut self) -> Result<()> {
        if matches!(self.mode, Mode::Executor) || self.executor.is_some() {
            self.poll_executor()?;
        }
        Ok(())
    }

    fn poll_executor(&mut self) -> Result<()> {
        let Some(exec) = self.executor.as_mut() else {
            return Ok(());
        };

        // ── Check process status ──────────────────────────────────────────────
        if let Some(ref mut child) = exec.process {
            match child.try_wait() {
                Ok(Some(_)) => {
                    exec.process = None;
                    exec.finished = true;
                }
                Ok(None) => {} // still running
                Err(_) => {
                    exec.process = None;
                    exec.finished = true;
                }
            }
        } else {
            exec.finished = true;
        }

        // ── Check if current log file has been completed (moved out of running/) ─
        let sipag_dir = self.sipag_dir.clone();
        let exec = self.executor.as_mut().unwrap();

        if let Some(ref cur) = exec.current {
            if !cur.log_path.exists() {
                // Task completed: check done/ and failed/ for the outcome
                let stem = cur.log_path.file_stem().unwrap_or_default().to_os_string();
                let done_log = sipag_dir.join("done").join(&stem).with_extension("log");
                let failed_log = sipag_dir.join("failed").join(&stem).with_extension("log");

                let (success, source_log) = if done_log.exists() {
                    (true, Some(done_log))
                } else if failed_log.exists() {
                    (false, Some(failed_log))
                } else {
                    (false, None)
                };

                // Read any remaining bytes from the completed log
                if let Some(log_path) = source_log {
                    if let Ok(mut f) = fs::File::open(&log_path) {
                        let pos = exec.current.as_ref().map(|c| c.pos).unwrap_or(0);
                        let _ = f.seek(SeekFrom::Start(pos));
                        let mut buf = String::new();
                        if f.read_to_string(&mut buf).is_ok() {
                            for line in buf.lines() {
                                exec.log_lines.push(LogLine::classify(line));
                            }
                        }
                    }
                }

                // Add summary line
                let duration = exec
                    .current
                    .as_ref()
                    .map(|c| c.started.elapsed())
                    .unwrap_or_default();
                let title = exec
                    .current
                    .as_ref()
                    .map(|c| c.task_title.clone())
                    .unwrap_or_default();
                let secs = duration.as_secs();
                let (mins, secs) = (secs / 60, secs % 60);
                let summary = if success {
                    format!("✓ Done: {title} ({mins}m {secs}s)")
                } else {
                    format!("✗ Failed: {title} ({mins}m {secs}s)")
                };
                exec.log_lines.push(LogLine {
                    text: summary,
                    kind: LogKind::Summary(success),
                });
                exec.current = None;

                if exec.auto_scroll {
                    exec.scroll = exec.log_lines.len().saturating_sub(1);
                }
            }
        }

        // ── Find a new log file in running/ ───────────────────────────────────
        let exec = self.executor.as_mut().unwrap();
        if exec.current.is_none() && !exec.finished {
            let running_dir = sipag_dir.join("running");
            if let Ok(entries) = fs::read_dir(&running_dir) {
                let log_path = entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .find(|p| p.extension().is_some_and(|x| x == "log"));

                if let Some(path) = log_path {
                    let md_path = path.with_extension("md");
                    let (task_title, _) = parse_task_brief(&md_path);

                    // Add a header line for the new task
                    exec.log_lines.push(LogLine {
                        text: format!("── {task_title} ──"),
                        kind: LogKind::Normal,
                    });

                    let file = fs::File::open(&path).ok();
                    if let Some(f) = file {
                        exec.current = Some(CurrentLog {
                            task_title,
                            log_path: path,
                            file: f,
                            pos: 0,
                            started: Instant::now(),
                        });
                    }
                }
            }
        }

        // ── Read new bytes from current log ───────────────────────────────────
        let exec = self.executor.as_mut().unwrap();
        if let Some(ref mut cur) = exec.current {
            let _ = cur.file.seek(SeekFrom::Start(cur.pos));
            let mut buf = String::new();
            if cur.file.read_to_string(&mut buf).is_ok() && !buf.is_empty() {
                cur.pos += buf.len() as u64;
                for line in buf.lines() {
                    exec.log_lines.push(LogLine::classify(line));
                }
            }
        }

        // ── Auto-scroll ───────────────────────────────────────────────────────
        let exec = self.executor.as_mut().unwrap();
        if exec.auto_scroll && !exec.log_lines.is_empty() {
            let visible = exec.viewport_height as usize;
            exec.scroll = exec.log_lines.len().saturating_sub(visible);
        }

        Ok(())
    }

    // ── Attach ────────────────────────────────────────────────────────────────

    /// Get the container name for the selected running task.
    pub fn selected_container_name(&self) -> Option<String> {
        let idx = self.task_list_state.selected()?;
        let task = self.tasks.get(idx)?;
        if task.status != TaskStatus::Running {
            return None;
        }
        // Read container: field from the tracking .md file
        let md_path = self
            .sipag_dir
            .join("running")
            .join(format!("{}.md", task.id));
        let content = fs::read_to_string(&md_path).ok()?;
        for line in content.lines() {
            if let Some(val) = line.strip_prefix("container:") {
                return Some(val.trim().to_string());
            }
        }
        None
    }

    // ── Accessors for ui ──────────────────────────────────────────────────────

    pub fn task_counts(&self) -> (usize, usize, usize, usize) {
        let mut q = 0;
        let mut r = 0;
        let mut d = 0;
        let mut f = 0;
        for t in &self.tasks {
            match t.status {
                TaskStatus::Queue => q += 1,
                TaskStatus::Running => r += 1,
                TaskStatus::Done => d += 1,
                TaskStatus::Failed => f += 1,
            }
        }
        (q, r, d, f)
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Read the first-line title and `repo:` field from a task .md file.
pub fn parse_task_brief(path: &PathBuf) -> (String, String) {
    let content = fs::read_to_string(path).unwrap_or_default();
    let mut in_fm = false;
    let mut fm_done = false;
    let mut repo = String::new();
    let mut title = String::new();
    let mut dashes = 0u32;

    for raw in content.lines() {
        let line = raw.trim();
        if !in_fm && !fm_done && line == "---" {
            in_fm = true;
            dashes += 1;
            continue;
        }
        if in_fm && line == "---" {
            in_fm = false;
            fm_done = true;
            dashes += 1;
            continue;
        }
        if in_fm {
            if let Some(v) = line.strip_prefix("repo:") {
                repo = v.trim().to_string();
            }
        } else if fm_done && !line.is_empty() {
            title = line.to_string();
            break;
        }
    }
    if dashes == 0 {
        // No frontmatter: first non-empty line is the title
        for raw in content.lines() {
            let line = raw.trim();
            if !line.is_empty() {
                title = line.to_string();
                break;
            }
        }
    }
    if title.is_empty() {
        title = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
    }
    (title, repo)
}

fn which_sipag() -> Option<PathBuf> {
    let output = Command::new("which").arg("sipag").output().ok()?;
    if output.status.success() {
        let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !s.is_empty() {
            return Some(PathBuf::from(s));
        }
    }
    None
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_normal_line() {
        let line = LogLine::classify("Cloning into '/work'...");
        assert_eq!(line.kind, LogKind::Normal);
        assert_eq!(line.text, "Cloning into '/work'...");
    }

    #[test]
    fn classify_commit_line() {
        let line = LogLine::classify("Ran git commit -m 'Add feature'");
        assert_eq!(line.kind, LogKind::Commit);
    }

    #[test]
    fn classify_test_line() {
        let line = LogLine::classify("Running cargo test --all-features");
        assert_eq!(line.kind, LogKind::Test);
    }

    #[test]
    fn classify_pr_line() {
        let line = LogLine::classify("Creating draft pull request #42");
        assert_eq!(line.kind, LogKind::Pr);
    }

    #[test]
    fn classify_error_line() {
        let line = LogLine::classify("error[E0308]: mismatched types");
        assert_eq!(line.kind, LogKind::Error);
    }

    #[test]
    fn classify_error_fatal() {
        let line = LogLine::classify("fatal: repository not found");
        assert_eq!(line.kind, LogKind::Error);
    }

    #[test]
    fn classify_git_push() {
        let line = LogLine::classify("Running git push origin my-branch");
        assert_eq!(line.kind, LogKind::Commit);
    }

    #[test]
    fn parse_task_brief_with_frontmatter() {
        use std::io::Write;
        let dir = std::env::temp_dir();
        let path = dir.join("test-task-brief.md");
        let mut f = fs::File::create(&path).unwrap();
        write!(
            f,
            "---\nrepo: myrepo\npriority: high\n---\nMy task title\n\nBody text here.\n"
        )
        .unwrap();
        let (title, repo) = parse_task_brief(&path);
        assert_eq!(title, "My task title");
        assert_eq!(repo, "myrepo");
        fs::remove_file(path).ok();
    }

    #[test]
    fn parse_task_brief_no_frontmatter() {
        use std::io::Write;
        let dir = std::env::temp_dir();
        let path = dir.join("test-task-brief-nofm.md");
        let mut f = fs::File::create(&path).unwrap();
        write!(f, "Simple task title\n\nBody here.").unwrap();
        let (title, repo) = parse_task_brief(&path);
        assert_eq!(title, "Simple task title");
        assert!(repo.is_empty());
        fs::remove_file(path).ok();
    }

    #[test]
    fn task_status_symbols() {
        assert_eq!(TaskStatus::Queue.symbol(), "·");
        assert_eq!(TaskStatus::Running.symbol(), "⧖");
        assert_eq!(TaskStatus::Done.symbol(), "✓");
        assert_eq!(TaskStatus::Failed.symbol(), "✗");
    }
}
