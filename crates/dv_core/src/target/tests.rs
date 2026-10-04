use std::sync::Arc;

use dv_test_support::FixtureRepo;

use super::*;
use crate::diff::{FileEntry, FileStatus, file_list};
use crate::testing::{git_repo, open_at};

fn oid(text: &str) -> Oid {
    Oid::parse(text).unwrap()
}

fn commit(text: &str) -> Side {
    Side::Commit(oid(text))
}

/// `(path, status, additions, deletions)` per entry.
fn summary(entries: &[FileEntry]) -> Vec<(String, FileStatus, u32, u32)> {
    entries
        .iter()
        .map(|e| (e.path.clone(), e.status, e.additions, e.deletions))
        .collect()
}

fn paths(entries: &[FileEntry]) -> Vec<&str> {
    entries.iter().map(|e| e.path.as_str()).collect()
}

#[test]
fn unborn_head_working_target() {
    let repo = FixtureRepo::new();
    repo.write("staged.txt", "s1\ns2\n")
        .write("untracked.txt", "u\n");
    repo.git(&["add", "staged.txt"]);
    let git = git_repo(&repo);

    let spec = Target::Working.resolve(&git).unwrap();
    assert_eq!(
        spec,
        DiffSpec {
            old: Side::EmptyTree,
            new: Side::Worktree
        }
    );
    let entries = file_list(&git, &spec).unwrap();
    assert_eq!(
        summary(&entries),
        [
            ("staged.txt".to_string(), FileStatus::Added, 2, 0),
            ("untracked.txt".to_string(), FileStatus::Untracked, 1, 0),
        ]
    );

    let staged = Target::Staged.resolve(&git).unwrap();
    assert_eq!(
        staged,
        DiffSpec {
            old: Side::EmptyTree,
            new: Side::Index
        }
    );
    assert_eq!(paths(&file_list(&git, &staged).unwrap()), ["staged.txt"]);
}

#[test]
fn working_and_staged_compare_head_with_the_worktree_and_the_index() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n").write("b.txt", "b\n");
    let head = repo.commit("base");
    repo.write("a.txt", "a2\n");
    repo.git(&["add", "a.txt"]);
    repo.write("b.txt", "b2\n");
    let git = git_repo(&repo);

    let working = Target::Working.resolve(&git).unwrap();
    assert_eq!(working.old, commit(&head));
    assert_eq!(working.new, Side::Worktree);
    assert_eq!(
        paths(&file_list(&git, &working).unwrap()),
        ["a.txt", "b.txt"]
    );

    let staged = Target::Staged.resolve(&git).unwrap();
    assert_eq!(staged.old, commit(&head));
    assert_eq!(staged.new, Side::Index);
    assert_eq!(paths(&file_list(&git, &staged).unwrap()), ["a.txt"]);
}

#[test]
fn root_commit_commit_target() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "one\ntwo\n").write("dir/b.txt", "b\n");
    let root = repo.commit("root");
    repo.write("a.txt", "one\n");
    let second = repo.commit("second");
    let git = git_repo(&repo);

    let spec = Target::Commit { rev: root.clone() }.resolve(&git).unwrap();
    assert_eq!(
        spec,
        DiffSpec {
            old: Side::EmptyTree,
            new: commit(&root)
        }
    );
    assert_eq!(
        summary(&file_list(&git, &spec).unwrap()),
        [
            ("a.txt".to_string(), FileStatus::Added, 2, 0),
            ("dir/b.txt".to_string(), FileStatus::Added, 1, 0),
        ]
    );

    // A symbolic revision resolves to the commit it names now, and the spec keeps the id.
    let spec = Target::Commit {
        rev: "HEAD".to_string(),
    }
    .resolve(&git)
    .unwrap();
    assert_eq!(spec.old, commit(&root));
    assert_eq!(spec.new, commit(&second));
    assert_eq!(
        summary(&file_list(&git, &spec).unwrap()),
        [("a.txt".to_string(), FileStatus::Modified, 0, 1)]
    );
}

#[test]
fn merge_commit_compares_with_its_first_parent() {
    let repo = FixtureRepo::new();
    repo.write("base.txt", "base\n");
    repo.commit("base");
    repo.branch("side")
        .checkout("side")
        .write("side.txt", "side\n");
    let side = repo.commit("side work");
    repo.checkout("main").write("main.txt", "main\n");
    let main = repo.commit("main work");
    repo.git(&["merge", "--no-ff", "-q", "-m", "merge side", "side"]);
    let merge = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
    let git = git_repo(&repo);

    assert_eq!(git.parents(&oid(&merge)).unwrap(), [oid(&main), oid(&side)]);
    let spec = Target::Commit { rev: merge.clone() }.resolve(&git).unwrap();
    assert_eq!(
        spec.old,
        commit(&main),
        "the first parent, not the merged branch"
    );
    assert_eq!(spec.new, commit(&merge));
    // What the merge brought into main: the side branch's file only.
    assert_eq!(paths(&file_list(&git, &spec).unwrap()), ["side.txt"]);
}

#[test]
fn three_dot_vs_two_dot_ranges() {
    let repo = FixtureRepo::new();
    repo.write("base.txt", "base\n");
    let base = repo.commit("base");
    repo.branch("feat")
        .checkout("feat")
        .write("feat.txt", "feat\n");
    let feat = repo.commit("feature");
    repo.checkout("main").write("main.txt", "main\n");
    let main = repo.commit("main moves on");
    let git = git_repo(&repo);

    let two_dot = Target::Range {
        from: "main".to_string(),
        to: "feat".to_string(),
        three_dot: false,
    }
    .resolve(&git)
    .unwrap();
    assert_eq!(
        two_dot,
        DiffSpec {
            old: commit(&main),
            new: commit(&feat)
        }
    );
    // Two-dot compares the tips: main's own commit shows as a deletion.
    assert_eq!(
        summary(&file_list(&git, &two_dot).unwrap()),
        [
            ("feat.txt".to_string(), FileStatus::Added, 1, 0),
            ("main.txt".to_string(), FileStatus::Deleted, 0, 1),
        ]
    );

    let three_dot = Target::Range {
        from: "main".to_string(),
        to: "feat".to_string(),
        three_dot: true,
    }
    .resolve(&git)
    .unwrap();
    assert_eq!(
        three_dot,
        DiffSpec {
            old: commit(&base),
            new: commit(&feat)
        }
    );
    // Three-dot shows only what feat did since it forked.
    assert_eq!(paths(&file_list(&git, &three_dot).unwrap()), ["feat.txt"]);
}

#[test]
fn branch_target_checked_out_vs_not() {
    let repo = FixtureRepo::new();
    repo.write("base.txt", "base\n");
    let base = repo.commit("base");
    repo.branch("feat")
        .checkout("feat")
        .write("feat.txt", "feat\n");
    let feat = repo.commit("feature");
    // Uncommitted work on the checked-out branch, tracked and untracked.
    repo.write("feat.txt", "feat\nmore\n")
        .write("new.txt", "new\n");
    let git = git_repo(&repo);
    let target = Target::Branch {
        branch: "feat".to_string(),
        base: Some("main".to_string()),
    };

    // Checked out here: the new side is the working tree, uncommitted work included.
    let spec = target.resolve(&git).unwrap();
    assert_eq!(
        spec,
        DiffSpec {
            old: commit(&base),
            new: Side::Worktree
        }
    );
    assert_eq!(
        summary(&file_list(&git, &spec).unwrap()),
        [
            ("feat.txt".to_string(), FileStatus::Added, 2, 0),
            ("new.txt".to_string(), FileStatus::Untracked, 1, 0),
        ]
    );
    // `refs/heads/feat` and `heads/feat` name the same checked-out branch.
    for name in ["refs/heads/feat", "heads/feat"] {
        let spelled = Target::Branch {
            branch: name.to_string(),
            base: Some("main".to_string()),
        };
        assert_eq!(spelled.resolve(&git).unwrap(), spec, "{name}");
    }

    // Not checked out here: the branch tip, and nothing of this worktree.
    repo.git(&["stash", "-u", "-q"]);
    repo.checkout("main");
    let spec = target.resolve(&git).unwrap();
    assert_eq!(
        spec,
        DiffSpec {
            old: commit(&base),
            new: commit(&feat)
        }
    );
    assert_eq!(
        summary(&file_list(&git, &spec).unwrap()),
        [("feat.txt".to_string(), FileStatus::Added, 1, 0)]
    );
}

#[test]
fn a_branch_target_follows_the_branch_when_a_tag_shares_its_name() {
    let repo = FixtureRepo::new();
    repo.write("base.txt", "base\n");
    let base = repo.commit("base");
    repo.branch("feat")
        .checkout("feat")
        .write("feat.txt", "feat\n");
    let feat = repo.commit("feature");
    // A tag `feat` at the base, which git's revision rules would pick over the branch.
    repo.git(&["tag", "feat", &base]);
    let git = git_repo(&repo);
    let target = Target::Branch {
        branch: "feat".to_string(),
        base: Some("main".to_string()),
    };
    // Checked out: the branch's working tree.
    assert_eq!(
        target.resolve(&git).unwrap(),
        DiffSpec {
            old: commit(&base),
            new: Side::Worktree
        }
    );
    // Not checked out: the branch's tip, not the tag's commit.
    repo.checkout("main");
    assert_eq!(
        target.resolve(&git).unwrap(),
        DiffSpec {
            old: commit(&base),
            new: commit(&feat)
        }
    );
}

#[test]
fn branch_checked_out_in_another_worktree_is_its_tip() {
    let repo = FixtureRepo::new();
    repo.write("base.txt", "base\n");
    let base = repo.commit("base");
    let worktree = repo.add_worktree("wt", "feat");
    std::fs::write(worktree.join("wip.txt"), "wip\n").unwrap();
    let target = Target::Branch {
        branch: "feat".to_string(),
        base: Some("main".to_string()),
    };

    // From the main worktree, `feat` is checked out elsewhere: its tip.
    let main_view = target.resolve(&git_repo(&repo)).unwrap();
    assert_eq!(main_view.new, commit(&base));
    // From the linked worktree it is checked out here: its working tree.
    let linked = GitRepo::new(Arc::new(open_at(&repo, &worktree)));
    let linked_view = target.resolve(&linked).unwrap();
    assert_eq!(linked_view.new, Side::Worktree);
    assert_eq!(
        paths(&file_list(&linked, &linked_view).unwrap()),
        ["wip.txt"]
    );
}

#[test]
fn branch_base_defaults_to_the_repos_default_branch() {
    let repo = FixtureRepo::new();
    repo.write("base.txt", "base\n");
    let base = repo.commit("base");
    repo.branch("feat").checkout("feat").write("f.txt", "f\n");
    repo.commit("feature");
    repo.checkout("main").write("m.txt", "m\n");
    let main = repo.commit("main moves on");
    let git = git_repo(&repo);
    let target = Target::Branch {
        branch: "feat".to_string(),
        base: None,
    };

    // A local main.
    assert_eq!(git.default_branch().unwrap().as_deref(), Some("main"));
    assert_eq!(target.resolve(&git).unwrap().old, commit(&base));

    // origin/HEAD wins: here it points at a remote-tracking `trunk` at main's tip.
    repo.git(&["update-ref", "refs/remotes/origin/trunk", &main]);
    repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/trunk",
    ]);
    assert_eq!(
        git.default_branch().unwrap().as_deref(),
        Some("origin/trunk")
    );
    assert_eq!(target.resolve(&git).unwrap().old, commit(&base));

    // An origin/HEAD that points at nothing is skipped.
    repo.git(&["update-ref", "-d", "refs/remotes/origin/trunk"]);
    assert_eq!(git.default_branch().unwrap().as_deref(), Some("main"));

    // master when there is no main.
    repo.git(&["branch", "-m", "main", "master"]);
    assert_eq!(git.default_branch().unwrap().as_deref(), Some("master"));

    // None of them: a typed error naming the branch.
    repo.git(&["branch", "-m", "master", "trunk"]);
    assert_eq!(git.default_branch().unwrap(), None);
    let err = target.resolve(&git).unwrap_err();
    assert!(
        matches!(&err, TargetError::NoDefaultBase { branch } if branch == "feat"),
        "{err:?}"
    );
}

#[test]
fn pr_targets_resolve_from_the_given_oids() {
    let repo = FixtureRepo::new();
    repo.write("base.txt", "base\n");
    let fork = repo.commit("base");
    repo.write("upstream.txt", "u\n");
    let base_tip = repo.commit("upstream moves on");
    repo.checkout(&fork);
    repo.git(&["checkout", "-q", "-b", "pr"]);
    repo.write("pr.txt", "pr\n");
    let head = repo.commit("pr work");
    let git = git_repo(&repo);

    let target = Target::Pr {
        owner: "kylekz".to_string(),
        repo: "difftest".to_string(),
        number: 7,
        base_oid: oid(&base_tip),
        head_oid: oid(&head),
    };
    let spec = target.resolve(&git).unwrap();
    assert_eq!(
        spec,
        DiffSpec {
            old: commit(&fork),
            new: commit(&head)
        }
    );
    assert_eq!(paths(&file_list(&git, &spec).unwrap()), ["pr.txt"]);

    // A head that was never fetched is an unknown revision, not a fetch.
    let missing = Target::Pr {
        owner: "kylekz".to_string(),
        repo: "difftest".to_string(),
        number: 7,
        base_oid: oid(&base_tip),
        head_oid: oid("0123456789abcdef0123456789abcdef01234567"),
    };
    let err = missing.resolve(&git).unwrap_err();
    assert!(
        matches!(&err, TargetError::Git(GitError::UnknownRevision { .. })),
        "{err:?}"
    );
}

#[test]
fn unrelated_histories_have_no_merge_base() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    repo.git(&["checkout", "-q", "--orphan", "lonely"]);
    repo.git(&["rm", "-rqf", "."]);
    repo.write("z.txt", "z\n");
    repo.commit("unrelated root");
    let git = git_repo(&repo);

    for target in [
        Target::Range {
            from: "main".to_string(),
            to: "lonely".to_string(),
            three_dot: true,
        },
        Target::Branch {
            branch: "lonely".to_string(),
            base: Some("main".to_string()),
        },
    ] {
        let err = target.resolve(&git).unwrap_err();
        assert!(
            matches!(&err, TargetError::NoMergeBase { a, b } if a == "main" && b == "lonely"),
            "{target}: {err:?}"
        );
        assert_eq!(err.to_string(), "main and lonely have no common history");
    }
    // Two-dot needs no merge base.
    let two_dot = Target::Range {
        from: "main".to_string(),
        to: "lonely".to_string(),
        three_dot: false,
    };
    assert!(two_dot.resolve(&git).is_ok());
}

#[test]
fn unknown_revisions_are_typed_errors() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    let git = git_repo(&repo);
    for target in [
        Target::Commit {
            rev: "nope".to_string(),
        },
        Target::Commit {
            rev: "--output=x".to_string(),
        },
        Target::Branch {
            branch: "nope".to_string(),
            base: Some("main".to_string()),
        },
        Target::Range {
            from: "main".to_string(),
            to: "nope".to_string(),
            three_dot: false,
        },
    ] {
        let err = target.resolve(&git).unwrap_err();
        assert!(
            matches!(&err, TargetError::Git(GitError::UnknownRevision { .. })),
            "{target}: {err:?}"
        );
    }
    assert!(!repo.path().join("x").exists());
}

#[test]
fn targets_and_specs_serialise_in_a_stable_shape() {
    let head = "0123456789abcdef0123456789abcdef01234567";
    let base = "89abcdef0123456789abcdef0123456789abcdef";
    let cases = [
        (Target::Working, "\"working\"".to_string()),
        (Target::Staged, "\"staged\"".to_string()),
        (
            Target::Branch {
                branch: "feat".to_string(),
                base: None,
            },
            r#"{"branch":{"branch":"feat","base":null}}"#.to_string(),
        ),
        (
            Target::Branch {
                branch: "feat".to_string(),
                base: Some("main".to_string()),
            },
            r#"{"branch":{"branch":"feat","base":"main"}}"#.to_string(),
        ),
        (
            Target::Commit {
                rev: "HEAD~1".to_string(),
            },
            r#"{"commit":{"rev":"HEAD~1"}}"#.to_string(),
        ),
        (
            Target::Range {
                from: "v1".to_string(),
                to: "v2".to_string(),
                three_dot: true,
            },
            r#"{"range":{"from":"v1","to":"v2","three_dot":true}}"#.to_string(),
        ),
        (
            Target::Pr {
                owner: "kylekz".to_string(),
                repo: "difftest".to_string(),
                number: 482,
                base_oid: oid(base),
                head_oid: oid(head),
            },
            format!(
                r#"{{"pr":{{"owner":"kylekz","repo":"difftest","number":482,"base_oid":"{base}","head_oid":"{head}"}}}}"#
            ),
        ),
    ];
    for (target, json) in cases {
        assert_eq!(serde_json::to_string(&target).unwrap(), json);
        assert_eq!(serde_json::from_str::<Target>(&json).unwrap(), target);
    }

    let spec = DiffSpec {
        old: commit(base),
        new: Side::Worktree,
    };
    let json = format!(r#"{{"old":{{"commit":"{base}"}},"new":"worktree"}}"#);
    assert_eq!(serde_json::to_string(&spec).unwrap(), json);
    assert_eq!(serde_json::from_str::<DiffSpec>(&json).unwrap(), spec);
    for (side, json) in [
        (Side::EmptyTree, "\"empty_tree\""),
        (Side::Index, "\"index\""),
        (Side::Worktree, "\"worktree\""),
    ] {
        assert_eq!(serde_json::to_string(&side).unwrap(), json);
    }
    assert!(serde_json::from_str::<Side>(r#"{"commit":"HEAD"}"#).is_err());
}

#[test]
fn targets_display_as_labels() {
    let labels = [
        (Target::Working, "working tree"),
        (Target::Staged, "staged changes"),
        (
            Target::Branch {
                branch: "feat".to_string(),
                base: None,
            },
            "branch feat",
        ),
        (
            Target::Branch {
                branch: "feat".to_string(),
                base: Some("main".to_string()),
            },
            "branch feat vs main",
        ),
        (
            Target::Range {
                from: "a".to_string(),
                to: "b".to_string(),
                three_dot: true,
            },
            "a...b",
        ),
    ];
    for (target, label) in labels {
        assert_eq!(target.to_string(), label);
    }
}
