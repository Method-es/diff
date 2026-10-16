//! A review is read in its home worktree whichever worktree the command runs in (PLAN §2.2,
//! §2.4, `dv_core::review::home`): a `working` review in the worktree it was made in, a
//! `branch` review in the worktree that has the branch checked out.

use serde_json::json;

use super::*;

/// `src/app.ts` as the linked worktree edits it: line 5 renamed, not committed.
fn linked_lines() -> Vec<String> {
    let mut lines = app_lines();
    lines[4] = "  userId: string;".to_string();
    lines
}

/// The fixture with a linked worktree on `feat` whose working tree edits `src/app.ts`.
fn with_linked() -> (Fixture, PathBuf) {
    let fixture = Fixture::new();
    let linked = fixture.repo.add_worktree("feature", "feat");
    std::fs::write(linked.join("src/app.ts"), text_of(&linked_lines())).unwrap();
    (fixture, linked)
}

/// What `comment list` says of each thread: its placement, current lines and code.
fn placements(json: &Value) -> Vec<(String, Value, Value)> {
    json["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|thread| {
            (
                thread["placement"].as_str().unwrap().to_string(),
                thread["anchor"]["current"].clone(),
                thread["current_code"].clone(),
            )
        })
        .collect()
}

#[test]
fn a_branch_review_means_the_same_from_every_worktree() {
    let (fixture, linked) = with_linked();
    // Started in the main work tree, which has `main` checked out.
    let review = fixture
        .run(&[
            "review", "new", "--branch", "feat", "--base", "main", "--json",
        ])
        .ok_json()["review"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // The linked worktree's edit is in it: the branch is checked out there.
    let id = review.as_str();
    fixture
        .run_in(
            &linked,
            &[
                "comment",
                "add",
                "--review",
                id,
                "--file",
                "src/app.ts",
                "--lines",
                "5",
                "--body",
                "Why userId?",
                "--json",
            ],
        )
        .ok_json();
    let from_linked = fixture
        .run_in(&linked, &["comment", "list", "--review", id, "--json"])
        .ok_json();
    let from_main = fixture
        .run(&["comment", "list", "--review", id, "--json"])
        .ok_json();
    assert_eq!(placements(&from_main), placements(&from_linked));
    assert_eq!(
        placements(&from_main),
        [(
            "unchanged".to_string(),
            json!({"path": "src/app.ts", "side": "new", "start_line": 5, "end_line": 5}),
            json!("  userId: string;"),
        )]
    );
    // It is the linked worktree's context, not the main work tree's.
    assert_eq!(
        fixture.run_in(&linked, &["status", "--json"]).ok_json()["current"]["id"],
        id
    );
    assert_eq!(
        fixture.run(&["status", "--json"]).ok_json()["current"],
        Value::Null
    );

    // Submitted from the main work tree, the round is the linked worktree's files.
    let ran = fixture.run(&["review", "submit", id, "--verdict", "request-changes"]);
    assert_eq!(ran.exit, Exit::Ok, "{}", ran.stderr);
    assert_eq!(
        fixture
            .repo
            .git(&["show", &format!("refs/dv/reviews/{id}/1:src/app.ts")]),
        text_of(&linked_lines()),
        "the linked worktree's edit, not the branch tip"
    );

    // Checked out nowhere, it is the branch's tip from everywhere.
    fixture.repo.git(&[
        "-C",
        &linked.to_string_lossy(),
        "checkout",
        "-q",
        "--detach",
    ]);
    let from_linked = fixture
        .run_in(&linked, &["comment", "list", "--review", id, "--json"])
        .ok_json();
    let from_main = fixture
        .run(&["comment", "list", "--review", id, "--json"])
        .ok_json();
    assert_eq!(placements(&from_main), placements(&from_linked));
    // The branch's tip doesn't change the file: the thread is found in it as the tip holds it,
    // its line edited there, not in the diff (PLAN §8).
    assert_eq!(placements(&from_main)[0].0, "edited");
    assert_eq!(from_main["threads"][0]["in_diff"], false);
    assert_eq!(placements(&from_main)[0].2, json!("  id: string;"));
}

#[test]
fn a_working_review_reads_its_own_worktree_and_not_one_that_is_gone() {
    let (fixture, linked) = with_linked();
    let working = ops::open_review(
        &ReviewStore::new(fixture.backend_at(&linked)),
        Target::Working,
        reviewer(),
    )
    .unwrap()
    .review;
    let id = working.id.as_str();
    fixture
        .run_in(
            &linked,
            &[
                "comment",
                "add",
                "--file",
                "src/app.ts",
                "--lines",
                "5",
                "--body",
                "Why?",
                "--json",
            ],
        )
        .ok_json();
    let from_main = fixture
        .run(&["comment", "list", "--review", id, "--json"])
        .ok_json();
    assert_eq!(placements(&from_main)[0].0, "unchanged");
    assert_eq!(placements(&from_main)[0].2, json!("  userId: string;"));

    // Its worktree removed, nothing reads it elsewhere: exit 1, `target`.
    fixture.repo.git(&[
        std::ffi::OsStr::new("worktree"),
        std::ffi::OsStr::new("remove"),
        std::ffi::OsStr::new("--force"),
        linked.as_os_str(),
    ]);
    for args in [
        &["comment", "list", "--review", id, "--json"][..],
        &["review", "submit", id, "--verdict", "approve", "--json"],
    ] {
        let error = fixture.run(args).error(Exit::Failure, "target");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("is no longer one of the repository's"),
            "{error}"
        );
    }
    // Nor is it anyone's context, and it can't be told which review to act on alone.
    assert_eq!(
        fixture.run(&["status", "--json"]).ok_json()["current"],
        Value::Null
    );
}
