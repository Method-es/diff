//! What the platform can do for the window material (PLAN D8, DESIGN §3), and applying a material
//! and a light/dark appearance to a window.
//!
//! [`MaterialSupport::probe`] asks the OS which materials it can show right now. On Windows,
//! Mica needs build 22621 or later (gpui sets it through `DWMWA_SYSTEMBACKDROP_TYPE`, which
//! older builds lack, and does nothing there), acrylic needs Windows 11 (build 22000; Windows 10
//! gets Opaque, PLAN D8), and both need "Transparency effects" on
//! (`HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\EnableTransparency`).
//! Linux shows Opaque, and so does macOS: gpui's `Blurred` there (an `NSVisualEffectView`
//! with its tint and saturation layers stripped) leaves the window clear on macOS 27, the
//! wallpaper sharp behind dv's translucent tints, with no blur at all (seen on the owner's Mac,
//! active and inactive alike), which reads worse than Opaque; vibrancy comes back once gpui's
//! blur does (PLAN D8).

use gpui::{Window, WindowBackgroundAppearance};

use crate::theme::{Appearance, Material};

/// The first Windows 11 build.
pub const WINDOWS_11_BUILD: u32 = 22000;
/// The first build with `DWMWA_SYSTEMBACKDROP_TYPE`, which gpui's Mica uses.
pub const MICA_BUILD: u32 = 22621;

/// Which translucent materials the platform can show. Opaque always works.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialSupport {
    pub mica: bool,
    pub acrylic: bool,
}

impl MaterialSupport {
    /// Opaque only: Linux, macOS, Windows 10, transparency effects off, tests.
    pub const OPAQUE_ONLY: Self = Self {
        mica: false,
        acrylic: false,
    };
    /// Every material.
    pub const ALL: Self = Self {
        mica: true,
        acrylic: true,
    };

    /// What this machine supports now.
    pub fn probe() -> Self {
        #[cfg(windows)]
        {
            Self::for_windows(windows::build_number(), windows::transparency_enabled())
        }
        #[cfg(not(windows))]
        {
            Self::OPAQUE_ONLY
        }
    }

    /// Windows' support for its `build` (unknown: none) with transparency effects on or off.
    pub fn for_windows(build: Option<u32>, transparency: bool) -> Self {
        let build = build.unwrap_or(0);
        Self {
            mica: transparency && build >= MICA_BUILD,
            acrylic: transparency && build >= WINDOWS_11_BUILD,
        }
    }

    pub fn supports(self, material: Material) -> bool {
        match material {
            Material::Mica => self.mica,
            Material::Acrylic => self.acrylic,
            Material::Opaque => true,
        }
    }

    /// The materials that can be shown, in [`Material::ALL`] order.
    pub fn available(self) -> Vec<Material> {
        Material::ALL
            .into_iter()
            .filter(|material| self.supports(*material))
            .collect()
    }

    /// The material on screen for the setting `wanted`: itself when supported, otherwise
    /// Opaque, which looks finished in its own right (DESIGN §3).
    pub fn resolve(self, wanted: Material) -> Material {
        if self.supports(wanted) {
            wanted
        } else {
            Material::Opaque
        }
    }
}

/// Switches `window` to `material`. gpui's `set_background_appearance` only adds its own effect
/// (Mica sets the DWM backdrop, Blurred an accent policy), so the previous material is cleared
/// first; otherwise going from Acrylic to Mica would leave both on.
pub fn apply_window_material(window: &Window, material: Material) {
    // Opaque switches the accent policy (Acrylic) off.
    window.set_background_appearance(WindowBackgroundAppearance::Opaque);
    #[cfg(windows)]
    if material != Material::Mica {
        windows::clear_system_backdrop(window);
    }
    if material != Material::Opaque {
        window.set_background_appearance(material.window_background());
    }
}

/// Tells the window manager whether `window` is dark or light, which tints its material (a light
/// Mica under a dark one looks muddy). gpui sets this from the OS's own setting at creation and
/// whenever the OS switches; dv's mode can differ from the OS's, so the shell sets it after both.
pub fn apply_window_appearance(window: &Window, appearance: Appearance) {
    #[cfg(windows)]
    windows::set_dark_mode(window, appearance == Appearance::Dark);
    #[cfg(not(windows))]
    let _ = (window, appearance);
}

#[cfg(windows)]
mod windows {
    use gpui::Window;
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, HWND};
    use windows_sys::Win32::Graphics::Dwm::{
        DWMSBT_NONE, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
        DwmSetWindowAttribute,
    };
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;

    /// The running Windows' build number. `RtlGetVersion` reports the real one, unlike
    /// `GetVersionEx` without an application manifest.
    pub fn build_number() -> Option<u32> {
        let mut info = OSVERSIONINFOW {
            dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32,
            dwMajorVersion: 0,
            dwMinorVersion: 0,
            dwBuildNumber: 0,
            dwPlatformId: 0,
            szCSDVersion: [0; 128],
        };
        // SAFETY: `info` is a valid OSVERSIONINFOW with its size field set, as the call requires.
        let status = unsafe { windows_sys::Wdk::System::SystemServices::RtlGetVersion(&mut info) };
        (status >= 0).then_some(info.dwBuildNumber)
    }

    /// Settings › Personalization › Colors › Transparency effects. A missing value means on,
    /// as Windows treats it.
    pub fn transparency_enabled() -> bool {
        let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain([0]).collect() };
        let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
        let value = wide("EnableTransparency");
        let mut data: u32 = 1;
        let mut size = size_of::<u32>() as u32;
        // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call; `data` and `size`
        // describe a 4-byte buffer, which RRF_RT_REG_DWORD fills.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut data as *mut u32).cast(),
                &mut size,
            )
        };
        status != ERROR_SUCCESS || data != 0
    }

    /// Turns the DWM system backdrop (Mica) off for `window`. A no-op where the attribute
    /// doesn't exist.
    pub fn clear_system_backdrop(window: &Window) {
        if build_number().is_none_or(|build| build < super::MICA_BUILD) {
            return;
        }
        let Some(hwnd) = hwnd(window) else {
            return;
        };
        let backdrop = DWMSBT_NONE;
        // SAFETY: `hwnd` is this live window's handle and the value is the 4-byte
        // DWM_SYSTEMBACKDROP_TYPE the attribute takes.
        let result = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE as u32,
                (&backdrop as *const i32).cast(),
                size_of_val(&backdrop) as u32,
            )
        };
        if result < 0 {
            log::warn!("couldn't clear the window's system backdrop: HRESULT {result:#x}");
        }
    }

    /// `DWMWA_USE_IMMERSIVE_DARK_MODE` for `window`.
    pub fn set_dark_mode(window: &Window, dark: bool) {
        let Some(hwnd) = hwnd(window) else {
            return;
        };
        let value: windows_sys::core::BOOL = dark.into();
        // SAFETY: `hwnd` is this live window's handle and the value is the 4-byte BOOL the
        // attribute takes.
        let result = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
                (&value as *const i32).cast(),
                size_of_val(&value) as u32,
            )
        };
        if result < 0 {
            log::warn!("couldn't set the window's dark mode: HRESULT {result:#x}");
        }
    }

    fn hwnd(window: &Window) -> Option<HWND> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};

        // Fully qualified: `Window` also has an inherent `window_handle()` (gpui's own handle).
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_support_follows_the_build_and_the_transparency_switch() {
        // Windows 10.
        assert_eq!(
            MaterialSupport::for_windows(Some(19045), true),
            MaterialSupport::OPAQUE_ONLY
        );
        // Windows 11 before DWMWA_SYSTEMBACKDROP_TYPE: acrylic only.
        assert_eq!(
            MaterialSupport::for_windows(Some(22000), true),
            MaterialSupport {
                mica: false,
                acrylic: true
            }
        );
        assert_eq!(
            MaterialSupport::for_windows(Some(26200), true),
            MaterialSupport::ALL
        );
        // Transparency effects off: opaque, whatever the build.
        assert_eq!(
            MaterialSupport::for_windows(Some(26200), false),
            MaterialSupport::OPAQUE_ONLY
        );
        assert_eq!(
            MaterialSupport::for_windows(None, true),
            MaterialSupport::OPAQUE_ONLY
        );
    }

    #[test]
    fn an_unsupported_material_resolves_to_opaque() {
        let acrylic_only = MaterialSupport {
            mica: false,
            acrylic: true,
        };
        assert_eq!(acrylic_only.resolve(Material::Mica), Material::Opaque);
        assert_eq!(acrylic_only.resolve(Material::Acrylic), Material::Acrylic);
        assert_eq!(
            MaterialSupport::OPAQUE_ONLY.resolve(Material::Acrylic),
            Material::Opaque
        );
        assert_eq!(MaterialSupport::ALL.resolve(Material::Mica), Material::Mica);
        assert_eq!(
            acrylic_only.available(),
            [Material::Acrylic, Material::Opaque]
        );
        assert_eq!(MaterialSupport::OPAQUE_ONLY.available(), [Material::Opaque]);
    }

    #[cfg(windows)]
    #[test]
    fn the_probe_reads_this_machine() {
        // Whatever the machine says, it is consistent with its own build and switch.
        let build = windows::build_number();
        assert!(build.is_some_and(|build| build >= 10240), "{build:?}");
        let support = MaterialSupport::probe();
        assert_eq!(
            support,
            MaterialSupport::for_windows(build, windows::transparency_enabled())
        );
    }
}
