//! The file that starts this dv again: what `~/.local/bin/dv` links to (`dv setup path`, PLAN
//! §12.2) and what the detached app runs (`dv open`, PLAN §13.1).
//!
//! That is this executable, except for the app's own binary and in a Linux AppImage. The app's
//! binary, `dv-gui` (`dv-gui.exe` on Windows, PLAN §13.1), refuses the commands, so the `dv`
//! beside it is the one that starts this dv again: what the app's own "Add to PATH" links on
//! macOS, where `dv.app/Contents/MacOS` holds both.
//!
//! In a Linux AppImage (PLAN §13.3) the executable lies in the AppImage's mount (`$APPDIR`),
//! which goes when the process that mounted it exits, so it is the AppImage itself
//! (`$APPIMAGE`). An AppImage's runtime sets both for the program it starts, and every child
//! inherits them: a terminal in another AppImage (an editor's) hands that one's `$APPIMAGE` to a
//! `dv` run there. So `$APPIMAGE` counts only while this executable lies inside `$APPDIR`, both
//! compared canonical.

use std::path::{Path, PathBuf};

use crate::Env;

/// The file that starts this dv again (module docs), reading `$APPIMAGE` and `$APPDIR` from
/// `env`.
pub fn this_executable(env: &Env) -> std::io::Result<PathBuf> {
    Ok(executable_for(env, std::env::current_exe()?))
}

/// [`this_executable`] for the executable `exe`.
pub fn executable_for(env: &Env, exe: PathBuf) -> PathBuf {
    let exe = console_beside(resolved(exe));
    if !cfg!(target_os = "linux") {
        return exe;
    }
    let (Some(image), Some(appdir)) = (env.non_empty("APPIMAGE"), env.non_empty("APPDIR")) else {
        return exe;
    };
    let image = PathBuf::from(image);
    let inside = match (
        std::fs::canonicalize(&exe),
        std::fs::canonicalize(Path::new(&appdir)),
    ) {
        (Ok(exe), Ok(appdir)) => exe.starts_with(appdir),
        _ => false,
    };
    if inside && image.is_absolute() && image.is_file() {
        image
    } else {
        exe
    }
}

/// `exe` with its links resolved on macOS, whose `current_exe` is the path dv was started by:
/// run through `~/.local/bin/dv`, the bundle's `dv` would otherwise be that link, beside which
/// there is no `dv-gui` to detach to, and an app started from it would lie outside its bundle,
/// where Velopack finds no install and updates are off. Elsewhere `exe` as it is (Linux's
/// `current_exe` is resolved already; Windows' canonical form is a `\\?\` path).
fn resolved(exe: PathBuf) -> PathBuf {
    if cfg!(target_os = "macos") {
        std::fs::canonicalize(&exe).unwrap_or(exe)
    } else {
        exe
    }
}

/// The `dv` beside `exe` when `exe` is the app's binary `dv-gui` and that `dv` is there (module
/// docs); else `exe`.
fn console_beside(exe: PathBuf) -> PathBuf {
    let (gui, console) = if cfg!(windows) {
        ("dv-gui.exe", "dv.exe")
    } else {
        ("dv-gui", "dv")
    };
    let is_gui = exe
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(gui));
    let beside = exe.with_file_name(console);
    if is_gui && beside.is_file() {
        beside
    } else {
        exe
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// A scratch folder and its path as [`resolved`] gives it (macOS's temp folder is behind a
    /// link, `/var` → `/private/var`).
    fn scratch() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = resolved(dir.path().to_path_buf());
        (dir, path)
    }

    fn env(image: &Path, appdir: &Path) -> Env {
        Env::Vars(BTreeMap::from([
            ("APPIMAGE".to_string(), image.display().to_string()),
            ("APPDIR".to_string(), appdir.display().to_string()),
        ]))
    }

    #[test]
    fn the_app_s_binary_is_the_dv_beside_it() {
        let (gui, console) = if cfg!(windows) {
            ("dv-gui.exe", "dv.exe")
        } else {
            ("dv-gui", "dv")
        };
        let (_scratch, dir) = scratch();
        let app = dir.join(gui);
        std::fs::write(&app, b"").unwrap();
        // Alone, it is itself.
        assert_eq!(executable_for(&Env::empty(), app.clone()), app);
        let dv = dir.join(console);
        std::fs::write(&dv, b"").unwrap();
        assert_eq!(executable_for(&Env::empty(), app.clone()), dv);
        assert_eq!(executable_for(&Env::empty(), dv.clone()), dv);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_link_to_the_bundle_s_dv_is_that_dv() {
        let (_scratch, dir) = scratch();
        let bundle = dir.join("dv.app").join("Contents").join("MacOS");
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(bundle.join("dv"), b"").unwrap();
        std::fs::write(bundle.join("dv-gui"), b"").unwrap();
        // `dv setup path`'s `~/.local/bin/dv`.
        let link = dir.join("bin").join("dv");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(bundle.join("dv"), &link).unwrap();
        assert_eq!(executable_for(&Env::empty(), link), bundle.join("dv"));
    }

    #[test]
    fn another_appimage_s_variables_are_not_this_dv_s() {
        let (_scratch, dir) = scratch();
        let editor = dir.join("Editor.AppImage");
        std::fs::write(&editor, b"").unwrap();
        let mount = dir.join("mount_editor");
        std::fs::create_dir_all(&mount).unwrap();
        // This dv lies outside the editor's mount: a dv started from its terminal.
        let exe = dir.join("bin").join("dv");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"").unwrap();
        assert_eq!(executable_for(&env(&editor, &mount), exe.clone()), exe);
        assert_eq!(executable_for(&Env::empty(), exe.clone()), exe);
    }

    #[test]
    fn inside_its_own_mount_dv_is_its_appimage() {
        let (_scratch, dir) = scratch();
        let image = dir.join("dv.AppImage");
        std::fs::write(&image, b"").unwrap();
        let mount = dir.join("mount_dv");
        let exe = mount.join("usr").join("bin").join("dv");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"").unwrap();
        let expected = if cfg!(target_os = "linux") {
            image.clone()
        } else {
            exe.clone()
        };
        assert_eq!(executable_for(&env(&image, &mount), exe.clone()), expected);
        // An AppImage that isn't there is no AppImage.
        let gone = dir.join("gone.AppImage");
        assert_eq!(executable_for(&env(&gone, &mount), exe.clone()), exe);
    }
}
