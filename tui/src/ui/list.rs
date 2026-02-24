use crate::app::{App, ListMode};
use ratatui::{
    layout::{Alignment, Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
    Frame,
};
use sipag_core::state::WorkerPhase;

pub fn render_list(f: &mut Frame, app: &App) {
    let area = f.area();

    let chunks = Layout::vertical([
        Constraint::Length(1), // header bar
        Constraint::Min(5),    // body (table)
        Constraint::Length(1), // footer bar
    ])
    .split(area);

    let is_archive = app.list_mode == ListMode::Archive;

    let active_count = app.tasks.iter().filter(|t| !t.phase.is_terminal()).count();
    let finished_count = app
        .tasks
        .iter()
        .filter(|t| t.phase == WorkerPhase::Finished)
        .count();
    let failed_count = app
        .tasks
        .iter()
        .filter(|t| t.phase == WorkerPhase::Failed)
        .count();

    // ── Header bar ────────────────────────────────────────────────────────────
    let mode_label = if is_archive { "[Archive]" } else { "[Active]" };
    let header_base = if is_archive {
        format!(" sipag {mode_label}  finished: {finished_count}  failed: {failed_count}")
    } else {
        format!(
            " sipag {mode_label}  workers: {active_count} ({} state files in {})",
            app.total_state_files,
            app.sipag_dir.display()
        )
    };
    let header_style = Style::default()
        .fg(Color::White)
        .bg(Color::DarkGray)
        .add_modifier(Modifier::BOLD);

    let header = Paragraph::new(Line::from(Span::styled(header_base, header_style)))
        .style(Style::default().bg(Color::DarkGray));
    f.render_widget(header, chunks[0]);

    // ── Table column headers ──────────────────────────────────────────────────
    let since_label = if is_archive { "ENDED" } else { "AGE" };
    let col_header = Row::new(vec![
        Cell::from("PR"),
        Cell::from("REPO"),
        Cell::from("PHASE"),
        Cell::from(since_label),
    ])
    .style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
    .height(1);

    // ── Task rows ─────────────────────────────────────────────────────────────
    let rows: Vec<Row> = app
        .tasks
        .iter()
        .map(|task| {
            let phase_style = match task.phase {
                WorkerPhase::Starting => Style::default().fg(Color::Yellow),
                WorkerPhase::Working => Style::default().fg(Color::Cyan),
                WorkerPhase::Finished => Style::default().fg(Color::Green),
                WorkerPhase::Failed => Style::default().fg(Color::Red),
            };

            let pr_str = if task.issues.is_empty() {
                format!("#{}", task.pr_num)
            } else {
                format!("#{} ({}i)", task.pr_num, task.issues.len())
            };

            let age_str = if is_archive {
                task.format_ended_age()
            } else {
                task.format_age()
            };

            Row::new(vec![
                Cell::from(pr_str),
                Cell::from(task.repo.clone()),
                Cell::from(task.phase.to_string()).style(phase_style),
                Cell::from(age_str),
            ])
            .height(1)
        })
        .collect();

    let widths = [
        Constraint::Length(14), // PR (+Ni)
        Constraint::Min(20),    // REPO (flexible)
        Constraint::Length(10), // PHASE
        Constraint::Length(10), // AGE / ENDED
    ];

    if app.tasks.is_empty() {
        let empty_msg = if is_archive {
            "\nNo archived workers.\n\nCompleted workers will appear here."
        } else {
            "\nNo workers running.\n\nStart with:  sipag dispatch --repo <owner/repo> --pr <N>"
        };
        let empty = Paragraph::new(empty_msg)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::TOP));
        f.render_widget(empty, chunks[1]);
    } else {
        let mut table_state = TableState::default().with_selected(Some(app.selected));
        let table = Table::new(rows, widths)
            .header(col_header)
            .block(Block::default().borders(Borders::TOP))
            .row_highlight_style(Style::default().bg(Color::DarkGray))
            .highlight_symbol("  > ");
        f.render_stateful_widget(table, chunks[1], &mut table_state);
    }

    // ── Footer bar ────────────────────────────────────────────────────────────
    let footer_text = if is_archive {
        " [Tab/a] active  [j/k] nav  [Enter] details  [x] dismiss  [q] quit"
    } else {
        let has_attachable = app
            .tasks
            .get(app.selected)
            .is_some_and(|t| !t.phase.is_terminal() && !t.container_id.is_empty());
        if has_attachable {
            " [Tab] archive  [j/↑↓] nav  [⏎] details  [a] attach  [d] done  [k] kill  [K] all  [q] quit"
        } else {
            " [Tab/a] archive  [j/↑↓] nav  [⏎] details  [d] done  [k] kill  [K] all  [q] quit"
        }
    };

    let footer = Paragraph::new(Line::from(footer_text))
        .style(Style::default().fg(Color::White).bg(Color::DarkGray));
    f.render_widget(footer, chunks[2]);
}
