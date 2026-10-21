//! Annotations (PLAN §19.1): an agent's explanation of its change, a thread waiting on no one
//! until the reviewer's reply asks about it.

use super::*;
use crate::review::ops::{
    annotate, delete_item, edit_body, open_review, reply, set_status, set_thread_state,
};

fn annotation_added(id: &ThreadId, author: Author) -> EventBody {
    EventBody::ThreadAdded {
        id: id.clone(),
        anchor: anchor("src/lib.rs", 3),
        body: "Kept the old parser: the new one drops comments.".to_string(),
        author,
        annotation: true,
    }
}

fn fold_bodies(bodies: Vec<EventBody>) -> Folded {
    let events: Vec<Event> = bodies
        .into_iter()
        .enumerate()
        .map(|(n, body)| event_at(n as u64 + 1, body))
        .collect();
    let id = ReviewId::parse("r-01j9z3k4m5n6p7q8r9s0t1v2w3").unwrap();
    fold(&id, &events).unwrap()
}

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
fn an_agents_annotation_waits_on_no_one() {
    let note = ThreadId::generate();
    let folded = fold_bodies(vec![
        created(Target::Working),
        annotation_added(&note, agent()),
    ]);
    assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
    let thread = folded.review.thread(&note).unwrap();
    assert_eq!(thread.state, ThreadState::Annotation);
    assert!(thread.annotation);
    assert!(!thread.state.is_waiting());
    let counts = folded.review.summary().threads;
    assert_eq!((counts.open, counts.annotation), (0, 1));
}

#[test]
fn only_an_agent_writes_an_annotation() {
    let note = ThreadId::generate();
    let folded = fold_bodies(vec![
        created(Target::Working),
        annotation_added(&note, reviewer()),
    ]);
    assert_eq!(
        rejections(&folded),
        vec![Rejection::AnnotationAuthor {
            author: AuthorKind::Reviewer
        }]
    );
    assert!(folded.review.thread(&note).is_none());
}

#[test]
fn the_reviewers_reply_asks_about_it_and_an_agents_adds_to_it() {
    let note = ThreadId::generate();
    let folded = fold_bodies(vec![
        created(Target::Working),
        annotation_added(&note, agent()),
        reply_added(&note, "And the tests cover both.", agent()),
        reply_added(&note, "More.", Author::agent("codex")),
    ]);
    assert_eq!(
        folded.review.thread(&note).unwrap().state,
        ThreadState::Annotation
    );

    let question = CommentId::generate();
    let folded = fold_bodies(vec![
        created(Target::Working),
        annotation_added(&note, agent()),
        EventBody::ReplyAdded {
            thread: note.clone(),
            id: question.clone(),
            body: "Why not fix the new one?".to_string(),
            author: reviewer(),
            anchor: None,
        },
        // Deleting the question moves nothing back.
        EventBody::Deleted {
            target_id: ItemId::Comment(question),
        },
    ]);
    assert!(folded.warnings.is_empty(), "{:?}", folded.warnings);
    let thread = folded.review.thread(&note).unwrap();
    assert_eq!(thread.state, ThreadState::Open);
    assert!(thread.annotation, "it stays an annotation by origin");
    assert_eq!(
        thread.last_change, None,
        "the reply is the move, no state event"
    );
    let counts = folded.review.summary().threads;
    assert_eq!((counts.open, counts.annotation), (1, 0));
}

#[test]
fn an_annotations_state_never_changes_and_nothing_moves_to_it() {
    let note = ThreadId::generate();
    let comment = ThreadId::generate();
    let folded = fold_bodies(vec![
        created(Target::Working),
        annotation_added(&note, agent()),
        thread_state(&note, ThreadState::Addressed, agent()),
        thread_state(&note, ThreadState::Resolved, reviewer()),
        thread_state(&note, ThreadState::Annotation, reviewer()),
        thread_added(&comment, reviewer()),
        thread_state(&comment, ThreadState::Annotation, reviewer()),
    ]);
    let annotation = Rejection::Annotation {
        thread: note.clone(),
    };
    assert_eq!(
        rejections(&folded),
        vec![
            annotation.clone(),
            annotation.clone(),
            annotation,
            Rejection::ThreadTransition {
                from: ThreadState::Open,
                to: ThreadState::Annotation,
                author: AuthorKind::Reviewer,
            },
        ]
    );
    assert_eq!(
        folded.review.thread(&note).unwrap().state,
        ThreadState::Annotation
    );
    assert_eq!(
        folded.review.thread(&comment).unwrap().state,
        ThreadState::Open
    );
}

#[test]
fn the_flag_is_written_only_for_an_annotation() {
    let comment = Event::new(thread_added(&ThreadId::generate(), reviewer()));
    let line = String::from_utf8(comment.to_line().unwrap()).unwrap();
    assert!(!line.contains("annotation"), "{line}");

    let note = Event::new(annotation_added(&ThreadId::generate(), agent()));
    let line = String::from_utf8(note.to_line().unwrap()).unwrap();
    assert!(line.contains(r#""annotation":true"#), "{line}");
    let parsed = parse_log(line.as_bytes());
    assert_eq!(parsed.events, vec![note]);
}

#[test]
fn annotating_is_an_agents_write_on_an_open_local_review() {
    let (_repo, store) = fixture_store();
    let id = open_review(&store, Target::Working, reviewer())
        .unwrap()
        .review
        .id;

    let note = annotate(&store, &id, anchor("src/lib.rs", 1), "Why", agent()).unwrap();
    let review = store.load(&id).unwrap().review;
    assert_eq!(review.thread(&note).unwrap().state, ThreadState::Annotation);

    assert!(matches!(
        annotate(&store, &id, anchor("src/lib.rs", 1), "Why", reviewer()),
        Err(OpError::NotAllowed { .. })
    ));
    assert!(matches!(
        annotate(&store, &id, anchor("src/lib.rs", 1), "  ", agent()),
        Err(OpError::Empty { .. })
    ));

    // An annotation's state doesn't change, whoever asks, before who may is looked at.
    for (state, author) in [
        (ThreadState::Addressed, agent()),
        (ThreadState::Resolved, agent()),
        (ThreadState::Resolved, reviewer()),
        (ThreadState::Open, reviewer()),
    ] {
        assert!(matches!(
            set_thread_state(&store, &id, &note, state, None, author, None),
            Err(OpError::Rejected(Rejection::Annotation { .. }))
        ));
    }

    // Its author corrects or withdraws it.
    let item = ItemId::Thread(note.clone());
    assert!(matches!(
        edit_body(&store, &id, &item, "x", &reviewer()),
        Err(OpError::NotAllowed { .. })
    ));
    edit_body(&store, &id, &item, "Why, better put", &agent()).unwrap();
    reply(&store, &id, &note, "Asked", reviewer(), None).unwrap();
    let review = store.load(&id).unwrap().review;
    assert_eq!(review.thread(&note).unwrap().body, "Why, better put");
    assert_eq!(review.thread(&note).unwrap().state, ThreadState::Open);
    delete_item(&store, &id, &item, &agent()).unwrap();

    set_status(&store, &id, ReviewStatus::Approved, &reviewer(), None).unwrap();
    assert!(matches!(
        annotate(&store, &id, anchor("src/lib.rs", 1), "Late", agent()),
        Err(OpError::Finished { .. })
    ));
}

#[test]
fn a_pr_review_takes_no_annotation() {
    let (_repo, store) = fixture_store();
    let target = Target::Pr {
        owner: "kylekz".to_string(),
        repo: "difftest".to_string(),
        number: 12,
        base_oid: oid('a'),
        head_oid: oid('b'),
    };
    let id = open_review(&store, target, reviewer()).unwrap().review.id;
    assert!(matches!(
        annotate(&store, &id, anchor("src/lib.rs", 1), "Why", agent()),
        Err(OpError::NotAllowed { .. })
    ));
    assert!(store.load(&id).unwrap().review.threads.is_empty());
}
