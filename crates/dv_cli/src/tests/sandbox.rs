//! What the in-process real-WSL tests share (`setup_wsl`, `reexec`): a scratch home of the
//! test's own under `DV_WSL_HOME` in the distro `DV_TEST_WSL_DISTRO` names, and this app's
//! Linux builds. Without `DV_TEST_WSL_DISTRO` those tests skip; with it, `DV_WSL_HOME` must be a
//! folder under the distro's `/tmp`, or they fail rather than touch a real home (CLAUDE.md).

use std::path::PathBuf;
use std::time::Duration;

use dv_backend::wsl::Shell;

/// The distro and a scratch home of the test's own, removed when dropped.
pub(crate) struct Sandbox {
    pub distro: String,
    pub home: String,
}

impl Sandbox {
    /// `<DV_WSL_HOME>/<name>-<this process>`, made empty, when `DV_TEST_WSL_DISTRO` asks for
    /// the real WSL; `None` (the test skips) otherwise.
    pub fn get(name: &str) -> Option<Self> {
        let distro = std::env::var("DV_TEST_WSL_DISTRO")
            .ok()
            .filter(|distro| !distro.trim().is_empty())?;
        let root = std::env::var("DV_WSL_HOME").unwrap_or_default();
        assert!(
            root.starts_with("/tmp/") && !root.contains(".."),
            "DV_TEST_WSL_DISTRO is set, so DV_WSL_HOME must be a scratch folder under \
             /tmp in {distro}, not {root:?}"
        );
        let home = format!(
            "{}/{name}-{}",
            root.trim_end_matches('/'),
            std::process::id()
        );
        let sandbox = Self { distro, home };
        sandbox.sh(r#"rm -rf -- "$1" && mkdir -p -- "$1""#);
        Some(sandbox)
    }

    /// Runs `script` in the distro with the home as `$HOME` and as `$1`; it must succeed.
    pub fn sh(&self, script: &str) {
        let shell = Shell::wsl(self.distro.clone(), Some(self.home.clone())).unwrap();
        let output = shell
            .run(script, &[&self.home], Duration::from_secs(60))
            .unwrap();
        assert!(output.status.success(), "{script}: {output:?}");
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        self.sh(r#"rm -rf -- "$1""#);
    }
}

/// `target/debug`, where `script/build-linux` puts the Linux build (this test's executable is
/// in `target/debug/deps`), unless `DV_LINUX_DIR` names another folder.
pub(crate) fn builds_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("DV_LINUX_DIR") {
        return PathBuf::from(dir);
    }
    let exe = std::env::current_exe().unwrap();
    exe.parent().unwrap().parent().unwrap().to_path_buf()
}
