//! `dv`: the entry point for the GUI, the CLI and, in the Linux build, `dv backend --stdio`
//! (docs/PLAN.md §4, §10, §13.1).
//!
//! gpui: **yes**, for the GUI. The headless commands live in `dv_cli` and never touch gpui
//! (PLAN §4).

// No console window for a release build on Windows (PLAN §13.1 splits the console `dv.exe`
// from this windowed app).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
#[cfg(feature = "automation")]
mod automation;
mod cli;
mod data_dir;
mod fatal;

use std::process::ExitCode;

use clap::Parser as _;

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .target(env_logger::Target::Stderr)
        .init();

    let cli = cli::Cli::parse();
    // A driver is waiting on a scripted run; a modal error box would only hang it.
    let show_dialog = !cli.automation_enabled();
    match app::run(cli) {
        Ok(code) => code,
        Err(error) => {
            fatal::report(&error, show_dialog);
            ExitCode::FAILURE
        }
    }
}
