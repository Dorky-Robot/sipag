pub mod board;
#[allow(dead_code)]
mod detail;
#[allow(dead_code)]
mod list;

#[allow(dead_code)]
use crate::app::{App, View};
use ratatui::Frame;

/// Top-level render dispatcher for the worker view — calls the correct view renderer.
#[allow(dead_code)]
pub fn render(f: &mut Frame, app: &App) {
    match app.view {
        View::List => list::render_list(f, app),
        View::Detail => detail::render_detail(f, app),
    }
}
