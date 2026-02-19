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

    let mut project_filter: Option<String> = None;
    let mut sipag_home: Option<PathBuf> = None;
    let mut legacy_project_dir: Option<PathBuf> = None;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--project" if i + 1 < args.len() => {
                project_filter = Some(args[i + 1].clone());
                i += 2;
            }
            "--home" if i + 1 < args.len() => {
                sipag_home = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            arg => {
                // Legacy: positional arg is a project directory
                legacy_project_dir = Some(PathBuf::from(arg));
                i += 1;
            }
        }
    }

    // Determine sipag home
    let home = if let Some(h) = sipag_home {
        h
    } else if let Some(h) = std::env::var_os("SIPAG_HOME") {
        PathBuf::from(h)
    } else {
        dirs_or_home()
    };

    // If legacy project dir is given and no ~/.sipag exists, use legacy mode
    if let Some(ref proj_dir) = legacy_project_dir {
        let proj_dir = std::fs::canonicalize(proj_dir)
            .with_context(|| format!("Invalid project directory: {}", proj_dir.display()))?;
        if !home.join("projects").exists() && proj_dir.join(".sipag").exists() {
            // Legacy single-project mode
            let cfg = config::load_legacy_config(&proj_dir)?;
            enable_raw_mode()?;
            stdout().execute(EnterAlternateScreen)?;
            let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
            let mut app_state = app::App::new_legacy(proj_dir, cfg)?;
            let result = app::run(&mut terminal, &mut app_state);
            disable_raw_mode()?;
            stdout().execute(LeaveAlternateScreen)?;
            return result;
        }
    }

    let cfg = config::load_global_config(&home)?;

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let mut app_state = app::App::new(home, cfg, project_filter)?;
    let result = app::run(&mut terminal, &mut app_state);

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    result
}

fn dirs_or_home() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".sipag")
    } else {
        PathBuf::from(".sipag")
    }
}
