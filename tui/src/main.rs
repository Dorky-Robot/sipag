mod app;
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

    let mut app = app::App::new()?;
    let result = run(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut app::App) -> Result<()> {
    let tick = Duration::from_millis(200);
    let mut last_tick = Instant::now();
    let mut last_task_refresh = Instant::now();

    loop {
        terminal.draw(|f| ui::render(f, app))?;

        let timeout = tick.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                // Ctrl-C always quits
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    return Ok(());
                }
                if app.handle_key(key)? {
                    return Ok(());
                }

                // Check if user requested to attach to a running container
                if let Some(container) = app.attach_request.take() {
                    // Suspend TUI
                    disable_raw_mode()?;
                    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

                    // Run docker exec -it <container> tmux attach -t claude
                    let status = std::process::Command::new("docker")
                        .args(["exec", "-it", &container, "tmux", "attach", "-t", "claude"])
                        .status();

                    if let Err(e) = status {
                        eprintln!("Failed to attach: {e}");
                        std::thread::sleep(Duration::from_secs(1));
                    }

                    // Resume TUI
                    enable_raw_mode()?;
                    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
                    terminal.clear()?;
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
