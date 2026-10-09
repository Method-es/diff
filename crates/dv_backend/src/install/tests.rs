//! The builds' lookup and hashes everywhere; on Linux the install, prune and probe scripts run
//! by this machine's `/bin/sh` (`Shell::local`) in a scratch home. The real distro is
//! `tests/wsl.rs`'.

use pretty_assertions::assert_eq;

use super::*;

#[test]
fn the_builds_are_found_through_dv_linux_dir_else_beside_this_executable() {
    assert_eq!(
        linux_dir(Some(r" D:\dv\builds ".into())),
        PathBuf::from(r"D:\dv\builds")
    );
    let beside = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    assert_eq!(linux_dir(None), beside);
    assert_eq!(linux_dir(Some("  ".into())), beside);
}

#[test]
fn builds_are_hashed_once_while_their_files_stay_the_same() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("dv-linux-x86_64");
    std::fs::write(&file, b"abc").unwrap();
    let builds = LinuxBuilds::find(dir.path()).unwrap();
    let build = builds.get(Arch::X86_64).unwrap();
    assert_eq!(
        build.hash,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(build.len, 3);
    assert_eq!(builds.get(Arch::Aarch64), None);
    assert_eq!(
        builds.hash_args(),
        [
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string(),
            "-".to_string()
        ]
    );
    assert_eq!(
        builds.missing(Arch::Aarch64),
        WslError::NoBuild {
            arch: Arch::Aarch64,
            path: dir.path().join("dv-linux-aarch64")
        }
    );

    // Another build of the same length: the new modification time hashes it again.
    std::fs::write(&file, b"xyz").unwrap();
    let later = std::time::SystemTime::now() + Duration::from_secs(5);
    File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(later)
        .unwrap();
    let again = LinuxBuild::at(dir.path(), Arch::X86_64).unwrap().unwrap();
    assert_eq!(
        again.hash,
        "3608bca1e44ea6c4d268eb6db02260269892c0b42b86bbf1e77a6fa16c3c9282"
    );
    // A folder named like a build is no build.
    std::fs::create_dir(dir.path().join("dv-linux-aarch64")).unwrap();
    assert_eq!(LinuxBuild::at(dir.path(), Arch::Aarch64), Ok(None));
    assert!(!LinuxBuilds::find(dir.path()).unwrap().is_empty());
    assert!(
        LinuxBuilds::find(&dir.path().join("none"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_probe_is_up_to_date_when_the_build_is_intact_and_the_link_runs_it() {
    let probe = |build, link| Probe {
        arch: Arch::X86_64,
        home: "/home/kyle".into(),
        path: "/home/kyle/.local/share/dv/bin/dv-abc".into(),
        build,
        link,
    };
    assert!(probe(BuildState::Ok, LinkState::Ok).up_to_date());
    assert!(probe(BuildState::Ok, LinkState::Directory).up_to_date());
    assert!(!probe(BuildState::Ok, LinkState::Other).up_to_date());
    assert!(!probe(BuildState::Ok, LinkState::None).up_to_date());
    assert!(!probe(BuildState::Corrupt, LinkState::Ok).up_to_date());
    assert!(!probe(BuildState::Missing, LinkState::None).up_to_date());
    assert_eq!(
        probe(BuildState::Ok, LinkState::Ok).link_path(),
        "/home/kyle/.local/bin/dv"
    );
}

/// The scripts under this machine's `/bin/sh` in a scratch home.
#[cfg(target_os = "linux")]
mod local {
    use std::path::Path;

    use pretty_assertions::assert_eq;

    use super::*;

    struct Scratch {
        _dir: tempfile::TempDir,
        home: PathBuf,
        builds: LinuxBuilds,
        shell: Shell,
    }

    impl Scratch {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().join("home");
            let folder = dir.path().join("builds");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(
                folder.join(Arch::host().file_name()),
                "#!/bin/sh\necho \"a stand-in dv: $*\"\n",
            )
            .unwrap();
            Self {
                builds: LinuxBuilds::find(&folder).unwrap(),
                shell: Shell::local(Some(home.to_str().unwrap().to_string())).unwrap(),
                home,
                _dir: dir,
            }
        }

        fn build(&self) -> &LinuxBuild {
            self.builds.get(Arch::host()).unwrap()
        }

        fn bin(&self) -> PathBuf {
            self.home.join(".local/share/dv/bin")
        }

        fn installed(&self) -> PathBuf {
            self.bin().join(format!("dv-{}", self.build().hash))
        }

        fn link(&self) -> PathBuf {
            self.home.join(".local/bin/dv")
        }

        fn probe(&self) -> Probe {
            probe(&self.shell, &self.builds).unwrap()
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn an_install_copies_the_build_checks_it_and_links_it() {
        let scratch = Scratch::new();
        let probe = scratch.probe();
        assert_eq!(
            (probe.build, probe.link, probe.arch),
            (BuildState::Missing, LinkState::None, Arch::host())
        );
        assert_eq!(probe.home, scratch.home.to_str().unwrap());
        assert_eq!(probe.path, scratch.installed().to_str().unwrap());

        let heard = Mutex::new(Vec::new());
        let installed = install(&scratch.shell, scratch.build(), &|done, total| {
            heard.lock().push((done, total));
        })
        .unwrap();
        assert_eq!(
            installed,
            Installed {
                path: scratch.installed().to_str().unwrap().to_string(),
                linked: true,
                warnings: Vec::new(),
            }
        );
        let len = scratch.build().len;
        assert_eq!(heard.into_inner(), [(0, len), (len, len)]);
        assert_eq!(
            std::fs::read(scratch.installed()).unwrap(),
            std::fs::read(&scratch.build().path).unwrap()
        );
        assert_eq!(
            std::fs::read_link(scratch.link()).unwrap(),
            scratch.installed()
        );
        // No temporary copy or link is left behind.
        assert_eq!(
            names(&scratch.bin()),
            [format!("dv-{}", scratch.build().hash)]
        );
        assert_eq!(names(&scratch.home.join(".local/bin")), ["dv"]);
        let ran = std::process::Command::new(scratch.link())
            .arg("--version")
            .output()
            .unwrap();
        assert_eq!(ran.stdout, b"a stand-in dv: --version\n");
        assert!(scratch.probe().up_to_date());

        // Again: the same result.
        install(&scratch.shell, scratch.build(), &|_, _| {}).unwrap();
        assert!(scratch.probe().up_to_date());
    }

    #[test]
    fn an_install_replaces_a_file_at_the_link_and_leaves_a_directory_alone() {
        let scratch = Scratch::new();
        std::fs::create_dir_all(scratch.link().parent().unwrap()).unwrap();
        // The first implementation's regular file.
        std::fs::write(scratch.link(), "an old dv").unwrap();
        assert_eq!(scratch.probe().link, LinkState::Other);
        install(&scratch.shell, scratch.build(), &|_, _| {}).unwrap();
        assert_eq!(
            std::fs::read_link(scratch.link()).unwrap(),
            scratch.installed()
        );

        std::fs::remove_file(scratch.link()).unwrap();
        std::fs::create_dir(scratch.link()).unwrap();
        std::fs::write(scratch.link().join("keep"), "mine").unwrap();
        let installed = install(&scratch.shell, scratch.build(), &|_, _| {}).unwrap();
        assert!(!installed.linked);
        assert_eq!(
            installed.warnings,
            [format!(
                "{} is a directory, so it doesn't run this dv; it was left alone",
                scratch.link().display()
            )]
        );
        assert_eq!(
            std::fs::read_to_string(scratch.link().join("keep")).unwrap(),
            "mine"
        );
        let probe = scratch.probe();
        assert_eq!(
            (probe.build, probe.link),
            (BuildState::Ok, LinkState::Directory)
        );
    }

    #[test]
    fn a_link_that_cant_be_made_is_a_warning_and_the_build_is_installed() {
        // `~/.local/bin` a file (an old tool's), where the link's folder goes.
        let scratch = Scratch::new();
        std::fs::create_dir_all(scratch.home.join(".local")).unwrap();
        std::fs::write(scratch.home.join(".local/bin"), "not a folder").unwrap();
        let installed = install(&scratch.shell, scratch.build(), &|_, _| {}).unwrap();
        assert_eq!(installed.path, scratch.installed().to_str().unwrap());
        assert!(!installed.linked);
        let [warning] = &installed.warnings[..] else {
            panic!("{:?}", installed.warnings);
        };
        assert!(
            warning.starts_with(&format!(
                "couldn't point {} at this dv (mkdir: ",
                scratch.link().display()
            )) && warning.ends_with("); it was left as it was"),
            "{warning}"
        );
        assert_eq!(
            std::fs::read(scratch.installed()).unwrap(),
            std::fs::read(&scratch.build().path).unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(scratch.home.join(".local/bin")).unwrap(),
            "not a folder"
        );
        // No temporary copy or link is left behind.
        assert_eq!(
            names(&scratch.bin()),
            [format!("dv-{}", scratch.build().hash)]
        );
        let probe = scratch.probe();
        assert_eq!((probe.build, probe.link), (BuildState::Ok, LinkState::None));
    }

    #[test]
    fn an_install_that_fails_before_reading_its_input_says_why_first() {
        // `~/.local` a file: the build's own folder can't be made, and the script ends before
        // it reads a build too big for the pipe's buffer, so writing it fails too.
        let scratch = Scratch::new();
        std::fs::write(scratch.home.join(".local"), "not a folder").unwrap();
        let big = tempfile::tempdir().unwrap();
        std::fs::write(
            big.path().join(Arch::host().file_name()),
            vec![b'#'; 4 * 1024 * 1024],
        )
        .unwrap();
        let builds = LinuxBuilds::find(big.path()).unwrap();
        let build = builds.get(Arch::host()).unwrap();
        let Err(WslError::Failed(message)) = install(&scratch.shell, build, &|_, _| {}) else {
            panic!("installed without a folder to install into");
        };
        assert!(
            message.starts_with("installing dv in this machine failed: mkdir: "),
            "{message}"
        );
        assert!(!message.contains("writing to"), "{message}");
    }

    #[test]
    fn a_copy_that_isnt_the_build_is_refused_and_removed() {
        let scratch = Scratch::new();
        let mut wrong = scratch.build().clone();
        wrong.hash = "0".repeat(64);
        let Err(error) = install(&scratch.shell, &wrong, &|_, _| {}) else {
            panic!("installed a copy that isn't the build");
        };
        assert_eq!(
            error,
            WslError::Failed(format!(
                "the copy of {} in this machine hashed to {}, not {}: nothing was installed",
                wrong.path.display(),
                scratch.build().hash,
                wrong.hash
            ))
        );
        assert_eq!(names(&scratch.bin()), Vec::<String>::new());
        assert!(!scratch.link().exists());
    }

    #[test]
    fn a_build_that_changed_on_disk_reads_as_corrupt() {
        let scratch = Scratch::new();
        install(&scratch.shell, scratch.build(), &|_, _| {}).unwrap();
        std::fs::write(scratch.installed(), "#!/bin/sh\necho another build\n").unwrap();
        let probe = scratch.probe();
        assert_eq!(
            (probe.build, probe.link),
            (BuildState::Corrupt, LinkState::Ok)
        );
        assert!(!probe.up_to_date());
    }

    #[test]
    fn a_prune_keeps_this_build_and_nothing_else_of_dvs() {
        let scratch = Scratch::new();
        install(&scratch.shell, scratch.build(), &|_, _| {}).unwrap();
        let bin = scratch.bin();
        for old in ["dv-0123", "dv-4567"] {
            std::fs::write(bin.join(old), "an old build").unwrap();
        }
        // An install in progress, and the first implementation's folders beside `bin`.
        std::fs::write(bin.join(".dv-abc.123"), "copying").unwrap();
        for kept in ["cli", "host"] {
            let dir = scratch.home.join(".local/share/dv").join(kept);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("dv"), "theirs").unwrap();
        }
        let mut pruned = prune(&scratch.shell, &scratch.builds).unwrap();
        pruned.sort();
        assert_eq!(pruned, ["dv-0123", "dv-4567"]);
        assert_eq!(
            names(&bin),
            [
                ".dv-abc.123".to_string(),
                format!("dv-{}", scratch.build().hash)
            ]
        );
        for kept in ["cli", "host"] {
            assert!(
                scratch
                    .home
                    .join(".local/share/dv")
                    .join(kept)
                    .join("dv")
                    .is_file()
            );
        }
        // The build `~/.local/bin/dv` runs stays, whoever pointed it there.
        std::fs::write(bin.join("dv-89ab"), "another app's build").unwrap();
        std::fs::remove_file(scratch.link()).unwrap();
        std::os::unix::fs::symlink(bin.join("dv-89ab"), scratch.link()).unwrap();
        assert_eq!(
            prune(&scratch.shell, &scratch.builds).unwrap(),
            Vec::<String>::new()
        );
        assert!(bin.join("dv-89ab").is_file());

        // Nothing installed yet is nothing to prune.
        let fresh = Scratch::new();
        assert_eq!(
            prune(&fresh.shell, &fresh.builds).unwrap(),
            Vec::<String>::new()
        );
    }
}
