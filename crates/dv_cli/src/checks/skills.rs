//! The `dv-review` skill for each coding agent set up in the home directory (docs/PLAN.md
//! §10.5, §12.2): where each agent reads it, whether it is there and the same as this dv's, and
//! its install.
//!
//! - **Claude Code** (`~/.claude` exists): `~/.claude/skills/dv-review/SKILL.md`.
//! - **Codex** (`$CODEX_HOME`, else `~/.codex`, exists): `~/.agents/skills/dv-review/SKILL.md`,
//!   the user-level skills folder Codex's documentation names (checked 2026-10-09;
//!   `$CODEX_HOME` holds Codex's own state, not the skills a user adds).
//! - **Cursor** (`~/.cursor` exists): the same `~/.agents/skills/dv-review/SKILL.md`, which
//!   Cursor reads too (its documentation, checked 2026-10-17): one file the two share.
//!
//! A skill that is there but differs from this build's `SKILL.md` is out of date: an older dv
//! installed it, or someone edited it. Installing writes the file beside itself and renames it
//! into place, so an agent never reads half a skill.

use std::path::{Path, PathBuf};

use super::{Check, Fix, Seams, State, id};
use crate::skill::{SKILL_MD, SKILL_NAME};

/// A coding agent dv installs the skill for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    /// `claude-code`, `codex` or `cursor`.
    pub id: &'static str,
    /// "Claude Code", "Codex", "Cursor".
    pub name: &'static str,
    /// The folder whose presence says the agent is set up here.
    pub home: PathBuf,
    /// Where its `SKILL.md` goes.
    pub skill: PathBuf,
}

impl Agent {
    /// The check's id, `skill-<agent>`.
    pub fn check_id(&self) -> &'static str {
        match self.id {
            "claude-code" => id::SKILL_CLAUDE_CODE,
            "codex" => id::SKILL_CODEX,
            _ => id::SKILL_CURSOR,
        }
    }

    /// Whether the agent is set up here.
    pub fn present(&self) -> bool {
        self.home.is_dir()
    }
}

/// The agents, in the order every list shows them, for the home `home` (module docs).
pub fn agents(home: &Path, codex_home: Option<&Path>) -> Vec<Agent> {
    let skill = |dir: PathBuf| dir.join("skills").join(SKILL_NAME).join("SKILL.md");
    vec![
        Agent {
            id: "claude-code",
            name: "Claude Code",
            home: home.join(".claude"),
            skill: skill(home.join(".claude")),
        },
        Agent {
            id: "codex",
            name: "Codex",
            home: codex_home.map_or_else(|| home.join(".codex"), Path::to_path_buf),
            skill: skill(home.join(".agents")),
        },
        Agent {
            id: "cursor",
            name: "Cursor",
            home: home.join(".cursor"),
            skill: skill(home.join(".agents")),
        },
    ]
}

/// Where the skill of the check `check_id` goes for `seams`' home: Codex's and Cursor's are
/// one file, which callers applying both install once.
pub fn skill_path(seams: &Seams, check_id: &str) -> Option<PathBuf> {
    let home = seams.home().ok()?;
    agents(home, seams.codex_home.as_deref())
        .into_iter()
        .find(|agent| agent.check_id() == check_id)
        .map(|agent| agent.skill)
}

/// What is at an agent's skill path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    Missing,
    /// There, but not this build's text.
    Differs,
    Same,
}

/// What is at `path` now.
pub fn installed(path: &Path) -> Result<Installed, String> {
    match std::fs::read(path) {
        Ok(bytes) if bytes == SKILL_MD.as_bytes() => Ok(Installed::Same),
        Ok(_) => Ok(Installed::Differs),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Installed::Missing),
        Err(error) => Err(format!("reading {}: {error}", path.display())),
    }
}

/// Writes this build's skill at `path` (module docs).
pub fn install(path: &Path) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("{} has no folder", path.display()))?;
    std::fs::create_dir_all(dir).map_err(|error| format!("creating {}: {error}", dir.display()))?;
    // A name of this write's own: two installs of one file (Codex's and Cursor's) never share
    // a temporary file.
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let write = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = dir.join(format!(".SKILL.md.{}.{write}.tmp", std::process::id()));
    std::fs::write(&temp, SKILL_MD)
        .and_then(|()| std::fs::rename(&temp, path))
        .map_err(|error| {
            let _ = std::fs::remove_file(&temp);
            format!("writing {}: {error}", path.display())
        })
}

/// One check per agent (module docs).
pub(super) fn checks(seams: &Seams) -> Vec<Check> {
    let home = match seams.home() {
        Ok(home) => home,
        Err(reason) => {
            return agents(Path::new("~"), None)
                .iter()
                .map(|agent| {
                    Check::new(
                        agent.check_id(),
                        title(agent),
                        State::Failed,
                        reason.clone(),
                    )
                })
                .collect();
        }
    };
    agents(home, seams.codex_home.as_deref())
        .iter()
        .map(check)
        .collect()
}

fn title(agent: &Agent) -> String {
    format!("dv-review skill for {}", agent.name)
}

fn check(agent: &Agent) -> Check {
    let title = title(agent);
    let id = agent.check_id();
    if !agent.present() {
        return Check::new(
            id,
            title,
            State::Skipped,
            format!(
                "{} isn't set up here, no folder: {}",
                agent.name,
                agent.home.display()
            ),
        );
    }
    let path = agent.skill.display();
    match installed(&agent.skill) {
        Ok(Installed::Same) => Check::new(id, title, State::Ok, format!("installed: {path}")),
        Ok(Installed::Missing) => {
            Check::new(id, title, State::Missing, format!("not installed: {path}"))
                .with_fix(Fix::apply("Install", "dv setup agents"))
        }
        Ok(Installed::Differs) => Check::new(
            id,
            title,
            State::OutOfDate,
            format!("out of date: differs from this dv's skill (an older dv's, or edited): {path}"),
        )
        .with_fix(Fix::apply("Update", "dv setup agents")),
        Err(reason) => Check::new(id, title, State::Failed, reason)
            .with_fix(Fix::apply("Install", "dv setup agents")),
    }
}

/// Installs the skill the check `check_id` names, when its agent is set up and its copy isn't
/// this build's; what was done.
pub(super) fn apply(seams: &Seams, check_id: &str) -> Result<String, String> {
    let home = seams.home()?;
    let agents = agents(home, seams.codex_home.as_deref());
    let agent = agents
        .iter()
        .find(|agent| agent.check_id() == check_id)
        .ok_or_else(|| format!("there is no check {check_id:?}"))?;
    if !agent.present() {
        return Err(format!(
            "{} isn't set up here (no {})",
            agent.name,
            agent.home.display()
        ));
    }
    let path = agent.skill.display();
    match installed(&agent.skill)? {
        Installed::Same => Ok(format!("the skill for {} is up to date", agent.name)),
        Installed::Missing => {
            install(&agent.skill)?;
            Ok(format!("installed the skill for {} at {path}", agent.name))
        }
        Installed::Differs => {
            install(&agent.skill)?;
            Ok(format!("updated the skill for {} at {path}", agent.name))
        }
    }
}
