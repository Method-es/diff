//! Interdiffs (PLAN §2.3, §6 step 6): a round's snapshot against the target's new side now, so
//! only what changed since the round shows — untracked files the snapshot holds included.

use std::path::Path;

use super::*;
use crate::git::Oid;
use crate::review::{ReviewId, Round, snapshot};
use crate::target::{Side, TargetError};

/// Snapshots `target`'s new side as round `n` and returns the closed round.
fn close_round(repo: &FixtureRepo, target: &Target, n: u32) -> Round {
    let git = git_repo(repo);
    let spec = target.resolve(&git).unwrap();
    let old_oid = match &spec.old {
        Side::Commit(oid) => oid.clone(),
        _ => git.empty_tree(),
    };
    let new_oid = snapshot::take(&git, &ReviewId::generate(), n, &spec, None).unwrap();
    Round {
        n,
        old_oid,
        new_oid,
        verdict: None,
        body: None,
        closed_at: 0,
        closed_from: None,
    }
}

/// `target`'s interdiff since `round`, listed.
fn since(repo: &FixtureRepo, target: Target, round: &Round) -> Listed {
    let git = git_repo(repo);
    let spec = target.resolve(&git).unwrap().since(round);
    let entries = file_list(&git, &spec).unwrap_or_else(|err| panic!("{err:?}"));
    Listed { git, spec, entries }
}

/// Every file under `.git/objects`, to tell whether something wrote one.
fn objects(repo: &FixtureRepo) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path.display().to_string());
            }
        }
    }
    let mut out = Vec::new();
    walk(&repo.path().join(".git").join("objects"), &mut out);
    out.sort();
    out
}

#[test]
fn since_compares_the_rounds_snapshot_with_the_new_side() {
    let round = Round {
        n: 2,
        old_oid: Oid::parse(&"a".repeat(40)).unwrap(),
        new_oid: Oid::parse(&"b".repeat(40)).unwrap(),
        verdict: None,
        body: None,
        closed_at: 0,
        closed_from: None,
    };
    for new in [
        Side::Worktree,
        Side::Index,
        Side::Commit(round.old_oid.clone()),
    ] {
        let spec = DiffSpec {
            old: Side::EmptyTree,
            new: new.clone(),
        };
        assert_eq!(
            spec.since(&round),
            DiffSpec {
                old: Side::Commit(round.new_oid.clone()),
                new
            }
        );
    }
}

#[test]
fn a_working_interdiff_shows_only_what_changed_since_the_round() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a1\n").write("b.txt", "b1\n");
    repo.commit("base");
    // Round 1: an edit and untracked files, which the snapshot holds.
    let similar = numbered(20);
    repo.write("a.txt", "a2\n")
        .write("u.txt", "u1\n")
        .write("u2.txt", "u2\n")
        .write("u3.txt", &similar)
        .write("u4.txt", "u4\n");
    let round = close_round(&repo, &Target::Working, 1);
    // Since: another edit, an untracked file changed, one left as it was, a new one, one
    // removed, and a near copy of an untracked file staged (git would take it for a rename of
    // the untracked one, which it sees deleted).
    repo.write("a.txt", "a3\n")
        .write("u2.txt", "u2 changed\n")
        .write("n.txt", "new\n")
        .remove("u4.txt")
        .write("u3copy.txt", format!("{similar}line 21\n"));
    repo.git(&["add", "u3copy.txt"]);
    let index = std::fs::read(repo.path().join(".git/index")).unwrap();
    let before = objects(&repo);

    let listed = since(&repo, Target::Working, &round);
    assert_eq!(
        listed.summary(),
        [
            ("a.txt", FileStatus::Modified, 1, 1),
            ("n.txt", FileStatus::Untracked, 1, 0),
            ("u2.txt", FileStatus::Modified, 1, 1),
            ("u3copy.txt", FileStatus::Added, 21, 0),
            ("u4.txt", FileStatus::Deleted, 0, 1),
        ]
    );
    let flagged: Vec<&str> = listed
        .entries
        .iter()
        .filter(|entry| entry.new_untracked)
        .map(|entry| entry.path.as_str())
        .collect();
    assert_eq!(flagged, ["u2.txt"], "the one still untracked and changed");
    assert_eq!(render(&listed.text("u2.txt")), ["-1: u2", "+:1 u2 changed"]);
    assert_eq!(render(&listed.text("a.txt")), ["-1: a2", "+:1 a3"]);
    assert_eq!(render(&listed.text("n.txt")), ["+:1 new"]);
    assert_eq!(render(&listed.text("u4.txt")), ["-1: u4"]);

    // Nothing of the user's was written: not the index, not an object (PLAN §2.4); and the
    // temp indexes are gone.
    assert_eq!(
        std::fs::read(repo.path().join(".git/index")).unwrap(),
        index
    );
    assert_eq!(objects(&repo), before);
    let tmp = repo.path().join(".git/dv/tmp");
    let left: Vec<_> = std::fs::read_dir(&tmp)
        .map(|dir| dir.flatten().map(|entry| entry.file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");

    // All changes still list the untracked files as new, as before.
    let all = Listed::of(&repo, Target::Working);
    assert_eq!(all.entry("u2.txt").status, FileStatus::Untracked);
    assert!(!all.entry("u2.txt").new_untracked);
}

#[test]
fn a_file_dropped_from_the_index_but_kept_is_compared_as_it_stands() {
    // The working tree, untracked files included, is the new side: a file `git rm --cached`
    // left on disk is still there, as `git add -A` would stage it.
    let repo = FixtureRepo::new();
    repo.write("kept.txt", "same\n")
        .write("edited.txt", "one\n");
    repo.commit("base");
    repo.git(&["rm", "-q", "--cached", "kept.txt", "edited.txt"]);
    repo.write("edited.txt", "two\n");
    let listed = Listed::of(&repo, Target::Working);
    assert_eq!(
        listed.summary(),
        [("edited.txt", FileStatus::Modified, 1, 1)]
    );
    assert!(listed.entry("edited.txt").new_untracked);
    assert_eq!(render(&listed.text("edited.txt")), ["-1: one", "+:1 two"]);
    // The staged view still shows the removal.
    assert_eq!(
        Listed::of(&repo, Target::Staged).summary(),
        [
            ("edited.txt", FileStatus::Deleted, 0, 1),
            ("kept.txt", FileStatus::Deleted, 0, 1)
        ]
    );
}

#[test]
fn a_staged_interdiff_shows_what_was_staged_since() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a1\n").write("b.txt", "b1\n");
    repo.commit("base");
    repo.write("a.txt", "a2\n");
    repo.git(&["add", "a.txt"]);
    let round = close_round(&repo, &Target::Staged, 1);
    repo.write("b.txt", "b2\n");
    repo.git(&["add", "b.txt"]);
    // Unstaged edits aren't the staged target's.
    repo.write("a.txt", "a3\n");
    let listed = since(&repo, Target::Staged, &round);
    assert_eq!(listed.summary(), [("b.txt", FileStatus::Modified, 1, 1)]);
    assert_eq!(render(&listed.text("b.txt")), ["-1: b1", "+:1 b2"]);
}

#[test]
fn a_branch_interdiff_follows_the_branch_checked_out_or_not() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a1\n").write("b.txt", "b1\n");
    repo.commit("base");
    repo.branch("feat").checkout("feat");
    repo.write("a.txt", "a2\n");
    repo.commit("feat 1");
    repo.write("b.txt", "b2\n");
    let feat = Target::Branch {
        branch: "feat".to_string(),
        base: Some("main".to_string()),
    };
    // Checked out: the new side is the working tree, uncommitted work included.
    let round = close_round(&repo, &feat, 1);
    repo.write("c.txt", "c1\n");
    let listed = since(&repo, feat.clone(), &round);
    assert_eq!(listed.spec.new, Side::Worktree);
    assert_eq!(listed.summary(), [("c.txt", FileStatus::Untracked, 1, 0)]);
    // Committed (`git add -A`), `c.txt` is tracked; `b.txt`, committed too, is as it was at
    // the round.
    repo.commit("feat 2");
    let listed = since(&repo, feat.clone(), &round);
    assert_eq!(listed.summary(), [("c.txt", FileStatus::Added, 1, 0)]);

    // Not checked out: the new side is the tip, and the round's snapshot is the tip it was.
    repo.checkout("main");
    let round = close_round(&repo, &feat, 2);
    assert_eq!(
        round.new_oid.as_str(),
        repo.git(&["rev-parse", "feat"]).trim()
    );
    repo.checkout("feat");
    repo.write("a.txt", "a3\n");
    repo.commit("feat 4");
    repo.checkout("main");
    let listed = since(&repo, feat, &round);
    assert!(matches!(listed.spec.new, Side::Commit(_)));
    assert_eq!(listed.summary(), [("a.txt", FileStatus::Modified, 1, 1)]);
}

#[test]
fn a_missing_snapshot_is_a_typed_error() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a1\n");
    repo.commit("base");
    repo.write("a.txt", "a2\n");
    let git = git_repo(&repo);
    let spec = Target::Working.resolve(&git).unwrap();
    let round = close_round(&repo, &Target::Working, 1);
    assert_eq!(
        spec.since_checked(&git, &round).unwrap(),
        spec.since(&round)
    );
    let gone = Round {
        new_oid: Oid::parse(&"e".repeat(40)).unwrap(),
        ..round
    };
    match spec.since_checked(&git, &gone) {
        Err(error @ TargetError::MissingSnapshot { .. }) => {
            assert_eq!(
                error.to_string(),
                "round 1's snapshot (eeeeeee) is missing from the repository",
                "one line, fit for a banner"
            );
            let TargetError::MissingSnapshot { round, oid } = error else {
                unreachable!()
            };
            assert_eq!((round, oid), (1, gone.new_oid.clone()));
        }
        other => panic!("{other:?}"),
    }
    // A tree is no commit.
    let tree = Oid::parse(repo.git(&["rev-parse", "HEAD^{tree}"]).trim()).unwrap();
    assert!(!git.has_commit(&tree).unwrap());
    assert!(git.has_commit(&gone.old_oid).unwrap());
}
