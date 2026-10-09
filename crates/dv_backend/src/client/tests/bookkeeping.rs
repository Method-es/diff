//! The connection's bookkeeping (PLAN §5.2): which starts continue the count of respawn
//! attempts and which begin a fresh one, when the idle period starts, how fresh a host check a
//! listing asks for, and an explicit start made while a reconnect is under way.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use dv_core::backend::{WatchHandle, WatchSpec};
use pretty_assertions::assert_eq;

use super::*;

const WITHIN: Duration = Duration::from_secs(10);

/// An in-process server on a host a test may stop, whose starts it may block, counting the
/// servers launched and recording how fresh each host check was asked for.
#[derive(Clone)]
struct Host {
    up: Arc<AtomicBool>,
    blocked: Arc<AtomicBool>,
    launches: Arc<AtomicU32>,
    asked: Arc<Mutex<Vec<Asked>>>,
}

impl Host {
    fn new(up: bool) -> Self {
        Self {
            up: Arc::new(AtomicBool::new(up)),
            blocked: Arc::default(),
            launches: Arc::default(),
            asked: Arc::default(),
        }
    }

    fn launcher(&self) -> Launcher {
        let host = self.clone();
        Launcher::custom(
            "an in-process server on a host the test controls",
            move || {
                host.launches.fetch_add(1, Ordering::SeqCst);
                if host.blocked.load(Ordering::SeqCst) {
                    return Err(io::Error::other("blocked by the test"));
                }
                host.up.store(true, Ordering::SeqCst);
                Launcher::InProcess.start()
            },
        )
        .with_host_check({
            let host = self.clone();
            move |asked| {
                host.asked.lock().push(asked);
                host.up.load(Ordering::SeqCst)
            }
        })
    }

    fn launches(&self) -> u32 {
        self.launches.load(Ordering::SeqCst)
    }

    /// The host checks asked since the last call.
    fn asked(&self) -> Vec<Asked> {
        std::mem::take(&mut *self.asked.lock())
    }
}

fn paced(host: &Host, backoff: Duration, idle: Option<Duration>) -> Arc<Connection> {
    Connection::with_options(
        "Ubuntu",
        host.launcher(),
        ConnectionOptions {
            backoff: vec![backoff; 3],
            idle,
        },
    )
}

/// Kills the server and waits for the reconnect to bring another.
fn kill_and_respawn(connection: &Connection, rx: &Receiver<ConnectionState>) {
    connection.kill();
    states_until(rx, WITHIN, |state| {
        matches!(state, ConnectionState::Reconnecting { .. })
    });
    states_until(rx, WITHIN, |state| *state == ConnectionState::Connected);
}

#[test]
fn a_server_after_an_idle_close_starts_a_fresh_count_of_attempts() {
    for explicit in [false, true] {
        let host = Host::new(true);
        let connection = paced(
            &host,
            Duration::from_millis(20),
            Some(Duration::from_millis(400)),
        );
        let (_subscription, rx) = states(&connection);
        connection.connect().unwrap();
        // Three servers that each died soon after their respawn: the count stands at 3.
        for _ in 0..3 {
            kill_and_respawn(&connection, &rx);
        }
        assert_eq!(host.launches(), 4);
        states_until(&rx, WITHIN, |state| *state == ConnectionState::Idle);

        // A new server, started automatically or explicitly, is no respawn: its first death
        // reconnects.
        if explicit {
            connection.retry().unwrap();
        } else {
            connection.connect().unwrap();
        }
        states_until(&rx, WITHIN, |state| *state == ConnectionState::Connected);
        connection.kill();
        assert_eq!(
            states_until(&rx, WITHIN, |state| *state == ConnectionState::Connected),
            [
                ConnectionState::Reconnecting { attempt: 1 },
                ConnectionState::Connected
            ],
            "explicit: {explicit}"
        );
    }
}

#[test]
fn the_idle_period_starts_when_the_last_watch_goes() {
    let repo = committed();
    let host = Host::new(true);
    let idle = Duration::from_millis(600);
    let connection = paced(&host, Duration::from_millis(20), Some(idle));
    let (_subscription, rx) = states(&connection);
    let backend = open(&connection, &repo);
    let handle: WatchHandle = backend
        .watch(WatchSpec::WORKTREE, Box::new(|_| {}))
        .unwrap();
    // A long quiet stretch: no calls while the review is read.
    std::thread::sleep(idle * 5 / 2);
    assert_eq!(connection.state(), ConnectionState::Connected);
    drop(handle);
    let dropped = Instant::now();
    states_until(&rx, WITHIN, |state| *state == ConnectionState::Idle);
    let after = dropped.elapsed();
    assert!(
        after + Duration::from_millis(50) >= idle,
        "closed {after:?} after the last watch went; the idle period is {idle:?}"
    );
}

#[test]
fn a_listing_asks_a_stopped_hosts_state_from_the_cache_and_a_start_afresh() {
    let host = Host::new(false);
    let connection = paced(&host, Duration::from_millis(20), None);
    // The first call is an automatic start: it asks afresh.
    assert!(connection.connect().is_err());
    assert_eq!(connection.state(), ConnectionState::Stopped);
    assert_eq!(host.asked(), [Asked::Fresh]);
    // Every listing of the stopped connection takes the cached answer.
    for _ in 0..3 {
        assert!(connection.resume().is_err());
    }
    assert_eq!(connection.host_running(Asked::Listing), Some(false));
    assert_eq!(host.asked(), [Asked::Listing; 4]);
    assert_eq!(host.launches(), 0);
    // Started outside dv: a listing resumes it; a respawn after a death asks afresh again.
    host.up.store(true, Ordering::SeqCst);
    connection.resume().unwrap();
    assert_eq!(host.asked(), [Asked::Listing]);
    let (_subscription, rx) = states(&connection);
    kill_and_respawn(&connection, &rx);
    assert_eq!(host.asked(), [Asked::Fresh]);
}

#[test]
fn an_explicit_start_during_a_reconnect_waits_for_it_and_takes_its_outcome() {
    let repo = committed();
    // Connected again: the explicit start succeeds once the reconnect has, starting nothing
    // more itself.
    let host = Host::new(true);
    let backoff = Duration::from_millis(300);
    let connection = paced(&host, backoff, None);
    let (_subscription, rx) = states(&connection);
    let backend = open(&connection, &repo);
    connection.kill();
    states_until(&rx, WITHIN, |state| {
        matches!(state, ConnectionState::Reconnecting { .. })
    });
    let started = Instant::now();
    connection.retry().unwrap();
    assert!(
        started.elapsed() + Duration::from_millis(100) >= backoff,
        "it waited for the reconnect: {:?}",
        started.elapsed()
    );
    assert_eq!(connection.state(), ConnectionState::Connected);
    assert_eq!(host.launches(), 2);
    assert_eq!(backend.worktree_len("a.txt").unwrap(), Some(2));

    // Failed for good: the explicit start fails as calls do, with why, and adds no attempt.
    host.blocked.store(true, Ordering::SeqCst);
    connection.kill();
    states_until(&rx, WITHIN, |state| {
        matches!(state, ConnectionState::Reconnecting { .. })
    });
    let error = connection.retry().unwrap_err();
    assert_eq!(
        error.to_string(),
        "Ubuntu can't be opened: the connection to dv failed: couldn't start dv: blocked by the \
         test"
    );
    assert!(matches!(connection.state(), ConnectionState::Failed { .. }));
    // The reconnect's attempts 2 and 3 (the last server died soon after its respawn), none of
    // retry's own.
    assert_eq!(host.launches(), 4);

    // Stopped by the user: the explicit start doesn't boot it behind the reconnect's back.
    host.blocked.store(false, Ordering::SeqCst);
    connection.retry().unwrap();
    host.up.store(false, Ordering::SeqCst);
    connection.kill();
    states_until(&rx, WITHIN, |state| {
        matches!(state, ConnectionState::Reconnecting { .. })
    });
    let error = connection.retry().unwrap_err();
    assert_eq!(
        error.to_string(),
        "Ubuntu can't be opened: Ubuntu isn't running"
    );
    assert_eq!(connection.state(), ConnectionState::Stopped);
    assert_eq!(host.launches(), 5);
}

#[test]
fn an_explicit_start_that_waits_out_a_reconnect_returns_once_the_watches_are_back() {
    // A fake server whose watches take a while to answer on every server after the first, and
    // which says when it answers one.
    let starts = Arc::new(AtomicU32::new(0));
    let answered = Arc::new(AtomicBool::new(false));
    let launcher = Launcher::custom("a fake server slow to watch again", {
        let answered = Arc::clone(&answered);
        move || {
            let respawn = starts.fetch_add(1, Ordering::SeqCst) > 0;
            let answered = Arc::clone(&answered);
            fake(move |mut input, mut output| {
                while let Some(request) = next_request(&mut input) {
                    let id = request["id"].clone();
                    match request["op"].as_str() {
                        Some("hello") => {
                            answer(&mut output, hello_reply(&id, PROTO, Value::Null));
                        }
                        Some("open") => answer(
                            &mut output,
                            json!({"id": id, "ok": {"repo": 1, "info": {
                                "location": {"local": "/srv/x"}, "toplevel": "/srv/x",
                                "git_dir": "/srv/x/.git", "common_dir": "/srv/x/.git",
                                "object_format": "sha1",
                                "git_version": {
                                    "major": 2, "minor": 43, "patch": 0, "text": "2.43.0"
                                },
                            }}}),
                        ),
                        Some("watch") => {
                            if respawn {
                                std::thread::sleep(Duration::from_millis(400));
                                answered.store(true, Ordering::SeqCst);
                            }
                            answer(
                                &mut output,
                                json!({"id": id, "ok": {"watch": 1, "worktree_error": null}}),
                            );
                        }
                        _ => {}
                    }
                }
            })
            .start()
        }
    });
    let connection = Connection::with_options(
        "Fake",
        launcher,
        ConnectionOptions {
            backoff: vec![Duration::from_millis(100); 3],
            idle: None,
        },
    );
    let (_subscription, rx) = states(&connection);
    let backend = connection.open("/srv/x", Vec::new()).unwrap();
    let (events, delivered) = mpsc::channel();
    let _handle: WatchHandle = backend
        .watch(
            WatchSpec::ALL,
            Box::new(move |event| {
                let _ = events.send(event);
            }),
        )
        .unwrap();

    connection.kill();
    states_until(&rx, WITHIN, |state| {
        matches!(state, ConnectionState::Reconnecting { .. })
    });
    connection.retry().unwrap();
    // The reconnect is connected before its watch is back; retry waited for the watch.
    assert!(
        answered.load(Ordering::SeqCst),
        "retry returned before the watch subscribed again"
    );
    let rescan = delivered.recv_timeout(WITHIN).unwrap();
    assert!(rescan.rescan, "{rescan:?}");
}

#[test]
fn waiting_for_a_reconnect_starts_nothing() {
    let repo = committed();
    let host = Host::new(true);
    let backoff = Duration::from_millis(300);
    let connection = paced(&host, backoff, None);
    let (_subscription, rx) = states(&connection);
    let backend = open(&connection, &repo);
    // Connected: nothing to wait for.
    connection.wait_for_reconnect().unwrap();
    assert_eq!(host.launches(), 1);

    // Reconnecting: it waits for the reconnect, which starts the only server.
    connection.kill();
    states_until(&rx, WITHIN, |state| {
        matches!(state, ConnectionState::Reconnecting { .. })
    });
    let started = Instant::now();
    connection.wait_for_reconnect().unwrap();
    assert!(
        started.elapsed() + Duration::from_millis(100) >= backoff,
        "it waited for the reconnect: {:?}",
        started.elapsed()
    );
    assert_eq!(connection.state(), ConnectionState::Connected);
    assert_eq!(host.launches(), 2);
    assert_eq!(backend.worktree_len("a.txt").unwrap(), Some(2));

    // The reconnect fails: so does the wait, and it starts nothing of its own, then or later
    // (`retry` would start a server).
    host.blocked.store(true, Ordering::SeqCst);
    connection.kill();
    states_until(&rx, WITHIN, |state| {
        matches!(state, ConnectionState::Reconnecting { .. })
    });
    let error = connection.wait_for_reconnect().unwrap_err();
    assert!(error.to_string().contains("blocked by the test"), "{error}");
    assert!(matches!(connection.state(), ConnectionState::Failed { .. }));
    let launched = host.launches();
    host.blocked.store(false, Ordering::SeqCst);
    assert!(connection.wait_for_reconnect().is_err());
    assert_eq!(host.launches(), launched);
    assert!(matches!(connection.state(), ConnectionState::Failed { .. }));

    // Stopped: the same, and the host stays stopped.
    connection.retry().unwrap();
    host.up.store(false, Ordering::SeqCst);
    connection.kill();
    states_until(&rx, WITHIN, |state| {
        matches!(state, ConnectionState::Reconnecting { .. })
    });
    let error = connection.wait_for_reconnect().unwrap_err();
    assert_eq!(
        error.to_string(),
        "Ubuntu can't be opened: Ubuntu isn't running"
    );
    let launched = host.launches();
    assert!(connection.wait_for_reconnect().is_err());
    assert_eq!(host.launches(), launched);
    assert!(!host.up.load(Ordering::SeqCst));
}
