//! Reporting an error that stops dv from starting (the data dir can't be made, the window can't
//! open): `main` reports what fails before the app starts, `app` what fails inside it.
//!
//! It is always logged. The app usually runs detached, with no console or stderr to log to
//! (PLAN §13.1), so on Windows it is also shown in a native message box before the process
//! exits non-zero; never under automation, where a modal box would hang the driver.

/// Logs `error` and, if `show_dialog`, shows it in a blocking message box (Windows only).
pub fn report(error: &anyhow::Error, show_dialog: bool) {
    log::error!("dv couldn't start: {error:#}");
    if show_dialog {
        show_message(&message(error));
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

/// Shows `message` in a blocking error box (Windows only: elsewhere the log line is the report).
#[cfg(windows)]
pub fn show_message(message: &str) {
    show_box(
        message,
        windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
    );
}

/// Shows `text` in a blocking information box: `dv-gui.exe --help` and `--version`, which have
/// no console (Windows only; elsewhere it is printed).
#[cfg(windows)]
pub fn show_info(text: &str) {
    show_box(
        text,
        windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION,
    );
}

/// Prints `text`: the console this binary has off Windows.
#[cfg(not(windows))]
pub fn show_info(text: &str) {
    println!("{text}");
}

/// A blocking message box of `message` with the `icon` style.
#[cfg(windows)]
fn show_box(message: &str, icon: windows_sys::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_OK, MB_SETFOREGROUND, MessageBoxW};

    let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain([0]).collect() };
    let (text, caption) = (wide(message), wide("dv"));
    // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call; a null owner makes
    // the box top-level.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | icon | MB_SETFOREGROUND,
        );
    }
}

/// Shows `message` in a blocking error box (Windows only: elsewhere the log line is the report).
#[cfg(not(windows))]
pub fn show_message(message: &str) {
    log::error!("{message}");
    // The log line is the report; the detached app's stderr goes nowhere, so `dv --no-detach`
    // shows it in the terminal.
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
