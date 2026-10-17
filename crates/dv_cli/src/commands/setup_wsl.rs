//! `dv setup wsl <distro> [--yes]` (docs/PLAN.md §5.2, §12): installs this build of dv in a WSL
//! distro, or checks that it is there and intact, and points the distro's `~/.local/bin/dv` at
//! it: the install the app makes when it first opens a repository there, ahead of time.
//!
//! On Windows only (inside a distro dv is the Linux build already: exit 2). The Linux build is
//! the one beside this executable (`DV_LINUX_DIR` names another folder), `script/build-linux`'s
//! output; the distro must be installed, and it boots if it isn't running. It asks before it
//! writes into the distro's home the way `dv setup agents` does: without `--yes` a terminal is
//! shown the plan and asked, anything else is refused with exit 2, and nothing to change needs
//! no answer. WSL, the distro or the Linux build unavailable is exit 4. `DV_WSL_HOME` stands in
//! for the distro user's home (tests; never the real one).
//!
//! A link it can't make (a directory at `~/.local/bin/dv`, a `~/.local/bin` it can't write)
//! leaves the install done: the app and the CLI run the build itself. It says so on stderr,
//! with how to fix it, and succeeds; the JSON's `linked` says whether `~/.local/bin/dv` runs
//! this build now.

use dv_backend::install::{
    self, BuildState, LINUX_DIR_VAR, LinkState, LinuxBuilds, Probe, WSL_HOME_VAR,
};
use dv_backend::wsl::{self, Arch, Shell, WslError};

use super::setup::{confirm, io_error};
use crate::args::{GlobalArgs, SetupWslArgs};
use crate::context::Context;
use crate::dto::SetupWslDoc;
use crate::error::{CliError, Exit, code};
use crate::output;

pub(crate) fn wsl(
    global: &GlobalArgs,
    args: &SetupWslArgs,
    yes: bool,
    ctx: &mut Context,
) -> Result<Exit, CliError> {
    if !cfg!(windows) {
        return Err(CliError::unsupported(
            "`dv setup wsl` runs on Windows; inside a distro this dv is the Linux build already",
        ));
    }
    let builds =
        LinuxBuilds::find(&install::linux_dir(ctx.env.var(LINUX_DIR_VAR))).map_err(unavailable)?;
    if builds.is_empty() {
        return Err(unavailable(builds.missing(Arch::host())));
    }
    let home = wsl::sandbox(ctx.env.var_lossy(WSL_HOME_VAR)).map_err(unavailable)?;
    let distro = wsl::find_distro(&args.distro).map_err(unavailable)?;
    let shell = Shell::wsl(distro.name.clone(), home).map_err(unavailable)?;
    let probe = install::probe(&shell, &builds).map_err(unavailable)?;
    let build = builds
        .get(probe.arch)
        .ok_or_else(|| unavailable(builds.missing(probe.arch)))?;
    let installed = !probe.up_to_date();
    let mut linked = probe.link == LinkState::Ok;
    if installed {
        if !(yes || args.yes.yes) {
            let lines = plan(&distro.name, &probe, build.len);
            if global.json || !ctx.stdin_is_terminal || !ctx.stdout_is_terminal {
                if !global.json {
                    output::text(ctx, &lines)?;
                }
                return Err(CliError::usage(format!(
                    "dv setup wsl installs dv into the home directory in {}: pass --yes to go \
                     ahead",
                    distro.name
                )));
            }
            output::text(ctx, &lines)?;
            if !confirm(ctx, &format!("Install dv in {}? [y/N] ", distro.name))? {
                return Err(CliError::new(
                    Exit::Failure,
                    code::CANCELLED,
                    "nothing installed",
                ));
            }
        }
        let done = install::install(&shell, build, &|_, _| {}).map_err(|error| {
            io_error(format!("installing dv in {} failed: {error}", distro.name))
        })?;
        for warning in &done.warnings {
            output::warning(ctx, warning)?;
        }
        linked = done.linked;
        if !linked {
            output::warning(ctx, &link_fix(&distro.name, &probe))?;
        }
    }
    let doc = SetupWslDoc {
        distro: distro.name.clone(),
        arch: probe.arch.name(),
        build: build.hash.clone(),
        installed,
        linked,
        path: probe.path.clone(),
    };
    if global.json {
        output::json(ctx, &doc)?;
    } else {
        let verb = if installed { "installed" } else { "up to date" };
        output::text(ctx, &[format!("{}  {verb}  {}", distro.name, probe.path)])?;
    }
    Ok(Exit::Ok)
}

/// What an install will do, before it does it.
fn plan(distro: &str, probe: &Probe, len: u64) -> Vec<String> {
    let verb = match probe.build {
        BuildState::Missing => "install",
        BuildState::Corrupt => "install again (the copy there differs)",
        BuildState::Ok => "keep",
    };
    let size = len as f64 / (1024.0 * 1024.0);
    let mut lines = vec![format!(
        "{distro}  {verb}  {} ({}, {size:.1} MB)",
        probe.path, probe.arch
    )];
    let link = probe.link_path();
    lines.push(match probe.link {
        LinkState::Ok => format!("{distro}  keep  {link} (runs it)"),
        LinkState::None => format!("{distro}  link  {link} to it"),
        LinkState::Other => format!("{distro}  link  {link} to it, replacing what is there"),
        LinkState::Directory => format!("{distro}  leave  {link} (a directory)"),
    });
    lines
}

/// How to have `~/.local/bin/dv` run the build when the install couldn't link it.
fn link_fix(distro: &str, probe: &Probe) -> String {
    let link = probe.link_path();
    let what = match probe.link {
        LinkState::Directory => format!("remove the directory {link}"),
        _ => format!("make {}/.local/bin a folder you can write to", probe.home),
    };
    format!(
        "to put this dv on the PATH in {distro}, {what}, then run `dv setup wsl {distro}` again"
    )
}

/// WSL, the distro or the Linux build unavailable: exit 4.
fn unavailable(error: WslError) -> CliError {
    CliError::unavailable(error.to_string())
}
