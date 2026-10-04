//! Reporting an error that stops dv from starting (the data dir can't be made, the window can't
//! open).
//!
//! It is always logged. A release build on Windows has no console, so there it is also shown in
//! a native message box before the process exits non-zero; never under automation, where a
//! modal box would hang the driver.

/// Logs `error` and, if `show_dialog`, shows it in a blocking message box (Windows only).
pub fn report(error: &anyhow::Error, show_dialog: bool) {
    log::error!("dv couldn't start: {error:#}");
    if show_dialog {
        show(&message(error));
    }
}

/// The message box text: what failed, then the chain of causes on separate lines.
fn message(error: &anyhow::Error) -> String {
    let mut text = String::from("dv couldn't start.\n");
    for cause in error.chain() {
        text.push('\n');
        text.push_str(&cause.to_string());
    }
    text
}

#[cfg(windows)]
fn show(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW,
    };

    let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain([0]).collect() };
    let (text, caption) = (wide(message), wide("dv"));
    // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call; a null owner makes
    // the box top-level.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

#[cfg(not(windows))]
fn show(_message: &str) {
    // The log line is the report: macOS and Linux builds are started from a terminal.
}

#[cfg(test)]
mod tests {
    use anyhow::Context as _;

    use super::*;

    #[test]
    fn the_message_lists_every_cause() {
        let error = Err::<(), _>(std::io::Error::other("access denied"))
            .context("creating the data directory D:\\data")
            .unwrap_err();
        assert_eq!(
            message(&error),
            "dv couldn't start.\n\
             \ncreating the data directory D:\\data\
             \naccess denied"
        );
    }
}
