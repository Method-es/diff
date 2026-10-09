//! Concurrent writers on one review (PLAN D5): threads and processes appending through the
//! checked operations at once, with no lock, lose nothing.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Barrier;
use std::time::{Duration, Instant};

use super::*;
use crate::backend::LocalBackend;
use crate::git::command::apply_overlay;
use crate::review::ops::{add_thread, mark_viewed, reply, set_thread_state};

/// Threads each writer adds; each also gets a reply and an "addressed".
const THREADS_PER_WRITER: u32 = 12;
const CHILD_TEST: &str = "review::tests::concurrency::review_child_writer";

/// One writer's work: add its threads, reply to and address each, mark its own path viewed.
/// Every step loads the review, checks the change and appends one event, racing every other
/// writer.
fn write_as(store: &ReviewStore, review: &ReviewId, writer: &str) {
    let author = Author::agent(writer);
    for n in 1..=THREADS_PER_WRITER {
        let mut at = anchor(&format!("{writer}.rs"), n);
        at.text = vec![format!("{writer} {n}")];
        let thread = add_thread(store, review, at, &format!("{writer} #{n}"), reviewer())
            .unwrap_or_else(|err| panic!("{writer} #{n}: {err}"));
        reply(
            store,
            review,
            &thread,
            &format!("{writer} reply {n}"),
            author.clone(),
            None,
        )
        .unwrap_or_else(|err| panic!("{writer} reply {n}: {err}"));
        set_thread_state(
            store,
            review,
            &thread,
            ThreadState::Addressed,
            Some(writer),
            author.clone(),
            None,
        )
        .unwrap_or_else(|err| panic!("{writer} address {n}: {err}"));
    }
    mark_viewed(store, review, &format!("{writer}.rs"), true)
        .unwrap_or_else(|err| panic!("{writer} viewed: {err}"));
}

/// One writer process for `concurrent_writers_lose_no_event`: opens the repo the parent names,
/// reports ready, waits for the go signal, writes. Does nothing when run on its own.
#[test]
#[ignore = "run as a child process by concurrent_writers_lose_no_event"]
fn review_child_writer() {
    let (Some(repo), Some(review), Some(writer), Some(gate)) = (
        std::env::var_os("DV_TEST_REVIEW_REPO"),
        std::env::var("DV_TEST_REVIEW_ID").ok(),
        std::env::var("DV_TEST_REVIEW_WRITER").ok(),
        std::env::var_os("DV_TEST_REVIEW_GATE"),
    ) else {
        return;
    };
    let gate = Path::new(&gate);
    // The parent applied the fixture's environment to this process, so default options match.
    let store = ReviewStore::new(Arc::new(LocalBackend::open(Path::new(&repo)).unwrap()));
    let review = ReviewId::parse(&review).unwrap();
    std::fs::write(gate.join(format!("ready-{writer}")), "").unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while !gate.join("go").exists() {
        assert!(Instant::now() < deadline, "no go signal");
        std::thread::sleep(Duration::from_millis(2));
    }
    write_as(&store, &review, &writer);
}

#[test]
fn concurrent_writers_lose_no_event() {
    let (repo, store) = fixture_store();
    let review = store.create(Target::Working, reviewer()).unwrap().id;
    let gate = repo.outside_dir("gate");
    let processes: Vec<String> = (0..2).map(|n| format!("proc{n}")).collect();
    let threads: Vec<String> = (0..4).map(|n| format!("thread{n}")).collect();

    let children: Vec<_> = processes
        .iter()
        .map(|writer| {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args([CHILD_TEST, "--exact", "--ignored", "--test-threads=1"]);
            apply_overlay(&mut command, &repo.env_overlay());
            command
                .env("DV_TEST_REVIEW_REPO", repo.path())
                .env("DV_TEST_REVIEW_ID", review.as_str())
                .env("DV_TEST_REVIEW_WRITER", writer)
                .env("DV_TEST_REVIEW_GATE", &gate)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let deadline = Instant::now() + Duration::from_secs(60);
    while !processes
        .iter()
        .all(|writer| gate.join(format!("ready-{writer}")).exists())
    {
        assert!(Instant::now() < deadline, "child writers never got ready");
        std::thread::sleep(Duration::from_millis(5));
    }

    let barrier = Arc::new(Barrier::new(threads.len() + 1));
    let handles: Vec<_> = threads
        .iter()
        .map(|writer| {
            let (store, barrier, review, writer) = (
                store.clone(),
                Arc::clone(&barrier),
                review.clone(),
                writer.clone(),
            );
            std::thread::spawn(move || {
                barrier.wait();
                write_as(&store, &review, &writer);
            })
        })
        .collect();
    std::fs::write(gate.join("go"), "").unwrap();
    barrier.wait();
    for handle in handles {
        handle.join().unwrap();
    }
    for child in children {
        let output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains(" 1 passed"),
            "{stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let loaded = store.load(&review).unwrap();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    let review = loaded.review;
    let writers: Vec<&String> = processes.iter().chain(&threads).collect();
    let per_writer = THREADS_PER_WRITER as usize;
    assert_eq!(review.threads.len(), writers.len() * per_writer);
    for writer in &writers {
        let path = format!("{writer}.rs");
        let mine: Vec<&Thread> = review
            .threads
            .iter()
            .filter(|thread| thread.anchor.path == path)
            .collect();
        assert_eq!(mine.len(), per_writer, "{writer}");
        for (n, thread) in (1..).zip(&mine) {
            assert_eq!(thread.body, format!("{writer} #{n}"), "own order kept");
            assert_eq!(thread.replies.len(), 1, "{writer} #{n}");
            assert_eq!(thread.replies[0].body, format!("{writer} reply {n}"));
            assert_eq!(thread.state, ThreadState::Addressed);
            assert_eq!(
                thread.last_change.as_ref().unwrap().note.as_deref(),
                Some(writer.as_str())
            );
        }
        assert!(review.is_viewed(1, &path), "{writer}");
    }
    // Every event of every writer is in the log exactly once: 3 per thread, 1 viewed each.
    let events = store.events(&review.id).unwrap();
    assert!(events.warnings.is_empty());
    assert_eq!(
        events.events.len(),
        1 + writers.len() * (3 * per_writer + 1)
    );
}
