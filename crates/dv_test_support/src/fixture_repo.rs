//! `FixtureRepo`: a real git repository in a temp dir, built step by step through the `git` CLI
//! (docs/PLAN.md §14.2).
//!
//! Fixtures are reproducible: every commit carries the same identity and a date derived from its
//! position ([`EPOCH`] + n × [`TICK_SECS`]), so the same script yields the same commit ids on
//! every machine. The fixture's own git calls ignore the machine's git configuration (system,
//! global and XDG files, and the `GIT_*` variables inherited from the test's environment that
//! change what git reads or writes: other repositories and configs, identities, `git init`
//! templates, the hash algorithm); the fixture sets the identity and dates itself. The settings
//! that matter are also written into the repo's own `.git/config` at creation, so code under test
//! that runs plain `git` against the fixture sees them too (identity, no signing,
//! `core.autocrlf=false` until a test sets it).

use std::cell::Cell;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

/// `user.name` of every fixture commit.
pub const AUTHOR_NAME: &str = "dv fixture";
/// `user.email` of every fixture commit.
pub const AUTHOR_EMAIL: &str = "fixture@dv.invalid";
/// The first commit's author and committer date: 2026-01-01T00:00:00Z, as Unix seconds.
pub const EPOCH: i64 = 1_767_225_600;
/// Each commit is dated this many seconds after the previous one.
pub const TICK_SECS: i64 = 60;

/// Variables that point git at another repository, index or configuration, or that shape what
/// `git init` creates (a template's hooks and files, SHA-256 object ids). A test can run inside a
/// git hook or under `git -c` (which exports `GIT_CONFIG_PARAMETERS`), so the fixture's git calls
/// drop them. The identity variables are set rather than dropped ([`FixtureRepo::git_command`]).
const INHERITED_GIT_VARS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_TEMPLATE_DIR",
    "GIT_DEFAULT_HASH",
];

/// Settings written into every fixture's `.git/config` at creation.
const LOCAL_CONFIG: &[(&str, &str)] = &[
    ("user.name", AUTHOR_NAME),
    ("user.email", AUTHOR_EMAIL),
    ("commit.gpgsign", "false"),
    ("tag.gpgsign", "false"),
    // Git for Windows' system config turns this on; the fixture's default is git's own, and CRLF
    // tests opt in with `config("core.autocrlf", "true")`.
    ("core.autocrlf", "false"),
    ("gc.auto", "0"),
];

/// A git repository in a temp dir, removed when the value is dropped.
///
/// Methods that change the repo return `&Self`, so a fixture reads as a short script:
///
/// ```ignore
/// let repo = FixtureRepo::new();
/// repo.write("src/lib.rs", "fn a() {}\n").set_attributes("*.png binary\n");
/// let base = repo.commit("base");
/// repo.branch("feat").checkout("feat").write("src/lib.rs", "fn b() {}\n");
/// let tip = repo.commit("change");
/// ```
///
/// Paths are relative to the repo root and may not leave it.
pub struct FixtureRepo {
    /// Owns `<root>/repo` (the work tree) and `<root>/home` (the fixture's `HOME`).
    root: tempfile::TempDir,
    path: PathBuf,
    home: PathBuf,
    /// Commits made so far; dates the next one.
    commits: Cell<i64>,
}

impl FixtureRepo {
    /// An empty repository (no commits) on branch `main`.
    pub fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("dv-fixture-")
            .tempdir()
            .unwrap_or_else(|err| panic!("creating a fixture temp dir: {err}"));
        let base = real_path(root.path());
        let path = base.join("repo");
        let home = base.join("home");
        for dir in [&path, &home] {
            std::fs::create_dir_all(dir)
                .unwrap_or_else(|err| panic!("creating {}: {err}", dir.display()));
        }
        let repo = Self {
            root,
            path,
            home,
            commits: Cell::new(0),
        };
        repo.git(&["-c", "init.defaultBranch=main", "init", "-q"]);
        for (key, value) in LOCAL_CONFIG {
            repo.config(key, value);
        }
        repo
    }

    /// The work tree's root.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A `git` command for this repo: `git -C <path>` with the fixture's environment (no system,
    /// global or XDG configuration, no system gitattributes, `LC_ALL=C`, the fixture identity,
    /// this commit's dates) and a null stdin. For calls [`FixtureRepo::git`] doesn't cover, such
    /// as feeding stdin or reading raw bytes.
    ///
    /// The identity is set in the environment as well as in `.git/config` because inherited
    /// `GIT_AUTHOR_*` / `GIT_COMMITTER_*` variables take precedence over `user.name` and
    /// `user.email`, and `git commit` exports `GIT_AUTHOR_*` to its hooks, so a test run from a
    /// hook would otherwise commit as the developer.
    pub fn git_command(&self) -> Command {
        let date = format!("{} +0000", EPOCH + self.commits.get() * TICK_SECS);
        let mut command = Command::new("git");
        command.arg("-C").arg(&self.path);
        for var in INHERITED_GIT_VARS {
            command.env_remove(var);
        }
        command
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("GIT_CONFIG_GLOBAL", self.home.join(".gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_MERGE_AUTOEDIT", "no")
            .env("LC_ALL", "C")
            .env("GIT_AUTHOR_NAME", AUTHOR_NAME)
            .env("GIT_AUTHOR_EMAIL", AUTHOR_EMAIL)
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_NAME", AUTHOR_NAME)
            .env("GIT_COMMITTER_EMAIL", AUTHOR_EMAIL)
            .env("GIT_COMMITTER_DATE", &date)
            .stdin(Stdio::null());
        command
    }

    /// Runs `git <args>` in the repo and returns its stdout exactly as printed (lossy UTF-8).
    /// Panics with git's stderr when it fails.
    pub fn git<S: AsRef<OsStr>>(&self, args: &[S]) -> String {
        let shown = args
            .iter()
            .map(|arg| arg.as_ref().to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        let output = self
            .git_command()
            .args(args)
            .output()
            .unwrap_or_else(|err| panic!("starting `git {shown}`: {err}"));
        if !output.status.success() {
            panic!(
                "`git {shown}` failed ({}) in {}:\n{}",
                output.status,
                self.path.display(),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Writes `contents` to `path` byte for byte, creating parent directories.
    pub fn write(&self, path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> &Self {
        let full = self.resolve(path.as_ref());
        create_parent(&full);
        std::fs::write(&full, contents)
            .unwrap_or_else(|err| panic!("writing {}: {err}", full.display()));
        self
    }

    /// Writes `text` to `path` with every line ending as CRLF (`\n` and `\r\n` in `text` alike).
    pub fn write_crlf(&self, path: impl AsRef<Path>, text: &str) -> &Self {
        self.write(path, text.replace("\r\n", "\n").replace('\n', "\r\n"))
    }

    /// Deletes a file, or a directory and everything in it, from the work tree. The next
    /// [`FixtureRepo::commit`] records the deletion.
    pub fn remove(&self, path: impl AsRef<Path>) -> &Self {
        let full = self.resolve(path.as_ref());
        let metadata = std::fs::symlink_metadata(&full)
            .unwrap_or_else(|err| panic!("removing {}: {err}", full.display()));
        let removed = if metadata.is_dir() {
            std::fs::remove_dir_all(&full)
        } else {
            std::fs::remove_file(&full)
        };
        removed.unwrap_or_else(|err| panic!("removing {}: {err}", full.display()));
        self
    }

    /// Moves `from` to `to` in the work tree, creating `to`'s parent directories. Committed
    /// unchanged, git's rename detection reports it as a rename.
    pub fn rename(&self, from: impl AsRef<Path>, to: impl AsRef<Path>) -> &Self {
        let (from, to) = (self.resolve(from.as_ref()), self.resolve(to.as_ref()));
        create_parent(&to);
        std::fs::rename(&from, &to)
            .unwrap_or_else(|err| panic!("renaming {} to {}: {err}", from.display(), to.display()));
        self
    }

    /// Stages everything in the work tree (`git add -A`: new, changed and deleted files) and
    /// commits it, returning the new commit's id. Panics when there is nothing to commit.
    pub fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
        let oid = self.git(&["rev-parse", "HEAD"]).trim().to_string();
        self.commits.set(self.commits.get() + 1);
        oid
    }

    /// Creates branch `name` at `HEAD` without switching to it.
    pub fn branch(&self, name: &str) -> &Self {
        self.git(&["branch", name]);
        self
    }

    /// Checks out a branch, or detaches `HEAD` at any other revision.
    pub fn checkout(&self, rev: &str) -> &Self {
        self.git(&["checkout", "-q", rev]);
        self
    }

    /// Sets `key` to `value` in the repo's own `.git/config`.
    pub fn config(&self, key: &str, value: &str) -> &Self {
        self.git(&["config", key, value]);
        self
    }

    /// Replaces the root `.gitattributes` with `text`.
    pub fn set_attributes(&self, text: &str) -> &Self {
        self.write(".gitattributes", text)
    }

    /// `path` under the repo root. Panics on an absolute path or one that climbs out with `..`,
    /// so a typo in a test can't write outside its fixture.
    fn resolve(&self, path: &Path) -> PathBuf {
        let inside = path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
        assert!(
            inside && path.components().next().is_some(),
            "fixture paths are relative to the repo root and stay inside it, got {}",
            path.display()
        );
        self.path.join(path)
    }
}

impl Default for FixtureRepo {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for FixtureRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FixtureRepo")
            .field("path", &self.path)
            .field("commits", &self.commits.get())
            .field("root", &self.root.path())
            .finish()
    }
}

fn create_parent(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .unwrap_or_else(|err| panic!("creating {}: {err}", parent.display()));
    }
}

/// The temp dir with symlinks resolved where that keeps the path usable: macOS's `/var` is a
/// link to `/private/var`, and git reports the resolved form. Windows keeps the path as given,
/// because resolving it produces a `\\?\` path many tools reject.
fn real_path(path: &Path) -> PathBuf {
    if cfg!(windows) {
        path.to_path_buf()
    } else {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }
}

#[cfg(test)]
mod tests;
