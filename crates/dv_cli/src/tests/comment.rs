//! `dv comment list` and `dv comment show`: threads placed where their code stands now
//! (docs/PLAN.md §8, §10.1), and threads found by id across reviews (PLAN §10.2).

use dv_core::review::{AnchorSide, ItemId, ThreadState, ops};
use dv_core::target::Target;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::*;

/// The working tree's `src/app.ts` with two lines inserted above everything and
/// `formatName` (lines 17–19) gone.
fn moved_lines() -> Vec<String> {
    let mut lines = edited_lines();
    lines.drain(16..19);
    lines.insert(0, "// cache first".to_string());
    lines.insert(1, "import { cache } from \"./cache\";".to_string());
    lines
}

fn thread_of<'v>(json: &'v Value, id: &ThreadId) -> &'v Value {
    json["threads"]
        .as_array()
        .unwrap()
        .iter()
        .find(|thread| thread["id"] == id.as_str())
        .unwrap_or_else(|| panic!("no thread {id} in {json}"))
}

fn listed(fixture: &Fixture, args: &[&str]) -> Vec<String> {
    let mut line = vec!["comment", "list", "--json"];
    line.extend_from_slice(args);
    let json = fixture.run(&line).ok_json();
    json["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|thread| thread["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn threads_are_placed_where_their_code_is_now() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let edited = fixture.thread(&review, "src/app.ts", 10, 10, "Why a cache?");
    let interface = fixture.thread(&review, "src/app.ts", 4, 7, "Add an email.");
    let format = fixture.thread(&review, "src/app.ts", 18, 18, "Trim earlier.");

    let json = fixture.run(&["comment", "list", "--json"]).ok_json();
    assert_eq!(json["review_id"], review.id.as_str());
    let ids: Vec<&str> = json["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|thread| thread["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [edited.as_str(), interface.as_str(), format.as_str()]);
    for thread in json["threads"].as_array().unwrap() {
        assert_eq!(thread["placement"], "unchanged");
        assert_eq!(thread["outdated"], false);
        assert_eq!(
            thread["anchor"]["current"]["start_line"],
            thread["anchor"]["original"]["start_line"]
        );
    }

    // The agent inserts two lines on top and deletes formatName.
    fixture.edit_app(&moved_lines());
    let json = fixture.run(&["comment", "list", "--json"]).ok_json();
    let thread = thread_of(&json, &interface);
    assert_eq!(
        thread["anchor"],
        json!({
            "original": {"round": 1, "path": "src/app.ts", "side": "new", "start_line": 4, "end_line": 7},
            "current": {"path": "src/app.ts", "side": "new", "start_line": 6, "end_line": 9},
        })
    );
    assert_eq!(thread["placement"], "moved");
    assert_eq!(thread["outdated"], false);
    assert_eq!(
        thread["code"],
        "export interface User {\n  id: string;\n  name: string;\n}"
    );
    assert_eq!(thread["current_code"], thread["code"]);
    assert_eq!(
        thread_of(&json, &edited)["anchor"]["current"]["start_line"],
        12
    );

    let gone = thread_of(&json, &format);
    assert_eq!(gone["placement"], "outdated");
    assert_eq!(gone["outdated"], true);
    assert_eq!(gone["path"], "src/app.ts");
    assert_eq!(gone["anchor"]["current"], json!(null));
    assert_eq!(gone["current_code"], json!(null));
    assert_eq!(gone["code"], "  return user.name.trim();");

    // Filters: by state, by outdated, by file.
    assert_eq!(
        listed(&fixture, &["--status", "outdated"]),
        [format.to_string()]
    );
    assert_eq!(listed(&fixture, &["--status", "open"]).len(), 3);
    assert_eq!(
        listed(&fixture, &["--status", "addressed"]),
        Vec::<String>::new()
    );
    assert_eq!(listed(&fixture, &["--file", "src/app.ts"]).len(), 3);
    assert_eq!(listed(&fixture, &["--file", "./src\\app.ts"]).len(), 3);
    let absolute = fixture.path().join("src").join("app.ts");
    assert_eq!(
        listed(&fixture, &["--file", absolute.to_str().unwrap()]).len(),
        3,
        "an absolute path inside the work tree"
    );
    assert_eq!(
        listed(&fixture, &["--file", "README.md"]),
        Vec::<String>::new()
    );

    let ran = fixture.run(&["comment", "list"]);
    assert_eq!(
        ran.lines(),
        [
            format!("{edited}  open  src/app.ts:12 (moved from 10)  Why a cache?"),
            format!("{interface}  open  src/app.ts:6-9 (moved from 4-7)  Add an email."),
            format!("{format}  open  src/app.ts (outdated, was 18)  Trim earlier."),
        ]
    );
    assert_eq!(
        fixture
            .run(&["comment", "list", "--status", "resolved"])
            .lines(),
        ["no matching threads"]
    );
}

#[test]
fn a_thread_shows_in_full() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let id = fixture.thread(
        &review,
        "src/app.ts",
        4,
        7,
        "Add an **email**.\n\nIt's needed.",
    );
    let store = fixture.store();
    ops::reply(&store, &review.id, &id, "On it.", agent(), None).unwrap();
    let dropped = ops::reply(&store, &review.id, &id, "Oops.", agent(), None).unwrap();
    ops::delete_item(&store, &review.id, &ItemId::Comment(dropped), &agent()).unwrap();
    ops::set_thread_state(
        &store,
        &review.id,
        &id,
        ThreadState::Addressed,
        Some("Added `email`."),
        agent(),
        None,
    )
    .unwrap();
    fixture.edit_app(&moved_lines());

    let loaded = fixture.store().load(&review.id).unwrap().review;
    let thread = loaded.thread(&id).unwrap();
    let json = fixture
        .run(&["comment", "show", id.as_str(), "--json"])
        .ok_json();
    assert_eq!(
        json["thread"],
        json!({
            "id": id.as_str(),
            "review_id": review.id.as_str(),
            "state": "addressed",
            "placement": "moved",
            "outdated": false,
            "path": "src/app.ts",
            "side": "new",
            "anchor": {
                "original": {"round": 1, "path": "src/app.ts", "side": "new", "start_line": 4, "end_line": 7},
                "current": {"path": "src/app.ts", "side": "new", "start_line": 6, "end_line": 9},
            },
            "code": "export interface User {\n  id: string;\n  name: string;\n}",
            "current_code": "export interface User {\n  id: string;\n  name: string;\n}",
            "body": "Add an **email**.\n\nIt's needed.",
            "author": {"kind": "reviewer", "name": "dv fixture"},
            "created_at": thread.created_at,
            "note": "Added `email`.",
            "replies": [{
                "id": thread.replies[0].id.as_str(),
                "author": {"kind": "agent", "name": "claude-code"},
                "body": "On it.",
                "created_at": thread.replies[0].created_at,
            }],
        })
    );

    let ran = fixture.run(&["comment", "show", id.as_str()]);
    assert_eq!(
        ran.lines(),
        [
            format!("{id}  addressed  src/app.ts:6-9 (moved from 4-7)").as_str(),
            format!("review: {}", review.id).as_str(),
            "written on: new side of src/app.ts, line 4-7 (round 1)",
            "by: reviewer dv fixture",
            "code:",
            "  4 | export interface User {",
            "  5 |   id: string;",
            "  6 |   name: string;",
            "  7 | }",
            "comment:",
            "  Add an **email**.",
            "  ",
            "  It's needed.",
            "reply from agent claude-code:",
            "  On it.",
            "addressed note:",
            "  Added `email`.",
        ]
    );
}

#[test]
fn a_thread_is_found_in_whichever_review_holds_it() {
    let mut fixture = Fixture::new();
    let working = fixture.review(Target::Working);
    later();
    let commit = fixture.review(Target::Commit { rev: "HEAD".into() });
    // A thread on the commit's new side: `src/app.ts` as committed.
    let anchor = fixture.anchor("src/app.ts", AnchorSide::Old, 10, 10);
    let anchor = dv_core::review::Anchor {
        side: AnchorSide::New,
        ..anchor
    };
    let id = ops::add_thread(
        &fixture.store(),
        &commit.id,
        anchor,
        "Old query.",
        reviewer(),
    )
    .unwrap();

    // `comment show` finds it without --review, though the working review is the current one.
    let json = fixture
        .run(&["comment", "show", id.as_str(), "--json"])
        .ok_json();
    assert_eq!(json["thread"]["review_id"], commit.id.as_str());
    assert_eq!(
        json["thread"]["placement"], "unchanged",
        "the commit doesn't move"
    );
    // $DV_REVIEW doesn't narrow the search; --review does.
    fixture
        .env
        .insert("DV_REVIEW".into(), working.id.to_string());
    let json = fixture
        .run(&["comment", "show", id.as_str(), "--json"])
        .ok_json();
    assert_eq!(json["thread"]["review_id"], commit.id.as_str());
    fixture.env.remove("DV_REVIEW");
    let error = fixture
        .run(&[
            "comment",
            "show",
            id.as_str(),
            "--review",
            working.id.as_str(),
            "--json",
        ])
        .error(Exit::Failure, "not_found");
    assert_eq!(
        error["message"],
        format!("no thread {id} in review {}", working.id)
    );

    // `comment list` lists the current review's threads only.
    assert_eq!(listed(&fixture, &[]), Vec::<String>::new());
    assert_eq!(fixture.run(&["comment", "list"]).lines(), ["no threads"]);
    assert_eq!(
        listed(&fixture, &["--review", commit.id.as_str()]),
        [id.to_string()]
    );

    // Unknown and malformed ids.
    let missing = ThreadId::generate();
    let error = fixture
        .run(&["comment", "show", missing.as_str(), "--json"])
        .error(Exit::Failure, "not_found");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with(&format!("no thread {missing} in the reviews of "))
    );
    fixture
        .run(&["comment", "show", "t-../x", "--json"])
        .error(Exit::Usage, "usage");
    fixture
        .run(&["comment", "show", working.id.as_str(), "--json"])
        .error(Exit::Usage, "usage");
    // A deleted thread is gone.
    ops::delete_item(
        &fixture.store(),
        &commit.id,
        &ItemId::Thread(id.clone()),
        &reviewer(),
    )
    .unwrap();
    fixture
        .run(&["comment", "show", id.as_str(), "--json"])
        .error(Exit::Failure, "not_found");
}

#[test]
fn renames_carry_threads_and_lost_sides_outdate_them() {
    let fixture = Fixture::new();
    let util = |b: u32| {
        format!(
            "export const a = 1;\nexport const b = {b};\nexport const c = 3;\n\
             export const d = 4;\nexport const e = 5;\nexport const f = 6;\n"
        )
    };
    fixture.repo.write("src/util.ts", util(2));
    fixture.repo.commit("util");
    fixture.repo.write("src/util.ts", util(20));
    let review = fixture.review(Target::Working);
    let on_util = fixture.thread(&review, "src/util.ts", 2, 2, "Why 20?");
    let old_side = fixture.anchor("src/app.ts", AnchorSide::Old, 10, 10);
    let on_old = ops::add_thread(
        &fixture.store(),
        &review.id,
        old_side,
        "Was fine.",
        reviewer(),
    )
    .unwrap();
    let on_new = fixture.thread(&review, "src/app.ts", 10, 10, "Cache?");

    // The agent renames util.ts (staged, so git pairs it) and deletes app.ts.
    fixture.repo.git(&["mv", "src/util.ts", "src/helpers.ts"]);
    std::fs::remove_file(fixture.path().join("src/app.ts")).unwrap();

    let json = fixture.run(&["comment", "list", "--json"]).ok_json();
    let renamed = thread_of(&json, &on_util);
    assert_eq!(renamed["path"], "src/helpers.ts");
    assert_eq!(renamed["placement"], "unchanged");
    assert_eq!(
        renamed["anchor"]["current"],
        json!({"path": "src/helpers.ts", "side": "new", "start_line": 2, "end_line": 2})
    );
    assert_eq!(renamed["anchor"]["original"]["path"], "src/util.ts");
    assert_eq!(renamed["current_code"], "export const b = 20;");
    // `--file` finds it by either name.
    assert_eq!(
        listed(&fixture, &["--file", "src/helpers.ts"]),
        [on_util.to_string()]
    );
    assert_eq!(
        listed(&fixture, &["--file", "src/util.ts"]),
        [on_util.to_string()]
    );

    // app.ts is deleted: its old side is still there, its new side is gone.
    let old = thread_of(&json, &on_old);
    assert_eq!(old["placement"], "unchanged");
    assert_eq!(old["side"], "old");
    // (`commit` took the working tree's app.ts along.)
    assert_eq!(old["current_code"], edited_lines()[9]);
    let new = thread_of(&json, &on_new);
    assert_eq!(new["placement"], "outdated");
    assert_eq!(new["path"], "src/app.ts");
    assert_eq!(new["anchor"]["current"], json!(null));
}

#[test]
fn a_working_review_is_placed_in_its_own_worktree() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let id = fixture.thread(&review, "src/app.ts", 10, 10, "Why a cache?");
    // Another worktree of the repository, on `HEAD` as committed: app.ts unchanged there.
    let linked = fixture.repo.add_worktree("feature", "feat");

    let json = fixture
        .run_in(&linked, &["comment", "show", id.as_str(), "--json"])
        .ok_json();
    assert_eq!(json["thread"]["placement"], "unchanged", "{json}");
    assert_eq!(json["thread"]["current_code"], edited_lines()[9]);
    let json = fixture
        .run_in(
            &linked,
            &["comment", "list", "--review", review.id.as_str(), "--json"],
        )
        .ok_json();
    assert_eq!(json["threads"][0]["outdated"], false);

    // From the other worktree, an absolute path to the reviewed file in the review's own
    // worktree is that file; one into the worktree the command runs in still works too.
    let in_review = fixture.path().join("src").join("app.ts");
    let in_linked = linked.join("src").join("app.ts");
    for absolute in [&in_review, &in_linked] {
        let file = absolute.to_str().unwrap();
        let json = fixture
            .run_in(
                &linked,
                &[
                    "comment",
                    "list",
                    "--review",
                    review.id.as_str(),
                    "--file",
                    file,
                    "--json",
                ],
            )
            .ok_json();
        assert_eq!(json["threads"][0]["id"], id.as_str(), "{file}: {json}");
        let json = fixture
            .run_in(
                &linked,
                &[
                    "comment",
                    "add",
                    "--review",
                    review.id.as_str(),
                    "--file",
                    file,
                    "--lines",
                    "3",
                    "--body",
                    "And here?",
                    "--json",
                ],
            )
            .ok_json();
        assert_eq!(json["thread"]["path"], "src/app.ts", "{file}: {json}");
    }
}
