use crate::app::App;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

pub fn render_detail(f: &mut Frame, app: &mut App) {
    if app.workers.is_empty() {
        return;
    }

    let Some(worker) = app.selected_worker() else {
        return;
    };
    let worker = worker.clone();

    let area = f.area();

    // ── Outer layout: content + help bar ─────────────────────────────────────
    let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).split(area);

    let outer_title = format!(
        " sipag ── {} #{} ",
        worker.repo, worker.issue_num
    );
    let outer_block = Block::default()
        .title(outer_title)
        .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
        .border_type(BorderType::Rounded);
    let content_area = outer_block.inner(chunks[0]);
    f.render_widget(outer_block, chunks[0]);

    let help_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);
    let help_inner = help_block.inner(chunks[1]);
    f.render_widget(help_block, chunks[1]);

    let help_text = Paragraph::new(Line::from(
        "  r:reload   g:top   G:bottom   j/k:scroll   q/Esc:back",
    ));
    f.render_widget(help_text, help_inner);

    // ── Metadata section ─────────────────────────────────────────────────────
    let mut meta_lines: Vec<Line> = Vec::new();
    meta_lines.push(Line::from(""));
    meta_lines.push(Line::from(Span::styled(
        format!("  {}", worker.issue_title),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    meta_lines.push(Line::from(""));

    let label = Style::default().add_modifier(Modifier::BOLD);
    let status_color = match worker.status.as_str() {
        "running" => Color::Cyan,
        "done" => Color::Green,
        "failed" => Color::Red,
        _ => Color::White,
    };

    meta_lines.push(Line::from(vec![
        Span::styled("  Repo:     ", label),
        Span::raw(worker.repo.clone()),
    ]));
    meta_lines.push(Line::from(vec![
        Span::styled("  Issue:    ", label),
        Span::raw(format!("#{}", worker.issue_num)),
    ]));
    meta_lines.push(Line::from(vec![
        Span::styled("  Status:   ", label),
        Span::styled(worker.status.clone(), Style::default().fg(status_color)),
    ]));
    meta_lines.push(Line::from(vec![
        Span::styled("  Duration: ", label),
        Span::raw(worker.format_duration()),
    ]));
    meta_lines.push(Line::from(vec![
        Span::styled("  Branch:   ", label),
        Span::raw(worker.branch.clone()),
    ]));
    if let Some(pr_num) = worker.pr_num {
        meta_lines.push(Line::from(vec![
            Span::styled("  PR:       ", label),
            Span::raw(format!(
                "#{pr_num}{}",
                worker
                    .pr_url
                    .as_deref()
                    .map(|u| format!("  {u}"))
                    .unwrap_or_default()
            )),
        ]));
    }
    meta_lines.push(Line::from(vec![
        Span::styled("  Started:  ", label),
        Span::raw(worker.started_at.clone()),
    ]));
    if let Some(ref ended) = worker.ended_at {
        meta_lines.push(Line::from(vec![
            Span::styled("  Ended:    ", label),
            Span::raw(ended.clone()),
        ]));
    }
    meta_lines.push(Line::from(""));

    let meta_height = meta_lines.len() as u16;

    // ── Split content into meta (top) + log (bottom) ──────────────────────────
    let meta_height_clamped = if app.log_lines.is_empty() {
        content_area.height
    } else {
        meta_height.min(content_area.height.saturating_sub(3))
    };

    let (meta_rect, log_rect) = if app.log_lines.is_empty() {
        (content_area, Rect::default())
    } else {
        let splits = Layout::vertical([
            Constraint::Length(meta_height_clamped),
            Constraint::Min(0),
        ])
        .split(content_area);
        (splits[0], splits[1])
    };

    let meta_para = Paragraph::new(meta_lines);
    f.render_widget(meta_para, meta_rect);

    // ── Log section ───────────────────────────────────────────────────────────
    if !app.log_lines.is_empty() && log_rect.height > 0 {
        // Update viewport height so key handler can clamp scroll correctly.
        app.log_viewport_height = log_rect.height.saturating_sub(1);

        let mut log_display: Vec<Line> = Vec::new();
        log_display.push(section_header(
            &format!("── Log ({} lines) ", app.log_lines.len()),
            content_area.width,
        ));

        let visible = log_rect.height.saturating_sub(1) as usize;
        let start = app.log_scroll;
        let end = (start + visible).min(app.log_lines.len());

        for line in &app.log_lines[start..end] {
            log_display.push(Line::from(format!("  {line}")));
        }

        let log_para = Paragraph::new(log_display);
        f.render_widget(log_para, log_rect);

        // Scroll position indicator
        if app.log_lines.len() > visible && log_rect.width > 12 {
            let max_scroll = app.log_lines.len().saturating_sub(visible);
            let indicator = format!("[{}/{}]", app.log_scroll + 1, max_scroll + 1);
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
    } else if app.log_lines.is_empty() && log_rect.height > 0 {
        let placeholder = Paragraph::new(Line::from(Span::styled(
            "  (log not loaded — navigate to list and press Enter, or log file not found)",
            Style::default().fg(Color::DarkGray),
        )));
        f.render_widget(placeholder, log_rect);
    }
}

fn section_header(label: &str, inner_width: u16) -> Line<'static> {
    let min_dashes = 2usize;
    let label_len = label.chars().count() + 2;
    let total = inner_width as usize;
    let dash_count = if total > label_len + min_dashes {
        total - label_len
    } else {
        min_dashes
    };
    let dashes = "─".repeat(dash_count);
    let text = format!("  {label}{dashes}");
    Line::from(Span::styled(
        text,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ))
}
