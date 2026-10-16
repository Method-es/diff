//! The connection's own behaviour: lazy start, the handshake, routing, death, and
//! ([`lifecycle`]) what follows a death; ([`rules`]) explicit and automatic starts, the idle
//! close, a reconnect that installs and a watch that can't come back as it was;
//! ([`bookkeeping`]) the count of respawn attempts, when the idle period starts, how fresh a
//! listing's host check is, and an explicit start during a reconnect. What a
//! `RemoteBackend` returns is the conformance suite's (`tests/conformance`); a real `dv backend
//! --stdio` child is `crates/dv/tests`'.

mod bookkeeping;
mod install;
mod lifecycle;
mod rules;

use std::io::{self, BufRead as _, BufReader, PipeReader, PipeWriter, Write as _};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use dv_core::backend::{Backend, GitOpts};
use dv_test_support::FixtureRepo;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::*;
use crate::launch::Transport;

fn committed() -> FixtureRepo {
    let repo = FixtureRepo::new();
    repo.write("a.txt", "a\n");
    repo.commit("base");
    repo
}

fn open(connection: &Arc<Connection>, repo: &FixtureRepo) -> RemoteBackend {
    connection
        .open(repo.path().to_str().unwrap(), repo.env_overlay())
        .unwrap()
}

/// A connection that, once its server dies, waits a minute before its first respawn: the
/// death's own effects stay in view.
fn patient(name: &str, launcher: Launcher) -> Arc<Connection> {
    Connection::with_backoff(name, launcher, vec![Duration::from_secs(60)])
}

/// A launcher whose server is `fake`, on a thread, reading requests and writing what it likes.
fn fake(
    fake: impl Fn(BufReader<PipeReader>, PipeWriter) + Send + Sync + Clone + 'static,
) -> Launcher {
    Launcher::custom("a fake server", move || {
        let (server_in, client_out) = io::pipe()?;
        let (client_in, server_out) = io::pipe()?;
        let fake = fake.clone();
        std::thread::spawn(move || fake(BufReader::new(server_in), server_out));
        Ok(Transport::new(client_in, client_out))
    })
}

/// The next request a fake server reads, `None` at the end of its input.
fn next_request(input: &mut BufReader<PipeReader>) -> Option<Value> {
    let mut line = String::new();
    match input.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(serde_json::from_str(&line).unwrap()),
    }
}

fn answer(output: &mut PipeWriter, line: Value) {
    output.write_all(format!("{line}\n").as_bytes()).unwrap();
}

/// What a fake server answers `hello` with.
fn hello_reply(id: &Value, proto: u32, build_ok: Value) -> Value {
    json!({"id": id, "ok": {
        "proto": proto, "build_ok": build_ok, "pid": 1, "os": "linux", "arch": "x86_64",
        "version": "0.0.0",
    }})
}

/// Every state `connection` reports from now on, on a channel.
fn states(connection: &Connection) -> (Subscription, Receiver<ConnectionState>) {
    let (tx, rx) = mpsc::channel();
    let subscription = connection.subscribe(move |state| {
        let _ = tx.send(state.clone());
    });
    (subscription, rx)
}

/// The states reported until one satisfies `last` (included), within `within`.
fn states_until(
    rx: &Receiver<ConnectionState>,
    within: Duration,
    last: impl Fn(&ConnectionState) -> bool,
) -> Vec<ConnectionState> {
    let deadline = Instant::now() + within;
    let mut seen = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(state) => {
                let done = last(&state);
                seen.push(state);
                if done {
                    return seen;
                }
            }
            Err(error) => panic!("no such state within {within:?} ({error:?}); saw {seen:?}"),
        }
    }
}

#[test]
fn the_server_starts_on_the_first_call_and_says_hello() {
    let repo = committed();
    let connection = Connection::new("in-process", Launcher::InProcess);
    assert_eq!(connection.state(), ConnectionState::Idle);
    assert_eq!(connection.server(), None);
    assert_eq!(connection.host(), Host::Local);
    assert_eq!(connection.generation(), 0);

    let backend = open(&connection, &repo);
    assert_eq!(connection.state(), ConnectionState::Connected);
    assert_eq!(connection.generation(), 1);
    let server = connection.server().unwrap();
    assert_eq!(server.proto, PROTO);
    assert_eq!(
        server.pid,
        std::process::id(),
        "a server on a thread of this test"
    );
    assert_eq!(server.build_ok, None);
    let output = backend.git(&["ls-files"], &GitOpts::default()).unwrap();
    assert_eq!(output.stdout_utf8(), Some("a.txt\n"));

    let diagnostics = connection.diagnostics();
    for line in [
        "connection: in-process\n",
        "launcher: dv's server on a thread of this process\n",
        "state: connected\n",
        "servers started: 1\n",
        "(protocol 2)",
        "history:\n",
        " s connecting\n",
        " s connected\n",
    ] {
        assert!(diagnostics.contains(line), "{line:?} in {diagnostics}");
    }
    assert!(
        diagnostics.contains(&format!("answered: {} ", std::env::consts::OS)),
        "{diagnostics}"
    );
}

#[test]
fn many_threads_share_one_connection() {
    let repo = committed();
    let connection = Connection::new("in-process", Launcher::InProcess);
    let backend = Arc::new(open(&connection, &repo));
    let threads: Vec<_> = (0..8)
        .map(|n| {
            let backend = Arc::clone(&backend);
            std::thread::spawn(move || {
                for _ in 0..20 {
                    let rel = format!("f{n}.txt");
                    assert_eq!(backend.worktree_len(&rel).unwrap(), None);
                    assert_eq!(backend.worktree_read("a.txt").unwrap().unwrap(), b"a\n");
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}

#[test]
fn concurrent_first_calls_start_one_server() {
    let starts = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let launcher = Launcher::custom("counted", {
        let starts = Arc::clone(&starts);
        move || {
            starts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Launcher::InProcess.start()
        }
    });
    let connection = Connection::new("in-process", launcher);
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let connection = Arc::clone(&connection);
            std::thread::spawn(move || connection.connect().unwrap())
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(starts.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn the_end_of_the_servers_output_is_its_death() {
    // A server that answers `hello` and `open`, then stops at the next request.
    let launcher = fake(|mut input, mut output| {
        while let Some(request) = next_request(&mut input) {
            match request["op"].as_str() {
                Some("hello") => {
                    answer(&mut output, hello_reply(&request["id"], PROTO, Value::Null))
                }
                Some("open") => answer(
                    &mut output,
                    json!({"id": request["id"], "ok": {"repo": 1, "info": {
                        "location": {"local": "/srv/x"}, "toplevel": "/srv/x",
                        "git_dir": "/srv/x/.git", "common_dir": "/srv/x/.git",
                        "object_format": "sha1",
                        "git_version": {"major": 2, "minor": 43, "patch": 0, "text": "2.43.0"},
                    }}}),
                ),
                _ => return,
            }
        }
    });
    let connection = patient("Fake", launcher);
    let backend = connection.open("/srv/x", Vec::new()).unwrap();
    assert_eq!(backend.repo().toplevel, "/srv/x");
    let error = backend.store_list("").unwrap_err();
    assert!(
        matches!(&error, BackendError::Disconnected { reason, .. } if reason == "dv's output ended"),
        "{error:?}"
    );
    assert_eq!(
        connection.state(),
        ConnectionState::Reconnecting { attempt: 1 }
    );
    assert_eq!(connection.server(), None);
    assert!(
        connection
            .diagnostics()
            .contains(" s dv died: dv's output ended\n"),
        "{}",
        connection.diagnostics()
    );
}

#[test]
fn noise_before_the_handshake_is_kept_and_after_it_is_a_death() {
    let launcher = fake(|mut input, mut output| {
        output
            .write_all(b"wsl: a warning before dv starts\n\n{\"not\": \"ours\"}\n")
            .unwrap();
        while let Some(request) = next_request(&mut input) {
            match request["op"].as_str() {
                Some("hello") => {
                    answer(&mut output, hello_reply(&request["id"], PROTO, Value::Null))
                }
                _ => output.write_all(b"garbage after the handshake\n").unwrap(),
            }
        }
    });
    let connection = patient("Fake", launcher);
    connection.connect().unwrap();
    let diagnostics = connection.diagnostics();
    assert!(
        diagnostics.contains("stdout: wsl: a warning before dv starts"),
        "{diagnostics}"
    );
    assert!(
        diagnostics.contains(r#"stdout: {"not": "ours"}"#),
        "{diagnostics}"
    );

    let error = connection.open("/srv/x", Vec::new()).unwrap_err();
    let BackendError::Disconnected { reason, .. } = &error else {
        panic!("{error:?}");
    };
    assert!(
        reason.starts_with("dv printed a line that isn't the protocol"),
        "{reason}"
    );
    assert!(reason.ends_with("garbage after the handshake"), "{reason}");
    assert_eq!(
        connection.state(),
        ConnectionState::Reconnecting { attempt: 1 }
    );
}

#[test]
fn a_reply_to_nothing_or_of_the_wrong_shape_is_a_death() {
    let launcher = fake(|mut input, mut output| {
        while let Some(request) = next_request(&mut input) {
            match request["op"].as_str() {
                Some("hello") => {
                    answer(&mut output, hello_reply(&request["id"], PROTO, Value::Null))
                }
                Some("open") => answer(&mut output, json!({"id": request["id"], "ok": [1, 2]})),
                _ => answer(&mut output, json!({"id": 999, "ok": null})),
            }
        }
    });
    let connection = patient("Fake", launcher.clone());
    let error = connection.open("/srv/x", Vec::new()).unwrap_err();
    let BackendError::Disconnected { reason, .. } = &error else {
        panic!("{error:?}");
    };
    assert!(
        reason.starts_with("dv answered open with something else"),
        "{reason}"
    );

    let connection = patient("Fake", launcher);
    connection.connect().unwrap();
    // Not `open`: the fake answers an id nobody sent.
    let link = connection.link().unwrap();
    let error = link
        .call::<Value>(Op::Close { repo: 1 }, Some(Duration::from_secs(10)))
        .unwrap_err();
    assert_eq!(
        error,
        CallError::Dead("dv answered a request it wasn't sent (id 999)".into())
    );
}

#[test]
fn a_server_of_another_protocol_or_build_is_refused() {
    let launcher = |proto: u32, build_ok: Value| {
        fake(move |mut input, mut output| {
            while let Some(request) = next_request(&mut input) {
                answer(
                    &mut output,
                    hello_reply(&request["id"], proto, build_ok.clone()),
                );
            }
        })
    };
    let connection = Connection::new("Ubuntu", launcher(PROTO + 1, Value::Null));
    let error = connection.connect().unwrap_err();
    assert_eq!(
        error.to_string(),
        "Ubuntu can't be opened: the connection to dv failed: dv in Ubuntu speaks protocol 3; \
         this app speaks 2"
    );
    assert!(matches!(connection.state(), ConnectionState::Failed { .. }));
    // An older one, asked for no build (a loopback), alike.
    let connection = Connection::new("loopback", launcher(PROTO - 1, Value::Null));
    let error = connection.connect().unwrap_err();
    assert!(
        error
            .to_string()
            .ends_with("dv in loopback speaks protocol 1; this app speaks 2"),
        "{error}"
    );

    let connection = Connection::new("Ubuntu", launcher(PROTO, json!(false)));
    let error = connection.connect().unwrap_err();
    assert_eq!(
        connection.state(),
        ConnectionState::Failed {
            error: "the dv in Ubuntu doesn't match this app".into()
        }
    );
    assert!(
        error.to_string().ends_with("doesn't match this app"),
        "{error}"
    );
    // The refused server is in the history, pid and all.
    assert!(
        connection
            .diagnostics()
            .contains(" s dv 0.0.0 answered: linux x86_64, pid 1\n"),
        "{}",
        connection.diagnostics()
    );
}

#[test]
fn a_server_that_cant_start_fails_the_connection() {
    let connection = Connection::new(
        "Nowhere",
        Launcher::custom("a launcher that fails", || {
            Err(io::Error::new(io::ErrorKind::NotFound, "no wsl.exe"))
        }),
    );
    let error = connection.connect().unwrap_err();
    assert!(
        matches!(error, BackendError::Unavailable { .. }),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "Nowhere can't be opened: the connection to dv failed: couldn't start dv: no wsl.exe"
    );
    assert!(
        connection
            .diagnostics()
            .contains("launcher: a launcher that fails")
    );
    // Calls fail the same way without starting anything.
    let error = connection.open("/srv/x", Vec::new()).unwrap_err();
    assert!(
        matches!(error, BackendError::Unavailable { .. }),
        "{error:?}"
    );
}

#[test]
fn a_process_that_isnt_dv_is_kept_in_the_diagnostics_and_its_exit_is_its_death() {
    // git prints its version on stdout, not the protocol, and exits before answering.
    let connection = Connection::new(
        "git",
        Launcher::Command {
            program: "git".into(),
            args: vec!["--version".into()],
            env: Vec::new(),
            build: None,
        },
    );
    let error = connection.connect().unwrap_err();
    let ConnectionState::Failed { error: reason } = connection.state() else {
        panic!("{error:?}");
    };
    // Its exit, its output's end or the write of `hello` into a closed pipe: whichever the
    // client meets first.
    assert!(
        reason.starts_with("dv exited (")
            || reason == "dv's output ended"
            || reason.starts_with("writing to dv failed"),
        "{reason}"
    );
    let diagnostics = connection.diagnostics();
    assert!(
        diagnostics.contains("launcher: git --version\n"),
        "{diagnostics}"
    );
    // The reader may meet git's line after the call failed.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !connection.diagnostics().contains("stdout: git version ") {
        assert!(Instant::now() < deadline, "{}", connection.diagnostics());
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_wsl_hosts_repositories_are_wsl_locations() {
    let info = RepoInfo {
        location: Location::Local("/home/kyle/api".into()),
        toplevel: "/home/kyle/api".into(),
        git_dir: "/home/kyle/api/.git".into(),
        common_dir: "/home/kyle/api/.git".into(),
        object_format: dv_core::ObjectFormat::Sha1,
        git_version: dv_core::backend::GitVersion::parse("git version 2.43.0").unwrap(),
    };
    let wsl = located(
        &Host::Wsl {
            distro: "Ubuntu".into(),
        },
        info.clone(),
    );
    assert_eq!(
        wsl.location,
        Location::Wsl {
            distro: "Ubuntu".into(),
            path: "/home/kyle/api".into()
        }
    );
    assert_eq!(
        RepoInfo {
            location: info.location.clone(),
            ..wsl
        },
        info
    );
    assert_eq!(located(&Host::Local, info.clone()), info);
}

#[test]
fn the_registry_keeps_one_connection_per_server() {
    let connections = Connections::new();
    assert!(connections.all().is_empty());
    let first = connections.loopback("loopback", || Launcher::InProcess);
    let again = connections.loopback("loopback", || panic!("made once"));
    assert!(Arc::ptr_eq(&first, &again));
    assert_eq!(first.name(), "loopback");
    assert_eq!(first.state(), ConnectionState::Idle, "nothing starts");

    // Distro names fold case, as wsl.exe takes them.
    assert_eq!(ConnectionKey::wsl("Ubuntu"), ConnectionKey::wsl("ubuntu"));
    let ubuntu = connections.get_or_insert_with(ConnectionKey::wsl("Ubuntu"), || {
        Connection::new("Ubuntu", Launcher::InProcess)
    });
    let same = connections.get_or_insert_with(ConnectionKey::wsl("UBUNTU"), || panic!("made once"));
    assert!(Arc::ptr_eq(&ubuntu, &same));
    assert_eq!(same.name(), "Ubuntu", "named as first asked");
    assert!(
        connections
            .get(&ConnectionKey::loopback("loopback"))
            .is_some()
    );
    assert!(connections.get(&ConnectionKey::wsl("Debian")).is_none());

    // A clone shares the registry; listed WSL first, then loopbacks.
    let names: Vec<String> = connections
        .clone()
        .all()
        .iter()
        .map(|connection| connection.name().to_string())
        .collect();
    assert_eq!(names, ["Ubuntu", "loopback"]);
}
