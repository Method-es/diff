//! Node's package resolution, read through a `Backend` (docs/PLAN.md §18.4): a package named
//! `name` is `<dir>/node_modules/<name>` in the importing directory or the nearest directory
//! above it that has one, never looking inside a `node_modules` directory itself, and the
//! package's own dependencies resolve from its real directory, symbolic links followed. That is
//! what makes the npm (hoisted), pnpm and bun layouts (`node_modules/.pnpm/…`,
//! `node_modules/.bun/…`, each package a link to its real folder beside its dependencies' links)
//! all resolve the same way.
//!
//! Like Node, the search goes on above the worktree root (a linked worktree inside the main
//! checkout, with no `node_modules` of its own, uses the checkout's) and follows links wherever
//! they lead (pnpm's global store, `npm link`). It reads with `Backend::probe_path`, absolute
//! paths on the backend's machine, read-only; a path's real location is found a component at a
//! time ([`real_path`]), each probe remembered for the resolution ([`Reader`]).

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

use dv_core::backend::{Backend, BackendError, PathProbe};
use serde_json::Value;

use crate::paths::PathStyle;

/// How many links one path may pass through before it counts as a loop (Linux's own limit).
const MAX_LINKS: usize = 40;

/// An absolute, normalised path on the backend's machine: its root (`/`, `C:/`,
/// `//server/share/`) and its components.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AbsPath {
    style: PathStyle,
    root: String,
    parts: Vec<String>,
}

impl AbsPath {
    /// `text` as an absolute path in `style`: separators either way on Windows, `.` and `..`
    /// resolved, a verbatim prefix (`\\?\`, a junction's `\??\`) dropped. `None` when it isn't
    /// absolute.
    pub fn parse(text: &str, style: PathStyle) -> Option<Self> {
        let (root, rest) = match style {
            PathStyle::Posix => ("/".to_string(), text.strip_prefix('/')?.to_string()),
            PathStyle::Windows => {
                let mut path = text.replace('\\', "/");
                for prefix in ["//?/UNC/", "/??/UNC/"] {
                    if let Some(unc) = path.strip_prefix(prefix) {
                        path = format!("//{unc}");
                    }
                }
                for prefix in ["//?/", "/??/"] {
                    if let Some(rest) = path.strip_prefix(prefix) {
                        path = rest.to_string();
                    }
                }
                let bytes = path.as_bytes();
                if bytes.len() >= 3
                    && bytes[0].is_ascii_alphabetic()
                    && bytes[1] == b':'
                    && bytes[2] == b'/'
                {
                    (path[..3].to_ascii_uppercase(), path[3..].to_string())
                } else {
                    let unc = path.strip_prefix("//")?;
                    let mut parts = unc.splitn(3, '/');
                    let server = parts.next().filter(|part| !part.is_empty())?;
                    let share = parts.next().filter(|part| !part.is_empty())?;
                    (
                        format!("//{server}/{share}/"),
                        parts.next().unwrap_or("").to_string(),
                    )
                }
            }
        };
        let mut path = Self {
            style,
            root,
            parts: Vec::new(),
        };
        path.push_relative(&rest);
        Some(path)
    }

    /// Appends a `/`-separated relative path, `.` and `..` resolved (`..` stops at the root).
    fn push_relative(&mut self, rel: &str) {
        for part in rel.split(['/', '\\']) {
            match part {
                "" | "." => {}
                ".." => {
                    self.parts.pop();
                }
                part => self.parts.push(part.to_string()),
            }
        }
    }

    /// This path and a `/`-separated relative path.
    pub fn join(&self, rel: &str) -> Self {
        let mut joined = self.clone();
        joined.push_relative(rel);
        joined
    }

    pub fn parent(&self) -> Option<Self> {
        let mut parent = self.clone();
        parent.parts.pop()?;
        Some(parent)
    }

    pub fn file_name(&self) -> Option<&str> {
        self.parts.last().map(String::as_str)
    }

    /// This path and every directory above it, ending with the root.
    pub fn ancestors(&self) -> Vec<Self> {
        let mut all = vec![self.clone()];
        while let Some(parent) = all.last().and_then(Self::parent) {
            all.push(parent);
        }
        all
    }

    /// The path as the backend's machine spells it (`\` on Windows).
    pub fn native(&self) -> String {
        let joined = format!("{}{}", self.root, self.parts.join("/"));
        match self.style {
            PathStyle::Posix => joined,
            PathStyle::Windows => joined.replace('/', "\\"),
        }
    }
}

impl std::fmt::Display for AbsPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.native())
    }
}

/// Probes paths through a backend, each path once per resolution.
pub struct Reader<'a> {
    backend: &'a dyn Backend,
    style: PathStyle,
    seen: RefCell<HashMap<String, PathProbe>>,
}

impl<'a> Reader<'a> {
    pub fn new(backend: &'a dyn Backend, style: PathStyle) -> Self {
        Self {
            backend,
            style,
            seen: RefCell::new(HashMap::new()),
        }
    }

    pub fn style(&self) -> PathStyle {
        self.style
    }

    /// The backend it reads through.
    pub fn backend(&self) -> &'a dyn Backend {
        self.backend
    }

    pub fn probe(&self, path: &AbsPath) -> Result<PathProbe, BackendError> {
        let native = path.native();
        if let Some(known) = self.seen.borrow().get(&native) {
            return Ok(known.clone());
        }
        let probe = self.backend.probe_path(&native)?;
        self.seen.borrow_mut().insert(native, probe.clone());
        Ok(probe)
    }

    /// Whether `path` exists (a file, a directory or a link).
    pub fn exists(&self, path: &AbsPath) -> Result<bool, BackendError> {
        Ok(self.probe(path)? != PathProbe::Missing)
    }

    /// A small file's bytes (a `package.json`), links followed; `None` when there is none.
    pub fn read_small(&self, path: &AbsPath) -> Result<Option<Vec<u8>>, BackendError> {
        let RealPath::Found(real) = real_path(self, path)? else {
            return Ok(None);
        };
        Ok(match self.probe(&real)? {
            PathProbe::File { contents, .. } => contents,
            _ => None,
        })
    }
}

/// Where a path really is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealPath {
    /// Every link on the way resolved.
    Found(AbsPath),
    Missing,
    /// The way there can't be read: a link whose target (or a directory on the way) dv may not
    /// read, or a loop of links. `at` is where, `reason` why.
    Unreadable {
        at: AbsPath,
        reason: String,
    },
}

/// `path` with every symbolic link on the way resolved.
pub fn real_path(reader: &Reader<'_>, path: &AbsPath) -> Result<RealPath, BackendError> {
    let mut pending: VecDeque<String> = path.parts.iter().cloned().collect();
    let mut done = AbsPath {
        parts: Vec::new(),
        ..path.clone()
    };
    let mut links = 0;
    while let Some(part) = pending.pop_front() {
        match part.as_str() {
            "" | "." => continue,
            ".." => {
                done.parts.pop();
                continue;
            }
            _ => {}
        }
        done.parts.push(part);
        let probe = match reader.probe(&done) {
            Ok(probe) => probe,
            Err(err @ BackendError::Io { .. }) => {
                return Ok(RealPath::Unreadable {
                    at: done,
                    reason: err.to_string(),
                });
            }
            Err(err) => return Err(err),
        };
        match probe {
            PathProbe::Missing => return Ok(RealPath::Missing),
            PathProbe::Dir => {}
            PathProbe::File { .. } if pending.iter().all(|rest| rest.is_empty() || rest == ".") => {
            }
            PathProbe::File { .. } => return Ok(RealPath::Missing),
            PathProbe::Link { target } => {
                links += 1;
                if links > MAX_LINKS {
                    return Ok(RealPath::Unreadable {
                        at: done,
                        reason: "too many links on the way (a loop?)".into(),
                    });
                }
                done.parts.pop();
                let parts: Vec<String> = match AbsPath::parse(&target, reader.style()) {
                    Some(absolute) => {
                        done = AbsPath {
                            parts: Vec::new(),
                            ..absolute.clone()
                        };
                        absolute.parts
                    }
                    None => target.split(['/', '\\']).map(str::to_string).collect(),
                };
                for part in parts.into_iter().rev() {
                    pending.push_front(part);
                }
            }
        }
    }
    Ok(RealPath::Found(done))
}

/// A package found by [`find_package`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// Where it was found: `<dir>/node_modules/<name>`.
    pub found_at: AbsPath,
    /// Its real directory, links resolved.
    pub real_dir: AbsPath,
    /// Its `package.json`.
    pub manifest: Value,
}

impl Package {
    pub fn version(&self) -> Option<&str> {
        self.manifest.get("version").and_then(Value::as_str)
    }
}

/// What [`find_package`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Search {
    Found(Package),
    /// No `node_modules/<name>` with a `package.json` in any of these directories.
    NotFound {
        looked_in: Vec<AbsPath>,
    },
    /// One was there, but dv couldn't read where it leads (the first such, for the message).
    Unreadable {
        found_at: AbsPath,
        at: AbsPath,
        reason: String,
    },
}

/// Node's resolution of the package `name` from the directory `from`, up to the filesystem's
/// root (module docs).
pub fn find_package(
    reader: &Reader<'_>,
    from: &AbsPath,
    name: &str,
) -> Result<Search, BackendError> {
    let mut looked_in = Vec::new();
    let mut unreadable = None;
    for dir in from.ancestors() {
        if dir.file_name() == Some("node_modules") {
            continue;
        }
        let found_at = dir.join("node_modules").join(name);
        match real_path(reader, &found_at)? {
            RealPath::Found(real_dir) => {
                match reader.read_small(&real_dir.join("package.json"))? {
                    Some(bytes) => {
                        let manifest = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                        return Ok(Search::Found(Package {
                            found_at,
                            real_dir,
                            manifest,
                        }));
                    }
                    None => looked_in.push(dir),
                }
            }
            RealPath::Missing => looked_in.push(dir),
            RealPath::Unreadable { at, reason } => {
                unreadable.get_or_insert(Search::Unreadable {
                    found_at,
                    at,
                    reason,
                });
            }
        }
    }
    Ok(unreadable.unwrap_or(Search::NotFound { looked_in }))
}

/// The major version of a semver string (`7` of `7.0.2`, of `7.1.0-dev.20260101`).
pub fn major_version(version: &str) -> Option<u64> {
    let digits: String = version
        .trim()
        .trim_start_matches('v')
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// Directories as a message lists them: `a, b or c`.
pub fn list_dirs(dirs: &[AbsPath]) -> String {
    let names: Vec<String> = dirs.iter().map(AbsPath::native).collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_paths_parse_and_normalise() {
        let posix = |text| AbsPath::parse(text, PathStyle::Posix).map(|path| path.native());
        assert_eq!(posix("/a/./b/../c/"), Some("/a/c".into()));
        assert_eq!(posix("/.."), Some("/".into()));
        assert_eq!(posix("a/b"), None);
        let windows = |text| AbsPath::parse(text, PathStyle::Windows).map(|path| path.native());
        assert_eq!(windows(r"d:\x\y"), Some(r"D:\x\y".into()));
        assert_eq!(windows("D:/x/../y"), Some(r"D:\y".into()));
        assert_eq!(windows(r"\\?\D:\store\ts"), Some(r"D:\store\ts".into()));
        assert_eq!(windows(r"\??\D:\store"), Some(r"D:\store".into()));
        assert_eq!(
            windows(r"\\?\UNC\srv\share\x"),
            Some(r"\\srv\share\x".into())
        );
        assert_eq!(windows(r"\\srv\share"), Some(r"\\srv\share\".into()));
        assert_eq!(windows(r"..\x"), None);
        let path = AbsPath::parse("/a/b", PathStyle::Posix).unwrap();
        assert_eq!(
            path.ancestors()
                .iter()
                .map(AbsPath::native)
                .collect::<Vec<_>>(),
            ["/a/b", "/a", "/"]
        );
        assert_eq!(path.join("../c/./d").native(), "/a/c/d");
        assert_eq!(list_dirs(&path.ancestors()), "/a/b, /a or /".to_string());
    }
}
