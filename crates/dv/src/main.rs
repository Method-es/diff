//! `dv`: the entry point for the GUI, the CLI and, in the Linux build, `dv backend --stdio`
//! (docs/PLAN.md §4, §10, §13.1).
//!
//! gpui: **yes**, for the GUI. One clap tree parses every command line ([`cli`]); a headless
//! command goes to `dv_cli` before any gpui code runs (PLAN §4), while `dv` alone and `dv open`
//! start the app, detached unless asked otherwise ([`launch`]). `dv.exe` is a console program
//! in every build, so the CLI prints and returns its exit code in a terminal (PLAN §13.1).

mod app;
#[cfg(feature = "automation")]
mod automation;
mod cli;
mod data_dir;
mod fatal;
mod launch;

use std::ffi::OsString;
use std::process::ExitCode;

use clap::Parser as _;
use dv_cli::{CliError, Context, Exit};

use crate::cli::{Cli, Command};
use crate::launch::Launch;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    let mut cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(error) => {
            let mut ctx = Context::process();
            return dv_cli::output::parse_error(&error, &args, &mut ctx).into();
        }
    };
    match cli.command.take() {
        Some(Command::Headless(command)) => {
            init_logging("warn");
            dv_cli::run(&cli.global, command, &mut Context::process()).into()
        }
        Some(Command::Open(open)) => gui(cli, Some(open), &args),
        None => gui(cli, None, &args),
    }
}

/// `dv` or `dv open`: the app, detached or in this process ([`launch`]).
fn gui(cli: Cli, open: Option<cli::OpenArgs>, args: &[OsString]) -> ExitCode {
    let json = cli.global.json;
    let report = |error: CliError| -> ExitCode {
        dv_cli::output::error(&mut Context::process(), json, &error).into()
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let startup = match launch::startup(&cli, open.as_ref(), &cwd) {
        Ok(startup) => startup,
        Err(error) => return report(error),
    };
    match launch::launch(&cli) {
        Launch::Detach => {
            init_logging("warn");
            if let Err(error) = launch::check(&startup, Default::default()) {
                return report(error);
            }
            match launch::detach(args) {
                Ok(()) => Exit::Ok.into(),
                Err(error) => report(CliError::new(
                    Exit::Failure,
                    dv_cli::error::code::IO,
                    format!("couldn't start the app: {error}"),
                )),
            }
        }
        Launch::InProcess => {
            init_logging("info");
            // A driver is waiting on a scripted run; a modal error box would only hang it.
            let show_dialog = !cli.automation_enabled();
            match app::run(cli, startup, show_dialog) {
                Ok(code) => code,
                Err(error) => {
                    fatal::report(&error, show_dialog);
                    ExitCode::FAILURE
                }
            }
        }
    }
}

/// Logs to stderr at `level` unless `RUST_LOG` says otherwise: `warn` for the CLI, whose
/// stderr a person or an agent reads, `info` for the app.
fn init_logging(level: &str) {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(level))
        .target(env_logger::Target::Stderr)
        .init();
}
