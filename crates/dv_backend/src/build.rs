//! Build hashes (docs/PLAN.md §5.2): the lowercase hex sha256 of a dv executable. The app sends
//! the hash of the Linux build it means a distro to run in `hello`; the server confirms it by
//! hashing its own executable, so both ends are one build.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest as _, Sha256};

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
