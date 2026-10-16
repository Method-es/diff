//! Child processes that die with this one (docs/PLAN.md §5.2, §10.4, §18.3).
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
//!
//! A process a backend spawns (`Backend::spawn_stdio`, PLAN §18.3) gets a [`Job`] of its own
//! instead: the same kill-on-close limit, so it dies with this process too, and one handle that
//! kills the process and everything it started, which a language server's own children need.

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
pub use windows::Job;

#[cfg(windows)]
mod windows {
    use std::os::windows::io::AsRawHandle as _;
    use std::process::Child;
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };

    /// The job's handle, as an integer so it can live in a static. It is never closed: the OS
    /// closes it when this process ends, which is when its children must end too.
    static JOB: OnceLock<Option<isize>> = OnceLock::new();

    pub(super) fn job() -> Option<HANDLE> {
        let job = JOB.get_or_init(|| match kill_on_close_job() {
            Ok(job) => Some(job as isize),
            Err(error) => {
                log::warn!("{error}; children rely on their stdin closing");
                None
            }
        });
        job.map(|job| job as HANDLE)
    }

    /// A new anonymous job that kills what is in it when its last handle closes.
    fn kill_on_close_job() -> Result<HANDLE, String> {
        // SAFETY: null attributes and a null name create an anonymous job object.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(format!(
                "couldn't create a job object ({})",
                std::io::Error::last_os_error()
            ));
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
            let error = std::io::Error::last_os_error();
            // SAFETY: `job` is the handle just created, closed once here.
            unsafe { CloseHandle(job) };
            return Err(format!("couldn't configure the job object ({error})"));
        }
        Ok(job)
    }

    /// Puts `child` in `job`; whether it is there now.
    fn assign_to(job: HANDLE, child: &Child) -> bool {
        // SAFETY: both handles are live: the caller keeps `job` open, and `child` owns its
        // process handle for the length of the call.
        unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) != 0 }
    }

    pub(super) fn assign(child: &Child) {
        let Some(job) = job() else {
            return;
        };
        if !assign_to(job, child) {
            log::warn!(
                "couldn't tie process {} to dv's job object ({}); it relies on its stdin closing",
                child.id(),
                std::io::Error::last_os_error()
            );
        }
    }

    /// One process and everything it starts (module docs): killed together by
    /// [`Job::terminate`], and when the job is dropped, or this process ends.
    #[derive(Debug)]
    pub struct Job {
        /// The job's handle, as an integer so the job is `Send` and `Sync`; closed on drop.
        handle: isize,
    }

    impl Job {
        /// A job holding `child` alone, `None` when the OS won't make one or put `child` in it
        /// (logged). Children `child` started before it joined stay outside; a language server
        /// starts its own only once it has read its first request.
        pub fn of(child: &Child) -> Option<Self> {
            let handle = match kill_on_close_job() {
                Ok(handle) => handle,
                Err(error) => {
                    log::warn!("{error} for process {}", child.id());
                    return None;
                }
            };
            // From here dropping the job closes the handle.
            let job = Self {
                handle: handle as isize,
            };
            if !assign_to(handle, child) {
                log::warn!(
                    "couldn't put process {} in a job of its own ({})",
                    child.id(),
                    std::io::Error::last_os_error()
                );
                return None;
            }
            Some(job)
        }

        /// Kills every process in the job, exiting with `code`.
        pub fn terminate(&self, code: u32) {
            // SAFETY: the handle is live until this job is dropped.
            if unsafe { TerminateJobObject(self.handle as HANDLE, code) } == 0 {
                log::debug!(
                    "couldn't end a process's job ({})",
                    std::io::Error::last_os_error()
                );
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is this job's own, closed once here; the kill-on-close limit
            // ends whatever is still in it.
            unsafe { CloseHandle(self.handle as HANDLE) };
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::os::windows::io::AsRawHandle as _;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::IsProcessInJob;

    fn pinging() -> std::process::Child {
        Command::new("cmd.exe")
            .args(["/c", "ping -n 30 127.0.0.1 > nul"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    fn ends_within(child: &mut std::process::Child, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }

    /// A process tied to this one is in a job that kills it when this process ends: shown
    /// here by the child seeing itself in a job (a process this test can't end without
    /// ending the test).
    #[test]
    fn a_tied_child_is_in_a_kill_on_close_job() {
        let mut child = pinging();
        super::tie_to_this_process(&child);
        let job = super::windows::job().expect("a job object");
        let mut inside = 0;
        // SAFETY: both handles are live for the call; `inside` is a BOOL to write.
        let asked = unsafe { IsProcessInJob(child.as_raw_handle() as HANDLE, job, &mut inside) };
        let _ = child.kill();
        assert!(ends_within(&mut child, Duration::from_secs(10)));
        assert_ne!(asked, 0, "{}", std::io::Error::last_os_error());
        assert_ne!(inside, 0, "the child is in dv's job");
    }

    #[test]
    fn a_job_of_its_own_ends_its_process_when_terminated_or_dropped() {
        let mut child = pinging();
        let job = super::Job::of(&child).expect("a job");
        job.terminate(7);
        assert!(ends_within(&mut child, Duration::from_secs(10)));
        assert_eq!(child.wait().unwrap().code(), Some(7));

        let mut child = pinging();
        let job = super::Job::of(&child).expect("a job");
        drop(job);
        assert!(ends_within(&mut child, Duration::from_secs(10)));
    }
}
