//! The `Backend` conformance suite (docs/PLAN.md §5.1, §14.2): the same cases against
//! `LocalBackend` and against `RemoteBackend` over a server on in-memory pipes
//! (`Launcher::InProcess`), so everything above the trait can't tell them apart. Each case is
//! a function taking a [`Subject`]; [`conformance!`] makes a `local::<case>` and a
//! `remote::<case>` test of it.

mod errors;
mod files;
mod git;
mod watch;

use std::path::Path;
use std::sync::Arc;

use dv_backend::{Connection, Launcher};
use dv_core::backend::{Backend, BackendError, LocalBackend, LocalBackendOptions};
use dv_test_support::FixtureRepo;
use parking_lot::Mutex;

/// Which implementation a case runs against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Local,
    Remote,
}

/// What a case opens its backends through: one connection per case on the remote side.
pub struct Subject {
    pub side: Side,
    connection: Mutex<Option<Arc<Connection>>>,
}

impl Subject {
    pub fn new(side: Side) -> Self {
        Self {
            side,
            connection: Mutex::new(None),
        }
    }

    /// The backend of the worktree at `repo`'s root, with the fixture's environment.
    pub fn open(&self, repo: &FixtureRepo) -> Arc<dyn Backend> {
        self.open_at(repo, repo.path())
            .unwrap_or_else(|error| panic!("opening {}: {error:?}", repo.path().display()))
    }

    /// The backend of the worktree containing `path`, with `repo`'s environment.
    pub fn open_at(
        &self,
        repo: &FixtureRepo,
        path: &Path,
    ) -> Result<Arc<dyn Backend>, BackendError> {
        let env = repo.env_overlay();
        match self.side {
            Side::Local => Ok(Arc::new(LocalBackend::open_with(
                path,
                LocalBackendOptions { env },
            )?)),
            Side::Remote => {
                let connection = self
                    .connection
                    .lock()
                    .get_or_insert_with(|| Connection::new("in-process", Launcher::InProcess))
                    .clone();
                let path = path.to_str().expect("fixture paths are Unicode");
                Ok(Arc::new(connection.open(path, env)?))
            }
        }
    }
}

/// `local::<case>` and `remote::<case>` tests of each case function in the calling module.
#[macro_export]
macro_rules! conformance {
    ($($case:ident),* $(,)?) => {
        mod local {
            $(
                #[test]
                fn $case() {
                    super::$case(&$crate::Subject::new($crate::Side::Local));
                }
            )*
        }
        mod remote {
            $(
                #[test]
                fn $case() {
                    super::$case(&$crate::Subject::new($crate::Side::Remote));
                }
            )*
        }
    };
}

/// A repo with two committed files, one of them two directories deep.
pub fn committed() -> FixtureRepo {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n").write("sub/deep/f.txt", "f\n");
    repo.commit("base");
    repo
}

/// Whether two paths name the same directory, however each is spelled.
pub fn same_dir(a: impl AsRef<Path>, b: impl AsRef<Path>) -> bool {
    let canonical = |path: &Path| {
        std::fs::canonicalize(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
    };
    canonical(a.as_ref()) == canonical(b.as_ref())
}
