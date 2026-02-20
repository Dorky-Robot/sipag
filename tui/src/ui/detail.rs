use crate::app::App;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

pub fn render_detail(f: &mut Frame, app: &App) {
    if app.tasks.is_empty() {
        return;
    }
    let task = &app.tasks[app.selected];
    let area = f.area();

    // ── Outer border + title ──────────────────────────────────────────────────
    let outer_title = format!(" sipag ── #{} ", task.id);
    let outer_block = Block::default()
        .title(outer_title)
        .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
        .border_type(BorderType::Rounded);

    // ── Help bar (bottom two rows) ────────────────────────────────────────────
    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).split(area);

    let content_area = outer_block.inner(chunks[0]);
    f.render_widget(outer_block, chunks[0]);

    let help_block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded);
    let help_inner = help_block.inner(chunks[1]);
    f.render_widget(help_block, chunks[1]);

    let help_text =
        Paragraph::new(Line::from("  r:retry  Esc:back")).style(Style::default());
    f.render_widget(help_text, help_inner);

    // ── Build the top section (title + metadata + description) ────────────────
    let mut top_lines: Vec<Line> = Vec::new();

    // Blank line then task title.
    top_lines.push(Line::from(""));
    top_lines.push(Line::from(Span::styled(
        format!("  {}", task.title),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    top_lines.push(Line::from(""));

    // Metadata fields.
    let label_style = Style::default().add_modifier(Modifier::BOLD);
    top_lines.push(Line::from(vec![
        Span::styled("  Repo:     ", label_style),
        Span::raw(task.repo.as_deref().unwrap_or("-")),
    ]));
    top_lines.push(Line::from(vec![
        Span::styled("  Status:   ", label_style),
        Span::raw(task.status.name()),
    ]));
    top_lines.push(Line::from(vec![
        Span::styled("  Priority: ", label_style),
        Span::raw(task.priority.as_deref().unwrap_or("-")),
    ]));
    if let Some(src) = &task.source {
        top_lines.push(Line::from(vec![
            Span::styled("  Source:   ", label_style),
            Span::raw(src.as_str()),
        ]));
    }
    top_lines.push(Line::from(vec![
        Span::styled("  Added:    ", label_style),
        Span::raw(task.format_age()),
    ]));
    top_lines.push(Line::from(""));

    // Description section.
    if !task.body.is_empty() {
        top_lines.push(section_header("── Description ", content_area.width));
        for body_line in task.body.lines() {
            top_lines.push(Line::from(format!("  {}", body_line)));
        }
        top_lines.push(Line::from(""));
    }

    let top_height = top_lines.len() as u16;

    // ── Split content_area into top (fixed) + log (fills rest) ───────────────
    // If top section overflows the content area, just render everything scrolled.
    // We try to give at least 3 rows to the log when it exists.
    let top_height_clamped = if app.log_lines.is_empty() {
        content_area.height
    } else {
        top_height.min(content_area.height.saturating_sub(3))
    };

    let (top_rect, log_rect) = if app.log_lines.is_empty() {
        (content_area, Rect::default())
    } else {
        let splits = Layout::vertical([
            Constraint::Length(top_height_clamped),
            Constraint::Min(0),
        ])
        .split(content_area);
        (splits[0], splits[1])
    };

    // ── Render the top section ────────────────────────────────────────────────
    let top_para = Paragraph::new(top_lines);
    f.render_widget(top_para, top_rect);

    // ── Render the log section ────────────────────────────────────────────────
    if !app.log_lines.is_empty() && log_rect.height > 0 {
        let mut log_lines: Vec<Line> = Vec::new();

        // Section header.
        log_lines.push(section_header(
            &format!("── Log (last {} lines) ", app.log_lines.len()),
            content_area.width,
        ));

        // Visible log rows: log_rect.height minus the header row (and 1 blank below).
        let visible_rows = log_rect.height.saturating_sub(1) as usize;

        let start = app.log_scroll;
        let end = (start + visible_rows).min(app.log_lines.len());

        for log_line in &app.log_lines[start..end] {
            log_lines.push(Line::from(format!("  {}", log_line)));
        }

        let log_para = Paragraph::new(log_lines);
        f.render_widget(log_para, log_rect);

        // Scroll indicator in top-right of log rect if there are more lines.
        if app.log_lines.len() > visible_rows && log_rect.width > 10 {
            let indicator = format!(
                "[{}/{}]",
                app.log_scroll + 1,
                app.log_lines.len().saturating_sub(visible_rows) + 1
            );
            let indicator_rect = Rect {
                x: log_rect.right().saturating_sub(indicator.len() as u16 + 1),
                y: log_rect.top(),
                width: indicator.len() as u16,
                height: 1,
            };
            let indicator_para = Paragraph::new(indicator)
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(indicator_para, indicator_rect);
        }
    }
}

/// Build a styled section-header line that spans the full inner width.
fn section_header(label: &str, inner_width: u16) -> Line<'static> {
    // Pad with dashes to the inner width (accounting for 2-char left indent).
    let min_dashes = 2usize;
    let label_len = label.chars().count() + 2; // "  " prefix
    let total = inner_width as usize;
    let dash_count = if total > label_len + min_dashes {
        total - label_len
    } else {
        min_dashes
    };
    let dashes = "─".repeat(dash_count);
    let text = format!("  {}{}", label, dashes);
    Line::from(Span::styled(
        text,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ))
}
