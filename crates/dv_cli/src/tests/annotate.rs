//! `dv comment annotate`, `edit` and `delete` (PLAN §19): an agent explains its change in
//! annotations that wait on no one, the reviewer's reply asks about one, and an item's author
//! corrects or withdraws it.

use dv_core::review::{ReviewStatus, ThreadState};
use dv_core::target::Target;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

const WHY: &str = "Reads through the cache: the query was the hot path.";

/// An annotation by `codex` on `src/app.ts` line 10, through the CLI; its id.
fn annotate(fixture: &Fixture) -> String {
    let json = fixture
        .run(&[
            "comment",
            "annotate",
            "--file",
            "src/app.ts",
            "--lines",
            "10",
            "--body",
            WHY,
            "--author",
            "codex",
            "--json",
        ])
        .ok_json();
    json["thread"]["id"].as_str().unwrap().to_string()
}

#[test]
fn an_agent_annotates_lines_and_files_and_nothing_waits_on_them() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let id = annotate(&fixture);
    let thread = &fixture.load(&review.id).threads[0];
    assert_eq!(thread.state, ThreadState::Annotation);
    assert!(thread.annotation);
    assert_eq!(thread.author, Author::agent("codex"));
    assert_eq!(
        fixture.events(&review.id).last().map(author_of),
        Some(Some(&Author::agent("codex")))
    );

    // On the whole file, as text.
    let ran = fixture.run(&[
        "comment",
        "annotate",
        "--file",
        "src/app.ts",
        "--body",
        "Why the file changed.",
        "--author",
        "codex",
    ]);
    assert_eq!(ran.exit, Exit::Ok, "{}", ran.stderr);
    let file_level = fixture.load(&review.id).threads[1].id.clone();
    assert_eq!(
        ran.lines(),
        vec![format!("annotated {file_level}  src/app.ts (file)").as_str()]
    );

    // Listed with the rest, but waiting on no one.
    let list = |status: &str| {
        let json = fixture
            .run(&["comment", "list", "--status", status, "--json"])
            .ok_json();
        json["threads"]
            .as_array()
            .unwrap()
            .iter()
            .map(|thread| thread["id"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(list("open"), Vec::<String>::new());
    assert_eq!(list("annotation"), vec![id.clone(), file_level.to_string()]);
    let shown = fixture.run(&["comment", "show", &id, "--json"]).ok_json();
    assert_eq!(shown["thread"]["state"], "annotation");
    assert_eq!(shown["thread"]["annotation"], true);
    assert_eq!(shown["thread"]["anchor"]["current"]["start_line"], 10);

    let status = fixture.run(&["review", "show", "--json"]).ok_json();
    assert_eq!(
        status["review"]["threads"],
        json!({"open": 0, "addressed": 0, "resolved": 0, "annotation": 2})
    );
    let text = fixture.run(&["review", "show"]);
    assert!(
        text.stdout
            .contains("0 open, 0 addressed, 0 resolved, 2 annotations"),
        "{}",
        text.stdout
    );
}

#[test]
fn only_an_agent_annotates() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let ran = fixture.run(&[
        "comment",
        "annotate",
        "--file",
        "src/app.ts",
        "--lines",
        "10",
        "--body",
        WHY,
        "--json",
    ]);
    let error = ran.error(Exit::Usage, "usage");
    assert!(
        error["message"].as_str().unwrap().starts_with(
            "`dv comment annotate` is an agent's explanation of its change: pass --author"
        ),
        "{error}"
    );
    assert!(fixture.load(&review.id).threads.is_empty());

    // The same files as `comment add`.
    fixture
        .run(&[
            "comment",
            "annotate",
            "--file",
            "README.md",
            "--body",
            WHY,
            "--author",
            "codex",
            "--json",
        ])
        .error(Exit::Failure, "not_found");
}

#[test]
fn an_annotations_state_never_changes_and_the_reviewers_reply_asks_about_it() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let id = annotate(&fixture);

    fixture
        .run(&["comment", "resolve", &id, "--json"])
        .error(Exit::Failure, "rejected");
    fixture
        .run(&["comment", "address", &id, "--author", "codex", "--json"])
        .error(Exit::Failure, "rejected");

    // The agent adds to it; it stays an annotation.
    fixture
        .run(&[
            "comment",
            "reply",
            &id,
            "--body",
            "Measured: 40 ms.",
            "--author",
            "codex",
            "--json",
        ])
        .ok_json();
    assert_eq!(
        fixture.load(&review.id).threads[0].state,
        ThreadState::Annotation
    );

    // The reviewer asks: an open thread the agent sees.
    let json = fixture
        .run(&[
            "comment",
            "reply",
            &id,
            "--body",
            "Why not invalidate on write?",
            "--json",
        ])
        .ok_json();
    assert_eq!(json["thread"]["state"], "open");
    assert_eq!(json["thread"]["annotation"], true);
    let open = fixture
        .run(&["comment", "list", "--status", "open", "--json"])
        .ok_json();
    assert_eq!(open["threads"][0]["id"], id.as_str());
}

#[test]
fn an_items_author_edits_or_deletes_it() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let id = annotate(&fixture);

    fixture
        .run(&["comment", "edit", &id, "--body", "x", "--json"])
        .error(Exit::Failure, "not_allowed");
    let json = fixture
        .run(&[
            "comment",
            "edit",
            &id,
            "--body",
            "Better put.",
            "--author",
            "codex",
            "--json",
        ])
        .ok_json();
    assert_eq!(json["thread"]["body"], "Better put.");

    let reply = fixture
        .run(&[
            "comment", "reply", &id, "--body", "Typo", "--author", "codex", "--json",
        ])
        .ok_json()["reply"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let json = fixture
        .run(&[
            "comment", "edit", &reply, "--body", "Fixed", "--author", "codex", "--json",
        ])
        .ok_json();
    assert_eq!(json["reply"]["body"], "Fixed");
    let json = fixture
        .run(&["comment", "delete", &reply, "--author", "codex", "--json"])
        .ok_json();
    assert_eq!(json["deleted"], reply.as_str());
    assert_eq!(json["thread"]["id"], id.as_str());
    assert_eq!(json["thread"]["replies"], json!([]));

    let ran = fixture.run(&["comment", "delete", &id, "--author", "codex"]);
    assert_eq!(ran.exit, Exit::Ok, "{}", ran.stderr);
    assert_eq!(ran.lines(), vec![format!("deleted {id}").as_str()]);
    assert_eq!(fixture.load(&review.id).live_threads().count(), 0);
    fixture
        .run(&["comment", "delete", &id, "--author", "codex", "--json"])
        .error(Exit::Failure, "not_found");
    fixture
        .run(&["comment", "delete", "x-1", "--json"])
        .error(Exit::Usage, "usage");
}

#[test]
fn a_finished_review_takes_no_annotation_edit_or_delete() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let id = annotate(&fixture);
    ops::set_status(
        &fixture.store(),
        &review.id,
        ReviewStatus::Approved,
        &reviewer(),
        None,
    )
    .unwrap();
    fixture
        .run(&[
            "--review",
            review.id.as_str(),
            "comment",
            "annotate",
            "--file",
            "src/app.ts",
            "--body",
            WHY,
            "--author",
            "codex",
            "--json",
        ])
        .error(Exit::Failure, "finished");
    fixture
        .run(&[
            "comment", "edit", &id, "--body", "x", "--author", "codex", "--json",
        ])
        .error(Exit::Failure, "finished");
    fixture
        .run(&["comment", "delete", &id, "--author", "codex", "--json"])
        .error(Exit::Failure, "finished");
}
