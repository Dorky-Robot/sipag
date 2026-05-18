//! Interactive board renderer — v4 multi-project task board view.

use crate::board_app::{BoardApp, InputMode};
use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Render the interactive board.
pub fn render_board(f: &mut Frame, app: &BoardApp) {
    let area = f.area();

    // If no projects exist, show empty state.
    if app.project_names.is_empty() {
        render_empty_state(f, area);
        return;
    }

    let chunks = Layout::vertical([
        Constraint::Length(1), // project tabs
        Constraint::Min(5),    // board columns
        Constraint::Length(1), // role status bar
        Constraint::Length(1), // keybindings footer
    ])
    .split(area);

    render_project_tabs(f, app, chunks[0]);
    render_columns(f, app, chunks[1]);
    render_role_bar(f, app, chunks[2]);
    render_footer(f, app, chunks[3]);

    // Input overlay.
    if app.input_mode != InputMode::Normal {
        render_input_overlay(f, app, area);
    }
}

fn render_empty_state(f: &mut Frame, area: Rect) {
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(area);

    // Header.
    let header = Paragraph::new(Line::from(" sipag")).style(
        Style::default()
            .fg(Color::White)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(header, chunks[0]);

    // Centered message.
    let msg = Paragraph::new(vec![
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(
            "No projects configured.",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Create one with:  sipag project add <name> --repo <owner/repo>",
            Style::default().fg(Color::DarkGray),
        )),
    ])
    .alignment(Alignment::Center);
    f.render_widget(msg, chunks[1]);

    // Footer.
    let footer = Paragraph::new(Line::from(" q:quit"))
        .style(Style::default().fg(Color::White).bg(Color::DarkGray));
    f.render_widget(footer, chunks[2]);
}

fn render_project_tabs(f: &mut Frame, app: &BoardApp, area: Rect) {
    let mut spans: Vec<Span> = vec![Span::styled(
        " Projects: ",
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )];

    for (i, name) in app.project_names.iter().enumerate() {
        let is_active = i == app.active_project_idx;
        let marker = if is_active { " \u{25cf}" } else { " \u{25cb}" };
        let style = if is_active {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(format!("{name}{marker}"), style));
        if i + 1 < app.project_names.len() {
            spans.push(Span::raw("  "));
        }
    }

    let line = Line::from(spans);
    let tabs = Paragraph::new(line).style(Style::default().bg(Color::DarkGray));
    f.render_widget(tabs, area);
}

fn render_columns(f: &mut Frame, app: &BoardApp, area: Rect) {
    if app.statuses.is_empty() {
        return;
    }

    let n_cols = app.statuses.len();
    let constraints: Vec<Constraint> = (0..n_cols)
        .map(|_| Constraint::Ratio(1, n_cols as u32))
        .collect();
    let col_areas = Layout::horizontal(constraints).split(area);

    for (ci, status) in app.statuses.iter().enumerate() {
        let is_focused = ci == app.col_idx;
        let col_tasks = app.columns.get(ci).map(|c| c.as_slice()).unwrap_or(&[]);

        let border_style = if is_focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let header_color = status_color(status);
        let title = format!(" {status} ({}) ", col_tasks.len());

        let block = Block::default()
            .title(Span::styled(
                title,
                Style::default()
                    .fg(header_color)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = block.inner(col_areas[ci]);
        f.render_widget(block, col_areas[ci]);

        // Render tasks in this column.
        if col_tasks.is_empty() {
            if inner.height > 0 {
                let empty =
                    Paragraph::new(Span::styled("  -", Style::default().fg(Color::DarkGray)));
                f.render_widget(empty, inner);
            }
            continue;
        }

        let mut lines: Vec<Line> = Vec::new();
        for (ri, task) in col_tasks.iter().enumerate() {
            let is_selected = is_focused && ri == app.row_idx;
            let prefix = if is_selected { "> " } else { "  " };
            let id_str = format!("#{}", task.id);
            let title_max = inner.width.saturating_sub(6) as usize;
            let title_display = if task.title.len() > title_max {
                format!("{}...", &task.title[..title_max.saturating_sub(3)])
            } else {
                task.title.clone()
            };

            let style = if is_selected {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let bg = if is_selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };

            lines.push(Line::from(vec![
                Span::styled(prefix.to_string(), bg),
                Span::styled(id_str, bg.patch(Style::default().fg(header_color))),
                Span::styled(format!(" {title_display}"), bg.patch(style)),
            ]));
        }

        let para = Paragraph::new(lines);
        f.render_widget(para, inner);
    }
}

fn render_role_bar(f: &mut Frame, app: &BoardApp, area: Rect) {
    let mut spans: Vec<Span> = vec![Span::styled(
        " Roles: ",
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )];

    if app.role_names.is_empty() {
        spans.push(Span::styled(
            "(none configured)",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        for (i, name) in app.role_names.iter().enumerate() {
            // Placeholder — no live agent data yet, show as offline.
            let style = Style::default().fg(Color::DarkGray);
            spans.push(Span::styled(format!("{name} \u{25cb}"), style));
            if i + 1 < app.role_names.len() {
                spans.push(Span::raw("  "));
            }
        }
    }

    let line = Line::from(spans);
    let bar = Paragraph::new(line).style(Style::default().bg(Color::DarkGray));
    f.render_widget(bar, area);
}

fn render_footer(f: &mut Frame, app: &BoardApp, area: Rect) {
    // Show status message if present, otherwise show keybindings.
    let text = if let Some(ref msg) = app.status_message {
        format!(" {msg}")
    } else if app.input_mode != InputMode::Normal {
        " Esc:cancel  Enter:confirm".to_string()
    } else {
        " j/k:nav  h/l:column  Tab:project  a:add  d:dispatch  m:move  Enter:advance  q:quit"
            .to_string()
    };

    let style = if app.status_message.is_some() {
        Style::default().fg(Color::Yellow).bg(Color::DarkGray)
    } else {
        Style::default().fg(Color::White).bg(Color::DarkGray)
    };

    let footer = Paragraph::new(Line::from(text)).style(style);
    f.render_widget(footer, area);
}

fn render_input_overlay(f: &mut Frame, app: &BoardApp, area: Rect) {
    let label = match app.input_mode {
        InputMode::AddTask => "New task title: ",
        InputMode::MoveTask => "Move to status: ",
        InputMode::Normal => return,
    };

    // Centered popup.
    let popup_width = 50u16.min(area.width.saturating_sub(4));
    let popup_height = 3u16;
    let x = (area.width.saturating_sub(popup_width)) / 2;
    let y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(x, y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);

    let input_text = format!("{}{}", label, app.input_buffer);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(Span::styled(
            " Input ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
    let para = Paragraph::new(input_text).block(block);
    f.render_widget(para, popup_area);
}

/// Map status name to a display color.
fn status_color(status: &str) -> Color {
    match status {
        "backlog" | "todo" => Color::Yellow,
        "in-progress" => Color::Cyan,
        "review" | "done" => Color::Green,
        _ => Color::White,
    }
}
