//! Window screenshots for the `screenshot` command.
//!
//! The platform handle is read on the main thread (it borrows the gpui
//! window); the pixel read-back and PNG encoding then run on a background
//! thread so the UI thread keeps pumping messages, which `PrintWindow`
//! relies on. Windows reads back through Win32 (`capture_windows`), Linux
//! through X11 (`capture_x11`); elsewhere the command reports it's missing.

use std::path::Path;

use anyhow::{Context as _, Result};
use gpui::Window;
use serde::{Deserialize, Serialize};

/// Where a screenshot's pixels come from (`screenshot`'s `source`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CaptureSource {
    /// The window's own composed frame: `PrintWindow(PW_RENDERFULLCONTENT)`
    /// on Windows, a `GetImage` of the window on X11. It works while other
    /// windows cover the window, so background runs use it. On Windows it
    /// includes Mica but not Acrylic's blur-behind, so translucent surfaces
    /// over Acrylic come out composited over black (`capture_windows`).
    #[default]
    Window,
    /// The window's client rect copied from the desktop as it is shown,
    /// blur-behind included. Only meaningful for a window in front (a
    /// foreground run); Windows only. The capture fails, rather than copying
    /// another window's pixels, when the window isn't the foreground window
    /// or something covers its client area.
    Screen,
}

/// What the capture thread needs to find the window again.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
pub(crate) struct CaptureTarget {
    #[cfg(windows)]
    hwnd: isize,
    #[cfg(target_os = "linux")]
    x11_window: u32,
}

/// Physical pixel size of a written screenshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
pub(crate) struct Captured {
    pub width: u32,
    pub height: u32,
}

#[cfg(windows)]
pub(crate) fn capture_target(window: &Window) -> Result<CaptureTarget> {
    Ok(CaptureTarget {
        hwnd: hwnd(window)?,
    })
}

/// The window's Win32 handle, as an integer so it can cross threads.
#[cfg(windows)]
pub(crate) fn hwnd(window: &Window) -> Result<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    // Fully qualified: `Window` also has an inherent `window_handle()` that
    // returns gpui's own handle.
    let handle = HasWindowHandle::window_handle(window)
        .map_err(|err| anyhow::anyhow!("window has no native handle: {err}"))?;
    match handle.as_raw() {
        RawWindowHandle::Win32(handle) => Ok(handle.hwnd.get()),
        other => anyhow::bail!("unexpected native window handle {other:?}"),
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn capture_target(window: &Window) -> Result<CaptureTarget> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = HasWindowHandle::window_handle(window)
        .map_err(|err| anyhow::anyhow!("window has no native handle: {err}"))?;
    let x11_window = match handle.as_raw() {
        RawWindowHandle::Xcb(handle) => handle.window.get(),
        RawWindowHandle::Xlib(handle) => u32::try_from(handle.window)
            .context("X11 window id does not fit the protocol's 32 bits")?,
        RawWindowHandle::Wayland(_) => {
            anyhow::bail!("window screenshots need X11; unset WAYLAND_DISPLAY to run under X")
        }
        other => anyhow::bail!("unexpected native window handle {other:?}"),
    };
    Ok(CaptureTarget { x11_window })
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) fn capture_target(_window: &Window) -> Result<CaptureTarget> {
    anyhow::bail!("window screenshots are only implemented on Windows and Linux (X11)")
}

/// Capture the window's client area and write it to `path` as a PNG.
#[cfg(windows)]
pub(crate) fn capture_png(
    target: CaptureTarget,
    path: &Path,
    source: CaptureSource,
) -> Result<Captured> {
    let image = crate::capture_windows::capture_client_area(target.hwnd, source)?;
    write_png(path, image.width, image.height, &image.rgba)?;
    Ok(Captured {
        width: image.width,
        height: image.height,
    })
}

/// Capture the window and write it to `path` as a PNG.
#[cfg(target_os = "linux")]
pub(crate) fn capture_png(
    target: CaptureTarget,
    path: &Path,
    source: CaptureSource,
) -> Result<Captured> {
    if source == CaptureSource::Screen {
        anyhow::bail!("screen captures are only implemented on Windows");
    }
    let image = crate::capture_x11::capture_window(target.x11_window)?;
    write_png(path, image.width, image.height, &image.rgba)?;
    Ok(Captured {
        width: image.width,
        height: image.height,
    })
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) fn capture_png(
    _target: CaptureTarget,
    _path: &Path,
    _source: CaptureSource,
) -> Result<Captured> {
    anyhow::bail!("window screenshots are only implemented on Windows and Linux (X11)")
}

/// Encode tightly packed 8-bit RGBA pixels as a PNG, creating parent
/// directories as needed.
#[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
pub(crate) fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let file =
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder
        .write_header()
        .with_context(|| format!("writing PNG header to {}", path.display()))?;
    writer
        .write_image_data(rgba)
        .with_context(|| format!("writing PNG data to {}", path.display()))?;
    writer
        .finish()
        .with_context(|| format!("finishing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_png_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/shot.png");
        let rgba: Vec<u8> = (0..(3 * 2))
            .flat_map(|i| [i as u8 * 10, 20, 30, 255])
            .collect();
        write_png(&path, 3, 2, &rgba).unwrap();

        let decoder =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (3, 2));
        assert_eq!(&buf[..info.buffer_size()], rgba.as_slice());
    }
}
