//! The file that starts this dv again: what `~/.local/bin/dv` links to (`dv setup path`, PLAN
//! §12.2) and what the detached app runs (`dv open`, PLAN §13.1).
//!
//! That is this executable, except in a Linux AppImage (PLAN §13.3): there the executable lies
//! in the AppImage's mount (`$APPDIR`), which goes when the process that mounted it exits, so it
//! is the AppImage itself (`$APPIMAGE`). An AppImage's runtime sets both for the program it
//! starts, and every child inherits them: a terminal in another AppImage (an editor's) hands
//! that one's `$APPIMAGE` to a `dv` run there. So `$APPIMAGE` counts only while this executable
//! lies inside `$APPDIR`, both compared canonical.

use std::path::{Path, PathBuf};

use crate::Env;

/// The file that starts this dv again (module docs), reading `$APPIMAGE` and `$APPDIR` from
/// `env`.
pub fn this_executable(env: &Env) -> std::io::Result<PathBuf> {
    Ok(executable_for(env, std::env::current_exe()?))
}

/// [`this_executable`] for the executable `exe`.
pub fn executable_for(env: &Env, exe: PathBuf) -> PathBuf {
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn env(image: &Path, appdir: &Path) -> Env {
        Env::Vars(BTreeMap::from([
            ("APPIMAGE".to_string(), image.display().to_string()),
            ("APPDIR".to_string(), appdir.display().to_string()),
        ]))
    }

    #[test]
    fn another_appimage_s_variables_are_not_this_dv_s() {
        let dir = tempfile::tempdir().unwrap();
        let editor = dir.path().join("Editor.AppImage");
        std::fs::write(&editor, b"").unwrap();
        let mount = dir.path().join("mount_editor");
        std::fs::create_dir_all(&mount).unwrap();
        // This dv lies outside the editor's mount: a dv started from its terminal.
        let exe = dir.path().join("bin").join("dv");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"").unwrap();
        assert_eq!(executable_for(&env(&editor, &mount), exe.clone()), exe);
        assert_eq!(executable_for(&Env::empty(), exe.clone()), exe);
    }

    #[test]
    fn inside_its_own_mount_dv_is_its_appimage() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("dv.AppImage");
        std::fs::write(&image, b"").unwrap();
        let mount = dir.path().join("mount_dv");
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
        let gone = dir.path().join("gone.AppImage");
        assert_eq!(executable_for(&env(&gone, &mount), exe.clone()), exe);
    }
}
