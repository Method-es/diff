//! The file watchers a server registers (`client/registerCapability` for
//! `workspace/didChangeWatchedFiles`) and which changed files they match (docs/PLAN.md §18.5).
//!
//! dv watches the worktree itself (the review's watch, PLAN §5.3) and tells a server about the
//! changes its registrations ask for. A glob is LSP's: `*` and `?` within one path segment, `**`
//! across segments, `{a,b}` alternatives, `[...]` ranges; a plain string is matched against the
//! file's absolute path (`/`-separated), a relative pattern against the path under its base.

use globset::{Glob, GlobBuilder, GlobMatcher};
use serde_json::Value;

use crate::paths::{PathStyle, WorkspacePaths};

/// LSP's `WatchKind` bits.
pub const CREATE: u8 = 1;
pub const CHANGE: u8 = 2;
pub const DELETE: u8 = 4;

/// One registered watcher.
#[derive(Debug, Clone)]
pub(crate) struct Watcher {
    /// The registration it came with, for `client/unregisterCapability`.
    pub registration: String,
    /// The base directory of a relative pattern, absolute and `/`-separated; `None` for a
    /// plain glob, matched against the whole path.
    base: Option<String>,
    glob: GlobMatcher,
    kind: u8,
}

impl Watcher {
    /// The watchers of a `workspace/didChangeWatchedFiles` registration's options; a glob that
    /// doesn't compile is skipped (and logged).
    pub fn from_options(
        registration: &str,
        options: &Value,
        paths: &WorkspacePaths,
    ) -> Vec<Watcher> {
        let Some(watchers) = options.get("watchers").and_then(Value::as_array) else {
            return Vec::new();
        };
        watchers
            .iter()
            .filter_map(|watcher| {
                let kind = watcher
                    .get("kind")
                    .and_then(Value::as_u64)
                    .map_or(CREATE | CHANGE | DELETE, |kind| kind as u8);
                let pattern = watcher.get("globPattern")?;
                let (base, glob) = match pattern {
                    Value::String(glob) => (None, glob.as_str()),
                    Value::Object(relative) => {
                        let base = match relative.get("baseUri")? {
                            Value::String(uri) => uri.as_str(),
                            // A workspace folder: `{uri, name}`.
                            folder => folder.get("uri")?.as_str()?,
                        };
                        let base = paths.path_of_uri(base)?.replace('\\', "/");
                        (Some(base), relative.get("pattern")?.as_str()?)
                    }
                    _ => return None,
                };
                match compile(glob, paths.style()) {
                    Ok(glob) => Some(Watcher {
                        registration: registration.to_string(),
                        base: base.map(|base| base.trim_end_matches('/').to_string()),
                        glob,
                        kind,
                    }),
                    Err(err) => {
                        log::warn!("a language server's file watcher {glob:?} is unusable: {err}");
                        None
                    }
                }
            })
            .collect()
    }

    /// Whether this watcher wants a change of `kind` to `absolute` (`/`-separated).
    pub fn matches(&self, absolute: &str, kind: u8, style: PathStyle) -> bool {
        if self.kind & kind == 0 {
            return false;
        }
        match &self.base {
            None => self.glob.is_match(absolute),
            Some(base) => {
                let under = match style {
                    PathStyle::Posix => absolute.strip_prefix(base.as_str()),
                    PathStyle::Windows => absolute
                        .get(..base.len())
                        .filter(|head| head.eq_ignore_ascii_case(base))
                        .map(|_| &absolute[base.len()..]),
                };
                under
                    .and_then(|rest| rest.strip_prefix('/'))
                    .is_some_and(|rest| self.glob.is_match(rest))
            }
        }
    }
}

fn compile(glob: &str, style: PathStyle) -> Result<GlobMatcher, globset::Error> {
    GlobBuilder::new(glob)
        .literal_separator(true)
        .case_insensitive(style == PathStyle::Windows)
        .build()
        .map(|glob: Glob| glob.compile_matcher())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn plain_and_relative_globs() {
        let paths = WorkspacePaths::new("/w/repo");
        let watchers = Watcher::from_options(
            "r1",
            &json!({"watchers": [
                {"globPattern": "**/*.{ts,tsx}"},
                {"globPattern": {"baseUri": "file:///w/repo/packages/a", "pattern": "*.json"}, "kind": 4},
                {"globPattern": "[unclosed"}
            ]}),
            &paths,
        );
        assert_eq!(watchers.len(), 2);
        let style = PathStyle::Posix;
        assert!(watchers[0].matches("/w/repo/src/a.tsx", CHANGE, style));
        assert!(!watchers[0].matches("/w/repo/src/a.js", CHANGE, style));
        assert!(watchers[1].matches("/w/repo/packages/a/package.json", DELETE, style));
        // Not for a change, and not below the base's own directory.
        assert!(!watchers[1].matches("/w/repo/packages/a/package.json", CHANGE, style));
        assert!(!watchers[1].matches("/w/repo/packages/a/x/y.json", DELETE, style));
        assert!(!watchers[1].matches("/w/repo/packages/ab/y.json", DELETE, style));
    }

    #[test]
    fn windows_globs_ignore_case() {
        let paths = WorkspacePaths::new(r"D:\Repo");
        let watchers = Watcher::from_options(
            "r1",
            &json!({"watchers": [
                {"globPattern": {"baseUri": "file:///d%3A/repo", "pattern": "**/tsconfig.json"}}
            ]}),
            &paths,
        );
        assert!(watchers[0].matches("D:/Repo/app/TSConfig.json", CREATE, PathStyle::Windows));
    }
}
