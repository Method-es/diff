//! `Backend::watch` and the `WatchHandle` contract (PLAN §5.3): dropping the handle stops the
//! watch before `drop` returns, the sink dropped by then (a sink in progress finished first),
//! except from inside the sink itself. OS watchers deliver late and duplicate events, so a
//! negative assertion first waits for quiet.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dv_core::backend::{Backend, WatchEvent, WatchHandle, WatchSpec};

use crate::{Subject, committed};

crate::conformance!(
    worktree_writes_are_reported,
    store_writes_are_reported,
    git_state_changes_are_reported,
    dropping_the_handle_stops_delivery,
    dropping_the_handle_waits_for_a_sink_in_progress,
    dropping_the_handle_from_inside_its_sink_stops_the_watch,
    two_watches_deliver_independently,
);

const QUIET: Duration = Duration::from_secs(1);
const FIRES_WITHIN: Duration = Duration::from_secs(5);

fn start(backend: &dyn Backend, spec: WatchSpec) -> (WatchHandle, Receiver<WatchEvent>) {
    let (handle, rx, _) = start_flagged(backend, spec);
    (handle, rx)
}

/// Sets its flag when dropped: a sink owning one tells when it is gone.
struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// [`start`], and a flag set once the sink is dropped.
fn start_flagged(
    backend: &dyn Backend,
    spec: WatchSpec,
) -> (WatchHandle, Receiver<WatchEvent>, Arc<AtomicBool>) {
    let (tx, rx) = mpsc::channel();
    let dropped = Arc::new(AtomicBool::new(false));
    let flag = DropFlag(Arc::clone(&dropped));
    let handle = backend
        .watch(
            spec,
            Box::new(move |event| {
                let _flag = &flag;
                let _ = tx.send(event);
            }),
        )
        .unwrap();
    (handle, rx, dropped)
}

/// Asserts the sink is gone now: its flag set, and nothing left but what it delivered before.
fn assert_sink_gone(rx: &Receiver<WatchEvent>, dropped: &AtomicBool) {
    assert!(dropped.load(Ordering::SeqCst), "the sink outlived drop");
    loop {
        match rx.try_recv() {
            Ok(_) => {}
            Err(error) => {
                assert_eq!(error, TryRecvError::Disconnected, "the sink outlived drop");
                return;
            }
        }
    }
}

/// Waits until an event satisfies `wanted` and returns every event seen up to it.
fn wait_for(rx: &Receiver<WatchEvent>, wanted: impl Fn(&WatchEvent) -> bool) -> Vec<WatchEvent> {
    let deadline = Instant::now() + FIRES_WITHIN;
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
            Err(err) => {
                panic!("no matching event within {FIRES_WITHIN:?} ({err:?}); saw {seen:#?}")
            }
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

fn has_worktree_path(path: &'static str) -> impl Fn(&WatchEvent) -> bool {
    move |event| event.worktree_paths.iter().any(|p| p == path)
}

fn has_store_path(path: &'static str) -> impl Fn(&WatchEvent) -> bool {
    move |event| event.store_paths.iter().any(|p| p == path)
}

fn worktree_writes_are_reported(subject: &Subject) {
    let repo = committed();
    repo.write(".gitignore", "*.log\n");
    let backend = subject.open(&repo);
    let (handle, rx) = start(backend.as_ref(), WatchSpec::ALL);
    assert_eq!(handle.worktree_error(), None);
    settle(&rx);

    repo.write("ignored.log", "x\n")
        .write("src/new.txt", "new\n");
    let mut events = wait_for(&rx, has_worktree_path("src/new.txt"));
    events.extend(settle(&rx));
    let last = events
        .iter()
        .find(|event| has_worktree_path("src/new.txt")(event))
        .unwrap();
    assert!(!last.rescan);
    assert!(
        last.worktree_paths.windows(2).all(|pair| pair[0] < pair[1]),
        "sorted and de-duplicated: {last:?}"
    );
    assert!(
        !events.iter().any(has_worktree_path("ignored.log")),
        "ignored paths are dropped: {events:#?}"
    );
}

fn store_writes_are_reported(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let (_handle, rx) = start(backend.as_ref(), WatchSpec::STORE);
    settle(&rx);
    backend.store_append("reviews/r-1.jsonl", b"{}").unwrap();
    let events = wait_for(&rx, has_store_path("reviews/r-1.jsonl"));
    assert!(
        events.iter().all(|event| event.worktree_paths.is_empty()),
        "a store watch reports no worktree paths: {events:#?}"
    );
}

fn git_state_changes_are_reported(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let (_handle, rx) = start(backend.as_ref(), WatchSpec::GIT);
    settle(&rx);
    repo.write("b.txt", "b\n");
    repo.commit("second");
    wait_for(&rx, |event| event.git_changed);
}

fn dropping_the_handle_stops_delivery(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let (handle, rx, dropped) = start_flagged(backend.as_ref(), WatchSpec::ALL);
    repo.write("before.txt", "x\n");
    wait_for(&rx, has_worktree_path("before.txt"));

    drop(handle);
    // Gone when the drop returned: what it delivered before may still be queued, nothing more.
    assert_sink_gone(&rx, &dropped);
    repo.write("after.txt", "x\n");
    backend.store_append("reviews/r-4.jsonl", b"{}").unwrap();
    std::thread::sleep(QUIET);
    assert_eq!(rx.try_recv().err(), Some(TryRecvError::Disconnected));
}

fn dropping_the_handle_waits_for_a_sink_in_progress(subject: &Subject) {
    // A sink still running when the handle drops: the drop returns once it has returned, and
    // the sink is gone by then.
    let repo = committed();
    let backend = subject.open(&repo);
    let dropped = Arc::new(AtomicBool::new(false));
    let returned = Arc::new(AtomicBool::new(false));
    let (inside_tx, inside) = mpsc::channel();
    let handle = backend
        .watch(WatchSpec::ALL, {
            let flag = DropFlag(Arc::clone(&dropped));
            let returned = Arc::clone(&returned);
            let mut first = true;
            Box::new(move |_| {
                let _flag = &flag;
                if std::mem::take(&mut first) {
                    let _ = inside_tx.send(());
                    std::thread::sleep(Duration::from_millis(700));
                    returned.store(true, Ordering::SeqCst);
                }
            })
        })
        .unwrap();
    repo.write("slow.txt", "x\n");
    inside
        .recv_timeout(FIRES_WITHIN)
        .expect("the sink got a batch");
    let started = Instant::now();
    drop(handle);
    assert!(
        returned.load(Ordering::SeqCst),
        "drop returned while the sink ran ({:?})",
        started.elapsed()
    );
    assert!(dropped.load(Ordering::SeqCst), "the sink outlived drop");
}

fn dropping_the_handle_from_inside_its_sink_stops_the_watch(subject: &Subject) {
    // A sink may drop its own handle on the delivery thread, which then must not join itself:
    // the drop returns, and the watch stops once the sink does. A self-join would hang, so the
    // test waits with a bound instead of on the sink.
    let repo = committed();
    let backend = subject.open(&repo);
    let slot: Arc<Mutex<Option<WatchHandle>>> = Arc::default();
    let (events_tx, events) = mpsc::channel();
    let (dropped_tx, dropped) = mpsc::channel();
    let gone = Arc::new(AtomicBool::new(false));
    let handle = backend
        .watch(WatchSpec::ALL, {
            let slot = Arc::clone(&slot);
            let flag = DropFlag(Arc::clone(&gone));
            Box::new(move |event| {
                let _flag = &flag;
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
        .recv_timeout(FIRES_WITHIN)
        .expect("the sink's drop of its own handle returned");
    // The watch stops once that sink returns: then the sink is gone.
    let deadline = Instant::now() + FIRES_WITHIN;
    while !gone.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "the sink outlived its watch");
        std::thread::sleep(Duration::from_millis(10));
    }
    repo.write("after.txt", "x\n");
    backend.store_append("reviews/r-6.jsonl", b"{}").unwrap();
    std::thread::sleep(QUIET);
    while events.try_recv().is_ok() {}
    assert_eq!(events.try_recv().err(), Some(TryRecvError::Disconnected));
}

fn two_watches_deliver_independently(subject: &Subject) {
    let repo = committed();
    let backend = subject.open(&repo);
    let (store, store_rx, store_dropped) = start_flagged(backend.as_ref(), WatchSpec::STORE);
    let (_worktree, worktree_rx) = start(backend.as_ref(), WatchSpec::WORKTREE);
    settle(&store_rx);
    settle(&worktree_rx);

    repo.write("one.txt", "x\n");
    wait_for(&worktree_rx, has_worktree_path("one.txt"));
    drop(store);
    assert_sink_gone(&store_rx, &store_dropped);
    repo.write("two.txt", "x\n");
    wait_for(&worktree_rx, has_worktree_path("two.txt"));
}
