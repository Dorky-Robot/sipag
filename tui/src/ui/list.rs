use crate::app::{App, TaskStatus};
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph},
    Frame,
};

pub fn render_list(f: &mut Frame, app: &mut App) {
    let area = f.area();

    // Split into: task list | bottom help bar.
    let chunks =
        Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).split(area);

    // ── Task list items ───────────────────────────────────────────────────────
    let items: Vec<ListItem> = app
        .tasks
        .iter()
        .map(|task| {
            let sym = task.status.symbol();
            let text = format!(" {} {}  {}", sym, task.id, task.title);
            let style = match task.status {
                TaskStatus::Queue => Style::default(),
                TaskStatus::Running => Style::default().fg(Color::Yellow),
                TaskStatus::Done => Style::default().fg(Color::Green),
                TaskStatus::Failed => Style::default().fg(Color::Red),
            };
            ListItem::new(text).style(style)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .title(" sipag ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(list, chunks[0], &mut app.task_list_state);

    // ── Bottom help bar ───────────────────────────────────────────────────────
    let (q, r, d, fai) = app.task_counts();
    let legend = format!(
        "  {} tasks  · {} queued  ⧖ {} running  ✓ {} done  ✗ {} failed",
        app.tasks.len(),
        q,
        r,
        d,
        fai,
    );
    let help = "  j/k:navigate  x:run-executor  r:retry-failed  q:quit";
    let bottom = Paragraph::new(vec![Line::from(legend), Line::from(help)]).block(
        Block::default().borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT),
    );
    f.render_widget(bottom, chunks[1]);
}
