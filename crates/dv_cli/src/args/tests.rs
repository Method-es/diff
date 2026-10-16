//! The command tree's shapes (docs/PLAN.md §10.1): every command parses as documented, global
//! flags go anywhere, and malformed lines are refused by clap itself.

use std::path::PathBuf;
use std::time::Duration;

use pretty_assertions::assert_eq;

use super::*;

fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(std::iter::once("dv").chain(args.iter().copied()))
}

fn command(args: &[&str]) -> Command {
    parse(args)
        .unwrap_or_else(|error| panic!("{args:?}: {error}"))
        .command
}

fn refused(args: &[&str]) {
    assert!(parse(args).is_err(), "{args:?} should be refused");
}

const REVIEW: &str = "r-01k6aaaaaaaaaaaaaaaaaaaaaa";
const THREAD: &str = "t-01k6aaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn the_tree_parses_every_documented_command() {
    assert_eq!(command(&["status"]), Command::Status);
    assert_eq!(
        command(&["review", "list", "--all", "--status", "ready,approved"]),
        Command::Review(ReviewCommand::List(ReviewListArgs {
            all: true,
            status: vec![StatusArg::Ready, StatusArg::Approved],
        }))
    );
    assert_eq!(
        command(&[
            "review",
            "list",
            "--status",
            "changes_requested",
            "--status",
            "in-progress"
        ]),
        Command::Review(ReviewCommand::List(ReviewListArgs {
            all: false,
            status: vec![StatusArg::ChangesRequested, StatusArg::InProgress],
        }))
    );
    assert_eq!(
        command(&["review", "show", REVIEW]),
        Command::Review(ReviewCommand::Show(ReviewIdArg {
            id: Some(REVIEW.into())
        }))
    );
    assert_eq!(
        command(&["review", "wait", "--timeout", "0.5"]),
        Command::Review(ReviewCommand::Wait(ReviewWaitArgs {
            review: ReviewIdArg { id: None },
            timeout: Some(Duration::from_millis(500)),
        }))
    );
    assert_eq!(
        command(&["review", "done", "--note", "All fixed"]),
        Command::Review(ReviewCommand::Done(ReviewDoneArgs {
            review: ReviewIdArg { id: None },
            note: Note {
                note: Some("All fixed".into()),
                note_file: None,
            },
            round: None,
        }))
    );
    assert_eq!(
        command(&["review", "done", REVIEW, "--round", "2"]),
        Command::Review(ReviewCommand::Done(ReviewDoneArgs {
            review: ReviewIdArg {
                id: Some(REVIEW.into())
            },
            note: Note::default(),
            round: Some(2),
        }))
    );
    assert_eq!(
        command(&[
            "review",
            "submit",
            "--verdict",
            "request-changes",
            "--body-file",
            "-"
        ]),
        Command::Review(ReviewCommand::Submit(ReviewSubmitArgs {
            review: ReviewIdArg { id: None },
            verdict: VerdictArg::RequestChanges,
            body: OptionalBody {
                body: None,
                body_file: Some(PathBuf::from("-")),
            },
        }))
    );
    assert_eq!(
        command(&["review", "close", REVIEW]),
        Command::Review(ReviewCommand::Close(RequiredReviewId { id: REVIEW.into() }))
    );
    assert_eq!(
        command(&["review", "delete", REVIEW]),
        Command::Review(ReviewCommand::Delete(RequiredReviewId {
            id: REVIEW.into()
        }))
    );
    assert_eq!(
        command(&[
            "comment", "list", "--status", "open", "--status", "outdated", "--file", "a.ts"
        ]),
        Command::Comment(CommentCommand::List(CommentListArgs {
            status: vec![ThreadStatusArg::Open, ThreadStatusArg::Outdated],
            file: Some("a.ts".into()),
            oneline: false,
        }))
    );
    assert_eq!(
        command(&["comment", "show", THREAD]),
        Command::Comment(CommentCommand::Show(ThreadIdArg { id: THREAD.into() }))
    );
    assert_eq!(
        command(&[
            "comment", "add", "--file", "a.ts", "--lines", "5:3", "--side", "old", "--body", "Why?"
        ]),
        Command::Comment(CommentCommand::Add(CommentAddArgs {
            file: "a.ts".into(),
            lines: Some(LineRange { start: 3, end: 5 }),
            side: SideArg::Old,
            body: Body {
                body: Some("Why?".into()),
                body_file: None,
            },
        }))
    );
    // Without `--lines`, a comment on the whole file.
    assert_eq!(
        command(&["comment", "add", "--file", "a.ts", "--body", "Split it"]),
        Command::Comment(CommentCommand::Add(CommentAddArgs {
            file: "a.ts".into(),
            lines: None,
            side: SideArg::New,
            body: Body {
                body: Some("Split it".into()),
                body_file: None,
            },
        }))
    );
    assert_eq!(
        command(&["comment", "reply", THREAD, "--body-file", "-"]),
        Command::Comment(CommentCommand::Reply(CommentReplyArgs {
            thread: ThreadIdArg { id: THREAD.into() },
            body: Body {
                body: None,
                body_file: Some(PathBuf::from("-")),
            },
        }))
    );
    assert_eq!(
        command(&["comment", "address", THREAD, "--note", "Renamed"]),
        Command::Comment(CommentCommand::Address(CommentAddressArgs {
            thread: ThreadIdArg { id: THREAD.into() },
            note: Note {
                note: Some("Renamed".into()),
                note_file: None,
            },
        }))
    );
    assert_eq!(
        command(&["comment", "resolve", THREAD]),
        Command::Comment(CommentCommand::Resolve(ThreadIdArg { id: THREAD.into() }))
    );
    assert_eq!(
        command(&["comment", "reopen", THREAD]),
        Command::Comment(CommentCommand::Reopen(ThreadIdArg { id: THREAD.into() }))
    );
    assert_eq!(
        command(&["setup", "agents", "--yes", "--home", "x"]),
        Command::Setup(SetupArgs {
            what: Some(SetupCommand::Agents(SetupAgentsArgs {
                yes: YesArg { yes: true },
                home: Some(PathBuf::from("x")),
            })),
        })
    );
    assert_eq!(
        command(&["setup", "wsl", "Ubuntu"]),
        Command::Setup(SetupArgs {
            what: Some(SetupCommand::Wsl(SetupWslArgs {
                distro: "Ubuntu".into(),
                yes: YesArg { yes: false },
            })),
        })
    );
    assert_eq!(
        command(&["setup"]),
        Command::Setup(SetupArgs { what: None })
    );
    assert_eq!(command(&["doctor"]), Command::Doctor);
    assert_eq!(
        command(&["backend", "--stdio"]),
        Command::Backend(BackendArgs { stdio: true })
    );
}

#[test]
fn review_new_takes_one_target() {
    let new = |args: &[&str]| match command(&[&["review", "new"], args].concat()) {
        Command::Review(ReviewCommand::New(new)) => new,
        other => panic!("{other:?}"),
    };
    let working = new(&[]);
    assert!(!working.staged && working.branch.is_none() && working.commit.is_none());
    assert!(new(&["--staged"]).staged);
    assert_eq!(
        new(&["--branch"]).branch,
        Some(None),
        "the branch checked out"
    );
    let branch = new(&["--branch", "feat", "--base", "dev"]);
    assert_eq!(branch.branch, Some(Some("feat".into())));
    assert_eq!(branch.base.as_deref(), Some("dev"));
    assert_eq!(
        new(&["--commit", "HEAD~1"]).commit.as_deref(),
        Some("HEAD~1")
    );
    assert_eq!(
        new(&["--range", "v1...v2"]).range,
        Some(RangeArg {
            from: "v1".into(),
            to: "v2".into(),
            three_dot: true
        })
    );
    assert_eq!(
        new(&["--range", "main..feat"]).range,
        Some(RangeArg {
            from: "main".into(),
            to: "feat".into(),
            three_dot: false
        })
    );
    assert_eq!(new(&["--pr", "#482"]).pr.as_deref(), Some("#482"));

    refused(&["review", "new", "--staged", "--commit", "HEAD"]);
    refused(&["review", "new", "--base", "main"]);
    refused(&["review", "new", "--range", "main"]);
    refused(&["review", "new", "--range", "..feat"]);
}

#[test]
fn global_flags_go_anywhere() {
    let cli = parse(&["--json", "review", "list"]).unwrap();
    assert!(cli.global.json);
    let cli = parse(&["review", "list", "--json", "--author", "codex"]).unwrap();
    assert!(cli.global.json);
    assert_eq!(cli.global.author.as_deref(), Some("codex"));
    let cli = parse(&["review", "--review", REVIEW, "show", "--repo", "../api"]).unwrap();
    assert_eq!(cli.global.review.as_deref(), Some(REVIEW));
    assert_eq!(cli.global.repo.as_deref(), Some("../api"));
    let cli = parse(&["status", "--wsl", "Ubuntu:/home/kyle/api"]).unwrap();
    assert_eq!(cli.global.wsl.as_deref(), Some("Ubuntu:/home/kyle/api"));
    refused(&["status", "--repo", "a", "--wsl", "Ubuntu:/a"]);
}

#[test]
fn malformed_arguments_are_refused() {
    refused(&[]);
    refused(&["review"]);
    refused(&["frobnicate"]);
    refused(&["review", "list", "--status", "bogus"]);
    refused(&["review", "wait", "--timeout", "0"]);
    refused(&["review", "wait", "--timeout", "-1"]);
    refused(&["review", "wait", "--timeout", "soon"]);
    refused(&["review", "wait", "--timeout", "inf"]);
    refused(&["review", "submit"]);
    refused(&["review", "submit", "--verdict", "maybe"]);
    refused(&[
        "review",
        "submit",
        "--verdict",
        "approve",
        "--body",
        "a",
        "--body-file",
        "-",
    ]);
    refused(&["review", "close"]);
    refused(&["comment", "add", "--file", "a.ts", "--lines", "3"]);
    refused(&[
        "comment", "add", "--file", "a.ts", "--lines", "0", "--body", "x",
    ]);
    refused(&[
        "comment", "add", "--file", "a.ts", "--lines", "three", "--body", "x",
    ]);
    refused(&[
        "comment", "add", "--file", "a.ts", "--lines", "3", "--side", "left", "--body", "x",
    ]);
    refused(&["comment", "reply", THREAD]);
    refused(&[
        "comment",
        "reply",
        THREAD,
        "--body",
        "a",
        "--body-file",
        "-",
    ]);
    refused(&["comment", "show"]);
}

#[test]
fn text_that_starts_with_a_dash_is_text() {
    // A Markdown list, the natural shape of a "what I did" note, a "-1" reply, a rule.
    let list = "- renamed foo\n- added a test";
    let note = |args: &[&str]| match command(args) {
        Command::Review(ReviewCommand::Done(done)) => done.note,
        Command::Comment(CommentCommand::Address(address)) => address.note,
        other => panic!("{other:?}"),
    };
    for text in [list, "-1, I disagree", "-- see above", "--- ", "- "] {
        assert_eq!(
            note(&["review", "done", "--note", text, "--json"])
                .note
                .as_deref(),
            Some(text)
        );
        assert_eq!(
            note(&["comment", "address", THREAD, "--note", text])
                .note
                .as_deref(),
            Some(text)
        );
        match command(&[
            "comment", "reply", THREAD, "--body", text, "--author", "codex",
        ]) {
            Command::Comment(CommentCommand::Reply(reply)) => {
                assert_eq!(reply.body.body.as_deref(), Some(text));
            }
            other => panic!("{other:?}"),
        }
        match command(&["review", "submit", "--verdict", "approve", "--body", text]) {
            Command::Review(ReviewCommand::Submit(submit)) => {
                assert_eq!(submit.body.body.as_deref(), Some(text));
            }
            other => panic!("{other:?}"),
        }
        match command(&[
            "comment", "add", "--file", "a.ts", "--lines", "3", "--body", text,
        ]) {
            Command::Comment(CommentCommand::Add(add)) => {
                assert_eq!(add.body.body.as_deref(), Some(text));
            }
            other => panic!("{other:?}"),
        }
    }
    // The global flags after the text still count.
    let cli = parse(&[
        "review", "done", "--note", list, "--json", "--author", "codex",
    ])
    .unwrap();
    assert!(cli.global.json);
    assert_eq!(cli.global.author.as_deref(), Some("codex"));

    // A text option followed by a flag lost its text: refused, not taken as the text.
    for args in [
        &["review", "done", "--note", "--json"][..],
        &["comment", "address", THREAD, "--note", "--author=codex"],
        &["comment", "reply", THREAD, "--body", "--json"],
        &["review", "submit", "--verdict", "approve", "--body", "-v"],
    ] {
        let error = parse(args).unwrap_err();
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::ValueValidation,
            "{args:?}"
        );
        assert!(
            error.to_string().contains("that is a flag, not text"),
            "{error}"
        );
    }

    // A lone `-` or `--` is no text: stdin meant through the inline option, or the end of the
    // options, which would otherwise be stored as the text and let a flag after it count.
    for args in [
        &["review", "done", "--note", "-"][..],
        &["comment", "address", THREAD, "--note", "--", "--json"],
        &["comment", "reply", THREAD, "--body", "--", "--json"],
        &["comment", "reply", THREAD, "--body", "-"],
        &["review", "submit", "--verdict", "approve", "--body", "--"],
        &[
            "comment", "add", "--file", "a.ts", "--lines", "3", "--body", "-",
        ],
    ] {
        let error = parse(args).unwrap_err();
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::ValueValidation,
            "{args:?}"
        );
        assert!(
            error.to_string().contains("alone isn't text")
                && error.to_string().contains("--body-file -")
                && error.to_string().contains("--note-file -"),
            "{error}"
        );
    }

    // A note from a file or stdin, never both forms at once.
    assert_eq!(
        note(&["review", "done", "--note-file", "-"]),
        Note {
            note: None,
            note_file: Some(PathBuf::from("-")),
        }
    );
    refused(&["review", "done", "--note", "a", "--note-file", "-"]);
    refused(&[
        "comment",
        "address",
        THREAD,
        "--note",
        "a",
        "--note-file",
        "n.md",
    ]);
}

#[test]
fn a_negative_timeout_gets_the_timeouts_own_message() {
    for timeout in ["-1", "-0.5"] {
        let error = parse(&["review", "wait", "--timeout", timeout]).unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
        assert!(
            error
                .to_string()
                .contains("the timeout must be a positive number of seconds"),
            "{error}"
        );
    }
}

#[test]
fn lines_and_timeouts_read_as_documented() {
    assert_eq!(parse_lines("7"), Ok(LineRange { start: 7, end: 7 }));
    assert_eq!(parse_lines("3:5"), Ok(LineRange { start: 3, end: 5 }));
    assert_eq!(parse_lines("5:3"), Ok(LineRange { start: 3, end: 5 }));
    assert_eq!(parse_lines("3-5"), Ok(LineRange { start: 3, end: 5 }));
    assert!(parse_lines("3:").is_err());
    assert!(parse_lines(":3").is_err());
    assert_eq!(parse_timeout("600"), Ok(Duration::from_secs(600)));
    assert_eq!(parse_timeout("0.25"), Ok(Duration::from_millis(250)));
    assert!(parse_timeout("NaN").is_err());
}

#[test]
fn the_tree_is_consistent() {
    use clap::CommandFactory as _;
    Cli::command().debug_assert();
}
