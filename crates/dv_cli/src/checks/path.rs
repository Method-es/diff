//! The CLI on `PATH` (docs/PLAN.md §12.2 item 3), so an agent's shell finds `dv`: what `dv
//! setup path` sets up and `dv setup path --remove` takes back.
//!
//! - **Windows:** the `dv.exe` a terminal runs is the first one along the machine's `Path`,
//!   then the user's (both through the [`super::user_path`] seam). This executable's folder
//!   there first is ok; another folder's `dv.exe` ahead of it (an older install) fails, naming
//!   it. Adding puts the folder in the user `Path` before the first entry holding another
//!   `dv.exe`, else at the end, and tells running programs (`WM_SETTINGCHANGE`), so a terminal
//!   opened afterwards finds it; one in the machine's `Path` stays ahead (only an administrator
//!   changes that), which the check keeps saying. The installer's hooks call it for the
//!   installed copy's folder; `--remove` takes out exactly that folder.
//! - **Linux, and inside a WSL distro:** `~/.local/bin/dv` a link to this dv (inside a distro
//!   the link `dv setup wsl` makes already), and `~/.local/bin` on the user's shell's `PATH`, as
//!   their interactive login shell prints it (`dv_lsp::resolve::shell::read_here`); when it
//!   isn't, a line in the startup file that shell reads puts it there ([`RC_MARK`] heads it, so
//!   it is added once and `--remove` finds it): `~/.zshrc` for zsh; for bash the login file it
//!   reads (`~/.bash_profile`, else `~/.bash_login`, else `~/.profile`);
//!   `~/.config/fish/conf.d/dv.fish` for fish; else `~/.profile`. The shell is the one that
//!   printed the `PATH`, else `$SHELL`. After adding the line the shell is asked again.
//! - **macOS:** skipped. macOS is unverified (PLAN §13.2), and its usual places
//!   (`/usr/local/bin`) need an admin prompt; link dv into a folder on `PATH` by hand.

use std::path::{Path, PathBuf};

use super::user_path::{self, Resolved};
use super::{Check, Fix, Seams, State, id};

const TITLE: &str = "dv on PATH";

/// The comment that heads the line `dv setup path` adds to a shell's startup file.
pub const RC_MARK: &str = "# Added by `dv setup path`: dv's folder on PATH";

/// Whether `text` (a startup file) holds the line [`RC_MARK`] heads: detection and removal
/// match it the same way.
pub fn has_mark(text: &str) -> bool {
    text.lines().any(|line| line.trim() == RC_MARK)
}

/// What `dv setup path` did (or, with `--remove`, took back).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    /// The folder on `PATH`: this executable's (Windows), `~/.local/bin` (Linux).
    pub dir: PathBuf,
    /// `added`, `unchanged`, `removed` or `absent` (nothing to remove).
    pub action: &'static str,
    /// Linux: `~/.local/bin/dv`, which links to this dv (`None` on Windows).
    pub link: Option<PathBuf>,
    /// Linux: the shell startup file changed, if one was.
    pub rc_file: Option<PathBuf>,
    /// Windows: a folder in the machine's `Path` whose `dv.exe` still comes first.
    pub shadowed_by: Option<PathBuf>,
}

impl Done {
    /// What was done, as a line.
    pub fn line(&self) -> String {
        let dir = self.dir.display();
        let line = match (self.action, &self.rc_file) {
            ("added", Some(rc)) => format!(
                "put {dir} on PATH through {}; shells started from now on find dv",
                rc.display()
            ),
            ("added", None) if cfg!(windows) => {
                format!("put {dir} in your user Path; terminals opened from now on find dv")
            }
            ("added", None) => format!("{dir}/dv runs this dv now"),
            ("unchanged", _) => format!("{dir} is on PATH already"),
            ("removed", _) => format!("took {dir} off PATH"),
            _ => format!("{dir} wasn't on PATH through dv"),
        };
        match &self.shadowed_by {
            Some(by) => format!(
                "{line}; but {} in the system Path still comes first",
                by.join("dv.exe").display()
            ),
            None => line,
        }
    }
}

pub(super) fn check(seams: &Seams) -> Check {
    if cfg!(windows) {
        windows_check(seams)
    } else if cfg!(target_os = "macos") {
        Check::new(id::PATH, TITLE, State::Skipped, MACOS)
    } else {
        unix::check(seams)
    }
}

/// `dv setup path`: puts dv on `PATH` here (module docs); `dir` stands in for this executable's
/// folder on Windows (tests, the installer's own copy).
pub fn add_dir(seams: &Seams, dir: Option<&Path>) -> Result<Done, String> {
    if cfg!(windows) {
        let dir = windows_dir(seams, dir)?;
        let value = seams.user_path.read()?;
        let vars = |name: &str| seams.var(name);
        let action = match user_path::with_added(value.as_deref(), &dir, &vars, &has_dv) {
            None => "unchanged",
            Some(value) => {
                seams.user_path.write(&value)?;
                "added"
            }
        };
        let shadowed_by = match resolve(seams, &dir)? {
            Resolved::Shadowed {
                by, machine: true, ..
            } => Some(by),
            _ => None,
        };
        Ok(Done {
            dir,
            action,
            link: None,
            rc_file: None,
            shadowed_by,
        })
    } else if cfg!(target_os = "macos") {
        Err(MACOS.to_string())
    } else {
        unix::add(seams)
    }
}

/// [`add_dir`] for this executable.
pub(super) fn add(seams: &Seams) -> Result<Done, String> {
    add_dir(seams, None)
}

/// `dv setup path --remove`: takes back what [`add_dir`] did, exactly.
pub fn remove_dir(seams: &Seams, dir: Option<&Path>) -> Result<Done, String> {
    if cfg!(windows) {
        let dir = windows_dir(seams, dir)?;
        let value = seams.user_path.read()?.unwrap_or_default();
        let vars = |name: &str| seams.var(name);
        let action = match user_path::with_removed(&value, &dir, &vars) {
            None => "absent",
            Some(value) => {
                seams.user_path.write(&value)?;
                "removed"
            }
        };
        Ok(Done {
            dir,
            action,
            link: None,
            rc_file: None,
            shadowed_by: None,
        })
    } else if cfg!(target_os = "macos") {
        Err(MACOS.to_string())
    } else {
        unix::remove(seams)
    }
}

/// What a plan says it will do, before it does it.
pub fn planned(seams: &Seams, dir: Option<&Path>, remove: bool) -> Result<Option<String>, String> {
    if cfg!(windows) {
        let dir = windows_dir(seams, dir)?;
        let value = seams.user_path.read()?;
        let vars = |name: &str| seams.var(name);
        let place = seams.user_path.describe();
        Ok(if remove {
            user_path::contains(value.as_deref().unwrap_or(""), &dir, &vars)
                .then(|| format!("remove {} from {place}", dir.display()))
        } else {
            user_path::with_added(value.as_deref(), &dir, &vars, &has_dv)
                .map(|_| format!("put {} in {place}, ahead of any other dv", dir.display()))
        })
    } else if cfg!(target_os = "macos") {
        Err(MACOS.to_string())
    } else {
        unix::planned(seams, remove)
    }
}

const MACOS: &str = "dv doesn't set up PATH on macOS yet: link dv into a folder on your PATH";

fn windows_dir(seams: &Seams, dir: Option<&Path>) -> Result<PathBuf, String> {
    dir.or(seams.exe_dir())
        .map(Path::to_path_buf)
        .ok_or_else(|| "can't tell which folder this dv is in".to_string())
}

/// Whether `folder` holds a `dv.exe`.
fn has_dv(folder: &Path) -> bool {
    folder.join("dv.exe").is_file()
}

/// Which `dv.exe` a terminal runs, for this executable's folder `dir`.
fn resolve(seams: &Seams, dir: &Path) -> Result<Resolved, String> {
    let machine = seams.user_path.read_machine()?.unwrap_or_default();
    let user = seams.user_path.read()?.unwrap_or_default();
    let vars = |name: &str| seams.var(name);
    Ok(user_path::resolve(&machine, &user, dir, &vars, &has_dv))
}

fn windows_check(seams: &Seams) -> Check {
    let dir = match windows_dir(seams, None) {
        Ok(dir) => dir,
        Err(reason) => return Check::new(id::PATH, TITLE, State::Failed, reason),
    };
    let place = seams.user_path.describe();
    match resolve(seams, &dir) {
        Err(reason) => Check::new(id::PATH, TITLE, State::Failed, reason),
        Ok(Resolved::Ours) => Check::new(
            id::PATH,
            TITLE,
            State::Ok,
            format!("first on PATH, in {place}: {}", dir.display()),
        ),
        Ok(Resolved::Absent) => Check::new(
            id::PATH,
            TITLE,
            State::Missing,
            format!(
                "not in {place}, so a terminal doesn't find this dv: {}",
                dir.display()
            ),
        )
        .with_fix(Fix::apply("Add to PATH", "dv setup path")),
        Ok(Resolved::Shadowed {
            by, machine: false, ..
        }) => Check::new(
            id::PATH,
            TITLE,
            State::Failed,
            format!(
                "another dv comes first on PATH, so a terminal runs it, not this one: {}",
                by.join("dv.exe").display()
            ),
        )
        .with_fix(Fix::apply("Put this dv first", "dv setup path")),
        Ok(Resolved::Shadowed {
            by, machine: true, ..
        }) => Check::new(
            id::PATH,
            TITLE,
            State::Failed,
            format!(
                "another dv in the system Path comes first, so a terminal runs it; take its \
                 folder out of the system Path (an administrator can): {}",
                by.join("dv.exe").display()
            ),
        ),
    }
}

#[cfg(unix)]
mod unix {
    //! Linux and WSL distros (module docs).

    use std::io::Write as _;
    use std::path::{Path, PathBuf};

    use dv_lsp::resolve::PathFrom;

    use super::{Check, Done, Fix, RC_MARK, Seams, State, TITLE, has_mark, id};

    /// What `~/.local/bin` and the user's shell look like now.
    struct Look {
        bin: PathBuf,
        link: PathBuf,
        /// `~/.local/bin/dv` runs this dv.
        linked: bool,
        /// `~/.local/bin` is on the shell's `PATH`.
        on_path: bool,
        /// The shell printed its `PATH` (else `on_path` can't be told).
        asked: bool,
        /// The shell's startup file a line goes in.
        rc: PathBuf,
        /// Whether [`RC_MARK`]'s line is in it already.
        rc_has_line: bool,
    }

    fn look(seams: &Seams) -> Result<Look, String> {
        let home = seams.home()?;
        let exe = seams
            .exe
            .as_deref()
            .ok_or_else(|| "can't tell where this dv is".to_string())?;
        let bin = home.join(".local").join("bin");
        let link = bin.join("dv");
        let linked = match (std::fs::canonicalize(&link), std::fs::canonicalize(exe)) {
            (Ok(target), Ok(exe)) => target == exe,
            _ => false,
        };
        let shell = dv_lsp::resolve::shell::read_here(&[(
            "HOME".to_string(),
            Some(home.display().to_string()),
        )]);
        let wanted = bin.display().to_string();
        let on_path = shell
            .path()
            .iter()
            .any(|dir| dir.trim_end_matches('/') == wanted);
        let (asked, shell_path) = match shell.from() {
            PathFrom::Shell { shell, .. } => (true, Some(shell.clone())),
            _ => (false, seams.var("SHELL")),
        };
        let shell_name = shell_path
            .as_deref()
            .and_then(|path| path.rsplit('/').next())
            .unwrap_or_default()
            .to_string();
        let rc = rc_file(home, &shell_name);
        let rc_has_line = std::fs::read_to_string(&rc).is_ok_and(|text| has_mark(&text));
        Ok(Look {
            bin,
            link,
            linked,
            on_path,
            asked,
            rc,
            rc_has_line,
        })
    }

    /// The startup file the shell named `shell` reads as an interactive login shell (module
    /// docs).
    pub(super) fn rc_file(home: &Path, shell: &str) -> PathBuf {
        match shell {
            "zsh" => home.join(".zshrc"),
            "bash" => [".bash_profile", ".bash_login"]
                .iter()
                .map(|name| home.join(name))
                .find(|file| file.is_file())
                .unwrap_or_else(|| home.join(".profile")),
            "fish" => home
                .join(".config")
                .join("fish")
                .join("conf.d")
                .join("dv.fish"),
            _ => home.join(".profile"),
        }
    }

    /// The line that puts `~/.local/bin` on `PATH` in `rc`, after [`RC_MARK`].
    fn rc_line(rc: &Path) -> &'static str {
        if rc.extension().is_some_and(|ext| ext == "fish") {
            "fish_add_path -g \"$HOME/.local/bin\""
        } else {
            "export PATH=\"$HOME/.local/bin:$PATH\""
        }
    }

    pub(super) fn check(seams: &Seams) -> Check {
        let look = match look(seams) {
            Ok(look) => look,
            Err(reason) => return Check::new(id::PATH, TITLE, State::Failed, reason),
        };
        let fix = Fix::apply("Add to PATH", "dv setup path");
        let link = look.link.display();
        let bin = look.bin.display();
        let rc = look.rc.display();
        match (look.linked, look.on_path) {
            (true, true) => Check::new(id::PATH, TITLE, State::Ok, format!("{link} runs this dv")),
            (false, _) => Check::new(
                id::PATH,
                TITLE,
                State::Missing,
                format!("doesn't run this dv: {link}"),
            )
            .with_fix(fix),
            (true, false) if look.rc_has_line && !look.asked => Check::new(
                id::PATH,
                TITLE,
                State::Ok,
                format!(
                    "{link} runs this dv, and {rc} puts {bin} on PATH (the shell couldn't be asked)"
                ),
            ),
            (true, false) if look.rc_has_line => Check::new(
                id::PATH,
                TITLE,
                State::Failed,
                format!("dv's line is in {rc}, but your shell's PATH still lacks {bin}"),
            ),
            (true, false) => Check::new(
                id::PATH,
                TITLE,
                State::Missing,
                format!("not on your shell's PATH: {bin}"),
            )
            .with_fix(fix),
        }
    }

    pub(super) fn planned(seams: &Seams, remove: bool) -> Result<Option<String>, String> {
        let look = look(seams)?;
        let mut steps = Vec::new();
        if remove {
            if look.linked {
                steps.push(format!("remove {}", look.link.display()));
            }
            if look.rc_has_line {
                steps.push(format!("take dv's line out of {}", look.rc.display()));
            }
        } else {
            if !look.linked {
                steps.push(format!("link {} to this dv", look.link.display()));
            }
            if !look.on_path && !look.rc_has_line {
                steps.push(format!(
                    "add a line to {} putting {} on PATH",
                    look.rc.display(),
                    look.bin.display()
                ));
            }
        }
        Ok((!steps.is_empty()).then(|| steps.join(", then ")))
    }

    pub(super) fn add(seams: &Seams) -> Result<Done, String> {
        let look = look(seams)?;
        let exe = seams
            .exe
            .as_deref()
            .ok_or_else(|| "can't tell where this dv is".to_string())?;
        let mut changed = false;
        if !look.linked {
            link(&look.link, exe)?;
            changed = true;
        }
        let mut rc_file = None;
        if !look.on_path && !look.rc_has_line {
            append_line(&look.rc)?;
            rc_file = Some(look.rc.clone());
            changed = true;
            // Asked again: the line must be what the shell reads.
            let again = self::look(seams)?;
            if again.asked && !again.on_path {
                return Err(format!(
                    "added dv's line to {}, but your shell's PATH still lacks {}",
                    look.rc.display(),
                    look.bin.display()
                ));
            }
        }
        Ok(Done {
            dir: look.bin,
            action: if changed { "added" } else { "unchanged" },
            link: Some(look.link),
            rc_file,
            shadowed_by: None,
        })
    }

    pub(super) fn remove(seams: &Seams) -> Result<Done, String> {
        let look = look(seams)?;
        let mut changed = false;
        if look.linked {
            std::fs::remove_file(&look.link)
                .map_err(|error| format!("removing {}: {error}", look.link.display()))?;
            changed = true;
        }
        let mut rc_file = None;
        if look.rc_has_line {
            remove_line(&look.rc)?;
            rc_file = Some(look.rc.clone());
            changed = true;
        }
        Ok(Done {
            dir: look.bin,
            action: if changed { "removed" } else { "absent" },
            link: Some(look.link),
            rc_file,
            shadowed_by: None,
        })
    }

    /// Points `link` at `exe`: a temporary link renamed over whatever is there.
    fn link(link: &Path, exe: &Path) -> Result<(), String> {
        let dir = link.parent().unwrap_or(Path::new("/"));
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("creating {}: {error}", dir.display()))?;
        if link.is_dir() {
            return Err(format!(
                "{} is a directory, so it can't run dv: remove it first",
                link.display()
            ));
        }
        let exe = std::fs::canonicalize(exe).unwrap_or_else(|_| exe.to_path_buf());
        let temp = dir.join(format!(".dv.{}", std::process::id()));
        let _ = std::fs::remove_file(&temp);
        std::os::unix::fs::symlink(&exe, &temp)
            .and_then(|()| std::fs::rename(&temp, link))
            .map_err(|error| {
                let _ = std::fs::remove_file(&temp);
                format!("linking {} to {}: {error}", link.display(), exe.display())
            })
    }

    /// Appends [`RC_MARK`] and the line after it to `rc`, a blank line before them.
    pub(super) fn append_line(rc: &Path) -> Result<(), String> {
        if let Some(dir) = rc.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|error| format!("creating {}: {error}", dir.display()))?;
        }
        let existing = std::fs::read_to_string(rc).unwrap_or_default();
        let lead = if existing.is_empty() || existing.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(rc)
            .and_then(|mut file| write!(file, "{lead}\n{RC_MARK}\n{}\n", rc_line(rc)))
            .map_err(|error| format!("writing {}: {error}", rc.display()))
    }

    /// Takes [`RC_MARK`]'s line (matched as [`has_mark`] matches it) and the line after it out
    /// of `rc` (a blank line before it too), and removes a file that held nothing else.
    pub(super) fn remove_line(rc: &Path) -> Result<(), String> {
        let text = std::fs::read_to_string(rc)
            .map_err(|error| format!("reading {}: {error}", rc.display()))?;
        let lines: Vec<&str> = text.lines().collect();
        let mut kept: Vec<&str> = Vec::new();
        let mut index = 0;
        while index < lines.len() {
            if lines[index].trim() == RC_MARK {
                if kept.last().is_some_and(|line| line.trim().is_empty()) {
                    kept.pop();
                }
                index += 2;
                continue;
            }
            kept.push(lines[index]);
            index += 1;
        }
        if kept.iter().all(|line| line.trim().is_empty()) {
            return std::fs::remove_file(rc)
                .map_err(|error| format!("removing {}: {error}", rc.display()));
        }
        let mut out = kept.join("\n");
        out.push('\n');
        std::fs::write(rc, out).map_err(|error| format!("writing {}: {error}", rc.display()))
    }

    #[cfg(test)]
    mod tests {
        use pretty_assertions::assert_eq;

        use super::*;

        #[test]
        fn each_shell_gets_the_file_it_reads() {
            let home = tempfile::tempdir().unwrap();
            let home = home.path();
            assert_eq!(rc_file(home, "zsh"), home.join(".zshrc"));
            assert_eq!(rc_file(home, "bash"), home.join(".profile"));
            std::fs::write(home.join(".bash_profile"), "").unwrap();
            assert_eq!(rc_file(home, "bash"), home.join(".bash_profile"));
            assert_eq!(
                rc_file(home, "fish"),
                home.join(".config/fish/conf.d/dv.fish")
            );
            assert_eq!(rc_file(home, ""), home.join(".profile"));
        }

        #[test]
        fn the_line_goes_in_and_comes_out_for_zsh_and_fish() {
            let home = tempfile::tempdir().unwrap();
            let zshrc = home.path().join(".zshrc");
            std::fs::write(&zshrc, "alias ll='ls -l'").unwrap();
            append_line(&zshrc).unwrap();
            let text = std::fs::read_to_string(&zshrc).unwrap();
            assert!(has_mark(&text), "{text}");
            assert!(
                text.ends_with("export PATH=\"$HOME/.local/bin:$PATH\"\n"),
                "{text}"
            );
            remove_line(&zshrc).unwrap();
            assert_eq!(
                std::fs::read_to_string(&zshrc).unwrap(),
                "alias ll='ls -l'\n"
            );

            // fish: a file of dv's own, gone once the line is.
            let fish = rc_file(home.path(), "fish");
            append_line(&fish).unwrap();
            let text = std::fs::read_to_string(&fish).unwrap();
            assert!(has_mark(&text), "{text}");
            assert!(
                text.contains("fish_add_path -g \"$HOME/.local/bin\""),
                "{text}"
            );
            remove_line(&fish).unwrap();
            assert!(!fish.exists());
            // The mark indented is still dv's.
            assert!(has_mark(&format!("  {RC_MARK}  \n")));
        }
    }
}

#[cfg(not(unix))]
mod unix {
    //! Linux's half has no meaning off Unix; Windows has its own above.

    use super::{Check, Done, Seams, State, TITLE, id};

    pub(super) fn check(_seams: &Seams) -> Check {
        Check::new(id::PATH, TITLE, State::Skipped, "not on this platform")
    }

    pub(super) fn planned(_seams: &Seams, _remove: bool) -> Result<Option<String>, String> {
        Err("not on this platform".to_string())
    }

    pub(super) fn add(_seams: &Seams) -> Result<Done, String> {
        Err("not on this platform".to_string())
    }

    pub(super) fn remove(_seams: &Seams) -> Result<Done, String> {
        Err("not on this platform".to_string())
    }
}
