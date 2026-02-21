use crate::app::App;
use crate::task::Status;
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table},
    Frame,
};

pub fn render_list(f: &mut Frame, app: &App) {
    let area = f.area();

    // Split into: task table | bottom bar (legend + help).
    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(3)]).split(area);

    // ── Table header ─────────────────────────────────────────────────────────
    let header = Row::new(vec![
        Cell::from("ID"),
        Cell::from("St"),
        Cell::from("Pri"),
        Cell::from("Repo"),
        Cell::from("Title"),
        Cell::from("Age"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD))
    .height(1);

    // ── Task rows ─────────────────────────────────────────────────────────────
    let rows: Vec<Row> = app
        .tasks
        .iter()
        .enumerate()
        .map(|(i, task)| {
            let style = if i == app.selected {
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let pri = task
                .priority
                .as_deref()
                .map(|p| match p {
                    "high" => "H",
                    "low" => "L",
                    _ => "M",
                })
                .unwrap_or("-");

            Row::new(vec![
                Cell::from(format!("{}", task.id)),
                Cell::from(task.status.icon()),
                Cell::from(pri),
                Cell::from(task.repo.as_deref().unwrap_or("-")),
                Cell::from(task.title.as_str()),
                Cell::from(task.format_age()),
            ])
            .style(style)
            .height(1)
        })
        .collect();

    // Column widths
    let widths = [
        Constraint::Length(4),  // ID
        Constraint::Length(3),  // St
        Constraint::Length(4),  // Pri
        Constraint::Length(10), // Repo
        Constraint::Min(20),    // Title
        Constraint::Length(6),  // Age
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(" sipag ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );

    f.render_widget(table, chunks[0]);

    // ── Bottom bar ────────────────────────────────────────────────────────────
    let pending = app
        .tasks
        .iter()
        .filter(|t| t.status == Status::Queue)
        .count();
    let running = app
        .tasks
        .iter()
        .filter(|t| t.status == Status::Running)
        .count();
    let done = app
        .tasks
        .iter()
        .filter(|t| t.status == Status::Done)
        .count();
    let failed = app
        .tasks
        .iter()
        .filter(|t| t.status == Status::Failed)
        .count();

    let legend = format!(
        "  · pending  ⧖ running  ✓ done  ✗ failed    {} tasks ({} pending, {} running, {} done, {} failed)",
        app.tasks.len(),
        pending,
        running,
        done,
        failed,
    );
    let help = "  j/k:navigate  Enter:detail  q:quit";

    let bottom = Paragraph::new(vec![Line::from(legend), Line::from(help)]).block(
        Block::default().borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT),
    );

    f.render_widget(bottom, chunks[1]);
}
