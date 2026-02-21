use crate::app::App;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

use crate::app::{LogKind, LogLine};

pub fn render_executor(f: &mut Frame, app: &App) {
    let area = f.area();

    let Some(exec) = &app.executor else {
        let msg = Paragraph::new("No executor running.").block(
            Block::default()
                .title(" sipag ── executor ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );
        f.render_widget(msg, area);
        return;
    };

    // ── Outer border + title ──────────────────────────────────────────────────
    let status_label = if exec.finished { "finished" } else { "running" };
    let outer_title = format!(" sipag ── executor ({status_label}) ");
    let outer_block = Block::default()
        .title(outer_title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    // ── Help bar (bottom row) ────────────────────────────────────────────────
    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).split(area);

    let content_area = outer_block.inner(chunks[0]);
    f.render_widget(outer_block, chunks[0]);

    let help_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);
    let help_inner = help_block.inner(chunks[1]);
    f.render_widget(help_block, chunks[1]);

    let help_text =
        Paragraph::new(Line::from("  j/k:scroll  G:bottom  Esc:back")).style(Style::default());
    f.render_widget(help_text, help_inner);

    // ── Log lines ─────────────────────────────────────────────────────────────
    if exec.log_lines.is_empty() {
        let waiting = Paragraph::new("  Waiting for log output...");
        f.render_widget(waiting, content_area);
        return;
    }

    // Store viewport height for auto-scroll calculation
    // (we can't mutate app here, so the caller sets it via on_tick)
    let visible_rows = content_area.height as usize;
    let start = exec.scroll;
    let end = (start + visible_rows).min(exec.log_lines.len());

    let mut lines: Vec<Line> = Vec::new();
    for log_line in &exec.log_lines[start..end] {
        lines.push(style_log_line(log_line));
    }

    let log_para = Paragraph::new(lines);
    f.render_widget(log_para, content_area);

    // Scroll indicator in top-right if there are more lines
    if exec.log_lines.len() > visible_rows && content_area.width > 10 {
        let indicator = format!(
            "[{}/{}]",
            exec.scroll + 1,
            exec.log_lines.len().saturating_sub(visible_rows) + 1
        );
        let indicator_rect = Rect {
            x: content_area
                .right()
                .saturating_sub(indicator.len() as u16 + 1),
            y: content_area.top(),
            width: indicator.len() as u16,
            height: 1,
        };
        let indicator_para = Paragraph::new(indicator).style(Style::default().fg(Color::DarkGray));
        f.render_widget(indicator_para, indicator_rect);
    }
}

fn style_log_line(log_line: &LogLine) -> Line<'static> {
    let style = match log_line.kind {
        LogKind::Commit => Style::default().fg(Color::Green),
        LogKind::Test => Style::default().fg(Color::Cyan),
        LogKind::Pr => Style::default().fg(Color::Magenta),
        LogKind::Error => Style::default().fg(Color::Red),
        LogKind::Summary(true) => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        LogKind::Summary(false) => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        LogKind::Normal => Style::default(),
    };
    Line::from(Span::styled(format!("  {}", log_line.text), style))
}
