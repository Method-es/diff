//! How `dv` (no subcommand) and `dv open` start the app (docs/PLAN.md §10.1, §13.1).
//!
//! `dv.exe` is a console program, so every headless command prints and returns its exit code
//! in a terminal. The app itself starts **detached**: the same binary run again with the hidden
//! `--no-detach`, with no console (`DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` on Windows, its
//! own process group elsewhere), its standard streams closed and none of the caller's handles
//! inherited ([`spawn_detached`]), while the first one returns 0 at once, also to a caller that
//! captures its output. `--automation` (a driver talks to it over stdio) and `--no-detach` run
//! the app in this process instead ([`launch`]).
//!
//! What the app starts on ([`Startup`]) is worked out before anything starts, so a PR (a later
//! version) is refused in the terminal; before detaching, a path that isn't a repository and a
//! review that doesn't exist are too ([`check`]), since the detached app has no terminal to say
//! so in. A second app on the same data dir is allowed for now (PLAN §17).

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use dv_cli::{CliError, GlobalArgs};
use dv_core::backend::{Backend, LocalBackend, LocalBackendOptions};
use dv_core::location::Location;
use dv_core::review::{ReviewId, ReviewStore, StoreError};

use crate::cli::{Cli, OpenArgs};

/// Where the app runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Launch {
    /// In this process: `--automation`, `--automation-pipe` or `--no-detach`.
    InProcess,
    /// In a detached copy of this binary, this one returning at once.
    Detach,
}

/// Where `cli` (a GUI command line: no subcommand, or `open`) runs the app.
pub fn launch(cli: &Cli) -> Launch {
    if cli.automation_enabled() || cli.no_detach {
        Launch::InProcess
    } else {
        Launch::Detach
    }
}

/// What the app opens once its window is up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Startup {
    /// Nothing: an automation run started without `open` (scenarios open what they need).
    Nothing,
    /// Bare `dv`: the working directory's repository, the switcher open on it, when it is one;
    /// nothing otherwise (dv started from a shortcut, say).
    CurrentRepo(Location),
    /// `dv open [<path>]`: that repository, the switcher open on it (the automation `open`
    /// command's behaviour); one that doesn't open shows why in the main area's banner.
    Repo(Location),
    /// `dv open [<path>] --review <id>`: that review, through `Shell::open`.
    Review(Location, ReviewId),
}

/// What the app should open for `cli`, whose subcommand was `open` (`open`'s arguments) or
/// none, run in `cwd`. A PR, two locations at once, a malformed review id and the headless
/// commands' `--json` and `--author` (the app prints nothing and acts as the reviewer, PLAN
/// §10.1) are usage errors (exit 2), before any window opens.
pub fn startup(cli: &Cli, open: Option<&OpenArgs>, cwd: &Path) -> Result<Startup, CliError> {
    if cli.global.json {
        return Err(CliError::usage(
            "`dv` and `dv open` start the app, which prints no JSON: --json is for the commands \
             that print (`dv status --json`)",
        ));
    }
    if cli.global.author.is_some() {
        return Err(CliError::usage(
            "the app always acts as the reviewer: --author is for the commands an agent runs \
             (`dv comment reply <id> --author <name>`)",
        ));
    }
    let target = open.and_then(|open| open.target.as_deref());
    if let Some(target) = target
        && is_pr(target)
    {
        return Err(CliError::unsupported(
            "GitHub PRs arrive in a later version; `dv open` takes a repository's path for now",
        ));
    }
    let named = target.is_some() || cli.global.repo.is_some() || cli.global.wsl.is_some();
    if target.is_some() && (cli.global.repo.is_some() || cli.global.wsl.is_some()) {
        return Err(CliError::usage(
            "`dv open` takes one repository: a path, --repo or --wsl",
        ));
    }
    let global = GlobalArgs {
        repo: target
            .map(str::to_string)
            .or_else(|| cli.global.repo.clone()),
        ..cli.global.clone()
    };
    let location = dv_cli::location(&global, cwd)?;
    if let Some(id) = &cli.global.review {
        let id = ReviewId::parse(id.trim())
            .map_err(|error| CliError::usage(format!("--review: {error}")))?;
        return Ok(Startup::Review(location, id));
    }
    Ok(if named || open.is_some() {
        Startup::Repo(location)
    } else if cli.automation_enabled() {
        Startup::Nothing
    } else {
        Startup::CurrentRepo(location)
    })
}

/// Whether `dv open`'s argument names a pull request: `#482` or a URL.
fn is_pr(target: &str) -> bool {
    let target = target.trim();
    let number = target
        .strip_prefix('#')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    number || target.starts_with("https://") || target.starts_with("http://")
}

/// Checks, before detaching, that `startup` names a repository that opens (the app passes the
/// default `options`: the user's own git configuration, as the app will open it) and, for a
/// review, one the repository has. A WSL location is left to the app, whose switcher says why
/// it can't open yet; bare `dv` outside a repository is fine (the app opens on nothing).
pub fn check(startup: &Startup, options: LocalBackendOptions) -> Result<(), CliError> {
    let (path, review) = match startup {
        Startup::Repo(Location::Local(path)) => (path, None),
        Startup::Review(Location::Local(path), id) => (path, Some(id)),
        _ => return Ok(()),
    };
    let backend: Arc<dyn Backend> = Arc::new(LocalBackend::open_with(path, options)?);
    if let Some(id) = review {
        let root = backend.repo().toplevel.clone();
        match ReviewStore::new(backend).load(id) {
            Ok(_) => {}
            Err(StoreError::UnknownReview { .. }) => {
                return Err(CliError::not_found(format!("no review {id} in {root}")));
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// The command line of the detached copy: `--no-detach`, then this one's arguments (`args`,
/// the program name first) as given.
pub fn detached_args(args: &[OsString]) -> Vec<OsString> {
    std::iter::once(OsString::from("--no-detach"))
        .chain(args.iter().skip(1).cloned())
        .collect()
}

/// Starts this binary again with [`detached_args`], detached from this process's console and
/// process group, its standard streams closed (module docs, [`spawn_detached`]).
pub fn detach(args: &[OsString]) -> std::io::Result<()> {
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command.args(detached_args(args));
    spawn_detached(command)
}

/// Starts `command` detached: no console and a process group of its own, its standard streams
/// closed, and none of this process's standard handles handed down. On Windows a child
/// inherits every inheritable handle of its parent, and this process's stdout and stderr are
/// whatever its caller gave it — a pipe, when the caller captures `dv open`'s output
/// (`$(dv open .)`, an agent's shell tool) — so the app would hold that pipe open for as long
/// as it runs, and the caller would wait for the window to close before seeing the end of the
/// output. They are made non-inheritable first; this process exits right after.
pub(crate) fn spawn_detached(mut command: std::process::Command) -> std::io::Result<()> {
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        keep_std_handles_from_children();
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    // The child is never waited for: it outlives this process.
    command.spawn().map(drop)
}

/// Makes this process's standard handles non-inheritable ([`spawn_detached`]). Best effort: a
/// handle that isn't there, or whose flag can't change, is left as it is.
#[cfg(windows)]
fn keep_std_handles_from_children() {
    use windows_sys::Win32::Foundation::{
        HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: `GetStdHandle` takes no pointers; `SetHandleInformation` changes one flag of
        // a handle this process holds, and a null or invalid handle is skipped before it.
        unsafe {
            let handle = GetStdHandle(which);
            if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

#[cfg(test)]
mod tests;
