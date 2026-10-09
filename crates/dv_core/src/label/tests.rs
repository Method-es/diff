use dv_test_support::FixtureRepo;

use super::*;
use crate::testing::git_repo;

#[test]
fn target_labels_name_what_is_compared() {
    let context = LabelContext {
        head: Some("main".into()),
        branch: Some("main".into()),
        default_base: Some("origin/main".into()),
    };
    let none = LabelContext::default();
    let branch = |base: Option<&str>| Target::Branch {
        branch: "feat".into(),
        base: base.map(str::to_string),
    };
    for (target, labels, label) in [
        (Target::Working, &context, "working tree vs main"),
        (Target::Working, &none, "working tree"),
        (Target::Staged, &context, "staged vs main"),
        (branch(None), &context, "feat vs origin/main"),
        (branch(Some("dev")), &context, "feat vs dev"),
        (branch(None), &none, "feat"),
        (
            Target::Commit {
                rev: "abc1234".into(),
            },
            &context,
            "commit abc1234",
        ),
        (
            Target::Range {
                from: "v1".into(),
                to: "v2".into(),
                three_dot: true,
            },
            &context,
            "v1...v2",
        ),
    ] {
        assert_eq!(target_label(&target, labels), label, "{target:?}");
    }
}

#[test]
fn side_labels_name_each_side() {
    let context = LabelContext {
        head: Some("main".into()),
        branch: Some("main".into()),
        default_base: Some("origin/main".into()),
    };
    let label = |target: Target| side_labels(&target, &context);
    let pair = |old: &str, new: &str| (old.to_string(), new.to_string());
    assert_eq!(label(Target::Working), pair("main", "working tree"));
    assert_eq!(label(Target::Staged), pair("main", "staged"));
    assert_eq!(
        label(Target::Branch {
            branch: "feat".into(),
            base: None
        }),
        pair("origin/main", "feat")
    );
    assert_eq!(
        label(Target::Commit { rev: "abc".into() }),
        pair("abc^", "abc")
    );
    assert_eq!(
        side_labels(&Target::Working, &LabelContext::default()),
        pair("HEAD", "working tree")
    );
}

#[test]
fn the_branch_target_is_the_checked_out_branch_against_the_default() {
    let labels = |branch: Option<&str>, base: Option<&str>| LabelContext {
        head: branch.map(str::to_string),
        branch: branch.map(str::to_string),
        default_base: base.map(str::to_string),
    };
    assert_eq!(
        labels(Some("feat"), Some("main")).branch_target(),
        Some(Target::Branch {
            branch: "feat".into(),
            base: None
        })
    );
    // Unpushed work on main is worth a look against origin/main.
    assert!(
        labels(Some("main"), Some("origin/main"))
            .branch_target()
            .is_some()
    );
    assert_eq!(labels(Some("main"), Some("main")).branch_target(), None);
    assert_eq!(labels(None, Some("main")).branch_target(), None, "detached");
    assert_eq!(labels(Some("feat"), None).branch_target(), None);
}

#[test]
fn the_context_is_read_from_the_repository() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    let head = repo.commit("base");
    repo.branch("feat").checkout("feat");
    let context = LabelContext::read(&git_repo(&repo));
    assert_eq!(context.head.as_deref(), Some("feat"));
    assert_eq!(context.branch.as_deref(), Some("feat"));
    assert_eq!(context.default_base.as_deref(), Some("main"));

    repo.checkout(&head);
    let detached = LabelContext::read(&git_repo(&repo));
    assert_eq!(detached.head.as_deref(), Some(&head[..7]));
    assert_eq!(detached.branch, None);
}

#[test]
fn a_working_review_is_named_by_its_own_worktrees_head() {
    use crate::backend::Backend as _;

    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    let head = repo.commit("base");
    let feature = repo.add_worktree("wt", "feature");
    let detached = repo.add_worktree("wt2", "other");
    let other = crate::testing::open_at(&repo, &detached);
    other
        .git(
            &["checkout", "-q", "--detach"],
            &crate::backend::GitOpts::new(),
        )
        .unwrap();

    let main = git_repo(&repo);
    let labels = Labels::read(&main);
    let wt = crate::testing::open_at(&repo, &feature)
        .repo()
        .toplevel
        .clone();
    let wt2 = other.repo().toplevel.clone();
    let here = main.info().toplevel.clone();
    assert_eq!(
        labels.label(&Target::Working, &here),
        "working tree vs main"
    );
    assert_eq!(
        labels.label(&Target::Working, &wt),
        "working tree vs feature"
    );
    assert_eq!(labels.label(&Target::Staged, &wt), "staged vs feature");
    assert_eq!(
        labels.label(&Target::Working, &wt2),
        format!("working tree vs {}", &head[..7])
    );
    // Recorded with the other separator or a trailing one, it is the same worktree.
    assert_eq!(
        labels.label(&Target::Working, &format!("{}/", wt.replace('\\', "/"))),
        "working tree vs feature"
    );
    // A worktree that is gone has no HEAD to name; other targets don't depend on it.
    assert_eq!(
        labels.label(&Target::Working, "/no/such/worktree"),
        "working tree"
    );
    let branch = Target::Branch {
        branch: "feature".to_string(),
        base: None,
    };
    assert_eq!(labels.label(&branch, &wt), "feature vs main");

    // Read from the linked worktree, the main checkout's review is named by main.
    let linked = Labels::read(&GitRepo::new(std::sync::Arc::new(crate::testing::open_at(
        &repo, &feature,
    ))));
    assert_eq!(
        linked.label(&Target::Working, &here),
        "working tree vs main"
    );
    assert_eq!(
        linked.label(&Target::Working, &wt),
        "working tree vs feature"
    );
}
