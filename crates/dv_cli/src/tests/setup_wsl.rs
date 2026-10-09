//! `dv setup wsl` (PLAN §5.2): refused off Windows; on Windows, what is unavailable says so
//! with exit 4, a missing Linux build and a bad sandbox home before any `wsl.exe` starts; and,
//! only when `DV_TEST_WSL_DISTRO` names a distro, the install itself, in a scratch home under
//! `DV_WSL_HOME` (a folder under the distro's `/tmp`), never the distro user's own.

use super::*;

#[cfg(not(windows))]
#[test]
fn it_runs_on_windows_only() {
    use pretty_assertions::assert_eq;

    let fixture = Fixture::new();
    let error = fixture
        .run(&["setup", "wsl", "Ubuntu", "--json"])
        .error(Exit::Usage, "unsupported");
    assert_eq!(
        error["message"],
        "`dv setup wsl` runs on Windows; inside a distro this dv is the Linux build already"
    );
}

#[cfg(windows)]
mod windows {
    use std::path::Path;

    use pretty_assertions::assert_eq;

    use super::*;
    use crate::tests::sandbox::{Sandbox, builds_dir};

    /// A folder holding a stand-in Linux build for x86_64.
    fn stand_in_builds() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("dv-linux-x86_64"), "a stand-in").unwrap();
        dir
    }

    fn with_env(vars: &[(&str, &str)]) -> Fixture {
        let mut fixture = Fixture::new();
        for (name, value) in vars {
            fixture.env.insert(name.to_string(), value.to_string());
        }
        fixture
    }

    #[test]
    fn without_a_linux_build_it_is_exit_4_naming_the_file() {
        let empty = tempfile::tempdir().unwrap();
        let fixture = with_env(&[("DV_LINUX_DIR", empty.path().to_str().unwrap())]);
        let error = fixture
            .run(&["setup", "wsl", "Ubuntu", "--json"])
            .error(Exit::Unavailable, "backend_unavailable");
        assert_eq!(
            error["message"],
            format!(
                "no Linux build of dv for x86_64: {} is missing (script/build-linux makes it)",
                empty.path().join("dv-linux-x86_64").display()
            )
        );
    }

    #[test]
    fn a_sandbox_home_that_isnt_a_posix_path_is_refused_before_wsl_starts() {
        // What Git Bash makes of `/tmp/x` in a variable passed to a Windows program. A distro
        // that can't exist: should the guard ever let this through, no real distro gets the
        // stand-in build.
        let builds = stand_in_builds();
        let fixture = with_env(&[
            ("DV_LINUX_DIR", builds.path().to_str().unwrap()),
            ("DV_WSL_HOME", "C:/Users/x/AppData/Local/Temp/x"),
        ]);
        let error = fixture
            .run(&["setup", "wsl", "dv-no-such-distro", "--json"])
            .error(Exit::Unavailable, "backend_unavailable");
        assert_eq!(
            error["message"],
            "DV_WSL_HOME must be an absolute path inside the distro, not \
             \"C:/Users/x/AppData/Local/Temp/x\""
        );
    }

    #[test]
    fn a_distro_that_isnt_there_is_exit_4() {
        // `wsl.exe --list` boots nothing.
        let builds = stand_in_builds();
        let fixture = with_env(&[("DV_LINUX_DIR", builds.path().to_str().unwrap())]);
        let error = fixture
            .run(&["setup", "wsl", "dv-no-such-distro", "--json"])
            .error(Exit::Unavailable, "backend_unavailable");
        let message = error["message"].as_str().unwrap();
        assert!(
            message.starts_with("there is no WSL distro named dv-no-such-distro (")
                || message.starts_with("WSL isn't installed here"),
            "{message}"
        );
    }

    #[test]
    fn it_installs_into_the_distro_once_asked_and_then_has_nothing_to_do() {
        let Some(sandbox) = Sandbox::get("setup-wsl") else {
            eprintln!("skipped: DV_TEST_WSL_DISTRO isn't set");
            return;
        };
        let builds = builds_dir();
        let build = dv_backend::install::LinuxBuild::at(&builds, dv_backend::wsl::Arch::X86_64)
            .unwrap()
            .expect("script/build-linux made the Linux build");
        let fixture = with_env(&[
            ("DV_LINUX_DIR", builds.to_str().unwrap()),
            ("DV_WSL_HOME", &sandbox.home),
        ]);
        let distro = sandbox.distro.as_str();
        let path = format!("{}/.local/share/dv/bin/dv-{}", sandbox.home, build.hash);

        // Not asked: the plan, and exit 2.
        let refused = fixture.run_stdin(&["setup", "wsl", distro], "");
        assert_eq!(refused.exit, Exit::Usage, "{}", refused.stderr);
        assert_eq!(
            refused.lines()[0],
            format!(
                "{distro}  install  {path} (x86_64, {:.1} MB)",
                build.len as f64 / (1024.0 * 1024.0)
            )
        );
        assert_eq!(
            refused.lines()[1],
            format!("{distro}  link  {}/.local/bin/dv to it", sandbox.home)
        );
        assert_eq!(
            refused.stderr,
            format!(
                "dv: dv setup wsl installs dv into the home directory in {distro}: pass --yes to \
                 go ahead\n"
            )
        );

        let json = fixture
            .run(&["setup", "wsl", distro, "--yes", "--json"])
            .ok_json();
        assert_eq!(
            json,
            serde_json::json!({
                "schema": 1,
                "distro": json["distro"],
                "arch": "x86_64",
                "build": build.hash,
                "installed": true,
                "linked": true,
                "path": path,
            })
        );
        assert!(
            json["distro"]
                .as_str()
                .unwrap()
                .eq_ignore_ascii_case(distro)
        );

        // Installed and intact: nothing to ask, nothing to do.
        let again = fixture.run(&["setup", "wsl", distro, "--json"]).ok_json();
        assert_eq!(again["installed"], false);
        let text = fixture.run(&["setup", "wsl", distro]);
        assert_eq!(text.exit, Exit::Ok);
        assert_eq!(
            text.stdout,
            format!("{}  up to date  {path}\n", json["distro"].as_str().unwrap())
        );
        assert!(Path::new(&builds).is_dir());
    }

    #[test]
    fn a_link_it_cant_make_is_a_warning_and_the_install_succeeds() {
        let Some(sandbox) = Sandbox::get("setup-wsl-ro") else {
            eprintln!("skipped: DV_TEST_WSL_DISTRO isn't set");
            return;
        };
        sandbox.sh(r#"mkdir -p "$1/.local/bin" && chmod 555 "$1/.local/bin""#);
        let builds = builds_dir();
        let fixture = with_env(&[
            ("DV_LINUX_DIR", builds.to_str().unwrap()),
            ("DV_WSL_HOME", &sandbox.home),
        ]);
        let ran = fixture.run(&["setup", "wsl", &sandbox.distro, "--yes", "--json"]);
        sandbox.sh(r#"chmod 755 "$1/.local/bin""#);
        let json = ran.ok_json();
        assert_eq!(json["installed"], true);
        // Root writes into the folder whatever its mode: then there is nothing to warn about.
        if json["linked"] == false {
            let link = format!("{}/.local/bin/dv", sandbox.home);
            let [refused, fix] = &ran.stderr.lines().collect::<Vec<_>>()[..] else {
                panic!("{}", ran.stderr);
            };
            assert!(
                refused.starts_with(&format!("dv: warning: couldn't point {link} at this dv (")),
                "{refused}"
            );
            assert_eq!(
                *fix,
                format!(
                    "dv: warning: to put this dv on the PATH in {}, make {}/.local/bin a folder \
                     you can write to, then run `dv setup wsl {}` again",
                    json["distro"].as_str().unwrap(),
                    sandbox.home,
                    json["distro"].as_str().unwrap()
                )
            );
        } else {
            assert_eq!(ran.stderr, "");
        }
    }
}
