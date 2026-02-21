use crate::app::App;
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table},
    Frame,
};

pub fn render_list(f: &mut Frame, app: &mut App) {
    let area = f.area();

    // Split into: header | table | bottom bar
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(0),
        Constraint::Length(3),
    ])
    .split(area);

    // ── Header bar ────────────────────────────────────────────────────────────
    let (running, done, failed) = app.worker_counts();
    let repos = app.active_repos();
    let repo_str = if repos.is_empty() {
        "no active repos".to_string()
    } else {
        repos.join(", ")
    };
    let header_text = format!(
        " sipag  │  {} running · {} done · {} failed  │  polling: {}",
        running, done, failed, repo_str
    );
    let header = Paragraph::new(Line::from(Span::styled(
        header_text,
        Style::default().add_modifier(Modifier::BOLD),
    )))
    .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(header, chunks[0]);

    // ── Table header ─────────────────────────────────────────────────────────
    let col_header = Row::new(vec![
        Cell::from("REPO"),
        Cell::from("ISSUE"),
        Cell::from("STATUS"),
        Cell::from("DURATION"),
        Cell::from("BRANCH / PR"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD))
    .height(1);

    // ── Worker rows ───────────────────────────────────────────────────────────
    let selected_idx = app.table_state.selected();
    let rows: Vec<Row> = app
        .workers
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let status_color = match w.status.as_str() {
                "running" => Color::Cyan,
                "done" => Color::Green,
                "failed" => Color::Red,
                _ => Color::White,
            };

            let row_style = if Some(i) == selected_idx {
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let status_style = if Some(i) == selected_idx {
                row_style
            } else {
                Style::default().fg(status_color)
            };

            let branch_col = match w.status.as_str() {
                "done" => w
                    .pr_num
                    .map(|n| format!("PR #{n} opened"))
                    .unwrap_or_else(|| w.branch.clone()),
                _ => w.branch.clone(),
            };

            Row::new(vec![
                Cell::from(w.repo.clone()).style(row_style),
                Cell::from(format!("#{}", w.issue_num)).style(row_style),
                Cell::from(w.status.clone()).style(status_style),
                Cell::from(w.format_duration()).style(row_style),
                Cell::from(branch_col).style(row_style),
            ])
            .height(1)
        })
        .collect();

    // Column widths
    let widths = [
        Constraint::Length(26),  // REPO
        Constraint::Length(8),   // ISSUE
        Constraint::Length(9),   // STATUS
        Constraint::Length(11),  // DURATION
        Constraint::Min(20),     // BRANCH / PR
    ];

    let table = Table::new(rows, widths)
        .header(col_header)
        .block(
            Block::default()
                .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                .border_type(BorderType::Plain),
        )
        // Disable automatic selection highlighting — rows are styled manually.
        .highlight_style(Style::default());

    f.render_stateful_widget(table, chunks[1], &mut app.table_state);

    // ── Bottom bar ────────────────────────────────────────────────────────────
    let help = "  j/k: navigate   Enter: view log   q: quit";
    let bottom = Paragraph::new(Line::from(help)).block(
        Block::default().borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM),
    );
    f.render_widget(bottom, chunks[2]);
}
