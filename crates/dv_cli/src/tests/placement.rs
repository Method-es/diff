//! Where the CLI says a thread is, in the cases that need more than a moved line (docs/PLAN.md
//! §8, §10.1): a file renamed without an edit, old-side threads in the text output, notes as
//! text, a review whose worktree is another repository's now, and the excerpt that stands
//! exactly where a thread's code does (§10.3).

use dv_core::review::{AnchorSide, ThreadState, ops};
use dv_core::target::Target;
use pretty_assertions::assert_eq;

use super::*;

#[test]
fn a_thread_in_a_file_renamed_without_an_edit_stays_on_its_lines() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let thread = fixture.thread(&review, "src/app.ts", 5, 5, "Is the id a string?");
    // The agent reverts its edit and renames the file: git prints the rename and no hunk.
    fixture.edit_app(&app_lines());
    fixture.repo.git(&["mv", "src/app.ts", "src/user.ts"]);

    let json = fixture.run(&["comment", "list", "--json"]).ok_json();
    let listed = &json["threads"][0];
    assert_eq!(listed["id"], thread.as_str());
    assert_eq!(listed["placement"], "unchanged", "{listed}");
    assert_eq!(listed["outdated"], false);
    assert_eq!(listed["path"], "src/user.ts");
    assert_eq!(
        listed["anchor"]["current"],
        serde_json::json!({"path": "src/user.ts", "side": "new", "start_line": 5, "end_line": 5})
    );
    assert_eq!(listed["current_code"], "  id: string;");

    // A reply records the file's name now, though its lines didn't move...
    let ran = fixture.run(&[
        "comment",
        "reply",
        thread.as_str(),
        "--author",
        "codex",
        "--body",
        "It is.",
        "--json",
    ]);
    assert_eq!(ran.exit, Exit::Ok, "{}", ran.stderr);
    assert_eq!(ran.json()["thread"]["placement"], "unchanged");
    let recorded = fixture.load(&review.id).thread(&thread).unwrap().clone();
    assert_eq!(recorded.anchor.path, "src/user.ts");
    assert_eq!(recorded.anchor.old_path.as_deref(), Some("src/app.ts"));
    assert_eq!(
        (recorded.anchor.start_line, recorded.anchor.end_line),
        (5, 5)
    );
    // ...so once the rename is committed, the thread is still found in it.
    fixture.repo.commit_staged("rename");
    fixture.repo.write("src/user.ts", text_of(&edited_lines()));
    let json = fixture.run(&["comment", "list", "--json"]).ok_json();
    let listed = &json["threads"][0];
    assert_eq!(listed["placement"], "unchanged", "{listed}");
    assert_eq!(listed["anchor"]["current"]["path"], "src/user.ts");
    assert_eq!(listed["anchor"]["current"]["start_line"], 5);
}

#[test]
fn an_old_side_thread_says_so_in_the_text_output() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let old = fixture.anchor("src/app.ts", AnchorSide::Old, 10, 10);
    let old = ops::add_thread(
        &fixture.store(),
        &review.id,
        old,
        "Was this slow?",
        reviewer(),
    )
    .unwrap();
    let new = fixture.thread(&review, "src/app.ts", 10, 10, "Why a cache?");

    let ran = fixture.run(&["comment", "list", "--oneline"]);
    assert_eq!(ran.exit, Exit::Ok, "{}", ran.stderr);
    let line = |id: &ThreadId| {
        ran.lines()
            .into_iter()
            .find(|line| line.starts_with(id.as_str()))
            .unwrap_or_else(|| panic!("{id} in {}", ran.stdout))
            .to_string()
    };
    assert_eq!(
        line(&old),
        format!("{old}  open  src/app.ts:10 (old)  Was this slow?")
    );
    assert_eq!(
        line(&new),
        format!("{new}  open  src/app.ts:10  Why a cache?")
    );

    // The write commands' lines say it too.
    let ran = fixture.run(&[
        "comment",
        "add",
        "--file",
        "src/app.ts",
        "--side",
        "old",
        "--lines",
        "2",
        "--body",
        "Unused?",
    ]);
    assert_eq!(ran.exit, Exit::Ok, "{}", ran.stderr);
    assert!(
        ran.stdout.trim_end().ends_with("  src/app.ts:2 (old)"),
        "{}",
        ran.stdout
    );
}

#[test]
fn notes_are_cleaned_like_bodies() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let thread = fixture.thread(&review, "src/app.ts", 10, 10, "Why a cache?");
    fixture.finish(&review.id, Verdict::RequestChanges);

    // A note from PowerShell: a byte order mark and CRLF line endings.
    let ran = fixture.run(&[
        "comment",
        "address",
        thread.as_str(),
        "--author",
        "codex",
        "--note",
        "\u{feff}Moved it.\r\nSee the reply.\r\n",
        "--json",
    ]);
    assert_eq!(ran.exit, Exit::Ok, "{}", ran.stderr);
    assert_eq!(ran.json()["thread"]["note"], "Moved it.\nSee the reply.");
    let ran = fixture.run(&[
        "review",
        "done",
        "--author",
        "codex",
        "--note",
        "\u{feff}All done.\r\nTwo files.\r\n",
        "--json",
    ]);
    assert_eq!(ran.exit, Exit::Ok, "{}", ran.stderr);
    assert_eq!(
        ran.json()["review"]["status_note"]["note"],
        "All done.\nTwo files."
    );
    let review = fixture.load(&review.id);
    assert_eq!(
        review.thread(&thread).unwrap().state,
        ThreadState::Addressed
    );
    // A note that is only what a pipe adds is no note.
    let ran = fixture.run(&[
        "comment",
        "address",
        thread.as_str(),
        "--author",
        "codex",
        "--note",
        "\u{feff}\r\n",
        "--json",
    ]);
    assert_eq!(ran.exit, Exit::Ok, "{}", ran.stderr);
    assert_eq!(ran.json()["thread"]["note"], serde_json::Value::Null);
}

#[test]
fn a_review_whose_worktree_is_not_this_repositorys_is_refused() {
    // A working review made in `original`, whose log was then copied into `copy` (a repository
    // copied, its review still naming the original's worktree).
    let original = Fixture::new();
    let review = original.review(Target::Working);
    original.thread(&review, "src/app.ts", 10, 10, "Why a cache?");
    let copy = Fixture::new();
    let log = format!("dv/reviews/{}.jsonl", review.id);
    let from = original.repo.git(&["rev-parse", "--git-common-dir"]);
    let to = copy.repo.git(&["rev-parse", "--git-common-dir"]);
    let (from, to) = (
        original.path().join(from.trim()).join(&log),
        copy.path().join(to.trim()).join(&log),
    );
    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    std::fs::copy(&from, &to).unwrap();

    let before = original.events(&review.id).len();
    let error = copy
        .run(&[
            "review",
            "submit",
            "--review",
            review.id.as_str(),
            "--verdict",
            "request-changes",
            "--json",
        ])
        .error(Exit::Failure, "target");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("is no longer one of the repository's"),
        "{error}"
    );
    assert_eq!(
        original.events(&review.id).len(),
        before,
        "nothing written into the other repository"
    );
    copy.run(&["comment", "list", "--review", review.id.as_str(), "--json"])
        .error(Exit::Failure, "target");
}

#[test]
fn a_line_edited_where_it_stands_is_edited_not_unchanged() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let thread = fixture.thread(&review, "src/app.ts", 5, 5, "Is the id a string?");
    // The agent edits the commented line where it stands.
    let mut lines = edited_lines();
    lines[4] = "  id: string | number;".to_string();
    fixture.edit_app(&lines);

    let json = fixture.run(&["comment", "list", "--json"]).ok_json();
    let listed = &json["threads"][0];
    assert_eq!(listed["id"], thread.as_str());
    assert_eq!(listed["placement"], "edited", "{listed}");
    assert_eq!(listed["outdated"], false);
    assert_eq!(listed["anchor"]["current"]["start_line"], 5);
    assert_eq!(listed["current_code"], "  id: string | number;");
    assert_ne!(listed["code"], listed["current_code"]);
    assert_eq!(listed["code_context"], serde_json::Value::Null);
    assert_eq!(
        listed["excerpt"],
        [
            " 2   | import { log } from \"./log\";",
            " 3   |",
            " 4   | export interface User {",
            "   - |   id: string;",
            ">5 + |   id: string | number;",
            " 6   |   name: string;",
            " 7   | }",
            " 8   |",
        ]
        .join("\n")
    );
    // The text says what the line was when commented on.
    let ran = fixture.run(&["comment", "list"]);
    assert_eq!(
        ran.lines()[11..],
        [
            "as commented in round 1:",
            "> |   id: string;",
            "reviewer dv fixture, round 1:",
            "    Is the id a string?",
        ]
    );
}

/// `excerpt` stands exactly when `anchor.current` does, and comes from the same diff: its `>`
/// rows are the current lines' text, numbered as `anchor.current` says on the new side.
/// `code_context` stands exactly when they don't (PLAN §10.3).
#[test]
fn an_excerpt_stands_exactly_where_the_code_does() {
    let fixture = Fixture::new();
    fixture.repo.write("src/logo.txt", "logo\n");
    fixture.repo.commit("logo");
    let review = fixture.review(Target::Working);
    fixture.thread(&review, "src/app.ts", 4, 7, "Add an email.");
    fixture.thread(&review, "src/app.ts", 10, 10, "Why a cache?");
    fixture.thread(&review, "src/app.ts", 17, 19, "Trim earlier.");
    let old = fixture.anchor("src/app.ts", AnchorSide::Old, 10, 10);
    ops::add_thread(&fixture.store(), &review.id, old, "Was fine.", reviewer()).unwrap();
    fixture.repo.write("src/logo.txt", "logo, bigger\n");
    fixture.thread(&review, "src/logo.txt", 1, 1, "Sharper?");
    // The agent moves the code down, deletes formatName and makes the logo binary.
    let mut lines = edited_lines();
    lines.drain(16..19);
    lines.insert(0, "// cache first".to_string());
    fixture.edit_app(&lines);
    fixture.repo.write("src/logo.txt", b"\x89PNG\0\0\x01");

    let json = fixture.run(&["comment", "list", "--json"]).ok_json();
    let threads = json["threads"].as_array().unwrap();
    let placements: Vec<&str> = threads
        .iter()
        .map(|thread| thread["placement"].as_str().unwrap())
        .collect();
    assert_eq!(
        placements,
        ["moved", "moved", "outdated", "unchanged", "outdated"]
    );
    for thread in threads {
        let current = &thread["anchor"]["current"];
        assert_eq!(thread["excerpt"].is_null(), current.is_null(), "{thread}");
        assert_eq!(
            thread["code_context"].is_null(),
            !current.is_null(),
            "{thread}"
        );
        let Some(excerpt) = thread["excerpt"].as_str() else {
            continue;
        };
        let marked: Vec<&str> = excerpt.lines().filter(|row| row.starts_with('>')).collect();
        let code: Vec<&str> = marked
            .iter()
            .map(|row| {
                let text = row.split_once(" |").unwrap().1;
                text.strip_prefix(' ').unwrap_or(text)
            })
            .collect();
        // Rows end without whitespace, so compare with each current line trimmed at its end.
        let current_code: Vec<&str> = thread["current_code"]
            .as_str()
            .unwrap()
            .split('\n')
            .map(str::trim_end)
            .collect();
        assert_eq!(code, current_code, "{thread}");
        if thread["side"] == "new" {
            let numbers: Vec<u64> = marked
                .iter()
                .map(|row| row[1..].split_whitespace().next().unwrap().parse().unwrap())
                .collect();
            let (start, end) = (
                current["start_line"].as_u64().unwrap(),
                current["end_line"].as_u64().unwrap(),
            );
            assert_eq!(numbers, (start..=end).collect::<Vec<_>>(), "{thread}");
        }
    }
    // The binary file's thread has no text to stand in: what stood around it when written.
    let binary = &threads[4];
    assert_eq!(binary["path"], "src/logo.txt");
    assert_eq!(
        binary["code_context"],
        serde_json::json!({"before": [], "after": []})
    );
}

#[test]
fn a_thread_on_a_file_the_review_doesnt_change_is_placed_and_not_in_the_diff() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    // The file view comments on any line: README.md never changed.
    fixture.repo.write("README.md", "# api\n\nSome words.\n");
    fixture.repo.git(&["add", "README.md"]);
    fixture.repo.commit_staged("readme");
    let thread = fixture.thread(&review, "README.md", 3, 3, "Say more?");
    // One the agent's edit was on, which it reverted since.
    let reverted = fixture.thread(&review, "src/app.ts", 5, 5, "Is the id a string?");
    fixture.edit_app(&app_lines());

    let json = fixture.run(&["comment", "list", "--json"]).ok_json();
    let listed = |id: &ThreadId| {
        json["threads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|thread| thread["id"] == id.as_str())
            .unwrap()
            .clone()
    };
    let readme = listed(&thread);
    assert_eq!(readme["placement"], "unchanged", "{readme}");
    assert_eq!(readme["outdated"], false);
    assert_eq!(readme["in_diff"], false);
    assert_eq!(readme["path"], "README.md");
    assert_eq!(
        readme["anchor"]["current"],
        serde_json::json!({"path": "README.md", "side": "new", "start_line": 3, "end_line": 3})
    );
    assert_eq!(readme["current_code"], "Some words.");
    assert!(readme["excerpt"].as_str().unwrap().contains("Some words."));
    let app = listed(&reverted);
    assert_eq!(app["placement"], "unchanged", "{app}");
    assert_eq!(app["in_diff"], false);
    assert_eq!(app["current_code"], "  id: string;");

    let ran = fixture.run(&["comment", "list", "--oneline"]);
    assert!(
        ran.stdout.contains(&format!(
            "{thread}  open  README.md:3 (not in the diff)  Say more?"
        )),
        "{}",
        ran.stdout
    );

    // Lines added above it, committed (still not in the diff): moved.
    fixture
        .repo
        .write("README.md", "# api\n\nIntro.\n\nSome words.\n");
    fixture.repo.git(&["add", "README.md"]);
    fixture.repo.commit_staged("intro");
    let shown = fixture
        .run(&["comment", "show", thread.as_str(), "--json"])
        .ok_json();
    assert_eq!(shown["thread"]["placement"], "moved");
    assert_eq!(shown["thread"]["in_diff"], false);
    assert_eq!(shown["thread"]["anchor"]["current"]["start_line"], 5);

    // The file gone: outdated, still not in the diff.
    fixture.repo.remove("README.md");
    fixture.repo.git(&["add", "-A"]);
    fixture.repo.commit_staged("no readme");
    let shown = fixture
        .run(&["comment", "show", thread.as_str(), "--json"])
        .ok_json();
    assert_eq!(shown["thread"]["placement"], "outdated");
    assert_eq!(shown["thread"]["outdated"], true);
    assert_eq!(shown["thread"]["in_diff"], false);
    assert_eq!(
        shown["thread"]["anchor"]["current"],
        serde_json::Value::Null
    );
}

#[test]
fn a_comment_on_a_whole_unchanged_file_and_an_old_side_thread_there_are_placed() {
    let fixture = Fixture::new();
    let review = fixture.review(Target::Working);
    let place = dv_core::review::AnchorPlace {
        round: 1,
        path: "README.md".to_string(),
        old_path: None,
        side: AnchorSide::New,
    };
    let file_level = ops::add_thread(
        &fixture.store(),
        &review.id,
        place.file_level(),
        "Docs?",
        reviewer(),
    )
    .unwrap();
    // Anchored on the old side while the file was the same text on both.
    let old = fixture.anchor("README.md", AnchorSide::Old, 1, 1);
    let old = ops::add_thread(&fixture.store(), &review.id, old, "Title?", reviewer()).unwrap();

    let json = fixture.run(&["comment", "list", "--json"]).ok_json();
    let listed = |id: &ThreadId| {
        json["threads"]
            .as_array()
            .unwrap()
            .iter()
            .find(|thread| thread["id"] == id.as_str())
            .unwrap()
            .clone()
    };
    let whole = listed(&file_level);
    assert_eq!(whole["placement"], "file", "{whole}");
    assert_eq!(
        (whole["outdated"].clone(), whole["in_diff"].clone()),
        (false.into(), false.into())
    );
    let old_side = listed(&old);
    assert_eq!(old_side["placement"], "unchanged", "{old_side}");
    assert_eq!(old_side["in_diff"], false);
    assert_eq!(old_side["current_code"], "# api");

    let ran = fixture.run(&["comment", "show", file_level.as_str()]);
    assert!(
        ran.stdout.contains(&format!(
            "{file_level}  open  README.md (file, not in the diff)\non the whole file, which the review doesn't change\n"
        )),
        "{}",
        ran.stdout
    );
    // The file gone: outdated.
    fixture.repo.remove("README.md");
    let json = fixture
        .run(&["comment", "show", file_level.as_str(), "--json"])
        .ok_json();
    assert_eq!(json["thread"]["outdated"], true);
}
