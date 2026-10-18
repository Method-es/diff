//! Line endings and attributes (PLAN D4): git normalises and classifies files exactly as it
//! does for `git diff`, so a CRLF checkout of an LF-normalised file edits as one line, not the
//! whole file.

use super::*;

/// The changed lines of a text diff, rendered.
fn changes(text: &TextDiff) -> Vec<String> {
    render(text)
        .into_iter()
        .filter(|line| !line.starts_with(' '))
        .collect()
}

#[test]
fn crlf_with_core_autocrlf_true_edits_one_line() {
    let repo = FixtureRepo::new();
    repo.config("core.autocrlf", "true");
    repo.write_crlf("win.txt", &numbered(10));
    repo.commit("base");
    // The index holds LF; the checkout is CRLF.
    assert!(!repo.git(&["show", "HEAD:win.txt"]).contains('\r'));
    repo.write_crlf("win.txt", &numbered(10).replace("line 5\n", "line five\n"));

    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(listed.summary(), [("win.txt", FileStatus::Modified, 1, 1)]);
    let text = listed.text("win.txt");
    assert_eq!(text.lines.len(), 11);
    assert_eq!(changes(&text), ["-5: line 5", "+:5 line five"]);
    assert!(text.lines.iter().all(|line| !line.text.contains('\r')));
    // git compares the normalised LF form, so no CR reaches the diff.
    assert!(!text.crlf);

    // The same edit committed, as a commit target.
    let edited = repo.commit("edit");
    let committed = Listed::of(&repo, Target::Commit { rev: edited });
    assert_eq!(
        committed.summary(),
        [("win.txt", FileStatus::Modified, 1, 1)]
    );
    assert_eq!(
        changes(&committed.text("win.txt")),
        ["-5: line 5", "+:5 line five"]
    );
    // An untracked CRLF file is normalised too.
    repo.write_crlf("new.txt", "a\nb\n");
    let working = Listed::of(&repo, Target::Working);
    assert_eq!(
        working.summary(),
        [("new.txt", FileStatus::Untracked, 2, 0)]
    );
    let text = working.text("new.txt");
    assert_eq!(render(&text), ["+:1 a", "+:2 b"]);
    // The parser strips a CR either way; only `crlf` tells whether git normalised the file.
    assert!(
        !text.crlf,
        "git normalises the untracked file before diffing"
    );
}

#[test]
fn gitattributes_eol_crlf_edits_one_line() {
    let repo = FixtureRepo::new();
    repo.set_attributes("* text eol=crlf\n");
    repo.write_crlf("doc.txt", &numbered(8));
    repo.commit("base");
    assert!(!repo.git(&["show", "HEAD:doc.txt"]).contains('\r'));
    repo.write_crlf("doc.txt", &numbered(8).replace("line 3\n", "line three\n"));

    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(listed.summary(), [("doc.txt", FileStatus::Modified, 1, 1)]);
    let text = listed.text("doc.txt");
    assert_eq!(text.lines.len(), 9);
    assert_eq!(changes(&text), ["-3: line 3", "+:3 line three"]);
    assert!(!text.crlf);
    // Staged, the index side is normalised as well.
    repo.git(&["add", "doc.txt"]);
    let staged = Listed::of(&repo, Target::Staged);
    assert_eq!(staged.summary(), [("doc.txt", FileStatus::Modified, 1, 1)]);
    assert_eq!(
        changes(&staged.text("doc.txt")),
        ["-3: line 3", "+:3 line three"]
    );
    // An untracked CRLF file is normalised too.
    repo.write_crlf("new.txt", "a\nb\n");
    let working = Listed::of(&repo, Target::Working);
    let entry = working.entry("new.txt");
    assert_eq!((entry.status, entry.additions), (FileStatus::Untracked, 2));
    let text = working.text("new.txt");
    assert_eq!(render(&text), ["+:1 a", "+:2 b"]);
    assert!(
        !text.crlf,
        "git normalises the untracked file before diffing"
    );
}

#[test]
fn committed_crlf_content_strips_the_cr_and_reports_it() {
    // Without normalisation the blobs themselves hold CRLF, and git's diff lines end in CR.
    let repo = FixtureRepo::new();
    repo.write_crlf("raw.txt", "a\nb\nc\n");
    repo.commit("base");
    assert!(repo.git(&["show", "HEAD:raw.txt"]).contains("\r\n"));
    repo.write_crlf("raw.txt", "a\nB\nc\n");
    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(listed.summary(), [("raw.txt", FileStatus::Modified, 1, 1)]);
    let text = listed.text("raw.txt");
    assert!(text.crlf);
    assert_eq!(render(&text), [" 1:1 a", "-2: b", "+:2 B", " 3:3 c"]);
    // A line-ending-only change still shows as a change.
    repo.write("raw.txt", "a\r\nb\nc\r\n");
    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(listed.summary(), [("raw.txt", FileStatus::Modified, 1, 1)]);
    assert_eq!(changes(&listed.text("raw.txt")), ["-2: b", "+:2 b"]);
}

#[test]
fn diff_attribute_unset_is_binary() {
    let repo = FixtureRepo::new();
    repo.set_attributes("*.lock -diff\n");
    repo.write("Cargo.lock", "a = 1\n")
        .write("plain.txt", "p\n");
    repo.commit("base");
    repo.write("Cargo.lock", "a = 2\n")
        .write("plain.txt", "q\n");
    // Untracked files follow the attribute too.
    repo.write("new.lock", "text\n");

    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(
        listed.summary(),
        [
            ("Cargo.lock", FileStatus::Modified, 0, 0),
            ("new.lock", FileStatus::Untracked, 0, 0),
            ("plain.txt", FileStatus::Modified, 1, 1),
        ]
    );
    for path in ["Cargo.lock", "new.lock"] {
        assert!(listed.entry(path).binary, "{path}");
    }
    assert_eq!(
        listed.diff("Cargo.lock"),
        FileDiff::Binary {
            old_len: Some(6),
            new_len: Some(6)
        }
    );
    assert_eq!(
        listed.diff("new.lock"),
        FileDiff::Binary {
            old_len: None,
            new_len: Some(5)
        }
    );
    assert!(!listed.entry("plain.txt").binary);
}

#[test]
fn binary_attribute_is_binary() {
    // `binary` is a macro for `-diff -merge -text`. The file is untracked, so dv counts it
    // itself: its own `check-attr` call must see the macro expand to an unset `diff`.
    let repo = FixtureRepo::new();
    repo.set_attributes("*.dat binary\n");
    repo.commit("base");
    repo.write("data.dat", "looks like text\n");
    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(
        listed.summary(),
        [("data.dat", FileStatus::Untracked, 0, 0)]
    );
    assert!(listed.entry("data.dat").binary);
    assert!(matches!(listed.diff("data.dat"), FileDiff::Binary { .. }));
}

#[test]
fn diff_attribute_set_forces_text() {
    let repo = FixtureRepo::new();
    repo.set_attributes("*.bin diff\n");
    repo.write("a.txt", "a\n");
    repo.commit("base");
    repo.write("zero.bin", b"one\n\0two\nthree\n".as_slice());
    let listed = Listed::of(&repo, Target::Working);
    let entry = listed.entry("zero.bin");
    assert!(!entry.binary);
    assert_eq!(entry.additions, 3);
    let text = listed.text("zero.bin");
    assert_eq!(text.new_line_count(), 3);
    assert_eq!(text.lines[1].text, "\0two");
}

#[test]
fn a_drivers_binary_setting_decides_for_untracked_files_as_in_git() {
    // dv counts untracked files itself, so it consults `diff.<driver>.binary` as git does:
    // true is binary, false is text, unset leaves it to the content.
    let repo = FixtureRepo::new();
    repo.set_attributes("*.foo diff=foo\n*.bar diff=bar\n*.baz diff=baz\n");
    repo.config("diff.foo.binary", "true")
        .config("diff.bar.binary", "false");
    repo.commit("base");
    repo.write("u.foo", "text\nmore\n")
        .write("u.bar", b"a\0b\nc\n".as_slice())
        .write("u.baz", "no binary setting\n");

    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(
        listed.summary(),
        [
            ("u.bar", FileStatus::Untracked, 2, 0),
            ("u.baz", FileStatus::Untracked, 1, 0),
            ("u.foo", FileStatus::Untracked, 0, 0),
        ]
    );
    assert!(listed.entry("u.foo").binary);
    assert_eq!(
        listed.diff("u.foo"),
        FileDiff::Binary {
            old_len: None,
            new_len: Some(10)
        }
    );
    assert!(!listed.entry("u.bar").binary);
    assert_eq!(listed.text("u.bar").lines[0].text, "a\0b");
    assert!(!listed.entry("u.baz").binary);
    // git's own counts for the same files.
    for (path, numstat) in [
        ("u.foo", "-\t-\t"),
        ("u.bar", "2\t0\t"),
        ("u.baz", "1\t0\t"),
    ] {
        let output = repo
            .git_command()
            .args(["diff", "--no-index", "--numstat", "--", "/dev/null", path])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.starts_with(numstat), "{path}: {stdout}");
    }
}

#[test]
fn an_untracked_file_with_a_working_tree_encoding_is_text_as_in_git() {
    // git converts a `working-tree-encoding` file to UTF-8 before it looks for a NUL, so a
    // UTF-16 file is text with its lines counted after conversion; dv asks git for such a
    // file's counts instead of sniffing the bytes on disk.
    let repo = FixtureRepo::new();
    repo.set_attributes("*.u16 text working-tree-encoding=UTF-16LE-BOM\n");
    repo.commit("base");
    let utf16 = |text: &str| -> Vec<u8> {
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        bytes
    };
    repo.write("n.u16", utf16("hi\nt\n"));
    // A plain UTF-16 file without the attribute stays binary, as in git.
    repo.write("plain.txt", utf16("hi\n"));

    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(
        listed.summary(),
        [
            ("n.u16", FileStatus::Untracked, 2, 0),
            ("plain.txt", FileStatus::Untracked, 0, 0),
        ]
    );
    assert!(!listed.entry("n.u16").binary);
    assert!(listed.entry("plain.txt").binary);
    assert_eq!(render(&listed.text("n.u16")), ["+:1 hi", "+:2 t"]);
    assert!(matches!(listed.diff("plain.txt"), FileDiff::Binary { .. }));

    // Staged, git's own numstat says the same.
    repo.git(&["add", "n.u16"]);
    let staged = Listed::of(&repo, Target::Staged);
    assert_eq!(staged.summary(), [("n.u16", FileStatus::Added, 2, 0)]);
    assert_eq!(render(&staged.text("n.u16")), ["+:1 hi", "+:2 t"]);
}

#[test]
fn an_untracked_file_with_a_clean_filter_is_counted_after_filtering_as_in_git() {
    // git runs a path's `filter` clean command before it sniffs or counts (a git-lfs asset is a
    // three-line text pointer to git, whatever bytes it holds), and the same file staged counts
    // as its filtered content; dv asks git for such a file's counts instead of reading the bytes
    // on disk, so the untracked file, its diff and the staged file agree (PLAN §6 step 1).
    let repo = FixtureRepo::new();
    repo.set_attributes("*.dat filter=ptr\n*.sec filter=enc\n*.raw filter=missing\n");
    // Each clean command reads its input, as a real filter does, then prints a fixed result:
    // a pointer, or bytes with a NUL (an encrypting filter). `missing` has no command, so git
    // applies no filter to `*.raw`.
    repo.config(
        "filter.ptr.clean",
        r"cat >/dev/null; printf 'version 1\noid sha256:abc\nsize 7\n'",
    )
    .config("filter.enc.clean", r"cat >/dev/null; printf 'x\000y\n'");
    repo.commit("base");
    // Binary on disk, a text pointer to git.
    repo.write("asset.dat", b"raw\0bin\n".as_slice())
        // 100 lines on disk, 3 to git.
        .write("notes.dat", numbered(100))
        // Text on disk, binary to git.
        .write("secret.sec", "secret\n")
        // Binary on disk and to git.
        .write("plain.raw", b"raw\0bin\n".as_slice());

    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(
        listed.summary(),
        [
            ("asset.dat", FileStatus::Untracked, 3, 0),
            ("notes.dat", FileStatus::Untracked, 3, 0),
            ("plain.raw", FileStatus::Untracked, 0, 0),
            ("secret.sec", FileStatus::Untracked, 0, 0),
        ]
    );
    for (path, binary) in [
        ("asset.dat", false),
        ("notes.dat", false),
        ("plain.raw", true),
        ("secret.sec", true),
    ] {
        assert_eq!(listed.entry(path).binary, binary, "{path}");
    }
    let pointer = ["+:1 version 1", "+:2 oid sha256:abc", "+:3 size 7"];
    assert_eq!(render(&listed.text("asset.dat")), pointer);
    assert_eq!(render(&listed.text("notes.dat")), pointer);
    for path in ["plain.raw", "secret.sec"] {
        assert!(
            matches!(listed.diff(path), FileDiff::Binary { .. }),
            "{path}"
        );
    }
    // git's own counts for the same files.
    for (path, numstat) in [
        ("asset.dat", "3\t0\t"),
        ("notes.dat", "3\t0\t"),
        ("plain.raw", "-\t-\t"),
        ("secret.sec", "-\t-\t"),
    ] {
        let output = repo
            .git_command()
            .args(["diff", "--no-index", "--numstat", "--", "/dev/null", path])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.starts_with(numstat), "{path}: {stdout}");
    }

    // Staged, git's own numstat says the same.
    repo.git(&["add", "asset.dat", "notes.dat", "secret.sec"]);
    let staged = Listed::of(&repo, Target::Staged);
    assert_eq!(
        staged.summary(),
        [
            ("asset.dat", FileStatus::Added, 3, 0),
            ("notes.dat", FileStatus::Added, 3, 0),
            ("secret.sec", FileStatus::Added, 0, 0),
        ]
    );
    assert!(!staged.entry("asset.dat").binary);
    assert!(staged.entry("secret.sec").binary);
    assert_eq!(render(&staged.text("asset.dat")), pointer);
}

#[test]
fn filter_drivers_are_the_ones_with_a_command_as_in_git() {
    // git applies a filter only when its driver has a `clean` or `process` command that isn't
    // empty, the last setting of each winning (a repo's empty `clean` turns off the user's);
    // a `filter` attribute naming any other driver converts nothing (`smudge` alone only acts
    // on checkout, and `required` alone makes git fail instead).
    let repo = FixtureRepo::new();
    std::fs::write(
        repo.home().join(".gitconfig"),
        "[filter \"emptied\"]\n\tclean = global-clean\n\
         [filter \"lfs\"]\n\tprocess = git-lfs filter-process\n\trequired = true\n",
    )
    .unwrap();
    repo.config("filter.emptied.clean", "")
        .config("filter.plain.clean", "cat")
        .config("filter.Dotted.Name.clean", "cat")
        .config("filter.smudge-only.smudge", "cat")
        .config("filter.required-only.required", "true");
    let mut drivers: Vec<String> = crate::diff::list::filter_drivers(&git_repo(&repo))
        .unwrap()
        .into_iter()
        .collect();
    drivers.sort();
    assert_eq!(drivers, ["Dotted.Name", "lfs", "plain"]);
    // No driver configured at all: git's exit code 1, not an error.
    assert!(
        crate::diff::list::filter_drivers(&git_repo(&FixtureRepo::new()))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn textconv_drivers_are_not_applied() {
    // Comment anchors and GitHub line numbers refer to the real lines (PLAN D4).
    let repo = FixtureRepo::new();
    repo.set_attributes("*.txt diff=upper\n");
    repo.config("diff.upper.textconv", "tr a-z A-Z <");
    repo.write("f.txt", "one\ntwo\n");
    repo.commit("base");
    repo.write("f.txt", "one\n2\n");
    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(listed.summary(), [("f.txt", FileStatus::Modified, 1, 1)]);
    assert_eq!(changes(&listed.text("f.txt")), ["-2: two", "+:2 2"]);
}
