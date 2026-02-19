use crate::config::SipagConfig;
use crate::state::{self, FullState};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Cell, Paragraph, Row, Scrollbar, ScrollbarOrientation,
              ScrollbarState, Table, TableState, Wrap},
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    All,
    Active,
    Done,
    Failed,
}

impl FilterMode {
    fn next(self) -> Self {
        match self {
            FilterMode::All => FilterMode::Active,
            FilterMode::Active => FilterMode::Done,
            FilterMode::Done => FilterMode::Failed,
            FilterMode::Failed => FilterMode::All,
        }
    }

    fn label(self) -> &'static str {
        match self {
            FilterMode::All => "All",
            FilterMode::Active => "Active",
            FilterMode::Done => "Done",
            FilterMode::Failed => "Failed",
        }
    }

    fn matches(self, status: &str) -> bool {
        match self {
            FilterMode::All => true,
            FilterMode::Active => matches!(status, "claimed" | "running" | "pushing"),
            FilterMode::Done => status == "done",
            FilterMode::Failed => status == "failed",
        }
    }
}

pub enum View {
    List,
    Detail(usize),
    Log(usize, u16), // task index, scroll offset
}

pub enum AppMode {
    /// New multi-project mode: reads from ~/.sipag/
    MultiProject {
        sipag_home: PathBuf,
        project_filter: Option<String>,
    },
    /// Legacy single-project mode: reads from <project_dir>/.sipag.d/
    Legacy {
        project_dir: PathBuf,
    },
}

pub struct App {
    pub mode: AppMode,
    pub config: SipagConfig,
    pub state: FullState,
    pub filter: FilterMode,
    pub filtered_indices: Vec<usize>,
    pub table_state: TableState,
    pub view: View,
    pub last_refresh: Instant,
}

impl App {
    pub fn new(sipag_home: PathBuf, config: SipagConfig, project_filter: Option<String>) -> Result<Self> {
        let st = state::read_state(&sipag_home, &project_filter)?;
        let mut app = App {
            mode: AppMode::MultiProject {
                sipag_home,
                project_filter,
            },
            config,
            state: st,
            filter: FilterMode::All,
            filtered_indices: Vec::new(),
            table_state: TableState::default(),
            view: View::List,
            last_refresh: Instant::now(),
        };
        app.rebuild_filtered();
        if !app.filtered_indices.is_empty() {
            app.table_state.select(Some(0));
        }
        Ok(app)
    }

    pub fn new_legacy(project_dir: PathBuf, config: SipagConfig) -> Result<Self> {
        let st = state::read_state_legacy(&project_dir)?;
        let mut app = App {
            mode: AppMode::Legacy { project_dir },
            config,
            state: st,
            filter: FilterMode::All,
            filtered_indices: Vec::new(),
            table_state: TableState::default(),
            view: View::List,
            last_refresh: Instant::now(),
        };
        app.rebuild_filtered();
        if !app.filtered_indices.is_empty() {
            app.table_state.select(Some(0));
        }
        Ok(app)
    }

    pub fn refresh(&mut self) {
        let new_state = match &self.mode {
            AppMode::MultiProject { sipag_home, project_filter } => {
                state::read_state(sipag_home, project_filter)
            }
            AppMode::Legacy { project_dir } => {
                state::read_state_legacy(project_dir)
            }
        };
        if let Ok(st) = new_state {
            self.state = st;
        }
        self.rebuild_filtered();
        self.last_refresh = Instant::now();
    }

    fn rebuild_filtered(&mut self) {
        self.filtered_indices = self
            .state
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| self.filter.matches(&t.status))
            .map(|(i, _)| i)
            .collect();

        if let Some(selected) = self.table_state.selected() {
            if selected >= self.filtered_indices.len() {
                if self.filtered_indices.is_empty() {
                    self.table_state.select(None);
                } else {
                    self.table_state.select(Some(self.filtered_indices.len() - 1));
                }
            }
        } else if !self.filtered_indices.is_empty() {
            self.table_state.select(Some(0));
        }
    }

    fn next_row(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let i = match self.table_state.selected() {
            Some(i) if i >= self.filtered_indices.len() - 1 => 0,
            Some(i) => i + 1,
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    fn previous_row(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let i = match self.table_state.selected() {
            Some(0) => self.filtered_indices.len() - 1,
            Some(i) => i - 1,
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    fn active_count(&self) -> usize {
        self.state
            .tasks
            .iter()
            .filter(|t| matches!(t.status.as_str(), "claimed" | "running" | "pushing"))
            .count()
    }

    fn is_multi_project(&self) -> bool {
        matches!(self.mode, AppMode::MultiProject { .. })
    }
}

// --- Event loop ---

pub fn run(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;

        if app.last_refresh.elapsed() >= Duration::from_secs(2) {
            app.refresh();
        }

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match &mut app.view {
                    View::List => match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('j') | KeyCode::Down => app.next_row(),
                        KeyCode::Char('k') | KeyCode::Up => app.previous_row(),
                        KeyCode::Tab => {
                            app.filter = app.filter.next();
                            app.rebuild_filtered();
                        }
                        KeyCode::Enter => {
                            if let Some(i) = app.table_state.selected() {
                                if let Some(&row_idx) = app.filtered_indices.get(i) {
                                    app.view = View::Detail(row_idx);
                                }
                            }
                        }
                        KeyCode::Char('l') => {
                            if let Some(i) = app.table_state.selected() {
                                if let Some(&row_idx) = app.filtered_indices.get(i) {
                                    app.view = View::Log(row_idx, 0);
                                }
                            }
                        }
                        _ => {}
                    },
                    View::Detail(_) => match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Esc | KeyCode::Backspace => app.view = View::List,
                        KeyCode::Char('l') => {
                            if let View::Detail(idx) = app.view {
                                app.view = View::Log(idx, 0);
                            }
                        }
                        _ => {}
                    },
                    View::Log(_, ref mut scroll) => match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Esc | KeyCode::Backspace => app.view = View::List,
                        KeyCode::Char('j') | KeyCode::Down => *scroll = scroll.saturating_add(1),
                        KeyCode::Char('k') | KeyCode::Up => *scroll = scroll.saturating_sub(1),
                        KeyCode::Char('G') | KeyCode::End => *scroll = u16::MAX,
                        KeyCode::Char('g') | KeyCode::Home => *scroll = 0,
                        _ => {}
                    },
                }
            }
        }
    }
}

// --- Rendering ---

fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(f.area());

    render_header(f, app, chunks[0]);

    match &app.view {
        View::List => render_list(f, app, chunks[1]),
        View::Detail(idx) => render_detail(f, app, *idx, chunks[1]),
        View::Log(idx, scroll) => render_log(f, app, *idx, *scroll, chunks[1]),
    }

    render_footer(f, app, chunks[2]);
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let daemon_status = if app.state.daemon.alive {
        let pid = app.state.daemon.pid.map_or("?".to_string(), |p| p.to_string());
        format!("● RUNNING (PID {})", pid)
    } else {
        "○ STOPPED".to_string()
    };

    let active = app.active_count();
    let max = app.config.max_workers;

    let header_text = if app.is_multi_project() {
        format!(
            " sipag  {}  workers: {}/{}  [{}]",
            daemon_status, active, max, app.filter.label()
        )
    } else {
        let repo = if app.config.repo.is_empty() {
            "unknown".to_string()
        } else {
            app.config.repo.clone()
        };
        format!(
            " sipag  {}  {}   workers: {}/{}  [{}]",
            repo, daemon_status, active, app.config.concurrency, app.filter.label()
        )
    };

    let header = Paragraph::new(header_text)
        .style(Style::default().fg(Color::White).bg(Color::DarkGray).bold());
    f.render_widget(header, area);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let text = match &app.view {
        View::List => " [j/k] nav  [Enter] detail  [l] log  [Tab] filter  [q] quit",
        View::Detail(_) => " [Esc] back  [l] log  [q] quit",
        View::Log(_, _) => " [Esc] back  [j/k] scroll  [G] bottom  [g] top  [q] quit",
    };
    let footer = Paragraph::new(text)
        .style(Style::default().fg(Color::White).bg(Color::DarkGray));
    f.render_widget(footer, area);
}

fn status_style(status: &str) -> Style {
    match status {
        "claimed" => Style::default().fg(Color::Yellow),
        "running" => Style::default().fg(Color::Cyan),
        "pushing" => Style::default().fg(Color::Blue),
        "done" => Style::default().fg(Color::Green),
        "failed" => Style::default().fg(Color::Red),
        _ => Style::default(),
    }
}

fn format_elapsed(started_at: &Option<String>) -> String {
    let Some(started) = started_at else {
        return "-".to_string();
    };
    let Ok(start) = chrono::DateTime::parse_from_rfc3339(started) else {
        // Try the format without timezone offset (worker writes Z suffix)
        let Ok(start) = chrono::NaiveDateTime::parse_from_str(started, "%Y-%m-%dT%H:%M:%SZ") else {
            return "-".to_string();
        };
        let start_utc = start.and_utc();
        let dur = chrono::Utc::now().signed_duration_since(start_utc);
        return format_duration(dur);
    };
    let dur = chrono::Utc::now().signed_duration_since(start);
    format_duration(dur)
}

fn format_duration(dur: chrono::Duration) -> String {
    let total_secs = dur.num_seconds().max(0);
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    if mins > 0 {
        format!("{}m {}s", mins, secs)
    } else {
        format!("{}s", secs)
    }
}

fn render_list(f: &mut Frame, app: &mut App, area: Rect) {
    if app.filtered_indices.is_empty() {
        let msg = match app.filter {
            FilterMode::All => "No tasks found. Waiting for workers...",
            FilterMode::Active => "No active tasks.",
            FilterMode::Done => "No completed tasks.",
            FilterMode::Failed => "No failed tasks.",
        };
        let p = Paragraph::new(msg)
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::TOP));
        f.render_widget(p, area);
        return;
    }

    let multi = app.is_multi_project();

    let header_labels: Vec<&str> = if multi {
        vec!["", "PROJECT", "ISSUE", "TITLE", "STATUS", "ELAPSED"]
    } else {
        vec!["", "ISSUE", "TITLE", "STATUS", "ELAPSED"]
    };
    let header_cells = header_labels
        .into_iter()
        .map(|h| Cell::from(h).style(Style::default().bold().fg(Color::Cyan)));
    let header = Row::new(header_cells).height(1);

    let rows: Vec<Row> = app
        .filtered_indices
        .iter()
        .map(|&idx| {
            let task = &app.state.tasks[idx];
            let ss = status_style(&task.status);
            let issue = format!("#{}", task.task_id);
            let title = truncate(&task.title, if multi { 35 } else { 40 });
            let elapsed = format_elapsed(&task.started_at);

            if multi {
                let project = truncate(&task.project, 15);
                Row::new(vec![
                    Cell::from(""),
                    Cell::from(project),
                    Cell::from(issue),
                    Cell::from(title),
                    Cell::from(task.status.clone()).style(ss),
                    Cell::from(elapsed),
                ])
            } else {
                Row::new(vec![
                    Cell::from(""),
                    Cell::from(issue),
                    Cell::from(title),
                    Cell::from(task.status.clone()).style(ss),
                    Cell::from(elapsed),
                ])
            }
        })
        .collect();

    let widths: Vec<Constraint> = if multi {
        vec![
            Constraint::Length(2),
            Constraint::Length(16),
            Constraint::Length(10),
            Constraint::Min(20),
            Constraint::Length(10),
            Constraint::Length(12),
        ]
    } else {
        vec![
            Constraint::Length(2),
            Constraint::Length(8),
            Constraint::Min(20),
            Constraint::Length(10),
            Constraint::Length(12),
        ]
    };

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::TOP))
        .row_highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("> ");

    f.render_stateful_widget(table, area, &mut app.table_state);
}

fn render_detail(f: &mut Frame, app: &App, idx: usize, area: Rect) {
    let Some(task) = app.state.tasks.get(idx) else {
        let p = Paragraph::new("No task selected.");
        f.render_widget(p, area);
        return;
    };

    let mut lines = Vec::new();

    if app.is_multi_project() {
        lines.push(Line::from(vec![
            Span::styled("Project:  ", Style::default().bold()),
            Span::raw(&task.project),
        ]));
    }

    lines.extend(vec![
        Line::from(vec![
            Span::styled("Issue:    ", Style::default().bold()),
            Span::raw(format!("#{}", task.task_id)),
        ]),
        Line::from(vec![
            Span::styled("Title:    ", Style::default().bold()),
            Span::raw(&task.title),
        ]),
        Line::from(vec![
            Span::styled("URL:      ", Style::default().bold()),
            Span::raw(&task.url),
        ]),
        Line::from(vec![
            Span::styled("Branch:   ", Style::default().bold()),
            Span::raw(&task.branch),
        ]),
        Line::from(vec![
            Span::styled("Status:   ", Style::default().bold()),
            Span::styled(&task.status, status_style(&task.status)),
        ]),
        Line::from(vec![
            Span::styled("Started:  ", Style::default().bold()),
            Span::raw(task.started_at.as_deref().unwrap_or("-")),
        ]),
        Line::from(vec![
            Span::styled("Elapsed:  ", Style::default().bold()),
            Span::raw(format_elapsed(&task.started_at)),
        ]),
    ]);

    if let Some(ref finished) = task.finished_at {
        lines.push(Line::from(vec![
            Span::styled("Finished: ", Style::default().bold()),
            Span::raw(finished),
        ]));
    }

    if let Some(ref pr) = task.pr_url {
        lines.push(Line::from(vec![
            Span::styled("PR:       ", Style::default().bold()),
            Span::styled(pr, Style::default().fg(Color::Green)),
        ]));
    }

    if let Some(ref error) = task.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Error:",
            Style::default().bold().fg(Color::Red),
        )));
        lines.push(Line::from(Span::styled(
            format!("  {}", error),
            Style::default().fg(Color::Red),
        )));
    }

    let detail = Paragraph::new(lines).block(
        Block::default()
            .title(format!(" Task #{} ", task.task_id))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );
    f.render_widget(detail, area);
}

fn render_log(f: &mut Frame, app: &App, idx: usize, scroll: u16, area: Rect) {
    let Some(task) = app.state.tasks.get(idx) else {
        let p = Paragraph::new("No task selected.");
        f.render_widget(p, area);
        return;
    };

    let content = match &app.mode {
        AppMode::MultiProject { sipag_home, .. } => {
            state::read_log_file(sipag_home, &task.project, &task.task_id)
        }
        AppMode::Legacy { project_dir } => {
            state::read_log_file_legacy(project_dir, &task.task_id)
        }
    };

    let lines: Vec<Line> = if content.is_empty() {
        vec![Line::from(Span::styled(
            "No log output yet...",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        content.lines().map(|l| Line::from(l.to_string())).collect()
    };

    let total_lines = lines.len() as u16;
    let visible = area.height.saturating_sub(2); // borders
    let max_scroll = total_lines.saturating_sub(visible);
    let actual_scroll = if scroll == u16::MAX {
        max_scroll
    } else {
        scroll.min(max_scroll)
    };

    let log = Paragraph::new(lines)
        .block(
            Block::default()
                .title(format!(" Log: worker-{}.log ", task.task_id))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false })
        .scroll((actual_scroll, 0));

    f.render_widget(log, area);

    // Scrollbar
    let mut scrollbar_state = ScrollbarState::new(max_scroll as usize)
        .position(actual_scroll as usize);
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
    f.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}
