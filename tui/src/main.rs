use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
    Terminal,
};

#[derive(Debug, Clone, Copy)]
enum Status {
    Pending,
    Running,
    Done,
    Failed,
}

impl Status {
    fn icon(self) -> &'static str {
        match self {
            Status::Pending => "·",
            Status::Running => "⧖",
            Status::Done => "✓",
            Status::Failed => "✗",
        }
    }
}

#[derive(Debug)]
struct Task {
    id: usize,
    status: Status,
    priority: String,
    repo: String,
    title: String,
    added: Option<DateTime<Utc>>,
}

fn parse_priority(p: &str) -> &'static str {
    let lower = p.to_lowercase();
    match lower.as_str() {
        "high" | "h" => "H",
        "low" | "l" => "L",
        _ => "M",
    }
}

fn parse_task_file(path: &Path, status: Status, id: usize) -> Option<Task> {
    let content = fs::read_to_string(path).ok()?;
    let mut lines = content.lines();
    let mut repo = String::new();
    let mut priority = "M".to_string();
    let mut added: Option<DateTime<Utc>> = None;
    let mut title = String::new();

    // Expect YAML frontmatter opening delimiter
    if lines.next()?.trim() != "---" {
        return None;
    }

    // Parse frontmatter key-value pairs until closing "---"
    for line in lines.by_ref() {
        if line.trim() == "---" {
            break;
        }
        if let Some(val) = line.strip_prefix("repo:") {
            repo = val.trim().to_string();
        } else if let Some(val) = line.strip_prefix("priority:") {
            priority = parse_priority(val.trim()).to_string();
        } else if let Some(val) = line.strip_prefix("added:") {
            added = DateTime::parse_from_rfc3339(val.trim())
                .ok()
                .map(|dt| dt.with_timezone(&Utc));
        }
    }

    // First non-empty line after frontmatter is the task title
    for line in lines {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            title = trimmed.to_string();
            break;
        }
    }

    if title.is_empty() {
        title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
    }

    Some(Task {
        id,
        status,
        priority,
        repo,
        title,
        added,
    })
}

fn format_age(added: Option<DateTime<Utc>>) -> String {
    let Some(added) = added else {
        return "-".to_string();
    };
    let secs = Utc::now()
        .signed_duration_since(added)
        .num_seconds()
        .max(0);
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

fn scan_tasks(sipag_dir: &Path) -> Vec<Task> {
    let dirs = [
        ("queue", Status::Pending),
        ("running", Status::Running),
        ("done", Status::Done),
        ("failed", Status::Failed),
    ];

    let mut tasks = Vec::new();
    let mut id = 1usize;

    for (dir_name, status) in dirs {
        let dir = sipag_dir.join(dir_name);
        if !dir.exists() {
            continue;
        }

        let Ok(read_dir) = fs::read_dir(&dir) else {
            continue;
        };

        let mut entries: Vec<_> = read_dir
            .flatten()
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext == "md")
                    .unwrap_or(false)
            })
            .collect();

        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            if let Some(task) = parse_task_file(&entry.path(), status, id) {
                tasks.push(task);
                id += 1;
            }
        }
    }

    tasks
}

fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        let mut t: String = chars[..max.saturating_sub(1)].iter().collect();
        t.push('…');
        t
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let sipag_dir = env::var("SIPAG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            env::var("HOME")
                .map(|h| PathBuf::from(h).join(".sipag"))
                .unwrap_or_else(|_| PathBuf::from("/tmp/.sipag"))
        });

    let tasks = scan_tasks(&sipag_dir);

    let pending = tasks
        .iter()
        .filter(|t| matches!(t.status, Status::Pending))
        .count();
    let running = tasks
        .iter()
        .filter(|t| matches!(t.status, Status::Running))
        .count();
    let done = tasks
        .iter()
        .filter(|t| matches!(t.status, Status::Done))
        .count();
    let failed = tasks
        .iter()
        .filter(|t| matches!(t.status, Status::Failed))
        .count();
    let total = tasks.len();

    loop {
        terminal.draw(|f| {
            let area = f.area();

            // Outer frame with title
            let outer = Block::default()
                .title(" sipag ")
                .borders(Borders::ALL);
            let inner = outer.inner(area);
            f.render_widget(outer, area);

            // Split inner: content area (grows) + footer (separator + keybinds)
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(0), Constraint::Length(2)])
                .split(inner);

            // Footer: top border acts as visual separator from content
            let footer_block = Block::default().borders(Borders::TOP);
            let footer_text_area = footer_block.inner(chunks[1]);
            f.render_widget(footer_block, chunks[1]);
            f.render_widget(Paragraph::new("  q:quit"), footer_text_area);

            // Content layout: padding + table + spacer + legend + spacer + summary + padding
            let content = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1), // top padding
                    Constraint::Min(3),    // table
                    Constraint::Length(1), // spacer
                    Constraint::Length(1), // legend
                    Constraint::Length(1), // spacer
                    Constraint::Length(1), // summary
                    Constraint::Length(1), // bottom padding
                ])
                .split(chunks[0]);

            // Left/right margin for table
            let table_area = content[1].inner(Margin {
                horizontal: 2,
                vertical: 0,
            });

            // Table header
            let header = Row::new([
                Cell::from("ID").style(Style::default().add_modifier(Modifier::BOLD)),
                Cell::from("St").style(Style::default().add_modifier(Modifier::BOLD)),
                Cell::from("Pri").style(Style::default().add_modifier(Modifier::BOLD)),
                Cell::from("Repo").style(Style::default().add_modifier(Modifier::BOLD)),
                Cell::from("Title").style(Style::default().add_modifier(Modifier::BOLD)),
                Cell::from("Age").style(Style::default().add_modifier(Modifier::BOLD)),
            ]);

            // Separator row under header
            let sep = Row::new([
                Cell::from("──"),
                Cell::from("──"),
                Cell::from("───"),
                Cell::from("───────"),
                Cell::from("─────────────────────────────"),
                Cell::from("──────"),
            ]);

            // Data rows
            let data_rows: Vec<Row> = tasks
                .iter()
                .map(|t| {
                    let status_style = match t.status {
                        Status::Pending => Style::default(),
                        Status::Running => Style::default().fg(Color::Yellow),
                        Status::Done => Style::default().fg(Color::Green),
                        Status::Failed => Style::default().fg(Color::Red),
                    };
                    Row::new([
                        Cell::from(t.id.to_string()),
                        Cell::from(t.status.icon()).style(status_style),
                        Cell::from(t.priority.as_str()),
                        Cell::from(truncate(&t.repo, 10)),
                        Cell::from(truncate(&t.title, 35)),
                        Cell::from(format_age(t.added)),
                    ])
                })
                .collect();

            let mut all_rows = vec![sep];
            all_rows.extend(data_rows);

            let widths = [
                Constraint::Length(4),  // ID
                Constraint::Length(3),  // St
                Constraint::Length(4),  // Pri
                Constraint::Length(10), // Repo
                Constraint::Min(10),    // Title (flexible)
                Constraint::Length(6),  // Age
            ];

            let table = Table::new(all_rows, widths)
                .header(header)
                .column_spacing(1);
            f.render_widget(table, table_area);

            // Legend
            let legend_area = content[3].inner(Margin {
                horizontal: 2,
                vertical: 0,
            });
            f.render_widget(
                Paragraph::new("· pending  ⧖ running  ✓ done  ✗ failed"),
                legend_area,
            );

            // Summary line
            let summary_area = content[5].inner(Margin {
                horizontal: 2,
                vertical: 0,
            });
            f.render_widget(
                Paragraph::new(format!(
                    "{} tasks ({} pending, {} running, {} done, {} failed)",
                    total, pending, running, done, failed
                )),
                summary_area,
            );
        })?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('q') {
                    break;
                }
            }
        }
    }

    Ok(())
}
