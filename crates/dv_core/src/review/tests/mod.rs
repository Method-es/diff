//! Tests for the review store (docs/PLAN.md §7): ids, the log format, the fold's rules, the
//! store on fixture repos, the checked operations and concurrent writers.

mod anchor;
mod concurrency;
mod event;
mod fold;
mod id;
mod ops;
mod round;
mod snapshot;
mod store;

use std::sync::Arc;

use dv_test_support::FixtureRepo;

use super::*;
use crate::git::Oid;
use crate::target::Target;
use crate::testing::open;

/// The reviewer every test acts as.
fn reviewer() -> Author {
    Author::reviewer("Fixture Reviewer")
}

fn agent() -> Author {
    Author::agent("claude-code")
}

/// An event with a fresh id at time `at`.
fn event_at(at: u64, body: EventBody) -> Event {
    Event {
        at,
        ..Event::new(body)
    }
}

fn created(target: Target) -> EventBody {
    EventBody::Created {
        target,
        author: reviewer(),
        worktree: "/repo".to_string(),
    }
}

/// A one-line anchor on the new side of `path` in round 1.
fn anchor(path: &str, line: u32) -> Anchor {
    Anchor {
        round: 1,
        path: path.to_string(),
        old_path: None,
        side: AnchorSide::New,
        start_line: line,
        end_line: line,
        text: vec![format!("line {line}")],
        context_before: vec!["before".to_string()],
        context_after: vec!["after".to_string()],
    }
}

fn thread_added(id: &ThreadId, author: Author) -> EventBody {
    EventBody::ThreadAdded {
        id: id.clone(),
        anchor: anchor("src/lib.rs", 3),
        body: "Why?".to_string(),
        author,
    }
}

fn thread_state(thread: &ThreadId, state: ThreadState, author: Author) -> EventBody {
    EventBody::ThreadState {
        thread: thread.clone(),
        state,
        note: None,
        author,
    }
}

/// A commit-looking object id made of one repeated hex digit.
fn oid(digit: char) -> Oid {
    Oid::parse(&digit.to_string().repeat(40)).unwrap()
}

/// The log bytes for `events`, one line each, as the store writes them.
fn log_of(events: &[Event]) -> Vec<u8> {
    let mut log = Vec::new();
    for event in events {
        log.extend(event.to_line().unwrap());
        log.push(b'\n');
    }
    log
}

/// A store on a fixture repo with one commit, opened hermetically.
fn fixture_store() -> (FixtureRepo, ReviewStore) {
    let repo = FixtureRepo::new();
    repo.write("src/lib.rs", "one\ntwo\nthree\n");
    repo.commit("base");
    let store = ReviewStore::new(Arc::new(open(&repo)));
    (repo, store)
}
