use std::io;

use dv_core::backend::{BackendError, GitOpts, GitOutput, WatchEvent, WatchSpec};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::*;

fn round_trip(request: &Request) -> Value {
    let line = serde_json::to_string(request).unwrap();
    assert!(!line.contains('\n'), "{line}");
    let back: Request = serde_json::from_str(&line).unwrap();
    assert_eq!(&back, request);
    serde_json::from_str(&line).unwrap()
}

#[test]
fn a_request_is_its_op_flattened_into_the_envelope() {
    let value = round_trip(&Request {
        id: 7,
        op: Op::Git {
            repo: 2,
            args: vec!["diff".into(), "--stat".into()],
            opts: GitOpts::new()
                .with_stdin("hi")
                .with_ok_codes(&[0, 1])
                .with_index_file("tmp/index")
                .into(),
        },
    });
    assert_eq!(
        value,
        json!({
            "id": 7,
            "op": "git",
            "repo": 2,
            "args": ["diff", "--stat"],
            "opts": {"stdin": "aGk=", "timeout_ms": null, "ok_codes": [0, 1], "index_file": "tmp/index"},
        })
    );

    let value = round_trip(&Request {
        id: 1,
        op: Op::Hello {
            proto: PROTO,
            build: None,
        },
    });
    assert_eq!(
        value,
        json!({"id": 1, "op": "hello", "proto": 2, "build": null})
    );

    let value = round_trip(&Request {
        id: 9,
        op: Op::Watch {
            repo: 3,
            spec: WatchSpec::ALL,
        },
    });
    assert_eq!(
        value,
        json!({"id": 9, "op": "watch", "repo": 3, "spec": {"worktree": true, "store": true, "git": true}})
    );
}

#[test]
fn every_op_round_trips_and_names_itself() {
    let ops = [
        Op::Hello {
            proto: 1,
            build: Some("ab".repeat(32)),
        },
        Op::Open {
            path: "/home/kyle/api".into(),
            env: vec![
                ("HOME".into(), Some("/tmp/h".into())),
                ("GIT_DIR".into(), None),
            ],
        },
        Op::Close { repo: 1 },
        Op::Git {
            repo: 1,
            args: vec!["status".into()],
            opts: WireGitOpts::default(),
        },
        Op::WorktreeRead {
            repo: 1,
            rel: "a.txt".into(),
        },
        Op::WorktreeLen {
            repo: 1,
            rel: "a.txt".into(),
        },
        Op::WorktreeTextStats {
            repo: 1,
            rel: "a.txt".into(),
        },
        Op::ProbePath {
            repo: 1,
            path: "/home/kyle/node_modules/typescript".into(),
        },
        Op::StoreAppend {
            repo: 1,
            rel: "reviews/r.jsonl".into(),
            line: b"{\"v\":1}".to_vec(),
        },
        Op::StoreRead {
            repo: 1,
            rel: "reviews/r.jsonl".into(),
        },
        Op::StoreList {
            repo: 1,
            dir: "reviews".into(),
        },
        Op::StoreWriteAtomic {
            repo: 1,
            rel: "x".into(),
            bytes: vec![0, 255, 10, 13],
        },
        Op::StoreRemove {
            repo: 1,
            rel: "x".into(),
        },
        Op::CopyIndexToStore {
            repo: 1,
            rel: "i".into(),
        },
        Op::Watch {
            repo: 1,
            spec: WatchSpec::STORE,
        },
        Op::Unwatch { watch: 4 },
    ];
    for (n, op) in ops.into_iter().enumerate() {
        let name = op.name();
        let value = round_trip(&Request { id: n as u64, op });
        assert_eq!(value["op"], name);
    }
}

#[test]
fn bytes_cross_as_base64_never_as_number_arrays() {
    let value = round_trip(&Request {
        id: 1,
        op: Op::StoreWriteAtomic {
            repo: 1,
            rel: "x".into(),
            bytes: vec![0, 255, 10, 13],
        },
    });
    assert_eq!(value["bytes"], "AP8KDQ==");
    let value = round_trip(&Request {
        id: 1,
        op: Op::StoreAppend {
            repo: 1,
            rel: "x".into(),
            line: b"line".to_vec(),
        },
    });
    assert_eq!(value["line"], "bGluZQ==");

    let output = WireGitOutput::from(GitOutput {
        code: 1,
        stdout: b"out\n".to_vec(),
        stderr: vec![0xff, 0xfe],
    });
    let value = serde_json::to_value(&output).unwrap();
    assert_eq!(
        value,
        json!({"code": 1, "stdout": "b3V0Cg==", "stderr": "//4="})
    );
    let back: GitOutput = serde_json::from_value::<WireGitOutput>(value)
        .unwrap()
        .into();
    assert_eq!(back.stderr, vec![0xff, 0xfe]);

    assert_eq!(
        serde_json::to_value(Some(Bytes(b"a\0b".to_vec()))).unwrap(),
        json!("YQBi")
    );
    assert_eq!(serde_json::to_value(None::<Bytes>).unwrap(), Value::Null);
    let bytes: Option<Bytes> = serde_json::from_str("\"YQBi\"").unwrap();
    assert_eq!(bytes, Some(Bytes(b"a\0b".to_vec())));
    assert!(serde_json::from_str::<Bytes>("\"not base64!\"").is_err());
    assert!(serde_json::from_str::<Bytes>("[1, 2]").is_err());
}

#[test]
fn optional_arguments_default() {
    let request: Request = serde_json::from_str(r#"{"id":1,"op":"hello","proto":1}"#).unwrap();
    assert_eq!(
        request.op,
        Op::Hello {
            proto: 1,
            build: None
        }
    );
    let request: Request = serde_json::from_str(r#"{"id":2,"op":"open","path":"/x"}"#).unwrap();
    assert_eq!(
        request.op,
        Op::Open {
            path: "/x".into(),
            env: Vec::new()
        }
    );
    let request: Request =
        serde_json::from_str(r#"{"id":3,"op":"git","repo":1,"args":["status"]}"#).unwrap();
    let Op::Git { opts, .. } = request.op else {
        panic!("{:?}", request.op);
    };
    assert_eq!(GitOpts::from(opts), GitOpts::default());
    let request: Request =
        serde_json::from_str(r#"{"id":4,"op":"git","repo":1,"args":[],"opts":{}}"#).unwrap();
    let Op::Git { opts, .. } = request.op else {
        panic!("{:?}", request.op);
    };
    assert_eq!(opts.ok_codes, [0]);
}

#[test]
fn unknown_ops_and_bad_lines_do_not_parse() {
    for line in [
        r#"{"id":1,"op":"frobnicate"}"#,
        r#"{"id":1}"#,
        r#"{"op":"close","repo":1}"#,
        r#"{"id":1,"op":"close"}"#,
        r#"{"id":"1","op":"close","repo":1}"#,
        "not json",
    ] {
        assert!(serde_json::from_str::<Request>(line).is_err(), "{line}");
    }
}

/// Every `BackendError` variant, with the fields an OS or git would give it.
fn every_error() -> Vec<BackendError> {
    let os_error = || io::Error::from(io::ErrorKind::NotFound);
    vec![
        BackendError::Unavailable {
            location: "Ubuntu:/home/kyle/api".into(),
            reason: "WSL isn't installed".into(),
        },
        BackendError::Disconnected {
            location: "Ubuntu".into(),
            reason: "dv's output ended".into(),
        },
        BackendError::PathNotFound {
            path: "/home/kyle/gone".into(),
        },
        BackendError::NotARepository {
            path: "/tmp".into(),
        },
        BackendError::BareRepository {
            path: "/srv/x.git".into(),
        },
        BackendError::InsideGitDir {
            path: "/srv/x/.git/refs".into(),
        },
        BackendError::UnsafeRepository {
            path: "/srv/theirs".into(),
        },
        BackendError::Spawn {
            args: vec!["status".into()],
            source: os_error(),
        },
        BackendError::Git {
            args: vec!["rev-parse".into(), "--verify".into(), "nope".into()],
            code: Some(128),
            stderr: "fatal: Needed a single revision\n".into(),
        },
        BackendError::Git {
            args: vec!["diff".into()],
            code: None,
            stderr: String::new(),
        },
        BackendError::Timeout {
            args: vec!["-c".into(), "a=b".into(), "log".into()],
            timeout_ms: 300,
        },
        BackendError::UnexpectedOutput {
            args: vec!["version".into()],
            detail: "\"hg version 6\"".into(),
        },
        BackendError::InvalidPath {
            path: "../x".into(),
            reason: "a `..` component".into(),
        },
        BackendError::InvalidLine {
            reason: "contains a newline".into(),
        },
        BackendError::Io {
            op: "reading".into(),
            path: "/srv/x/a.txt".into(),
            source: io::Error::new(io::ErrorKind::IsADirectory, "is a directory"),
        },
        BackendError::Io {
            op: "removing".into(),
            path: "/srv/x/.git/dv/r".into(),
            // Access denied, on Windows and on Unix.
            source: io::Error::from_raw_os_error(if cfg!(windows) { 5 } else { 13 }),
        },
        BackendError::Watch {
            path: "/srv/x".into(),
            message: "inotify watch limit reached".into(),
        },
        BackendError::InvalidProcess {
            program: "tsc".into(),
            reason: "the program must be an absolute path".into(),
        },
        BackendError::StartProcess {
            program: "/srv/x/node_modules/.bin/tsc".into(),
            source: io::Error::from_raw_os_error(2),
        },
    ]
}

#[test]
fn every_error_reads_the_same_after_the_wire() {
    for error in every_error() {
        let line = serde_json::to_string(&WireError::from(&error)).unwrap();
        let back = serde_json::from_str::<WireError>(&line)
            .unwrap()
            .into_backend("Ubuntu");
        assert_eq!(
            std::mem::discriminant(&back),
            std::mem::discriminant(&error),
            "{line}"
        );
        assert_eq!(back.to_string(), error.to_string(), "{line}");
        assert_eq!(back.detail(), error.detail(), "{line}");
        // The same wire form again: nothing was lost on the way.
        assert_eq!(
            serde_json::to_string(&WireError::from(&back)).unwrap(),
            line
        );
    }
}

#[test]
fn io_errors_keep_their_kind_and_message() {
    for kind in [
        io::ErrorKind::NotFound,
        io::ErrorKind::PermissionDenied,
        io::ErrorKind::IsADirectory,
        io::ErrorKind::NotADirectory,
        io::ErrorKind::StorageFull,
        io::ErrorKind::Other,
    ] {
        let error = io::Error::new(kind, "message");
        let back = io::Error::from(WireIoError::from(&error));
        assert_eq!(back.kind(), kind);
        assert_eq!(back.to_string(), "message");
    }
    let os = io::Error::from_raw_os_error(2);
    let back = io::Error::from(WireIoError::from(&os));
    assert_eq!(back.kind(), os.kind());
    assert_eq!(back.to_string(), os.to_string());
    // A kind this build doesn't know reads as `Other`, its message intact.
    let back = io::Error::from(WireIoError {
        kind: "SomethingNew".into(),
        message: "it broke".into(),
    });
    assert_eq!(back.kind(), io::ErrorKind::Other);
    assert_eq!(back.to_string(), "it broke");
}

#[test]
fn a_protocol_error_makes_the_repository_unavailable() {
    let error = WireError::protocol("no repository is open as handle 7").into_backend("Ubuntu");
    assert!(
        matches!(&error, BackendError::Unavailable { location, .. } if location == "Ubuntu"),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "Ubuntu can't be opened: dv there couldn't run the request: no repository is open as \
         handle 7"
    );
    let value = serde_json::to_value(WireError::protocol("bad")).unwrap();
    assert_eq!(value, json!({"kind": "protocol", "message": "bad"}));
}

#[test]
fn response_lines() {
    assert_eq!(ok_line(3, &()), b"{\"id\":3,\"ok\":null}\n");
    assert_eq!(
        ok_line(4, &Some(Bytes(b"x\ny".to_vec()))),
        b"{\"id\":4,\"ok\":\"eAp5\"}\n"
    );
    assert_eq!(
        err_line(0, &WireError::protocol("not a request")),
        b"{\"id\":0,\"err\":{\"kind\":\"protocol\",\"message\":\"not a request\"}}\n"
    );
    let event = WatchEvent {
        worktree_paths: vec!["a\nb.txt".into()],
        ..WatchEvent::default()
    };
    let line = watch_line(2, &event);
    assert_eq!(line.iter().filter(|&&b| b == b'\n').count(), 1);
    assert!(line.ends_with(b"\n"));

    // As the client reads them.
    let incoming: Incoming = serde_json::from_slice(&ok_line(3, &())).unwrap();
    assert_eq!(incoming.id, Some(3));
    assert!(incoming.ok.is_none() && incoming.err.is_none());
    let incoming: Incoming =
        serde_json::from_slice(&ok_line(5, &OpenReplyLike { repo: 1 })).unwrap();
    assert_eq!(incoming.ok.unwrap().get(), r#"{"repo":1}"#);
    let incoming: Incoming =
        serde_json::from_slice(&err_line(6, &WireError::protocol("x"))).unwrap();
    let error: WireError = serde_json::from_str(incoming.err.unwrap().get()).unwrap();
    assert_eq!(error, WireError::protocol("x"));
    let incoming: Incoming = serde_json::from_slice(&line).unwrap();
    assert_eq!((incoming.id, incoming.watch), (None, Some(2)));
    assert_eq!(incoming.event, Some(event));
}

#[test]
fn a_processs_lines_carry_its_id_and_read_back() {
    let out = proc_out_line(4, b"Content-Length: 2\r\n\r\n{}\x00\xff");
    assert_eq!(
        String::from_utf8(out.clone()).unwrap(),
        "{\"proc\":4,\"out\":\"Q29udGVudC1MZW5ndGg6IDINCg0Ke30A/w==\"}\n"
    );
    let err = proc_err_line(4, "warn: \"x\"\nnext");
    assert_eq!(err.iter().filter(|&&b| b == b'\n').count(), 1);
    assert_eq!(
        String::from_utf8(proc_exit_line(4, Some(3))).unwrap(),
        "{\"proc\":4,\"exit\":3}\n"
    );
    assert_eq!(
        String::from_utf8(proc_exit_line(4, None)).unwrap(),
        "{\"proc\":4,\"exit\":null}\n"
    );

    // As the client reads them: `exit` null and absent differ.
    let incoming: Incoming = serde_json::from_slice(&out).unwrap();
    assert_eq!(incoming.process, Some(4));
    assert_eq!(
        incoming.out.as_deref(),
        Some(&b"Content-Length: 2\r\n\r\n{}\x00\xff"[..])
    );
    assert_eq!(incoming.exit, None);
    let incoming: Incoming = serde_json::from_slice(&err).unwrap();
    let line: String = serde_json::from_str(incoming.err.unwrap().get()).unwrap();
    assert_eq!(line, "warn: \"x\"\nnext");
    let incoming: Incoming = serde_json::from_slice(&proc_exit_line(4, None)).unwrap();
    assert_eq!(incoming.exit, Some(None));
    let incoming: Incoming = serde_json::from_slice(&proc_exit_line(4, Some(0))).unwrap();
    assert_eq!(incoming.exit, Some(Some(0)));
}

#[test]
fn the_process_ops_read_as_sent() {
    let spec = ProcessSpec::new("/usr/bin/tsc")
        .args(["--lsp", "--stdio"])
        .cwd("packages/app")
        .env("NODE_OPTIONS", None);
    let request = Request {
        id: 9,
        op: Op::Spawn { repo: 1, spec },
    };
    let line = serde_json::to_string(&request).unwrap();
    assert_eq!(
        line,
        r#"{"id":9,"op":"spawn","repo":1,"spec":{"program":"/usr/bin/tsc","args":["--lsp","--stdio"],"cwd":"packages/app","env":[["NODE_OPTIONS",null]]}}"#
    );
    assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), request);
    let write: Request =
        serde_json::from_str(r#"{"id":10,"op":"proc_write","proc":2,"bytes":"aGk="}"#).unwrap();
    assert_eq!(
        write.op,
        Op::ProcWrite {
            process: 2,
            bytes: b"hi".to_vec()
        }
    );
    let kill: Request = serde_json::from_str(r#"{"id":11,"op":"proc_kill","proc":2}"#).unwrap();
    assert_eq!(kill.op, Op::ProcKill { process: 2 });
    // A spec needs only its program.
    let bare: Request =
        serde_json::from_str(r#"{"id":12,"op":"spawn","repo":1,"spec":{"program":"/bin/cat"}}"#)
            .unwrap();
    assert_eq!(
        bare.op,
        Op::Spawn {
            repo: 1,
            spec: ProcessSpec::new("/bin/cat")
        }
    );
    assert_eq!(
        serde_json::to_string(&SpawnReply {
            process: 2,
            pid: 77
        })
        .unwrap(),
        r#"{"proc":2,"pid":77}"#
    );
}

#[derive(serde::Serialize)]
struct OpenReplyLike {
    repo: u32,
}
