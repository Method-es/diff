//! The `Backend` conformance suite (docs/PLAN.md §5.1, §14.2): the same cases against
//! `LocalBackend` and against `RemoteBackend` over a server on in-memory pipes
//! (`Launcher::InProcess`), so everything above the trait can't tell them apart. Each case is
//! a function taking a [`Subject`]; [`conformance!`] makes a `local::<case>` and a
//! `remote::<case>` test of it, and with `child:` also a `child::<case>` test against a server
//! that is a process of its own (the `dv_test_child serve` loopback), where processes it
//! spawns are grandchildren of the test.

mod errors;
mod files;
mod git;
mod process;
mod server;
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
    /// `RemoteBackend` over a child process's pipes.
    Child,
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
            Side::Remote | Side::Child => {
                let connection = self.connection();
                let path = path.to_str().expect("fixture paths are Unicode");
                Ok(Arc::new(connection.open(path, env)?))
            }
        }
    }

    /// The case's connection, made on first use (remote sides only).
    pub fn connection(&self) -> Arc<Connection> {
        self.connection
            .lock()
            .get_or_insert_with(|| match self.side {
                Side::Child => Connection::new("child", child_server()),
                _ => Connection::new("in-process", Launcher::InProcess),
            })
            .clone()
    }
}

/// The helper program (`src/bin/dv_test_child.rs`).
pub fn helper() -> String {
    env!("CARGO_BIN_EXE_dv_test_child").to_string()
}

/// A loopback server that is a process of its own: the helper's `serve`.
pub fn child_server() -> Launcher {
    Launcher::Command {
        program: helper().into(),
        args: vec!["serve".into()],
        env: vec![("RUST_LOG".into(), None)],
        build: None,
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
    (child: $($case:ident),* $(,)?) => {
        $crate::conformance!($($case),*);
        mod child {
            $(
                #[test]
                fn $case() {
                    super::$case(&$crate::Subject::new($crate::Side::Child));
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
