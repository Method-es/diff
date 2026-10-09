//! Exit codes and error documents (docs/PLAN.md §10.3): repositories that can't be used, parse
//! errors under `--json`, and the commands of later versions.

use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[test]
fn a_wsl_repository_is_exit_4_without_starting_anything() {
    let fixture = Fixture::new();
    for args in [
        ["status", "--repo", r"\\wsl.localhost\Ubuntu\home\kyle\api"],
        ["status", "--repo", r"\\wsl$\Ubuntu\home\kyle\api"],
        ["status", "--wsl", "Ubuntu:/home/kyle/api"],
    ] {
        let mut line = args.to_vec();
        line.push("--json");
        let error = fixture
            .run(&line)
            .error(Exit::Unavailable, "backend_unavailable");
        assert_eq!(
            error["message"],
            "Ubuntu:/home/kyle/api is inside WSL: run dv inside the distro for now \
             (wsl -d Ubuntu --cd /home/kyle/api dv …); dv on Windows reaches WSL repositories in \
             a later version"
        );
    }
    fixture
        .run(&["comment", "list", "--wsl", "Ubuntu:relative", "--json"])
        .error(Exit::Usage, "usage");
    fixture
        .run(&["setup", "wsl", "Ubuntu", "--json"])
        .error(Exit::Unavailable, "backend_unavailable");
}

#[test]
fn outside_a_repository_is_exit_1() {
    let fixture = Fixture::new();
    let outside = fixture.outside("plain");
    let error = fixture
        .run_in(&outside, &["status", "--json"])
        .error(Exit::Failure, "not_a_repository");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with("not a git repository")
    );
    let missing = fixture.path().join("no-such-dir");
    let missing = missing.to_str().unwrap();
    fixture
        .run(&["review", "list", "--repo", missing, "--json"])
        .error(Exit::Failure, "not_a_repository");

    // A relative --repo resolves against the working directory.
    let json = fixture
        .run_in(
            &fixture.path().join("src"),
            &["review", "list", "--repo", "..", "--json"],
        )
        .ok_json();
    assert_eq!(json["reviews"], json!([]));
}

#[test]
fn parse_errors_are_json_when_json_is_asked_for() {
    let fixture = Fixture::new();
    let ran = fixture.run(&["review", "list", "--bogus", "--json"]);
    let error = ran.error(Exit::Usage, "usage");
    assert_eq!(error["message"], "unexpected argument '--bogus' found");
    assert!(
        ran.stderr.contains("--bogus"),
        "clap's message on stderr: {}",
        ran.stderr
    );

    // Without --json, stderr alone.
    let ran = fixture.run(&["review", "list", "--bogus"]);
    assert_eq!(ran.exit, Exit::Usage);
    assert_eq!(ran.stdout, "");
    assert!(
        ran.stderr
            .starts_with("error: unexpected argument '--bogus'")
    );

    // --json anywhere, even where it doesn't parse.
    let ran = fixture.run(&["--json", "review", "wait", "--timeout", "never"]);
    let error = ran.error(Exit::Usage, "usage");
    assert!(
        error["message"].as_str().unwrap().contains("--timeout"),
        "{error}"
    );
    // Spelt with a value, which clap refuses for a flag: the agent still asked for JSON.
    for json in ["--json=true", "--json="] {
        let ran = fixture.run(&[json, "status"]);
        let error = ran.error(Exit::Usage, "usage");
        assert!(
            error["message"].as_str().unwrap().contains("--json"),
            "{error}"
        );
        let ran = fixture.run(&["status", json]);
        ran.error(Exit::Usage, "usage");
    }
    // After a bare `--`, `--json` is a value, not the flag.
    let ran = fixture.run(&["review", "new", "--", "--json"]);
    assert_eq!(ran.exit, Exit::Usage);
    assert_eq!(ran.stdout, "");
    // A `--` given as a text option's value is refused as text, and isn't the end of the
    // options: the `--json` after it still asks for JSON, and nothing is written.
    let review = fixture.review(Target::Working);
    let thread = fixture.thread(&review, "src/app.ts", 10, 10, "Why a cache?");
    let before = fixture.events(&review.id).len();
    for args in [
        &[
            "comment",
            "reply",
            thread.as_str(),
            "--body",
            "--",
            "--json",
        ][..],
        &[
            "comment",
            "address",
            thread.as_str(),
            "--note",
            "--",
            "--json",
        ],
        &[
            "review", "done", "--author", "codex", "--note", "-", "--json",
        ],
    ] {
        let ran = fixture.run(args);
        let error = ran.error(Exit::Usage, "usage");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("alone isn't text"),
            "{args:?}: {error}"
        );
    }
    assert_eq!(fixture.events(&review.id).len(), before);
    // A negative timeout gets the timeout's own message, not clap's tip to write `-- -1`.
    let ran = fixture.run(&["review", "wait", "--timeout", "-1", "--json"]);
    let error = ran.error(Exit::Usage, "usage");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("the timeout must be a positive number of seconds"),
        "{error}"
    );

    // A missing subcommand says so, wherever --json is: after the group it counts as an
    // argument and clap says what is missing; before it, clap shows the group's help instead,
    // whose first line is no message.
    for (line, group) in [
        (&["review", "--json"][..], "dv review"),
        (&["--json", "review"], "dv review"),
        (&["--json", "comment"], "dv comment"),
        (&["comment", "--json"], "dv comment"),
    ] {
        let ran = fixture.run(line);
        assert_eq!(ran.exit, Exit::Usage, "{line:?}");
        // stderr: clap's error, or the group's help, which lists its commands.
        assert!(
            ran.stderr.starts_with("error: ") || ran.stderr.contains("Commands:"),
            "{line:?}: {}",
            ran.stderr
        );
        let json = ran.json();
        assert_eq!(json["error"]["code"], "usage", "{line:?}");
        assert_eq!(
            json["error"]["message"],
            format!("'{group}' requires a subcommand but one was not provided"),
            "{line:?}"
        );
    }

    // Help and the version print on stdout and succeed.
    let ran = fixture.run(&["--help"]);
    assert_eq!(ran.exit, Exit::Ok);
    assert!(ran.stdout.contains("comment"), "{}", ran.stdout);
    let ran = fixture.run(&["--version"]);
    assert_eq!(ran.exit, Exit::Ok);
    assert_eq!(ran.stdout, format!("dv {}\n", env!("CARGO_PKG_VERSION")));
}

#[test]
fn later_commands_are_refused_with_what_to_do() {
    let fixture = Fixture::new();
    let refused = |args: &[&str], exit: Exit, code: &str| {
        let mut line = args.to_vec();
        line.push("--json");
        fixture.run(&line).error(exit, code)["message"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let message = refused(
        &["review", "new", "--pr", "482"],
        Exit::Usage,
        "unsupported",
    );
    assert!(
        message.starts_with("GitHub PRs arrive in a later version"),
        "{message}"
    );
    refused(&["doctor"], Exit::Usage, "unsupported");
    refused(&["backend", "--stdio"], Exit::Usage, "unsupported");
    refused(&["setup", "path"], Exit::Usage, "unsupported");
    refused(&["setup", "all"], Exit::Usage, "unsupported");
    refused(&["setup"], Exit::Usage, "usage");
}

#[test]
fn a_relative_path_from_a_working_directory_inside_wsl_is_exit_4() {
    let fixture = Fixture::new();
    // PowerShell can stand in a distro's folder; nothing there is opened or started.
    let inside = std::path::PathBuf::from(r"\\wsl.localhost\Ubuntu\home\kyle");
    for args in [
        vec!["status", "--json"],
        vec!["status", "--repo", ".", "--json"],
        vec!["comment", "list", "--repo", "api", "--json"],
        vec!["review", "list", "--repo", "../kyle/api", "--json"],
    ] {
        let error = fixture
            .run_in(&inside, &args)
            .error(Exit::Unavailable, "backend_unavailable");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .starts_with("Ubuntu:/home/kyle"),
            "{args:?}: {error}"
        );
    }
    let global = crate::GlobalArgs {
        repo: Some("api".to_string()),
        ..crate::GlobalArgs::default()
    };
    assert_eq!(
        crate::location(&global, &inside).unwrap(),
        dv_core::location::Location::Wsl {
            distro: "Ubuntu".to_string(),
            path: "/home/kyle/api".to_string(),
        }
    );
}

#[test]
fn a_missing_argument_is_named_in_the_json_message() {
    let fixture = Fixture::new();
    let error = fixture
        .run(&["review", "new", "--base", "main", "--json"])
        .error(Exit::Usage, "usage");
    assert_eq!(
        error["message"],
        "the following required arguments were not provided: --branch [<BRANCH>]"
    );
    let error = fixture
        .run(&[
            "comment",
            "add",
            "--file",
            "src/app.ts",
            "--lines",
            "1",
            "--json",
        ])
        .error(Exit::Usage, "usage");
    let message = error["message"].as_str().unwrap();
    assert!(
        message.starts_with("the following required arguments were not provided: ")
            && message.contains("--body"),
        "{message}"
    );
}
