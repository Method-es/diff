//! The fold: events → state, and the rules it enforces in file order (PLAN §2.4, §2.5, §7).

use super::*;
use crate::review::fold::{check_status, check_thread_state};

fn review_id() -> ReviewId {
    ReviewId::parse("r-01j9z3k4m5n6p7q8r9s0t1v2w3").unwrap()
}

/// The events for `bodies`, at times 1, 2, 3, ….
fn events(bodies: Vec<EventBody>) -> Vec<Event> {
    bodies
        .into_iter()
        .enumerate()
        .map(|(n, body)| event_at(n as u64 + 1, body))
        .collect()
}

fn fold_bodies(bodies: Vec<EventBody>) -> Folded {
    fold(&review_id(), &events(bodies)).unwrap()
}

/// The rejection reasons, in order.
fn rejections(folded: &Folded) -> Vec<Rejection> {
    folded
        .warnings
        .iter()
        .map(|warning| match warning {
            LogWarning::Rejected { reason, .. } => reason.clone(),
            other => panic!("{other:?}"),
        })
        .collect()
}

#[test]
fn every_kind_folds_into_the_state() {
    let thread = ThreadId::generate();
    let reply = CommentId::generate();
    let folded = fold_bodies(vec![
        created(Target::Working),
        EventBody::Viewed {
            round: 1,
            path: "a.txt".to_string(),
            on: true,
        },
        thread_added(&thread, reviewer()),
        EventBody::ReplyAdded {
            thread: thread.clone(),
            id: reply.clone(),
            body: "On it.".to_string(),
            author: agent(),
        },
        EventBody::Status {
            status: ReviewStatus::ChangesRequested,
        },
        EventBody::RoundClosed {
            n: 1,
            new_oid: oid('b'),
            old_oid: oid('a'),
            verdict: Some(Verdict::RequestChanges),
        },
        EventBody::ThreadState {
            thread: thread.clone(),
            state: ThreadState::Addressed,
            note: Some("renamed".to_string()),
            author: agent(),
        },
        EventBody::BodyEdited {
            target_id: ItemId::Comment(reply.clone()),
            body: "Done.".to_string(),
        },
        EventBody::BodyEdited {
            target_id: ItemId::Thread(thread.clone()),
            body: "Why this name?".to_string(),
        },
        EventBody::PrLinked {
            owner: "kylekz".to_string(),
            repo: "difftest".to_string(),
            number: 4,
        },
        EventBody::Status {
            status: ReviewStatus::Ready,
        },
        EventBody::Viewed {
            round: 2,
            path: "b.txt".to_string(),
            on: true,
        },
    ]);
    assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
    let review = folded.review;
    assert_eq!(review.id, review_id());
    assert_eq!(review.target, Target::Working);
    assert_eq!(review.worktree, "/repo");
    assert_eq!(review.author, reviewer());
    assert_eq!((review.created_at, review.updated_at), (1, 12));
    assert_eq!(review.status, ReviewStatus::Ready);
    assert_eq!(
        review.rounds,
        [Round {
            n: 1,
            old_oid: oid('a'),
            new_oid: oid('b'),
            verdict: Some(Verdict::RequestChanges),
            closed_at: 6,
        }]
    );
    assert_eq!(review.current_round(), 2);
    assert!(review.is_viewed(1, "a.txt") && !review.is_viewed(2, "a.txt"));
    assert!(review.is_viewed(2, "b.txt"));
    assert_eq!(
        review.pr,
        Some(PrLink {
            owner: "kylekz".to_string(),
            repo: "difftest".to_string(),
            number: 4
        })
    );

    let [t] = &review.threads[..] else {
        panic!("{:?}", review.threads);
    };
    assert_eq!(t.id, thread);
    assert_eq!(t.body, "Why this name?");
    assert_eq!((t.created_at, t.edited_at), (3, Some(9)));
    assert_eq!(t.author, reviewer());
    assert_eq!(t.anchor, anchor("src/lib.rs", 3));
    assert_eq!(t.state, ThreadState::Addressed);
    assert_eq!(
        t.last_change,
        Some(StateChange {
            state: ThreadState::Addressed,
            note: Some("renamed".to_string()),
            author: agent(),
            at: 7,
        })
    );
    let [r] = &t.replies[..] else {
        panic!("{:?}", t.replies);
    };
    assert_eq!((r.id.clone(), r.body.as_str()), (reply, "Done."));
    assert_eq!(
        (r.created_at, r.edited_at, r.deleted_at),
        (4, Some(8), None)
    );
    assert_eq!(r.author, agent());

    let summary = review.summary();
    assert_eq!(summary.current_round, 2);
    assert_eq!(summary.threads.addressed, 1);
    assert_eq!(summary.threads.open + summary.threads.resolved, 0);
}

#[test]
fn a_pr_review_starts_as_a_draft_and_submission_makes_it_read_only() {
    let pr = Target::Pr {
        owner: "kylekz".to_string(),
        repo: "difftest".to_string(),
        number: 9,
        base_oid: oid('a'),
        head_oid: oid('b'),
    };
    let thread = ThreadId::generate();
    let folded = fold_bodies(vec![
        created(pr),
        thread_added(&thread, reviewer()),
        EventBody::Submitted {
            github_review_id: 77,
            url: "https://example.invalid/r/77".to_string(),
            event: Verdict::Comment,
        },
        // Everything after the submission is refused, a second submission included.
        EventBody::ReplyAdded {
            thread: thread.clone(),
            id: CommentId::generate(),
            body: "late".to_string(),
            author: agent(),
        },
        EventBody::Viewed {
            round: 1,
            path: "a".to_string(),
            on: true,
        },
        EventBody::Submitted {
            github_review_id: 78,
            url: "https://example.invalid/r/78".to_string(),
            event: Verdict::Approve,
        },
        EventBody::Status {
            status: ReviewStatus::Closed,
        },
    ]);
    assert_eq!(
        rejections(&folded),
        [
            Rejection::Submitted,
            Rejection::Submitted,
            Rejection::Submitted,
            Rejection::Submitted
        ]
    );
    let review = folded.review;
    assert_eq!(review.status, ReviewStatus::Submitted);
    assert!(review.is_pr_review());
    assert_eq!(review.submissions.len(), 1);
    assert_eq!(review.submissions[0].github_review_id, 77);
    assert_eq!(review.submissions[0].at, 3);
    assert_eq!(review.pr.as_ref().map(|pr| pr.number), Some(9));
    assert!(review.threads[0].replies.is_empty());
    assert!(review.viewed.is_empty());
}

#[test]
fn only_a_draft_can_be_submitted() {
    let folded = fold_bodies(vec![
        created(Target::Working),
        EventBody::Submitted {
            github_review_id: 1,
            url: String::new(),
            event: Verdict::Approve,
        },
    ]);
    assert_eq!(
        rejections(&folded),
        [Rejection::NotSubmittable {
            status: ReviewStatus::Open
        }]
    );
    assert!(folded.review.submissions.is_empty());
}

#[test]
fn pr_links_match_a_pr_target_and_can_change_on_a_local_review() {
    let link = |number| EventBody::PrLinked {
        owner: "KyleKZ".to_string(),
        repo: "DiffTest".to_string(),
        number,
    };
    let pr = Target::Pr {
        owner: "kylekz".to_string(),
        repo: "difftest".to_string(),
        number: 9,
        base_oid: oid('a'),
        head_oid: oid('b'),
    };
    let folded = fold_bodies(vec![created(pr), link(9), link(10)]);
    let [Rejection::PrMismatch { target, linked }] = &rejections(&folded)[..] else {
        panic!("{:?}", folded.warnings);
    };
    assert_eq!((target.number, linked.number), (9, 10));
    assert_eq!(folded.review.pr.unwrap().number, 9);
    // Owner and repo compare case-insensitively, so the link above was a no-op.
    assert_eq!(folded.review.updated_at, 1);

    let folded = fold_bodies(vec![created(Target::Working), link(3), link(3), link(4)]);
    assert!(folded.warnings.is_empty());
    assert_eq!(folded.review.pr.unwrap().number, 4);
    assert_eq!(folded.review.updated_at, 4);
}

#[test]
fn events_naming_unknown_ids_are_skipped_with_warnings() {
    let thread = ThreadId::generate();
    let ghost = ThreadId::generate();
    let ghost_reply = CommentId::generate();
    let folded = fold_bodies(vec![
        created(Target::Working),
        thread_added(&thread, reviewer()),
        EventBody::ReplyAdded {
            thread: ghost.clone(),
            id: CommentId::generate(),
            body: "?".to_string(),
            author: agent(),
        },
        thread_state(&ghost, ThreadState::Addressed, agent()),
        EventBody::BodyEdited {
            target_id: ItemId::Comment(ghost_reply.clone()),
            body: "?".to_string(),
        },
        EventBody::Deleted {
            target_id: ItemId::Thread(ghost.clone()),
        },
        // Ids are unique: a second thread or reply with a used id is refused.
        thread_added(&thread, agent()),
    ]);
    assert_eq!(
        rejections(&folded),
        [
            Rejection::UnknownThread {
                thread: ghost.clone()
            },
            Rejection::UnknownThread {
                thread: ghost.clone()
            },
            Rejection::UnknownItem {
                item: ItemId::Comment(ghost_reply)
            },
            Rejection::UnknownThread { thread: ghost },
            Rejection::DuplicateId {
                id: thread.to_string()
            },
        ]
    );
    let review = folded.review;
    assert_eq!(review.threads.len(), 1);
    assert_eq!(review.threads[0].author, reviewer());
    assert_eq!(review.updated_at, 2, "skipped events don't touch the state");
    // Each warning names the event and its kind.
    let LogWarning::Rejected { kind, .. } = &folded.warnings[0] else {
        panic!()
    };
    assert_eq!(kind, "reply_added");
}

#[test]
fn deleted_threads_and_replies_are_tombstones() {
    let thread = ThreadId::generate();
    let other = ThreadId::generate();
    let reply = CommentId::generate();
    let folded = fold_bodies(vec![
        created(Target::Working),
        thread_added(&thread, reviewer()),
        thread_added(&other, reviewer()),
        EventBody::ReplyAdded {
            thread: other.clone(),
            id: reply.clone(),
            body: "x".to_string(),
            author: agent(),
        },
        EventBody::Deleted {
            target_id: ItemId::Comment(reply.clone()),
        },
        EventBody::Deleted {
            target_id: ItemId::Thread(thread.clone()),
        },
        // Later events naming a tombstone are recognised and refused.
        EventBody::BodyEdited {
            target_id: ItemId::Comment(reply.clone()),
            body: "y".to_string(),
        },
        EventBody::Deleted {
            target_id: ItemId::Comment(reply.clone()),
        },
        EventBody::ReplyAdded {
            thread: thread.clone(),
            id: CommentId::generate(),
            body: "z".to_string(),
            author: agent(),
        },
        thread_state(&thread, ThreadState::Resolved, reviewer()),
        EventBody::Deleted {
            target_id: ItemId::Thread(thread.clone()),
        },
        // A thread id is not reusable after deletion either.
        thread_added(&thread, reviewer()),
    ]);
    let deleted_reply = Rejection::Deleted {
        item: ItemId::Comment(reply.clone()),
    };
    let deleted_thread = Rejection::Deleted {
        item: ItemId::Thread(thread.clone()),
    };
    assert_eq!(
        rejections(&folded),
        [
            deleted_reply.clone(),
            deleted_reply,
            deleted_thread.clone(),
            deleted_thread.clone(),
            deleted_thread,
            Rejection::DuplicateId {
                id: thread.to_string()
            },
        ]
    );
    let review = folded.review;
    assert_eq!(review.threads.len(), 2, "tombstones stay in the state");
    assert_eq!(review.threads[0].deleted_at, Some(6));
    assert_eq!(review.threads[0].state, ThreadState::Open);
    assert_eq!(review.threads[1].replies[0].deleted_at, Some(5));
    assert_eq!(review.threads[1].replies[0].body, "x");
    let live: Vec<&ThreadId> = review.live_threads().map(|t| &t.id).collect();
    assert_eq!(live, [&other]);
    assert_eq!(review.summary().threads.open, 1);
}

#[test]
fn replies_in_a_deleted_thread_are_gone_with_it() {
    let thread = ThreadId::generate();
    let reply = CommentId::generate();
    let folded = fold_bodies(vec![
        created(Target::Working),
        thread_added(&thread, reviewer()),
        EventBody::ReplyAdded {
            thread: thread.clone(),
            id: reply.clone(),
            body: "x".to_string(),
            author: agent(),
        },
        EventBody::Deleted {
            target_id: ItemId::Thread(thread.clone()),
        },
        EventBody::BodyEdited {
            target_id: ItemId::Comment(reply),
            body: "y".to_string(),
        },
    ]);
    assert_eq!(
        rejections(&folded),
        [Rejection::Deleted {
            item: ItemId::Thread(thread)
        }]
    );
}

/// Who may move a local thread from one state to another (PLAN §2.5).
#[test]
fn thread_state_transitions_follow_the_table() {
    use AuthorKind::{Agent, Github, Reviewer};
    use ThreadState::{Addressed, Open, Resolved};
    let id = ThreadId::generate();
    let states = [Open, Addressed, Resolved];
    for from in states {
        for to in states {
            for by in [Reviewer, Agent, Github] {
                let expected = match (from, to, by) {
                    (_, _, Github) => "refused",
                    (Open, Open, _) | (Resolved, Resolved, _) => "unchanged",
                    // Agents only address; addressing again replaces the note.
                    (Open | Addressed, Addressed, Agent) => "changed",
                    (_, _, Agent) => "refused",
                    // The reviewer may do anything but address a resolved thread.
                    (Resolved, Addressed, Reviewer) => "refused",
                    (_, _, Reviewer) => "changed",
                };
                let actual = match check_thread_state(&id, Reviewer, from, to, by) {
                    Ok(Outcome::Changed) => "changed",
                    Ok(Outcome::Unchanged) => "unchanged",
                    Err(_) => "refused",
                };
                assert_eq!(actual, expected, "{from} -> {to} by {by}");
            }
        }
    }
    // A thread mirrored from GitHub moves only by GitHub's events.
    for by in [Reviewer, Agent] {
        assert_eq!(
            check_thread_state(&id, Github, Open, Resolved, by),
            Err(Rejection::ReadOnlyThread { thread: id.clone() })
        );
    }
    assert_eq!(
        check_thread_state(&id, Github, Resolved, Open, Github),
        Ok(Outcome::Changed)
    );
    assert_eq!(
        check_thread_state(&id, Github, Open, Open, Github),
        Ok(Outcome::Unchanged)
    );
}

#[test]
fn a_late_address_after_the_reviewer_resolved_is_skipped() {
    // The agent validated "address" against a state where the thread was open, but the
    // reviewer's "resolve" landed first: file order decides, and resolved stays resolved.
    let thread = ThreadId::generate();
    let folded = fold_bodies(vec![
        created(Target::Working),
        thread_added(&thread, reviewer()),
        thread_state(&thread, ThreadState::Resolved, reviewer()),
        thread_state(&thread, ThreadState::Addressed, agent()),
        thread_state(&thread, ThreadState::Open, agent()),
        thread_state(&thread, ThreadState::Open, reviewer()),
        thread_state(&thread, ThreadState::Addressed, agent()),
    ]);
    assert_eq!(
        rejections(&folded),
        [
            Rejection::ThreadTransition {
                from: ThreadState::Resolved,
                to: ThreadState::Addressed,
                author: AuthorKind::Agent
            },
            Rejection::ThreadTransition {
                from: ThreadState::Resolved,
                to: ThreadState::Open,
                author: AuthorKind::Agent
            },
        ]
    );
    let thread = &folded.review.threads[0];
    assert_eq!(thread.state, ThreadState::Addressed);
    assert_eq!(thread.last_change.as_ref().unwrap().at, 7);
}

#[test]
fn github_threads_take_only_github_replies() {
    let mirrored = ThreadId::generate();
    let local = ThreadId::generate();
    let reply = |thread: &ThreadId, author: Author| EventBody::ReplyAdded {
        thread: thread.clone(),
        id: CommentId::generate(),
        body: "x".to_string(),
        author,
    };
    let folded = fold_bodies(vec![
        created(Target::Working),
        thread_added(&mirrored, Author::github("octocat")),
        thread_added(&local, reviewer()),
        reply(&mirrored, Author::github("hubot")),
        reply(&mirrored, reviewer()),
        reply(&local, Author::github("hubot")),
        reply(&local, agent()),
    ]);
    assert_eq!(
        rejections(&folded),
        [
            Rejection::ReadOnlyThread {
                thread: mirrored.clone()
            },
            Rejection::GithubOnLocalThread {
                thread: local.clone()
            },
        ]
    );
    assert_eq!(folded.review.threads[0].replies.len(), 1);
    assert_eq!(folded.review.threads[1].replies.len(), 1);
}

/// Which status moves are allowed (PLAN §2.4).
#[test]
fn status_transitions_follow_the_table() {
    use ReviewStatus::*;
    let all = [
        Open,
        ChangesRequested,
        InProgress,
        Ready,
        Approved,
        Closed,
        Draft,
        Submitted,
    ];
    let active = [Open, ChangesRequested, InProgress, Ready];
    let targets = [ChangesRequested, InProgress, Ready, Approved, Closed];
    for from in all {
        for to in all {
            let expected = if from == to {
                Ok(Outcome::Unchanged)
            } else if (active.contains(&from) && targets.contains(&to))
                || (from == Draft && to == Closed)
            {
                Ok(Outcome::Changed)
            } else {
                Err(Rejection::StatusTransition { from, to })
            };
            assert_eq!(check_status(from, to), expected, "{from} -> {to}");
        }
    }
    for status in [Approved, Closed, Submitted] {
        assert!(status.is_final() && !status.is_open());
    }

    // In a log: a final status sticks, and a local review never becomes a draft.
    let folded = fold_bodies(vec![
        created(Target::Working),
        EventBody::Status { status: Ready },
        EventBody::Status { status: Ready },
        EventBody::Status { status: Draft },
        EventBody::Status { status: Approved },
        EventBody::Status { status: Ready },
    ]);
    assert_eq!(
        rejections(&folded),
        [
            Rejection::StatusTransition {
                from: Ready,
                to: Draft
            },
            Rejection::StatusTransition {
                from: Approved,
                to: Ready
            },
        ]
    );
    assert_eq!(folded.review.status, Approved);
    assert_eq!(
        folded.review.updated_at, 5,
        "the repeated status was a no-op"
    );
}

#[test]
fn rounds_close_in_order_and_viewed_needs_an_existing_round() {
    let round = |n| EventBody::RoundClosed {
        n,
        new_oid: oid('b'),
        old_oid: oid('a'),
        verdict: None,
    };
    let viewed = |round, path: &str, on| EventBody::Viewed {
        round,
        path: path.to_string(),
        on,
    };
    let folded = fold_bodies(vec![
        created(Target::Working),
        round(2),
        round(1),
        round(1),
        viewed(3, "a", true),
        viewed(2, "a", true),
        viewed(2, "a", true),
        viewed(1, "b", true),
        viewed(1, "b", false),
        viewed(2, "../a", true),
        viewed(0, "a", true),
    ]);
    assert_eq!(
        rejections(&folded),
        [
            Rejection::RoundOrder {
                expected: 1,
                got: 2
            },
            Rejection::RoundOrder {
                expected: 2,
                got: 1
            },
            Rejection::UnknownRound {
                round: 3,
                current: 2
            },
            Rejection::InvalidPath {
                path: "../a".to_string()
            },
            Rejection::UnknownRound {
                round: 0,
                current: 2
            },
        ]
    );
    let review = folded.review;
    assert_eq!(review.rounds.len(), 1);
    assert_eq!(review.rounds[0].verdict, None);
    assert!(review.is_viewed(2, "a"));
    assert!(!review.is_viewed(1, "b"));
    assert!(
        !review.viewed.contains_key(&1),
        "an emptied round leaves no entry"
    );
}

#[test]
fn anchors_are_checked() {
    let bad = |edit: fn(&mut Anchor)| {
        let mut anchor = anchor("src/lib.rs", 3);
        edit(&mut anchor);
        EventBody::ThreadAdded {
            id: ThreadId::generate(),
            anchor,
            body: "x".to_string(),
            author: reviewer(),
        }
    };
    let folded = fold_bodies(vec![
        created(Target::Working),
        bad(|a| a.path = "/abs".to_string()),
        bad(|a| a.path = "a/../b".to_string()),
        bad(|a| a.old_path = Some(String::new())),
        bad(|a| a.start_line = 0),
        bad(|a| a.end_line = 2),
        bad(|a| a.end_line = 4),
        bad(|a| a.round = 2),
        bad(|a| {
            a.end_line = 5;
            a.text = vec!["3".into(), "4".into(), "5".into()];
            a.old_path = Some("old/lib.rs".into());
            a.side = AnchorSide::Old;
        }),
    ]);
    let reasons = rejections(&folded);
    assert_eq!(reasons.len(), 7, "{reasons:?}");
    assert!(
        reasons[..6]
            .iter()
            .all(|r| matches!(r, Rejection::InvalidAnchor { .. })),
        "{reasons:?}"
    );
    assert_eq!(
        reasons[6],
        Rejection::UnknownRound {
            round: 2,
            current: 1
        }
    );
    let [thread] = &folded.review.threads[..] else {
        panic!()
    };
    assert_eq!(thread.anchor.line_count(), 3);
}

#[test]
fn the_review_starts_at_its_created_event() {
    let id = review_id();
    // Nothing before `created` applies, a second `created` is refused, and a log without one
    // has no review.
    let early = event_at(
        1,
        EventBody::Status {
            status: ReviewStatus::Closed,
        },
    );
    let first = event_at(2, created(Target::Working));
    let second = event_at(3, created(Target::Staged));
    let folded = fold(&id, &[early.clone(), first, second.clone()]).unwrap();
    assert_eq!(folded.review.target, Target::Working);
    assert_eq!(folded.review.status, ReviewStatus::Open);
    assert_eq!(folded.review.created_at, 2);
    let rejected: Vec<(EventId, Rejection)> = folded
        .warnings
        .iter()
        .map(|w| match w {
            LogWarning::Rejected { eid, reason, .. } => (eid.clone(), reason.clone()),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        rejected,
        [
            (early.eid.clone(), Rejection::NotCreated),
            (second.eid, Rejection::AlreadyCreated)
        ]
    );
    assert_eq!(fold(&id, &[early]), Err(FoldError::NotCreated));
    assert_eq!(fold(&id, &[]), Err(FoldError::NotCreated));
}

#[test]
fn an_event_that_lands_twice_folds_once() {
    let thread = ThreadId::generate();
    let base = events(vec![
        created(Target::Working),
        thread_added(&thread, reviewer()),
    ]);
    let reply = event_at(
        3,
        EventBody::ReplyAdded {
            thread: thread.clone(),
            id: CommentId::generate(),
            body: "x".to_string(),
            author: agent(),
        },
    );
    let mut all = base;
    all.push(reply.clone());
    all.push(reply.clone());
    let folded = fold(&review_id(), &all).unwrap();
    assert_eq!(rejections(&folded), [Rejection::DuplicateEvent]);
    assert_eq!(folded.review.threads[0].replies.len(), 1);
}

#[test]
fn a_log_folds_through_bad_lines() {
    // A torn last line, a hand-mangled line and a newer dv's event, around valid ones.
    let thread = ThreadId::generate();
    let good = events(vec![
        created(Target::Working),
        thread_added(&thread, reviewer()),
        thread_state(&thread, ThreadState::Resolved, reviewer()),
    ]);
    let mut log = log_of(&good[..2]);
    log.extend(b"{\"v\":1,\"eid\":\n");
    log.extend(br#"{"v":1,"eid":"e-01j9z3k4m5n6p7q8r9s0t1v2w3","at":9,"kind":"thread_pinned","thread":"x"}"#);
    log.extend(b"\n");
    log.extend(log_of(&good[2..]));
    let torn = event_at(4, thread_state(&thread, ThreadState::Open, reviewer()))
        .to_line()
        .unwrap();
    log.extend(&torn[..torn.len() - 3]);

    let parsed = parse_log(&log);
    assert_eq!(parsed.newer_events, 1);
    assert_eq!(parsed.warnings.len(), 2, "{:?}", parsed.warnings);
    let folded = fold(&review_id(), &parsed.events).unwrap();
    assert!(folded.warnings.is_empty());
    assert_eq!(folded.review.threads[0].state, ThreadState::Resolved);
}

#[test]
fn rejections_and_warnings_serialise_for_the_cache() {
    let warnings = vec![
        LogWarning::Malformed {
            line: 3,
            error: "expected value".to_string(),
        },
        LogWarning::Rejected {
            eid: EventId::generate(),
            kind: "thread_state".to_string(),
            reason: Rejection::ThreadTransition {
                from: ThreadState::Resolved,
                to: ThreadState::Addressed,
                author: AuthorKind::Agent,
            },
        },
        LogWarning::Rejected {
            eid: EventId::generate(),
            kind: "status".to_string(),
            reason: Rejection::Submitted,
        },
    ];
    let json = serde_json::to_string(&warnings).unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<LogWarning>>(&json).unwrap(),
        warnings
    );
    assert_eq!(warnings[0].to_string(), "line 3: expected value");
    assert!(
        warnings[1]
            .to_string()
            .ends_with(": an agent can't move a thread from resolved to addressed"),
        "{}",
        warnings[1]
    );
}
