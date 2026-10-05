//! Where a repository lives: a path on this machine, or a path inside a WSL distro
//! (docs/PLAN.md §2.1, §10.1).
//!
//! A [`Location`] is what the user names (`--repo`, `--wsl`, a pasted path) and what a review
//! records. Opening one yields the [`Backend`] every other layer works through (PLAN §5.1).

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::backend::{Backend, BackendError, LocalBackend};

/// A repository's location.
///
/// Serialised externally tagged in snake_case — `{"local": "D:\\code\\api"}`,
/// `{"wsl": {"distro": "Ubuntu", "path": "/home/kyle/api"}}` — a stable shape, because reviews
/// record locations in their targets (PLAN §7).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Location {
    /// A path on this machine's filesystem, as given: it may be relative or point anywhere inside
    /// the work tree ([`LocalBackend::open`] resolves the root).
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

    /// Opens the backend for this location (PLAN §5.1): a [`LocalBackend`] honouring the user's
    /// own git configuration for a local path. A WSL location needs the in-distro backend
    /// (PLAN D6), which this build doesn't have, so it fails with
    /// [`BackendError::Unavailable`] and never falls back to running `wsl.exe` per command.
    pub fn open_backend(&self) -> Result<Arc<dyn Backend>, BackendError> {
        match self {
            Location::Local(path) => Ok(Arc::new(LocalBackend::open(path)?)),
            Location::Wsl { .. } => Err(BackendError::Unavailable {
                location: self.display_name(),
                reason: "WSL support arrives later".to_string(),
            }),
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display_name())
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
