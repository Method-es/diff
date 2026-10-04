//! `resize` on Windows: set the client area to an exact size, even one
//! taller or wider than the display.
//!
//! gpui's `Window::resize` calls `SetWindowPos`, whose `WM_WINDOWPOSCHANGING`
//! lets `DefWindowProc` clamp the window to the default maximum tracking
//! size, roughly the display. A portrait reader layout such as 1080x1920 on
//! a 1440-logical-pixel-high display therefore never arrives. Passing
//! `SWP_NOSENDCHANGING` skips that clamp; `WM_SIZE` still reaches gpui, so
//! the viewport, `state` and screenshots all follow. The window's minimum
//! size (the app's, else the system's) is still enforced, as it is for a
//! user dragging the border.

use anyhow::{Result, bail};
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowRect, IsWindow, MINMAXINFO, SM_CXMINTRACK, SM_CYMINTRACK,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSENDCHANGING, SWP_NOZORDER, SendMessageW, SetWindowPos,
    WM_GETMINMAXINFO,
};

/// Resize `hwnd` so its client area is `width` x `height` physical pixels.
/// `scale` (physical per logical pixel) only phrases errors in logical
/// pixels, the unit scenarios use.
///
/// Must not run inside a gpui window update: `SetWindowPos` delivers
/// `WM_SIZE` to gpui's window procedure synchronously, and that needs the
/// app. gpui's own `resize` defers its call to the executor for the same
/// reason.
pub(crate) fn set_client_size(hwnd: isize, width: i32, height: i32, scale: f32) -> Result<()> {
    let hwnd = hwnd as HWND;
    // SAFETY: `IsWindow` accepts any value.
    if unsafe { IsWindow(hwnd) } == 0 {
        bail!("the window no longer exists");
    }
    // Sizes are in the calling (UI) thread's DPI context, the one gpui's own
    // `resize` computes in: physical pixels for a per-monitor-aware app.
    let (mut window_rect, mut client_rect) = (RECT::default(), RECT::default());
    // SAFETY: a live window and valid out-pointers to locals.
    if unsafe { GetWindowRect(hwnd, &mut window_rect) } == 0
        || unsafe { GetClientRect(hwnd, &mut client_rect) } == 0
    {
        bail!("reading the window's rects failed");
    }
    let (outer_w, outer_h) = outer_size(width, height, &window_rect, &client_rect);

    let (min_w, min_h) = minimum_outer_size(hwnd);
    if outer_w < min_w || outer_h < min_h {
        let logical = |physical: i32| (physical as f32 / scale).round();
        let (border_w, border_h) = (outer_w - width, outer_h - height);
        bail!(
            "{}x{} is below the window's minimum size of {}x{}",
            logical(width),
            logical(height),
            logical(min_w - border_w),
            logical(min_h - border_h)
        );
    }

    // SAFETY: a live window; the flags keep its position, z-order and
    // activation and skip only the WM_WINDOWPOSCHANGING clamp.
    let ok = unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            outer_w,
            outer_h,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSENDCHANGING,
        )
    };
    if ok == 0 {
        bail!("SetWindowPos failed: {}", std::io::Error::last_os_error());
    }
    Ok(())
}

/// The outer (frame) size that gives a `width` x `height` client area: the
/// frame is whatever the window rect has beyond the client rect, which also
/// covers gpui's custom title bars (their client area spans the frame).
fn outer_size(width: i32, height: i32, window_rect: &RECT, client_rect: &RECT) -> (i32, i32) {
    let border_w = (window_rect.right - window_rect.left) - (client_rect.right - client_rect.left);
    let border_h = (window_rect.bottom - window_rect.top) - (client_rect.bottom - client_rect.top);
    (width + border_w, height + border_h)
}

/// The smallest outer size the window accepts: what it answers to
/// `WM_GETMINMAXINFO`, pre-filled with the system minimum the way Windows
/// does before asking.
fn minimum_outer_size(hwnd: HWND) -> (i32, i32) {
    // SAFETY: a live window; both calls only read metrics.
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    let mut info = MINMAXINFO {
        // SAFETY: plain metric lookups.
        ptMinTrackSize: unsafe {
            POINT {
                x: GetSystemMetricsForDpi(SM_CXMINTRACK, dpi),
                y: GetSystemMetricsForDpi(SM_CYMINTRACK, dpi),
            }
        },
        ..Default::default()
    };
    // SAFETY: WM_GETMINMAXINFO's lParam is a MINMAXINFO the handler may
    // update; `info` outlives the synchronous call.
    unsafe {
        SendMessageW(
            hwnd,
            WM_GETMINMAXINFO,
            0,
            &mut info as *mut MINMAXINFO as LPARAM,
        );
    }
    (info.ptMinTrackSize.x, info.ptMinTrackSize.y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetSystemMetrics, SM_CYVIRTUALSCREEN, WS_OVERLAPPEDWINDOW,
    };

    /// A hidden, ordinary top-level window of the system "STATIC" class.
    struct TestWindow(HWND);

    impl TestWindow {
        fn new() -> Self {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            // SAFETY: a system class, no parent, not shown.
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    std::ptr::null(),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    400,
                    300,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            };
            assert!(!hwnd.is_null(), "CreateWindowExW failed");
            Self(hwnd)
        }

        fn client_size(&self) -> (i32, i32) {
            let mut rect = RECT::default();
            // SAFETY: a live window and a valid out-pointer.
            assert_ne!(unsafe { GetClientRect(self.0, &mut rect) }, 0);
            (rect.right - rect.left, rect.bottom - rect.top)
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            // SAFETY: created by this test on this thread.
            unsafe {
                DestroyWindow(self.0);
            }
        }
    }

    #[test]
    fn outer_size_adds_the_frame() {
        let window = RECT {
            left: 100,
            top: 50,
            right: 100 + 1216,
            bottom: 50 + 839,
        };
        let client = RECT {
            left: 0,
            top: 0,
            right: 1200,
            bottom: 800,
        };
        assert_eq!(outer_size(1080, 1920, &window, &client), (1096, 1959));
        let frameless = client;
        assert_eq!(outer_size(10, 20, &frameless, &client), (10, 20));
    }

    #[test]
    fn windows_can_grow_taller_than_the_screen() {
        let window = TestWindow::new();
        // SAFETY: a plain metric lookup.
        let screen_h = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
        let target = (640, screen_h * 2 + 100);
        set_client_size(window.0 as isize, target.0, target.1, 1.0).unwrap();
        assert_eq!(window.client_size(), target);
        set_client_size(window.0 as isize, 500, 400, 1.0).unwrap();
        assert_eq!(window.client_size(), (500, 400));
    }

    #[test]
    fn the_minimum_size_still_applies() {
        let window = TestWindow::new();
        let err = set_client_size(window.0 as isize, 2, 2, 2.0).unwrap_err();
        let message = err.to_string();
        assert!(
            message.starts_with("1x1 is below the window's minimum size of "),
            "{message}"
        );
    }

    #[test]
    fn dead_windows_are_reported() {
        let err = set_client_size(0, 100, 100, 1.0).unwrap_err();
        assert!(err.to_string().contains("no longer exists"), "{err}");
    }
}
