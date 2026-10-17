//! How the app reaches WSL (PLAN §5.2): which distros there are and which run, and how a
//! distro's `dv backend --stdio` starts. [`WslAccess::System`] is this machine's WSL;
//! [`WslAccess::Scripted`] stands in for it wherever nothing may reach a real distro.
//!
//! The owner's distro home holds the dv the owner installed and is off limits to tests and
//! automation runs (CLAUDE.md): they install into a scratch home through `DV_WSL_HOME` or not
//! at all. So an automation run without `DV_WSL_HOME` naming a scratch home gets the stand-in
//! ([`WslAccess::for_run`]), whose distros are the ones a scenario names (`wsl_distros`,
//! tests/e2e/README.md), running or not as it says (its connections' host check reads them),
//! and whose launch is refused, and dv_ui's own tests can't construct the real thing at all.

use std::sync::Arc;

use dv_backend::launch::{HostCheck, LaunchError, StartFn};
use dv_backend::wsl::{Distro, WslError};
use dv_backend::{Launcher, StartOptions};
use parking_lot::Mutex;

/// Why the stand-in refuses to start a distro's server.
pub const REFUSED: &str = "an automation run starts dv in a WSL distro only with DV_WSL_HOME set \
                           (a scratch home inside the distro)";

/// How the app reaches WSL (module docs). Cloning shares a stand-in's distros.
#[derive(Clone)]
pub enum WslAccess {
    /// This machine's WSL: `dv_backend::wsl::distros` and [`Launcher::wsl`] (this app's Linux
    /// builds, and `DV_WSL_HOME` when set).
    System,
    /// A stand-in: the distros it was told of, none of which a server ever starts in.
    Scripted(Arc<Mutex<Vec<Distro>>>),
}

impl WslAccess {
    /// What a run of the app uses: this machine's WSL, except in an automation run without
    /// `DV_WSL_HOME` naming a scratch home (module docs).
    pub fn for_run(automation: bool) -> Self {
        Self::for_run_with(automation, dv_backend::wsl::home_var())
    }

    /// [`WslAccess::for_run`] with `home` as `DV_WSL_HOME`'s value: an automation run gets this
    /// machine's WSL only when it names a scratch home as `dv_backend::wsl::sandbox` reads it;
    /// unset, blank or anything else that isn't one, the stand-in.
    pub fn for_run_with(automation: bool, home: Option<String>) -> Self {
        if automation && !dv_backend::wsl::is_sandbox(home) {
            Self::scripted(Vec::new())
        } else {
            Self::System
        }
    }

    /// A stand-in knowing `distros`.
    pub fn scripted(distros: Vec<Distro>) -> Self {
        Self::Scripted(Arc::new(Mutex::new(distros)))
    }

    pub fn is_scripted(&self) -> bool {
        matches!(self, Self::Scripted(_))
    }

    /// Tells a stand-in which distros there are from now on; `false` for this machine's WSL,
    /// which says so itself.
    pub fn set_distros(&self, distros: Vec<Distro>) -> bool {
        match self {
            Self::System => false,
            Self::Scripted(known) => {
                *known.lock() = distros;
                true
            }
        }
    }

    /// The installed distros and which run. Blocks while `wsl.exe` runs; never on a UI thread.
    pub fn distros(&self) -> Result<Vec<Distro>, WslError> {
        match self {
            Self::System => {
                guard_tests();
                dv_backend::wsl::distros()
            }
            Self::Scripted(known) => Ok(known.lock().clone()),
        }
    }

    /// How `distro`'s server starts.
    pub fn launcher(&self, distro: &str) -> Launcher {
        match self {
            Self::System => {
                guard_tests();
                Launcher::wsl(distro)
            }
            Self::Scripted(known) => {
                let refuse: Arc<StartFn> = Arc::new(|_: &StartOptions<'_>| {
                    Err(LaunchError {
                        message: REFUSED.to_string(),
                        permanent: true,
                    })
                });
                // Its host check reads the distros it was told of, as the real one lists them.
                let known = Arc::clone(known);
                let name = distro.to_string();
                let running: Arc<HostCheck> = Arc::new(move |_| {
                    known
                        .lock()
                        .iter()
                        .any(|distro| distro.running && distro.name.eq_ignore_ascii_case(&name))
                });
                Launcher::Custom {
                    label: format!("automation's stand-in for dv in {distro}, which never starts"),
                    start: refuse,
                    reinstalls: false,
                    running: Some(running),
                }
            }
        }
    }
}

/// dv_ui's tests never reach this machine's WSL: a test that would is a bug (module docs).
pub(crate) fn guard_tests() {
    if cfg!(test) {
        panic!("dv_ui's tests reach WSL only through WslAccess::Scripted");
    }
}
