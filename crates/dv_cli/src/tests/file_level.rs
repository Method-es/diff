//! Comments on a whole file (docs/PLAN.md §8, §10.1, §10.3): `dv comment add` without
//! `--lines`, how `comment list` and `comment show` print and report one, and writes on one
//! whose file was renamed or deleted since.

use dv_core::review::{AnchorSide, EventBody};
use dv_core::target::Target;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

/// The review's last event.
fn last(fixture: &Fixture, id: &ReviewId) -> EventBody {
    fixture.events(id).pop().unwrap()
}

/// The anchor the review's last reply recorded.
fn last_reply_anchor(fixture: &Fixture, id: &ReviewId) -> Option<Anchor> {
    fixture
        .events(id)
        .into_iter()
        .rev()
        .find_map(|event| match event {
            EventBody::ReplyAdded { anchor, .. } => Some(anchor),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no reply in review {id}"))
}

#[test]
fn comment_add_without_lines_comments_on_the_whole_file() {
    let fixture = Fixture::new();
    // A committed file the working tree deletes, and an untracked binary.
    fixture.edit_app(&app_lines());
    fixture
        .repo
        .write("src/gone.ts", "export const gone = 1;\n");
    fixture.repo.commit("gone");
    fixture.edit_app(&edited_lines());
    fixture.repo.remove("src/gone.ts");
    fixture
        .repo
        .write("logo.png", b"\x89PNG\r\n\x1a\n\0\0\0binary");
    let review = fixture.review(Target::Working);

    let json = fixture
        .run(&[
            "comment",
            "add",
            "--file",
            "src/app.ts",
            "--body",
            "Split this file",
            "--json",
        ])
        .ok_json();
    let thread = &json["thread"];
    assert_eq!(thread["placement"], "file");
    assert_eq!(thread["file_level"], true);
    assert_eq!(thread["outdated"], false);
    assert_eq!(thread["path"], "src/app.ts");
    assert_eq!(
        thread["anchor"],
        json!({
            "original": {"round": 1, "path": "src/app.ts", "side": "new", "start_line": 0, "end_line": 0},
            "current": null,
        })
    );
    for key in ["code_context", "current_code", "excerpt", "trail"] {
        assert_eq!(thread[key], json!(null), "{key}");
    }
    assert_eq!(thread["code"], "");
    match last(&fixture, &review.id) {
        EventBody::ThreadAdded { anchor, .. } => {
            assert!(anchor.is_file_level(), "{anchor:?}");
            assert_eq!(anchor.side, AnchorSide::New);
        }
        other => panic!("{other:?}"),
    }

    // A line comment says it isn't one.
    let lines = fixture
        .run(&[
            "comment",
            "add",
            "--file",
            "src/app.ts",
            "--lines",
            "10",
            "--body",
            "Cache?",
            "--json",
        ])
        .ok_json();
    assert_eq!(lines["thread"]["file_level"], false);

    // A binary file takes a file comment (no lines are read), as text.
    let added = fixture.run(&["comment", "add", "--file", "logo.png", "--body", "Too big"]);
    assert_eq!(added.exit, Exit::Ok, "{}", added.stderr);
    let [line] = added.lines()[..] else {
        panic!("{}", added.stdout)
    };
    assert!(line.starts_with("added t-"), "{line}");
    assert!(line.ends_with("  logo.png (file)"), "{line}");

    // A deleted file has its old side only, which `--side old` comments on.
    let refused = fixture.run(&[
        "comment",
        "add",
        "--file",
        "src/gone.ts",
        "--body",
        "Why?",
        "--json",
    ]);
    let error = refused.error(Exit::Failure, "rejected");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("is deleted: it has no new side (`--side old` comments on it as it was)"),
        "{error}"
    );
    let old = fixture
        .run(&[
            "comment",
            "add",
            "--file",
            "src/gone.ts",
            "--side",
            "old",
            "--body",
            "Why delete it?",
            "--json",
        ])
        .ok_json();
    assert_eq!(old["thread"]["placement"], "file");
    assert_eq!(old["thread"]["side"], "old");
    let text = fixture.run(&["comment", "show", old["thread"]["id"].as_str().unwrap()]);
    assert_eq!(
        text.lines()[2],
        format!(
            "{}  open  src/gone.ts (file, old)",
            old["thread"]["id"].as_str().unwrap()
        )
    );
    // An untracked file has no old side.
    fixture
        .run(&[
            "comment", "add", "--file", "logo.png", "--side", "old", "--body", "x", "--json",
        ])
        .error(Exit::Failure, "rejected");
}

#[test]
fn a_file_comment_reads_as_one_and_takes_replies() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let anchor = Anchor::file_level(1, "src/app.ts".to_string(), None, AnchorSide::New);
    let id = ops::add_thread(
        &fixture.store(),
        &review.id,
        anchor,
        "Split this file",
        reviewer(),
    )
    .unwrap();

    let shown = fixture.run(&["comment", "show", id.as_str()]);
    assert_eq!(shown.exit, Exit::Ok, "{}", shown.stderr);
    assert_eq!(
        shown.lines()[2..],
        [
            format!("{id}  open  src/app.ts (file)").as_str(),
            "on the whole file",
            "reviewer dv fixture, round 1:",
            "    Split this file",
        ]
    );
    let oneline = fixture.run(&["comment", "list", "--oneline"]);
    assert_eq!(
        oneline.lines(),
        [format!("{id}  open  src/app.ts (file)  Split this file").as_str()]
    );
    let listed = fixture.run(&["comment", "list", "--file", "src/app.ts", "--json"]);
    assert_eq!(listed.ok_json()["threads"][0]["id"], id.as_str());

    // The agent replies and addresses it; nothing re-anchors a file standing where it was.
    let replied = fixture.run_stdin(
        &[
            "comment",
            "reply",
            id.as_str(),
            "--author",
            "claude-code",
            "--body-file",
            "-",
        ],
        "Split into two modules.\n",
    );
    assert_eq!(replied.exit, Exit::Ok, "{}", replied.stderr);
    assert!(
        replied.lines()[0].ends_with("  src/app.ts (file)"),
        "{}",
        replied.stdout
    );
    assert_eq!(last_reply_anchor(&fixture, &review.id), None);
    let addressed = fixture
        .run(&[
            "comment",
            "address",
            id.as_str(),
            "--author",
            "claude-code",
            "--note",
            "Split",
            "--json",
        ])
        .ok_json();
    assert_eq!(addressed["thread"]["state"], "addressed");
    assert_eq!(addressed["thread"]["placement"], "file");
    let shown = fixture.run(&["comment", "show", id.as_str()]);
    assert_eq!(
        shown.lines()[2..],
        [
            format!("{id}  addressed  src/app.ts (file)").as_str(),
            "on the whole file",
            "reviewer dv fixture, round 1:",
            "    Split this file",
            "agent claude-code:",
            "    Split into two modules.",
            "addressed note:",
            "    Split",
        ]
    );
}

#[test]
fn a_file_comment_follows_its_file_and_outdates_once_it_is_gone() {
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
    fixture
        .repo
        .write("src/draft.ts", "export const draft = 1;\n");
    let review = fixture.review(Target::Working);
    let on = |path: &str| {
        let anchor = Anchor::file_level(1, path.to_string(), None, AnchorSide::New);
        ops::add_thread(&fixture.store(), &review.id, anchor, "Hm", reviewer()).unwrap()
    };
    let (util, app, draft) = (on("src/util.ts"), on("src/app.ts"), on("src/draft.ts"));

    // util.ts renamed (staged, so git pairs it), app.ts deleted, the untracked draft gone.
    fixture.repo.git(&["mv", "src/util.ts", "src/helpers.ts"]);
    fixture.repo.remove("src/app.ts");
    fixture.repo.remove("src/draft.ts");

    let json = fixture.run(&["comment", "list", "--json"]).ok_json();
    let of = |id: &ThreadId| {
        json["threads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|thread| thread["id"] == id.as_str())
            .unwrap()
            .clone()
    };
    let renamed = of(&util);
    assert_eq!(
        (
            &renamed["placement"],
            &renamed["path"],
            &renamed["outdated"]
        ),
        (&json!("file"), &json!("src/helpers.ts"), &json!(false))
    );
    assert_eq!(renamed["anchor"]["original"]["path"], "src/util.ts");
    let deleted = of(&app);
    assert_eq!(
        (
            &deleted["placement"],
            &deleted["path"],
            &deleted["outdated"]
        ),
        (&json!("outdated"), &json!("src/app.ts"), &json!(true))
    );
    assert_eq!(of(&draft)["placement"], "outdated");
    assert_eq!(
        fixture
            .run(&["comment", "list", "--status", "outdated", "--json"])
            .ok_json()["threads"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let block = |id: &ThreadId| {
        let shown = fixture.run(&["comment", "show", id.as_str()]);
        shown.lines()[2..4]
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        block(&util),
        [
            format!("{util}  open  src/helpers.ts (file)"),
            "on the whole file".to_string()
        ]
    );
    assert_eq!(
        block(&app),
        [
            format!("{app}  open  src/app.ts (file, outdated)"),
            "on the whole file, which is deleted now".to_string()
        ]
    );
    assert_eq!(
        block(&draft),
        [
            format!("{draft}  open  src/draft.ts (file, outdated)"),
            "on the whole file, which isn't among the review's changes now".to_string()
        ]
    );

    // A reply on the renamed file's comment records it under the name now, still on the
    // whole file.
    let replied = fixture.run(&["comment", "reply", util.as_str(), "--body", "Fine"]);
    assert_eq!(replied.exit, Exit::Ok, "{}", replied.stderr);
    let anchor = last_reply_anchor(&fixture, &review.id).expect("the rename is recorded");
    assert!(anchor.is_file_level(), "{anchor:?}");
    assert_eq!(
        (
            anchor.path.as_str(),
            anchor.old_path.as_deref(),
            anchor.round
        ),
        ("src/helpers.ts", Some("src/util.ts"), 1)
    );
    // Replies to the outdated ones record nothing.
    fixture
        .run(&["comment", "reply", app.as_str(), "--body", "Gone", "--json"])
        .ok_json();
    assert_eq!(last_reply_anchor(&fixture, &review.id), None);
}

#[test]
fn a_file_comment_follows_an_untracked_file_renamed_between_rounds() {
    let fixture = Fixture::new();
    let hook = "import { useAuth } from \"./use-auth\";\n\nexport function useCan(permission: string) {\n  const { user } = useAuth();\n  if (!user) {\n    return false;\n  }\n  return user.permissions.includes(permission);\n}\n";
    fixture.repo.write("src/hooks/use-can.ts", hook);
    let review = fixture.review(Target::Working);
    let anchor = Anchor::file_level(1, "src/hooks/use-can.ts".to_string(), None, AnchorSide::New);
    let id = ops::add_thread(
        &fixture.store(),
        &review.id,
        anchor,
        "Rename it",
        reviewer(),
    )
    .unwrap();
    fixture.finish(&review.id, Verdict::RequestChanges);
    fixture.repo.remove("src/hooks/use-can.ts");
    fixture.repo.write(
        "src/hooks/use-has-permission.ts",
        hook.replace("useCan", "useHasPermission"),
    );
    let json = fixture
        .run(&["comment", "show", id.as_str(), "--json"])
        .ok_json();
    assert_eq!(json["thread"]["placement"], "file");
    assert_eq!(json["thread"]["path"], "src/hooks/use-has-permission.ts");
    // The agent's reply records it under the name now, in the round in progress.
    fixture
        .run(&[
            "comment",
            "reply",
            id.as_str(),
            "--author",
            "claude-code",
            "--body",
            "Renamed",
            "--json",
        ])
        .ok_json();
    let anchor = last_reply_anchor(&fixture, &review.id).expect("the rename is recorded");
    assert!(anchor.is_file_level());
    assert_eq!(
        (anchor.path.as_str(), anchor.round),
        ("src/hooks/use-has-permission.ts", 2)
    );
}

#[test]
fn a_file_comment_goes_on_the_old_side_of_a_deleted_file_only() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    // app.ts is modified: a comment on the whole of it goes on its new side.
    let refused = fixture.run(&[
        "comment",
        "add",
        "--file",
        "src/app.ts",
        "--side",
        "old",
        "--body",
        "x",
        "--json",
    ]);
    let error = refused.error(Exit::Failure, "rejected");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("src/app.ts isn't deleted: a comment on the whole file goes on its new side"),
        "{error}"
    );
    assert!(fixture.load(&review.id).live_threads().next().is_none());
}

#[test]
fn an_old_side_file_comment_is_named_by_its_sides_path_as_the_gui_names_it() {
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
    // An old-side comment on the whole file (an older writer's shape), then the file renamed.
    let anchor = Anchor::file_level(1, "src/util.ts".to_string(), None, AnchorSide::Old);
    let id = ops::add_thread(&fixture.store(), &review.id, anchor, "Hm", reviewer()).unwrap();
    fixture.repo.git(&["mv", "src/util.ts", "src/helpers.ts"]);
    let json = fixture
        .run(&["comment", "show", id.as_str(), "--json"])
        .ok_json();
    assert_eq!(json["thread"]["placement"], "file");
    assert_eq!(json["thread"]["path"], "src/helpers.ts");
    // The text names it by its old side's path, `AnchorPlace::for_entry`, as the GUI does.
    let shown = fixture.run(&["comment", "show", id.as_str()]);
    assert_eq!(
        shown.lines()[2],
        format!("{id}  open  src/util.ts (file, old)")
    );
}
