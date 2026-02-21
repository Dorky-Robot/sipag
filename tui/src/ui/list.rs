use crate::app::{App, ListMode};
use crate::task::Status;
use ratatui::{
    layout::{Alignment, Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
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

    let is_archive = app.list_mode == ListMode::Archive;

    // Count tasks by status (for header display)
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
    let mode_label = if is_archive { "[Archive]" } else { "[Active]" };
    let header_base = if is_archive {
        format!(
            " sipag {}  done: {}  failed: {}",
            mode_label, done_count, failed_count
        )
    } else {
        format!(
            " sipag {}  running: {}  queue: {}",
            mode_label, running_count, queue_count
        )
    };
    let header_style = Style::default()
        .fg(Color::White)
        .bg(Color::DarkGray)
        .add_modifier(Modifier::BOLD);

    let header_line = if !is_archive && app.draining {
        Line::from(vec![
            Span::styled(header_base, header_style),
            Span::styled(
                "  [DRAINING]",
                Style::default()
                    .fg(Color::Yellow)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
    } else {
        Line::from(Span::styled(header_base, header_style))
    };

    let header = Paragraph::new(header_line).style(Style::default().bg(Color::DarkGray));
    f.render_widget(header, chunks[0]);

    // ── Table column headers ──────────────────────────────────────────────────
    let since_label = if is_archive { "ENDED" } else { "SINCE" };
    let col_header = Row::new(vec![
        Cell::from("REPO"),
        Cell::from("ISSUE"),
        Cell::from("TITLE"),
        Cell::from("STATUS"),
        Cell::from(since_label),
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

            let age_str = if is_archive {
                task.format_ended_age()
            } else {
                task.format_age()
            };

            Row::new(vec![
                Cell::from(repo.to_string()),
                Cell::from(issue_str),
                Cell::from(task.title.clone()),
                Cell::from(task.status.name()).style(status_style),
                Cell::from(age_str),
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
        Constraint::Length(10), // SINCE / ENDED
    ];

    if app.tasks.is_empty() {
        let empty_msg = if is_archive {
            "\nNo archived workers.\n\nCompleted workers will appear here."
        } else {
            "\nNo workers running.\n\nStart with:  sipag work <owner/repo>"
        };
        let empty = Paragraph::new(empty_msg)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::TOP));
        f.render_widget(empty, chunks[1]);
    } else {
        let mut table_state = TableState::default().with_selected(Some(app.selected));
        let table = Table::new(rows, widths)
            .header(col_header)
            .block(Block::default().borders(Borders::TOP))
            .row_highlight_style(Style::default().bg(Color::DarkGray))
            .highlight_symbol("  > ");
        f.render_stateful_widget(table, chunks[1], &mut table_state);
    }

    // ── Footer bar ────────────────────────────────────────────────────────────
    let footer_text = if is_archive {
        " [Tab/a] active  [↑↓/j] nav  [Enter] details  [x] dismiss  [q] quit"
    } else {
        let selected_task = app.tasks.get(app.selected);
        let has_running = selected_task.is_some_and(|t| t.status == Status::Running);
        if has_running {
            " [Tab] archive  [↑↓/j] nav  [Enter] details  [a] attach  [d] drain  [k] kill  [K] kill all  [r] resume  [q] quit"
        } else {
            " [Tab/a] archive  [↑↓/j] nav  [Enter] details  [d] drain  [k] kill  [K] kill all  [r] resume  [q] quit"
        }
    };

    let footer = Paragraph::new(Line::from(footer_text))
        .style(Style::default().fg(Color::White).bg(Color::DarkGray));
    f.render_widget(footer, chunks[2]);
}
