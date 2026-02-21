mod app;
mod executor;
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

    let mut app = app::App::new()?;
    let result = run(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut app::App,
) -> Result<()>
where
    B::Error: Send + Sync + 'static,
{
    let tick = Duration::from_millis(200);
    let mut last_tick = Instant::now();
    let mut last_task_refresh = Instant::now();

    loop {
        terminal.draw(|f| ui::render(f, app))?;

        let timeout = tick.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                // Ctrl-C always quits
                if key.code == KeyCode::Char('c')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
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

        // Refresh task list from disk every second
        if last_task_refresh.elapsed() >= Duration::from_secs(1) {
            app.refresh_tasks()?;
            last_task_refresh = Instant::now();
        }
    }
}
