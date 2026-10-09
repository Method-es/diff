//! Every error a backend returns reads the same through the wire: the same variant, the same
//! `Display` and the same `detail()` (DESIGN §4.4's banner headline and its diagnostics).

use std::time::Duration;

use dv_core::backend::{Backend, BackendError, GitOpts, WatchSpec};
use pretty_assertions::assert_eq;

use crate::{Side, Subject, committed};

/// Asserts `local` and `remote` are the same failure, and returns the local one.
fn same<T: std::fmt::Debug>(
    what: &str,
    local: Result<T, BackendError>,
    remote: Result<T, BackendError>,
) -> BackendError {
    let local = local.expect_err(what);
    let remote = remote.expect_err(what);
    assert_eq!(
        std::mem::discriminant(&remote),
        std::mem::discriminant(&local),
        "{what}: {local:?} vs {remote:?}"
    );
    assert_eq!(remote.to_string(), local.to_string(), "{what}");
    assert_eq!(remote.detail(), local.detail(), "{what}");
    local
}

#[test]
fn opening_what_isnt_a_worktree_fails_alike() {
    let repo = committed();
    let (local, remote) = (Subject::new(Side::Local), Subject::new(Side::Remote));
    let outside = repo.outside_dir("plain");
    let bare = repo.bare_clone("bare.git");
    let cases = [
        ("not a repository", outside.clone()),
        ("missing", outside.join("missing")),
        ("a file", repo.path().join("a.txt")),
        ("bare", bare),
        ("inside the git dir", repo.path().join(".git").join("refs")),
    ];
    let mut kinds = Vec::new();
    for (what, path) in cases {
        let error = same(
            what,
            local.open_at(&repo, &path).map(drop),
            remote.open_at(&repo, &path).map(drop),
        );
        kinds.push(
            format!("{error:?}")
                .split([' ', '{'])
                .next()
                .unwrap()
                .to_string(),
        );
    }
    assert_eq!(
        kinds,
        [
            "NotARepository",
            "PathNotFound",
            "NotARepository",
            "BareRepository",
            "InsideGitDir"
        ]
    );
}

#[test]
fn every_call_fails_alike() {
    let repo = committed();
    let local = Subject::new(Side::Local).open(&repo);
    let remote = Subject::new(Side::Remote).open(&repo);
    let both = |f: &dyn Fn(&dyn Backend) -> Result<(), BackendError>| {
        (f(local.as_ref()), f(remote.as_ref()))
    };

    let (l, r) = both(&|b| {
        b.git(&["rev-parse", "--verify", "nope"], &GitOpts::default())
            .map(drop)
    });
    assert!(matches!(same("git", l, r), BackendError::Git { .. }));

    let (l, r) = both(&|b| {
        b.git(
            &["-c", "alias.slow=!sleep 3", "slow"],
            &GitOpts::new().with_timeout(Duration::from_millis(200)),
        )
        .map(drop)
    });
    assert!(matches!(
        same("timeout", l, r),
        BackendError::Timeout { .. }
    ));

    let (l, r) = both(&|b| b.worktree_read("../x").map(drop));
    assert!(matches!(
        same("invalid path", l, r),
        BackendError::InvalidPath { .. }
    ));

    let (l, r) = both(&|b| b.store_append("reviews/r.jsonl", b"a\nb"));
    assert!(matches!(
        same("invalid line", l, r),
        BackendError::InvalidLine { .. }
    ));

    let (l, r) = both(&|b| b.worktree_read("sub").map(drop));
    let error = same("a directory", l, r);
    assert!(matches!(error, BackendError::Io { .. }));
    assert!(error.to_string().starts_with("reading "), "{error}");

    let (l, r) = both(&|b| {
        b.git(&["status"], &GitOpts::new().with_index_file("a/../b"))
            .map(drop)
    });
    assert!(matches!(
        same("index file", l, r),
        BackendError::InvalidPath { .. }
    ));
}

#[test]
fn a_watch_the_os_refuses_fails_alike() {
    // A linked worktree removed after it opened: the store is still watched, the worktree
    // can't be, and a watch of the worktree alone fails.
    let repo = committed();
    let worktree = repo.add_worktree("wt", "feat");
    let local = Subject::new(Side::Local).open_at(&repo, &worktree).unwrap();
    let remote = Subject::new(Side::Remote)
        .open_at(&repo, &worktree)
        .unwrap();
    std::fs::remove_dir_all(&worktree).unwrap();

    let error = same(
        "watch",
        local.watch(WatchSpec::WORKTREE, Box::new(|_| {})).map(drop),
        remote
            .watch(WatchSpec::WORKTREE, Box::new(|_| {}))
            .map(drop),
    );
    assert!(matches!(error, BackendError::Watch { .. }), "{error:?}");

    let local = local.watch(WatchSpec::ALL, Box::new(|_| {})).unwrap();
    let remote = remote.watch(WatchSpec::ALL, Box::new(|_| {})).unwrap();
    assert!(local.worktree_error().is_some(), "{local:?}");
    assert_eq!(remote.worktree_error(), local.worktree_error());
}
