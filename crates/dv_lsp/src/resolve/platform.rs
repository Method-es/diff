//! The machine a server runs on, as a resolver picks a native package for it (docs/PLAN.md
//! §18.4): its OS and its CPUs, in the order to try them.
//!
//! dv itself may run emulated (an x64 build on ARM64 Windows, or under Rosetta on a Mac), so
//! this machine's CPU is asked of the OS, and dv's own build is the fallback: the native
//! package first, then the one dv's build could run beside it.

/// The OS and CPUs of the machine a backend runs on, in Rust's spelling
/// (`std::env::consts::OS` and `ARCH`): this machine's for a local repository, a WSL server's
/// `hello` for one in a distro.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Platform {
    pub os: String,
    /// Never empty: the native CPU first.
    archs: Vec<String>,
}

impl Platform {
    pub fn new(os: impl Into<String>, arch: impl Into<String>) -> Self {
        Self {
            os: os.into(),
            archs: vec![arch.into()],
        }
    }

    /// Also tries `arch`'s packages, after the ones before it.
    pub fn with_fallback_arch(mut self, arch: impl Into<String>) -> Self {
        let arch = arch.into();
        if !self.archs.contains(&arch) {
            self.archs.push(arch);
        }
        self
    }

    /// The machine dv runs on: its native CPU, then dv's own build's when they differ.
    pub fn this_machine() -> Self {
        let own = std::env::consts::ARCH;
        Self::new(std::env::consts::OS, native_arch().unwrap_or(own)).with_fallback_arch(own)
    }

    /// The CPU to name: the native one.
    pub fn arch(&self) -> &str {
        &self.archs[0]
    }

    /// Every CPU whose packages may run here, the native one first.
    pub fn archs(&self) -> &[String] {
        &self.archs
    }

    pub fn is_windows(&self) -> bool {
        self.os == "windows"
    }

    /// Node's name for the OS (`process.platform`): `win32`, `linux`, `darwin`.
    pub fn node_os(&self) -> Option<&'static str> {
        Some(match self.os.as_str() {
            "windows" => "win32",
            "linux" => "linux",
            "macos" => "darwin",
            "freebsd" => "freebsd",
            _ => return None,
        })
    }

    /// Node's name for a CPU (`process.arch`): `x64`, `arm64`.
    pub fn node_arch(arch: &str) -> Option<&'static str> {
        Some(match arch {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            "x86" => "ia32",
            "arm" => "arm",
            _ => return None,
        })
    }
}

/// This machine's own CPU when the OS says, which differs from dv's build when dv is emulated.
#[cfg(windows)]
fn native_arch() -> Option<&'static str> {
    use windows_sys::Win32::System::SystemInformation::IMAGE_FILE_MACHINE;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, IsWow64Process2};

    let mut process: IMAGE_FILE_MACHINE = 0;
    let mut native: IMAGE_FILE_MACHINE = 0;
    // SAFETY: the pseudo-handle of this process and two valid out-pointers.
    let ok = unsafe { IsWow64Process2(GetCurrentProcess(), &mut process, &mut native) };
    if ok == 0 {
        return None;
    }
    match native {
        0x8664 => Some("x86_64"),
        0xAA64 => Some("aarch64"),
        0x014C => Some("x86"),
        _ => None,
    }
}

/// Under Rosetta, `sysctl.proc_translated` is 1 and the machine is ARM64.
#[cfg(target_os = "macos")]
fn native_arch() -> Option<&'static str> {
    let mut translated: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: a NUL-terminated name and an out-buffer of the size given.
    let ok = unsafe {
        libc::sysctlbyname(
            c"sysctl.proc_translated".as_ptr(),
            (&mut translated as *mut libc::c_int).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (ok == 0 && translated == 1).then_some("aarch64")
}

/// The kernel's machine name (`uname -m`).
#[cfg(all(unix, not(target_os = "macos")))]
fn native_arch() -> Option<&'static str> {
    // SAFETY: `uname` fills the zeroed struct it is given.
    let mut name: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut name) } != 0 {
        return None;
    }
    // SAFETY: `machine` is NUL-terminated by `uname`.
    let machine = unsafe { std::ffi::CStr::from_ptr(name.machine.as_ptr()) };
    match machine.to_bytes() {
        b"x86_64" | b"amd64" => Some("x86_64"),
        b"aarch64" | b"arm64" => Some("aarch64"),
        _ => None,
    }
}

#[cfg(not(any(windows, unix)))]
fn native_arch() -> Option<&'static str> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_native_cpu_comes_first() {
        let platform = Platform::new("windows", "aarch64").with_fallback_arch("x86_64");
        assert_eq!(platform.arch(), "aarch64");
        assert_eq!(platform.archs(), ["aarch64", "x86_64"]);
        assert_eq!(
            Platform::new("linux", "x86_64")
                .with_fallback_arch("x86_64")
                .archs(),
            ["x86_64"]
        );
        let here = Platform::this_machine();
        assert_eq!(here.os, std::env::consts::OS);
        assert!(here.archs().contains(&std::env::consts::ARCH.to_string()));
        assert_eq!(Platform::node_arch("aarch64"), Some("arm64"));
        assert_eq!(Platform::new("macos", "x86_64").node_os(), Some("darwin"));
    }
}
