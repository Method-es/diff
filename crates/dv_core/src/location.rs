//! Where a repository lives: a path on this machine, or a path inside a WSL distro
//! (docs/PLAN.md §2.1, §10.1).
//!
//! A [`Location`] is what the user names (`--repo`, `--wsl`, a pasted path) and what a review
//! records. A local one opens as a [`LocalBackend`](crate::backend::LocalBackend); a WSL one
//! only through the distro's own dv (PLAN D6): the app opens it over a connection to `dv
//! backend --stdio` there (`dv_ui`'s `Locations`, PLAN §5.1), and the CLI on Windows runs the
//! whole command there (PLAN §10.4).
//!
//! WSL mounts each Windows drive at its default automount root, `/mnt/<letter>`:
//! [`drive_path`] spells such a POSIX path for Windows (`/mnt/d/code` → `D:\code`), and
//! [`mount_path`] a drive's path for the distro (`D:\code` → `/mnt/d/code`), so a path given on
//! the other side of the boundary still names its folder (PLAN §10.1).

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A repository's location.
///
/// Serialised externally tagged in snake_case — `{"local": "D:\\code\\api"}`,
/// `{"wsl": {"distro": "Ubuntu", "path": "/home/kyle/api"}}` — a stable shape, because reviews
/// record locations in their targets (PLAN §7).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Location {
    /// A path on this machine's filesystem, as given: it may be relative or point anywhere inside
    /// the work tree ([`crate::backend::LocalBackend::open`] resolves the root).
    Local(PathBuf),
    /// A path inside a WSL distro. `path` is an absolute POSIX path (`/home/kyle/api`), never a
    /// UNC path.
    Wsl { distro: String, path: String },
}

/// Why a path or `--wsl` argument isn't a [`Location`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LocationError {
    #[error("the repository path is empty")]
    Empty,
    #[error("{arg} names no WSL distro (expected \\\\wsl.localhost\\<distro>\\<path>)")]
    MissingDistro { arg: String },
    #[error("{distro:?} is not a WSL distro name")]
    InvalidDistro { distro: String },
    #[error("invalid WSL location {arg:?} (expected <distro>:/absolute/path)")]
    InvalidWslArg { arg: String },
}

/// The UNC hosts through which Windows exposes WSL distros' filesystems
/// (`\\wsl.localhost\<distro>\…`, and the older `\\wsl$\<distro>\…`).
const WSL_UNC_HOSTS: [&str; 2] = ["wsl.localhost", "wsl$"];

impl Location {
    /// Interprets a path argument (`--repo`, `dv open <path>`, a pasted path).
    ///
    /// `\\wsl.localhost\<distro>\<path>` and `\\wsl$\<distro>\<path>` — host matched
    /// case-insensitively, either slash, also behind the `\\?\UNC\` verbatim prefix — become
    /// [`Location::Wsl`] with `<path>` as an absolute POSIX path (`/` when empty). Everything
    /// else, other UNC paths included, is [`Location::Local`] verbatim.
    pub fn parse(arg: &str) -> Result<Self, LocationError> {
        if arg.is_empty() {
            return Err(LocationError::Empty);
        }
        let Some(rest) = strip_wsl_unc(arg) else {
            return Ok(Location::Local(PathBuf::from(arg)));
        };
        let (distro, path) = rest.split_once(['\\', '/']).unwrap_or((rest, ""));
        if distro.is_empty() {
            return Err(LocationError::MissingDistro {
                arg: arg.to_string(),
            });
        }
        validate_distro(distro)?;
        Ok(Location::Wsl {
            distro: distro.to_string(),
            path: normalize_posix(path),
        })
    }

    /// Interprets a `--wsl <distro>:<path>` argument such as `Ubuntu:/home/kyle/api`; the path
    /// must be absolute.
    pub fn parse_wsl(arg: &str) -> Result<Self, LocationError> {
        let invalid = || LocationError::InvalidWslArg {
            arg: arg.to_string(),
        };
        let (distro, path) = arg.split_once(':').ok_or_else(invalid)?;
        if distro.is_empty() || !path.starts_with('/') {
            return Err(invalid());
        }
        validate_distro(distro)?;
        Ok(Location::Wsl {
            distro: distro.to_string(),
            path: normalize_posix(path),
        })
    }

    /// The full name for titles and messages: `D:\code\api`, `Ubuntu:/home/kyle/api`.
    pub fn display_name(&self) -> String {
        match self {
            Location::Local(path) => path.display().to_string(),
            Location::Wsl { distro, path } => format!("{distro}:{path}"),
        }
    }

    /// The Windows spelling of a WSL location's path, through the share Windows exposes the
    /// distro's filesystem on: `\\wsl.localhost\Ubuntu\home\kyle\api` (`\\wsl.localhost\Ubuntu\`
    /// for its root). `None` for a local path, which is already spelled for this machine.
    pub fn unc_path(&self) -> Option<String> {
        match self {
            Location::Local(_) => None,
            Location::Wsl { distro, path } => {
                let rest = path.trim_start_matches('/').replace('/', "\\");
                Some(format!(r"\\wsl.localhost\{distro}\{rest}"))
            }
        }
    }

    /// The file `rel` (a `/`-separated path inside the repository whose worktree root this
    /// location is) as this machine opens it, for "Reveal in folder": under a local root, or
    /// through the share Windows exposes a distro's files on for a WSL one
    /// (`\\wsl.localhost\Ubuntu\home\kyle\api\src\main.rs`, [`Location::unc_path`]). Never a
    /// POSIX path read as one of this machine's.
    pub fn file_path(&self, rel: &str) -> PathBuf {
        let root = match self {
            Location::Local(path) => path.clone(),
            Location::Wsl { .. } => PathBuf::from(self.unc_path().unwrap_or_default()),
        };
        rel.split('/')
            .filter(|part| !part.is_empty())
            .fold(root, |at, part| at.join(part))
    }

    /// Whether paths in this repository compare without case: a local one's on Windows and
    /// macOS, whose file systems usually ignore it; never a WSL one's, on the distro's Linux
    /// file system, wherever the app runs.
    pub fn folds_case(&self) -> bool {
        match self {
            Location::Local(_) => cfg!(any(windows, target_os = "macos")),
            Location::Wsl { .. } => false,
        }
    }

    /// Whether `self` and `other` name the same place: equal, or WSL locations whose distros
    /// differ only in case (`wsl.exe` takes distro names without case, so `ubuntu:/srv/x` is
    /// `Ubuntu:/srv/x`). What a list of repositories compares where WSL's own spelling of the
    /// distro isn't known.
    pub fn same_place(&self, other: &Location) -> bool {
        match (self, other) {
            (
                Location::Wsl { distro, path },
                Location::Wsl {
                    distro: other_distro,
                    path: other_path,
                },
            ) => path == other_path && distro.to_lowercase() == other_distro.to_lowercase(),
            _ => self == other,
        }
    }

    /// The short name for lists: the last path component (`api`); the distro name for a distro's
    /// root, the whole path when it has no last component (`D:\`, `.`).
    pub fn short_name(&self) -> String {
        match self {
            Location::Local(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            Location::Wsl { distro, path } => match path.rsplit('/').next() {
                Some(last) if !last.is_empty() => last.to_string(),
                _ => distro.clone(),
            },
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display_name())
    }
}

/// `path`, an absolute POSIX path, as the Windows path of the drive it is on when it is under
/// WSL's default automount root (`/mnt/<letter>`, module docs): `/mnt/d/code` → `D:\code`,
/// `/mnt/c` → `C:\`. `None` for any other path. The inverse of [`mount_path`].
pub fn drive_path(path: &str) -> Option<String> {
    let rest = path.strip_prefix("/mnt/")?;
    let (letter, rest) = rest.split_once('/').unwrap_or((rest, ""));
    let drive = drive_letter(letter)?;
    let parts: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();
    Some(format!(
        r"{}:\{}",
        drive.to_ascii_uppercase(),
        parts.join(r"\")
    ))
}

/// `path`, a Windows drive's path (`D:\code\api`, `d:/code/api`, `D:`), as WSL mounts it at its
/// default automount root (module docs): `/mnt/d/code/api`, `/mnt/d` for the drive's root.
/// Either slash separates, and empty and `.` components go (`..` stays: only the file system
/// resolves it). `None` for anything else: a relative or POSIX path, a share, a drive-relative
/// `D:code`. The inverse of [`drive_path`].
pub fn mount_path(path: &str) -> Option<String> {
    let (head, rest) = path.split_at_checked(2)?;
    let drive = drive_letter(head.strip_suffix(':')?)?;
    if !(rest.is_empty() || rest.starts_with(['\\', '/'])) {
        return None;
    }
    let mut mounted = format!("/mnt/{}", drive.to_ascii_lowercase());
    for part in rest
        .split(['\\', '/'])
        .filter(|part| !part.is_empty() && *part != ".")
    {
        mounted.push('/');
        mounted.push_str(part);
    }
    Some(mounted)
}

/// The drive `letter` names: one ASCII letter.
fn drive_letter(letter: &str) -> Option<char> {
    let mut chars = letter.chars();
    match (chars.next(), chars.next()) {
        (Some(drive), None) if drive.is_ascii_alphabetic() => Some(drive),
        _ => None,
    }
}

/// The text after `\\<wsl host>\` when `arg` is a WSL UNC path, else `None`. A bare
/// `\\wsl.localhost` yields `Some("")`, which [`Location::parse`] reports as a missing distro.
fn strip_wsl_unc(arg: &str) -> Option<&str> {
    let is_sep = |b: u8| b == b'\\' || b == b'/';
    let bytes = arg.as_bytes();
    if bytes.len() < 2 || !is_sep(bytes[0]) || !is_sep(bytes[1]) {
        return None;
    }
    // `\\?\UNC\wsl.localhost\…` is the verbatim spelling Windows APIs (canonicalize) produce.
    let rest = strip_prefix_ignore_ascii_case(&arg[2..], "?\\UNC\\").unwrap_or(&arg[2..]);
    WSL_UNC_HOSTS.iter().find_map(|host| {
        let after = strip_prefix_ignore_ascii_case(rest, host)?;
        match after.as_bytes().first() {
            None => Some(""),
            // The separator is ASCII, so `1..` is a char boundary.
            Some(&b) if is_sep(b) => Some(&after[1..]),
            Some(_) => None,
        }
    })
}

/// `text` without `prefix` (an ASCII string) when it starts with it, ignoring ASCII case.
fn strip_prefix_ignore_ascii_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.as_bytes().get(..prefix.len())?;
    // `prefix` is ASCII, so a match ends on a char boundary.
    head.eq_ignore_ascii_case(prefix.as_bytes())
        .then(|| &text[prefix.len()..])
}

/// Rejects names that can't be a distro and could confuse the `wsl.exe -d <distro>` command line
/// the in-distro backend is started with (PLAN D6): empty, separators, `:`, whitespace or control
/// characters, a leading `-`.
fn validate_distro(distro: &str) -> Result<(), LocationError> {
    let bad_char = |c: char| matches!(c, '\\' | '/' | ':') || c.is_whitespace() || c.is_control();
    if distro.is_empty() || distro.starts_with('-') || distro.chars().any(bad_char) {
        return Err(LocationError::InvalidDistro {
            distro: distro.to_string(),
        });
    }
    Ok(())
}

/// An absolute POSIX path from the components of `path` split on either slash, without empty and
/// `.` components; `..` stays, because only the distro can resolve it (symlinks).
fn normalize_posix(path: &str) -> String {
    let parts: Vec<&str> = path
        .split(['\\', '/'])
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    format!("/{}", parts.join("/"))
}

#[cfg(test)]
mod tests;
