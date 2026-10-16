//! The shell-`PATH` reader against the real WSL (docs/PLAN.md §18.4, §18.7): a distro's
//! `dv backend --stdio` in a sandboxed home runs the user's shell, and fake shells there, and
//! the reader finds the `PATH` between its markers or falls back. Runs only when
//! `DV_TEST_WSL_DISTRO` names a distro, with `DV_WSL_HOME` a scratch folder under its `/tmp`
//! (CLAUDE.md), one test at a time, after `script/build-linux`:
//!
//! ```text
//! DV_TEST_WSL_DISTRO=Ubuntu DV_WSL_HOME=/tmp/dv-check-1 \
//!     cargo test -p dv_lsp --test wsl_shell -- --test-threads=1
//! ```
//!
//! The sandbox's `$HOME` has none of the user's startup files, so this checks the markers and
//! the fallbacks, not what the user's own files add.

#![cfg(windows)]

#[path = "../../dv_backend/tests/wsl_common/mod.rs"]
mod wsl_common;

use std::time::Duration;

use dv_backend::install;
use dv_backend::wsl::{self, Shell};
use dv_backend::{Connection, Launcher};
use dv_lsp::resolve::{PathFrom, shell};
use pretty_assertions::assert_eq;

use self::wsl_common::*;

/// The distro and the scratch root, when the tests run.
struct Sandbox {
    distro: String,
    root: String,
}

impl Sandbox {
    fn get() -> Option<Self> {
        let distro = std::env::var("DV_TEST_WSL_DISTRO")
            .ok()
            .filter(|distro| !distro.trim().is_empty())?;
        let root = std::env::var(install::WSL_HOME_VAR).unwrap_or_default();
        let root = wsl::sandbox(Some(root.clone()))
            .ok()
            .flatten()
            .filter(|root| root.starts_with("/tmp/"))
            .unwrap_or_else(|| {
                panic!(
                    "DV_TEST_WSL_DISTRO is set, so DV_WSL_HOME must be a scratch folder under \
                     /tmp in {distro}, not {root:?}"
                )
            });
        Some(Self {
            distro: distro.trim().to_string(),
            root,
        })
    }

    /// A home of the test's own, removed when dropped, and the shell whose `$HOME` it is.
    fn home(&self, name: &str) -> (TmpDir, Shell) {
        let root = Shell::wsl(self.distro.clone(), Some(self.root.clone())).unwrap();
        let dir = TmpDir::new(&root, &self.root, name);
        let shell = Shell::wsl(self.distro.clone(), Some(dir.path.clone())).unwrap();
        (dir, shell)
    }

    fn connection(&self, home: &TmpDir) -> std::sync::Arc<Connection> {
        Connection::new(
            &self.distro,
            Launcher::Wsl {
                distro: self.distro.clone(),
                builds: builds_dir(),
                home: Some(home.path.clone()),
            },
        )
    }
}

macro_rules! sandbox {
    () => {
        match Sandbox::get() {
            Some(sandbox) => sandbox,
            None => {
                eprintln!("skipped: DV_TEST_WSL_DISTRO isn't set");
                return;
            }
        }
    };
}

/// Writes `text` to `path` in the distro, executable.
fn script(shell: &Shell, path: &str, text: &str) {
    sh(
        shell,
        r#"printf '%s' "$2" > "$1" && chmod 755 "$1""#,
        &[path, text],
    );
}

#[test]
fn the_users_shell_in_the_sandbox_gives_a_path() {
    let sandbox = sandbox!();
    let (home, shell) = sandbox.home("lsp-shell");
    let repo = home.join("repo");
    scratch_repo(&shell, &repo);
    let connection = sandbox.connection(&home);
    connection.retry().unwrap();
    let backend = connection.open(&repo, Vec::new()).unwrap();
    let host = shell::read(&backend);
    assert_eq!(host.home(), Some(home.path.as_str()));
    assert!(
        host.path().iter().any(|dir| dir == "/usr/bin"),
        "{:?} from {:?}",
        host.path(),
        host.from()
    );
    assert!(
        host.path().iter().all(|dir| !dir.starts_with("/mnt/c/")),
        "Windows folders are left out: {:?}",
        host.path()
    );
    assert!(
        matches!(host.from(), PathFrom::Shell { .. } | PathFrom::Backend),
        "{:?}",
        host.from()
    );
}

#[test]
fn fake_shells_in_the_distro_print_banners_or_hang() {
    let sandbox = sandbox!();
    let (home, shell) = sandbox.home("lsp-fake-shell");
    let repo = home.join("repo");
    scratch_repo(&shell, &repo);
    let fake = |name: &str, hang: bool| {
        let path = home.join(name);
        let hang = if hang { "sleep 30" } else { "true" };
        script(
            &shell,
            &path,
            &format!(
                "#!/bin/sh\necho 'Welcome to the fake shell'\nprintf '\\033]0;title\\007'\n\
                 cat >/dev/null\ninteractive=no\nfor arg in \"$@\"; do [ \"$arg\" = -i ] && \
                 interactive=yes; done\nnewpath=/usr/bin:/bin\nif [ $interactive = yes ]; then \
                 {hang}; newpath=/opt/fake/bin:/usr/bin:/mnt/c/Windows; fi\nwhile [ $# -gt 0 ] \
                 && [ \"$1\" != -c ]; do shift; done\nshift\nPATH=$newpath\neval \"$1\"\necho \
                 bye\n"
            ),
        );
        let env = home.join(&format!("{name}-env"));
        script(
            &shell,
            &env,
            &format!(
                "#!/bin/sh\necho HOME={}\necho SHELL={path}\necho PATH=/sbin\n",
                home.path
            ),
        );
        (path, env)
    };
    let connection = sandbox.connection(&home);
    connection.retry().unwrap();
    let backend = connection.open(&repo, Vec::new()).unwrap();

    let (zsh, env) = fake("zsh", false);
    let host = shell::read_with(&backend, &env, Duration::from_secs(10));
    assert_eq!(host.path(), ["/opt/fake/bin", "/usr/bin"]);
    assert_eq!(
        host.from(),
        &PathFrom::Shell {
            shell: zsh,
            flags: "-l -i".into()
        }
    );

    let (hanging, env) = fake("hanging", true);
    let host = shell::read_with(&backend, &env, Duration::from_secs(1));
    assert_eq!(host.path(), ["/usr/bin", "/bin"]);
    assert_eq!(
        host.from(),
        &PathFrom::Shell {
            shell: hanging,
            flags: "-l".into()
        }
    );
}
