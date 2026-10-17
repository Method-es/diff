//! The Windows `Path` behind a seam (docs/PLAN.md §12.2, CLAUDE.md): the user's
//! (`HKCU\Environment`, value `Path`), which `dv setup path` changes and then broadcasts
//! `WM_SETTINGCHANGE` for, so programs started afterwards see it, and the machine's
//! (`HKLM\…\Session Manager\Environment`), which dv only reads: Windows puts it first on every
//! process's `PATH`. [`UserPath::Registry`] is the real values; [`UserPath::File`] is text files
//! that stand in for them (`DV_USER_PATH_FILE`, and `DV_MACHINE_PATH_FILE` for the machine's,
//! empty without it): every test and every automation run uses them, and no test reads or writes
//! the real registry. In this crate's own tests nothing names the registry at all: without the
//! variable the seam refuses ([`UserPath::Refused`]).
//!
//! A value is a `;`-separated list; an entry matches a folder by its expansion
//! (`%USERPROFILE%\bin`, through the caller's variables), without regard to case, quotes or a
//! trailing `\`. [`resolve`] says which folder's `dv.exe` a terminal runs, the machine's
//! entries first; adding puts the folder before the first user entry that holds another
//! `dv.exe` (so it isn't shadowed there), else at the end; removing takes out exactly the
//! entries that match it, leaving no empty one.

use std::path::{Path, PathBuf};

/// The variable naming the file that stands in for the user `Path`.
pub const USER_PATH_FILE_VAR: &str = "DV_USER_PATH_FILE";

/// The variable naming the file that stands in for the machine's `Path` (with the user's).
pub const MACHINE_PATH_FILE_VAR: &str = "DV_MACHINE_PATH_FILE";

/// A variable's value by its name, for `%NAME%` in an entry.
pub type Vars<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Where the `Path` values are read from and written to (module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserPath {
    /// The registry (Windows).
    Registry,
    /// UTF-8 text files holding the values; none there is an unset `Path`.
    File {
        user: PathBuf,
        machine: Option<PathBuf>,
    },
    /// Neither may be used: this crate's tests without [`USER_PATH_FILE_VAR`].
    Refused,
}

impl UserPath {
    /// The seam the variables name (module docs): `user` (`DV_USER_PATH_FILE`) and `machine`
    /// (`DV_MACHINE_PATH_FILE`) when the first is set, else the registry (refused in this
    /// crate's tests).
    pub fn from_vars(user: Option<String>, machine: Option<String>) -> Self {
        let set = |value: Option<String>| {
            value
                .filter(|value| !value.trim().is_empty())
                .map(|value| PathBuf::from(value.trim()))
        };
        match set(user) {
            Some(user) => Self::File {
                user,
                machine: set(machine),
            },
            None if cfg!(test) => Self::Refused,
            None => Self::Registry,
        }
    }

    /// The stand-in files `user` and no machine `Path`.
    pub fn file(user: PathBuf) -> Self {
        Self::File {
            user,
            machine: None,
        }
    }

    /// The user `Path`, `None` when it isn't set.
    pub fn read(&self) -> Result<Option<String>, String> {
        match self {
            Self::Registry => registry::read(false).map(|value| value.map(|(text, _)| text)),
            Self::File { user, .. } => read_file(user),
            Self::Refused => Err(refused()),
        }
    }

    /// The machine's `Path`, `None` when it isn't set (or has no stand-in).
    pub fn read_machine(&self) -> Result<Option<String>, String> {
        match self {
            Self::Registry => registry::read(true).map(|value| value.map(|(text, _)| text)),
            Self::File { machine, .. } => machine.as_deref().map_or(Ok(None), read_file),
            Self::Refused => Err(refused()),
        }
    }

    /// Replaces the user `Path` with `value`; the registry's keeps its type (`REG_EXPAND_SZ`
    /// unless it was a plain `REG_SZ`), and running programs are told it changed.
    pub fn write(&self, value: &str) -> Result<(), String> {
        match self {
            Self::Registry => registry::write(value),
            Self::File { user, .. } => {
                if let Some(dir) = user.parent()
                    && !dir.as_os_str().is_empty()
                {
                    std::fs::create_dir_all(dir)
                        .map_err(|error| format!("creating {}: {error}", dir.display()))?;
                }
                std::fs::write(user, format!("{value}\n"))
                    .map_err(|error| format!("writing {}: {error}", user.display()))
            }
            Self::Refused => Err(refused()),
        }
    }

    /// How messages name the user `Path`.
    pub fn describe(&self) -> String {
        match self {
            Self::Registry => "your user Path".to_string(),
            Self::File { user, .. } => format!("the user Path ({})", user.display()),
            Self::Refused => "the user Path".to_string(),
        }
    }
}

fn read_file(file: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(file) {
        Ok(text) => Ok(Some(text.trim_end_matches(['\r', '\n']).to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("reading {}: {error}", file.display())),
    }
}

fn refused() -> String {
    format!("this build reads the user Path only through {USER_PATH_FILE_VAR}")
}

/// Which `dv.exe` a terminal runs (module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// The folder's own comes first.
    Ours,
    /// Another folder's comes first: in the machine's `Path` (`machine`) or the user's. Ours is
    /// on the `Path` after it, or not at all (`listed`).
    Shadowed {
        by: PathBuf,
        machine: bool,
        listed: bool,
    },
    /// Neither the folder nor any other `dv.exe` is on the `Path`.
    Absent,
}

/// Which `dv.exe` the `Path` `machine` then `user` runs, for this executable's folder `dir`;
/// `has_dv` says whether a folder holds a `dv.exe`.
pub fn resolve(
    machine: &str,
    user: &str,
    dir: &Path,
    vars: Vars<'_>,
    has_dv: &dyn Fn(&Path) -> bool,
) -> Resolved {
    let listed = contains(machine, dir, vars) || contains(user, dir, vars);
    let all = entries(machine)
        .map(|entry| (entry, true))
        .chain(entries(user).map(|entry| (entry, false)));
    for (entry, in_machine) in all {
        if same_dir(entry, dir, vars) {
            return Resolved::Ours;
        }
        let folder = PathBuf::from(expand(entry.trim_matches('"'), vars));
        if has_dv(&folder) {
            return Resolved::Shadowed {
                by: folder,
                machine: in_machine,
                listed,
            };
        }
    }
    Resolved::Absent
}

/// Whether `value` lists `dir` (module docs).
pub fn contains(value: &str, dir: &Path, vars: Vars<'_>) -> bool {
    entries(value).any(|entry| same_dir(entry, dir, vars))
}

/// `value` with `dir` before its first entry whose folder holds another `dv.exe` (`has_dv`),
/// else at its end; any other entry for `dir` taken out. `None` when it is already where it
/// would go.
pub fn with_added(
    value: Option<&str>,
    dir: &Path,
    vars: Vars<'_>,
    has_dv: &dyn Fn(&Path) -> bool,
) -> Option<String> {
    let value = value.unwrap_or("");
    let listed: Vec<&str> = entries(value).collect();
    let first_ours = listed.iter().position(|entry| same_dir(entry, dir, vars));
    let first_other = listed.iter().position(|entry| {
        !same_dir(entry, dir, vars) && has_dv(&PathBuf::from(expand(entry.trim_matches('"'), vars)))
    });
    match (first_ours, first_other) {
        (Some(ours), Some(other)) if ours < other => return None,
        (Some(_), None) => return None,
        _ => {}
    }
    let mut kept: Vec<String> = listed
        .iter()
        .filter(|entry| !same_dir(entry, dir, vars))
        .map(|entry| entry.to_string())
        .collect();
    let at = first_other
        .map(|other| {
            listed[..other]
                .iter()
                .filter(|entry| !same_dir(entry, dir, vars))
                .count()
        })
        .unwrap_or(kept.len());
    kept.insert(at, dir.display().to_string());
    Some(kept.join(";"))
}

/// `value` without the entries that match `dir` (and no empty ones), or `None` when it has
/// none.
pub fn with_removed(value: &str, dir: &Path, vars: Vars<'_>) -> Option<String> {
    if !contains(value, dir, vars) {
        return None;
    }
    let kept: Vec<&str> = entries(value)
        .filter(|entry| !same_dir(entry, dir, vars))
        .collect();
    Some(kept.join(";"))
}

/// The entries of `value` that name something.
fn entries(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
}

/// Whether the `Path` entry `entry` names `dir`.
fn same_dir(entry: &str, dir: &Path, vars: Vars<'_>) -> bool {
    let normal = |text: &str| {
        text.trim()
            .trim_matches('"')
            .trim_end_matches(['\\', '/'])
            .replace('/', "\\")
            .to_lowercase()
    };
    normal(&expand(entry, vars)) == normal(&dir.display().to_string())
}

/// `text` with each `%NAME%` replaced by `vars`' value for it (one it doesn't know stays as
/// written, as Windows leaves it).
fn expand(text: &str, vars: Vars<'_>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match vars(name).filter(|_| !name.is_empty()) {
                    Some(value) => {
                        out.push_str(&rest[..start]);
                        out.push_str(&value);
                    }
                    None => out.push_str(&rest[..start + end + 2]),
                }
                rest = &after[end + 1..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

#[cfg(windows)]
mod registry {
    //! The `Path` values themselves.

    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_EXPAND_SZ,
        REG_SZ, REG_VALUE_TYPE, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegCloseKey,
        RegGetValueW, RegOpenKeyExW, RegSetValueExW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
    };

    /// The machine's environment key, under `HKEY_LOCAL_MACHINE`.
    const MACHINE_KEY: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The machine's (`machine`) or the user's value and its type, unexpanded; `None` when
    /// there is none.
    pub(super) fn read(machine: bool) -> Result<Option<(String, REG_VALUE_TYPE)>, String> {
        let (root, key, whose) = if machine {
            (HKEY_LOCAL_MACHINE, wide(MACHINE_KEY), "the system Path")
        } else {
            (HKEY_CURRENT_USER, wide("Environment"), "the user Path")
        };
        let name = wide("Path");
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
        let mut kind: REG_VALUE_TYPE = 0;
        let mut size: u32 = 0;
        // SAFETY: the strings are NUL-terminated; a null buffer asks for the size.
        let status = unsafe {
            RegGetValueW(
                root,
                key.as_ptr(),
                name.as_ptr(),
                flags,
                &mut kind,
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(format!("reading {whose} failed (error {status})"));
        }
        let mut buffer = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut size = (buffer.len() * 2) as u32;
        // SAFETY: `buffer` holds `size` bytes.
        let status = unsafe {
            RegGetValueW(
                root,
                key.as_ptr(),
                name.as_ptr(),
                flags,
                &mut kind,
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!("reading {whose} failed (error {status})"));
        }
        let len = buffer
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(buffer.len());
        Ok(Some((String::from_utf16_lossy(&buffer[..len]), kind)))
    }

    /// Sets the user's value (module docs), then tells running programs the environment
    /// changed.
    pub(super) fn write(value: &str) -> Result<(), String> {
        let kind = match read(false)? {
            Some((_, REG_SZ)) => REG_SZ,
            _ => REG_EXPAND_SZ,
        };
        let subkey = wide("Environment");
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: `subkey` is NUL-terminated and `key` receives the handle.
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                &mut key,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!("opening HKCU\\Environment failed (error {status})"));
        }
        let name = wide("Path");
        let data = wide(value);
        // SAFETY: `data` is `data.len() * 2` bytes, NUL included, as REG_*SZ wants.
        let status = unsafe {
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                kind,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        // SAFETY: `key` was opened above.
        unsafe { RegCloseKey(key) };
        if status != ERROR_SUCCESS {
            return Err(format!("writing the user Path failed (error {status})"));
        }
        let environment = wide("Environment");
        let mut result = 0usize;
        // SAFETY: a broadcast with a NUL-terminated string, bounded by its timeout.
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                environment.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                5000,
                &mut result,
            )
        };
        Ok(())
    }
}

#[cfg(not(windows))]
mod registry {
    //! There is no registry here.

    pub(super) fn read(_machine: bool) -> Result<Option<(String, u32)>, String> {
        Err("the Path in the registry is Windows's".to_string())
    }

    pub(super) fn write(_value: &str) -> Result<(), String> {
        Err("the Path in the registry is Windows's".to_string())
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn none(_: &str) -> Option<String> {
        None
    }

    fn no_dv(_: &Path) -> bool {
        false
    }

    #[test]
    fn a_folder_matches_however_the_entry_spells_it() {
        let dir = Path::new(r"C:\Users\k\AppData\Local\dv");
        for value in [
            r"C:\Windows;C:\Users\k\AppData\Local\dv",
            r"c:\users\K\appdata\local\DV\;C:\Windows",
            r#""C:\Users\k\AppData\Local\dv""#,
            r"C:/Users/k/AppData/Local/dv",
        ] {
            assert!(contains(value, dir, &none), "{value}");
        }
        assert!(!contains(r"C:\Users\k\AppData\Local\dv2", dir, &none));
        assert!(!contains("", dir, &none));
    }

    #[test]
    fn adding_appends_once_and_removing_takes_out_exactly_that_folder() {
        let dir = Path::new(r"D:\dv");
        assert_eq!(
            with_added(None, dir, &none, &no_dv).as_deref(),
            Some(r"D:\dv")
        );
        assert_eq!(
            with_added(Some(""), dir, &none, &no_dv).as_deref(),
            Some(r"D:\dv")
        );
        assert_eq!(
            with_added(Some(r"C:\a;C:\b;"), dir, &none, &no_dv).as_deref(),
            Some(r"C:\a;C:\b;D:\dv")
        );
        assert_eq!(with_added(Some(r"C:\a;d:\DV\"), dir, &none, &no_dv), None);
        assert_eq!(
            with_removed(r"C:\a;D:\dv;C:\b;D:\dv\", dir, &none).as_deref(),
            Some(r"C:\a;C:\b")
        );
        // No empty entry is left behind.
        assert_eq!(
            with_removed(r"C:\a;;D:\dv;;C:\b;", dir, &none).as_deref(),
            Some(r"C:\a;C:\b")
        );
        assert_eq!(with_removed(r"D:\dv", dir, &none).as_deref(), Some(""));
        assert_eq!(with_removed(r"C:\a;D:\dv2", dir, &none), None);
    }

    #[test]
    fn adding_goes_before_another_dv_and_moves_ours_ahead_of_it() {
        let dir = Path::new(r"D:\dv");
        let old = |folder: &Path| folder == Path::new(r"D:\old");
        assert_eq!(
            with_added(Some(r"C:\a;D:\old;C:\b"), dir, &none, &old).as_deref(),
            Some(r"C:\a;D:\dv;D:\old;C:\b")
        );
        // Listed after the other one: moved ahead of it.
        assert_eq!(
            with_added(Some(r"C:\a;D:\old;D:\dv"), dir, &none, &old).as_deref(),
            Some(r"C:\a;D:\dv;D:\old")
        );
        // Ahead already: nothing to do.
        assert_eq!(with_added(Some(r"D:\dv;D:\old"), dir, &none, &old), None);
    }

    #[test]
    fn the_dv_a_terminal_runs_is_the_first_on_the_machine_then_the_user_path() {
        let dir = Path::new(r"D:\dv");
        let old = |folder: &Path| folder == Path::new(r"D:\old");
        assert_eq!(resolve("", r"C:\a;D:\dv", dir, &none, &old), Resolved::Ours);
        assert_eq!(resolve("", r"C:\a", dir, &none, &old), Resolved::Absent);
        assert_eq!(
            resolve("", r"D:\old;D:\dv", dir, &none, &old),
            Resolved::Shadowed {
                by: PathBuf::from(r"D:\old"),
                machine: false,
                listed: true,
            }
        );
        assert_eq!(
            resolve(r"C:\Windows;D:\old", r"D:\dv", dir, &none, &old),
            Resolved::Shadowed {
                by: PathBuf::from(r"D:\old"),
                machine: true,
                listed: true,
            }
        );
        assert_eq!(
            resolve("", r"D:\old", dir, &none, &old),
            Resolved::Shadowed {
                by: PathBuf::from(r"D:\old"),
                machine: false,
                listed: false,
            }
        );
    }

    #[test]
    fn variables_in_an_entry_expand_through_the_lookup_and_unknown_ones_stay() {
        let vars = |name: &str| (name == "TOOLS").then(|| r"E:\tools".to_string());
        assert_eq!(expand(r"%TOOLS%\dv", &vars), r"E:\tools\dv");
        assert_eq!(
            expand(r"%DV_NO_SUCH_VAR%\dv", &vars),
            r"%DV_NO_SUCH_VAR%\dv"
        );
        assert_eq!(expand("50%", &vars), "50%");
        assert!(contains(r"%TOOLS%\dv", Path::new(r"E:\tools\dv"), &vars));
    }

    #[test]
    fn this_crates_tests_never_reach_the_registry() {
        assert_eq!(UserPath::from_vars(None, None), UserPath::Refused);
        assert_eq!(
            UserPath::from_vars(Some(" ".into()), None),
            UserPath::Refused
        );
        assert!(UserPath::Refused.read().is_err());
        assert!(UserPath::Refused.read_machine().is_err());
        assert!(UserPath::Refused.write("x").is_err());
        let file = tempfile::tempdir().unwrap();
        let seam = UserPath::from_vars(
            Some(file.path().join("Path.txt").display().to_string()),
            None,
        );
        assert_eq!(seam.read(), Ok(None));
        assert_eq!(seam.read_machine(), Ok(None));
        seam.write(r"C:\a;D:\dv").unwrap();
        assert_eq!(seam.read(), Ok(Some(r"C:\a;D:\dv".to_string())));
    }
}
