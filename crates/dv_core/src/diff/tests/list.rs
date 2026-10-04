//! File list fixture tests (PLAN §6 step 1).

use std::time::Duration;

use super::*;

#[test]
fn every_file_has_its_totals_up_front() {
    let repo = FixtureRepo::new();
    repo.write("keep.txt", numbered(5))
        .write("edit.txt", numbered(5))
        .write("gone.txt", "a\nb\n");
    repo.commit("base");
    repo.write(
        "edit.txt",
        "line 1\nline two\nline 3\nline 4\nline 5\nline 6\n",
    )
    .remove("gone.txt")
    .write("new-staged.txt", "x\ny\nz\n")
    .write("untracked.txt", "u1\nu2\nno newline");
    repo.git(&["add", "new-staged.txt"]);

    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(
        listed.summary(),
        [
            ("edit.txt", FileStatus::Modified, 2, 1),
            ("gone.txt", FileStatus::Deleted, 0, 2),
            ("new-staged.txt", FileStatus::Added, 3, 0),
            ("untracked.txt", FileStatus::Untracked, 3, 0),
        ]
    );
    let gone = listed.entry("gone.txt");
    assert_eq!(gone.new_mode, None);
    assert_eq!(gone.old_mode, Some(FileMode::Regular));
    assert!(gone.old_oid.is_some() && gone.new_oid.is_none());
    let untracked = listed.entry("untracked.txt");
    assert_eq!((untracked.old_mode, untracked.new_mode), (None, None));
    assert!(!untracked.binary);
}

#[test]
fn untracked_files_honour_the_ignore_rules() {
    let repo = FixtureRepo::new();
    repo.write(".gitignore", "target/\n*.log\n");
    repo.commit("base");
    repo.write("target/debug/out.bin", "x")
        .write("app.log", "log\n")
        .write("src/new.rs", "fn main() {}\n")
        .write("deep/er/still.txt", "s\n");
    std::fs::create_dir_all(repo.path().join("empty-dir")).unwrap();

    let listed = Listed::of(&repo, Target::Working);
    let paths: Vec<_> = listed.entries.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(paths, ["deep/er/still.txt", "src/new.rs"]);
    // Untracked files belong to the working tree only.
    assert!(Listed::of(&repo, Target::Staged).entries.is_empty());
}

#[test]
fn a_stat_dirty_file_is_not_listed() {
    let repo = FixtureRepo::new();
    repo.write("same.txt", "same\n").write("edited.txt", "a\n");
    repo.commit("base");
    // A new mtime with the same content: git's index stat data no longer matches, and the git
    // policy keeps git from refreshing it (PLAN §5.1), so `--raw` reports the file as modified.
    let file = std::fs::File::options()
        .write(true)
        .open(repo.path().join("same.txt"))
        .unwrap();
    file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();
    drop(file);
    repo.write("edited.txt", "b\n");

    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(
        listed.summary(),
        [("edited.txt", FileStatus::Modified, 1, 1)]
    );
    // The raw listing does show it, so the join is what drops it.
    let raw = repo.git(&["-c", "diff.autoRefreshIndex=false", "diff", "HEAD", "--raw"]);
    assert!(raw.contains("same.txt"), "{raw}");
}

#[test]
fn copy_detection_is_off_even_when_configured() {
    // dv lists renames but not copies (`-M` only), like GitHub; `-M` also overrides a user's
    // `diff.renames=copies`, which would otherwise report the copy.
    let repo = FixtureRepo::new();
    repo.config("diff.renames", "copies");
    repo.write("src.txt", numbered(30));
    repo.commit("base");
    repo.write("copy.txt", numbered(30))
        .write("src.txt", format!("{}extra\n", numbered(30)));
    let copied = repo.commit("copy");
    // git itself reports the copy under this configuration.
    let plain = repo.git(&["diff", "--name-status", "HEAD~1", "HEAD"]);
    assert!(plain.contains("C100\tsrc.txt\tcopy.txt"), "{plain}");

    let listed = Listed::of(&repo, Target::Commit { rev: copied });
    assert_eq!(
        listed.summary(),
        [
            ("copy.txt", FileStatus::Added, 30, 0),
            ("src.txt", FileStatus::Modified, 1, 0),
        ]
    );
    assert_eq!(listed.entry("copy.txt").old_path, None);
}

#[test]
fn totals_are_myers_whatever_a_diff_driver_prefers() {
    // A driver's `diff.<driver>.algorithm` (git 2.41+) beats `-c diff.algorithm=myers` but not
    // `--diff-algorithm=myers`; the list's totals must match the hunks, which are Myers (PLAN
    // D4). An input where Myers and histogram disagree (as in the per-file test).
    let repo = FixtureRepo::new();
    repo.set_attributes("*.c diff=weird\n");
    repo.config("diff.weird.algorithm", "histogram");
    repo.write("f.c", "{\ny\ny\ny\nreturn;\n{\n");
    repo.commit("before");
    repo.write("f.c", "{\ny\n{\ny\n{\n");
    let changed = repo.commit("after");
    let listed = Listed::of(&repo, Target::Commit { rev: changed });
    if listed.git.info().git_version.at_least(2, 41) {
        // git itself follows the driver.
        assert_eq!(
            repo.git(&["diff", "--numstat", "HEAD~1", "HEAD"]),
            "2\t3\tf.c\n"
        );
    }
    assert_eq!(listed.summary(), [("f.c", FileStatus::Modified, 1, 2)]);
    let text = listed.text("f.c");
    let count = |kind: LineKind| text.lines.iter().filter(|line| line.kind == kind).count();
    assert_eq!((count(LineKind::Added), count(LineKind::Removed)), (1, 2));
}

#[test]
fn an_untracked_nested_repository_is_listed_as_a_submodule() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    repo.write("nested/n.txt", "n\n");
    repo.git(&["-C", "nested", "init", "-q"]);
    repo.git(&["-C", "nested", "add", "-A"]);
    repo.git(&["-C", "nested", "commit", "-q", "-m", "nested"]);

    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(listed.summary(), [("nested", FileStatus::Untracked, 0, 0)]);
    let nested = listed.entry("nested");
    assert!(nested.is_submodule());
    assert_eq!(
        listed.diff("nested"),
        FileDiff::Submodule {
            old_oid: None,
            new_oid: None,
            dirty: false
        }
    );
}

#[test]
fn a_clean_tree_lists_nothing() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    for target in [Target::Working, Target::Staged] {
        assert!(Listed::of(&repo, target).entries.is_empty());
    }
}

#[test]
fn the_list_never_writes_the_index() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n").write("b.txt", "b\n");
    repo.commit("base");
    let file = std::fs::File::options()
        .write(true)
        .open(repo.path().join("a.txt"))
        .unwrap();
    file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();
    drop(file);
    repo.write("b.txt", "b2\n").write("c.txt", "c\n");
    let index = repo.path().join(".git").join("index");
    let before = std::fs::read(&index).unwrap();

    let listed = Listed::of(&repo, Target::Working);
    for entry in &listed.entries {
        listed.diff(&entry.path);
    }
    assert_eq!(std::fs::read(&index).unwrap(), before, "PLAN §2.4");
}
