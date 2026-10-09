//! Child processes that die with this one (docs/PLAN.md §5.2, §10.4).
//!
//! A server's `wsl.exe` ends when the app closes its stdin, and the CLI's run inside a distro
//! when the command does. But a process that is killed (a tool's timeout, Task Manager, a
//! `taskkill` without `/T`) closes nothing in order, and a child left behind would run on:
//! the CLI's `wsl.exe` and the distro's `dv review wait` behind it until their round ends.
//! So on Windows every such child joins a job object this process holds open and never
//! closes, created with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: when this process ends, however
//! it ends, the OS closes the job and kills what is in it, and killing `wsl.exe` ends what it
//! runs inside the distro. Ctrl-C reaches the child through the console it shares, as before.
//! Elsewhere there is nothing to do: the Linux build starts no `wsl.exe`.

use std::process::Child;

/// Puts `child` in this process's kill-on-close job (module docs), best effort: when that
/// fails it is logged and the child relies on its stdin closing.
pub fn tie_to_this_process(child: &Child) {
    #[cfg(windows)]
    windows::assign(child);
    #[cfg(not(windows))]
    let _ = child;
}

#[cfg(windows)]
mod windows {
    use std::os::windows::io::AsRawHandle as _;
    use std::process::Child;
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    /// The job's handle, as an integer so it can live in a static. It is never closed: the OS
    /// closes it when this process ends, which is when its children must end too.
    static JOB: OnceLock<Option<isize>> = OnceLock::new();

    pub(super) fn job() -> Option<HANDLE> {
        let job = JOB.get_or_init(|| {
            // SAFETY: null attributes and a null name create an anonymous job object.
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() {
                log::warn!(
                    "couldn't create a job object ({}); children rely on their stdin closing",
                    std::io::Error::last_os_error()
                );
                return None;
            }
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: `job` is a fresh job handle and `info` is the structure this information
            // class takes, passed with its exact size.
            let set = unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&raw const info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if set == 0 {
                log::warn!(
                    "couldn't configure the job object ({}); children rely on their stdin closing",
                    std::io::Error::last_os_error()
                );
                return None;
            }
            Some(job as isize)
        });
        job.map(|job| job as HANDLE)
    }

    pub(super) fn assign(child: &Child) {
        let Some(job) = job() else {
            return;
        };
        // SAFETY: both handles are live: the job's is never closed, and `child` owns its
        // process handle for the length of the call.
        let assigned = unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) };
        if assigned == 0 {
            log::warn!(
                "couldn't tie process {} to dv's job object ({}); it relies on its stdin closing",
                child.id(),
                std::io::Error::last_os_error()
            );
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    /// A process tied to this one is in a job that kills it when this process ends: shown
    /// here by the child seeing itself in a job (a process this test can't end without
    /// ending the test).
    #[test]
    fn a_tied_child_is_in_a_kill_on_close_job() {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::System::JobObjects::IsProcessInJob;

        let mut child = Command::new("cmd.exe")
            .args(["/c", "ping -n 30 127.0.0.1 > nul"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        super::tie_to_this_process(&child);
        let job = super::windows::job().expect("a job object");
        let mut inside = 0;
        // SAFETY: both handles are live for the call; `inside` is a BOOL to write.
        let asked = unsafe { IsProcessInJob(child.as_raw_handle() as HANDLE, job, &mut inside) };
        let _ = child.kill();
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "the child never ended");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_ne!(asked, 0, "{}", std::io::Error::last_os_error());
        assert_ne!(inside, 0, "the child is in dv's job");
    }
}
