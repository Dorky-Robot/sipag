use crate::app::App;
use crate::task::Status;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use sipag_core::worker::state::format_duration;

pub fn render_detail(f: &mut Frame, app: &App) {
    if app.tasks.is_empty() {
        return;
    }
    let task = &app.tasks[app.selected];
    let area = f.area();

    // 3-part layout: header bar | body | footer bar
    let chunks = Layout::vertical([
        Constraint::Length(1), // header bar
        Constraint::Min(0),    // body
        Constraint::Length(1), // footer bar
    ])
    .split(area);

    // ── Header bar ────────────────────────────────────────────────────────────
    let issue_str = task.issue.map(|n| format!(" #{}", n)).unwrap_or_default();
    let header_text = format!(" sipag Detail{} — {}", issue_str, task.title);
    let header = Paragraph::new(Line::from(header_text)).style(
        Style::default()
            .fg(Color::White)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(header, chunks[0]);

    // ── Footer bar ────────────────────────────────────────────────────────────
    let is_terminal = task.status == Status::Done || task.status == Status::Failed;
    // Only task-file backed failed tasks support in-TUI retry.
    let can_retry = task.status == Status::Failed && !task.file_path.as_os_str().is_empty();

    let footer_text = if task.status == Status::Running {
        " [Esc] back  [j/k] scroll  [a] attach  [q] quit"
    } else if can_retry {
        " [Esc] back  [j/k] scroll  [r] retry  [x] dismiss  [q] quit"
    } else if is_terminal {
        " [Esc] back  [j/k] scroll  [x] dismiss  [q] quit"
    } else {
        " [Esc] back  [j/k] scroll  [q] quit"
    };
    let footer = Paragraph::new(Line::from(footer_text))
        .style(Style::default().fg(Color::White).bg(Color::DarkGray));
    f.render_widget(footer, chunks[2]);

    // ── Outer content block (Cyan borders) ───────────────────────────────────
    let outer_block = Block::default()
        .title(" Detail ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let content_area = outer_block.inner(chunks[1]);
    f.render_widget(outer_block, chunks[1]);

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

    let status_style = match task.status {
        Status::Queue => Style::default().fg(Color::Yellow),
        Status::Running => Style::default().fg(Color::Cyan),
        Status::Done => Style::default().fg(Color::Green),
        Status::Failed => Style::default().fg(Color::Red),
    };
    top_lines.push(Line::from(vec![
        Span::styled("  Status:   ", label_style),
        Span::styled(task.status.name(), status_style),
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

    // ── Extra metadata for done/failed archive tasks ──────────────────────────
    if task.status == Status::Done {
        // Duration
        top_lines.push(Line::from(vec![
            Span::styled("  Duration: ", label_style),
            Span::raw(format_duration(task.duration_s)),
        ]));

        // Completion time (human age)
        if task.ended_at.is_some() {
            top_lines.push(Line::from(vec![
                Span::styled("  Ended:    ", label_style),
                Span::raw(format!("{} ago", task.format_ended_age())),
            ]));
        }

        // PR info
        if let Some(pr_num) = task.pr_num {
            let pr_display = task
                .pr_url
                .as_deref()
                .map(|url| format!("PR #{} — {}", pr_num, url))
                .unwrap_or_else(|| format!("PR #{}", pr_num));
            top_lines.push(Line::from(vec![
                Span::styled("  PR:       ", label_style),
                Span::styled(
                    pr_display,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::UNDERLINED),
                ),
            ]));
        }
    } else if task.status == Status::Failed {
        // Duration
        top_lines.push(Line::from(vec![
            Span::styled("  Duration: ", label_style),
            Span::raw(format_duration(task.duration_s)),
        ]));

        // Completion time (human age)
        if task.ended_at.is_some() {
            top_lines.push(Line::from(vec![
                Span::styled("  Ended:    ", label_style),
                Span::raw(format!("{} ago", task.format_ended_age())),
            ]));
        }

        // Exit code
        let exit_str = task
            .exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "-".to_string());
        top_lines.push(Line::from(vec![
            Span::styled("  Exit:     ", label_style),
            Span::styled(exit_str, Style::default().fg(Color::Red)),
        ]));
    }

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
    let top_height_clamped = if app.log_lines.is_empty() {
        content_area.height
    } else {
        top_height.min(content_area.height.saturating_sub(3))
    };

    let (top_rect, log_rect) = if app.log_lines.is_empty() {
        (content_area, Rect::default())
    } else {
        let splits = Layout::vertical([Constraint::Length(top_height_clamped), Constraint::Min(0)])
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
            let indicator_para =
                Paragraph::new(indicator).style(Style::default().fg(Color::DarkGray));
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
