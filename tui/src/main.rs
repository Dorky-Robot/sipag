mod app;
mod task;
mod ui;

use anyhow::Result;
use app::{App, View};
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::path::PathBuf;
use task::load_tasks;

fn main() -> Result<()> {
    // Set up terminal.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Resolve the sipag data directory.
    let sipag_dir = std::env::var("SIPAG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| String::from("/tmp"));
            PathBuf::from(home).join(".sipag")
        });

    let tasks = load_tasks(&sipag_dir);
    let mut app = App::new(tasks);

    let result = run_app(&mut terminal, &mut app);

    // Restore terminal regardless of how we exited.
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    loop {
        terminal.draw(|f| ui::render(f, app))?;

        if let Event::Key(key) = event::read()? {
            match app.view {
                View::List => match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Char('j') | KeyCode::Down => app.select_next(),
                    KeyCode::Char('k') | KeyCode::Up => app.select_prev(),
                    KeyCode::Enter => app.open_detail(),
                    _ => {}
                },
                View::Detail => match key.code {
                    KeyCode::Esc => app.close_detail(),
                    KeyCode::Char('j') | KeyCode::Down => app.scroll_log_down(),
                    KeyCode::Char('k') | KeyCode::Up => app.scroll_log_up(),
                    KeyCode::Char('r') => app.retry_task(),
                    _ => {}
                },
            }
        }
    }
}
