use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use sipag_core::task::{list_tasks, TaskFile, TaskStatus};
use std::io::{stdout, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

// ── Mode ─────────────────────────────────────────────────────────────────────

enum Mode {
    TaskList,
    LogView,
}

// ── Log view state ────────────────────────────────────────────────────────────

struct LogView {
    task_id: String,
    /// Complete log lines (fully newline-terminated).
    lines: Vec<String>,
    /// Buffered partial last line (no trailing newline yet).
    partial: String,
    /// Index of the first visible line (top of viewport).
    scroll: usize,
    /// When true, always show the bottom of the log.
    auto_scroll: bool,
    /// Open file handle for streaming new content.
    file: Option<std::fs::File>,
    /// Current read position in the file.
    pos: u64,
    /// True while the task is still in running/.
    is_running: bool,
}

impl LogView {
    /// Append raw bytes to the line buffer, handling partial lines correctly.
    fn append_bytes(&mut self, bytes: &[u8]) {
        let s = String::from_utf8_lossy(bytes);
        let combined = format!("{}{}", self.partial, s);
        let has_trailing_newline = combined.ends_with('\n');
        let mut segs: Vec<String> = combined.split('\n').map(|s| s.to_string()).collect();
        if has_trailing_newline {
            // The trailing empty segment from split is noise; remove it.
            segs.pop();
            self.partial.clear();
        } else {
            // Last segment is incomplete; hold it until the next read.
            self.partial = segs.pop().unwrap_or_default();
        }
        self.lines.extend(segs);
    }
}

// ── App ───────────────────────────────────────────────────────────────────────

struct App {
    tasks: Vec<TaskFile>,
    state: ListState,
    mode: Mode,
    log_view: Option<LogView>,
}

impl App {
    fn new(tasks: Vec<TaskFile>) -> Self {
        let mut state = ListState::default();
        if !tasks.is_empty() {
            state.select(Some(0));
        }
        Self {
            tasks,
            state,
            mode: Mode::TaskList,
            log_view: None,
        }
    }

    fn next(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        let i = match self.state.selected() {
            Some(i) => (i + 1) % self.tasks.len(),
            None => 0,
        };
        self.state.select(Some(i));
    }

    fn previous(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        let i = match self.state.selected() {
            Some(0) | None => self.tasks.len().saturating_sub(1),
            Some(i) => i - 1,
        };
        self.state.select(Some(i));
    }

    fn selected_task(&self) -> Option<&TaskFile> {
        self.state.selected().and_then(|i| self.tasks.get(i))
    }

    /// Open the log viewer for the currently selected task.
    fn open_log_view(&mut self, sipag_dir: &Path) {
        let Some(task) = self.selected_task() else {
            return;
        };
        let task_id = task.name.clone();

        // Locate the log file (prefer running/, then done/, failed/)
        let mut log_path = None;
        let mut is_running = false;
        for (subdir, running) in &[("running", true), ("done", false), ("failed", false)] {
            let candidate = sipag_dir.join(subdir).join(format!("{task_id}.log"));
            if candidate.exists() {
                log_path = Some(candidate);
                is_running = *running;
                break;
            }
        }

        let Some(log_path) = log_path else {
            return; // No log yet for this task
        };

        let file = std::fs::File::open(&log_path).ok();
        self.log_view = Some(LogView {
            task_id,
            lines: Vec::new(),
            partial: String::new(),
            scroll: 0,
            auto_scroll: true,
            file,
            pos: 0,
            is_running,
        });
        self.mode = Mode::LogView;
    }

    /// Poll the open log file for new content and update is_running status.
    fn poll_log(&mut self, sipag_dir: &Path) {
        let Some(lv) = self.log_view.as_mut() else {
            return;
        };

        // Read any new bytes
        if let Some(ref mut file) = lv.file {
            let _ = file.seek(SeekFrom::Start(lv.pos));
            let mut buf = Vec::new();
            if let Ok(n) = file.read_to_end(&mut buf) {
                if n > 0 {
                    lv.pos += n as u64;
                    lv.append_bytes(&buf);
                }
            }
        }

        // Detect task completion
        if lv.is_running {
            let tracking = sipag_dir.join("running").join(format!("{}.md", lv.task_id));
            if !tracking.exists() {
                lv.is_running = false;
                // The file was just renamed; do one extra read via the still-valid fd
                if let Some(ref mut file) = lv.file {
                    let _ = file.seek(SeekFrom::Start(lv.pos));
                    let mut buf = Vec::new();
                    if let Ok(n) = file.read_to_end(&mut buf) {
                        if n > 0 {
                            lv.pos += n as u64;
                            lv.append_bytes(&buf);
                        }
                    }
                }
                // Flush any partial line that remained
                if !lv.partial.is_empty() {
                    let p = std::mem::take(&mut lv.partial);
                    lv.lines.push(p);
                }
            }
        }

        // Auto-scroll to bottom
        if lv.auto_scroll && !lv.lines.is_empty() {
            lv.scroll = lv.lines.len().saturating_sub(1);
        }
    }
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn status_color(status: &TaskStatus) -> Color {
    match status {
        TaskStatus::Queue => Color::Yellow,
        TaskStatus::Running => Color::Cyan,
        TaskStatus::Done => Color::Green,
        TaskStatus::Failed => Color::Red,
    }
}

fn render(frame: &mut Frame, app: &mut App) {
    match app.mode {
        Mode::TaskList => render_task_list(frame, app),
        Mode::LogView => render_log_view(frame, app),
    }
}

fn render_task_list(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(5)])
        .split(area);

    // --- Task list ---
    let items: Vec<ListItem> = app
        .tasks
        .iter()
        .map(|task| {
            let color = status_color(&task.status);
            let repo_display = task
                .repo
                .as_deref()
                .and_then(|r| r.split('/').next_back())
                .unwrap_or("-");
            let line = Line::from(vec![
                Span::styled(
                    format!(" [{:7}] ", task.status.as_str()),
                    Style::default().fg(color),
                ),
                Span::styled(
                    format!("{:<12} ", &task.priority),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(format!("{:<20} ", repo_display)),
                Span::styled(task.title.clone(), Style::default()),
            ]);
            ListItem::new(line)
        })
        .collect();

    let task_count = app.tasks.len();
    let list = List::new(items)
        .block(
            Block::default()
                .title(format!(" sipag — {} task(s) ", task_count))
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::REVERSED),
        )
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(list, chunks[0], &mut app.state);

    // --- Detail / help pane ---
    let detail_text = if let Some(task) = app.selected_task() {
        let repo = task.repo.as_deref().unwrap_or("-");
        let source = task.source.as_deref().unwrap_or("-");
        format!(
            " Name: {}  |  Repo: {}  |  Priority: {}  |  Source: {}\n {}",
            task.name, repo, task.priority, source, task.title
        )
    } else {
        " No tasks".to_string()
    };

    let help = Paragraph::new(vec![
        Line::from(Span::styled(detail_text, Style::default().fg(Color::White))),
        Line::from(Span::styled(
            " ↑/k: up   ↓/j: down   Enter: view log   q/Esc: quit   r: refresh",
            Style::default().fg(Color::DarkGray),
        )),
    ])
    .block(Block::default().borders(Borders::ALL).title(" Detail "));

    frame.render_widget(help, chunks[1]);
}

fn render_log_view(frame: &mut Frame, app: &mut App) {
    let Some(lv) = &app.log_view else {
        return;
    };

    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);

    let log_height = chunks[0].height.saturating_sub(2) as usize; // subtract borders
    let total = lv.lines.len();

    // Also count the in-progress partial line as a visible line
    let display_total = total + if lv.partial.is_empty() { 0 } else { 1 };

    let start = if lv.auto_scroll {
        display_total.saturating_sub(log_height)
    } else {
        lv.scroll.min(display_total.saturating_sub(log_height))
    };
    let end = (start + log_height).min(total);

    let mut text_lines: Vec<Line> = lv.lines[start..end]
        .iter()
        .map(|l| Line::from(l.as_str()))
        .collect();

    // Show the partial last line (if visible in the viewport)
    if !lv.partial.is_empty() && end >= total {
        text_lines.push(Line::from(Span::styled(
            lv.partial.as_str(),
            Style::default().fg(Color::DarkGray),
        )));
    }

    let status_indicator = if lv.is_running {
        " [following]"
    } else {
        " [done]"
    };
    let title = format!(" {} {}", lv.task_id, status_indicator);

    let paragraph =
        Paragraph::new(text_lines).block(Block::default().title(title).borders(Borders::ALL));

    frame.render_widget(paragraph, chunks[0]);

    // Help bar
    let help_text = if lv.is_running {
        " Esc/q: back   j/↓: scroll down   k/↑: scroll up   G: follow bottom"
    } else {
        " Esc/q: back   j/↓: scroll down   k/↑: scroll up"
    };
    let help = Paragraph::new(Line::from(Span::styled(
        help_text,
        Style::default().fg(Color::DarkGray),
    )));
    frame.render_widget(help, chunks[1]);
}

// ── Event loop ────────────────────────────────────────────────────────────────

/// Run the interactive TUI, loading tasks from sipag_dir.
pub fn run_tui(sipag_dir: &Path) -> Result<()> {
    let tasks = list_tasks(sipag_dir).unwrap_or_default();
    let mut app = App::new(tasks);

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    loop {
        terminal.draw(|frame| render(frame, &mut app))?;

        // Poll for log updates even when there is no keyboard input
        if matches!(app.mode, Mode::LogView) {
            app.poll_log(sipag_dir);
        }

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                let quit = match app.mode {
                    Mode::TaskList => handle_task_list_key(&mut app, key, sipag_dir),
                    Mode::LogView => handle_log_view_key(&mut app, key),
                };
                if quit {
                    break;
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

fn handle_task_list_key(app: &mut App, key: event::KeyEvent, sipag_dir: &Path) -> bool {
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), _)
        | (KeyCode::Esc, _)
        | (KeyCode::Char('c'), KeyModifiers::CONTROL) => return true,
        (KeyCode::Down, _) | (KeyCode::Char('j'), _) => app.next(),
        (KeyCode::Up, _) | (KeyCode::Char('k'), _) => app.previous(),
        (KeyCode::Enter, _) => app.open_log_view(sipag_dir),
        (KeyCode::Char('r'), _) => {
            let idx = app.state.selected();
            app.tasks = list_tasks(sipag_dir).unwrap_or_default();
            app.state.select(idx.filter(|&i| i < app.tasks.len()));
        }
        _ => {}
    }
    false
}

fn handle_log_view_key(app: &mut App, key: event::KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.mode = Mode::TaskList;
            // Keep log_view so re-opening is instant if user presses Enter again
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if let Some(lv) = app.log_view.as_mut() {
                lv.auto_scroll = false;
                lv.scroll = lv.scroll.saturating_add(1);
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let Some(lv) = app.log_view.as_mut() {
                lv.auto_scroll = false;
                lv.scroll = lv.scroll.saturating_sub(1);
            }
        }
        KeyCode::Char('G') => {
            if let Some(lv) = app.log_view.as_mut() {
                lv.auto_scroll = true;
            }
        }
        _ => {}
    }
    false
}
