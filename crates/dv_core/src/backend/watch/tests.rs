//! Watch tests against real OS watchers. OS watchers deliver late and duplicate events, so every
//! negative assertion first waits for a sentinel write made after the paths under test, then for
//! a second of quiet.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dv_test_support::FixtureRepo;

use crate::backend::{Backend, LocalBackend, WatchEvent, WatchHandle, WatchSpec};
use crate::testing::{open, open_at};

const QUIET: Duration = Duration::from_secs(1);
/// How soon a change is reported once the watch is running: the bound
/// `a_file_write_is_reported_within_two_seconds` holds the watcher to.
const FIRES_WITHIN: Duration = Duration::from_secs(2);
/// How long the other tests wait for an event they only sync on: far past [`FIRES_WITHIN`], so a
/// busy machine's slow git or watcher start never reads as a missing event.
const ARRIVES_WITHIN: Duration = Duration::from_secs(10);

fn start(backend: &LocalBackend, spec: WatchSpec) -> (WatchHandle, Receiver<WatchEvent>) {
    let (tx, rx) = mpsc::channel();
    let handle = backend
        .watch(
            spec,
            Box::new(move |event| {
                let _ = tx.send(event);
            }),
        )
        .unwrap();
    (handle, rx)
}

/// Waits until an event satisfies `wanted` and returns every event seen up to and including it.
fn wait_for(
    rx: &Receiver<WatchEvent>,
    within: Duration,
    wanted: impl Fn(&WatchEvent) -> bool,
) -> Vec<WatchEvent> {
    let deadline = Instant::now() + within;
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(event) => {
                let done = wanted(&event);
                seen.push(event);
                if done {
                    return seen;
                }
            }
            Err(err) => panic!("no matching event within {within:?} ({err:?}); saw {seen:#?}"),
        }
    }
}

/// Collects events until [`QUIET`] passes without one.
fn settle(rx: &Receiver<WatchEvent>) -> Vec<WatchEvent> {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut seen = Vec::new();
    loop {
        match rx.recv_timeout(QUIET) {
            Ok(event) => seen.push(event),
            Err(RecvTimeoutError::Timeout) => return seen,
            Err(RecvTimeoutError::Disconnected) => panic!("the watch stopped"),
        }
        assert!(Instant::now() < deadline, "events never settled: {seen:#?}");
    }
}

fn worktree_paths(events: &[WatchEvent]) -> Vec<&str> {
    events
        .iter()
        .flat_map(|event| event.worktree_paths.iter().map(String::as_str))
        .collect()
}

fn has_worktree_path(path: &'static str) -> impl Fn(&WatchEvent) -> bool {
    move |event| event.worktree_paths.iter().any(|p| p == path)
}

fn has_store_path(path: &'static str) -> impl Fn(&WatchEvent) -> bool {
    move |event| event.store_paths.iter().any(|p| p == path)
}

fn committed(files: &[(&str, &str)]) -> FixtureRepo {
    let repo = FixtureRepo::new();
    repo.write("README.md", "readme\n");
    for (path, contents) in files {
        repo.write(path, contents);
    }
    repo.commit("base");
    repo
}

/// Writes a small file at `rel` below `dir` (a worktree root), creating its parent directories.
fn write_in(dir: &Path, rel: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "x\n").unwrap();
}

#[test]
fn a_file_write_is_reported_within_two_seconds() {
    let repo = committed(&[]);
    let backend = open(&repo);
    let (_handle, rx) = start(&backend, WatchSpec::ALL);
    // A first change warms the watch up (its ignore rules read through git), so the bound
    // below times the report alone.
    repo.write("warm-up.txt", "x\n");
    wait_for(&rx, ARRIVES_WITHIN, has_worktree_path("warm-up.txt"));
    repo.write("src/new.txt", "new\n");
    let events = wait_for(&rx, FIRES_WITHIN, has_worktree_path("src/new.txt"));
    let last = events.last().unwrap();
    assert!(!last.rescan);
    assert!(
        last.worktree_paths.windows(2).all(|pair| pair[0] < pair[1]),
        "sorted and de-duplicated: {last:?}"
    );
}

#[test]
fn files_written_straight_into_new_directories_are_reported() {
    let repo = committed(&[]);
    let backend = open(&repo);
    let (_handle, rx) = start(&backend, WatchSpec::ALL);
    settle(&rx);

    // Each directory is new, so on inotify its watch is registered only after its own event.
    repo.write("a/b/c/deep.txt", "x\n")
        .write("a/b/sibling.txt", "x\n");
    let mut events = wait_for(&rx, ARRIVES_WITHIN, has_worktree_path("a/b/c/deep.txt"));
    events.extend(settle(&rx));
    let paths = worktree_paths(&events);
    for expected in ["a", "a/b", "a/b/c", "a/b/c/deep.txt", "a/b/sibling.txt"] {
        assert!(paths.contains(&expected), "{expected}: {paths:?}");
    }
}

#[test]
fn a_directory_moved_in_reports_its_contents_minus_ignored_ones() {
    let repo = committed(&[(".gitignore", "*.log\n")]);
    let staging = repo.outside_dir("staging");
    let moved = staging.join("moved");
    std::fs::create_dir_all(moved.join("sub")).unwrap();
    for (path, contents) in [
        ("a.txt", "a\n"),
        ("sub/b.txt", "b\n"),
        ("debug.log", "ignored by the root rules\n"),
        (".gitignore", "secret.txt\n"),
        ("secret.txt", "ignored by the rules it brings\n"),
    ] {
        std::fs::write(moved.join(path), contents).unwrap();
    }
    let backend = open(&repo);
    let (_handle, rx) = start(&backend, WatchSpec::ALL);
    settle(&rx);

    std::fs::rename(&moved, repo.path().join("moved")).unwrap();
    let mut events = wait_for(&rx, ARRIVES_WITHIN, has_worktree_path("moved/sub/b.txt"));
    events.extend(settle(&rx));
    let paths = worktree_paths(&events);
    for expected in ["moved", "moved/a.txt", "moved/sub", "moved/.gitignore"] {
        assert!(paths.contains(&expected), "{expected}: {paths:?}");
    }
    for ignored in ["moved/debug.log", "moved/secret.txt"] {
        assert!(!paths.contains(&ignored), "{ignored}: {paths:?}");
    }
}

#[test]
fn ignored_paths_and_the_git_dir_are_not_reported() {
    let repo = committed(&[(".gitignore", "target/\nnode_modules/\n*.log\n")]);
    let backend = open(&repo);
    let (_handle, rx) = start(&backend, WatchSpec::ALL);
    settle(&rx);

    repo.write("target/x", "build output\n")
        .write("target/debug/deep/y", "build output\n")
        .write("node_modules/y", "dependency\n")
        .write("debug.log", "log\n")
        .config("dv.touch", "the git dir changes too");
    repo.write("sentinel.txt", "after the rest\n");
    let mut events = wait_for(&rx, ARRIVES_WITHIN, has_worktree_path("sentinel.txt"));
    events.extend(settle(&rx));

    let paths = worktree_paths(&events);
    assert!(
        paths.iter().all(|path| !path.starts_with("target")
            && !path.starts_with("node_modules")
            && !path.ends_with(".log")
            && !path.starts_with(".git")),
        "{paths:?}"
    );
    assert!(
        events.iter().all(|event| event.store_paths.is_empty()),
        "{events:#?}"
    );
}

#[test]
fn removing_an_ignored_directory_is_not_reported() {
    // Directory-only patterns match only directories, and a removed path can't be asked what it
    // was; a file named like such a pattern is no directory, so its removal still counts.
    let repo = committed(&[(".gitignore", "target/\nnode_modules/\nout/\nbuild/\n")]);
    repo.write("target/debug/x", "build output\n")
        .write("node_modules/pkg/y", "dependency\n")
        .write("build", "a file, not a directory\n");
    let staging = repo.outside_dir("staging");
    let backend = open(&repo);
    let (_handle, rx) = start(&backend, WatchSpec::ALL);
    // An ignored directory that appears while the watch runs.
    repo.write("out/z", "build output\n");
    // The watch reads the ignore rules (a few git calls) before it reports anything, and they
    // learn which ignored paths are directories by listing the tree. Waiting for a write made
    // after `out/z` waits for them, so the directories removed below were there to be listed
    // however slow git is.
    repo.write("ready.txt", "the rules are loaded\n");
    wait_for(&rx, Duration::from_secs(10), has_worktree_path("ready.txt"));
    settle(&rx);

    repo.remove("target").remove("out").remove("build");
    std::fs::rename(
        repo.path().join("node_modules"),
        staging.join("node_modules"),
    )
    .unwrap();
    repo.write("sentinel.txt", "after the rest\n");
    let mut events = wait_for(&rx, ARRIVES_WITHIN, has_worktree_path("sentinel.txt"));
    events.extend(settle(&rx));
    let paths = worktree_paths(&events);
    assert!(paths.contains(&"build"), "{paths:?}");
    for ignored in ["target", "node_modules", "out"] {
        assert!(
            paths.iter().all(|path| !path.starts_with(ignored)),
            "{ignored}: {paths:?}"
        );
    }
}

#[test]
fn store_writes_are_reported_as_store_paths() {
    let repo = committed(&[]);
    let backend = open(&repo);
    let (_handle, rx) = start(&backend, WatchSpec::ALL);
    settle(&rx);

    backend
        .store_append("reviews/r-1.jsonl", br#"{"kind":"created"}"#)
        .unwrap();
    let mut events = wait_for(&rx, ARRIVES_WITHIN, has_store_path("reviews/r-1.jsonl"));
    backend
        .store_write_atomic("reviews/r-1.state.json", b"{}")
        .unwrap();
    events.extend(wait_for(
        &rx,
        ARRIVES_WITHIN,
        has_store_path("reviews/r-1.state.json"),
    ));
    events.extend(settle(&rx));

    for event in &events {
        assert!(
            event.worktree_paths.is_empty(),
            "store writes aren't worktree changes: {event:?}"
        );
        assert!(
            event
                .store_paths
                .iter()
                .all(|path| path.split('/').all(|part| !part.starts_with('.'))),
            "temp files aren't reported: {event:?}"
        );
    }
}

#[test]
fn a_store_only_watch_ignores_the_worktree() {
    let repo = committed(&[]);
    let backend = open(&repo);
    let (_handle, rx) = start(&backend, WatchSpec::STORE);
    repo.write("ignored-by-spec.txt", "x\n");
    backend.store_append("reviews/r-2.jsonl", b"{}").unwrap();
    let mut events = wait_for(&rx, ARRIVES_WITHIN, has_store_path("reviews/r-2.jsonl"));
    events.extend(settle(&rx));
    assert!(worktree_paths(&events).is_empty(), "{events:#?}");
}

#[test]
fn a_linked_worktree_watches_the_shared_store() {
    let repo = committed(&[]);
    let worktree = repo.add_worktree("wt", "feat");
    let linked = open_at(&repo, &worktree);
    let (_handle, rx) = start(&linked, WatchSpec::ALL);

    // Written through the main worktree's backend, into the common dir outside this worktree.
    open(&repo)
        .store_append("reviews/r-3.jsonl", b"{}")
        .unwrap();
    wait_for(&rx, ARRIVES_WITHIN, has_store_path("reviews/r-3.jsonl"));
    std::fs::write(worktree.join("in-the-worktree.txt"), "x\n").unwrap();
    wait_for(
        &rx,
        ARRIVES_WITHIN,
        has_worktree_path("in-the-worktree.txt"),
    );
}

#[test]
fn a_worktree_the_os_cant_watch_leaves_the_store_watched() {
    // On Linux the usual cause is inotify's per-user watch limit; here the worktree is gone.
    let repo = committed(&[]);
    let worktree = repo.add_worktree("wt", "feat");
    let linked = open_at(&repo, &worktree);
    assert!(
        linked
            .watch(WatchSpec::ALL, Box::new(|_| {}))
            .unwrap()
            .worktree_error()
            .is_none()
    );
    std::fs::remove_dir_all(&worktree).unwrap();

    let (handle, rx) = start(&linked, WatchSpec::ALL);
    assert!(handle.worktree_error().is_some(), "{handle:?}");
    open(&repo)
        .store_append("reviews/r-5.jsonl", b"{}")
        .unwrap();
    let mut events = wait_for(&rx, ARRIVES_WITHIN, has_store_path("reviews/r-5.jsonl"));
    events.extend(settle(&rx));
    assert!(worktree_paths(&events).is_empty(), "{events:#?}");
    // A watch of the worktree alone has nothing to fall back on, so it fails.
    assert!(matches!(
        linked.watch(WatchSpec::WORKTREE, Box::new(|_| {})),
        Err(crate::backend::BackendError::Watch { .. })
    ));
}

#[test]
fn dropping_the_handle_stops_events() {
    let repo = committed(&[]);
    let backend = open(&repo);
    let (handle, rx) = start(&backend, WatchSpec::ALL);
    repo.write("before.txt", "x\n");
    wait_for(&rx, ARRIVES_WITHIN, has_worktree_path("before.txt"));

    drop(handle);
    // Everything delivered before the drop returned may still be queued.
    while rx.try_recv().is_ok() {}
    repo.write("after.txt", "x\n");
    backend.store_append("reviews/r-4.jsonl", b"{}").unwrap();
    // The sink was dropped with the watch thread, so the channel ends without another event.
    assert_eq!(
        rx.recv_timeout(QUIET).map(|event| format!("{event:?}")),
        Err(RecvTimeoutError::Disconnected)
    );
}

#[test]
fn dropping_the_handle_from_inside_its_sink_stops_the_watch() {
    // A sink may decide the watch is no longer needed and drop the handle itself, on the watch
    // thread, which then must not join itself: the drop returns, and the watch stops once the
    // sink does (`WatchHandle`'s contract). A self-join would hang, so the test waits with a
    // bound instead of on the sink.
    let repo = committed(&[]);
    let backend = open(&repo);
    let slot: Arc<Mutex<Option<WatchHandle>>> = Arc::default();
    let (events_tx, events) = mpsc::channel();
    let (dropped_tx, dropped) = mpsc::channel();
    let handle = backend
        .watch(WatchSpec::ALL, {
            let slot = Arc::clone(&slot);
            Box::new(move |event| {
                let _ = events_tx.send(event);
                let handle = slot.lock().unwrap().take();
                if let Some(handle) = handle {
                    drop(handle);
                    let _ = dropped_tx.send(());
                }
            })
        })
        .unwrap();
    *slot.lock().unwrap() = Some(handle);

    repo.write("first.txt", "x\n");
    dropped
        .recv_timeout(ARRIVES_WITHIN)
        .expect("the sink's drop of its own handle returned");
    // Everything the sink saw up to the drop.
    while events.try_recv().is_ok() {}
    repo.write("after.txt", "x\n");
    backend.store_append("reviews/r-6.jsonl", b"{}").unwrap();
    // The watch thread ended after the sink returned and dropped the sink with it.
    assert_eq!(
        events.recv_timeout(QUIET).map(|event| format!("{event:?}")),
        Err(RecvTimeoutError::Disconnected)
    );
}

/// A stress check of the OS watcher's stop, slow and probabilistic, so run on demand (`cargo
/// test -p dv_core -- --ignored never_wedges`), on Windows above all, whenever `notify` changes:
/// a watch that stops while a change it hasn't delivered yet is queued must not arm another read
/// of changes on the directory handle it closed. On Windows that handle's number may already
/// name a directory another thread of this process is listing, and the read, synchronous on such
/// a handle, wedges that listing and the watcher's own thread for good (notify 8's Windows
/// backend does this). Writers keep changes queued and listers keep opening directories while
/// watches start and stop; every one of them must still finish.
#[test]
#[ignore = "a 30 s stress check of the OS watcher's stop; run on demand"]
fn stopping_watches_while_the_worktree_changes_never_wedges_a_directory_listing() {
    const WRITERS: usize = 4;
    const LISTERS: usize = 8;
    let repo = committed(&[]);
    for n in 0..WRITERS {
        write_in(repo.path(), &format!("churn/{n}/keep.txt"));
    }
    for n in 0..LISTERS {
        write_in(repo.path(), &format!("listed/{n}/keep.txt"));
    }
    let backend = Arc::new(open(&repo));
    let stop = Arc::new(AtomicBool::new(false));
    let (done_tx, done) = mpsc::channel();
    for n in 0..WRITERS {
        let (stop, done_tx) = (Arc::clone(&stop), done_tx.clone());
        let dir = repo.path().join(format!("churn/{n}"));
        std::thread::spawn(move || {
            let mut i = 0u32;
            while !stop.load(Ordering::Relaxed) {
                // A write another process holds up for a moment (a virus scanner) is no wedge.
                let _ = std::fs::write(dir.join(format!("f{}.txt", i % 20)), i.to_string());
                i += 1;
            }
            let _ = done_tx.send(());
        });
    }
    for n in 0..LISTERS {
        let (stop, done_tx) = (Arc::clone(&stop), done_tx.clone());
        let dir = repo.path().join(format!("listed/{n}"));
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    entries.for_each(drop);
                }
            }
            let _ = done_tx.send(());
        });
    }
    let (stopped_tx, stopped) = mpsc::channel();
    let watchers: Vec<_> = (0..8u64)
        .map(|n| {
            let (backend, stop) = (Arc::clone(&backend), Arc::clone(&stop));
            let ended = Ended(stopped_tx.clone());
            std::thread::spawn(move || {
                let _ended = ended;
                let mut round = n;
                while !stop.load(Ordering::Relaxed) {
                    let handle = backend
                        .watch(WatchSpec::WORKTREE, Box::new(|_| {}))
                        .unwrap();
                    std::thread::sleep(Duration::from_millis(2 + round % 5));
                    drop(handle);
                    round += 1;
                }
            })
        })
        .collect();
    std::thread::sleep(Duration::from_secs(30));
    stop.store(true, Ordering::Relaxed);
    for _ in 0..WRITERS + LISTERS {
        done.recv_timeout(Duration::from_secs(10))
            .expect("a directory listing or a write is wedged");
    }
    for _ in 0..watchers.len() {
        stopped
            .recv_timeout(Duration::from_secs(10))
            .expect("a watch's start or stop is wedged");
    }
    // Every watcher thread has ended: the joins only pass on a panic.
    for watcher in watchers {
        watcher.join().unwrap();
    }
}

/// Says on its channel that its thread ended, by a panic too, when dropped.
struct Ended(mpsc::Sender<()>);

impl Drop for Ended {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

#[test]
fn gitignore_edits_apply_to_later_events() {
    let repo = committed(&[]);
    let backend = open(&repo);
    let (_handle, rx) = start(&backend, WatchSpec::ALL);
    settle(&rx);

    repo.write(".gitignore", "logs/\n");
    wait_for(&rx, ARRIVES_WITHIN, has_worktree_path(".gitignore"));
    repo.write("logs/a.txt", "x\n");
    repo.write("sentinel.txt", "after the rest\n");
    let mut events = wait_for(&rx, ARRIVES_WITHIN, has_worktree_path("sentinel.txt"));
    events.extend(settle(&rx));
    let paths = worktree_paths(&events);
    assert!(
        paths.iter().all(|path| !path.starts_with("logs")),
        "{paths:?}"
    );
}

#[test]
fn nested_gitignores_and_re_includes_follow_git() {
    let repo = committed(&[
        (".gitignore", "*.gen\n"),
        ("pkg/.gitignore", "!keep.gen\nout/\n"),
    ]);
    let backend = open(&repo);
    let (_handle, rx) = start(&backend, WatchSpec::ALL);
    settle(&rx);

    repo.write("top.gen", "x\n")
        .write("pkg/other.gen", "x\n")
        .write("pkg/out/x.txt", "x\n")
        .write("pkg/keep.gen", "x\n");
    repo.write("sentinel.txt", "after the rest\n");
    let mut events = wait_for(&rx, ARRIVES_WITHIN, has_worktree_path("sentinel.txt"));
    events.extend(settle(&rx));
    let paths = worktree_paths(&events);
    assert!(
        paths.contains(&"pkg/keep.gen"),
        "re-included by the nearer file: {paths:?}"
    );
    for ignored in ["top.gen", "pkg/other.gen", "pkg/out", "pkg/out/x.txt"] {
        assert!(!paths.contains(&ignored), "{ignored}: {paths:?}");
    }
}

#[test]
fn tracked_files_matching_an_ignore_rule_are_still_reported() {
    let repo = FixtureRepo::new();
    repo.write("build/keep.txt", "tracked\n");
    repo.git(&["add", "-f", "build/keep.txt"]);
    repo.write(".gitignore", "build/\n");
    repo.commit("base");
    let backend = open(&repo);
    let (_handle, rx) = start(&backend, WatchSpec::ALL);
    settle(&rx);

    repo.write("build/keep.txt", "changed\n")
        .write("build/untracked.txt", "x\n");
    repo.write("sentinel.txt", "after the rest\n");
    let mut events = wait_for(&rx, ARRIVES_WITHIN, has_worktree_path("sentinel.txt"));
    events.extend(settle(&rx));
    let paths = worktree_paths(&events);
    assert!(paths.contains(&"build/keep.txt"), "{paths:?}");
    assert!(!paths.contains(&"build/untracked.txt"), "{paths:?}");
}

#[test]
fn info_exclude_and_the_global_excludes_file_apply() {
    let repo = committed(&[]);
    repo.write(".git/info/exclude", "secret/\n");
    // The default global excludes file under the fixture's XDG_CONFIG_HOME.
    let global = repo.home().join(".config").join("git");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::write(global.join("ignore"), "*.tmp\n").unwrap();
    let backend = open(&repo);
    let (_handle, rx) = start(&backend, WatchSpec::ALL);
    settle(&rx);

    repo.write("secret/key.txt", "x\n")
        .write("scratch.tmp", "x\n");
    repo.write("sentinel.txt", "after the rest\n");
    let mut events = wait_for(&rx, ARRIVES_WITHIN, has_worktree_path("sentinel.txt"));
    events.extend(settle(&rx));
    let paths = worktree_paths(&events);
    assert!(
        paths
            .iter()
            .all(|path| !path.starts_with("secret") && !path.ends_with(".tmp")),
        "{paths:?}"
    );
}

#[test]
fn core_excludes_file_replaces_the_default_global_excludes_file() {
    let repo = committed(&[]);
    // `--path` expands the `~`.
    std::fs::write(repo.home().join("my-ignore"), "*.ex\n").unwrap();
    std::fs::write(
        repo.home().join(".gitconfig"),
        "[core]\n\texcludesFile = ~/my-ignore\n",
    )
    .unwrap();
    // The default file under XDG_CONFIG_HOME, which git no longer reads.
    let xdg = repo.home().join(".config").join("git");
    std::fs::create_dir_all(&xdg).unwrap();
    std::fs::write(xdg.join("ignore"), "*.xdg\n").unwrap();
    let backend = open(&repo);
    let (_handle, rx) = start(&backend, WatchSpec::ALL);
    settle(&rx);

    repo.write("a.ex", "x\n").write("b.xdg", "x\n");
    repo.write("sentinel.txt", "after the rest\n");
    let mut events = wait_for(&rx, ARRIVES_WITHIN, has_worktree_path("sentinel.txt"));
    events.extend(settle(&rx));
    let paths = worktree_paths(&events);
    assert!(!paths.contains(&"a.ex"), "{paths:?}");
    assert!(paths.contains(&"b.xdg"), "{paths:?}");
    // git agrees.
    let ignored = |path: &str| {
        repo.git_command()
            .args(["check-ignore", "-q", path])
            .status()
            .unwrap()
            .success()
    };
    assert!(ignored("a.ex") && !ignored("b.xdg"));
}

#[test]
fn info_exclude_edits_apply_to_later_events() {
    // `<common-dir>/info/exclude` is shared by every worktree; a linked worktree's lies outside
    // its own tree.
    let repo = committed(&[]);
    let worktree = repo.add_worktree("wt", "feat");
    for (at, dir) in [
        (repo.path().to_path_buf(), "late-main"),
        (worktree, "late-wt"),
    ] {
        let backend = open_at(&repo, &at);
        let (_handle, rx) = start(&backend, WatchSpec::ALL);
        settle(&rx);

        repo.write(".git/info/exclude", format!("{dir}/\n"));
        // The edit is not a worktree change; its own event only reloads the rules.
        settle(&rx);
        write_in(&at, &format!("{dir}/x"));
        write_in(&at, "sentinel.txt");
        let mut events = wait_for(&rx, ARRIVES_WITHIN, has_worktree_path("sentinel.txt"));
        events.extend(settle(&rx));
        let paths = worktree_paths(&events);
        assert!(paths.iter().all(|path| !path.starts_with(dir)), "{paths:?}");
    }
}

#[test]
fn core_ignorecase_decides_whether_patterns_match_another_case() {
    for (ignorecase, reported) in [("true", false), ("false", true)] {
        let repo = committed(&[(".gitignore", "target/\n")]);
        repo.config("core.ignorecase", ignorecase);
        let backend = open(&repo);
        let (_handle, rx) = start(&backend, WatchSpec::ALL);
        settle(&rx);

        repo.write("Target/x", "build output\n");
        repo.write("sentinel.txt", "after the rest\n");
        let mut events = wait_for(&rx, ARRIVES_WITHIN, has_worktree_path("sentinel.txt"));
        events.extend(settle(&rx));
        let paths = worktree_paths(&events);
        assert_eq!(
            paths.contains(&"Target/x"),
            reported,
            "ignorecase={ignorecase}: {paths:?}"
        );
        // git agrees.
        let ignored = repo
            .git_command()
            .args(["check-ignore", "-q", "Target/x"])
            .status()
            .unwrap()
            .success();
        assert_eq!(ignored, !reported, "ignorecase={ignorecase}");
    }
}

/// Creates a symbolic link to a directory; `false` when the OS refuses (Windows without
/// Developer Mode or the symlink privilege), which skips the calling test.
fn symlink_dir(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    let created = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let created = std::os::windows::fs::symlink_dir(target, link);
    match created {
        Ok(()) => true,
        // ERROR_PRIVILEGE_NOT_HELD
        Err(err) if cfg!(windows) && err.raw_os_error() == Some(1314) => {
            eprintln!("skipped: can't create a symbolic link here ({err})");
            false
        }
        Err(err) => panic!("linking {} to {}: {err}", link.display(), target.display()),
    }
}

#[test]
fn a_symbolic_link_to_a_directory_is_one_path_as_in_git() {
    // git stores a link as its target path and never descends into it, and a directory-only
    // pattern (`foo/`) doesn't match a link (PLAN §5.3).
    let repo = committed(&[(".gitignore", "foo/\n")]);
    let target = repo.outside_dir("target-tree");
    std::fs::create_dir_all(target.join("sub").join("deeper")).unwrap();
    std::fs::write(target.join("top.txt"), "t\n").unwrap();
    std::fs::write(target.join("sub").join("deeper").join("f.txt"), "f\n").unwrap();
    // A link that exists when the watch starts.
    if !symlink_dir(&target, &repo.path().join("early")) {
        return;
    }
    let backend = open(&repo);
    let (_handle, rx) = start(&backend, WatchSpec::ALL);
    settle(&rx);

    // Links that appear while the watch runs, and a change inside their target.
    assert!(symlink_dir(&target, &repo.path().join("sym")));
    assert!(symlink_dir(&target, &repo.path().join("foo")));
    std::fs::write(target.join("top.txt"), "changed through the target\n").unwrap();
    repo.write("sentinel.txt", "after the rest\n");
    let mut events = wait_for(&rx, ARRIVES_WITHIN, has_worktree_path("sentinel.txt"));
    events.extend(settle(&rx));
    let paths = worktree_paths(&events);
    for link in ["sym", "foo"] {
        assert!(paths.contains(&link), "{link}: {paths:?}");
    }
    for link in ["early/", "sym/", "foo/"] {
        assert!(
            paths.iter().all(|path| !path.starts_with(link)),
            "{link}: {paths:?}"
        );
    }
    // git agrees.
    assert_eq!(
        repo.git(&["status", "--porcelain", "-uall"]),
        "?? early\n?? foo\n?? sentinel.txt\n?? sym\n"
    );
}

fn git_changed(event: &WatchEvent) -> bool {
    event.git_changed
}

/// Runs git in `dir` (a linked worktree, say) with the fixture's environment.
fn git_in(repo: &FixtureRepo, dir: &Path, args: &[&str]) {
    let output = repo
        .git_command()
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Creates and removes `rel` below `dir`: a lock git took and gave up again.
fn lock_churn(dir: &Path, rel: &str) {
    let path = dir.join(rel);
    std::fs::write(&path, "in progress\n").unwrap();
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn a_ref_move_without_a_worktree_change_fires_git_changed() {
    let repo = committed(&[]);
    let backend = open(&repo);
    let (_handle, rx) = start(&backend, WatchSpec::ALL);
    settle(&rx);

    // A new branch, then a commit that changes no file: only refs move.
    repo.branch("feature");
    let mut events = wait_for(&rx, ARRIVES_WITHIN, git_changed);
    events.extend(settle(&rx));
    repo.git(&["commit", "-q", "--allow-empty", "-m", "nothing"]);
    events.extend(wait_for(&rx, ARRIVES_WITHIN, git_changed));
    events.extend(settle(&rx));
    assert!(worktree_paths(&events).is_empty(), "{events:#?}");
    assert!(
        events.iter().all(|event| event.store_paths.is_empty()),
        "{events:#?}"
    );
}

#[test]
fn staging_fires_git_changed() {
    let repo = committed(&[]);
    let backend = open(&repo);
    let (_handle, rx) = start(&backend, WatchSpec::ALL);
    repo.write("staged.txt", "x\n");
    let mut events = wait_for(&rx, ARRIVES_WITHIN, has_worktree_path("staged.txt"));
    events.extend(settle(&rx));
    assert!(!events.iter().any(git_changed), "{events:#?}");

    repo.git(&["add", "staged.txt"]);
    let events = wait_for(&rx, ARRIVES_WITHIN, git_changed);
    assert!(worktree_paths(&events).is_empty(), "{events:#?}");
}

#[test]
fn dv_refs_and_lock_churn_do_not_fire_git_changed() {
    let repo = committed(&[]);
    let backend = open(&repo);
    let (_handle, rx) = start(&backend, WatchSpec::ALL);
    settle(&rx);

    // A round snapshot's ref (PLAN §2.3), created, moved and deleted.
    let head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
    repo.git(&["update-ref", "refs/dv/reviews/r-x/1", &head]);
    let other = repo
        .git(&["commit-tree", "-m", "other", &format!("{head}^{{tree}}")])
        .trim()
        .to_string();
    repo.git(&["update-ref", "refs/dv/reviews/r-x/1", &other]);
    repo.git(&["update-ref", "-d", "refs/dv/reviews/r-x/1"]);
    // Locks that come and go without their final name changing.
    let git_dir = repo.path().join(".git");
    for lock in [
        "index.lock",
        "HEAD.lock",
        "packed-refs.lock",
        "refs/heads/main.lock",
    ] {
        lock_churn(&git_dir, lock);
    }
    repo.write("sentinel.txt", "after the rest\n");
    let mut events = wait_for(&rx, ARRIVES_WITHIN, has_worktree_path("sentinel.txt"));
    events.extend(settle(&rx));
    assert!(!events.iter().any(git_changed), "{events:#?}");
}

#[test]
fn a_git_only_watch_reports_the_git_state_alone() {
    let repo = committed(&[]);
    let backend = open(&repo);
    let (_handle, rx) = start(&backend, WatchSpec::GIT);
    repo.write("not-watched.txt", "x\n");
    backend.store_append("reviews/r-7.jsonl", b"{}").unwrap();
    repo.branch("feature");
    let mut events = wait_for(&rx, ARRIVES_WITHIN, git_changed);
    events.extend(settle(&rx));
    for event in &events {
        assert!(
            event.worktree_paths.is_empty() && event.store_paths.is_empty(),
            "{events:#?}"
        );
    }
    // The file is untracked, so staging it changes the index.
    repo.git(&["add", "not-watched.txt"]);
    wait_for(&rx, ARRIVES_WITHIN, git_changed);
}

#[test]
fn a_linked_worktree_reports_its_own_head_and_the_shared_refs() {
    let repo = committed(&[]);
    let worktree = repo.add_worktree("wt", "feat");
    let linked = open_at(&repo, &worktree);
    let (_handle, rx) = start(&linked, WatchSpec::ALL);
    settle(&rx);

    // Its own HEAD, in `.git/worktrees/wt/`.
    git_in(&repo, &worktree, &["checkout", "-q", "--detach"]);
    wait_for(&rx, ARRIVES_WITHIN, git_changed);
    settle(&rx);
    // A branch made from the main worktree, in the shared refs.
    repo.branch("from-main");
    wait_for(&rx, ARRIVES_WITHIN, git_changed);
    settle(&rx);

    // The main worktree's own HEAD and index are not this worktree's state.
    repo.checkout("--detach");
    repo.write("staged-in-main.txt", "x\n");
    repo.git(&["add", "staged-in-main.txt"]);
    write_in(&worktree, "sentinel.txt");
    let mut events = wait_for(&rx, ARRIVES_WITHIN, has_worktree_path("sentinel.txt"));
    events.extend(settle(&rx));
    assert!(!events.iter().any(git_changed), "{events:#?}");
}

#[test]
fn git_state_is_head_index_and_refs_outside_dv() {
    let roots = |git_dir: &str| super::Roots {
        toplevel: Path::new("/r").to_path_buf(),
        git_dir: Path::new(git_dir).to_path_buf(),
        common_dir: Path::new("/r/.git").to_path_buf(),
        store_root: Path::new("/r/.git/dv").to_path_buf(),
    };
    let main = roots("/r/.git");
    let linked = roots("/r/.git/worktrees/wt");
    // (path, for the main worktree, for the linked one)
    let cases = [
        ("/r/.git/HEAD", true, false),
        ("/r/.git/index", true, false),
        ("/r/.git/worktrees/wt/HEAD", false, true),
        ("/r/.git/worktrees/wt/index", false, true),
        ("/r/.git/packed-refs", true, true),
        ("/r/.git/refs/heads/main", true, true),
        ("/r/.git/refs/remotes/origin/main", true, true),
        ("/r/.git/refs/tags/v1", true, true),
        ("/r/.git/refs/heads", true, true),
        ("/r/.git/reftable/tables.list", true, true),
        ("/r/.git/refs/dv", false, false),
        ("/r/.git/refs/dv/reviews/r-x/1", false, false),
        ("/r/.git/refs/dvx", true, true),
        ("/r/.git/refs", false, false),
        ("/r/.git/index.lock", false, false),
        ("/r/.git/HEAD.lock", false, false),
        ("/r/.git/packed-refs.lock", false, false),
        ("/r/.git/refs/heads/main.lock", false, false),
        ("/r/.git/reftable/tables.list.lock", false, false),
        ("/r/.git/ORIG_HEAD", false, false),
        ("/r/.git/FETCH_HEAD", false, false),
        ("/r/.git/config", false, false),
        ("/r/.git/logs/HEAD", false, false),
        ("/r/.git/logs/refs/heads/main", false, false),
        ("/r/.git/objects/ab/cdef", false, false),
        ("/r/.git/worktrees/wt/refs/bisect/bad", false, false),
        ("/r/src/HEAD", false, false),
    ];
    for (path, in_main, in_linked) in cases {
        let path = Path::new(path);
        assert_eq!(super::is_git_state(&main, path), in_main, "main: {path:?}");
        assert_eq!(
            super::is_git_state(&linked, path),
            in_linked,
            "linked: {path:?}"
        );
    }
}

#[test]
fn an_oversized_batch_becomes_a_rescan() {
    let mut batch = super::Batch::default();
    batch
        .worktree
        .extend((0..=super::MAX_PATHS).map(|n| format!("f{n}")));
    let event = batch.into_event().unwrap();
    assert!(event.rescan);
    assert!(event.worktree_paths.is_empty() && event.store_paths.is_empty());

    assert_eq!(super::Batch::default().into_event(), None);
    let git_only = super::Batch {
        git_changed: true,
        ..super::Batch::default()
    };
    assert_eq!(
        git_only.into_event(),
        Some(WatchEvent {
            git_changed: true,
            ..WatchEvent::default()
        })
    );
}
