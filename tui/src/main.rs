#[allow(dead_code)]
mod app;
mod board_app;
#[allow(dead_code)]
mod task;
mod ui;

use anyhow::Result;
use ratatui::crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{
    io,
    time::{Duration, Instant},
};

fn main() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = board_app::BoardApp::new()?;
    let result = run_board(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_board(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut board_app::BoardApp,
) -> Result<()> {
    let tick = Duration::from_millis(200);
    let mut last_tick = Instant::now();
    let mut last_board_refresh = Instant::now();

    loop {
        terminal.draw(|f| ui::board::render_board(f, app))?;

        let timeout = tick.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                // Ctrl-C always quits.
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    return Ok(());
                }
                if app.handle_key(key)? {
                    return Ok(());
                }
            }
        }

        if last_tick.elapsed() >= tick {
            app.on_tick()?;
            last_tick = Instant::now();
        }

        // Refresh board data from disk every 2 seconds.
        if last_board_refresh.elapsed() >= Duration::from_secs(2) {
            app.load_board()?;
            last_board_refresh = Instant::now();
        }
    }
}
