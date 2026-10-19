//! The other dv processes running from an installed copy (docs/PLAN.md D9): applying an update
//! stops every process whose executable is under the install, so Restart to update counts
//! them first and asks ([`others_in`]). Each process's executable is read from the system: on
//! Windows a Toolhelp snapshot of the processes and each one's image path
//! (`QueryFullProcessImageNameW`, which works across users' processes of this session without
//! more rights than querying); on macOS `proc_listallpids` and `proc_pidpath`, which read this
//! user's processes. One that can't be read isn't counted.

#[cfg(windows)]
use std::os::windows::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};

#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
#[cfg(windows)]
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};

/// How many of `processes` (`(pid, image)`) run an executable under `dir`, not counting
/// `own` (this process): the comparison ignores case, as Windows' and macOS's default file
/// systems do.
pub fn count_under(dir: &Path, processes: &[(u32, PathBuf)], own: u32) -> usize {
    let separator = std::path::MAIN_SEPARATOR;
    let dir = dir.to_string_lossy().to_lowercase();
    let dir = dir.trim_end_matches(separator);
    processes
        .iter()
        .filter(|(pid, image)| {
            *pid != own && {
                let image = image.to_string_lossy().to_lowercase();
                image
                    .strip_prefix(dir)
                    .is_some_and(|rest| rest.starts_with(separator))
            }
        })
        .count()
}

/// The other processes running an executable under `dir` (module docs).
pub fn others_in(dir: &Path) -> usize {
    count_under(dir, &running(), std::process::id())
}

/// Every process of this machine whose image path can be read, with its id.
#[cfg(windows)]
fn running() -> Vec<(u32, PathBuf)> {
    let mut found = Vec::new();
    // SAFETY: the snapshot handle is checked before use and closed once; the entry is
    // zeroed with its size set, as Process32FirstW requires.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
            return found;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more {
            if let Some(image) = image_of(entry.th32ProcessID) {
                found.push((entry.th32ProcessID, image));
            }
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    found
}

/// The executable process `pid` runs, when it can be read.
#[cfg(windows)]
fn image_of(pid: u32) -> Option<PathBuf> {
    // SAFETY: the process handle is checked and closed once; the buffer and its length are
    // passed together, and only what the call wrote is read.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buffer = vec![0u16; 1024];
        let mut len = buffer.len() as u32;
        let ok =
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut len)
                != 0;
        CloseHandle(process);
        ok.then(|| PathBuf::from(std::ffi::OsString::from_wide(&buffer[..len as usize])))
    }
}

/// Every process whose executable path can be read (this user's), with its id.
#[cfg(target_os = "macos")]
fn running() -> Vec<(u32, PathBuf)> {
    use std::os::unix::ffi::OsStringExt as _;

    // The count can grow between the two calls: room for more, and a short read is fine.
    // SAFETY: a null buffer of size 0 asks only for the count.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    let Ok(count) = usize::try_from(count) else {
        return Vec::new();
    };
    let mut pids = vec![0 as libc::pid_t; count + 64];
    let bytes = libc::c_int::try_from(pids.len() * std::mem::size_of::<libc::pid_t>())
        .unwrap_or(libc::c_int::MAX);
    // SAFETY: the buffer holds `bytes` bytes of pids; the call writes at most that many and
    // returns how many pids it wrote.
    let written = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    let Ok(written) = usize::try_from(written) else {
        return Vec::new();
    };
    pids.truncate(written.min(pids.len()));
    pids.into_iter()
        .filter(|&pid| pid > 0)
        .filter_map(|pid| {
            let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
            // SAFETY: the buffer and its length are passed together; the call returns the
            // path's length in bytes, and only those are read.
            let len =
                unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
            let len = usize::try_from(len).ok().filter(|&len| len > 0)?;
            buffer.truncate(len);
            let pid = u32::try_from(pid).ok()?;
            Some((pid, PathBuf::from(std::ffi::OsString::from_vec(buffer))))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn the_processes_under_the_install_count_but_this_one() {
        let current = Path::new(r"C:\Users\me\AppData\Local\dv\current");
        let processes = vec![
            (
                1,
                PathBuf::from(r"C:\Users\me\AppData\Local\dv\current\dv-gui.exe"),
            ),
            (
                2,
                PathBuf::from(r"c:\users\ME\appdata\local\DV\current\dv.exe"),
            ),
            (
                3,
                PathBuf::from(r"C:\Users\me\AppData\Local\dv\current-old\dv.exe"),
            ),
            (4, PathBuf::from(r"C:\Users\me\AppData\Local\dv\Update.exe")),
            (5, PathBuf::from(r"D:\Software\diff\target\release\dv.exe")),
        ];
        assert_eq!(count_under(current, &processes, 99), 2);
        // This process isn't counted.
        assert_eq!(count_under(current, &processes, 1), 1);
        let with_slash = Path::new(r"C:\Users\me\AppData\Local\dv\current\");
        assert_eq!(count_under(with_slash, &processes, 99), 2);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_processes_in_the_bundle_count_but_this_one() {
        let bundle = Path::new("/Applications/dv.app/Contents/MacOS");
        let processes = vec![
            (
                1,
                PathBuf::from("/Applications/dv.app/Contents/MacOS/dv-gui"),
            ),
            (2, PathBuf::from("/applications/DV.app/Contents/MacOS/dv")),
            (
                3,
                PathBuf::from("/Applications/dv.app/Contents/MacOS-old/dv"),
            ),
            (
                4,
                PathBuf::from("/Applications/dv.app/Contents/Resources/x"),
            ),
            (5, PathBuf::from("/Users/me/code/dv/target/debug/dv")),
        ];
        assert_eq!(count_under(bundle, &processes, 99), 2);
        assert_eq!(count_under(bundle, &processes, 1), 1);
        let with_slash = Path::new("/Applications/dv.app/Contents/MacOS/");
        assert_eq!(count_under(with_slash, &processes, 99), 2);
    }

    #[test]
    fn this_process_is_found_and_not_counted() {
        let exe = std::env::current_exe().unwrap();
        let dir = exe.parent().unwrap();
        let processes = running();
        assert!(
            processes.iter().any(|(pid, _)| *pid == std::process::id()),
            "the snapshot lists this process"
        );
        // Other test processes may run from the same folder; this one never counts.
        assert_eq!(
            count_under(dir, &processes, std::process::id()),
            count_under(dir, &processes, 0) - 1
        );
    }
}
