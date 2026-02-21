use crate::app::App;
use crate::executor::LogKind;
use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

pub fn render_executor(f: &mut Frame, app: &mut App) {
    let Some(ref mut exec) = app.executor else {
        return;
    };

    let area = f.area();

    // ── Outer block ───────────────────────────────────────────────────────────
    let title = if exec.finished {
        " sipag ── executor [done] "
    } else {
        " sipag ── executor [running] "
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);
    let inner = block.inner(area);
    f.render_widget(block, area);

    // ── Split: log area (fills) + help bar (1 line) ───────────────────────────
    let chunks =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(inner);

    let log_rect = chunks[0];
    let help_rect = chunks[1];

    // Update viewport height so auto-scroll stays accurate.
    exec.viewport_height = log_rect.height;

    // ── Help bar ──────────────────────────────────────────────────────────────
    let help = if exec.auto_scroll {
        "  j:scroll-down  k:scroll-up  G:auto-scroll(on)  Esc:back"
    } else {
        "  j:scroll-down  k:scroll-up  G:auto-scroll(off)  Esc:back"
    };
    let help_para = Paragraph::new(help).style(Style::default().fg(Color::DarkGray));
    f.render_widget(help_para, help_rect);

    // ── Log lines ─────────────────────────────────────────────────────────────
    let viewport = log_rect.height as usize;
    let start = exec.scroll;
    let end = (start + viewport).min(exec.log_lines.len());

    let lines: Vec<Line> = exec.log_lines[start..end]
        .iter()
        .map(|ll| {
            let style = match ll.kind {
                LogKind::Commit => Style::default().fg(Color::Green),
                LogKind::Test => Style::default().fg(Color::Yellow),
                LogKind::Pr => Style::default().fg(Color::Cyan),
                LogKind::Error => Style::default().fg(Color::Red),
                LogKind::Summary(true) => Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
                LogKind::Summary(false) => Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
                LogKind::Normal => Style::default(),
            };
            Line::from(Span::styled(ll.text.clone(), style))
        })
        .collect();

    let log_para = Paragraph::new(lines);
    f.render_widget(log_para, log_rect);
}
