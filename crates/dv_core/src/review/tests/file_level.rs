//! Comments on a whole file (PLAN §2.5, §8): a file-level anchor is placed in its file by the
//! file rule alone, renames and round history included, never relocated, and re-taken under a
//! renamed file's name by the next write.

use crate::diff::{FileEntry, FileMode, FileStatus, file_list};
use crate::review::anchor::AnchorPlace;
use crate::review::history::{History, read_round_history, trace};
use crate::review::ops::{add_thread, finish_round, open_review};
use crate::review::relocate::{
    Placement, Relocation, SideText, place_thread, relocate, relocated_anchor, shows_edited,
    shows_moved,
};
use crate::testing::git_repo;

use super::*;

fn entry(path: &str, old_path: Option<&str>, status: FileStatus) -> FileEntry {
    FileEntry {
        path: path.to_string(),
        old_path: old_path.map(str::to_string),
        status,
        additions: 1,
        deletions: 1,
        binary: false,
        old_mode: Some(FileMode::Regular),
        new_mode: Some(FileMode::Regular),
        old_oid: None,
        new_oid: None,
        new_untracked: false,
    }
}

fn file_thread(path: &str, side: AnchorSide) -> Thread {
    Thread {
        id: ThreadId::generate(),
        anchor: Anchor::file_level(1, path.to_string(), None, side),
        original_anchor: None,
        author: reviewer(),
        body: "About the whole file".to_string(),
        created_at: 1,
        edited_at: None,
        deleted_at: None,
        state: ThreadState::Open,
        last_change: None,
        replies: Vec::new(),
    }
}

/// Placement never reads a file-level thread's text.
fn never(_: &FileEntry, _: AnchorSide) -> SideText<Vec<&'static str>> {
    panic!("a comment on a whole file reads no text")
}

#[test]
fn a_file_thread_is_placed_in_its_file_without_reading_it() {
    let thread = file_thread("a.rs", AnchorSide::New);
    let modified = [entry("a.rs", None, FileStatus::Modified)];
    let placed = place_thread(&thread, &modified, History::None, never);
    assert_eq!(
        placed,
        Placement::File {
            file: "a.rs".to_string()
        }
    );
    assert_eq!(placed.as_str(), "file");
    assert!(placed.is_file() && !placed.is_outdated());
    assert_eq!(placed.lines(&thread.anchor), None);
    assert_eq!(placed.file(), Some("a.rs"));
    assert!(!shows_moved(&thread, &placed) && !shows_edited(&thread, &placed));
    // Unchanged in place, a write records nothing.
    assert_eq!(
        relocated_anchor(&thread.anchor, &placed, &modified[0], &[] as &[&str], 2),
        None
    );

    // Any status with its side holds it, a binary file included (nothing is read).
    for status in [
        FileStatus::Added,
        FileStatus::Untracked,
        FileStatus::TypeChanged,
    ] {
        let listed = [entry("a.rs", None, status)];
        assert!(place_thread(&thread, &listed, History::None, never).is_file());
    }
    let binary = FileEntry {
        binary: true,
        ..entry("a.rs", None, FileStatus::Modified)
    };
    assert!(place_thread(&thread, &[binary], History::None, never).is_file());
}

#[test]
fn a_file_thread_follows_a_rename_and_records_the_new_name() {
    let thread = file_thread("a.rs", AnchorSide::New);
    let renamed = entry("b.rs", Some("a.rs"), FileStatus::Renamed { similarity: 80 });
    let placed = place_thread(
        &thread,
        &[entry("c.rs", None, FileStatus::Added), renamed.clone()],
        History::None,
        never,
    );
    assert_eq!(
        placed,
        Placement::File {
            file: "b.rs".to_string()
        }
    );
    let recorded = relocated_anchor(&thread.anchor, &placed, &renamed, &[] as &[&str], 2)
        .expect("the rename is recorded");
    assert!(recorded.is_file_level());
    assert_eq!(
        (
            recorded.path.as_str(),
            recorded.old_path.as_deref(),
            recorded.round
        ),
        ("b.rs", Some("a.rs"), 2)
    );
    // Recorded, it stands in the file once the rename is committed.
    let recorded_thread = Thread {
        anchor: recorded,
        original_anchor: Some(thread.anchor.clone()),
        ..thread.clone()
    };
    let committed = [entry("b.rs", None, FileStatus::Modified)];
    assert!(place_thread(&recorded_thread, &committed, History::None, never).is_file());
    assert!(!shows_moved(
        &recorded_thread,
        &place_thread(&recorded_thread, &committed, History::None, never)
    ));
}

#[test]
fn a_file_thread_on_a_file_gone_or_without_its_side_is_outdated() {
    let new_side = file_thread("a.rs", AnchorSide::New);
    // Deleted since: outdated in that file.
    let deleted = [entry("a.rs", None, FileStatus::Deleted)];
    assert_eq!(
        place_thread(&new_side, &deleted, History::None, never),
        Placement::Outdated {
            file: Some("a.rs".to_string()),
            trail: None
        }
    );
    // Its round's history not read yet: it waits, the deleted file named.
    assert_eq!(
        place_thread(&new_side, &deleted, History::Pending, never),
        Placement::Pending {
            file: Some("a.rs".to_string())
        }
    );
    // Its file left the list: the "Not in the diff" section.
    assert_eq!(
        place_thread(
            &new_side,
            &[entry("other.rs", None, FileStatus::Modified)],
            History::None,
            never
        ),
        Placement::Outdated {
            file: None,
            trail: None
        }
    );
    // The old side of a deleted file is where a comment on it goes; an added file has none.
    let old_side = file_thread("a.rs", AnchorSide::Old);
    assert!(place_thread(&old_side, &deleted, History::None, never).is_file());
    assert!(
        place_thread(
            &old_side,
            &[entry("a.rs", None, FileStatus::Added)],
            History::None,
            never
        )
        .is_outdated()
    );
}

#[test]
fn nothing_relocates_or_traces_a_file_level_anchor() {
    let anchor = Anchor::file_level(1, "a.rs".to_string(), None, AnchorSide::New);
    let lines = ["fn main() {}", "", "}"];
    assert_eq!(relocate(&anchor, &lines), Relocation::Outdated);
    assert_eq!(relocate(&anchor, &[] as &[&str]), Relocation::Outdated);
    assert!(!anchor.matches(&lines));
    assert_eq!(trace(&anchor, &lines, &["other"]), None);
    let place = AnchorPlace {
        round: 1,
        path: "a.rs".to_string(),
        old_path: None,
        side: AnchorSide::New,
    };
    assert_eq!(place.file_level(), anchor);
}

const HOOK: &str = "import { useAuth } from \"./use-auth\";\n\nexport function useCan(permission: string) {\n  const { user } = useAuth();\n  if (!user) {\n    return false;\n  }\n  return user.permissions.includes(permission);\n}\n";

/// A `working` review of an untracked hook with a comment on the whole file, round 1 closed.
fn file_comment_review() -> (dv_test_support::FixtureRepo, ReviewStore, Review, ThreadId) {
    let (repo, store) = fixture_store();
    repo.write("src/hooks/use-can.ts", HOOK);
    let opened = open_review(&store, Target::Working, reviewer()).unwrap();
    let anchor = Anchor::file_level(1, "src/hooks/use-can.ts".to_string(), None, AnchorSide::New);
    let thread = add_thread(
        &store,
        &opened.review.id,
        anchor,
        "Split this hook",
        reviewer(),
    )
    .unwrap();
    let finished = finish_round(
        &store,
        &git_repo(&repo),
        &opened.review.id,
        Some(Verdict::RequestChanges),
        None,
        &reviewer(),
    )
    .unwrap();
    (repo, store, finished.review, thread)
}

/// Where `thread` stands now as the CLI places it, its round's history read for its path the
/// way the CLI reads it (only once placing it without one leaves it outdated), and the entry
/// it is in.
fn place(
    repo: &dv_test_support::FixtureRepo,
    review: &Review,
    thread: &ThreadId,
) -> (Placement, Option<FileEntry>) {
    let git = git_repo(repo);
    let thread = review.thread(thread).unwrap();
    let resolved = review.target.resolve(&git).unwrap();
    let entries = file_list(&git, &resolved).unwrap();
    let without = place_thread(thread, &entries, History::None, never);
    let placement = if without.is_outdated() {
        let round = &review.rounds[thread.anchor.round as usize - 1];
        let history = read_round_history(
            &git,
            &resolved,
            round,
            std::slice::from_ref(&thread.anchor.path),
            None,
        )
        .unwrap();
        place_thread(thread, &entries, History::Read(&history), never)
    } else {
        without
    };
    let entry = placement
        .file()
        .and_then(|path| entries.iter().find(|entry| entry.path == path))
        .cloned();
    (placement, entry)
}

#[test]
fn a_file_thread_follows_an_untracked_file_renamed_between_rounds() {
    let (repo, _store, review, thread) = file_comment_review();
    repo.remove("src/hooks/use-can.ts");
    repo.write(
        "src/hooks/use-has-permission.ts",
        HOOK.replace("useCan", "useHasPermission"),
    );
    let (placement, entry) = place(&repo, &review, &thread);
    assert_eq!(
        placement,
        Placement::File {
            file: "src/hooks/use-has-permission.ts".to_string()
        }
    );
    let entry = entry.unwrap();
    assert_eq!(entry.status, FileStatus::Untracked);
    // The next write records it under the name now, still a comment on the whole file.
    let found = review.thread(&thread).unwrap();
    let recorded = relocated_anchor(&found.anchor, &placement, &entry, &[] as &[&str], 2).unwrap();
    assert!(recorded.is_file_level());
    assert_eq!(recorded.path, "src/hooks/use-has-permission.ts");
}

#[test]
fn a_file_thread_whose_file_was_deleted_since_leaves_the_diff_or_is_outdated_in_it() {
    // An untracked file deleted: nothing lists it, nothing pairs it.
    let (repo, _store, review, thread) = file_comment_review();
    repo.remove("src/hooks/use-can.ts");
    let (placement, _) = place(&repo, &review, &thread);
    assert_eq!(
        placement,
        Placement::Outdated {
            file: None,
            trail: None
        }
    );

    // A tracked file deleted: the review lists it deleted, and the thread is outdated there.
    let (repo, store) = fixture_store();
    repo.write("src/lib.rs", "one\ntwo\nthree\nfour\n");
    let opened = open_review(&store, Target::Working, reviewer()).unwrap();
    let anchor = Anchor::file_level(1, "src/lib.rs".to_string(), None, AnchorSide::New);
    let thread = add_thread(&store, &opened.review.id, anchor, "Keep it?", reviewer()).unwrap();
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
    repo.remove("src/lib.rs");
    let (placement, entry) = place(&repo, &finished.review, &thread);
    assert_eq!(
        placement,
        Placement::Outdated {
            file: Some("src/lib.rs".to_string()),
            trail: None
        }
    );
    assert_eq!(entry.unwrap().status, FileStatus::Deleted);
    // A comment on its old side stands in it, and anchors with the store's checks.
    let deleted = file_list(&git, &finished.review.target.resolve(&git).unwrap()).unwrap();
    let deleted = deleted.iter().find(|e| e.path == "src/lib.rs").unwrap();
    let place = AnchorPlace::for_entry(deleted, AnchorSide::Old, 2);
    let old = add_thread(
        &store,
        &opened.review.id,
        place.file_level(),
        "Why delete it?",
        reviewer(),
    )
    .unwrap();
    let review = store.load(&opened.review.id).unwrap().review;
    let found = review.thread(&old).unwrap();
    let entries = file_list(&git, &review.target.resolve(&git).unwrap()).unwrap();
    assert!(place_thread(found, &entries, History::None, never).is_file());
}
