use crate::app::App;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use sipag_core::state::{format_duration, WorkerPhase};

pub fn render_detail(f: &mut Frame, app: &App) {
    if app.tasks.is_empty() {
        return;
    }
    let task = &app.tasks[app.selected];
    let area = f.area();

    let chunks = Layout::vertical([
        Constraint::Length(1), // header bar
        Constraint::Min(0),    // body
        Constraint::Length(1), // footer bar
    ])
    .split(area);

    // ── Header bar ────────────────────────────────────────────────────────────
    let header_text = format!(" sipag  PR #{} — {}", task.pr_num, task.repo);
    let header = Paragraph::new(Line::from(header_text)).style(
        Style::default()
            .fg(Color::White)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(header, chunks[0]);

    // ── Footer bar ────────────────────────────────────────────────────────────
    let footer_text = if !task.phase.is_terminal() && !task.container_id.is_empty() {
        " [Esc] back  [j/k] scroll  [a] attach  [q] quit"
    } else if task.phase.is_terminal() {
        " [Esc] back  [j/k] scroll  [x] dismiss  [q] quit"
    } else {
        " [Esc] back  [j/k] scroll  [q] quit"
    };
    let footer = Paragraph::new(Line::from(footer_text))
        .style(Style::default().fg(Color::White).bg(Color::DarkGray));
    f.render_widget(footer, chunks[2]);

    // ── Outer content block ───────────────────────────────────────────────────
    let outer_block = Block::default()
        .title(" Detail ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let content_area = outer_block.inner(chunks[1]);
    f.render_widget(outer_block, chunks[1]);

    // ── Build the top section (metadata) ──────────────────────────────────────
    let mut top_lines: Vec<Line> = Vec::new();
    let label_style = Style::default().add_modifier(Modifier::BOLD);

    top_lines.push(Line::from(""));
    top_lines.push(Line::from(Span::styled(
        format!("  PR #{}", task.pr_num),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    top_lines.push(Line::from(""));

    // Metadata fields.
    top_lines.push(Line::from(vec![
        Span::styled("  Repo:     ", label_style),
        Span::raw(task.repo.clone()),
    ]));
    top_lines.push(Line::from(vec![
        Span::styled("  Branch:   ", label_style),
        Span::raw(task.branch.clone()),
    ]));

    let phase_style = match task.phase {
        WorkerPhase::Starting => Style::default().fg(Color::Yellow),
        WorkerPhase::Working => Style::default().fg(Color::Cyan),
        WorkerPhase::Finished => Style::default().fg(Color::Green),
        WorkerPhase::Failed => Style::default().fg(Color::Red),
    };
    top_lines.push(Line::from(vec![
        Span::styled("  Phase:    ", label_style),
        Span::styled(task.phase.to_string(), phase_style),
    ]));

    top_lines.push(Line::from(vec![
        Span::styled("  Started:  ", label_style),
        Span::raw(format!("{} ago", task.format_age())),
    ]));

    // Issues addressed.
    if !task.issues.is_empty() {
        let issues_str = task
            .issues
            .iter()
            .map(|n| format!("#{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        top_lines.push(Line::from(vec![
            Span::styled("  Issues:   ", label_style),
            Span::raw(issues_str),
        ]));
    }

    // ── Terminal-phase fields (finished or failed) ────────────────────────────
    if task.phase.is_terminal() {
        if let Some(duration) = task.duration_secs() {
            top_lines.push(Line::from(vec![
                Span::styled("  Duration: ", label_style),
                Span::raw(format_duration(duration)),
            ]));
        }

        if task.ended.is_some() {
            top_lines.push(Line::from(vec![
                Span::styled("  Ended:    ", label_style),
                Span::raw(format!("{} ago", task.format_ended_age())),
            ]));
        }

        if let Some(code) = task.exit_code {
            let code_style = if code == 0 {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Red)
            };
            top_lines.push(Line::from(vec![
                Span::styled("  Exit:     ", label_style),
                Span::styled(code.to_string(), code_style),
            ]));
        }

        if let Some(ref error) = task.error {
            top_lines.push(Line::from(vec![
                Span::styled("  Error:    ", label_style),
                Span::styled(error.clone(), Style::default().fg(Color::Red)),
            ]));
        }
    }

    // Container ID.
    if !task.container_id.is_empty() {
        let short = if task.container_id.len() > 12 {
            &task.container_id[..12]
        } else {
            &task.container_id
        };
        top_lines.push(Line::from(vec![
            Span::styled("  Container:", label_style),
            Span::raw(format!(" {short}")),
        ]));
    }

    top_lines.push(Line::from(""));

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

        log_lines.push(section_header(
            &format!("── Log ({} lines) ", app.log_lines.len()),
            content_area.width,
        ));

        let visible_rows = log_rect.height.saturating_sub(1) as usize;
        // Clamp here (not in the model) because visible_rows is a renderer concept.
        let start = app
            .log_scroll
            .min(app.log_lines.len().saturating_sub(visible_rows));
        let end = (start + visible_rows).min(app.log_lines.len());

        for log_line in &app.log_lines[start..end] {
            log_lines.push(Line::from(format!("  {}", log_line)));
        }

        let log_para = Paragraph::new(log_lines);
        f.render_widget(log_para, log_rect);

        // Scroll indicator — use the clamped `start` so the position matches what is rendered.
        if app.log_lines.len() > visible_rows && log_rect.width > 10 {
            let indicator = format!(
                "[{}/{}]",
                start + 1,
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
    let min_dashes = 2usize;
    let label_len = label.chars().count() + 2;
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
