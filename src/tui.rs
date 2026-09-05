mod app;
mod tui3;

use crate::config::{GlobalConfig, Paths};

pub(crate) fn run_main(
    paths: Paths,
    global: Option<GlobalConfig>,
    global_error: Option<String>,
) -> Result<(), String> {
    tui3::run(paths, global, global_error)
}
