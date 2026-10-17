//! What this build of dv is (docs/PLAN.md §5.2, §13.2): its version and channel, and build
//! hashes.
//!
//! **Version and channel.** `script/package` builds a release with `DV_CHANNEL=release` and
//! `DV_VERSION=<its version>` set, which this module reads at compile time ([`CHANNEL`],
//! [`VERSION`]); every other build is a development build of the workspace's version.
//! `dv --version` and `dv status --json` say which, and a packaged build alone takes a WSL
//! distro's `~/.local/bin/dv` from another (`crate::install`).
//!
//! **Build hashes:** the lowercase hex sha256 of a dv executable. The app sends the hash of the
//! Linux build it means a distro to run in `hello`; the server confirms it by hashing its own
//! executable, so both ends are one build.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest as _, Sha256};

/// The channel `script/package` built this for (`DV_CHANNEL` at compile time, `release`), or
/// `None` for a development build.
pub const CHANNEL: Option<&str> = match option_env!("DV_CHANNEL") {
    Some(channel) if !channel.is_empty() => Some(channel),
    _ => None,
};

/// This build's version: `DV_VERSION` at compile time (a packaged release's, PLAN §13.2), else
/// the workspace's.
pub const VERSION: &str = match option_env!("DV_VERSION") {
    Some(version) if !version.is_empty() => version,
    _ => env!("CARGO_PKG_VERSION"),
};

/// Whether `script/package` built this ([`CHANNEL`]).
pub const fn is_packaged() -> bool {
    CHANNEL.is_some()
}

/// The channel as `dv status --json` names it: `release` (or what `script/package` set), else
/// `development`.
pub fn channel_name() -> &'static str {
    CHANNEL.unwrap_or("development")
}

/// What `dv --version` prints after `dv `: the version, and `(development build)` unless this
/// is a packaged one.
pub fn version_text() -> &'static str {
    static TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TEXT.get_or_init(|| {
        if is_packaged() {
            VERSION.to_string()
        } else {
            format!("{VERSION} (development build)")
        }
    })
}

/// The lowercase hex sha256 of everything `reader` yields.
pub fn sha256_hex(mut reader: impl Read) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hasher.update(&buffer[..read]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// The build hash of the file at `path`.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    sha256_hex(File::open(path)?)
}

/// The build hash of this process's own executable. On Linux it is read through
/// `/proc/self/exe`, which opens the running binary even after its file was replaced or
/// removed (an install prunes old builds while their servers run, PLAN §5.2), where
/// `current_exe()` would name a path that is gone.
pub fn own_hash() -> io::Result<String> {
    if cfg!(target_os = "linux") {
        return sha256_file(Path::new("/proc/self/exe"));
    }
    sha256_file(&std::env::current_exe()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_lowercase_hex_sha256() {
        assert_eq!(
            sha256_hex(&b""[..]).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(&b"abc"[..]).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // Longer than one read.
        let big = vec![b'x'; 600 * 1024];
        assert_eq!(sha256_hex(&big[..]).unwrap().len(), 64);
    }

    #[test]
    fn this_process_hashes_its_own_executable() {
        let own = own_hash().unwrap();
        let exe = std::env::current_exe().unwrap();
        assert_eq!(own, sha256_file(&exe).unwrap());
    }
}
