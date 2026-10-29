//! The app's icon in the Dock and the app switcher on macOS when dv runs outside an app bundle
//! (a development build, `cargo run`, `target/release/dv`; PLAN §13.1). AppKit takes an app's
//! icon from its bundle's `CFBundleIconFile`, so a bare executable shows the generic one; this
//! hands AppKit the bundled icon itself (`-[NSApplication setApplicationIconImage:]`), the same
//! `resources/macos/dv.icns` the packaged app's bundle carries. A packaged build runs from its
//! bundle and is left alone.

use std::ffi::{CStr, c_char, c_void};

use objc2::runtime::AnyObject;
use objc2::{class, msg_send};

/// The icon the packaged app's bundle carries (`script/package`).
const ICNS: &[u8] = include_bytes!("../resources/macos/dv.icns");

/// Gives the app the bundled icon when it isn't running from an app bundle (module docs).
/// Nothing happens when AppKit can't read it.
pub fn set_unless_bundled() {
    // SAFETY: plain AppKit and Foundation messages on the main thread, where gpui runs the app;
    // every object returned is checked for nil before use, and `ICNS` outlives the copy
    // `dataWithBytes:length:` makes of it.
    unsafe {
        let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
        if bundle.is_null() {
            return;
        }
        let path: *mut AnyObject = msg_send![bundle, bundlePath];
        if !path.is_null() {
            let utf8: *const c_char = msg_send![path, UTF8String];
            if !utf8.is_null() && is_app_bundle(&CStr::from_ptr(utf8).to_string_lossy()) {
                return;
            }
        }
        let data: *mut AnyObject = msg_send![
            class!(NSData),
            dataWithBytes: ICNS.as_ptr().cast::<c_void>(),
            length: ICNS.len()
        ];
        if data.is_null() {
            return;
        }
        let image: *mut AnyObject = msg_send![class!(NSImage), alloc];
        let image: *mut AnyObject = msg_send![image, initWithData: data];
        if image.is_null() {
            log::warn!("AppKit couldn't read the app icon");
            return;
        }
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: image];
    }
}

/// Whether `bundle_path` (`-[NSBundle bundlePath]` of the main bundle) is an app bundle: for a
/// bare executable it is the executable's folder.
fn is_app_bundle(bundle_path: &str) -> bool {
    bundle_path.trim_end_matches('/').ends_with(".app")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_app_bundle_keeps_its_own_icon() {
        assert!(is_app_bundle("/Applications/dv.app"));
        assert!(is_app_bundle("/Applications/dv.app/"));
        assert!(!is_app_bundle("/Users/kyle/code/dv/target/release"));
        assert!(!is_app_bundle("/Users/kyle/apps"));
    }

    #[test]
    fn the_embedded_icon_is_an_icns_file() {
        assert_eq!(&ICNS[..4], b"icns");
    }
}
