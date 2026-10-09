//! When a server starts (PLAN §5.2's connecting rule): only an explicit start
//! (`Connection::retry`) boots a stopped host; every automatic one — a call finding no server,
//! a respawn after a death — asks the launcher's host check first and launches nothing on a
//! stopped host. A server unused for the idle period closes and the next call starts another.
//! A reconnect that installs fails calls at once. A watch the new server can't take back as it
//! was ends, so its consumer starts another.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::RecvTimeoutError;

use dv_core::backend::{WatchEvent, WatchHandle, WatchSpec};
use pretty_assertions::assert_eq;

use super::*;
use crate::launch::StartFn;

const WITHIN: Duration = Duration::from_secs(10);

/// An in-process server on a host a test stops (a stand-in for a distro's running state, read
/// by the launcher's host check), counting the servers launched.
#[derive(Clone)]
struct StoppableHost {
    up: Arc<AtomicBool>,
    launches: Arc<AtomicU32>,
}

impl StoppableHost {
    fn running(up: bool) -> Self {
        Self {
            up: Arc::new(AtomicBool::new(up)),
            launches: Arc::default(),
        }
    }

    fn launcher(&self) -> Launcher {
        let host = self.clone();
        Launcher::custom("an in-process server on a host the test stops", move || {
            host.launches.fetch_add(1, Ordering::SeqCst);
            // A launch boots the host, as `wsl.exe -d` boots a distro.
            host.up.store(true, Ordering::SeqCst);
            Launcher::InProcess.start()
        })
        .with_host_check({
            let up = Arc::clone(&self.up);
            move |_| up.load(Ordering::SeqCst)
        })
    }

    /// The user stops it (`wsl --shutdown`).
    fn stop(&self) {
        self.up.store(false, Ordering::SeqCst);
    }

    fn launches(&self) -> u32 {
        self.launches.load(Ordering::SeqCst)
    }
}

fn stopped(name: &str) -> String {
    format!("{name} can't be opened: {name} isn't running")
}

/// A watch whose events arrive on a channel, which closes when its sink is dropped.
fn watch(backend: &dyn Backend, spec: WatchSpec) -> (WatchHandle, Receiver<WatchEvent>) {
    let (tx, rx) = mpsc::channel();
    let handle = backend
        .watch(
            spec,
            Box::new(move |event| {
                let _ = tx.send(event);
            }),
        )
        .unwrap();
    (handle, rx)
}

#[test]
fn a_death_on_a_stopped_host_launches_nothing_and_ends_stopped_until_an_explicit_start() {
    let repo = committed();
    let host = StoppableHost::running(true);
    let connection = Connection::with_backoff(
        "Ubuntu",
        host.launcher(),
        vec![Duration::from_millis(50); 3],
    );
    let (_subscription, rx) = states(&connection);
    let backend = open(&connection, &repo);
    states_until(&rx, WITHIN, |state| *state == ConnectionState::Connected);
    assert_eq!(host.launches(), 1);

    // The user stops the distro, and its server dies with it.
    host.stop();
    connection.kill();
    assert_eq!(
        states_until(&rx, WITHIN, |state| *state == ConnectionState::Stopped),
        [
            ConnectionState::Reconnecting { attempt: 1 },
            ConnectionState::Stopped
        ]
    );
    assert_eq!(host.launches(), 1, "the respawn launched nothing");
    assert!(
        connection
            .diagnostics()
            .contains(" s attempt 1: Ubuntu isn't running, so dv wasn't started\n"),
        "{}",
        connection.diagnostics()
    );
    // Calls fail at once, saying why, and start nothing; nothing starts by itself.
    let error = backend.worktree_len("a.txt").unwrap_err();
    assert!(
        matches!(error, BackendError::Unavailable { .. }),
        "{error:?}"
    );
    assert_eq!(error.to_string(), stopped("Ubuntu"));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(host.launches(), 1);
    assert_eq!(connection.state(), ConnectionState::Stopped);

    // The explicit start boots it, and the backend works on the new server.
    connection.retry().unwrap();
    assert_eq!(host.launches(), 2);
    assert_eq!(connection.state(), ConnectionState::Connected);
    assert_eq!(backend.worktree_len("a.txt").unwrap(), Some(2));
}

#[test]
fn a_death_on_a_running_host_respawns() {
    let repo = committed();
    let host = StoppableHost::running(true);
    let connection = Connection::with_backoff(
        "Ubuntu",
        host.launcher(),
        vec![Duration::from_millis(50); 3],
    );
    let (_subscription, rx) = states(&connection);
    let backend = open(&connection, &repo);
    states_until(&rx, WITHIN, |state| *state == ConnectionState::Connected);
    connection.kill();
    assert_eq!(
        states_until(&rx, WITHIN, |state| *state == ConnectionState::Connected),
        [
            ConnectionState::Reconnecting { attempt: 1 },
            ConnectionState::Connected
        ]
    );
    assert_eq!(host.launches(), 2);
    assert_eq!(backend.worktree_len("a.txt").unwrap(), Some(2));
}

#[test]
fn a_first_call_on_a_stopped_host_launches_nothing_and_an_explicit_start_does() {
    let repo = committed();
    let host = StoppableHost::running(false);
    let connection = Connection::new("Ubuntu", host.launcher());
    let error = connection.connect().unwrap_err();
    assert_eq!(error.to_string(), stopped("Ubuntu"));
    let error = connection
        .open(repo.path().to_str().unwrap(), repo.env_overlay())
        .unwrap_err();
    assert_eq!(error.to_string(), stopped("Ubuntu"));
    assert_eq!(connection.state(), ConnectionState::Stopped);
    assert_eq!(host.launches(), 0);
    assert_eq!(connection.generation(), 0);

    connection.retry().unwrap();
    assert_eq!(host.launches(), 1);
    let backend = open(&connection, &repo);
    assert_eq!(backend.worktree_len("a.txt").unwrap(), Some(2));
    // Connected, an explicit start has nothing to do.
    connection.retry().unwrap();
    assert_eq!(host.launches(), 1);
}

#[test]
fn an_unused_server_closes_and_the_next_call_starts_another_automatically() {
    let repo = committed();
    let host = StoppableHost::running(true);
    let idle = Duration::from_millis(300);
    let connection = Connection::with_options(
        "Ubuntu",
        host.launcher(),
        ConnectionOptions {
            backoff: vec![Duration::from_millis(50); 3],
            idle: Some(idle),
        },
    );
    let (_subscription, rx) = states(&connection);
    let backend = open(&connection, &repo);
    assert_eq!(
        states_until(&rx, WITHIN, |state| *state == ConnectionState::Idle),
        [
            ConnectionState::Connecting,
            ConnectionState::Connected,
            ConnectionState::Idle
        ],
        "closed once unused"
    );
    assert_eq!(connection.server(), None);
    assert!(
        connection
            .diagnostics()
            .contains(" s dv closed after 0.3 s unused\n"),
        "{}",
        connection.diagnostics()
    );

    // The backend stays valid: its next call starts a server and opens its worktree there.
    assert_eq!(backend.worktree_len("a.txt").unwrap(), Some(2));
    assert_eq!(host.launches(), 2);
    assert_eq!(connection.generation(), 2);

    // A watch keeps the server up past the idle period.
    let (handle, events) = watch(&backend, WatchSpec::WORKTREE);
    std::thread::sleep(idle * 3);
    assert_eq!(connection.state(), ConnectionState::Connected);
    assert_eq!(connection.generation(), 2);
    repo.write("watched.txt", "x\n");
    let deadline = Instant::now() + WITHIN;
    loop {
        let event = events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("the watch follows the worktree");
        if event
            .worktree_paths
            .iter()
            .any(|path| path == "watched.txt")
        {
            break;
        }
    }

    // Unwatched, it closes again; with its host stopped meanwhile, the next call starts
    // nothing.
    drop(handle);
    states_until(&rx, WITHIN, |state| *state == ConnectionState::Idle);
    host.stop();
    let error = backend.worktree_len("a.txt").unwrap_err();
    assert_eq!(error.to_string(), stopped("Ubuntu"));
    assert_eq!(connection.state(), ConnectionState::Stopped);
    assert_eq!(host.launches(), 2);
}

#[test]
fn calls_fail_at_once_while_a_reconnect_installs_the_build() {
    let repo = committed();
    let (release, released) = mpsc::channel::<()>();
    let released = Arc::new(Mutex::new(released));
    let starts = Arc::new(AtomicU32::new(0));
    let start: Arc<StartFn> = Arc::new({
        let starts = Arc::clone(&starts);
        move |options: &StartOptions<'_>| {
            if starts.fetch_add(1, Ordering::SeqCst) > 0 {
                // The respawn finds its build gone and copies it, which takes until the test
                // lets it finish.
                (options.progress)(0, 10);
                let _ = released.lock().recv_timeout(WITHIN);
                (options.progress)(10, 10);
            }
            Ok(Launcher::InProcess.start()?)
        }
    });
    let launcher = Launcher::Custom {
        label: "a launcher that installs on a respawn".into(),
        start,
        reinstalls: true,
        running: None,
    };
    let connection =
        Connection::with_backoff("Ubuntu", launcher, vec![Duration::from_millis(20); 3]);
    let (_subscription, rx) = states(&connection);
    let backend = Arc::new(open(&connection, &repo));
    connection.kill();
    states_until(&rx, WITHIN, |state| {
        matches!(state, ConnectionState::Installing { .. })
    });

    let (tx, done) = mpsc::channel();
    std::thread::spawn({
        let backend = Arc::clone(&backend);
        move || {
            let started = Instant::now();
            let result = backend.worktree_len("a.txt");
            let _ = tx.send((result, started.elapsed()));
        }
    });
    let (result, took) = done.recv_timeout(WITHIN * 2).expect("the call ends");
    assert!(
        took < Duration::from_secs(1),
        "at once, not when the install ends: {took:?}"
    );
    let error = result.unwrap_err();
    assert!(
        matches!(error, BackendError::Disconnected { .. }),
        "{error:?}"
    );
    assert_eq!(
        connection.state(),
        ConnectionState::Installing { done: 0, total: 10 },
        "the install's progress is the state"
    );

    release.send(()).unwrap();
    states_until(&rx, WITHIN, |state| *state == ConnectionState::Connected);
    assert_eq!(backend.worktree_len("a.txt").unwrap(), Some(2));
}

/// A fake server whose watches answer `respawned` (`{"ok": …}` or `{"err": …}`, the id added)
/// on every server after the first, and `{"watch": 1, "worktree_error": null}` on the first.
fn watching(respawned: Value) -> Launcher {
    let starts = Arc::new(AtomicU32::new(0));
    Launcher::custom("a fake server whose watches change", move || {
        let respawn = starts.fetch_add(1, Ordering::SeqCst) > 0;
        let respawned = respawned.clone();
        fake(move |mut input, mut output| {
            while let Some(request) = next_request(&mut input) {
                let id = request["id"].clone();
                match request["op"].as_str() {
                    Some("hello") => answer(&mut output, hello_reply(&id, PROTO, Value::Null)),
                    Some("open") => answer(
                        &mut output,
                        json!({"id": id, "ok": {"repo": 1, "info": {
                            "location": {"local": "/srv/x"}, "toplevel": "/srv/x",
                            "git_dir": "/srv/x/.git", "common_dir": "/srv/x/.git",
                            "object_format": "sha1",
                            "git_version": {"major": 2, "minor": 43, "patch": 0, "text": "2.43.0"},
                        }}}),
                    ),
                    Some("watch") if respawn => {
                        let mut reply = respawned.clone();
                        reply["id"] = id;
                        answer(&mut output, reply);
                    }
                    Some("watch") => answer(
                        &mut output,
                        json!({"id": id, "ok": {"watch": 1, "worktree_error": null}}),
                    ),
                    _ => {}
                }
            }
        })
        .start()
    })
}

#[test]
fn a_watch_the_new_server_takes_differently_ends_and_a_new_one_says_what_it_covers() {
    for (respawned, case) in [
        (
            json!({"ok": {"watch": 1, "worktree_error": "inotify's watch limit"}}),
            "the store alone",
        ),
        (
            json!({"err": {"kind": "path_not_found", "path": "/srv/x"}}),
            "refused",
        ),
    ] {
        let connection = Connection::with_backoff(
            "Fake",
            watching(respawned),
            vec![Duration::from_millis(20); 3],
        );
        let (_subscription, rx) = states(&connection);
        let backend = connection.open("/srv/x", Vec::new()).unwrap();
        let (handle, events) = watch(&backend, WatchSpec::ALL);
        assert_eq!(handle.worktree_error(), None, "{case}");

        connection.kill();
        states_until(&rx, WITHIN, |state| *state == ConnectionState::Connected);
        // The watch ended: no rescan, its sink dropped (the channel closes), and the
        // connection subscribes it no more.
        assert_eq!(
            events.recv_timeout(WITHIN).err(),
            Some(RecvTimeoutError::Disconnected),
            "{case}"
        );
        assert!(connection.watches.lock().is_empty(), "{case}");

        // The consumer's new watch says what the server watches now.
        match backend.watch(WatchSpec::ALL, Box::new(|_| {})) {
            Ok(again) => assert_eq!(
                again.worktree_error(),
                Some("inotify's watch limit"),
                "{case}"
            ),
            Err(error) => assert!(
                matches!(error, BackendError::PathNotFound { .. }),
                "{case}: {error:?}"
            ),
        }
        drop(handle);
    }
}

#[test]
fn a_stopped_connection_resumes_automatically_only_once_its_host_runs() {
    let host = StoppableHost::running(false);
    let connection = Connection::new("Ubuntu", host.launcher());
    assert!(connection.connect().is_err());
    assert_eq!(connection.state(), ConnectionState::Stopped);
    // Still stopped: nothing starts.
    assert_eq!(
        connection.resume().unwrap_err().to_string(),
        stopped("Ubuntu")
    );
    assert_eq!(host.launches(), 0);
    // Started outside dv: an automatic start goes ahead.
    host.up.store(true, Ordering::SeqCst);
    connection.resume().unwrap();
    assert_eq!(connection.state(), ConnectionState::Connected);
    assert_eq!(host.launches(), 1);
    // Connected, it is a plain connect.
    connection.resume().unwrap();
    assert_eq!(host.launches(), 1);
}
