use crate::app::{App, TaskStatus};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table, TableState},
    Frame,
};

pub fn render_list(f: &mut Frame, app: &App) {
    let area = f.area();

    // Split into: task table | bottom bar (legend + help).
    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(3)]).split(area);

    // ── Table header ─────────────────────────────────────────────────────────
    let header = Row::new(vec![
        Cell::from("St"),
        Cell::from("Repo"),
        Cell::from("Title"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD))
    .height(1);

    // ── Task rows ─────────────────────────────────────────────────────────────
    let selected_idx = app.task_list_state.selected();
    let rows: Vec<Row> = app
        .tasks
        .iter()
        .enumerate()
        .map(|(i, task)| {
            let style = if Some(i) == selected_idx {
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let repo_display = if task.repo.is_empty() {
                "-"
            } else {
                task.repo.as_str()
            };

            Row::new(vec![
                Cell::from(task.status.symbol()),
                Cell::from(repo_display.to_string()),
                Cell::from(task.title.as_str()),
            ])
            .style(style)
            .height(1)
        })
        .collect();

    // Column widths
    let widths = [
        Constraint::Length(3),  // St
        Constraint::Length(14), // Repo
        Constraint::Min(20),    // Title
    ];

    let table = Table::new(rows, widths).header(header).block(
        Block::default()
            .title(" sipag ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded),
    );

    // Use a mutable clone of the table state for rendering (ratatui requires &mut)
    let mut table_state = TableState::default();
    table_state.select(selected_idx);
    f.render_stateful_widget(table, chunks[0], &mut table_state);

    // ── Bottom bar ────────────────────────────────────────────────────────────
    let (pending, running, done, failed) = app.task_counts();

    let legend = format!(
        "  · pending  ⧖ running  ✓ done  ✗ failed    {} tasks ({} pending, {} running, {} done, {} failed)",
        app.tasks.len(),
        pending,
        running,
        done,
        failed,
    );

    // Show attach hint when a running task is selected
    let has_running_selected = selected_idx
        .and_then(|i| app.tasks.get(i))
        .is_some_and(|t| t.status == TaskStatus::Running);

    let help = if has_running_selected {
        "  j/k:navigate  a:attach  x:execute  r:retry  q:quit"
    } else {
        "  j/k:navigate  x:execute  r:retry  q:quit"
    };

    let bottom = Paragraph::new(vec![Line::from(legend), Line::from(help)])
        .block(Block::default().borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT));

    f.render_widget(bottom, chunks[1]);
}
