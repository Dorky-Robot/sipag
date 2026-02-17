mod app;
mod config;
mod state;

use anyhow::{Context, Result};
use crossterm::{
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::prelude::*;
use std::io::stdout;
use std::path::PathBuf;

fn main() -> Result<()> {
    // Panic hook: restore terminal before printing panic
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = stdout().execute(LeaveAlternateScreen);
        default_hook(info);
    }));

    let project_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    let project_dir = std::fs::canonicalize(&project_dir)
        .with_context(|| format!("Invalid project directory: {}", project_dir.display()))?;

    let cfg = config::load_config(&project_dir)?;

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let mut app_state = app::App::new(project_dir, cfg)?;
    let result = app::run(&mut terminal, &mut app_state);

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    result
}
