mod detail;
mod list;

use crate::app::{App, View};
use ratatui::Frame;

/// Top-level render dispatcher — calls the correct view renderer.
pub fn render(f: &mut Frame, app: &App) {
    match app.view {
        View::List => list::render_list(f, app),
        View::Detail => detail::render_detail(f, app),
    }
}
