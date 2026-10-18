//! What a panic leaves behind in the app (PLAN §13.1). The app runs detached, with no console
//! or stderr, and a panic inside one of the window's callbacks can't unwind out of it, so the
//! process aborts and the window just vanishes. Before that, the hook appends the panic (dv's
//! version, the thread, the message, where it happened and a backtrace) to `crash.log` in the
//! data dir, then runs the default hook.

use std::backtrace::Backtrace;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The file under the data dir a panic is written to.
pub const FILE_NAME: &str = "crash.log";

/// Installs the hook for this process, writing to `data_dir`'s [`FILE_NAME`].
pub fn install(data_dir: &Path) {
    let path = data_dir.join(FILE_NAME);
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write(&path, info);
        default(info);
    }));
}

/// Appends one report for `info`, best effort: a panic hook has nowhere to report its own
/// failure.
fn write(path: &PathBuf, info: &PanicHookInfo<'_>) {
    let report = report(info, &Backtrace::force_capture().to_string());
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(report.as_bytes());
    }
}

/// One report: a header line, the message with its place, then the backtrace.
fn report(info: &PanicHookInfo<'_>, backtrace: &str) -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default();
    let thread = std::thread::current();
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".to_string());
    let place = info
        .location()
        .map(|location| format!("{}:{}", location.file(), location.line()))
        .unwrap_or_else(|| "an unknown place".to_string());
    format!(
        "--- dv {} panicked (unix time {seconds}) on thread {}\n{message}\nat {place}\n{backtrace}\n",
        dv_cli::version_text(),
        thread.name().unwrap_or("unnamed"),
    )
}

#[cfg(test)]
mod tests {
    use std::panic::{self, AssertUnwindSafe};
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn a_panic_is_written_to_the_data_dir_with_its_message_and_place() {
        // The hook is process-wide: install it, panic once, put the previous one back.
        static HOOK: Mutex<()> = Mutex::new(());
        let _guard = HOOK.lock().unwrap_or_else(|error| error.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let previous = panic::take_hook();
        install(dir.path());
        let caught = panic::catch_unwind(AssertUnwindSafe(|| {
            panic!("cannot read Shell while it is already being updated")
        }));
        let _ = panic::take_hook();
        panic::set_hook(previous);
        assert!(caught.is_err());
        let log = std::fs::read_to_string(dir.path().join(FILE_NAME)).unwrap();
        assert!(log.starts_with("--- dv "), "{log}");
        assert!(
            log.contains("cannot read Shell while it is already being updated"),
            "{log}"
        );
        assert!(log.contains("crash.rs:"), "{log}");
    }
}
