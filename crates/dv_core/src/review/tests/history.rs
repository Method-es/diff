//! A thread's round history (PLAN §8, "Round history"): where its code went ([`trace`]), its
//! file followed through renames since its round on fixture repos (git's, and an untracked
//! file paired by content), and placement with it.

use crate::diff::{DiffLimits, FileStatus, file_diff, file_list};
use crate::review::anchor::{AnchorPlace, anchor_selection, side_lines};
use crate::review::history::{
    FileHistory, History, RoundHistory, Trail, Went, content_match, content_similarity,
    is_substantive, read_round_history, trace,
};
use crate::review::ops::{add_thread, finish_round, open_review};
use crate::review::relocate::REWRITE_SLACK;
use crate::review::relocate::{Placement, SideText, place_thread, relocated_anchor};
use crate::testing::git_repo;

use super::*;

/// `count` distinct lines of code-like text.
fn text(count: usize) -> Vec<String> {
    (1..=count)
        .map(|n| format!("    let value_{n} = compute(request, {n}); // line {n}"))
        .collect()
}

/// The new-side anchor of lines `from..=to` of `lines`, in round 1 of `path`.
fn anchor_in(path: &str, lines: &[String], from: u32, to: u32) -> Anchor {
    anchor_selection(
        AnchorPlace {
            round: 1,
            path: path.to_string(),
            old_path: None,
            side: AnchorSide::New,
        },
        lines,
        from,
        to,
    )
    .unwrap()
}

fn replaced(start_line: u32, end_line: u32) -> Option<Went> {
    Some(Went::Replaced {
        start_line,
        end_line,
    })
}

fn removed(before_line: u32) -> Option<Went> {
    Some(Went::Removed { before_line })
}

/// `lines` with `from..=to` (1-based) replaced by `with`.
fn splice(lines: &[String], from: usize, to: usize, with: &[&str]) -> Vec<String> {
    let mut out = lines.to_vec();
    out.splice(from - 1..to, with.iter().map(|line| line.to_string()));
    out
}

#[test]
fn code_replaced_since_the_round_points_at_what_replaced_it() {
    let snapshot = text(40);
    let anchor = anchor_in("a.rs", &snapshot, 10, 11);
    // Lines 10–11 rewritten into three new lines, some lines inserted above as well.
    let mut now = splice(&snapshot, 10, 11, &["fn a() {", "    b();", "}"]);
    now.splice(2..2, ["// one".to_string(), "// two".to_string()]);
    assert_eq!(trace(&anchor, &snapshot, &now), replaced(12, 14));
    // Found in the snapshot elsewhere than its lines (it moved before the round closed): the
    // lines it stood at then are followed.
    let mut moved = snapshot.clone();
    moved.splice(0..0, ["// header".to_string()]);
    let anchor_then = anchor_in("a.rs", &snapshot, 10, 11);
    let now = splice(&moved, 11, 12, &["something else entirely"]);
    assert_eq!(trace(&anchor_then, &moved, &now), replaced(11, 11));
}

#[test]
fn code_removed_since_the_round_says_where_it_stood() {
    let snapshot = text(20);
    // In the middle.
    let anchor = anchor_in("a.rs", &snapshot, 8, 9);
    let now = splice(&snapshot, 8, 9, &[]);
    assert_eq!(trace(&anchor, &snapshot, &now), removed(8));
    // At the top.
    let anchor = anchor_in("a.rs", &snapshot, 1, 2);
    let now = splice(&snapshot, 1, 2, &[]);
    assert_eq!(trace(&anchor, &snapshot, &now), removed(1));
    // At the end: one past the last line now.
    let anchor = anchor_in("a.rs", &snapshot, 19, 20);
    let now = splice(&snapshot, 19, 20, &[]);
    assert_eq!(trace(&anchor, &snapshot, &now), removed(19));
    assert_eq!(now.len(), 18);
}

#[test]
fn an_insertion_counts_only_between_two_changed_blocks() {
    let snapshot = text(20);
    let anchor = anchor_in("a.rs", &snapshot, 8, 10);
    // Lines 8 and 10 rewritten, a line inserted between them, 9 kept: the replacement runs
    // from the first rewritten line to the last, the insertion and line 9 among them.
    let mut now = snapshot.clone();
    now[7] = "first rewritten".to_string();
    now[9] = "last rewritten".to_string();
    now.insert(8, "inserted".to_string());
    assert_eq!(trace(&anchor, &snapshot, &now), replaced(8, 11));
    // Line 8 deleted and a line inserted after line 9, still inside the range but after the
    // only changed block: the insertion doesn't count, so the code was removed (where line 8
    // stood), never "replaced" by its own line 9 left standing.
    let mut now = snapshot.clone();
    now.insert(9, "inserted".to_string());
    now.remove(7);
    assert_eq!(trace(&anchor, &snapshot, &now), removed(8));
}

#[test]
fn deletions_alone_are_removed_never_replaced_by_the_lines_left() {
    let snapshot = text(20);
    let anchor = anchor_in("a.rs", &snapshot, 8, 10);
    // Lines 8 and 10 deleted, line 9 left between: removed, before where line 8 stood.
    let mut now = snapshot.clone();
    now.remove(9);
    now.remove(7);
    assert_eq!(trace(&anchor, &snapshot, &now), removed(8));
}

#[test]
fn a_file_rewritten_whole_points_at_the_start_of_the_rewrite_only() {
    let snapshot = text(30);
    let anchor = anchor_in("a.rs", &snapshot, 20, 21);
    let now: Vec<String> = (1..=200).map(|n| format!("rewritten {n}")).collect();
    // The whole file is one change block: clipped to the anchored count plus the slack.
    let clipped = u32::try_from(2 + REWRITE_SLACK).unwrap();
    assert_eq!(trace(&anchor, &snapshot, &now), replaced(1, clipped));
}

#[test]
fn no_trail_for_an_anchor_not_in_the_snapshot_or_on_the_old_side() {
    let written = text(20);
    let anchor = anchor_in("a.rs", &written, 5, 5);
    // The snapshot no longer holds the anchored text (edited before the round closed).
    let snapshot: Vec<String> = (1..=20).map(|n| format!("other {n}")).collect();
    let now = splice(&snapshot, 5, 5, &["changed"]);
    assert_eq!(trace(&anchor, &snapshot, &now), None);
    // A snapshot is a new side: an old-side anchor has no trail.
    let old = Anchor {
        side: AnchorSide::Old,
        ..anchor_in("a.rs", &written, 5, 5)
    };
    assert_eq!(trace(&old, &written, &splice(&written, 5, 5, &["x"])), None);
    // Lines the diff leaves as they were.
    assert_eq!(trace(&anchor, &written, &written), None);
}

#[test]
fn content_similarity_weighs_substantive_lines_by_their_length() {
    // `}` and `);` say nothing; each other line weighs its trimmed length.
    assert!(is_substantive("  let x = 1;"));
    assert!(!is_substantive("  });"));
    assert!(!is_substantive("*/"));
    assert_eq!(
        content_similarity(&["abcd", "  }", "", "xyz1"], &["abcd  ", "efgh"]),
        2.0 * 4.0 / 16.0
    );
    assert_eq!(
        content_similarity(&["abc", "abc"], &["abc"]),
        2.0 * 3.0 / 9.0
    );
    let empty: [&str; 0] = [];
    assert_eq!(content_similarity(&empty, &["  ", "}"]), 0.0);
}

#[test]
fn small_unrelated_files_never_pair() {
    // Two six-line functions that share `return 0;` and their braces: half their lines alike,
    // nothing of substance.
    let a = [
        "function count(items) {",
        "  if (!items) {",
        "    return 0;",
        "  }",
        "  return items.length;",
        "}",
    ];
    let b = [
        "function parse(text) {",
        "  if (text === \"\") {",
        "    return 0;",
        "  }",
        "  return Number(text);",
        "}",
    ];
    assert_eq!(content_match(&a, &b), None);
    // Two barrel files sharing one of their two lines.
    let a = ["export * from './types';", "export * from './utils';"];
    let b = ["export * from './types';", "export * from './hooks';"];
    assert_eq!(content_match(&a, &b), None);
    // The owner's rename: the same hook, its function renamed.
    let score = content_match(&lines_of(USE_CAN), &lines_of(USE_HAS_PERMISSION));
    assert!(score.is_some_and(|score| score > 0.5), "{score:?}");
}

/// The hook the owner's agent wrote, then renamed between rounds (DESIGN §4.1's
/// `use-can.ts` → `use-has-permission.ts`).
const USE_CAN: &str = "import { useAuth } from \"./use-auth\";\n\nexport function useCan(permission: string) {\n  const { user } = useAuth();\n  if (!user) {\n    return false;\n  }\n  return user.permissions.includes(permission);\n}\n";

/// [`USE_CAN`] renamed, its function renamed with it.
const USE_HAS_PERMISSION: &str = "import { useAuth } from \"./use-auth\";\n\nexport function useHasPermission(permission: string) {\n  const { user } = useAuth();\n  if (!user) {\n    return false;\n  }\n  return user.permissions.includes(permission);\n}\n";

fn lines_of(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

/// A repo with a base commit, the agent's untracked hook, a `working` review with a thread on
/// the hook's line 4 and round 1 closed.
fn untracked_hook_review() -> (FixtureRepo, ReviewStore, Review, ThreadId) {
    let (repo, store) = fixture_store();
    repo.write("src/hooks/use-can.ts", USE_CAN);
    let opened = open_review(&store, Target::Working, reviewer()).unwrap();
    let anchor = anchor_in("src/hooks/use-can.ts", &lines_of(USE_CAN), 4, 4);
    let thread = add_thread(
        &store,
        &opened.review.id,
        anchor,
        "A missing user?",
        reviewer(),
    )
    .unwrap();
    let git = git_repo(&repo);
    let finished = finish_round(
        &store,
        &git,
        &opened.review.id,
        Some(Verdict::RequestChanges),
        None,
        &reviewer(),
    )
    .unwrap();
    (repo, store, finished.review, thread)
}

/// Where `thread` of `review` stands now, as the CLI places it: the target resolved, its file
/// list, every file's full text, and its round's history read for its path.
fn place(repo: &FixtureRepo, review: &Review, thread: &ThreadId) -> (Placement, RoundHistory) {
    let git = git_repo(repo);
    let thread = review.thread(thread).unwrap();
    let resolved = review.target.resolve(&git).unwrap();
    let entries = file_list(&git, &resolved).unwrap();
    let round = &review.rounds[thread.anchor.round as usize - 1];
    let history = read_round_history(
        &git,
        &resolved,
        round,
        std::slice::from_ref(&thread.anchor.path),
        None,
    )
    .unwrap();
    // A second read, from the first, takes the snapshot's texts from it: the same history.
    let again = read_round_history(
        &git,
        &resolved,
        round,
        std::slice::from_ref(&thread.anchor.path),
        Some(&history),
    )
    .unwrap();
    assert_eq!(again, history);
    let placement = place_thread(thread, &entries, History::Read(&history), |entry, side| {
        let diff = file_diff(&git, &resolved, entry, &DiffLimits::UNLIMITED).unwrap();
        match diff.as_text() {
            Some(text) => SideText::Lines(
                side_lines(text, side)
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<_>>(),
            ),
            None => SideText::Unavailable,
        }
    });
    (placement, history)
}

#[test]
fn an_untracked_file_renamed_between_rounds_is_followed_by_its_content() {
    let (repo, store, review, thread) = untracked_hook_review();
    // The agent renames the hook it never committed: the review's list shows only the new
    // name, untracked, and git can't pair the two.
    repo.remove("src/hooks/use-can.ts");
    repo.write("src/hooks/use-has-permission.ts", USE_HAS_PERMISSION);
    // A file of the same extension with other content is no match.
    repo.write("src/hooks/use-theme.ts", "export const theme = \"dark\";\n");

    let (placement, history) = place(&repo, &review, &thread);
    assert_eq!(
        history.file("src/hooks/use-can.ts"),
        Some(&FileHistory {
            now: Some("src/hooks/use-has-permission.ts".to_string()),
            snapshot: Some(lines_of(USE_CAN)),
        })
    );
    assert_eq!(
        placement,
        Placement::Unchanged {
            file: "src/hooks/use-has-permission.ts".to_string()
        }
    );
    assert!(
        history
            .read_paths()
            .contains("src/hooks/use-has-permission.ts")
    );

    // A reply records the thread under the file's name now.
    let git = git_repo(&repo);
    let resolved = review.target.resolve(&git).unwrap();
    let entries = file_list(&git, &resolved).unwrap();
    let entry = entries
        .iter()
        .find(|entry| entry.path == "src/hooks/use-has-permission.ts")
        .unwrap();
    assert_eq!(entry.status, FileStatus::Untracked);
    let diff = file_diff(&git, &resolved, entry, &DiffLimits::UNLIMITED).unwrap();
    let lines = side_lines(diff.as_text().unwrap(), AnchorSide::New);
    let found = store.load(&review.id).unwrap().review;
    let found = found.thread(&thread).unwrap();
    let recorded = relocated_anchor(&found.anchor, &placement, entry, &lines, 2).unwrap();
    assert_eq!(recorded.path, "src/hooks/use-has-permission.ts");
    assert_eq!((recorded.start_line, recorded.round), (4, 2));
}

#[test]
fn an_untracked_file_too_unlike_the_snapshot_is_not_paired() {
    let (repo, _store, review, thread) = untracked_hook_review();
    repo.remove("src/hooks/use-can.ts");
    // Half its lines kept or fewer: under git's rename threshold.
    repo.write(
        "src/hooks/use-has-permission.ts",
        "export function other() {\n  return 1;\n}\nconst a = 1;\nconst b = 2;\nconst c = 3;\n  return false;\n",
    );
    let (placement, history) = place(&repo, &review, &thread);
    let file = history.file("src/hooks/use-can.ts").unwrap();
    assert_eq!(file.now, None);
    assert!(
        content_match(
            file.snapshot.as_deref().unwrap(),
            &lines_of(
                &std::fs::read_to_string(repo.path().join("src/hooks/use-has-permission.ts"))
                    .unwrap()
            )
        )
        .is_none()
    );
    assert_eq!(
        placement,
        Placement::Outdated {
            file: None,
            trail: None
        }
    );
}

#[test]
fn a_tie_between_candidates_goes_to_the_one_ranked_first() {
    let (repo, _store, review, thread) = untracked_hook_review();
    repo.remove("src/hooks/use-can.ts");
    // Two copies alike: the one with the hook's extension wins over another extension, and
    // between two of the same extension, the name more like the hook's.
    repo.write("src/hooks/copy.js", USE_HAS_PERMISSION);
    repo.write("src/hooks/use-can-do.ts", USE_HAS_PERMISSION);
    repo.write("src/hooks/zzz.ts", USE_HAS_PERMISSION);
    let (placement, history) = place(&repo, &review, &thread);
    assert_eq!(
        history.file("src/hooks/use-can.ts").unwrap().now.as_deref(),
        Some("src/hooks/use-can-do.ts")
    );
    assert_eq!(placement.file(), Some("src/hooks/use-can-do.ts"));
}

#[test]
fn a_file_committed_on_a_branch_and_renamed_between_rounds_follows_gits_rename() {
    let (repo, store) = fixture_store();
    repo.checkout("main");
    repo.git(&["checkout", "-q", "-b", "feat"]);
    repo.write("src/hooks/use-can.ts", USE_CAN);
    repo.commit("add the hook");
    repo.checkout("main");
    let target = Target::Branch {
        branch: "feat".to_string(),
        base: Some("main".to_string()),
    };
    let opened = open_review(&store, target, reviewer()).unwrap();
    let anchor = anchor_in("src/hooks/use-can.ts", &lines_of(USE_CAN), 8, 8);
    let thread = add_thread(
        &store,
        &opened.review.id,
        anchor,
        "Why `includes`?",
        reviewer(),
    )
    .unwrap();
    let git = git_repo(&repo);
    finish_round(
        &store,
        &git,
        &opened.review.id,
        Some(Verdict::RequestChanges),
        None,
        &reviewer(),
    )
    .unwrap();
    // The agent renames it on the branch and rewrites the commented line: the branch's list
    // has only the new name, added (it never existed on main).
    repo.checkout("feat");
    repo.remove("src/hooks/use-can.ts");
    repo.write(
        "src/hooks/use-has-permission.ts",
        USE_HAS_PERMISSION.replace(
            "  return user.permissions.includes(permission);\n",
            "  throw new Error(\"forbidden\");\n  process.exit(1);\n",
        ),
    );
    repo.commit("rename the hook");
    repo.checkout("main");

    let review = store.load(&opened.review.id).unwrap().review;
    let (placement, history) = place(&repo, &review, &thread);
    let file = history.file("src/hooks/use-can.ts").unwrap();
    assert_eq!(file.now.as_deref(), Some("src/hooks/use-has-permission.ts"));
    assert_eq!(file.snapshot.as_deref(), Some(&lines_of(USE_CAN)[..]));
    // Its line was rewritten into two: outdated, the trail at those two lines.
    assert_eq!(
        placement,
        Placement::Outdated {
            file: Some("src/hooks/use-has-permission.ts".to_string()),
            trail: Some(Trail {
                round: 1,
                went: Went::Replaced {
                    start_line: 8,
                    end_line: 9
                }
            })
        }
    );
    assert!(placement.is_outdated());
    assert_eq!(
        placement.lines(&review.thread(&thread).unwrap().anchor),
        None
    );
}

#[test]
fn code_rewritten_since_the_round_is_outdated_with_its_trail_and_records_nothing() {
    let (repo, store) = fixture_store();
    let written = text(30);
    repo.write("src/app.rs", written.join("\n") + "\n");
    let opened = open_review(&store, Target::Working, reviewer()).unwrap();
    let anchor = anchor_in("src/app.rs", &written, 12, 13);
    let thread = add_thread(&store, &opened.review.id, anchor, "Split this.", reviewer()).unwrap();
    let git = git_repo(&repo);
    let finished = finish_round(
        &store,
        &git,
        &opened.review.id,
        Some(Verdict::RequestChanges),
        None,
        &reviewer(),
    )
    .unwrap();
    let now = splice(
        &written,
        12,
        13,
        &["fn split_a() {}", "fn split_b() {}", "fn split_c() {}"],
    );
    repo.write("src/app.rs", now.join("\n") + "\n");

    let (placement, _) = place(&repo, &finished.review, &thread);
    let trail = Trail {
        round: 1,
        went: Went::Replaced {
            start_line: 12,
            end_line: 14,
        },
    };
    assert_eq!(
        placement,
        Placement::Outdated {
            file: Some("src/app.rs".to_string()),
            trail: Some(trail)
        }
    );
    assert_eq!(placement.as_str(), "outdated");
    assert_eq!(trail.after_line(), Some(14));
    assert_eq!(trail.lines(), (12, 14));
    // Its history not read yet, it waits rather than show outdated first.
    let found = finished.review.thread(&thread).unwrap();
    let entries = file_list(&git, &finished.review.target.resolve(&git).unwrap()).unwrap();
    let pending = place_thread(found, &entries, History::Pending, |_, _| {
        SideText::Lines(now.clone())
    });
    assert_eq!(
        pending,
        Placement::Pending {
            file: Some("src/app.rs".to_string())
        }
    );
    // A round in progress has no history: outdated, without a trail.
    let plain = place_thread(found, &entries, History::None, |_, _| {
        SideText::Lines(now.clone())
    });
    assert_eq!(
        plain,
        Placement::Outdated {
            file: Some("src/app.rs".to_string()),
            trail: None
        }
    );
    // A reply records no anchor from a trail: the thread isn't re-anchored onto other code.
    let entry = entries
        .iter()
        .find(|entry| entry.path == "src/app.rs")
        .unwrap();
    assert_eq!(
        relocated_anchor(&found.anchor, &placement, entry, &now, 2),
        None
    );
}

#[test]
fn removed_at_the_top_heads_the_file_and_a_round_says_nothing_of_a_path_it_wasnt_read_for() {
    let trail = Trail {
        round: 2,
        went: Went::Removed { before_line: 1 },
    };
    assert_eq!(trail.after_line(), None);
    assert_eq!(trail.lines(), (1, 1));
    let trail = Trail {
        round: 2,
        went: Went::Removed { before_line: 7 },
    };
    assert_eq!(trail.after_line(), Some(6));

    // A history read for other paths: the thread waits for one read for its own.
    let mut history = RoundHistory::new(1);
    history.insert(
        "other.rs",
        FileHistory {
            now: Some("other.rs".to_string()),
            snapshot: None,
        },
    );
    let lines = text(10);
    let thread = Thread {
        id: ThreadId::generate(),
        anchor: anchor_in("gone.rs", &lines, 2, 2),
        original_anchor: None,
        author: reviewer(),
        body: "Why?".to_string(),
        created_at: 1,
        edited_at: None,
        deleted_at: None,
        state: ThreadState::Open,
        last_change: None,
        replies: Vec::new(),
    };
    let placed = place_thread(&thread, &[], History::Read(&history), |_, _| {
        SideText::Lines(lines.clone())
    });
    assert_eq!(placed, Placement::Pending { file: None });
}
