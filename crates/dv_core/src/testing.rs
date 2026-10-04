//! Helpers shared by dv_core's unit tests: backends opened hermetically on fixture repos
//! (docs/PLAN.md §14.1).

use std::path::Path;
use std::sync::Arc;

use dv_test_support::FixtureRepo;

use crate::backend::{LocalBackend, LocalBackendOptions};
use crate::git::GitRepo;

/// Options that run git with exactly the fixture's environment.
pub(crate) fn options(repo: &FixtureRepo) -> LocalBackendOptions {
    LocalBackendOptions {
        env: repo.env_overlay(),
    }
}

/// A backend on the fixture's work tree.
pub(crate) fn open(repo: &FixtureRepo) -> LocalBackend {
    open_at(repo, repo.path())
}

/// A backend opened at `path` (a subdirectory, a linked worktree) with the fixture's environment.
pub(crate) fn open_at(repo: &FixtureRepo, path: &Path) -> LocalBackend {
    LocalBackend::open_with(path, options(repo))
        .unwrap_or_else(|err| panic!("opening {}: {err:?}", path.display()))
}

/// A [`GitRepo`] over [`open`].
pub(crate) fn git_repo(repo: &FixtureRepo) -> GitRepo {
    GitRepo::new(Arc::new(open(repo)))
}

/// Whether two paths name the same directory, however each is spelled.
pub(crate) fn same_dir(a: impl AsRef<Path>, b: impl AsRef<Path>) -> bool {
    let canonical = |path: &Path| {
        std::fs::canonicalize(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
    };
    canonical(a.as_ref()) == canonical(b.as_ref())
}
