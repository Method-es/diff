//! One side's files whole (PLAN §18.1 "The file view"): a file's text at a side is the text the
//! diff pipeline reads for that side, line for line, and a side's paths are what it holds.

use super::*;
use crate::review::{AnchorSide, side_lines};

/// `path` at `side`, which must be text.
fn text_at(git: &GitRepo, side: &Side, path: &str) -> TextDiff {
    match side_file(git, side, path, &DiffLimits::default()) {
        Ok(Some(FileDiff::Text(text))) => text,
        other => panic!("{path} at {side}: expected text, got {other:?}"),
    }
}

fn lines(text: &TextDiff) -> Vec<&str> {
    side_lines(text, AnchorSide::New)
}

#[test]
fn a_side_files_text_is_its_lines_as_context() {
    let repo = FixtureRepo::new();
    repo.write("src/app.ts", "one\ntwo\nthree\n");
    repo.commit("base");
    let git = git_repo(&repo);
    let text = text_at(&git, &Side::Worktree, "src/app.ts");
    assert_eq!(render(&text), [" 1:1 one", " 2:2 two", " 3:3 three"]);
    assert!(!text.has_changes());
    assert!(text.new_eof_newline && text.old_eof_newline);
    // The index and HEAD hold the same.
    assert_eq!(text_at(&git, &Side::Index, "src/app.ts"), text);
    let head = git.head().unwrap().unwrap();
    assert_eq!(text_at(&git, &Side::Commit(head), "src/app.ts"), text);
}

#[test]
fn the_working_tree_side_is_the_new_side_the_stream_shows() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", numbered(8));
    repo.commit("base");
    repo.write(
        "a.txt",
        numbered(8).replace("line 4\n", "line four\nmore\n"),
    );
    let listed = Listed::of(&repo, Target::Working);
    let stream = listed.text("a.txt");
    let whole = text_at(&listed.git, &Side::Worktree, "a.txt");
    assert_eq!(lines(&whole), side_lines(&stream, AnchorSide::New));
    // The index still holds the committed text.
    assert_eq!(
        lines(&text_at(&listed.git, &Side::Index, "a.txt")),
        numbered(8).lines().collect::<Vec<_>>()
    );
}

#[test]
fn crlf_under_autocrlf_reads_as_git_compares_it() {
    let repo = FixtureRepo::new();
    repo.config("core.autocrlf", "true");
    repo.write_crlf("win.txt", &numbered(5));
    repo.commit("base");
    let git = git_repo(&repo);
    // Unchanged in the working tree, whose checkout is CRLF.
    let whole = text_at(&git, &Side::Worktree, "win.txt");
    assert_eq!(lines(&whole), numbered(5).lines().collect::<Vec<_>>());
    assert!(!whole.crlf, "git compares the normalised LF form");
    // Edited, the file view's lines are the stream's.
    repo.write_crlf("win.txt", &numbered(5).replace("line 2\n", "line two\n"));
    let listed = Listed::of(&repo, Target::Working);
    let whole = text_at(&listed.git, &Side::Worktree, "win.txt");
    assert_eq!(
        lines(&whole),
        side_lines(&listed.text("win.txt"), AnchorSide::New)
    );
    assert!(whole.lines.iter().all(|line| !line.text.contains('\r')));
}

#[test]
fn committed_crlf_content_keeps_its_lines_without_the_cr() {
    let repo = FixtureRepo::new();
    repo.config("core.autocrlf", "false");
    repo.write_crlf("dos.txt", "a\nb\n");
    repo.commit("base");
    let git = git_repo(&repo);
    let whole = text_at(&git, &Side::Index, "dos.txt");
    assert_eq!(lines(&whole), ["a", "b"]);
    assert!(whole.crlf);
}

#[test]
fn an_untracked_file_is_on_the_working_tree_side_only() {
    let repo = FixtureRepo::new();
    repo.write("kept.txt", "k\n").write(".gitignore", "*.log\n");
    repo.commit("base");
    repo.write("notes/new file.md", "# Title\nbody\n")
        .write("debug.log", "ignored\n");
    let git = git_repo(&repo);
    let whole = text_at(&git, &Side::Worktree, "notes/new file.md");
    assert_eq!(lines(&whole), ["# Title", "body"]);
    assert_eq!(
        side_file(
            &git,
            &Side::Index,
            "notes/new file.md",
            &DiffLimits::default()
        )
        .unwrap(),
        None
    );
    // An ignored file isn't on any side dv reads.
    assert_eq!(
        side_file(&git, &Side::Worktree, "debug.log", &DiffLimits::default()).unwrap(),
        None
    );
    assert_eq!(
        side_file(&git, &Side::Worktree, "nowhere.txt", &DiffLimits::default()).unwrap(),
        None
    );
}

#[test]
fn a_path_in_a_subdirectory_with_spaces_and_non_ascii() {
    let repo = FixtureRepo::new();
    let path = "src/dir with space/ünïcödé ✓.txt";
    repo.write(path, "héllo\nwörld\n");
    repo.commit("base");
    let git = git_repo(&repo);
    assert_eq!(
        lines(&text_at(&git, &Side::Worktree, path)),
        ["héllo", "wörld"]
    );
    assert_eq!(
        lines(&text_at(&git, &Side::Index, path)),
        ["héllo", "wörld"]
    );
    assert!(
        side_paths(&git, &Side::Worktree)
            .unwrap()
            .contains(&path.to_string())
    );
}

#[test]
fn a_symbolic_link_is_its_target_path() {
    let repo = FixtureRepo::new();
    repo.write("target.txt", "t\n");
    repo.commit("base");
    repo.stage("120000", &repo.hash_blob("target.txt"), "link");
    let commit = repo.commit_staged("link");
    let git = git_repo(&repo);
    for side in [
        Side::Index,
        Side::Commit(crate::git::Oid::parse(&commit).unwrap()),
    ] {
        let whole = text_at(&git, &side, "link");
        assert_eq!(lines(&whole), ["target.txt"]);
        assert!(!whole.new_eof_newline, "a link's target has no newline");
    }
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_in_the_working_tree_is_its_target_path() {
    let repo = FixtureRepo::new();
    repo.write("target.txt", "t\n");
    repo.commit("base");
    std::os::unix::fs::symlink("target.txt", repo.path().join("link")).unwrap();
    let git = git_repo(&repo);
    assert_eq!(
        lines(&text_at(&git, &Side::Worktree, "link")),
        ["target.txt"]
    );
    repo.commit("link");
    assert_eq!(
        lines(&text_at(&git, &Side::Worktree, "link")),
        ["target.txt"]
    );
}

#[test]
fn binary_and_too_large_files_say_so() {
    let repo = FixtureRepo::new();
    repo.write("logo.png", [137u8, 80, 78, 71, 0, 0, 0, 13])
        .write("big.txt", numbered(30));
    repo.commit("base");
    let git = git_repo(&repo);
    assert!(matches!(
        side_file(&git, &Side::Worktree, "logo.png", &DiffLimits::default()).unwrap(),
        Some(FileDiff::Binary { .. })
    ));
    let small = DiffLimits {
        max_bytes: u64::MAX,
        max_lines: 10,
    };
    assert!(matches!(
        side_file(&git, &Side::Worktree, "big.txt", &small).unwrap(),
        Some(FileDiff::TooLarge { .. })
    ));
}

#[test]
fn a_file_deleted_from_the_working_tree_is_still_in_the_index() {
    let repo = FixtureRepo::new();
    repo.write("gone.txt", "g\n").write("kept.txt", "k\n");
    repo.commit("base");
    repo.remove("gone.txt");
    let git = git_repo(&repo);
    assert_eq!(
        side_file(&git, &Side::Worktree, "gone.txt", &DiffLimits::default()).unwrap(),
        None
    );
    assert_eq!(lines(&text_at(&git, &Side::Index, "gone.txt")), ["g"]);
    assert_eq!(side_paths(&git, &Side::Worktree).unwrap(), ["kept.txt"]);
    assert_eq!(
        side_paths(&git, &Side::Index).unwrap(),
        ["gone.txt", "kept.txt"]
    );
}

#[test]
fn a_sides_paths_are_what_it_holds() {
    let repo = FixtureRepo::new();
    repo.write("b.txt", "b\n")
        .write("src/a.ts", "a\n")
        .write(".gitignore", "*.log\n");
    let base = repo.commit("base");
    repo.write("src/new.ts", "n\n").write("x.log", "ignored\n");
    repo.git(&["add", "src/new.ts"]);
    repo.write("untracked.md", "u\n");
    let git = git_repo(&repo);
    assert_eq!(
        side_paths(&git, &Side::Worktree).unwrap(),
        [
            ".gitignore",
            "b.txt",
            "src/a.ts",
            "src/new.ts",
            "untracked.md"
        ]
    );
    assert_eq!(
        side_paths(&git, &Side::Index).unwrap(),
        [".gitignore", "b.txt", "src/a.ts", "src/new.ts"]
    );
    assert_eq!(
        side_paths(&git, &Side::Commit(crate::git::Oid::parse(&base).unwrap())).unwrap(),
        [".gitignore", "b.txt", "src/a.ts"]
    );
    assert!(side_paths(&git, &Side::EmptyTree).unwrap().is_empty());
}

/// How long one file's read at a side takes (the file view's budget, PLAN §18.8: 100 ms for a
/// local repository): `cargo test -p dv_core side_file_timing -- --ignored --nocapture`.
#[test]
#[ignore = "a timing, not a check"]
fn side_file_timing() {
    let repo = FixtureRepo::new();
    let text: String = (1..=5_000)
        .map(|n| format!("export const value_{n} = compute({n}, \"some text\");\n"))
        .collect();
    repo.write("src/big.ts", &text);
    repo.commit("base");
    let git = git_repo(&repo);
    for side in [Side::Worktree, Side::Index] {
        let started = std::time::Instant::now();
        let read = text_at(&git, &side, "src/big.ts");
        println!(
            "{side}: {} lines in {:?}",
            read.lines.len(),
            started.elapsed()
        );
    }
    let started = std::time::Instant::now();
    let paths = side_paths(&git, &Side::Worktree).unwrap();
    println!("paths: {} in {:?}", paths.len(), started.elapsed());
}
