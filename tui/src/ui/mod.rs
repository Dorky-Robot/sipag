mod detail;
mod list;

use crate::app::{App, Mode};
use ratatui::Frame;

/// Top-level render dispatcher — calls the correct view renderer.
pub fn render(f: &mut Frame, app: &mut App) {
    match app.mode {
        Mode::TaskList => list::render_list(f, app),
        Mode::Executor => detail::render_executor(f, app),
    }
}
