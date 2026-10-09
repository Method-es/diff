//! A launcher that installs builds, as `Launcher::Wsl` does (PLAN §5.2): the state while a
//! build is copied, the one reinstall a server whose build doesn't match gets, and a launch
//! that can't succeed failing the connection at once.

use std::sync::atomic::{AtomicU32, Ordering};

use pretty_assertions::assert_eq;

use super::*;
use crate::launch::{LaunchError, StartFn};

const WITHIN: Duration = Duration::from_secs(10);

/// A server that answers every request with `hello`'s answer, `build_ok` in it, until its
/// input ends.
fn answering(build_ok: Value) -> io::Result<Transport> {
    fake(move |mut input, mut output| {
        while let Some(request) = next_request(&mut input) {
            answer(
                &mut output,
                hello_reply(&request["id"], PROTO, build_ok.clone()),
            );
        }
    })
    .start()
}

/// A launcher that reinstalls, as `start` does it.
fn installer(
    start: impl Fn(&StartOptions<'_>) -> Result<Transport, LaunchError> + Send + Sync + 'static,
) -> Launcher {
    let start: Arc<StartFn> = Arc::new(start);
    Launcher::Custom {
        label: "an installing launcher".into(),
        start,
        reinstalls: true,
        running: None,
    }
}

#[test]
fn a_server_of_another_build_is_installed_again_once_with_the_progress_in_the_state() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let launcher = installer({
        let asked = Arc::clone(&asked);
        move |options| {
            asked.lock().push(options.reinstall.map(str::to_string));
            let build_ok = if options.reinstall.is_some() {
                (options.progress)(0, 10);
                (options.progress)(10, 10);
                json!(true)
            } else {
                json!(false)
            };
            Ok(answering(build_ok)?)
        }
    });
    let connection = Connection::new("Ubuntu", launcher);
    let (_subscription, rx) = states(&connection);
    connection.connect().unwrap();
    let seen = states_until(&rx, WITHIN, |state| *state == ConnectionState::Connected);
    assert_eq!(
        seen,
        [
            ConnectionState::Connecting,
            ConnectionState::Installing { done: 0, total: 10 },
            ConnectionState::Installing {
                done: 10,
                total: 10
            },
            ConnectionState::Connecting,
            ConnectionState::Connected,
        ]
    );
    // The reinstall names the architecture the mismatched server reported.
    assert_eq!(*asked.lock(), [None, Some("x86_64".to_string())]);
    assert_eq!(connection.server().unwrap().build_ok, Some(true));
    let diagnostics = connection.diagnostics();
    assert!(
        diagnostics.contains(" s the dv in Ubuntu doesn't match this app: installing it again\n"),
        "{diagnostics}"
    );
    // An install's progress is one line of the history.
    assert_eq!(
        diagnostics.matches("installing dv (").count(),
        1,
        "{diagnostics}"
    );
}

#[test]
fn a_build_still_wrong_after_its_reinstall_fails_the_connection() {
    let starts = Arc::new(AtomicU32::new(0));
    let launcher = installer({
        let starts = Arc::clone(&starts);
        move |_| {
            starts.fetch_add(1, Ordering::SeqCst);
            Ok(answering(json!(false))?)
        }
    });
    let connection = Connection::new("Ubuntu", launcher);
    let error = connection.connect().unwrap_err();
    assert_eq!(
        error.to_string(),
        "Ubuntu can't be opened: the connection to dv failed: the dv in Ubuntu doesn't match \
         this app"
    );
    assert_eq!(starts.load(Ordering::SeqCst), 2);
}

#[test]
fn a_launcher_that_doesnt_install_isnt_asked_to() {
    let starts = Arc::new(AtomicU32::new(0));
    let launcher = Launcher::custom("a plain launcher", {
        let starts = Arc::clone(&starts);
        move || {
            starts.fetch_add(1, Ordering::SeqCst);
            answering(json!(false))
        }
    });
    let connection = Connection::new("loopback", launcher);
    connection.connect().unwrap_err();
    assert_eq!(starts.load(Ordering::SeqCst), 1);
}

#[test]
fn a_launch_that_cant_succeed_fails_a_respawn_at_once() {
    let starts = Arc::new(AtomicU32::new(0));
    let launcher = installer({
        let starts = Arc::clone(&starts);
        move |_| {
            if starts.fetch_add(1, Ordering::SeqCst) == 0 {
                return Ok(Launcher::InProcess.start()?);
            }
            Err(LaunchError {
                message: "no Linux build of dv for x86_64".into(),
                permanent: true,
            })
        }
    });
    let connection =
        Connection::with_backoff("Ubuntu", launcher, vec![Duration::from_millis(20); 3]);
    let (_subscription, rx) = states(&connection);
    connection.connect().unwrap();
    connection.kill();
    let seen = states_until(&rx, WITHIN, |state| {
        matches!(state, ConnectionState::Failed { .. })
    });
    assert_eq!(
        seen[2..],
        [
            ConnectionState::Reconnecting { attempt: 1 },
            ConnectionState::Failed {
                error: "couldn't start dv: no Linux build of dv for x86_64".into()
            }
        ]
    );
    assert_eq!(starts.load(Ordering::SeqCst), 2);
}
