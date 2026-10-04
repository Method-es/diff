//! The file watcher behind [`super::LocalBackend`]'s `watch` (docs/PLAN.md §5.3).
//!
//! One `notify` watcher (the OS's recommended backend) covers the worktree and the review store
//! (`<git-common-dir>/dv/`, watched on its own when it lies outside the worktree, as it does for a
//! linked worktree). A background thread filters and coalesces its events:
//!
//! - Ignored worktree paths are dropped, decided the way git decides with the `ignore` crate's
//!   gitignore matcher: every `.gitignore` in the worktree (the nearest one wins), then
//!   `info/exclude`, then the global excludes file, and nothing inside an ignored directory can be
//!   re-included. Files git tracks although a pattern matches them (force-added) still count,
//!   because git still diffs them; `git ls-files --cached --ignored --exclude-standard` lists
//!   them once at start. `.gitignore` files are re-read as they change. A removed path can't be
//!   asked whether it was a directory, so the watch remembers the ignored directories it has
//!   seen, and removing `target/` under a `target/` pattern is dropped like a write inside it.
//! - A symbolic link is one path, never followed, because git stores a link as its target path
//!   and never descends into one: neither the OS watch (`notify` is told not to follow links)
//!   nor the listing of a new directory enters a link to a directory, and a directory-only
//!   pattern (`foo/`) doesn't match a link named `foo`.
//! - The git dir is dropped, except the store, which maps to store paths. `<common-dir>/info/`
//!   is watched for `info/exclude` edits on its own when it lies outside the worktree (a linked
//!   worktree's).
//! - Read-only access events are dropped (git reading files must not look like a change).
//! - A directory that appears (created or moved in) is listed before its batch is delivered, and
//!   everything in it that isn't ignored is reported too. inotify registers a new directory's
//!   watch only after reporting the directory, so files written into it in between would
//!   otherwise never be reported; a directory moved in brings its files without an event each.
//! - Paths are collected for [`COALESCE`] after the first event of a batch, then delivered once
//!   each, sorted, so a burst of writes (an agent saving many files) is one refresh. A batch with
//!   more than [`MAX_PATHS`] paths is delivered as a rescan.
//!
//! The OS watch of the worktree registers ignored directories too (inotify needs one watch per
//! directory, so a huge `node_modules/` can exhaust the per-user limit on Linux). When it fails
//! and the store was asked for as well, the watch runs for the store alone and the handle says
//! why the worktree isn't watched ([`WatchHandle::worktree_error`]), rather than losing the
//! store's notifications too.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher as _};

use super::{BackendError, GitOpts, WatchEvent, WatchHandle, WatchSink, WatchSpec};
use crate::git::command::{self, EnvOverlay};

/// How long a batch collects events after its first one (PLAN §5.3).
pub(crate) const COALESCE: Duration = Duration::from_millis(150);

/// The time between a directory's event and listing it, so the OS watcher has registered the
/// directory by then and whatever the listing misses arrives as events.
const NEW_DIR_SETTLE: Duration = Duration::from_millis(20);

/// The most paths one batch lists; past this, consumers get a rescan rather than a huge list.
const MAX_PATHS: usize = 10_000;

/// The directories a watch classifies event paths against.
#[derive(Debug, Clone)]
pub(crate) struct Roots {
    pub toplevel: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub store_root: PathBuf,
}

impl Roots {
    /// The roots in the spelling the OS watcher reports. macOS reports resolved paths (`/private
    /// /var/…` for `/var/…`), so they are resolved there and on Linux; Windows keeps them as
    /// given, because the watcher reports paths under the watched path as given, and resolving
    /// would produce `\\?\` paths.
    fn resolved(self) -> Self {
        if cfg!(windows) {
            return self;
        }
        let resolve = |path: PathBuf| fs::canonicalize(&path).unwrap_or(path);
        Self {
            toplevel: resolve(self.toplevel),
            git_dir: resolve(self.git_dir),
            common_dir: resolve(self.common_dir),
            // Created before the watch starts; resolved after that in `start`.
            store_root: self.store_root,
        }
    }
}

enum Msg {
    Fs(notify::Result<notify::Event>),
    Stop,
}

/// Starts a watch (module docs). The OS watches are registered before this returns, so a change
/// made after it returns is reported.
pub(crate) fn start(
    roots: Roots,
    env: Vec<(OsString, Option<OsString>)>,
    mut spec: WatchSpec,
    sink: WatchSink,
) -> Result<WatchHandle, BackendError> {
    let mut roots = roots.resolved();
    let watch_error = |path: &Path, message: String| BackendError::Watch {
        path: path.display().to_string(),
        message,
    };
    if spec.store {
        // The directory must exist to be watched; the store creates it on first write anyway.
        fs::create_dir_all(&roots.store_root)
            .map_err(|err| BackendError::io("creating", &roots.store_root, err))?;
        if !cfg!(windows) {
            roots.store_root = fs::canonicalize(&roots.store_root).unwrap_or(roots.store_root);
        }
    }

    let (tx, rx) = mpsc::channel();
    let events = tx.clone();
    let handler = move |event: notify::Result<notify::Event>| {
        // The receiver is gone only once the watch has stopped.
        let _ = events.send(Msg::Fs(event));
    };
    // Links are never followed (module docs): inotify and kqueue would otherwise watch, and
    // report changes inside, the directory a link points to.
    let config = notify::Config::default().with_follow_symlinks(false);
    let mut watcher = notify::RecommendedWatcher::new(handler, config)
        .map_err(|err| watch_error(&roots.toplevel, err.to_string()))?;
    let mut worktree_error = None;
    if spec.worktree
        && let Err(err) = watcher.watch(&roots.toplevel, RecursiveMode::Recursive)
    {
        if !spec.store {
            return Err(watch_error(&roots.toplevel, err.to_string()));
        }
        log::warn!(
            "watching {} failed, watching the review store only: {err}",
            roots.toplevel.display()
        );
        // A recursive watch that failed partway (inotify's limit) may have registered part of
        // the tree; none of it is reported.
        let _ = watcher.unwatch(&roots.toplevel);
        worktree_error = Some(err.to_string());
        spec.worktree = false;
    }
    let info_dir = roots.common_dir.join("info");
    if spec.worktree
        && !info_dir.starts_with(&roots.toplevel)
        && info_dir.is_dir()
        && let Err(err) = watcher.watch(&info_dir, RecursiveMode::NonRecursive)
    {
        // Edits to `info/exclude` then apply from the next watch on.
        log::warn!("watching {} failed: {err}", info_dir.display());
    }
    let store_inside_worktree = spec.worktree && roots.store_root.starts_with(&roots.toplevel);
    if spec.store && !store_inside_worktree {
        watcher
            .watch(&roots.store_root, RecursiveMode::Recursive)
            .map_err(|err| watch_error(&roots.store_root, err.to_string()))?;
    }

    let stop = Arc::new(AtomicBool::new(false));
    let thread = std::thread::Builder::new()
        .name("dv-watch".to_string())
        .spawn({
            let stop = Arc::clone(&stop);
            let roots = roots.clone();
            move || run(roots, env, spec, stop, rx, sink)
        })
        .map_err(|err| watch_error(&roots.toplevel, err.to_string()))?;
    let handle = WatchHandle::new(Guard {
        stop,
        tx,
        watcher: Some(watcher),
        thread: Some(thread),
    });
    Ok(match worktree_error {
        Some(message) => handle.with_worktree_error(message),
        None => handle,
    })
}

/// Stops the watch when dropped: the OS watch first, then the thread, which is joined so that
/// the sink is never called after the handle's drop returns.
struct Guard {
    stop: Arc<AtomicBool>,
    tx: Sender<Msg>,
    watcher: Option<notify::RecommendedWatcher>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        drop(self.watcher.take());
        let _ = self.tx.send(Msg::Stop);
        if let Some(thread) = self.thread.take() {
            // Dropped from inside the sink, the thread is this one: it stops when the sink
            // returns and sees `Msg::Stop`, and joining it here would deadlock.
            if thread.thread().id() != std::thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}

/// The watch thread: loads the ignore rules, then delivers one coalesced batch at a time.
fn run(
    roots: Roots,
    env: Vec<(OsString, Option<OsString>)>,
    spec: WatchSpec,
    stop: Arc<AtomicBool>,
    rx: Receiver<Msg>,
    mut sink: WatchSink,
) {
    // Events that arrive while the rules load wait in the channel.
    let rules = if spec.worktree {
        IgnoreRules::load(&roots, &env, &stop)
    } else {
        IgnoreRules::empty(&roots.toplevel)
    };
    let mut filter = Filter {
        info_exclude: roots.common_dir.join("info").join("exclude"),
        roots,
        spec,
        rules,
        stop,
    };
    loop {
        let first = match rx.recv() {
            Ok(Msg::Fs(event)) => event,
            Ok(Msg::Stop) | Err(_) => return,
        };
        let mut batch = Batch::default();
        filter.add(&mut batch, first);
        let deadline = Instant::now() + COALESCE;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match rx.recv_timeout(left) {
                Ok(Msg::Fs(event)) => filter.add(&mut batch, event),
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
        filter.list_new_dirs(&mut batch);
        if filter.stop.load(Ordering::Relaxed) {
            return;
        }
        if let Some(event) = batch.into_event() {
            sink(event);
        }
    }
}

/// The paths one batch has collected so far.
#[derive(Default)]
struct Batch {
    worktree: BTreeSet<String>,
    store: BTreeSet<String>,
    rescan: bool,
    /// Directories that appeared in this batch, listed before it is delivered.
    new_dirs: Vec<NewDir>,
    /// When the last of `new_dirs` was seen.
    last_new_dir: Option<Instant>,
}

enum NewDir {
    Worktree(String),
    Store(String),
}

impl Batch {
    fn new_dir(&mut self, dir: NewDir) {
        self.new_dirs.push(dir);
        self.last_new_dir = Some(Instant::now());
    }

    fn into_event(self) -> Option<WatchEvent> {
        if self.worktree.len() + self.store.len() > MAX_PATHS {
            return Some(WatchEvent {
                rescan: true,
                ..WatchEvent::default()
            });
        }
        if self.worktree.is_empty() && self.store.is_empty() && !self.rescan {
            return None;
        }
        Some(WatchEvent {
            worktree_paths: self.worktree.into_iter().collect(),
            store_paths: self.store.into_iter().collect(),
            rescan: self.rescan,
        })
    }
}

/// Classifies OS events into a [`Batch`] (module docs).
struct Filter {
    roots: Roots,
    spec: WatchSpec,
    rules: IgnoreRules,
    /// `<common-dir>/info/exclude`, re-read when it changes.
    info_exclude: PathBuf,
    stop: Arc<AtomicBool>,
}

impl Filter {
    fn add(&mut self, batch: &mut Batch, event: notify::Result<notify::Event>) {
        let event = match event {
            Ok(event) => event,
            Err(err) => {
                // A watch error can mean lost events (Windows reports a buffer overflow this
                // way), so consumers rescan rather than miss a change.
                log::warn!("file watcher error, requesting a rescan: {err}");
                batch.rescan = true;
                return;
            }
        };
        if event.need_rescan() {
            batch.rescan = true;
        }
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        let appeared = matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_))
        );
        for path in &event.paths {
            self.classify(batch, path, appeared);
        }
    }

    fn classify(&mut self, batch: &mut Batch, path: &Path, appeared: bool) {
        if let Ok(rel) = path.strip_prefix(&self.roots.store_root) {
            if !self.spec.store {
                return;
            }
            // Dot files aren't store entries: `store_write_atomic`'s temp files start with `.`
            // ([`super::paths::STORE_TEMP_PREFIX`]).
            if let Some(rel) = rel_string(rel)
                && !rel.is_empty()
                && !rel
                    .rsplit('/')
                    .next()
                    .is_some_and(|name| name.starts_with('.'))
            {
                if appeared && is_real_dir(path) {
                    batch.new_dir(NewDir::Store(rel.clone()));
                }
                batch.store.insert(rel);
            }
            return;
        }
        if path == self.info_exclude {
            self.rules.reload_info_exclude(&self.info_exclude);
            return;
        }
        if !self.spec.worktree
            || path.starts_with(&self.roots.git_dir)
            || path.starts_with(&self.roots.common_dir)
        {
            return;
        }
        let Some(rel) = path
            .strip_prefix(&self.roots.toplevel)
            .ok()
            .and_then(rel_string)
        else {
            return;
        };
        // A linked worktree's `.git` file, and nested repositories' git dirs.
        if rel.is_empty() || rel.split('/').any(|component| component == ".git") {
            return;
        }
        if rel == ".gitignore" || rel.ends_with("/.gitignore") {
            self.rules.reload_dir(parent(&rel));
        }
        // The link itself, never its target: a link to a directory is no directory to git
        // (module docs).
        let metadata = fs::symlink_metadata(path).ok();
        let exists = metadata.is_some();
        // A removed path has no type left to look up; it was a directory if it was last seen
        // as an ignored one, so a directory-only pattern (`target/`) still matches it.
        let is_dir = match &metadata {
            Some(metadata) => metadata.is_dir(),
            None => self.rules.ignored_dirs.contains(&rel),
        };
        let ignored = self.rules.is_ignored(&rel, is_dir) && !self.rules.tracked.contains(&rel);
        if exists {
            self.rules.note_type(&rel, is_dir, ignored);
        }
        if ignored {
            return;
        }
        if appeared && exists && is_dir {
            batch.new_dir(NewDir::Worktree(rel.clone()));
        }
        batch.worktree.insert(rel);
    }

    /// Adds what the batch's new directories contain (module docs), loading any `.gitignore`
    /// files they brought on the way.
    fn list_new_dirs(&mut self, batch: &mut Batch) {
        let Some(last) = batch.last_new_dir else {
            return;
        };
        if let Some(wait) = NEW_DIR_SETTLE.checked_sub(last.elapsed()) {
            std::thread::sleep(wait);
        }
        for dir in std::mem::take(&mut batch.new_dirs) {
            match dir {
                NewDir::Worktree(rel) => {
                    self.rules
                        .load_tree(&rel, &self.stop, Some(&mut batch.worktree));
                }
                NewDir::Store(rel) => list_store(&self.roots.store_root, &rel, &mut batch.store),
            }
        }
    }
}

/// Adds every file and directory below the store directory `rel` to `found` (dot files excepted),
/// stopping at [`MAX_PATHS`].
fn list_store(root: &Path, rel: &str, found: &mut BTreeSet<String>) {
    let mut pending = vec![rel.to_string()];
    while let Some(dir) = pending.pop() {
        let mut absolute = root.to_path_buf();
        absolute.extend(dir.split('/'));
        let Ok(entries) = fs::read_dir(&absolute) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            if found.len() > MAX_PATHS {
                return;
            }
            let child = format!("{dir}/{name}");
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                pending.push(child.clone());
            }
            found.insert(child);
        }
    }
}

/// Whether `path` is a directory itself, not a symbolic link to one (or, on Windows, a junction).
fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

/// A path relative to a root as a `/`-separated string; `None` when a component isn't UTF-8 or
/// isn't a plain name.
fn rel_string(rel: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in rel.components() {
        match component {
            Component::Normal(name) => parts.push(name.to_str()?),
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

/// The directory part of a `/`-separated relative path (`""` at the top level).
fn parent(rel: &str) -> &str {
    rel.rfind('/').map_or("", |slash| &rel[..slash])
}

/// git's ignore rules for one worktree (module docs). Paths are `/`-separated and relative to
/// the worktree root.
struct IgnoreRules {
    root: PathBuf,
    /// `core.ignoreCase`, which makes git's pattern matching case-insensitive (the default on
    /// Windows and macOS).
    case_insensitive: bool,
    /// The parsed `.gitignore` of each directory that has one (`""` is the root).
    per_dir: HashMap<String, Gitignore>,
    info_exclude: Gitignore,
    global: Gitignore,
    /// Tracked files that an ignore rule matches; git still reports their changes.
    tracked: HashSet<String>,
    /// Directories ignored by a rule of their own (not just inside an ignored directory), as
    /// last seen on disk. A removed path can't be asked whether it was a directory, and a
    /// directory-only pattern (`target/`) matches only one, so removing `target/` would
    /// otherwise look like a change ([`IgnoreRules::note_type`]).
    ignored_dirs: HashSet<String>,
}

impl IgnoreRules {
    fn empty(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            case_insensitive: false,
            per_dir: HashMap::new(),
            info_exclude: Gitignore::empty(),
            global: Gitignore::empty(),
            tracked: HashSet::new(),
            ignored_dirs: HashSet::new(),
        }
    }

    /// Records what an existing path turned out to be: an ignored directory goes into
    /// [`IgnoreRules::ignored_dirs`] unless it sits inside another ignored directory (whose
    /// own entry covers it), anything else leaves it.
    fn note_type(&mut self, rel: &str, is_dir: bool, ignored: bool) {
        if is_dir && ignored {
            if !self.parent_ignored(rel) {
                self.ignored_dirs.insert(rel.to_string());
            }
        } else {
            self.ignored_dirs.remove(rel);
        }
    }

    /// Reads the configuration through git (with the backend's environment, so the global
    /// excludes file is the one git itself would use), then every `.gitignore` outside ignored
    /// directories, then the tracked-but-ignored files.
    fn load(roots: &Roots, env: &EnvOverlay, stop: &AtomicBool) -> Self {
        let git = |args: &[&str]| {
            let opts = GitOpts::new().with_ok_codes(&[0, 1]);
            match command::run(&roots.toplevel, env, args, &opts) {
                Ok(output) if output.code == 0 => output.stdout_utf8().map(str::to_string),
                Ok(_) => None,
                Err(err) => {
                    log::warn!("reading ignore configuration: {err}");
                    None
                }
            }
        };
        let mut rules = Self::empty(&roots.toplevel);
        rules.case_insensitive = git(&["config", "--bool", "--get", "core.ignorecase"])
            .is_some_and(|value| value.trim() == "true");
        rules.reload_info_exclude(&roots.common_dir.join("info").join("exclude"));
        let global = git(&["config", "--path", "--get", "core.excludesFile"])
            .map(|value| value.trim_end_matches(['\r', '\n']).to_string())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| default_global_excludes(env));
        if let Some(global) = global {
            rules.global = rules.parse(&roots.toplevel, &global);
        }
        rules.load_tree("", stop, None);
        rules.tracked = git(&[
            "ls-files",
            "-z",
            "--cached",
            "--ignored",
            "--exclude-standard",
        ])
        .map(|listing| {
            listing
                .split('\0')
                .filter(|path| !path.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
        rules
    }

    /// Loads the `.gitignore` files of `start` and of every directory below it that isn't
    /// ignored (git doesn't read them inside ignored directories). With `found`, also adds every
    /// path below `start` that isn't ignored, stopping at [`MAX_PATHS`].
    fn load_tree(
        &mut self,
        start: &str,
        stop: &AtomicBool,
        mut found: Option<&mut BTreeSet<String>>,
    ) {
        let mut pending = vec![start.to_string()];
        while let Some(dir) = pending.pop() {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            // A directory replaced by a link since its event (or its parent's listing) isn't
            // entered either. The worktree root itself is what git opened, link or not.
            let absolute = self.absolute(&dir);
            if !dir.is_empty() && !is_real_dir(&absolute) {
                continue;
            }
            self.reload_dir(&dir);
            let Ok(entries) = fs::read_dir(&absolute) else {
                continue;
            };
            for entry in entries.flatten() {
                // `file_type` doesn't follow symlinks, so a link to a directory isn't entered.
                let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                if name == ".git" || (!is_dir && found.is_none()) {
                    continue;
                }
                let rel = if dir.is_empty() {
                    name
                } else {
                    format!("{dir}/{name}")
                };
                if self.is_ignored(&rel, is_dir) && !self.tracked.contains(&rel) {
                    self.note_type(&rel, is_dir, true);
                    continue;
                }
                // Past the cap the batch becomes a rescan; the walk goes on for the rules.
                if let Some(found) = found.as_deref_mut()
                    && found.len() <= MAX_PATHS
                {
                    found.insert(rel.clone());
                }
                if is_dir {
                    pending.push(rel);
                }
            }
        }
    }

    /// Re-reads `dir`'s `.gitignore` (forgetting it when it is gone).
    fn reload_dir(&mut self, dir: &str) {
        let absolute = self.absolute(dir);
        let file = absolute.join(".gitignore");
        if file.is_file() {
            let parsed = self.parse(&absolute, &file);
            self.per_dir.insert(dir.to_string(), parsed);
        } else {
            self.per_dir.remove(dir);
        }
    }

    fn reload_info_exclude(&mut self, file: &Path) {
        self.info_exclude = self.parse(&self.root.clone(), file);
    }

    /// Whether git ignores `rel`: it or one of its parent directories is excluded.
    fn is_ignored(&self, rel: &str, is_dir: bool) -> bool {
        self.parent_ignored(rel) || self.decide(rel, is_dir) == Some(true)
    }

    /// Whether one of `rel`'s parent directories is excluded.
    fn parent_ignored(&self, rel: &str) -> bool {
        rel.match_indices('/')
            .any(|(slash, _)| self.decide(&rel[..slash], true) == Some(true))
    }

    /// The rule that decides `rel` itself: `Some(true)` excluded, `Some(false)` re-included,
    /// `None` no rule matches. Precedence per gitignore(5): the `.gitignore` nearest the path
    /// first, then `info/exclude`, then the global file; within a file, the last match.
    fn decide(&self, rel: &str, is_dir: bool) -> Option<bool> {
        let mut dir = parent(rel);
        loop {
            if let Some(rules) = self.per_dir.get(dir) {
                let below = if dir.is_empty() {
                    rel
                } else {
                    &rel[dir.len() + 1..]
                };
                if let Some(decision) = decision(rules.matched(below, is_dir)) {
                    return Some(decision);
                }
            }
            if dir.is_empty() {
                break;
            }
            dir = parent(dir);
        }
        [&self.info_exclude, &self.global]
            .into_iter()
            .find_map(|rules| decision(rules.matched(rel, is_dir)))
    }

    /// Parses an ignore file whose patterns are relative to `base`; empty when there is no file.
    /// A bad pattern is logged and skipped, as git skips it.
    fn parse(&self, base: &Path, file: &Path) -> Gitignore {
        if !file.is_file() {
            return Gitignore::empty();
        }
        let mut builder = GitignoreBuilder::new(base);
        if let Err(err) = builder.case_insensitive(self.case_insensitive) {
            log::warn!("{}: {err}", file.display());
        }
        if let Some(err) = builder.add(file) {
            log::warn!("{}: {err}", file.display());
        }
        builder.build().unwrap_or_else(|err| {
            log::warn!("{}: {err}", file.display());
            Gitignore::empty()
        })
    }

    fn absolute(&self, rel: &str) -> PathBuf {
        let mut path = self.root.clone();
        if !rel.is_empty() {
            path.extend(rel.split('/'));
        }
        path
    }
}

fn decision<T>(matched: Match<T>) -> Option<bool> {
    match matched {
        Match::Ignore(_) => Some(true),
        Match::Whitelist(_) => Some(false),
        Match::None => None,
    }
}

/// git's default global excludes file when `core.excludesFile` is unset:
/// `$XDG_CONFIG_HOME/git/ignore`, else `$HOME/.config/git/ignore`, with the variables as the
/// backend's git sees them (its overlay first, then this process's environment).
fn default_global_excludes(env: &EnvOverlay) -> Option<PathBuf> {
    let var = |name: &str| -> Option<OsString> {
        let value = match env.iter().rev().find(|(key, _)| key == name) {
            Some((_, value)) => value.clone(),
            None => std::env::var_os(name),
        };
        value.filter(|value| !value.is_empty())
    };
    if let Some(xdg) = var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("git").join("ignore"));
    }
    // git for Windows falls back to the user profile when `HOME` is unset.
    let home = var("HOME").or_else(|| cfg!(windows).then(|| var("USERPROFILE")).flatten())?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("git")
            .join("ignore"),
    )
}

#[cfg(test)]
mod tests;
