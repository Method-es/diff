//! Threads read against their round's snapshot (docs/PLAN.md §8 "Round history", §10.3): a
//! file the agent renamed between rounds, untracked or committed, is followed; code rewritten
//! or removed since the round says where it went, in the text and in `--json`.

use dv_core::review::Verdict;
use dv_core::target::Target;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

/// The hook an agent wrote and never committed, then renamed between rounds.
const USE_CAN: &str = "import { useAuth } from \"./use-auth\";\n\nexport function useCan(permission: string) {\n  const { user } = useAuth();\n  if (!user) {\n    return false;\n  }\n  return user.permissions.includes(permission);\n}\n";

fn renamed_hook() -> String {
    USE_CAN.replace("useCan", "useHasPermission")
}

/// The one thread `comment list --json` lists.
fn listed(fixture: &Fixture) -> serde_json::Value {
    let json = fixture.run(&["comment", "list", "--json"]).ok_json();
    json["threads"][0].clone()
}

#[test]
fn an_untracked_file_renamed_between_rounds_keeps_its_thread() {
    let fixture = Fixture::new();
    fixture.repo.write("src/hooks/use-can.ts", USE_CAN);
    let review = fixture.review(Target::Working);
    let thread = fixture.thread(&review, "src/hooks/use-can.ts", 8, 8, "Why `includes`?");
    fixture.finish(&review.id, Verdict::RequestChanges);
    // The agent renames it: the list says "added use-has-permission.ts", untracked, and git
    // can't pair it with the name the thread was written on.
    fixture.repo.remove("src/hooks/use-can.ts");
    fixture
        .repo
        .write("src/hooks/use-has-permission.ts", renamed_hook());

    let thread_json = listed(&fixture);
    assert_eq!(thread_json["id"], thread.as_str());
    assert_eq!(thread_json["placement"], "unchanged", "{thread_json}");
    assert_eq!(thread_json["path"], "src/hooks/use-has-permission.ts");
    assert_eq!(
        thread_json["anchor"]["current"],
        json!({"path": "src/hooks/use-has-permission.ts", "side": "new", "start_line": 8, "end_line": 8})
    );
    assert_eq!(thread_json["trail"], serde_json::Value::Null);

    // A reply records it under the new name, so it stays once the file is committed.
    let ran = fixture.run(&[
        "comment",
        "reply",
        thread.as_str(),
        "--author",
        "codex",
        "--body",
        "Kept: permissions are a short list.",
    ]);
    assert_eq!(ran.exit, Exit::Ok, "{}", ran.stderr);
    let recorded = fixture.load(&review.id).thread(&thread).unwrap().clone();
    assert_eq!(recorded.anchor.path, "src/hooks/use-has-permission.ts");
    assert_eq!(recorded.anchor.round, 2);
}

#[test]
fn a_file_committed_on_a_branch_and_renamed_between_rounds_keeps_its_thread() {
    let fixture = Fixture::new();
    // The branch's own file, which never existed on main.
    fixture.repo.git(&["stash", "-q"]);
    fixture.repo.git(&["checkout", "-q", "-b", "feat"]);
    fixture.repo.write("src/hooks/use-can.ts", USE_CAN);
    fixture.repo.commit("add the hook");
    fixture.repo.checkout("main");
    let review = fixture.review(Target::Branch {
        branch: "feat".to_string(),
        base: Some("main".to_string()),
    });
    let anchor_text: Vec<&str> = USE_CAN.lines().collect();
    let anchor = dv_core::review::anchor_selection(
        AnchorPlace {
            round: 1,
            path: "src/hooks/use-can.ts".to_string(),
            old_path: None,
            side: AnchorSide::New,
        },
        &anchor_text,
        8,
        8,
    )
    .unwrap();
    let thread = ops::add_thread(
        &fixture.store(),
        &review.id,
        anchor,
        "Why `includes`?",
        reviewer(),
    )
    .unwrap();
    fixture.finish(&review.id, Verdict::RequestChanges);
    fixture.repo.checkout("feat");
    fixture.repo.remove("src/hooks/use-can.ts");
    fixture
        .repo
        .write("src/hooks/use-has-permission.ts", renamed_hook());
    fixture.repo.commit("rename the hook");
    fixture.repo.checkout("main");

    // On main, the branch's review is named by its id.
    let id = review.id.to_string();
    let json = fixture
        .run(&["comment", "list", "--review", &id, "--json"])
        .ok_json();
    let thread_json = &json["threads"][0];
    assert_eq!(thread_json["id"], thread.as_str());
    assert_eq!(thread_json["placement"], "unchanged", "{thread_json}");
    assert_eq!(
        thread_json["anchor"]["current"]["path"],
        "src/hooks/use-has-permission.ts"
    );
}

#[test]
fn code_replaced_since_the_round_says_which_lines_replaced_it() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let thread = fixture.thread(&review, "src/app.ts", 11, 13, "Return null instead?");
    fixture.finish(&review.id, Verdict::RequestChanges);
    // The agent rewrites the commented check into something else entirely.
    let mut lines = edited_lines();
    lines.splice(
        10..13,
        [
            "  return row ? toUser(row) : undefined;".to_string(),
            "}".to_string(),
            String::new(),
            "function toUser(row: Row): User {".to_string(),
        ],
    );
    fixture.edit_app(&lines);

    let thread_json = listed(&fixture);
    assert_eq!(thread_json["id"], thread.as_str());
    assert_eq!(thread_json["placement"], "outdated", "{thread_json}");
    assert_eq!(thread_json["outdated"], true);
    assert_eq!(thread_json["anchor"]["current"], serde_json::Value::Null);
    assert_eq!(thread_json["excerpt"], serde_json::Value::Null);
    let trail = &thread_json["trail"];
    assert_eq!(trail["round"], 1);
    assert_eq!(trail["went"], "replaced");
    assert_eq!(trail["path"], "src/app.ts");
    assert_eq!(
        (trail["start_line"].clone(), trail["end_line"].clone()),
        (json!(11), json!(14))
    );
    assert_eq!(trail["before_line"], serde_json::Value::Null);
    assert_eq!(
        trail["code"],
        "  return row ? toUser(row) : undefined;\n}\n\nfunction toUser(row: Row): User {"
    );
    assert!(
        trail["excerpt"]
            .as_str()
            .unwrap()
            .contains(">11 + |   return row ? toUser(row) : undefined;"),
        "{trail}"
    );

    // The text says it too: where the code went, and the lines that replaced it.
    let ran = fixture.run(&["comment", "show", thread.as_str()]);
    assert_eq!(ran.exit, Exit::Ok, "{}", ran.stderr);
    let lines = ran.lines();
    assert!(
        lines.contains(
            &format!("{thread}  open  src/app.ts (outdated, was 11-13, replaced by 11-14)")
                .as_str()
        ),
        "{}",
        ran.stdout
    );
    let at = lines
        .iter()
        .position(|line| {
            *line == "code changed since round 1; these lines replaced it (src/app.ts:11-14):"
        })
        .unwrap_or_else(|| panic!("{}", ran.stdout));
    assert!(
        lines[at + 1..]
            .iter()
            .any(|line| line.starts_with(">11 + |"))
    );
    assert!(
        lines[..at].contains(&"as commented in round 1 (not found in the file now):"),
        "the original code comes first: {}",
        ran.stdout
    );

    // A reply records no anchor from it: the thread isn't re-anchored onto other code.
    let ran = fixture.run(&[
        "comment",
        "reply",
        thread.as_str(),
        "--author",
        "codex",
        "--body",
        "Done.",
    ]);
    assert_eq!(ran.exit, Exit::Ok, "{}", ran.stderr);
    let recorded = fixture.load(&review.id).thread(&thread).unwrap().clone();
    assert_eq!(recorded.original_anchor, None);
    assert_eq!(recorded.anchor.start_line, 11);
}

#[test]
fn code_removed_since_the_round_says_where_it_stood() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let thread = fixture.thread(&review, "src/app.ts", 17, 19, "Is this used?");
    fixture.finish(&review.id, Verdict::RequestChanges);
    // The agent deletes formatName, the blank line before it too.
    let mut lines = edited_lines();
    lines.drain(15..19);
    fixture.edit_app(&lines);

    let thread_json = listed(&fixture);
    assert_eq!(thread_json["placement"], "outdated", "{thread_json}");
    let trail = &thread_json["trail"];
    assert_eq!(trail["went"], "removed", "{thread_json}");
    assert_eq!(trail["before_line"], 16);
    assert_eq!(trail["start_line"], serde_json::Value::Null);
    assert_eq!(trail["excerpt"], serde_json::Value::Null);
    let ran = fixture.run(&["comment", "show", thread.as_str()]);
    assert!(
        ran.lines().contains(
            &"code changed since round 1; it was removed: it stood before line 16 of src/app.ts"
        ),
        "{}",
        ran.stdout
    );
    assert!(
        ran.stdout
            .contains("src/app.ts (outdated, was 17-19, removed before 16)"),
        "{}",
        ran.stdout
    );
}

#[test]
fn code_removed_at_the_end_of_its_file_says_so() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let thread = fixture.thread(&review, "src/app.ts", 20, 20, "Drop the marker.");
    fixture.finish(&review.id, Verdict::RequestChanges);
    let mut lines = edited_lines();
    lines.truncate(19);
    fixture.edit_app(&lines);

    let thread_json = listed(&fixture);
    assert_eq!(thread_json["trail"]["went"], "removed", "{thread_json}");
    assert_eq!(thread_json["trail"]["before_line"], 20);
    let ran = fixture.run(&["comment", "show", thread.as_str()]);
    assert!(
        ran.lines().contains(
            &"code changed since round 1; it was removed: it stood at the end of src/app.ts"
        ),
        "{}",
        ran.stdout
    );
    // Its header says so too, rather than name a line past the file's end.
    assert!(
        ran.stdout
            .contains("src/app.ts (outdated, was 20, removed at the end)"),
        "{}",
        ran.stdout
    );
}

#[test]
fn a_thread_of_a_round_in_progress_reads_no_history() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let thread = fixture.thread(&review, "src/app.ts", 17, 19, "Is this used?");
    let mut lines = edited_lines();
    lines.drain(15..19);
    fixture.edit_app(&lines);
    let thread_json = listed(&fixture);
    assert_eq!(thread_json["id"], thread.as_str());
    assert_eq!(thread_json["placement"], "outdated");
    assert_eq!(thread_json["trail"], serde_json::Value::Null);
}
