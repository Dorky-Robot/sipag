use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
    Terminal,
};

// ── Data model ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum Status {
    Pending,
    Running,
    Done,
    Failed,
}

impl Status {
    fn icon(&self) -> &'static str {
        match self {
            Status::Pending => "·",
            Status::Running => "⧖",
            Status::Done => "✓",
            Status::Failed => "✗",
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Running => "running",
            Status::Done => "done",
            Status::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug)]
enum Priority {
    High,
    Medium,
    Low,
    Unknown,
}

impl Priority {
    fn display(&self) -> &'static str {
        match self {
            Priority::High => "H",
            Priority::Medium => "M",
            Priority::Low => "L",
            Priority::Unknown => "-",
        }
    }
}

#[derive(Clone, Debug)]
struct Task {
    id: usize,
    status: Status,
    priority: Priority,
    repo: String,
    title: String,
    added: Option<DateTime<Utc>>,
}

// ── Age formatting ─────────────────────────────────────────────────────────────

fn format_age(added: Option<&DateTime<Utc>>) -> String {
    let Some(added) = added else {
        return "-".to_string();
    };
    let now = Utc::now();
    let diff = now.signed_duration_since(*added);
    let secs = diff.num_seconds().max(0);
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

// ── Task file parsing ──────────────────────────────────────────────────────────

fn parse_task_file(path: &Path, status: Status, id: usize) -> Option<Task> {
    let content = fs::read_to_string(path).ok()?;
    let mut repo = String::new();
    let mut priority = Priority::Medium;
    let mut added: Option<DateTime<Utc>> = None;
    let mut title = String::new();

    let mut in_frontmatter = false;
    let mut frontmatter_done = false;

    for (i, line) in content.lines().enumerate() {
        if i == 0 && line.trim() == "---" {
            in_frontmatter = true;
            continue;
        }
        if in_frontmatter {
            if line.trim() == "---" {
                in_frontmatter = false;
                frontmatter_done = true;
                continue;
            }
            if let Some(val) = line.strip_prefix("repo: ") {
                repo = val.trim().to_string();
            } else if let Some(val) = line.strip_prefix("priority: ") {
                priority = match val.trim().to_lowercase().as_str() {
                    "high" | "h" => Priority::High,
                    "medium" | "m" => Priority::Medium,
                    "low" | "l" => Priority::Low,
                    _ => Priority::Unknown,
                };
            } else if let Some(val) = line.strip_prefix("added: ") {
                added = DateTime::parse_from_rfc3339(val.trim())
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc));
            }
        } else if frontmatter_done && !line.trim().is_empty() {
            title = line.trim().to_string();
            break;
        }
    }

    // No frontmatter — first non-empty line is the title
    if !frontmatter_done {
        for line in content.lines() {
            if !line.trim().is_empty() {
                title = line.trim().to_string();
                break;
            }
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

fn load_tasks(sipag_dir: &Path) -> Vec<Task> {
    let mut tasks = Vec::new();
    let mut id = 1;

    let dirs = [
        (sipag_dir.join("queue"), Status::Pending),
        (sipag_dir.join("running"), Status::Running),
        (sipag_dir.join("done"), Status::Done),
        (sipag_dir.join("failed"), Status::Failed),
    ];

    for (dir, status) in &dirs {
        if let Ok(entries) = fs::read_dir(dir) {
            let mut files: Vec<_> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
                .collect();
            files.sort_by_key(|e| e.file_name());

            for entry in files {
                if let Some(task) = parse_task_file(&entry.path(), status.clone(), id) {
                    tasks.push(task);
                    id += 1;
                }
            }
        }
    }

    tasks
}

// ── Application state ──────────────────────────────────────────────────────────

struct App {
    tasks: Vec<Task>,
    table_state: TableState,
    filter: Option<Status>,
}

impl App {
    fn new(tasks: Vec<Task>) -> Self {
        let mut table_state = TableState::default();
        if !tasks.is_empty() {
            table_state.select(Some(0));
        }
        App {
            tasks,
            table_state,
            filter: None,
        }
    }

    fn filtered_tasks(&self) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|t| match &self.filter {
                None => true,
                Some(f) => &t.status == f,
            })
            .collect()
    }

    fn move_down(&mut self) {
        let len = self.filtered_tasks().len();
        if len == 0 {
            return;
        }
        let next = match self.table_state.selected() {
            Some(i) => (i + 1) % len,
            None => 0,
        };
        self.table_state.select(Some(next));
    }

    fn move_up(&mut self) {
        let len = self.filtered_tasks().len();
        if len == 0 {
            return;
        }
        let prev = match self.table_state.selected() {
            Some(0) | None => len - 1,
            Some(i) => i - 1,
        };
        self.table_state.select(Some(prev));
    }

    fn set_filter(&mut self, filter: Option<Status>) {
        self.filter = filter;
        let len = self.filtered_tasks().len();
        if len == 0 {
            self.table_state.select(None);
        } else {
            self.table_state.select(Some(0));
        }
    }

    fn filter_label(&self) -> &'static str {
        match &self.filter {
            None => "all",
            Some(s) => s.label(),
        }
    }

    fn summary(&self) -> String {
        let tasks = self.filtered_tasks();
        let total = tasks.len();
        if self.filter.is_none() {
            let pending = tasks.iter().filter(|t| t.status == Status::Pending).count();
            let running = tasks.iter().filter(|t| t.status == Status::Running).count();
            let done = tasks.iter().filter(|t| t.status == Status::Done).count();
            let failed = tasks.iter().filter(|t| t.status == Status::Failed).count();
            format!(
                "{} tasks ({} pending, {} running, {} done, {} failed)",
                total, pending, running, done, failed
            )
        } else {
            format!("{} {} tasks", total, self.filter_label())
        }
    }
}

// ── Rendering ──────────────────────────────────────────────────────────────────

fn ui(frame: &mut ratatui::Frame, app: &mut App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),       // task table
            Constraint::Length(2),    // legend + summary
            Constraint::Length(1),    // status bar
        ])
        .split(area);

    // Header row
    let header = Row::new(vec!["ID", "St", "Pri", "Repo", "Title", "Age"])
        .style(Style::default().add_modifier(Modifier::BOLD));

    // Data rows
    let filtered = app.filtered_tasks();
    let rows: Vec<Row> = filtered
        .iter()
        .map(|task| {
            Row::new(vec![
                Cell::from(task.id.to_string()),
                Cell::from(task.status.icon()),
                Cell::from(task.priority.display()),
                Cell::from(task.repo.clone()),
                Cell::from(task.title.clone()),
                Cell::from(format_age(task.added.as_ref())),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(4),
        Constraint::Length(3),
        Constraint::Length(4),
        Constraint::Length(12),
        Constraint::Min(20),
        Constraint::Length(6),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(" sipag "))
        .row_highlight_style(
            Style::default()
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        );

    frame.render_stateful_widget(table, chunks[0], &mut app.table_state);

    // Legend + summary
    let legend = "  · pending  ⧖ running  ✓ done  ✗ failed";
    let summary_text = format!("{}\n  {}", legend, app.summary());
    let summary = Paragraph::new(summary_text);
    frame.render_widget(summary, chunks[1]);

    // Status bar
    let status_bar =
        Paragraph::new("  j/k:navigate  1-4:filter  0:all  Enter:detail  q:quit")
            .style(Style::default().bg(Color::DarkGray));
    frame.render_widget(status_bar, chunks[2]);
}

// ── Entry point ────────────────────────────────────────────────────────────────

fn main() -> std::io::Result<()> {
    let sipag_dir = std::env::var("SIPAG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".sipag")
        });

    let tasks = load_tasks(&sipag_dir);
    let mut app = App::new(tasks);

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Event loop
    let result = run_app(&mut terminal, &mut app);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> std::io::Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('j') | KeyCode::Down => app.move_down(),
                        KeyCode::Char('k') | KeyCode::Up => app.move_up(),
                        KeyCode::Char('1') => app.set_filter(Some(Status::Pending)),
                        KeyCode::Char('2') => app.set_filter(Some(Status::Running)),
                        KeyCode::Char('3') => app.set_filter(Some(Status::Done)),
                        KeyCode::Char('4') => app.set_filter(Some(Status::Failed)),
                        KeyCode::Char('0') | KeyCode::Char('a') => app.set_filter(None),
                        _ => {}
                    }
                }
            }
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task(id: usize, status: Status) -> Task {
        Task {
            id,
            status,
            priority: Priority::Medium,
            repo: "repo".to_string(),
            title: format!("task {}", id),
            added: None,
        }
    }

    // --- format_age ---

    #[test]
    fn test_format_age_none() {
        assert_eq!(format_age(None), "-");
    }

    #[test]
    fn test_format_age_recent() {
        let dt = Utc::now() - chrono::Duration::seconds(30);
        let result = format_age(Some(&dt));
        assert!(result.ends_with('s'), "expected seconds, got: {}", result);
    }

    #[test]
    fn test_format_age_minutes() {
        let dt = Utc::now() - chrono::Duration::minutes(5);
        assert_eq!(format_age(Some(&dt)), "5m");
    }

    #[test]
    fn test_format_age_hours() {
        let dt = Utc::now() - chrono::Duration::hours(3);
        assert_eq!(format_age(Some(&dt)), "3h");
    }

    #[test]
    fn test_format_age_days() {
        let dt = Utc::now() - chrono::Duration::days(2);
        assert_eq!(format_age(Some(&dt)), "2d");
    }

    // --- Status icons and labels ---

    #[test]
    fn test_status_icons() {
        assert_eq!(Status::Pending.icon(), "·");
        assert_eq!(Status::Running.icon(), "⧖");
        assert_eq!(Status::Done.icon(), "✓");
        assert_eq!(Status::Failed.icon(), "✗");
    }

    #[test]
    fn test_status_labels() {
        assert_eq!(Status::Pending.label(), "pending");
        assert_eq!(Status::Running.label(), "running");
        assert_eq!(Status::Done.label(), "done");
        assert_eq!(Status::Failed.label(), "failed");
    }

    // --- Priority display ---

    #[test]
    fn test_priority_display() {
        assert_eq!(Priority::High.display(), "H");
        assert_eq!(Priority::Medium.display(), "M");
        assert_eq!(Priority::Low.display(), "L");
        assert_eq!(Priority::Unknown.display(), "-");
    }

    // --- Navigation: move_down wraps ---

    #[test]
    fn test_move_down_wraps_at_end() {
        let tasks = vec![make_task(1, Status::Pending), make_task(2, Status::Pending)];
        let mut app = App::new(tasks);
        // Start at last element
        app.table_state.select(Some(1));
        app.move_down();
        assert_eq!(app.table_state.selected(), Some(0));
    }

    #[test]
    fn test_move_down_advances() {
        let tasks = vec![make_task(1, Status::Pending), make_task(2, Status::Pending)];
        let mut app = App::new(tasks);
        app.table_state.select(Some(0));
        app.move_down();
        assert_eq!(app.table_state.selected(), Some(1));
    }

    // --- Navigation: move_up wraps ---

    #[test]
    fn test_move_up_wraps_at_top() {
        let tasks = vec![make_task(1, Status::Pending), make_task(2, Status::Pending)];
        let mut app = App::new(tasks);
        app.table_state.select(Some(0));
        app.move_up();
        assert_eq!(app.table_state.selected(), Some(1));
    }

    #[test]
    fn test_move_up_retreats() {
        let tasks = vec![make_task(1, Status::Pending), make_task(2, Status::Pending)];
        let mut app = App::new(tasks);
        app.table_state.select(Some(1));
        app.move_up();
        assert_eq!(app.table_state.selected(), Some(0));
    }

    // --- Navigation: empty list ---

    #[test]
    fn test_move_on_empty_list() {
        let mut app = App::new(vec![]);
        app.move_down();
        assert_eq!(app.table_state.selected(), None);
        app.move_up();
        assert_eq!(app.table_state.selected(), None);
    }

    // --- Filtering ---

    #[test]
    fn test_filter_pending_only() {
        let tasks = vec![
            make_task(1, Status::Pending),
            make_task(2, Status::Done),
            make_task(3, Status::Failed),
        ];
        let mut app = App::new(tasks);
        app.set_filter(Some(Status::Pending));
        let result = app.filtered_tasks();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, 1);
    }

    #[test]
    fn test_filter_done_only() {
        let tasks = vec![
            make_task(1, Status::Pending),
            make_task(2, Status::Done),
            make_task(3, Status::Done),
        ];
        let mut app = App::new(tasks);
        app.set_filter(Some(Status::Done));
        assert_eq!(app.filtered_tasks().len(), 2);
    }

    #[test]
    fn test_filter_all_shows_everything() {
        let tasks = vec![
            make_task(1, Status::Pending),
            make_task(2, Status::Running),
            make_task(3, Status::Done),
            make_task(4, Status::Failed),
        ];
        let mut app = App::new(tasks);
        app.set_filter(Some(Status::Pending)); // apply a filter first
        app.set_filter(None);                   // then clear it
        assert_eq!(app.filtered_tasks().len(), 4);
    }

    #[test]
    fn test_filter_resets_selection_to_zero() {
        let tasks = vec![
            make_task(1, Status::Pending),
            make_task(2, Status::Done),
        ];
        let mut app = App::new(tasks);
        app.table_state.select(Some(1));
        app.set_filter(Some(Status::Pending));
        assert_eq!(app.table_state.selected(), Some(0));
    }

    #[test]
    fn test_filter_empty_result_deselects() {
        let tasks = vec![make_task(1, Status::Pending)];
        let mut app = App::new(tasks);
        app.set_filter(Some(Status::Done)); // no done tasks
        assert_eq!(app.table_state.selected(), None);
    }

    // --- Summary line ---

    #[test]
    fn test_summary_all() {
        let tasks = vec![
            make_task(1, Status::Pending),
            make_task(2, Status::Running),
            make_task(3, Status::Done),
            make_task(4, Status::Failed),
        ];
        let app = App::new(tasks);
        let s = app.summary();
        assert!(s.contains("4 tasks"), "got: {}", s);
        assert!(s.contains("1 pending"), "got: {}", s);
        assert!(s.contains("1 running"), "got: {}", s);
        assert!(s.contains("1 done"), "got: {}", s);
        assert!(s.contains("1 failed"), "got: {}", s);
    }

    #[test]
    fn test_summary_filtered() {
        let tasks = vec![
            make_task(1, Status::Pending),
            make_task(2, Status::Pending),
            make_task(3, Status::Done),
        ];
        let mut app = App::new(tasks);
        app.set_filter(Some(Status::Pending));
        let s = app.summary();
        assert_eq!(s, "2 pending tasks");
    }

    // --- Frontmatter parsing ---

    #[test]
    fn test_parse_task_file_with_frontmatter() {
        use std::io::Write;

        let dir = std::env::temp_dir();
        let path = dir.join("test_task_frontmatter.md");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(
            f,
            "---\nrepo: myrepo\npriority: high\nadded: 2024-01-01T12:00:00Z\n---\nFix the bug\nMore details"
        )
        .unwrap();

        let task = parse_task_file(&path, Status::Pending, 1).unwrap();
        assert_eq!(task.repo, "myrepo");
        assert_eq!(task.title, "Fix the bug");
        assert!(matches!(task.priority, Priority::High));
        assert!(task.added.is_some());

        fs::remove_file(path).ok();
    }

    #[test]
    fn test_parse_task_file_no_frontmatter() {
        use std::io::Write;

        let dir = std::env::temp_dir();
        let path = dir.join("test_task_no_fm.md");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, "Simple task title\nSome description").unwrap();

        let task = parse_task_file(&path, Status::Done, 2).unwrap();
        assert_eq!(task.title, "Simple task title");
        assert_eq!(task.repo, "");

        fs::remove_file(path).ok();
    }

    #[test]
    fn test_parse_task_file_empty_title_uses_filename() {
        use std::io::Write;

        let dir = std::env::temp_dir();
        let path = dir.join("my-task-slug.md");
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, "---\nrepo: r\n---\n").unwrap();

        let task = parse_task_file(&path, Status::Failed, 3).unwrap();
        assert_eq!(task.title, "my-task-slug");

        fs::remove_file(path).ok();
    }
}
