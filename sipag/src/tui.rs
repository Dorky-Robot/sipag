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
use sipag_core::events::last_event;
use sipag_core::task::{list_tasks, TaskFile, TaskStatus};
use std::io::stdout;
use std::path::Path;

struct App {
    tasks: Vec<TaskFile>,
    state: ListState,
    sipag_dir: std::path::PathBuf,
}

impl App {
    fn new(tasks: Vec<TaskFile>, sipag_dir: std::path::PathBuf) -> Self {
        let mut state = ListState::default();
        if !tasks.is_empty() {
            state.select(Some(0));
        }
        Self { tasks, state, sipag_dir }
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
}

fn status_color(status: &TaskStatus) -> Color {
    match status {
        TaskStatus::Queue => Color::Yellow,
        TaskStatus::Running => Color::Cyan,
        TaskStatus::Done => Color::Green,
        TaskStatus::Failed => Color::Red,
    }
}

fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // Split into list pane (top) and detail/help pane (bottom)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(5)])
        .split(area);

    // Pre-capture sipag_dir to avoid borrow conflict inside the iterator closure.
    let sipag_dir = app.sipag_dir.clone();

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

            // For running tasks, show latest event progress inline.
            let progress: Option<String> = if task.status == TaskStatus::Running {
                let events_path = sipag_dir
                    .join("running")
                    .join(format!("{}.events", task.name));
                last_event(&events_path).map(|ev| format!("[{}] {}", ev.event, ev.msg))
            } else {
                None
            };

            let title_text = if let Some(ref prog) = progress {
                format!("{} — {}", task.title, prog)
            } else {
                task.title.clone()
            };

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
                Span::styled(title_text, Style::default()),
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
        Line::from(Span::styled(
            detail_text,
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            " ↑/k: up   ↓/j: down   q/Esc: quit   r: refresh",
            Style::default().fg(Color::DarkGray),
        )),
    ])
    .block(Block::default().borders(Borders::ALL).title(" Detail "));

    frame.render_widget(help, chunks[1]);
}

/// Run the interactive TUI, loading tasks from sipag_dir.
pub fn run_tui(sipag_dir: &Path) -> Result<()> {
    let tasks = list_tasks(sipag_dir).unwrap_or_default();
    let mut app = App::new(tasks, sipag_dir.to_path_buf());

    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    loop {
        terminal.draw(|frame| render(frame, &mut app))?;

        if let Event::Key(key) = event::read()? {
            match (key.code, key.modifiers) {
                (KeyCode::Char('q'), _)
                | (KeyCode::Esc, _)
                | (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,
                (KeyCode::Down, _) | (KeyCode::Char('j'), _) => app.next(),
                (KeyCode::Up, _) | (KeyCode::Char('k'), _) => app.previous(),
                (KeyCode::Char('r'), _) => {
                    // Refresh task list
                    let idx = app.state.selected();
                    app.tasks = list_tasks(&app.sipag_dir).unwrap_or_default();
                    app.state.select(idx.filter(|&i| i < app.tasks.len()));
                }
                _ => {}
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}
