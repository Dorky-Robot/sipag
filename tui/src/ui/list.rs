use crate::app::App;
use crate::task::Status;
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
    Frame,
};

pub fn render_list(f: &mut Frame, app: &App) {
    let area = f.area();

    // 3-part layout: header bar | body (table) | footer bar
    let chunks = Layout::vertical([
        Constraint::Length(1), // header bar
        Constraint::Min(5),    // body (table)
        Constraint::Length(1), // footer bar
    ])
    .split(area);

    // Count tasks by status
    let queue_count = app
        .tasks
        .iter()
        .filter(|t| t.status == Status::Queue)
        .count();
    let running_count = app
        .tasks
        .iter()
        .filter(|t| t.status == Status::Running)
        .count();
    let done_count = app
        .tasks
        .iter()
        .filter(|t| t.status == Status::Done)
        .count();
    let failed_count = app
        .tasks
        .iter()
        .filter(|t| t.status == Status::Failed)
        .count();

    // ── Header bar ────────────────────────────────────────────────────────────
    let header_text = format!(
        " sipag Status [All]  running: {}  done: {}  failed: {}  queue: {}",
        running_count, done_count, failed_count, queue_count
    );
    let header = Paragraph::new(Line::from(header_text)).style(
        Style::default()
            .fg(Color::White)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(header, chunks[0]);

    // ── Table column headers ──────────────────────────────────────────────────
    let col_header = Row::new(vec![
        Cell::from("REPO"),
        Cell::from("ISSUE"),
        Cell::from("TITLE"),
        Cell::from("STATUS"),
        Cell::from("SINCE"),
    ])
    .style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
    .height(1);

    // ── Task rows ─────────────────────────────────────────────────────────────
    let rows: Vec<Row> = app
        .tasks
        .iter()
        .map(|task| {
            let status_style = match task.status {
                Status::Queue => Style::default().fg(Color::Yellow),
                Status::Running => Style::default().fg(Color::Cyan),
                Status::Done => Style::default().fg(Color::Green),
                Status::Failed => Style::default().fg(Color::Red),
            };

            let issue_str = task
                .issue
                .map(|n| format!("#{}", n))
                .unwrap_or_else(|| "-".to_string());
            let repo = task.repo.as_deref().unwrap_or("-");

            Row::new(vec![
                Cell::from(repo.to_string()),
                Cell::from(issue_str),
                Cell::from(task.title.clone()),
                Cell::from(task.status.name()).style(status_style),
                Cell::from(task.format_age()),
            ])
            .height(1)
        })
        .collect();

    // Column widths (fixed per spec)
    let widths = [
        Constraint::Length(20), // REPO
        Constraint::Length(7),  // ISSUE
        Constraint::Min(20),    // TITLE (flexible)
        Constraint::Length(10), // STATUS
        Constraint::Length(10), // SINCE
    ];

    let mut table_state = TableState::default().with_selected(Some(app.selected));
    let table = Table::new(rows, widths)
        .header(col_header)
        .block(Block::default().borders(Borders::TOP))
        .row_highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("  > ");

    f.render_stateful_widget(table, chunks[1], &mut table_state);

    // ── Footer bar ────────────────────────────────────────────────────────────
    let selected_task = app.tasks.get(app.selected);
    let has_running = selected_task.is_some_and(|t| t.status == Status::Running);
    let has_failed = selected_task.is_some_and(|t| t.status == Status::Failed);

    let footer_text = if has_running {
        " [j/k] navigate  [Enter] details  [a] attach  [q] quit"
    } else if has_failed {
        " [j/k] navigate  [Enter] details  [r] retry  [q] quit"
    } else {
        " [j/k] navigate  [Enter] details  [q] quit"
    };

    let footer = Paragraph::new(Line::from(footer_text))
        .style(Style::default().fg(Color::White).bg(Color::DarkGray));
    f.render_widget(footer, chunks[2]);
}
