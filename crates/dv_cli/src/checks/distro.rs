//! A WSL distro's checks, seen from Windows (docs/PLAN.md §12.2): dv installed there, then the
//! distro's own list, which **the distro's Linux dv** reads (`dv doctor --json` run inside it),
//! so it sees the distro user's own home, shell and `gh`.
//!
//! - **Listing** ([`listed`]) is `wsl.exe --list`, which boots nothing; it runs only when this
//!   app has a Linux build to check against, so a dv without one (a development build before
//!   `script/build-linux`) starts no `wsl.exe` at all. The distros container tools make for
//!   their VMs ([`VM_DISTROS`]: Docker Desktop's, Rancher Desktop's, Podman's machines) aren't
//!   the user's: they aren't listed.
//! - **A stopped distro** is listed as stopped, with no checks: it is never booted to check
//!   it. [`start`] boots it, when the user asks.
//! - **A running one** ([`environment`]), asked afresh whether it still runs right before
//!   anything runs in it (`wsl::running_now`; a distro stopped in the instant between that and
//!   the probe would still boot): `dv_backend::install::probe` says whether this app's
//!   build is installed there and intact ([`id::DV`], Install: `dv_backend::install::install`);
//!   when it is, that build runs `dv doctor --json` in the distro (`dv_backend::wsl::RUN`, the
//!   line the CLI runs a command there with, PLAN §10.4) and its checks follow. Applying one of
//!   those runs that build again: `dv setup agents --yes --json` for a skill, `dv setup path
//!   --yes --json` for `PATH` ([`apply`]).
//!
//! The commands run through `wsl.exe` directly rather than a repository's backend connection:
//! a backend's processes belong to an open worktree (`Backend::spawn_stdio`), and setup runs
//! before any repository there is open. Every one of them runs with the sandbox seam
//! `DV_WSL_HOME` as `$HOME` when set (`dv_backend::wsl::Shell`), the distro dv's checks and
//! writes with it, and with [`Seams::distro_env`]. [`Seams::only_distros`] names the only distros
//! listed checks, starts and fixes reach (an automation run's).

use std::time::Duration;

use dv_backend::install::{self, BuildState, LinkState, LinuxBuilds, Probe};
use dv_backend::wsl::{self, Distro, Shell, WslError};
use serde::Deserialize;

use super::{Check, Environment, Fix, Kind, Seams, State, id};

/// How long the distro's `dv doctor` may take: its user's shell, `gh` asking GitHub.
const DOCTOR_TIMEOUT: Duration = Duration::from_secs(90);

/// How long a `dv setup` run there may take.
const SETUP_TIMEOUT: Duration = Duration::from_secs(60);

/// Why the distros aren't listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unlisted {
    /// This app has no Linux build to check them against; why.
    NoBuild(String),
    /// WSL isn't installed (or this isn't Windows).
    NoWsl,
}

/// The prefixes of the distros container tools make for their own VMs (module docs).
pub const VM_DISTROS: &[&str] = &["docker-desktop", "rancher-desktop", "podman-machine"];

/// The distros, and which run, but the container tools' VMs (module docs); nothing is booted.
pub fn listed(seams: &Seams) -> Result<Vec<Distro>, Unlisted> {
    builds(seams)
        .map_err(|error| Unlisted::NoBuild(format!("this dv can't check WSL distros: {error}")))?;
    match wsl::distros() {
        Ok(all) => Ok(all
            .into_iter()
            .filter(|distro| !is_vm(&distro.name))
            .collect()),
        Err(_) => Err(Unlisted::NoWsl),
    }
}

/// Whether `name` is one of the container tools' VM distros ([`VM_DISTROS`]).
pub fn is_vm(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    VM_DISTROS.iter().any(|prefix| name.starts_with(prefix))
}

/// This app's Linux builds, or why there are none.
fn builds(seams: &Seams) -> Result<LinuxBuilds, WslError> {
    let builds = LinuxBuilds::find(&seams.linux_dir)?;
    if builds.is_empty() {
        return Err(builds.missing(wsl::Arch::host()));
    }
    Ok(builds)
}

fn shell(seams: &Seams, distro: &str) -> Result<Shell, String> {
    let shell = Shell::wsl(distro.to_string(), seams.wsl_home.clone())
        .map_err(|error| error.to_string())?;
    Ok(seams
        .distro_env
        .iter()
        .fold(shell, |shell, (name, value)| shell.with_env(name, value)))
}

/// Why this run may not reach `distro` ([`Seams::only_distros`]), if it may not.
fn unreachable(seams: &Seams, distro: &str) -> Option<String> {
    (!seams.may_check(distro)).then(|| {
        format!(
            "this run doesn't reach {distro} (an automation run reaches only its sandbox's distro)"
        )
    })
}

/// `distro`'s environment (module docs): a stopped one with no checks, a running one with dv's
/// install and then its own checks.
pub fn environment(seams: &Seams, distro: &Distro) -> Environment {
    let mut env = Environment {
        name: distro.name.clone(),
        kind: Kind::Wsl,
        running: distro.running,
        note: None,
        checks: Vec::new(),
    };
    if !distro.running {
        env.note = Some(format!(
            "{} isn't running: start it to check it (dv never starts it by itself)",
            distro.name
        ));
        return env;
    }
    if let Some(reason) = unreachable(seams, &distro.name) {
        env.note = Some(reason);
        return env;
    }
    // Listed running, perhaps a while ago: asked afresh before anything starts in it.
    if !wsl::running_now(&distro.name) {
        env.running = false;
        env.note = Some(format!(
            "{} isn't running: start it to check it (dv never starts it by itself)",
            distro.name
        ));
        return env;
    }
    let installed = match probe(seams, &distro.name) {
        Ok((builds, probe)) => {
            let check = dv_check(&distro.name, &probe);
            let ready = probe.build == BuildState::Ok;
            env.checks.push(check);
            ready.then_some(builds)
        }
        Err(reason) => {
            env.checks.push(
                Check::new(id::DV, title(&distro.name), State::Failed, reason).with_fix(
                    Fix::apply("Install", format!("dv setup wsl {}", distro.name)),
                ),
            );
            None
        }
    };
    let Some(builds) = installed else {
        env.note = Some(format!(
            "the other checks run once dv is installed in {}",
            distro.name
        ));
        return env;
    };
    match doctor(seams, &distro.name, &builds) {
        Ok(theirs) => env.checks.extend(theirs.checks),
        // A failed check, so `dv doctor` says something is wrong (PLAN §10.3).
        Err(reason) => env.checks.push(Check::new(
            id::CHECKS,
            format!("dv's checks in {}", distro.name),
            State::Failed,
            format!("couldn't run dv in {}: {reason}", distro.name),
        )),
    }
    env
}

fn title(distro: &str) -> String {
    format!("dv in {distro}")
}

fn probe(seams: &Seams, distro: &str) -> Result<(LinuxBuilds, Probe), String> {
    let builds = builds(seams).map_err(|error| error.to_string())?;
    let shell = shell(seams, distro)?;
    let probe = install::probe(&shell, &builds).map_err(|error| error.to_string())?;
    Ok((builds, probe))
}

/// The [`id::DV`] check for what `probe` found.
fn dv_check(distro: &str, probe: &Probe) -> Check {
    let fix = |action: &str| Fix::apply(action, format!("dv setup wsl {distro}"));
    let this = if probe.packaged {
        "this dv's build (packaged)"
    } else {
        "this development build"
    };
    match (probe.build, probe.link) {
        (BuildState::Ok, LinkState::Ok) => Check::new(
            id::DV,
            title(distro),
            State::Ok,
            format!(
                "{this} is installed at {}, and {} runs it",
                probe.path,
                probe.link_path()
            ),
        ),
        (BuildState::Ok, LinkState::Directory) => Check::new(
            id::DV,
            title(distro),
            State::Ok,
            format!("{this} is installed at {}", probe.path),
        ),
        // A development build leaves the installed copy's link alone (PLAN §5.2).
        (BuildState::Ok, LinkState::Packaged) if !probe.packaged => Check::new(
            id::DV,
            title(distro),
            State::Ok,
            format!(
                "{this} is installed at {}; {} runs the installed dv (packaged)",
                probe.path,
                probe.link_path()
            ),
        ),
        (BuildState::Ok, _) => Check::new(
            id::DV,
            title(distro),
            State::OutOfDate,
            format!("installed, but {} doesn't run it", probe.link_path()),
        )
        .with_fix(fix("Install")),
        (BuildState::Corrupt, _) => Check::new(
            id::DV,
            title(distro),
            State::OutOfDate,
            format!("{} differs from this dv's build", probe.path),
        )
        .with_fix(fix("Install")),
        (BuildState::Missing, _) => Check::new(
            id::DV,
            title(distro),
            State::Missing,
            format!("this dv's build isn't installed in {distro}"),
        )
        .with_fix(fix("Install")),
    }
}

/// The document the distro's `dv doctor --json` prints.
#[derive(Deserialize)]
struct DoctorDoc {
    environments: Vec<Environment>,
}

/// The distro's own checks, as its dv reads them.
fn doctor(seams: &Seams, distro: &str, builds: &LinuxBuilds) -> Result<Environment, String> {
    let output = run_dv(seams, distro, builds, &["doctor", "--json"], DOCTOR_TIMEOUT)?;
    let stdout = wsl::decode(&output.stdout);
    let doc: DoctorDoc = serde_json::from_str(stdout.trim()).map_err(|error| {
        format!(
            "its dv doctor printed what dv can't read ({error}): {}",
            first_words(&output)
        )
    })?;
    doc.environments
        .into_iter()
        .next()
        .ok_or_else(|| "its dv doctor listed no environment".to_string())
}

/// Runs this app's build in `distro` with `args` (`dv_backend::wsl::RUN`, in `/`).
fn run_dv(
    seams: &Seams,
    distro: &str,
    builds: &LinuxBuilds,
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let shell = shell(seams, distro)?;
    let hashes = builds.hash_args();
    let mut line: Vec<&str> = vec![&hashes[0], &hashes[1], "/"];
    line.extend_from_slice(args);
    let output = shell
        .run(wsl::RUN, &line, timeout)
        .map_err(|error| error.to_string())?;
    match output.status.code() {
        Some(wsl::RUN_MISSING) => Err(format!("this dv's build isn't installed in {distro}")),
        Some(code) if wsl::RUN_NO_BUILD.contains(&code) => {
            Err(format!("this dv has no build for {distro}'s machine"))
        }
        _ => Ok(output),
    }
}

/// The first thing a run said on stderr (a `dv: …` line), else on stdout.
fn first_words(output: &std::process::Output) -> String {
    let stderr = wsl::decode(&output.stderr);
    let stdout = wsl::decode(&output.stdout);
    stderr
        .lines()
        .chain(stdout.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.strip_prefix("dv: ").unwrap_or(line).to_string())
        .unwrap_or_else(|| format!("it printed nothing ({})", output.status))
}

/// Applies check `id`'s fix in `distro` (module docs), `progress` hearing an install's bytes;
/// what was done, as a line.
pub fn apply(
    seams: &Seams,
    distro: &str,
    id: &str,
    progress: &dyn Fn(u64, u64),
) -> Result<String, String> {
    if let Some(reason) = unreachable(seams, distro) {
        return Err(reason);
    }
    match id {
        id::DV => {
            let (builds, probe) = probe(seams, distro)?;
            if probe.up_to_date() {
                return Ok(format!("dv is installed in {distro}"));
            }
            let build = builds
                .get(probe.arch)
                .ok_or_else(|| builds.missing(probe.arch).to_string())?;
            let done = install::install(&shell(seams, distro)?, build, progress)
                .map_err(|error| error.to_string())?;
            Ok(if done.linked {
                format!("installed dv in {distro}")
            } else {
                format!("installed dv in {distro}; {}", done.warnings.join("; "))
            })
        }
        id::PATH => setup(seams, distro, &["setup", "path", "--yes", "--json"])
            .map(|()| format!("put dv on PATH in {distro}")),
        id if id::is_skill(id) => setup(seams, distro, &["setup", "agents", "--yes", "--json"])
            .map(|()| format!("installed the dv-review skill in {distro}")),
        id::GIT | id::GH => Err(format!(
            "dv doesn't change that in {distro}; run the command it shows there"
        )),
        other => Err(format!("there is no check {other:?} in {distro}")),
    }
}

/// Runs `dv <args>` in `distro`, which must succeed.
fn setup(seams: &Seams, distro: &str, args: &[&str]) -> Result<(), String> {
    let builds = builds(seams).map_err(|error| error.to_string())?;
    let output = run_dv(seams, distro, &builds, args, SETUP_TIMEOUT)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(first_words(&output))
    }
}

/// Boots `distro`, when the user asks (the setup screen's Start): the one thing here that
/// starts a stopped distro.
pub fn start(seams: &Seams, distro: &str) -> Result<(), String> {
    if let Some(reason) = unreachable(seams, distro) {
        return Err(reason);
    }
    let shell = shell(seams, distro)?;
    let output = shell
        .run("exit 0", &[], wsl::START_TIMEOUT)
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("{distro} didn't start: {}", first_words(&output)))
    }
}
