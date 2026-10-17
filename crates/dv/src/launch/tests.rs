//! Where the app runs and what it opens (docs/PLAN.md §10.1, §13.1), as pure decisions: no test
//! ever starts the app.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use clap::Parser as _;
use dv_core::review::{Author, ReviewId, ReviewStore, ops};
use dv_core::target::Target;
use dv_test_support::FixtureRepo;
use pretty_assertions::assert_eq;

use super::*;
use crate::cli::Command;

/// `dv <args>` as a GUI command line, with `open`'s arguments when it was `dv open`.
fn gui(args: &[&str]) -> (Cli, Option<OpenArgs>) {
    let mut cli = Cli::try_parse_from(std::iter::once("dv").chain(args.iter().copied()))
        .unwrap_or_else(|error| panic!("{args:?}: {error}"));
    match cli.command.take() {
        None => (cli, None),
        Some(Command::Open(open)) => (cli, Some(open)),
        Some(other) => panic!("{args:?} is headless: {other:?}"),
    }
}

fn startup_of(args: &[&str], cwd: &Path) -> Result<Startup, CliError> {
    let (cli, open) = gui(args);
    startup(&cli, open.as_ref(), cwd, None)
}

fn cwd() -> PathBuf {
    std::env::temp_dir().join("dv-launch-tests")
}

const REVIEW: &str = "r-01k6zzzzzzzzzzzzzzzzzzzzzz";

#[test]
fn the_app_detaches_unless_automated_or_told_not_to() {
    assert_eq!(launch(&gui(&[]).0, Entry::Console), Launch::Detach);
    assert_eq!(
        launch(&gui(&["open", "."]).0, Entry::Console),
        Launch::Detach
    );
    assert_eq!(
        launch(&gui(&["--no-detach"]).0, Entry::Console),
        Launch::InProcess
    );
    assert_eq!(
        launch(&gui(&["--no-detach", "open", "."]).0, Entry::Console),
        Launch::InProcess
    );
    assert_eq!(
        launch(&gui(&["--automation"]).0, Entry::Console),
        Launch::InProcess
    );
    assert_eq!(
        launch(&gui(&["--automation-pipe", "dv-x"]).0, Entry::Console),
        Launch::InProcess
    );
}

#[test]
fn dv_gui_always_runs_the_app_in_its_own_process() {
    for line in [&[][..], &["open", "."], &["--no-detach"]] {
        assert_eq!(
            launch(&gui(line).0, Entry::Gui),
            Launch::InProcess,
            "{line:?}"
        );
    }
}

#[test]
fn the_detached_app_is_dv_gui_beside_a_packaged_dv_and_itself_otherwise() {
    let exe = Path::new(r"C:\Users\me\AppData\Local\dv\current\dv.exe");
    let gui = exe.with_file_name("dv-gui.exe");
    let with_gui = detached_program(exe, true, |path| path == gui);
    if cfg!(windows) {
        assert_eq!(with_gui, gui);
    } else {
        // macOS and Linux have one binary for both.
        assert_eq!(with_gui, exe);
    }
    // A packaged build without it: this executable again, as `--no-detach`.
    assert_eq!(detached_program(exe, true, |_| false), exe);
    // A development build never starts a `dv-gui.exe` beside it, which may be an older
    // build's (`cargo build --bin dv`).
    assert_eq!(detached_program(exe, false, |path| path == gui), exe);
}

fn words(line: &[&str]) -> Vec<OsString> {
    line.iter().map(OsString::from).collect()
}

/// The detached line for `dv <args>` run in `cwd`, `current_is_repo` saying whether a bare
/// `dv` is in a repository.
fn detached(args: &[&str], cwd: &Path, current_is_repo: bool) -> Vec<OsString> {
    let startup = startup_of(args, cwd).unwrap();
    let mut line = vec!["dv"];
    line.extend_from_slice(args);
    detached_args(&words(&line), &startup, current_is_repo)
}

#[test]
fn the_detached_copy_runs_in_process_and_names_the_repository_absolutely() {
    let cwd = cwd();
    let repo = cwd.join("repo");
    let review = detached(
        &["--data-dir", r"D:\x", "open", "repo", "--review", REVIEW],
        &cwd,
        false,
    );
    assert_eq!(
        review,
        [
            OsString::from("--data-dir"),
            OsString::from(r"D:\x"),
            OsString::from("--no-detach"),
            OsString::from("open"),
            repo.clone().into_os_string(),
            OsString::from("--review"),
            OsString::from(REVIEW),
        ]
    );
    // That line parses, runs in process, and opens the same review from any folder.
    let mut line = vec![OsString::from("dv")];
    line.extend(review);
    let cli = Cli::try_parse_from(line).unwrap();
    assert_eq!(launch(&cli, Entry::Console), Launch::InProcess);
    let Some(Command::Open(open)) = &cli.command else {
        panic!("{:?}", cli.command);
    };
    let elsewhere = std::env::temp_dir().join("somewhere-else");
    assert_eq!(
        startup(&cli, Some(open), &elsewhere, None).unwrap(),
        Startup::Review(
            Location::Local(repo.clone()),
            ReviewId::parse(REVIEW).unwrap()
        )
    );

    // `dv open .` and `--repo` become `open <absolute path>`; the app's options stay.
    let dot = detached(&["--window-size", "1100x700", "open", "."], &cwd, false);
    assert_eq!(
        &dot[..3],
        &words(&["--window-size", "1100x700", "--no-detach"])[..]
    );
    assert_eq!(dot[3], "open");
    let opened = PathBuf::from(&dot[4]);
    assert!(opened.is_absolute(), "{opened:?}");
    assert_eq!(std::path::absolute(&opened).unwrap(), opened);
    // `.` resolved: the working directory itself.
    assert_eq!(opened, cwd);
    assert_eq!(dot.len(), 5);
    let flag = detached(&["--repo", "repo"], &cwd, false);
    assert_eq!(
        flag,
        [
            OsString::from("--no-detach"),
            OsString::from("open"),
            repo.clone().into_os_string()
        ]
    );
}

#[test]
fn a_bare_dv_passes_its_repository_absolutely_and_nothing_elsewhere() {
    let cwd = cwd();
    assert_eq!(
        detached(&[], &cwd, true),
        [
            OsString::from("--no-detach"),
            OsString::from("open"),
            cwd.clone().into_os_string()
        ]
    );
    assert_eq!(detached(&[], &cwd, false), words(&["--no-detach"]));
    // A WSL repository goes as its share, whatever this folder is.
    let wsl = detached(
        &["open", r"\\wsl.localhost\Ubuntu\home\kyle\api"],
        &cwd,
        false,
    );
    assert_eq!(
        wsl,
        words(&[
            "--no-detach",
            "open",
            r"\\wsl.localhost\Ubuntu\home\kyle\api"
        ])
    );
    assert_eq!(
        open_args(
            &Location::Wsl {
                distro: "Ubuntu".into(),
                path: "/tmp/x".into()
            },
            None
        ),
        words(&["open", r"\\wsl.localhost\Ubuntu\tmp\x"])
    );
}

#[test]
fn only_a_bare_dv_in_a_repository_reads_as_one() {
    let repo = FixtureRepo::new();
    let options = || LocalBackendOptions {
        env: repo.env_overlay(),
    };
    let here = Startup::CurrentRepo(Location::Local(repo.path().to_path_buf()));
    assert!(current_is_repo(&here, options()));
    let empty = tempfile::tempdir().unwrap();
    let elsewhere = Startup::CurrentRepo(Location::Local(empty.path().to_path_buf()));
    assert!(!current_is_repo(&elsewhere, options()));
    let named = Startup::Repo(Location::Local(repo.path().to_path_buf()));
    assert!(!current_is_repo(&named, options()));
}

#[test]
fn bare_dv_opens_the_current_repository_if_it_is_one() {
    let cwd = cwd();
    assert_eq!(
        startup_of(&[], &cwd).unwrap(),
        Startup::CurrentRepo(Location::Local(cwd.clone()))
    );
    // An automation run opens only what the scenario asks for.
    assert_eq!(
        startup_of(&["--automation"], &cwd).unwrap(),
        Startup::Nothing
    );
    // Named on the bare line, it is asked for.
    assert_eq!(
        startup_of(&["--repo", "api"], &cwd).unwrap(),
        Startup::Repo(Location::Local(cwd.join("api")))
    );
}

#[test]
fn dv_open_names_a_repository_or_a_review() {
    let cwd = cwd();
    assert_eq!(
        startup_of(&["open"], &cwd).unwrap(),
        Startup::Repo(Location::Local(cwd.clone()))
    );
    assert_eq!(
        startup_of(&["open", "api"], &cwd).unwrap(),
        Startup::Repo(Location::Local(cwd.join("api")))
    );
    assert_eq!(
        startup_of(&["--automation", "open", "api"], &cwd).unwrap(),
        Startup::Repo(Location::Local(cwd.join("api")))
    );
    assert_eq!(
        startup_of(&["open", r"\\wsl.localhost\Ubuntu\home\kyle\api"], &cwd).unwrap(),
        Startup::Repo(Location::Wsl {
            distro: "Ubuntu".into(),
            path: "/home/kyle/api".into()
        })
    );
    assert_eq!(
        startup_of(&["open", "--wsl", "Ubuntu:/srv/api"], &cwd).unwrap(),
        Startup::Repo(Location::Wsl {
            distro: "Ubuntu".into(),
            path: "/srv/api".into()
        })
    );
    let review = ReviewId::parse(REVIEW).unwrap();
    assert_eq!(
        startup_of(&["open", "api", "--review", REVIEW], &cwd).unwrap(),
        Startup::Review(Location::Local(cwd.join("api")), review.clone())
    );
    assert_eq!(
        startup_of(&["--review", REVIEW], &cwd).unwrap(),
        Startup::Review(Location::Local(cwd.clone()), review)
    );
}

#[test]
fn inside_a_distro_dv_open_takes_a_windows_drive_as_its_mount() {
    let in_ubuntu = |args: &[&str]| {
        let (cli, open) = gui(args);
        startup(&cli, open.as_ref(), Path::new("/home/kyle"), Some("Ubuntu")).unwrap()
    };
    for args in [
        &["open", r"D:\code\api"][..],
        &["open", "d:/code/api"],
        &["--repo", r"D:\code\api\", "open"],
    ] {
        assert_eq!(
            in_ubuntu(args),
            Startup::Repo(Location::Local(PathBuf::from("/mnt/d/code/api"))),
            "{args:?}"
        );
    }
}

#[test]
fn prs_and_mistakes_are_refused_before_any_window() {
    let cwd = cwd();
    for pr in [
        "#482",
        "https://github.com/kylekz/difftest/pull/7",
        "http://x/pull/1",
    ] {
        let error = startup_of(&["open", pr], &cwd).unwrap_err();
        assert_eq!(error.exit, dv_cli::Exit::Usage, "{pr}");
        assert!(
            error
                .message
                .starts_with("GitHub PRs arrive in a later version")
        );
    }
    // `#` alone, or followed by more than digits, is a path.
    assert!(matches!(
        startup_of(&["open", "#notes"], &cwd),
        Ok(Startup::Repo(_))
    ));
    let error = startup_of(&["open", "api", "--repo", "web"], &cwd).unwrap_err();
    assert_eq!(error.exit, dv_cli::Exit::Usage);
    let error = startup_of(&["open", "--review", "r-bad"], &cwd).unwrap_err();
    assert_eq!(error.exit, dv_cli::Exit::Usage);
    assert!(error.message.starts_with("--review: "));
}

const JSON_REFUSED: &str = "`dv` and `dv open` start the app, which prints no JSON: --json is \
                            for the commands that print (`dv status --json`)";
const AUTHOR_REFUSED: &str = "the app always acts as the reviewer: --author is for the commands \
                              an agent runs (`dv comment reply <id> --author <name>`)";

#[test]
fn the_headless_commands_flags_are_refused_on_an_app_line() {
    let cwd = cwd();
    // An agent asking for JSON gets a usage error (as JSON, `main` reports it so) rather than a
    // window and nothing on stdout; `--author` means nothing to the app, which is the
    // reviewer's.
    for (args, message) in [
        (&["--json"][..], JSON_REFUSED),
        (&["open", ".", "--json"], JSON_REFUSED),
        (&["--author", "codex"], AUTHOR_REFUSED),
        (&["open", "--author", "claude-code"], AUTHOR_REFUSED),
    ] {
        let error = startup_of(args, &cwd).unwrap_err();
        assert_eq!(error.exit, dv_cli::Exit::Usage, "{args:?}");
        assert_eq!(error.code, dv_cli::error::code::USAGE, "{args:?}");
        // One sentence as written, without a wrapped literal's indentation inside it.
        assert_eq!(error.message, message, "{args:?}");
        assert!(!error.message.contains("  "), "{:?}", error.message);
    }
    // The global flags the app does take still work there.
    assert!(startup_of(&["--repo", "api", "--review", REVIEW], &cwd).is_ok());
}

#[test]
fn a_detaching_dv_checks_the_repository_and_the_review_first() {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    let options = || LocalBackendOptions {
        env: repo.env_overlay(),
    };
    let backend: Arc<dyn Backend> =
        Arc::new(LocalBackend::open_with(repo.path(), options()).unwrap());
    let review = ops::open_review(
        &ReviewStore::new(backend),
        Target::Working,
        Author::reviewer("dv fixture"),
    )
    .unwrap()
    .review;
    let here = Location::Local(repo.path().to_path_buf());

    assert_eq!(check(&Startup::Repo(here.clone()), options()), Ok(()));
    assert_eq!(
        check(&Startup::Review(here.clone(), review.id.clone()), options()),
        Ok(())
    );
    let missing = ReviewId::generate();
    let error = check(&Startup::Review(here, missing.clone()), options()).unwrap_err();
    assert_eq!(error.code, dv_cli::error::code::NOT_FOUND);
    assert!(
        error
            .message
            .starts_with(&format!("no review {missing} in "))
    );

    let outside = repo.outside_dir("plain");
    let error = check(&Startup::Repo(Location::Local(outside.clone())), options()).unwrap_err();
    assert_eq!(error.code, dv_cli::error::code::NOT_A_REPOSITORY);
    // Bare `dv` outside a repository, and WSL locations, are the app's to deal with.
    assert_eq!(
        check(&Startup::CurrentRepo(Location::Local(outside)), options()),
        Ok(())
    );
    let wsl = Location::Wsl {
        distro: "Ubuntu".into(),
        path: "/srv/api".into(),
    };
    assert_eq!(check(&Startup::Repo(wsl.clone()), options()), Ok(()));
    assert_eq!(
        check(&Startup::Review(wsl, missing.clone()), options()),
        Ok(()),
        "a WSL review is the app's to find, through the distro"
    );
    assert_eq!(check(&Startup::Nothing, options()), Ok(()));
}

/// The role [`detach_probe`] plays in a process [`a_detached_app_leaves_the_callers_output_alone`]
/// starts: `parent` (dv starting the app) or `child` (the app).
const PROBE: &str = "DV_DETACH_PROBE";

#[test]
fn a_detached_app_leaves_the_callers_output_alone() {
    // This test is a caller that captures `dv open`'s output. It runs this test binary as the
    // parent, which starts a child through `spawn_detached`, as `dv` starts the app, and
    // exits; the child stays a few seconds, as the app stays open. The end of the parent's
    // output must come when the parent exits, not when the child does.
    let started = Instant::now();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "launch::tests::detach_probe",
            "--ignored",
            "--test-threads=1",
        ])
        .env(PROBE, "parent")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let waited = started.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("detach_probe ... ok"), "{stdout}");
    assert!(
        waited < Duration::from_millis(2_500),
        "the caller waited {waited:?}: the detached child held its output open"
    );
}

#[test]
#[ignore = "a process `a_detached_app_leaves_the_callers_output_alone` starts"]
fn detach_probe() {
    match std::env::var(PROBE).as_deref() {
        Ok("parent") => {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args(["--exact", "launch::tests::detach_probe", "--ignored"])
                .env(PROBE, "child");
            spawn_detached(child).unwrap();
        }
        Ok("child") => std::thread::sleep(Duration::from_secs(5)),
        _ => {}
    }
}
