mod app;
mod context_ui;

use crate::config::{GlobalConfig, Paths};
use crate::theme::{self, Theme};
use std::cell::Cell;

// The palette belongs to the TUI session thread; Context scan workers do not render.
thread_local! {
    static ACTIVE_THEME: Cell<Theme> = Cell::new(theme::default_theme());
}

fn activate_theme(global: &GlobalConfig) -> Result<(), String> {
    let selected = theme::resolve(global.theme())?;
    ACTIVE_THEME.set(selected);
    Ok(())
}

fn reset_theme() {
    ACTIVE_THEME.set(theme::default_theme());
}

fn active_theme() -> Theme {
    ACTIVE_THEME.get()
}

pub(crate) fn run_main(
    paths: Paths,
    global: Option<GlobalConfig>,
    global_error: Option<String>,
) -> Result<(), String> {
    app::run(paths, global, global_error)
}
