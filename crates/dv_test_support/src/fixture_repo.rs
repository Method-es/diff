//! `FixtureRepo`: a real git repository in a temp dir, built step by step through the `git` CLI
//! (docs/PLAN.md §14.2).
//!
//! Fixtures are reproducible: every commit carries the same identity and a date derived from its
//! position ([`EPOCH`] + n × [`TICK_SECS`]), so the same script yields the same commit ids on
//! every machine. The fixture's own git calls ignore the machine's git configuration (system,
//! global and XDG files, and the `GIT_*` variables inherited from the test's environment that
//! change what git reads, writes or prints: other repositories and configs, pathspec magic, diff
//! options and external diff tools, attribute sources, identities, `git init` templates, the
//! hash algorithm); the fixture sets the identity and dates itself. The settings that matter are
//! also written into the repo's own `.git/config` at creation, so code under test that runs plain
//! `git` against the fixture sees them too (identity, no signing, `core.autocrlf=false` until a
//! test sets it).
//!
//! Code under test that spawns git itself (dv_core's `LocalBackend`) gets the same isolation
//! through [`FixtureRepo::env_overlay`]: the exact set of variables the fixture's own git calls
//! set and remove, so a test sees the fixture's configuration and nothing of the developer's.

use std::cell::Cell;
use std::ffi::{OsStr, OsString};
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

/// Variables that point git at another repository, index or configuration, that change how it
/// reads pathspecs or prints diffs (`GIT_DIFF_OPTS` sets the context size, `GIT_EXTERNAL_DIFF`
/// replaces the diff, `GIT_ATTR_SOURCE` reads `.gitattributes` from another tree), or that shape
/// what `git init` creates (a template's hooks and files, SHA-256 object ids). A test can run
/// inside a git hook, under `git -c` (which exports `GIT_CONFIG_PARAMETERS`) or
/// `git --attr-source` (which exports `GIT_ATTR_SOURCE`), or in a shell that exports any of
/// them, so the fixture's git calls drop them. The identity variables are set rather than dropped ([`FixtureRepo::env_overlay`]).
///
/// A superset of dv_core's `git::command::DROPPED_ENV` (which this crate can't import, being
/// dv_core's dev-dependency), so a test's reference `git diff` and the code under test drop the
/// same variables; a dv_core test checks the two lists stay in step.
const INHERITED_GIT_VARS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_QUARANTINE_PATH",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_LITERAL_PATHSPECS",
    "GIT_GLOB_PATHSPECS",
    "GIT_NOGLOB_PATHSPECS",
    "GIT_ICASE_PATHSPECS",
    "GIT_DIFF_OPTS",
    "GIT_EXTERNAL_DIFF",
    "GIT_ATTR_SOURCE",
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
    /// Owns `<root>/repo` (the work tree), `<root>/home` (the fixture's `HOME`) and whatever else
    /// a test creates beside them ([`FixtureRepo::add_worktree`], [`FixtureRepo::bare_clone`],
    /// [`FixtureRepo::outside_dir`]).
    root: tempfile::TempDir,
    /// `root`'s path in the form git reports it ([`real_path`]).
    base: PathBuf,
    path: PathBuf,
    home: PathBuf,
    /// Commits made so far; dates the next one.
    commits: Cell<i64>,
}

impl FixtureRepo {
    /// An empty repository (no commits) on branch `main`, with SHA-1 object ids.
    pub fn new() -> Self {
        Self::init(&[])
    }

    /// An empty repository (no commits) on branch `main` whose object ids are SHA-256
    /// (`git init --object-format=sha256`).
    pub fn new_sha256() -> Self {
        Self::init(&["--object-format=sha256"])
    }

    fn init(extra: &[&str]) -> Self {
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
            base,
            path,
            home,
            commits: Cell::new(0),
        };
        let mut init = vec!["-c", "init.defaultBranch=main", "init", "-q"];
        init.extend_from_slice(extra);
        repo.git(&init);
        for (key, value) in LOCAL_CONFIG {
            repo.config(key, value);
        }
        repo
    }

    /// The work tree's root.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The fixture's `HOME`, where its global git configuration (`.gitconfig`) and XDG files
    /// (`.config/git/ignore`, …) live. Empty until a test writes something there.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// The environment changes every git the fixture runs gets, as `(name, Some(value))` to set
    /// and `(name, None)` to remove: none of the developer's system, global or XDG configuration
    /// (`HOME`, `XDG_CONFIG_HOME` and `GIT_CONFIG_GLOBAL` point into the fixture), no system
    /// gitattributes, no repository discovery above the fixture's temp dir
    /// (`GIT_CEILING_DIRECTORIES`), `LC_ALL=C`, the fixture identity and the next commit's dates,
    /// and none of the inherited variables that point git elsewhere or change what it prints
    /// ([`INHERITED_GIT_VARS`]).
    ///
    /// Tests hand this to code that spawns git itself (dv_core's `LocalBackendOptions`), so that
    /// code runs against exactly the fixture's configuration on every machine.
    ///
    /// The identity is set here as well as in `.git/config` because inherited `GIT_AUTHOR_*` /
    /// `GIT_COMMITTER_*` variables take precedence over `user.name` and `user.email`, and
    /// `git commit` exports `GIT_AUTHOR_*` to its hooks, so a test run from a hook would
    /// otherwise commit as the developer.
    pub fn env_overlay(&self) -> Vec<(OsString, Option<OsString>)> {
        let date = format!("{} +0000", EPOCH + self.commits.get() * TICK_SECS);
        let set = |name: &str, value: &OsStr| (OsString::from(name), Some(value.to_os_string()));
        let mut env: Vec<_> = INHERITED_GIT_VARS
            .iter()
            .map(|var| (OsString::from(var), None))
            .collect();
        env.extend([
            set("HOME", self.home.as_os_str()),
            set("XDG_CONFIG_HOME", self.home.join(".config").as_os_str()),
            set(
                "GIT_CONFIG_GLOBAL",
                self.home.join(".gitconfig").as_os_str(),
            ),
            set("GIT_CONFIG_NOSYSTEM", "1".as_ref()),
            set("GIT_ATTR_NOSYSTEM", "1".as_ref()),
            set("GIT_CEILING_DIRECTORIES", self.base.as_os_str()),
            set("GIT_TERMINAL_PROMPT", "0".as_ref()),
            set("GIT_MERGE_AUTOEDIT", "no".as_ref()),
            set("LC_ALL", "C".as_ref()),
            set("GIT_AUTHOR_NAME", AUTHOR_NAME.as_ref()),
            set("GIT_AUTHOR_EMAIL", AUTHOR_EMAIL.as_ref()),
            set("GIT_AUTHOR_DATE", date.as_ref()),
            set("GIT_COMMITTER_NAME", AUTHOR_NAME.as_ref()),
            set("GIT_COMMITTER_EMAIL", AUTHOR_EMAIL.as_ref()),
            set("GIT_COMMITTER_DATE", date.as_ref()),
        ]);
        env
    }

    /// A `git` command for this repo: `git -C <path>` with [`FixtureRepo::env_overlay`] applied
    /// and a null stdin. For calls [`FixtureRepo::git`] doesn't cover, such as feeding stdin or
    /// reading raw bytes.
    pub fn git_command(&self) -> Command {
        let mut command = Command::new("git");
        command.arg("-C").arg(&self.path);
        apply_env(&mut command, &self.env_overlay());
        command.stdin(Stdio::null());
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

    /// Writes `contents` into the object store as a blob (`git hash-object -w`) and returns its
    /// id, for [`FixtureRepo::stage`].
    pub fn hash_blob(&self, contents: impl AsRef<[u8]>) -> String {
        let mut child = self
            .git_command()
            .args(["hash-object", "-w", "--stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|err| panic!("starting `git hash-object`: {err}"));
        {
            use std::io::Write as _;
            let mut stdin = child.stdin.take().expect("stdin was requested as a pipe");
            stdin
                .write_all(contents.as_ref())
                .unwrap_or_else(|err| panic!("feeding `git hash-object`: {err}"));
        }
        let output = child
            .wait_with_output()
            .unwrap_or_else(|err| panic!("running `git hash-object`: {err}"));
        assert!(
            output.status.success(),
            "`git hash-object` failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Puts an entry straight into the index (`git update-index --add --cacheinfo`), whatever
    /// the work tree holds: `mode` as git writes it (`100644`, `100755`, `120000` for a symlink,
    /// `160000` for a submodule's gitlink), `oid` a blob from [`FixtureRepo::hash_blob`] or, for
    /// a gitlink, any commit id. [`FixtureRepo::commit_staged`] commits it.
    ///
    /// Paths may hold characters no Windows file name can (`"`, a tab): git for Windows refuses
    /// those in the index unless `core.protectNTFS` is off, which this one call turns off, so
    /// tests of git's name quoting run on every OS.
    pub fn stage(&self, mode: &str, oid: &str, path: &str) -> &Self {
        self.git(&[
            "-c",
            "core.protectNTFS=false",
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("{mode},{oid},{path}"),
        ]);
        self
    }

    /// Commits the index exactly as it is, without [`FixtureRepo::commit`]'s `git add -A`, and
    /// returns the new commit's id: for entries made with [`FixtureRepo::stage`] or
    /// `git update-index`, which the work tree doesn't reflect.
    pub fn commit_staged(&self, message: &str) -> String {
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

    /// Adds a linked worktree (`git worktree add`) on a new branch `branch` created at `HEAD`,
    /// at `<temp dir>/worktrees/<name>` beside the main work tree, and returns its path. Its git
    /// dir is `.git/worktrees/<name>`; its common dir stays the main repo's `.git`.
    pub fn add_worktree(&self, name: &str, branch: &str) -> PathBuf {
        let path = self.beside("worktrees", name);
        self.git(&[
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("-q"),
            OsStr::new("-b"),
            OsStr::new(branch),
            path.as_os_str(),
        ]);
        path
    }

    /// Clones this repo as a bare repository to `<temp dir>/<name>` and returns its path.
    pub fn bare_clone(&self, name: &str) -> PathBuf {
        let path = self.beside("", name);
        self.git(&[
            OsStr::new("clone"),
            OsStr::new("-q"),
            OsStr::new("--bare"),
            self.path.as_os_str(),
            path.as_os_str(),
        ]);
        path
    }

    /// Creates an empty directory `<temp dir>/<name>` outside every repository (the fixture's
    /// git calls and [`FixtureRepo::env_overlay`] stop repository discovery at the temp dir) and
    /// returns its path.
    pub fn outside_dir(&self, name: &str) -> PathBuf {
        let path = self.beside("", name);
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|err| panic!("creating {}: {err}", path.display()));
        path
    }

    /// `<temp dir>/<group>/<name>` (`<temp dir>/<name>` for an empty group): a new entry beside
    /// the work tree. Panics on a name that isn't a plain file name, or one that is taken.
    fn beside(&self, group: &str, name: &str) -> PathBuf {
        let plain = !name.is_empty()
            && !name.starts_with('.')
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        assert!(
            plain && !["repo", "home", "worktrees"].contains(&name),
            "names beside the fixture repo are plain file names other than repo, home and \
             worktrees, got {name:?}"
        );
        let path = self.base.join(group).join(name);
        assert!(!path.exists(), "{} already exists", path.display());
        path
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

/// Applies an [`FixtureRepo::env_overlay`]-shaped list to `command`.
fn apply_env(command: &mut Command, env: &[(OsString, Option<OsString>)]) {
    for (name, value) in env {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
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
