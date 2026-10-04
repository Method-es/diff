//! Per-file diff fixture tests (PLAN §6 step 2, D4).

use std::ops::Range;

use super::*;

/// A file edited in one line, committed, as the target `commit HEAD`.
fn one_commit(path: &str, before: &str, after: &str) -> (FixtureRepo, Listed) {
    let repo = FixtureRepo::new();
    repo.write(path, before);
    repo.commit("before");
    repo.write(path, after);
    let after = repo.commit("after");
    let listed = Listed::of(&repo, Target::Commit { rev: after });
    (repo, listed)
}

#[test]
fn a_modified_file_has_both_sides_complete_and_aligned() {
    let before = numbered(30);
    let after = before.replace("line 15\n", "line fifteen\n");
    let (_repo, listed) = one_commit("f.txt", &before, &after);
    let text = listed.text("f.txt");
    assert_eq!(text.lines.len(), 31, "every line of both sides");
    assert_eq!(text.old_line_count(), 30);
    assert_eq!(text.new_line_count(), 30);
    let rendered = render(&text);
    assert_eq!(rendered[0], " 1:1 line 1");
    assert_eq!(rendered[14], "-15: line 15");
    assert_eq!(rendered[15], "+:15 line fifteen");
    assert_eq!(rendered[30], " 30:30 line 30");
    assert!(text.old_eof_newline && text.new_eof_newline && !text.crlf);
    // Intraline marks the changed word only.
    assert_eq!(text.lines[14].words, [Range { start: 5, end: 7 }]);
    assert_eq!(text.lines[15].words, [Range { start: 5, end: 12 }]);
    // Collapsed with 3 lines of context.
    assert_eq!(
        listed.diff("f.txt").segments(3),
        vec![
            Segment::Gap {
                lines: 0..11,
                expandable_above: false,
                expandable_below: true
            },
            Segment::Lines(11..19),
            Segment::Gap {
                lines: 19..31,
                expandable_above: true,
                expandable_below: false
            },
        ]
    );
}

#[test]
fn a_real_binary_file() {
    let repo = FixtureRepo::new();
    repo.write("img.png", b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".as_slice());
    repo.commit("base");
    repo.write(
        "img.png",
        b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR-changed".as_slice(),
    );
    let listed = Listed::of(&repo, Target::Working);
    let entry = listed.entry("img.png");
    assert!(entry.binary);
    assert_eq!((entry.additions, entry.deletions), (0, 0));
    assert_eq!(
        listed.diff("img.png"),
        FileDiff::Binary {
            old_len: Some(16),
            new_len: Some(24)
        }
    );
}

#[test]
fn rename_without_edits() {
    let repo = FixtureRepo::new();
    repo.write("old/name.txt", numbered(10));
    repo.commit("base");
    repo.rename("old/name.txt", "new/name.txt");
    let moved = repo.commit("move");
    let listed = Listed::of(&repo, Target::Commit { rev: moved });
    assert_eq!(
        listed.summary(),
        [(
            "new/name.txt",
            FileStatus::Renamed { similarity: 100 },
            0,
            0
        )]
    );
    assert_eq!(
        listed.entry("new/name.txt").old_path.as_deref(),
        Some("old/name.txt")
    );
    // Nothing changed inside: no lines, like GitHub's "renamed without changes".
    assert_eq!(listed.text("new/name.txt"), TextDiff::empty());
}

#[test]
fn rename_with_edits() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", numbered(20));
    repo.commit("base");
    repo.remove("a.txt")
        .write("b.txt", numbered(20).replace("line 10\n", "line ten\n"));
    // Uncommitted but staged, so the working tree target pairs the two paths.
    repo.git(&["add", "-A"]);
    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(listed.entries.len(), 1, "{:#?}", listed.entries);
    let entry = listed.entry("b.txt");
    assert!(
        matches!(entry.status, FileStatus::Renamed { similarity } if (90..100).contains(&similarity)),
        "{entry:?}"
    );
    assert_eq!((entry.additions, entry.deletions), (1, 1));
    assert_eq!(entry.old_path.as_deref(), Some("a.txt"));
    let text = listed.text("b.txt");
    assert_eq!(text.lines.len(), 21);
    let changes: Vec<_> = render(&text)
        .into_iter()
        .filter(|line| !line.starts_with(' '))
        .collect();
    assert_eq!(changes, ["-10: line 10", "+:10 line ten"]);
}

#[test]
fn a_deleted_file() {
    let repo = FixtureRepo::new();
    repo.write("gone.txt", "a\nb\n").write("keep.txt", "k\n");
    repo.commit("base");
    repo.remove("gone.txt");
    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(listed.summary(), [("gone.txt", FileStatus::Deleted, 0, 2)]);
    let text = listed.text("gone.txt");
    assert_eq!(render(&text), ["-1: a", "-2: b"]);
}

#[test]
fn untracked_text_file() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    repo.write(
        "notes/new file.md",
        "# Title\n\nbody\nlast line, no newline",
    );
    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(
        listed.summary(),
        [("notes/new file.md", FileStatus::Untracked, 4, 0)]
    );
    let text = listed.text("notes/new file.md");
    assert_eq!(
        render(&text),
        [
            "+:1 # Title",
            "+:2 ",
            "+:3 body",
            "+:4 last line, no newline"
        ]
    );
    assert!(text.old_eof_newline);
    assert!(!text.new_eof_newline);
}

#[test]
fn untracked_binary_file() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    let mut bytes = b"GIF89a".to_vec();
    bytes.extend([0u8; 10]);
    bytes.extend(b"\nline\nline\n");
    repo.write("anim.gif", &bytes);
    // A NUL past git's 8000-byte sniff doesn't make a file binary.
    let mut late_nul = "x\n".repeat(4500).into_bytes();
    late_nul.push(0);
    repo.write("late-nul.txt", &late_nul);

    let listed = Listed::of(&repo, Target::Working);
    let gif = listed.entry("anim.gif");
    assert!(gif.binary);
    assert_eq!((gif.additions, gif.deletions), (0, 0));
    assert_eq!(
        listed.diff("anim.gif"),
        FileDiff::Binary {
            old_len: None,
            new_len: Some(bytes.len() as u64)
        }
    );
    let late = listed.entry("late-nul.txt");
    assert!(!late.binary);
    assert_eq!(late.additions, 4501);
}

#[cfg(unix)]
#[test]
fn untracked_symlinks_are_their_target_paths() {
    let repo = FixtureRepo::new();
    repo.write("target.txt", "a\nb\nc\n")
        .write("dir/inside.txt", "i\n");
    repo.commit("base");
    for (target, name) in [
        ("target.txt", "link"),
        ("/nonexistent/x", "dangling"),
        ("dir", "dirlink"),
    ] {
        std::os::unix::fs::symlink(target, repo.path().join(name)).unwrap();
    }
    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(
        listed.summary(),
        [
            ("dangling", FileStatus::Untracked, 1, 0),
            ("dirlink", FileStatus::Untracked, 1, 0),
            ("link", FileStatus::Untracked, 1, 0),
        ]
    );
    for (path, target) in [
        ("link", "target.txt"),
        ("dangling", "/nonexistent/x"),
        ("dirlink", "dir"),
    ] {
        assert_eq!(listed.entry(path).new_mode, Some(FileMode::Symlink));
        let text = listed.text(path);
        assert_eq!(render(&text), [format!("+:1 {target}")], "{path}");
        assert!(!text.new_eof_newline, "{path}");
    }
    // Exactly what git shows for a new link.
    // (`--no-index` exits 1 when the sides differ.)
    let output = repo
        .git_command()
        .args(["diff", "--no-index", "--", "/dev/null", "link"])
        .output()
        .unwrap();
    let git_view = String::from_utf8_lossy(&output.stdout);
    assert!(
        git_view.ends_with("+target.txt\n\\ No newline at end of file\n"),
        "{git_view}"
    );

    // Committed, a link diffs through git like any file.
    repo.git(&["add", "link"]);
    let staged = Listed::of(&repo, Target::Staged);
    assert_eq!(staged.entry("link").new_mode, Some(FileMode::Symlink));
    assert_eq!(render(&staged.text("link")), ["+:1 target.txt"]);
}

#[test]
fn paths_with_spaces_unicode_quotes_and_dashes() {
    // Characters every OS allows in a file name; a double quote and a tab follow below through
    // the index, because Windows file names can't hold them.
    let names = [
        "dir with spaces/file name.txt",
        "ünïcödé/日本語.txt",
        "it's 'quoted'.txt",
        "-leading-dash.txt",
        "--double-dash",
        "[ab] (1).txt",
    ];
    let repo = FixtureRepo::new();
    for name in names {
        repo.write(name, "old\n");
    }
    repo.write("plain.txt", "p\n");
    repo.commit("base");
    for name in names {
        repo.write(name, "new\n");
    }
    // The same names untracked.
    for name in names {
        repo.write(format!("untracked/{name}"), "u\n");
    }

    let listed = Listed::of(&repo, Target::Working);
    for name in names {
        assert_eq!(listed.entry(name).status, FileStatus::Modified, "{name}");
        assert_eq!(render(&listed.text(name)), ["-1: old", "+:1 new"], "{name}");
        let untracked = format!("untracked/{name}");
        assert_eq!(listed.entry(&untracked).status, FileStatus::Untracked);
        assert_eq!(render(&listed.text(&untracked)), ["+:1 u"], "{untracked}");
    }
    assert_eq!(listed.entries.len(), names.len() * 2);
}

#[test]
fn a_file_named_dash_is_read_from_disk() {
    // `git diff --no-index` reads stdin for a path of exactly `-`, even after `--`.
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    repo.write("-", "dash\nline2\n").write("sub/-", "nested\n");

    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(
        listed.summary(),
        [
            ("-", FileStatus::Untracked, 2, 0),
            ("sub/-", FileStatus::Untracked, 1, 0),
        ]
    );
    assert_eq!(render(&listed.text("-")), ["+:1 dash", "+:2 line2"]);
    assert_eq!(render(&listed.text("sub/-")), ["+:1 nested"]);

    // Read from disk, so git converts it as it does any untracked file.
    repo.config("core.autocrlf", "true");
    repo.write_crlf("-", "crlf\n");
    let listed = Listed::of(&repo, Target::Working);
    let text = listed.text("-");
    assert_eq!(render(&text), ["+:1 crlf"]);
    assert!(!text.crlf, "normalised like any untracked file");

    // Tracked, the path goes through a `:(literal)` pathspec.
    repo.git(&["add", "-"]);
    let staged = Listed::of(&repo, Target::Staged);
    assert_eq!(render(&staged.text("-")), ["+:1 crlf"]);
}

#[test]
fn names_git_quotes_in_patch_headers() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    // No backslash: git for Windows reads one in a pathspec as a directory separator.
    let names = [
        "q\"uote.txt",
        "tab\there.txt",
        "new\nline.txt",
        "ctrl\u{1}char.txt",
    ];
    for name in names {
        repo.stage("100644", &repo.hash_blob("old\n"), name);
    }
    repo.commit_staged("odd names");
    for name in names {
        repo.stage("100644", &repo.hash_blob("new\n"), name);
    }
    let changed = repo.commit_staged("edit odd names");

    let listed = Listed::of(&repo, Target::Commit { rev: changed });
    for name in names {
        assert_eq!(listed.entry(name).status, FileStatus::Modified, "{name:?}");
        assert_eq!(
            render(&listed.text(name)),
            ["-1: old", "+:1 new"],
            "{name:?}"
        );
    }
}

#[test]
fn empty_file_added() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    repo.write("empty.txt", "");
    let added = repo.commit("add empty");
    let listed = Listed::of(&repo, Target::Commit { rev: added });
    assert_eq!(listed.summary(), [("empty.txt", FileStatus::Added, 0, 0)]);
    assert_eq!(listed.text("empty.txt"), TextDiff::empty());
    // Untracked, too.
    repo.write("untracked-empty.txt", "");
    let working = Listed::of(&repo, Target::Working);
    assert_eq!(
        working.summary(),
        [("untracked-empty.txt", FileStatus::Untracked, 0, 0)]
    );
    assert_eq!(working.text("untracked-empty.txt"), TextDiff::empty());
}

#[test]
fn empty_file_removed() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n").write("empty.txt", "");
    repo.commit("base");
    repo.remove("empty.txt");
    let removed = repo.commit("remove empty");
    let listed = Listed::of(&repo, Target::Commit { rev: removed });
    assert_eq!(listed.summary(), [("empty.txt", FileStatus::Deleted, 0, 0)]);
    assert_eq!(listed.text("empty.txt"), TextDiff::empty());
}

#[test]
fn no_newline_at_eof_on_the_old_side() {
    let (_repo, listed) = one_commit("f.txt", "a\nb", "a\nb\n");
    let text = listed.text("f.txt");
    assert_eq!(render(&text), [" 1:1 a", "-2: b", "+:2 b"]);
    assert!(!text.old_eof_newline);
    assert!(text.new_eof_newline);
    // The pair's text is identical; only the newline differs, so no words are marked.
    assert!(text.lines.iter().all(|line| line.words.is_empty()));
}

#[test]
fn no_newline_at_eof_on_the_new_side() {
    let (_repo, listed) = one_commit("f.txt", "a\nb\n", "a\nc");
    let text = listed.text("f.txt");
    assert_eq!(render(&text), [" 1:1 a", "-2: b", "+:2 c"]);
    assert!(text.old_eof_newline);
    assert!(!text.new_eof_newline);
}

#[test]
fn no_newline_at_eof_on_both_sides() {
    let (_repo, listed) = one_commit("f.txt", "a\nz", "A\nz");
    let text = listed.text("f.txt");
    assert_eq!(render(&text), ["-1: a", "+:1 A", " 2:2 z"]);
    assert!(!text.old_eof_newline && !text.new_eof_newline);
    // Both sides changed and both lack the newline.
    let (_repo, listed) = one_commit("g.txt", "a\nb", "a\nc");
    let text = listed.text("g.txt");
    assert_eq!(render(&text), [" 1:1 a", "-2: b", "+:2 c"]);
    assert!(!text.old_eof_newline && !text.new_eof_newline);
}

#[test]
fn mode_change_only() {
    let repo = FixtureRepo::new();
    repo.write("run.sh", "#!/bin/sh\necho hi\n");
    repo.commit("base");
    // Through the index, so the test means the same where `core.filemode` is off (Windows).
    repo.git(&["update-index", "--chmod=+x", "run.sh"]);
    let staged = Listed::of(&repo, Target::Staged);
    assert_eq!(staged.summary(), [("run.sh", FileStatus::Modified, 0, 0)]);
    let entry = staged.entry("run.sh");
    assert_eq!(entry.old_mode, Some(FileMode::Regular));
    assert_eq!(entry.new_mode, Some(FileMode::Executable));
    assert_eq!(staged.text("run.sh"), TextDiff::empty(), "no hunks");

    let chmod = repo.commit_staged("chmod");
    let committed = Listed::of(&repo, Target::Commit { rev: chmod });
    assert_eq!(
        committed.entry("run.sh").new_mode,
        Some(FileMode::Executable)
    );
    assert_eq!(committed.text("run.sh"), TextDiff::empty());
}

#[test]
fn a_type_change_shows_the_old_content_removed_and_the_new_added() {
    let repo = FixtureRepo::new();
    repo.write("link", "one\ntwo\n");
    repo.commit("base");
    repo.stage("120000", &repo.hash_blob("target/path"), "link");
    let changed = repo.commit_staged("file to symlink");
    let listed = Listed::of(&repo, Target::Commit { rev: changed });
    assert_eq!(listed.summary(), [("link", FileStatus::TypeChanged, 1, 2)]);
    assert_eq!(listed.entry("link").new_mode, Some(FileMode::Symlink));
    let text = listed.text("link");
    assert_eq!(render(&text), ["-1: one", "-2: two", "+:1 target/path"]);
    assert!(text.old_eof_newline);
    assert!(!text.new_eof_newline, "a symlink's target has no newline");
}

#[test]
fn submodule() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    let first = "1111111111111111111111111111111111111111";
    let second = "2222222222222222222222222222222222222222";
    repo.stage("160000", first, "sub");
    let added = repo.commit_staged("add submodule");
    repo.stage("160000", second, "sub");
    let bumped = repo.commit_staged("bump submodule");

    let listed = Listed::of(&repo, Target::Commit { rev: added });
    assert_eq!(listed.entry("sub").status, FileStatus::Added);
    assert_eq!(
        listed.diff("sub"),
        FileDiff::Submodule {
            old_oid: None,
            new_oid: Oid::parse(first),
            dirty: false
        }
    );
    let listed = Listed::of(&repo, Target::Commit { rev: bumped });
    let entry = listed.entry("sub");
    assert_eq!(entry.status, FileStatus::Modified);
    assert!(entry.is_submodule());
    assert_eq!(
        listed.diff("sub"),
        FileDiff::Submodule {
            old_oid: Oid::parse(first),
            new_oid: Oid::parse(second),
            dirty: false
        }
    );
}

/// A repo whose `sub` is a submodule checkout (an embedded repository) committed at its commit
/// `s1` and since advanced to `s2`, uncommitted in the outer repo. Returns the repo, `s1` and
/// `s2`.
fn advanced_submodule_checkout() -> (FixtureRepo, String, String) {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n").write("sub/s.txt", "s\n");
    repo.git(&["-C", "sub", "init", "-q"]);
    repo.git(&["-C", "sub", "add", "-A"]);
    repo.git(&["-C", "sub", "commit", "-q", "-m", "s1"]);
    let sub_head = || {
        repo.git(&["-C", "sub", "rev-parse", "HEAD"])
            .trim()
            .to_string()
    };
    let s1 = sub_head();
    // `git add` records the nested repository as a gitlink.
    repo.commit("base with submodule");
    repo.write("sub/s.txt", "s\ns2\n");
    repo.git(&["-C", "sub", "commit", "-q", "-am", "s2"]);
    let s2 = sub_head();
    (repo, s1, s2)
}

#[test]
fn a_submodule_checkout_in_the_working_tree() {
    let (repo, s1, s2) = advanced_submodule_checkout();
    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(listed.entry("sub").status, FileStatus::Modified);
    assert_eq!(
        listed.diff("sub"),
        FileDiff::Submodule {
            old_oid: Oid::parse(&s1),
            new_oid: Oid::parse(&s2),
            dirty: false
        }
    );
    // Uncommitted changes inside the submodule make its checkout dirty.
    repo.write("sub/s.txt", "s\ns2\ns3\n");
    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(
        listed.diff("sub"),
        FileDiff::Submodule {
            old_oid: Oid::parse(&s1),
            new_oid: Oid::parse(&s2),
            dirty: true
        }
    );
}

#[test]
fn submodule_presentation_settings_are_ignored() {
    // `diff.submodule=log` or `diff` would replace the `Subproject commit` lines dv reads, and
    // `diff.ignoreSubmodules=all` or a `.gitmodules` `ignore = all` would hide the submodule
    // altogether (PLAN D4, §6 step 2).
    let (repo, s1, s2) = advanced_submodule_checkout();
    repo.write("sub/s.txt", "s\ns2\ns3\n");
    repo.config("diff.ignoreSubmodules", "all");
    for presentation in ["log", "diff", "short"] {
        repo.config("diff.submodule", presentation);
        let listed = Listed::of(&repo, Target::Working);
        assert_eq!(
            listed.summary(),
            [("sub", FileStatus::Modified, 1, 1)],
            "{presentation}"
        );
        assert_eq!(
            listed.diff("sub"),
            FileDiff::Submodule {
                old_oid: Oid::parse(&s1),
                new_oid: Oid::parse(&s2),
                dirty: true
            },
            "{presentation}"
        );
    }

    // A committed bump, as GitHub lists it, however the repo asks git to ignore the submodule.
    repo.git(&["config", "--unset", "diff.ignoreSubmodules"]);
    let bump = repo.commit("bump");
    repo.config("diff.ignoreSubmodules", "all").write(
        ".gitmodules",
        "[submodule \"sub\"]\n\tpath = sub\n\turl = ./sub\n\tignore = all\n",
    );
    let listed = Listed::of(&repo, Target::Commit { rev: bump });
    assert_eq!(listed.summary(), [("sub", FileStatus::Modified, 1, 1)]);
    assert_eq!(
        listed.diff("sub"),
        FileDiff::Submodule {
            old_oid: Oid::parse(&s1),
            new_oid: Oid::parse(&s2),
            dirty: false
        }
    );
}

#[test]
fn a_type_change_between_a_submodule_and_a_file_shows_the_file() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    let gitlink = "1111111111111111111111111111111111111111";
    repo.stage("160000", gitlink, "sub");
    repo.commit_staged("add submodule");
    repo.stage("100644", &repo.hash_blob("now a file\n"), "sub");
    let to_file = repo.commit_staged("submodule to file");
    repo.stage("160000", gitlink, "sub");
    let to_submodule = repo.commit_staged("file to submodule");
    let subproject = format!("Subproject commit {gitlink}");

    // git prints the gitlink's line removed and the file's lines added, as for any type change.
    let listed = Listed::of(&repo, Target::Commit { rev: to_file });
    assert_eq!(listed.summary(), [("sub", FileStatus::TypeChanged, 1, 1)]);
    assert_eq!(
        render(&listed.text("sub")),
        [format!("-1: {subproject}"), "+:1 now a file".to_string()]
    );
    let listed = Listed::of(&repo, Target::Commit { rev: to_submodule });
    assert_eq!(listed.summary(), [("sub", FileStatus::TypeChanged, 1, 1)]);
    assert_eq!(
        render(&listed.text("sub")),
        ["-1: now a file".to_string(), format!("+:1 {subproject}")]
    );
}

#[test]
fn a_working_tree_file_hashed_only_by_rename_detection_is_measured_on_disk() {
    // Rename detection hashes the working tree side of every added and deleted candidate
    // without writing the object, so `--raw` prints an id `cat-file` doesn't know; the size
    // check measures such a side on disk.
    let repo = FixtureRepo::new();
    let original = numbered(20);
    repo.write("a.txt", &original)
        .write("gone.txt", "unrelated\n");
    repo.commit("base");
    // A staged rename, edited after staging.
    repo.git(&["mv", "a.txt", "b.txt"]);
    repo.write(
        "b.txt",
        original.replace("line 10\n", "line ten, and longer\n"),
    );
    // A staged addition, edited after staging, beside a deletion (which sets rename detection
    // going).
    repo.write("new.txt", "short\n");
    repo.git(&["add", "new.txt"]);
    repo.write("new.txt", "short\nand a much longer second line\n");
    repo.git(&["rm", "-q", "gone.txt"]);

    let listed = Listed::of(&repo, Target::Working);
    for (path, old_len) in [("b.txt", Some(original.len() as u64)), ("new.txt", None)] {
        let oid = listed.entry(path).new_oid.clone();
        let oid = oid.unwrap_or_else(|| panic!("{path}: rename detection hashed it"));
        let in_store = repo
            .git_command()
            .args(["cat-file", "-e", oid.as_str()])
            .status()
            .unwrap();
        assert!(
            !in_store.success(),
            "{path}: {oid} isn't in the object store"
        );
        // A byte cap the new side exceeds by one byte, and the old side doesn't.
        let on_disk = std::fs::metadata(repo.path().join(path)).unwrap().len();
        let limits = DiffLimits {
            max_bytes: on_disk - 1,
            max_lines: MAX_FILE_LINES,
        };
        assert_eq!(
            listed.diff_with(path, &limits),
            FileDiff::TooLarge {
                old_len,
                new_len: Some(on_disk)
            },
            "{path}"
        );
    }
}

#[test]
fn too_large_cap() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    // Over the line cap (and small in bytes).
    let many_lines = "x\n".repeat(MAX_FILE_LINES + 1);
    // Over the byte cap in three lines.
    let long_line = "y".repeat(MAX_FILE_BYTES as usize / 2);
    let big_bytes = format!("{long_line}\n{long_line}\n{long_line}\n");
    repo.write("many-lines.txt", &many_lines)
        .write("big.txt", &big_bytes)
        .write("ok.txt", "x\n".repeat(MAX_FILE_LINES));
    let added = repo.commit("large files");
    let listed = Listed::of(&repo, Target::Commit { rev: added });

    // Totals are known up front all the same.
    assert_eq!(
        listed.entry("many-lines.txt").additions as usize,
        MAX_FILE_LINES + 1
    );
    assert_eq!(listed.entry("big.txt").additions, 3);
    assert_eq!(
        listed.diff("many-lines.txt"),
        FileDiff::TooLarge {
            old_len: None,
            new_len: Some(many_lines.len() as u64)
        }
    );
    assert_eq!(
        listed.diff("big.txt"),
        FileDiff::TooLarge {
            old_len: None,
            new_len: Some(big_bytes.len() as u64)
        }
    );
    // Exactly at the line cap is shown.
    assert_eq!(listed.text("ok.txt").new_line_count(), MAX_FILE_LINES);

    // "Load anyway" diffs without the caps.
    for path in ["many-lines.txt", "big.txt"] {
        let FileDiff::Text(text) = listed.diff_with(path, &DiffLimits::UNLIMITED) else {
            panic!("{path} should load without limits");
        };
        assert!(text.lines.iter().all(|line| line.kind == LineKind::Added));
    }
    assert_eq!(
        listed
            .diff_with("big.txt", &DiffLimits::UNLIMITED)
            .as_text()
            .unwrap()
            .lines[0]
            .text
            .len(),
        long_line.len()
    );

    // An untracked file is capped by its size on disk.
    repo.write("untracked-big.txt", &big_bytes);
    let working = Listed::of(&repo, Target::Working);
    assert_eq!(
        working.diff("untracked-big.txt"),
        FileDiff::TooLarge {
            old_len: None,
            new_len: Some(big_bytes.len() as u64)
        }
    );
}

#[test]
fn too_large_cap_counts_unchanged_lines() {
    // One edited line in a long file: the totals are small, the sides are not.
    let repo = FixtureRepo::new();
    let just_over = numbered(MAX_FILE_LINES + 1);
    // Long enough that the output alone shows a side over the cap, before any parsing.
    let far_over = numbered(MAX_FILE_LINES * 2 + 100);
    repo.write("just-over.txt", &just_over)
        .write("far-over.txt", &far_over)
        .write("at-cap.txt", numbered(MAX_FILE_LINES));
    repo.commit("base");
    let edit = |text: &str| text.replacen("line 7\n", "line seven\n", 1);
    repo.write("just-over.txt", edit(&just_over))
        .write("far-over.txt", edit(&far_over))
        .write("at-cap.txt", edit(&numbered(MAX_FILE_LINES)));
    let listed = Listed::of(&repo, Target::Working);
    for path in ["just-over.txt", "far-over.txt"] {
        assert_eq!(
            (listed.entry(path).additions, listed.entry(path).deletions),
            (1, 1)
        );
        assert!(
            matches!(listed.diff(path), FileDiff::TooLarge { .. }),
            "{path}"
        );
        let unlimited = listed.diff_with(path, &DiffLimits::UNLIMITED);
        assert!(unlimited.as_text().is_some(), "{path}");
    }
    let text = listed.text("at-cap.txt");
    assert_eq!(text.old_line_count(), MAX_FILE_LINES);
    assert_eq!(text.new_line_count(), MAX_FILE_LINES);
}

#[test]
fn user_config_diff_algorithm_histogram_is_ignored() {
    // An input where Myers and histogram disagree (found by search): Myers adds one line and
    // removes two in separate places, histogram replaces a block.
    let before = "{\ny\ny\ny\nreturn;\n{\n";
    let after = "{\ny\n{\ny\n{\n";
    let repo = FixtureRepo::new();
    repo.config("diff.algorithm", "histogram");
    repo.write("f.c", before);
    repo.commit("before");
    repo.write("f.c", after);
    let changed = repo.commit("after");
    let myers = repo.git(&["diff", "--diff-algorithm=myers", "-U0", "HEAD~1", "HEAD"]);
    let histogram = repo.git(&["diff", "-U0", "HEAD~1", "HEAD"]);
    assert_ne!(myers, histogram, "the input must tell the algorithms apart");

    let listed = Listed::of(&repo, Target::Commit { rev: changed });
    assert_eq!(listed.summary(), [("f.c", FileStatus::Modified, 1, 2)]);
    let text = listed.text("f.c");
    // Exactly git's Myers output, line for line.
    let changes: Vec<_> = render(&text)
        .into_iter()
        .filter(|line| !line.starts_with(' '))
        .collect();
    let myers_changes: Vec<String> = myers
        .lines()
        .skip_while(|line| !line.starts_with("@@"))
        .filter(|line| line.starts_with(['+', '-']))
        .map(str::to_string)
        .collect();
    assert_eq!(myers_changes, ["+{", "-y", "-return;"]);
    assert_eq!(changes, ["+:3 {", "-4: y", "-5: return;"]);
}

#[test]
fn user_config_diff_indent_heuristic_false_is_ignored() {
    // A block that slides: the inserted `b`, `  x`, blank could be lines 3-5 or 7-9 of the new
    // side. git's indent heuristic (its default, and what GitHub shows) picks 3-5; with
    // `diff.indentHeuristic=false` git picks 7-9 (PLAN D4).
    let before = "a\n\nb\n  x\n\nb\n  y\n";
    let after = "a\n\nb\n  x\n\nb\n  x\n\nb\n  y\n";
    let repo = FixtureRepo::new();
    repo.config("diff.indentHeuristic", "false");
    repo.write("f.txt", before);
    repo.commit("before");
    repo.write("f.txt", after);
    let changed = repo.commit("after");
    let with_heuristic = repo.git(&["-c", "diff.indentHeuristic=true", "diff", "HEAD~1", "HEAD"]);
    let without = repo.git(&["diff", "HEAD~1", "HEAD"]);
    assert_ne!(with_heuristic, without, "the input must slide");

    let added = |listed: &Listed| -> Vec<String> {
        render(&listed.text("f.txt"))
            .into_iter()
            .filter(|line| line.starts_with('+'))
            .collect()
    };
    let expected = ["+:3 b", "+:4   x", "+:5 "];
    let committed = Listed::of(&repo, Target::Commit { rev: changed });
    assert_eq!(committed.summary(), [("f.txt", FileStatus::Modified, 3, 0)]);
    assert_eq!(added(&committed), expected);
    // The same edit uncommitted, and untracked (`git diff --no-index`).
    repo.git(&["reset", "-q", "--soft", "HEAD~1"]);
    repo.git(&["reset", "-q"]);
    assert_eq!(added(&Listed::of(&repo, Target::Working)), expected);
    repo.write("u.txt", after);
    let untracked = Listed::of(&repo, Target::Working);
    let text = untracked.text("u.txt");
    assert_eq!(text.new_line_count(), 10);
    assert!(text.lines.iter().all(|line| line.kind == LineKind::Added));
}

#[test]
fn unmerged_files() {
    let repo = FixtureRepo::new();
    repo.write("c.txt", "base\n");
    repo.commit("base");
    repo.branch("side")
        .checkout("side")
        .write("c.txt", "side\n");
    repo.commit("side");
    repo.checkout("main").write("c.txt", "main\n");
    repo.commit("main");
    let merge = repo
        .git_command()
        .args(["merge", "-q", "side"])
        .output()
        .unwrap();
    assert!(!merge.status.success(), "the merge conflicts");

    // The index holds no single version.
    let staged = Listed::of(&repo, Target::Staged);
    assert_eq!(staged.summary(), [("c.txt", FileStatus::Unmerged, 0, 0)]);
    assert_eq!(staged.diff("c.txt"), FileDiff::Unmerged);
    // The working tree holds the conflict markers, a plain text change against HEAD.
    let working = Listed::of(&repo, Target::Working);
    assert_eq!(working.entry("c.txt").status, FileStatus::Modified);
    let added: Vec<_> = working
        .text("c.txt")
        .lines
        .iter()
        .filter(|line| line.kind == LineKind::Added)
        .map(|line| line.text.clone())
        .collect();
    assert_eq!(added[0], "<<<<<<< HEAD");
}

#[test]
fn a_file_that_stopped_differing_is_an_empty_diff() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    repo.write("a.txt", "b\n").write("u.txt", "u\n");
    let listed = Listed::of(&repo, Target::Working);
    repo.write("a.txt", "a\n").remove("u.txt");
    assert_eq!(listed.text("a.txt"), TextDiff::empty());
    assert_eq!(listed.text("u.txt"), TextDiff::empty());
}

#[test]
fn sha256_repositories_diff_the_same_way() {
    let repo = FixtureRepo::new_sha256();
    repo.write("a.txt", "one\n");
    repo.commit("root");
    repo.write("a.txt", "two\n").write("u.txt", "u\n");
    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(
        listed.summary(),
        [
            ("a.txt", FileStatus::Modified, 1, 1),
            ("u.txt", FileStatus::Untracked, 1, 0),
        ]
    );
    assert_eq!(
        listed
            .entry("a.txt")
            .old_oid
            .as_ref()
            .unwrap()
            .as_str()
            .len(),
        64
    );
    assert_eq!(render(&listed.text("a.txt")), ["-1: one", "+:1 two"]);
    // The root commit diffs against the SHA-256 empty tree.
    let root = Listed::of(
        &repo,
        Target::Commit {
            rev: "HEAD".to_string(),
        },
    );
    assert_eq!(render(&root.text("a.txt")), ["+:1 one"]);
}
