//! The `dv` command tree (clap), the versioned JSON output DTOs agents parse, and the setup/doctor
//! checks shared by the CLI and the onboarding screen (docs/PLAN.md §10, §12).
//!
//! gpui: **never**. The CLI runs headless, on Windows and inside WSL distros; `script/check` fails
//! when gpui appears in its dependency tree (PLAN §4, §13.2).
//!
//! - [`args`]: the global flags and the headless commands, which the `dv` binary composes into
//!   its one tree beside the GUI's options and `dv open` (PLAN §10.1).
//! - [`Context`]: the working directory, environment, standard streams and git environment a
//!   command runs with, so commands run in-process in tests.
//! - [`run`] runs a parsed command and reports its error; [`main_with`] also parses.
//! - [`dto`]: the JSON documents (PLAN §10.3); [`error`]: exit codes and error codes.
//!
//! - [`skill`]: the `dv-review` agent skill `dv setup agents` installs (PLAN §10.5).
//!
//! Inside, `repo` finds and opens the repository (a WSL one is exit 4), `select` picks the
//! review a command means (PLAN §10.2), `identity` who it acts as (PLAN §2.5), `placement`
//! where threads stand now and where a write records them (PLAN §8), `body` reads a
//! command's text (`--body`, `--body-file`, stdin), and `commands` does the work.

pub mod args;
mod body;
mod commands;
mod context;
pub mod dto;
pub mod error;
mod identity;
pub mod output;
mod placement;
mod repo;
mod select;
pub mod skill;

use std::ffi::OsString;

use clap::Parser as _;

pub use args::{Cli, Command, GlobalArgs};
pub use context::{Context, Env};
pub use error::{CliError, Exit};
pub use identity::CLAUDE_CODE;
pub use repo::location;

/// Runs a parsed headless command with `ctx`; an error is reported on stderr (and, with
/// `--json`, on stdout) before its exit code is returned (PLAN §10.3).
pub fn run(global: &GlobalArgs, command: Command, ctx: &mut Context) -> Exit {
    match commands::run(global, command, ctx) {
        Ok(exit) => exit,
        Err(error) => output::error(ctx, global.json, &error),
    }
}

/// Parses `args` (the program name first) as a headless command and runs it with `ctx`: what
/// the `dv` binary does for its headless commands, without the GUI's options and `dv open`.
pub fn main_with<I, T>(args: I, ctx: &mut Context) -> Exit
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    match Cli::try_parse_from(&args) {
        Ok(cli) => run(&cli.global, cli.command, ctx),
        Err(error) => output::parse_error(&error, &args, ctx),
    }
}

#[cfg(test)]
mod tests;
