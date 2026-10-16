//! The machine a backend runs on, as the `PATH` and JavaScript resolvers read it (docs/PLAN.md
//! §18.4): its `PATH`, its user's home, and where bun installs.
//!
//! On this machine that is this process's environment ([`HostEnv::this_process`]). In a WSL
//! distro the backend runs under `wsl.exe --exec`, without the user's profile, which is where
//! bun, nvm and the like add their folders, so the app reads the user's shell instead, once per
//! connection ([`super::shell`]).
//!
//! A distro's `PATH` also carries the Windows folders WSL appends (`/mnt/c/…`), whose programs
//! WSL interop would run as Windows executables; every entry under `/mnt/<drive>` is left out.

use dv_core::backend::{BackendError, PathProbe};

use super::node::{AbsPath, Reader, RealPath, real_path};
use crate::paths::PathStyle;

/// Where a [`HostEnv`]'s `PATH` came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathFrom {
    /// This process's environment.
    Process,
    /// The user's shell printed it, run with these flags (`-l -i`, `-i` or `-l`) and `-c`.
    Shell { shell: String, flags: String },
    /// The backend's own environment: the shell couldn't say.
    Backend,
    /// No `PATH` is known: nothing is found on it.
    Unknown,
}

/// What the `PATH` and JavaScript resolvers know of the backend's machine (module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEnv {
    style: PathStyle,
    /// `PATH`'s absolute folders in order, each once, `/mnt/<drive>` left out.
    path: Vec<String>,
    /// `PATH` as it was given, every entry kept: what the servers run with, so a shim on it
    /// (asdf's `exec asdf exec node`, pnpm's `exec node …`) finds what it calls.
    raw_path: Option<String>,
    /// The user's home.
    home: Option<String>,
    /// `BUN_INSTALL`, where bun is installed when not `~/.bun`.
    bun_install: Option<String>,
    from: PathFrom,
}

impl HostEnv {
    /// A machine spelling paths in `style` whose `PATH` is `path` (`:`-separated on POSIX,
    /// `;` on Windows), its home `home` and bun's folder `bun_install`, as `from` gave them.
    pub fn new(
        style: PathStyle,
        path: Option<&str>,
        home: Option<&str>,
        bun_install: Option<&str>,
        from: PathFrom,
    ) -> Self {
        let separator = match style {
            PathStyle::Posix => ':',
            PathStyle::Windows => ';',
        };
        let mut dirs: Vec<String> = Vec::new();
        for entry in path.unwrap_or("").split(separator) {
            let entry = entry.trim().trim_matches('"');
            if !super::is_absolute(entry) || (style == PathStyle::Posix && under_mnt_drive(entry)) {
                continue;
            }
            let entry = entry.to_string();
            if !dirs.contains(&entry) {
                dirs.push(entry);
            }
        }
        let absolute = |value: Option<&str>| {
            value
                .map(str::trim)
                .filter(|value| super::is_absolute(value))
                .map(str::to_string)
        };
        Self {
            style,
            raw_path: path
                .map(str::to_string)
                .filter(|path| !path.trim().is_empty()),
            path: dirs,
            home: absolute(home),
            bun_install: absolute(bun_install),
            from,
        }
    }

    /// This process's environment: `PATH`, the home (`USERPROFILE` on Windows, else `HOME`)
    /// and `BUN_INSTALL`.
    pub fn this_process() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        let (style, home) = if cfg!(windows) {
            (
                PathStyle::Windows,
                var("USERPROFILE").or_else(|| var("HOME")),
            )
        } else {
            (PathStyle::Posix, var("HOME"))
        };
        Self::new(
            style,
            var("PATH").as_deref(),
            home.as_deref(),
            var("BUN_INSTALL").as_deref(),
            PathFrom::Process,
        )
    }

    /// Nothing known: no `PATH`, no home.
    pub fn unknown(style: PathStyle) -> Self {
        Self::new(style, None, None, None, PathFrom::Unknown)
    }

    pub fn path(&self) -> &[String] {
        &self.path
    }

    /// `PATH` as given, unfiltered: the environment every server of this machine gets.
    pub fn raw_path(&self) -> Option<&str> {
        self.raw_path.as_deref()
    }

    pub fn home(&self) -> Option<&str> {
        self.home.as_deref()
    }

    pub fn from(&self) -> &PathFrom {
        &self.from
    }

    pub fn style(&self) -> PathStyle {
        self.style
    }

    /// bun's folder: `BUN_INSTALL`, else `~/.bun`.
    pub fn bun_dir(&self) -> Option<AbsPath> {
        match &self.bun_install {
            Some(dir) => AbsPath::parse(dir, self.style),
            None => AbsPath::parse(self.home.as_deref()?, self.style).map(|home| home.join(".bun")),
        }
    }

    /// Where `PATH` came from, as a missing server's reason says it.
    pub fn path_label(&self) -> String {
        match &self.from {
            PathFrom::Process => "PATH".to_string(),
            PathFrom::Shell { shell, flags } => format!("the PATH of {shell} {flags}"),
            PathFrom::Backend => "the backend's PATH".to_string(),
            PathFrom::Unknown => "PATH (unknown)".to_string(),
        }
    }

    /// The executable named `name` on `PATH` (`name.exe` on Windows: a batch file, which only
    /// `cmd.exe` runs, never counts), the first folder that has one.
    pub fn find(&self, reader: &Reader<'_>, name: &str) -> Result<Option<PathHit>, BackendError> {
        let file = match self.style {
            PathStyle::Windows => format!("{name}.exe"),
            PathStyle::Posix => name.to_string(),
        };
        self.find_file(reader, &file)
    }

    /// The file `file` (its exact name) in the first folder of `PATH` that has one.
    pub fn find_file(
        &self,
        reader: &Reader<'_>,
        file: &str,
    ) -> Result<Option<PathHit>, BackendError> {
        for dir in &self.path {
            let Some(dir) = AbsPath::parse(dir, self.style) else {
                continue;
            };
            let at = dir.join(file);
            if let Some(real) = real_file(reader, &at)? {
                return Ok(Some(PathHit { at, real }));
            }
        }
        Ok(None)
    }
}

/// A file found on `PATH`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathHit {
    /// Where `PATH` has it (a link, maybe).
    pub at: AbsPath,
    /// Where it really is, every link resolved.
    pub real: AbsPath,
}

/// Where the file at `path` really is, when it is a file (links followed); `None` for nothing,
/// a directory, or a way there dv can't read.
pub fn real_file(reader: &Reader<'_>, path: &AbsPath) -> Result<Option<AbsPath>, BackendError> {
    let RealPath::Found(real) = real_path(reader, path)? else {
        return Ok(None);
    };
    Ok(matches!(reader.probe(&real)?, PathProbe::File { .. }).then_some(real))
}

/// Whether a POSIX path is `/mnt/<drive>` or under it: a Windows drive in WSL.
fn under_mnt_drive(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/mnt/") else {
        return false;
    };
    let drive = rest.split('/').next().unwrap_or("");
    drive.len() == 1 && drive.chars().all(|ch| ch.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_distro_path_leaves_windows_drives_out() {
        let env = HostEnv::new(
            PathStyle::Posix,
            Some(
                "/home/kyle/.bun/bin:relative/bin::/usr/bin:/mnt/c/Users/x/.bun/bin:\
                 /mnt/d/Software/diff/dist:/mnt/wsl/bin:/mnt/c:/usr/bin:/Docker/host/bin",
            ),
            Some("/home/kyle"),
            None,
            PathFrom::Shell {
                shell: "/usr/bin/zsh".into(),
                flags: "-l -i".into(),
            },
        );
        assert_eq!(
            env.path(),
            [
                "/home/kyle/.bun/bin",
                "/usr/bin",
                "/mnt/wsl/bin",
                "/Docker/host/bin"
            ]
        );
        assert_eq!(
            env.bun_dir().map(|dir| dir.native()),
            Some("/home/kyle/.bun".to_string())
        );
        assert_eq!(env.path_label(), "the PATH of /usr/bin/zsh -l -i");
    }

    #[test]
    fn a_windows_path_splits_on_semicolons() {
        let env = HostEnv::new(
            PathStyle::Windows,
            Some(r#"C:\Windows;"C:\Program Files\nodejs";%APPDATA%\npm;;C:\Windows"#),
            Some(r"C:\Users\kyle"),
            Some(r"D:\bun"),
            PathFrom::Process,
        );
        assert_eq!(env.path(), [r"C:\Windows", r"C:\Program Files\nodejs"]);
        assert_eq!(
            env.bun_dir().map(|dir| dir.native()),
            Some(r"D:\bun".to_string())
        );
        assert_eq!(HostEnv::unknown(PathStyle::Posix).bun_dir(), None);
    }
}
