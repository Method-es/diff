//! The setup and doctor checks (docs/PLAN.md §12): one list per **environment** — this machine
//! (Windows, macOS or Linux, a WSL distro when dv runs inside one) and, on Windows, each WSL
//! distro — that `dv doctor`, `dv setup`, the app's first-run screen and its Settings › Setup
//! all show.
//!
//! Each [`Check`] has an id, a title, a [`State`] with a one-line detail, and a [`Fix`]: the
//! button that applies it (dv can do it: install the skill, add dv to `PATH`) and the command
//! that does the same by hand (`dv setup agents`, `gh auth login`). Detecting is read-only and
//! fast; applying is idempotent.
//!
//! The checks, in the order they list:
//! 1. [`id::DV`], **dv in the distro** (a distro's, from Windows): this app's Linux build
//!    installed and intact (`dv_backend::install::probe`), with Install ([`distro`]).
//! 2. [`id::PATH`], **the CLI on `PATH`** ([`path`]): on Windows this executable's folder in
//!    the user `Path` (through [`user_path`]'s seam); on Linux and in a distro
//!    `~/.local/bin/dv` running this dv and `~/.local/bin` on the user's shell's `PATH`; on
//!    macOS skipped, unverified (PLAN §13.2).
//! 3. **The `dv-review` skill** per agent ([`skills`]): Claude Code, Codex, Cursor.
//! 4. [`id::GIT`], **git identity** ([`git`]): `user.name` and `user.email`, no apply.
//! 5. [`id::GH`], **GitHub** ([`gh`]): `gh` there and signed in, no apply.
//!
//! A distro's checks run as that distro's Linux dv (`dv doctor --json` there), so they read and
//! write the distro user's own home; a stopped distro is listed as stopped and never booted to
//! check it ([`distro`]). Everything the checks read and write outside the process goes through
//! [`Seams`], which tests and automation runs point at scratch folders.

pub mod distro;
mod gh;
mod git;
pub mod path;
mod process;
mod seams;
pub mod skills;
pub mod user_path;

use serde::{Deserialize, Serialize};

pub use seams::{GH_VAR, ONLY_DISTROS_VAR, Seams};

/// The checks' ids, as `dv doctor --json`, the app's automation targets and its state name
/// them.
pub mod id {
    /// dv installed in a WSL distro.
    pub const DV: &str = "dv";
    /// The CLI on `PATH`.
    pub const PATH: &str = "path";
    /// The `dv-review` skill for Claude Code.
    pub const SKILL_CLAUDE_CODE: &str = "skill-claude-code";
    /// The skill for Codex.
    pub const SKILL_CODEX: &str = "skill-codex";
    /// The skill for Cursor.
    pub const SKILL_CURSOR: &str = "skill-cursor";
    /// git's `user.name` and `user.email`.
    pub const GIT: &str = "git";
    /// `gh`, signed in.
    pub const GH: &str = "gh";
    /// WSL distros, when this dv can't check them (no Linux build beside it).
    pub const WSL: &str = "wsl";
    /// A running distro's own checks, when its dv couldn't run them.
    pub const CHECKS: &str = "checks";

    /// Whether `id` names one of the skill checks.
    pub fn is_skill(id: &str) -> bool {
        id.starts_with("skill-")
    }
}

/// How a check stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Ok,
    Missing,
    /// There, but not as this dv has it: a skill an older dv installed, or one edited since.
    OutOfDate,
    /// Couldn't be told, or broken.
    Failed,
    /// Doesn't apply here: the agent isn't installed, macOS's PATH.
    Skipped,
}

impl State {
    /// `ok`, `missing`, `out_of_date`, `failed` or `skipped`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Missing => "missing",
            Self::OutOfDate => "out_of_date",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }

    /// Something to do: missing, out of date or failed. `dv doctor` exits 1 when any check is.
    pub fn needs_fixing(self) -> bool {
        matches!(self, Self::Missing | Self::OutOfDate | Self::Failed)
    }
}

/// How a check that needs fixing is fixed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fix {
    /// The button that applies it ("Install", "Update", "Add to PATH"), when dv can; `None`
    /// when only the command does (`gh auth login`).
    pub action: Option<String>,
    /// The command that fixes it by hand, to copy or run: `dv setup agents`, `gh auth login`.
    pub command: Option<String>,
}

impl Fix {
    fn apply(action: &str, command: impl Into<String>) -> Option<Self> {
        Some(Self {
            action: Some(action.to_string()),
            command: Some(command.into()),
        })
    }

    fn command(command: impl Into<String>) -> Option<Self> {
        Some(Self {
            action: None,
            command: Some(command.into()),
        })
    }
}

/// One check's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// One of [`id`].
    pub id: String,
    /// What is checked: "dv on PATH", "dv-review skill for Claude Code".
    pub title: String,
    pub state: State,
    /// One line on how it stands.
    pub detail: String,
    /// How to fix it, when it needs fixing and can be.
    pub fix: Option<Fix>,
}

impl Check {
    fn new(id: &str, title: impl Into<String>, state: State, detail: impl Into<String>) -> Self {
        Self {
            id: id.to_string(),
            title: title.into(),
            state,
            detail: detail.into(),
            fix: None,
        }
    }

    fn with_fix(mut self, fix: Option<Fix>) -> Self {
        self.fix = fix;
        self
    }

    /// Whether dv can apply its fix ([`apply_here`], [`distro::apply`]).
    pub fn can_apply(&self) -> bool {
        self.state.needs_fixing() && self.fix.as_ref().is_some_and(|fix| fix.action.is_some())
    }
}

/// What kind of machine an environment is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Windows,
    Macos,
    Linux,
    /// A WSL distro: seen from Windows, or this machine when dv runs inside one.
    Wsl,
}

impl Kind {
    pub fn key(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Macos => "macos",
            Self::Linux => "linux",
            Self::Wsl => "wsl",
        }
    }
}

/// One environment's checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    /// "Windows", "macOS", "Linux", or the distro as WSL names it ("Ubuntu").
    pub name: String,
    pub kind: Kind,
    /// Whether it runs: a stopped distro has no checks until it is started (never booted to
    /// check it).
    pub running: bool,
    /// Why its checks are missing or short, when they are: "Ubuntu isn't running", "the other
    /// checks run once dv is installed in Ubuntu".
    pub note: Option<String>,
    pub checks: Vec<Check>,
}

impl Environment {
    pub fn check(&self, id: &str) -> Option<&Check> {
        self.checks.iter().find(|check| check.id == id)
    }

    /// Whether any of its checks needs fixing.
    pub fn needs_fixing(&self) -> bool {
        self.checks.iter().any(|check| check.state.needs_fixing())
    }
}

/// This machine's environment: its name and kind, and its checks (module docs) but the
/// distros'.
pub fn this_machine(seams: &Seams) -> Environment {
    let (name, kind) = this_machine_name(seams);
    let mut checks = vec![path::check(seams)];
    checks.extend(skills::checks(seams));
    checks.push(git::check(seams));
    checks.push(gh::check(seams));
    Environment {
        name,
        kind,
        running: true,
        note: None,
        checks,
    }
}

/// This machine's environment with only the checks the status bar's notice needs, the skills:
/// what the app checks at startup, without running git, `gh` or the user's shell (PLAN §12.1).
pub fn this_machine_skills(seams: &Seams) -> Environment {
    let (name, kind) = this_machine_name(seams);
    Environment {
        name,
        kind,
        running: true,
        note: None,
        checks: skills::checks(seams),
    }
}

/// What this machine is called among the environments.
pub fn this_machine_name(seams: &Seams) -> (String, Kind) {
    if cfg!(windows) {
        ("Windows".to_string(), Kind::Windows)
    } else if cfg!(target_os = "macos") {
        ("macOS".to_string(), Kind::Macos)
    } else if let Some(distro) = &seams.this_distro {
        (distro.clone(), Kind::Wsl)
    } else {
        ("Linux".to_string(), Kind::Linux)
    }
}

/// Every environment: this machine, then (on Windows) each WSL distro ([`distro`]). When this
/// dv has no Linux build beside it the distros can't be checked, which this machine's list says
/// (a skipped [`id::WSL`] row) without starting `wsl.exe`. Blocks while the checks run (git,
/// `gh`, a distro's dv); never call it on a UI thread.
pub fn environments(seams: &Seams) -> Vec<Environment> {
    let mut here = this_machine(seams);
    if !cfg!(windows) {
        return vec![here];
    }
    match distro::listed(seams) {
        Ok(distros) => {
            let mut all = vec![here];
            all.extend(
                distros
                    .iter()
                    .map(|distro| distro::environment(seams, distro)),
            );
            all
        }
        Err(distro::Unlisted::NoBuild(reason)) => {
            here.checks
                .push(Check::new(id::WSL, "WSL distros", State::Skipped, reason));
            vec![here]
        }
        Err(distro::Unlisted::NoWsl) => vec![here],
    }
}

/// Applies check `id`'s fix on this machine; what was done, as a line. Idempotent: a check that
/// needs nothing is left alone.
pub fn apply_here(seams: &Seams, id: &str) -> Result<String, String> {
    match id {
        id::PATH => path::add(seams).map(|done| done.line()),
        id if id::is_skill(id) => skills::apply(seams, id),
        id::GIT | id::GH => Err(format!(
            "dv doesn't change {}; run the command it shows",
            if id == id::GIT {
                "git's configuration"
            } else {
                "gh's sign-in"
            }
        )),
        other => Err(format!("there is no check {other:?} on this machine")),
    }
}
