//! Velopack in dv's Windows and macOS binaries (docs/PLAN.md D9, §12, §13.3): [`run`] is the
//! first thing `main` does in `dv` and `dv-gui`.
//!
//! **The hooks** are Windows': Velopack runs none on macOS, where `dv setup path` (the app's
//! first-run screen offers it) links `~/.local/bin/dv` to the bundle's `dv` (PLAN §12.2), a
//! path every update keeps. On Windows Velopack's `Update.exe` runs the main executable
//! (`dv-gui.exe`) with `--veloapp-install`, `--veloapp-updated` or `--veloapp-uninstall` and the version as it
//! installs, updates or uninstalls the app; `VelopackApp` runs the hook and exits, before
//! anything else of dv runs, with no window. Each is `dv setup path`'s, called as a function
//! ([`apply`]): after an install the installed copy's folder goes on the user `Path` (`dv setup
//! path --yes`), before an uninstall it comes off (`dv setup path --remove`), so PATH stays the
//! app's to manage (PLAN §12) and the installer has no logic of its own. An update changes
//! nothing on PATH: the folder is always Velopack's stable `current` ([`installed_dir`]), never
//! a versioned one, and a user who took it off with `--remove` keeps it off. The hooks are
//! quick (one registry read and write) and never show anything; a hook that fails leaves `dv
//! doctor`'s "dv on PATH" saying so. Only a packaged build that Velopack's locator finds
//! installed acts on them; anything else does nothing.
//!
//! **Updates apply only from Restart to update** (`crate::updates`): applying one ends every dv
//! process running from the install, so neither binary applies a downloaded update by itself
//! at startup.
//!
//! A command line that isn't all Unicode skips Velopack (its builder reads `env::args`, which
//! panics on one; the hooks' arguments are ASCII) ([`should_run`]). A development build isn't
//! installed: Velopack finds no `Update.exe` above it (no app bundle around it, on macOS) and
//! does nothing, and no test ever runs a binary with the hooks' arguments.

use std::ffi::OsString;
#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;

#[cfg(windows)]
use dv_cli::checks::{Seams, path};
use velopack::VelopackApp;
use velopack::locator::{LocationContext, auto_locate_app_manifest};

/// What `Update.exe` is doing ([`apply`]).
#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hook {
    Installed,
    Updated,
    Uninstalling,
}

/// Whether Velopack may read this command line `args` (module docs).
pub fn should_run(args: &[OsString]) -> bool {
    args.iter().all(|arg| arg.to_str().is_some())
}

/// Hands this process's command line to Velopack first (module docs): a hook runs and the
/// process exits; anything else goes on.
pub fn run() {
    let args: Vec<OsString> = std::env::args_os().collect();
    if !should_run(&args) {
        return;
    }
    let mut app = VelopackApp::build().set_auto_apply_on_startup(false);
    #[cfg(windows)]
    {
        app = app
            .on_after_install_fast_callback(|_| hook(Hook::Installed))
            .on_after_update_fast_callback(|_| hook(Hook::Updated))
            .on_before_uninstall_fast_callback(|_| hook(Hook::Uninstalling));
    }
    app.run();
}

/// Runs `hook` for the installed copy, with the real user `Path` unless `DV_USER_PATH_FILE`
/// names its stand-in (the seam, PLAN §12).
#[cfg(windows)]
fn hook(hook: Hook) {
    let cwd = std::env::current_dir().unwrap_or_default();
    let seams = Seams::from_env(&dv_cli::Env::Process, &cwd, Vec::new());
    if let Some(Err(error)) = hook_in(hook, installed_dir().as_deref(), &seams) {
        // No console and no window: `dv doctor` shows what PATH holds.
        eprintln!("dv: {error}");
    }
}

/// `hook` for the installed copy in `dir` (`None`: this isn't one, and nothing happens),
/// through `seams`; `None` when it does nothing.
#[cfg(windows)]
pub fn hook_in(
    hook: Hook,
    dir: Option<&Path>,
    seams: &Seams,
) -> Option<Result<path::Done, String>> {
    let dir = dir?;
    match hook {
        Hook::Updated => None,
        Hook::Installed | Hook::Uninstalling => Some(apply(hook, dir, seams)),
    }
}

/// The installed copy's folder, Velopack's `current` on Windows and the bundle's
/// `Contents/MacOS` on macOS: `None` unless this is a packaged build that Velopack's locator
/// finds installed.
pub fn installed_dir() -> Option<PathBuf> {
    if !dv_cli::is_packaged() {
        return None;
    }
    auto_locate_app_manifest(LocationContext::FromCurrentExe)
        .ok()
        .map(|locator| locator.get_current_bin_dir())
}

/// What `hook` does to PATH for the installed copy in `dir` (module docs), through `seams`.
#[cfg(windows)]
pub fn apply(hook: Hook, dir: &Path, seams: &Seams) -> Result<path::Done, String> {
    match hook {
        Hook::Installed => path::add_dir(seams, Some(dir)),
        Hook::Uninstalling => path::remove_dir(seams, Some(dir)),
        Hook::Updated => Err("an update leaves PATH as it is".to_string()),
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::collections::BTreeMap;

    use pretty_assertions::assert_eq;

    use super::*;

    /// Seams whose user `Path` is the stand-in `file` (never the registry).
    fn seams(file: &Path) -> Seams {
        let vars = BTreeMap::from([("DV_USER_PATH_FILE".to_string(), file.display().to_string())]);
        Seams::from_env(&dv_cli::Env::Vars(vars), Path::new("."), Vec::new())
    }

    #[test]
    fn install_puts_the_current_folder_on_path_and_uninstall_takes_it_off() {
        let scratch = tempfile::tempdir().unwrap();
        let file = scratch.path().join("user-path.txt");
        let old = scratch.path().join("dist");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("dv.exe"), "the first implementation's").unwrap();
        let current = scratch.path().join("dv").join("current");
        std::fs::write(&file, format!(r"C:\Windows;{}", old.display())).unwrap();
        let seams = seams(&file);
        let read = || {
            std::fs::read_to_string(&file)
                .unwrap()
                .trim_end()
                .to_string()
        };

        // Ahead of the older dv.exe's folder.
        let done = hook_in(Hook::Installed, Some(&current), &seams).unwrap();
        assert_eq!(done.unwrap().action, "added");
        assert_eq!(
            read(),
            format!(r"C:\Windows;{};{}", current.display(), old.display())
        );
        let done = hook_in(Hook::Uninstalling, Some(&current), &seams).unwrap();
        assert_eq!(done.unwrap().action, "removed");
        assert_eq!(read(), format!(r"C:\Windows;{}", old.display()));
        // Again: nothing to take off.
        let done = hook_in(Hook::Uninstalling, Some(&current), &seams).unwrap();
        assert_eq!(done.unwrap().action, "absent");
    }

    #[test]
    fn an_update_leaves_path_alone_even_after_a_remove() {
        let scratch = tempfile::tempdir().unwrap();
        let file = scratch.path().join("user-path.txt");
        let current = scratch.path().join("dv").join("current");
        std::fs::write(&file, r"C:\Windows").unwrap();
        let seams = seams(&file);
        // `dv setup path --remove` took it off; the next update doesn't put it back.
        assert!(hook_in(Hook::Updated, Some(&current), &seams).is_none());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), r"C:\Windows");
    }

    #[test]
    fn a_build_that_isnt_installed_does_nothing() {
        let scratch = tempfile::tempdir().unwrap();
        let file = scratch.path().join("user-path.txt");
        let seams = seams(&file);
        for hook in [Hook::Installed, Hook::Updated, Hook::Uninstalling] {
            assert!(hook_in(hook, None, &seams).is_none(), "{hook:?}");
        }
        assert!(!file.exists());
        // A development build in `target`: not packaged, and no `Update.exe` above it.
        assert_eq!(installed_dir(), None);
        assert!(auto_locate_app_manifest(LocationContext::FromCurrentExe).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn a_command_line_that_isnt_unicode_skips_velopack() {
        use std::os::windows::ffi::OsStringExt as _;
        let plain: Vec<OsString> = ["dv-gui.exe", "--veloapp-install", "0.1.0"]
            .iter()
            .map(OsString::from)
            .collect();
        assert!(should_run(&plain));
        // A lone surrogate: a file name Windows allows and Rust's `env::args` panics on.
        let mut odd = plain.clone();
        odd.push(OsString::from_wide(&[0x0061, 0xD800]));
        assert!(!should_run(&odd));
    }
}
