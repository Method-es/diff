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
    assert_eq!(launch(&gui(&[]).0), Launch::Detach);
    assert_eq!(launch(&gui(&["open", "."]).0), Launch::Detach);
    assert_eq!(launch(&gui(&["--no-detach"]).0), Launch::InProcess);
    assert_eq!(
        launch(&gui(&["--no-detach", "open", "."]).0),
        Launch::InProcess
    );
    assert_eq!(launch(&gui(&["--automation"]).0), Launch::InProcess);
    assert_eq!(
        launch(&gui(&["--automation-pipe", "dv-x"]).0),
        Launch::InProcess
    );
}

#[test]
fn the_detached_copy_runs_in_process_with_the_same_arguments() {
    let args: Vec<OsString> = [
        "dv",
        "--data-dir",
        "D:\\x",
        "open",
        "repo",
        "--review",
        REVIEW,
    ]
    .iter()
    .map(OsString::from)
    .collect();
    let detached = detached_args(&args);
    assert_eq!(
        detached,
        [
            "--no-detach",
            "--data-dir",
            "D:\\x",
            "open",
            "repo",
            "--review",
            REVIEW
        ]
        .iter()
        .map(OsString::from)
        .collect::<Vec<_>>()
    );
    // That line parses, and runs in process.
    let mut line = vec![OsString::from("dv")];
    line.extend(detached);
    let cli = Cli::try_parse_from(line).unwrap();
    assert_eq!(launch(&cli), Launch::InProcess);
    assert!(matches!(cli.command, Some(Command::Open(_))));
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
