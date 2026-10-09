//! The pending pick (PLAN §5.2, DESIGN §4.4): a pick whose connection is the trouble waits for
//! it, raises one banner at most, never replaces unsaved text by itself, and runs again once the
//! connection is back.

use pretty_assertions::assert_eq;

use super::*;

#[gpui::test]
fn an_open_that_loses_its_connection_every_time_waits_for_it_in_the_banner(
    cx: &mut TestAppContext,
) {
    use std::io::{BufRead as _, BufReader, Write as _};

    let (_dir, shell, cx) = open_shell(cx);
    let repo = fixture();
    // A server that answers `hello`, then dies when asked to open anything: the connection is
    // lost under every open, until its respawns run out.
    let launcher = Launcher::custom("a server that dies when asked to open", || {
        let (server_in, client_out) = std::io::pipe()?;
        let (client_in, mut server_out) = std::io::pipe()?;
        std::thread::spawn(move || {
            let mut input = BufReader::new(server_in);
            let mut line = String::new();
            while input.read_line(&mut line).is_ok_and(|read| read > 0) {
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                line.clear();
                if request["op"] != "hello" {
                    return;
                }
                let reply = serde_json::json!({"id": request["id"], "ok": {
                    "proto": dv_backend::proto::PROTO, "build_ok": null, "pid": 1,
                    "os": "linux", "arch": "x86_64", "version": "0.0.0",
                }});
                let _ = writeln!(server_out, "{reply}");
            }
        });
        Ok(dv_backend::Transport::new(client_in, client_out))
    });
    let connection =
        Connection::with_backoff("loopback", launcher, vec![Duration::from_millis(10); 3]);
    let locations = cx.update(|_, cx| cx.services().locations.clone());
    locations.update(cx, |locations, _| {
        locations.register(
            repo.path().to_path_buf(),
            Route {
                env: repo.env_overlay(),
                connection: Some(connection.clone()),
            },
        )
    });
    let location = Location::Local(repo.path().to_path_buf());
    let failure = open_review(
        &shell,
        location.clone(),
        OpenRequest::Target(Target::Working),
        cx,
    )
    .expect_err("the server dies under it");
    assert!(failure.disconnected, "{failure:?}");
    assert_eq!(
        failure.message,
        format!("couldn't open {}", location.display_name())
    );
    assert!(
        failure.detail.as_deref().is_some_and(|detail| detail
            .contains("lost the connection to dv in loopback")
            && detail.contains("connection: loopback")),
        "{failure:?}"
    );
    assert!(matches!(connection.state(), ConnectionState::Failed { .. }));
    // No review is open to report the connection: the main area does, and keeps the pick for
    // when the connection is back.
    shell.read_with(cx, |shell, _| {
        assert_eq!(shell.banner().map(|banner| &banner.failure), Some(&failure));
        assert!(shell.session().is_none());
        let pending = shell.pending_pick().expect("the pick waits");
        assert_eq!(pending.location, location);
        assert_eq!(pending.request, OpenRequest::Target(Target::Working));
    });
    // Dismissed, it goes with its banner.
    shell.update(cx, |shell, cx| shell.dismiss_banner(cx));
    shell.read_with(cx, |shell, _| {
        assert!(shell.banner().is_none());
        assert!(shell.pending_pick().is_none());
    });
}

/// Writes requests to a server, failing the first one of `op` while `armed`: the server dies
/// under that call, as a crash would.
struct DiesAt {
    inner: Box<dyn std::io::Write + Send>,
    op: &'static str,
    armed: Arc<AtomicBool>,
}

impl std::io::Write for DiesAt {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let call = format!("\"op\":\"{}\"", self.op);
        if String::from_utf8_lossy(buf).contains(&call) && self.armed.swap(false, Ordering::SeqCst)
        {
            return Err(std::io::Error::other("the test killed dv"));
        }
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[gpui::test]
fn a_review_whose_server_dies_while_it_loads_opens_once_the_connection_is_back(
    cx: &mut TestAppContext,
) {
    let (_dir, shell, cx) = open_shell(cx);
    let repo = fixture();
    // The repository opens; then the server dies under the first read of a review's log.
    let armed = Arc::new(AtomicBool::new(true));
    let launcher = Launcher::custom("an in-process server that dies under a read once", {
        let armed = armed.clone();
        move || {
            let transport = Launcher::InProcess.start()?;
            Ok(dv_backend::Transport {
                writer: Box::new(DiesAt {
                    inner: transport.writer,
                    op: "store_read",
                    armed: armed.clone(),
                }),
                ..transport
            })
        }
    });
    let connection =
        Connection::with_backoff("loopback", launcher, vec![Duration::from_millis(10); 3]);
    let locations = cx.update(|_, cx| cx.services().locations.clone());
    locations.update(cx, |locations, _| {
        locations.register(
            repo.path().to_path_buf(),
            Route {
                env: repo.env_overlay(),
                connection: Some(connection.clone()),
            },
        )
    });
    let location = Location::Local(repo.path().to_path_buf());
    let opened = open_review(&shell, location, OpenRequest::Target(Target::Working), cx)
        .expect("it opens once the connection is back");
    assert!(
        !armed.load(Ordering::SeqCst),
        "the server died under the open"
    );
    assert_eq!(connection.generation(), 2, "a second server answered");
    settle(cx, &shell, "the file list", |shell, cx| {
        shell
            .session()
            .is_some_and(|session| session.read(cx).files().len() == 3)
    });
    cx.update(|_, cx| {
        let view = shell.read(cx);
        assert_eq!(view.session().unwrap().read(cx).review_id(), &opened.review);
        assert!(view.banner().is_none(), "{:?}", view.banner());
        assert!(view.pending_pick().is_none());
        assert_eq!(
            view.status_line(cx).location,
            "loopback · watching for changes"
        );
    });
}

#[gpui::test]
fn a_pick_whose_reconnect_fails_raises_one_banner_and_opens_after_retry(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = open_shell(cx);
    let repo = fixture();
    let blocked = Arc::new(AtomicBool::new(false));
    let launcher = Launcher::custom("an in-process server tests block", {
        let blocked = blocked.clone();
        move || {
            if blocked.load(Ordering::SeqCst) {
                return Err(std::io::Error::other("blocked by the test"));
            }
            Launcher::InProcess.start()
        }
    });
    // One respawn, late enough to pick a review while it is awaited.
    let connection =
        Connection::with_backoff("loopback", launcher, vec![Duration::from_millis(500)]);
    let locations = cx.update(|_, cx| cx.services().locations.clone());
    locations.update(cx, |locations, _| {
        locations.register(
            repo.path().to_path_buf(),
            Route {
                env: repo.env_overlay(),
                connection: Some(connection.clone()),
            },
        )
    });
    let location = Location::Local(repo.path().to_path_buf());
    open_review(
        &shell,
        location.clone(),
        OpenRequest::Target(Target::Working),
        cx,
    )
    .expect("it opens through the loopback");

    // Its server dies and can't start again; meanwhile another review of the repo is picked.
    blocked.store(true, Ordering::SeqCst);
    connection.kill();
    assert!(matches!(
        connection.state(),
        ConnectionState::Reconnecting { .. }
    ));
    let failure = open_review(
        &shell,
        location.clone(),
        OpenRequest::Target(Target::Staged),
        cx,
    )
    .expect_err("the reconnect fails");
    assert!(
        matches!(connection.state(), ConnectionState::Failed { .. }),
        "{failure:?}"
    );
    // One banner: the open review's, about its connection; the pick waits for it.
    cx.update(|_, cx| {
        let view = shell.read(cx);
        assert!(view.banner().is_none(), "{:?}", view.banner());
        assert!(view.backend_down(cx).is_some());
        let pending = view.pending_pick().expect("the pick waits");
        assert_eq!(pending.request, OpenRequest::Target(Target::Staged));
        assert_eq!(
            view.session().unwrap().read(cx).review().target,
            Target::Working
        );
        assert_eq!(view.status_line(cx).location, "loopback · disconnected");
    });

    // The backend banner's Retry: connected, the pick opens by itself, and no banner is left.
    blocked.store(false, Ordering::SeqCst);
    shell.update(cx, |shell, cx| shell.retry_backend(cx));
    settle(cx, &shell, "the pick to open", |shell, cx| {
        shell
            .session()
            .is_some_and(|session| session.read(cx).review().target == Target::Staged)
    });
    cx.update(|_, cx| {
        let view = shell.read(cx);
        assert!(view.banner().is_none(), "{:?}", view.banner());
        assert!(view.backend_down(cx).is_none());
        assert!(view.pending_pick().is_none());
        assert_eq!(
            view.status_line(cx).location,
            "loopback · watching for changes"
        );
    });
}

#[gpui::test]
fn with_no_review_open_a_pick_waits_in_the_banner_and_opens_when_its_connection_is_back(
    cx: &mut TestAppContext,
) {
    let (_dir, shell, cx) = open_shell(cx);
    let repo = fixture();
    let blocked = Arc::new(AtomicBool::new(true));
    let connection = blockable_loopback(blocked.clone());
    let locations = cx.update(|_, cx| cx.services().locations.clone());
    locations.update(cx, |locations, _| {
        locations.register(
            repo.path().to_path_buf(),
            Route {
                env: repo.env_overlay(),
                connection: Some(connection.clone()),
            },
        )
    });
    let location = Location::Local(repo.path().to_path_buf());
    let failure = open_review(
        &shell,
        location.clone(),
        OpenRequest::Target(Target::Working),
        cx,
    )
    .expect_err("its server can't start");
    assert_eq!(
        failure.message,
        format!("couldn't open {}", location.display_name())
    );
    cx.update(|_, cx| {
        let view = shell.read(cx);
        assert_eq!(view.banner().map(|banner| &banner.failure), Some(&failure));
        assert!(view.pending_pick().is_some());
        assert!(view.session().is_none());
        assert_eq!(view.status_line(cx).location, "Local · can't be opened");
    });

    // Brought back by something else (the switcher's Retry row): the pick runs again by
    // itself, and its banner goes.
    blocked.store(false, Ordering::SeqCst);
    connection.retry().expect("it starts now");
    settle(cx, &shell, "the pick to open", |shell, _| {
        shell.session().is_some()
    });
    cx.update(|_, cx| {
        let view = shell.read(cx);
        assert!(view.banner().is_none(), "{:?}", view.banner());
        assert!(view.pending_pick().is_none());
        assert_eq!(
            view.session().unwrap().read(cx).review().target,
            Target::Working
        );
    });
}

/// `repo` routed through a loopback whose server the test blocks, and its working tree's
/// review open through it with its files listed.
fn open_through_a_blockable_loopback(
    shell: &Entity<Shell>,
    repo: &dv_test_support::FixtureRepo,
    cx: &mut VisualTestContext,
) -> (Arc<AtomicBool>, Arc<Connection>, Location) {
    let blocked = Arc::new(AtomicBool::new(false));
    let connection = blockable_loopback(blocked.clone());
    let locations = cx.update(|_, cx| cx.services().locations.clone());
    locations.update(cx, |locations, _| {
        locations.register(
            repo.path().to_path_buf(),
            Route {
                env: repo.env_overlay(),
                connection: Some(connection.clone()),
            },
        )
    });
    let location = Location::Local(repo.path().to_path_buf());
    open_review(
        shell,
        location.clone(),
        OpenRequest::Target(Target::Working),
        cx,
    )
    .expect("it opens through the loopback");
    settle(cx, shell, "the file list", |shell, cx| {
        shell
            .session()
            .is_some_and(|session| session.read(cx).files().len() == 3)
    });
    (blocked, connection, location)
}

/// Kills the open review's server, which can't start again, then picks the staged changes of
/// the same repository: the pick waits for the connection, with no banner of its own (the open
/// review's backend banner reports the connection).
fn pick_behind_a_failed_connection(
    shell: &Entity<Shell>,
    blocked: &AtomicBool,
    connection: &Connection,
    location: &Location,
    cx: &mut VisualTestContext,
) {
    blocked.store(true, Ordering::SeqCst);
    connection.kill();
    settle(cx, shell, "the connection to fail", |shell, cx| {
        shell.backend_down(cx).is_some()
    });
    open_review(
        shell,
        location.clone(),
        OpenRequest::Target(Target::Staged),
        cx,
    )
    .expect_err("its connection is down");
    cx.update(|_, cx| {
        let view = shell.read(cx);
        assert!(view.banner().is_none(), "{:?}", view.banner());
        let pending = view.pending_pick().expect("the pick waits");
        assert_eq!(pending.request, OpenRequest::Target(Target::Staged));
    });
}

/// Lets the server start again and brings the connection back with the backend banner's
/// Retry.
fn bring_the_connection_back(
    shell: &Entity<Shell>,
    blocked: &AtomicBool,
    cx: &mut VisualTestContext,
) {
    blocked.store(false, Ordering::SeqCst);
    shell.update(cx, |shell, cx| shell.retry_backend(cx));
    settle(cx, shell, "the connection to come back", |shell, cx| {
        shell.backend_down(cx).is_none() && shell.status_line(cx).health == Health::Live
    });
}

/// The target of the open review.
fn open_target(shell: &Entity<Shell>, cx: &mut VisualTestContext) -> Option<Target> {
    shell.read_with(cx, |shell, cx| {
        shell
            .session()
            .map(|session| session.read(cx).review().target.clone())
    })
}

#[gpui::test]
fn a_waiting_pick_leaves_an_open_sheet_alone_and_opens_once_it_closes(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = open_shell(cx);
    let repo = fixture();
    let (blocked, connection, location) = open_through_a_blockable_loopback(&shell, &repo, cx);
    pick_behind_a_failed_connection(&shell, &blocked, &connection, &location, cx);

    // The reviewer goes on with the open review: a round summary, typed in the sheet.
    press(cx, "secondary-enter tab");
    cx.simulate_input("My long unsaved round summary");
    let summary = |cx: &mut VisualTestContext| {
        shell.read_with(cx, |shell, cx| {
            shell.sheet().map(|sheet| sheet.read(cx).summary_text(cx))
        })
    };
    assert_eq!(
        summary(cx).as_deref(),
        Some("My long unsaved round summary")
    );

    // The connection comes back: the review under the sheet stays, and the pick waits.
    bring_the_connection_back(&shell, &blocked, cx);
    assert_eq!(open_target(&shell, cx), Some(Target::Working));
    assert_eq!(
        summary(cx).as_deref(),
        Some("My long unsaved round summary")
    );
    shell.read_with(cx, |shell, _| {
        assert!(shell.pending_pick().is_some(), "the pick still waits");
        assert!(shell.banner().is_none(), "{:?}", shell.banner());
    });

    // The sheet is cancelled: the pick opens now.
    let sheet = shell.read_with(cx, |shell, _| shell.sheet().cloned().expect("the sheet"));
    sheet.update(cx, |sheet, cx| sheet.dismiss(cx));
    settle(cx, &shell, "the pick to open", |shell, cx| {
        shell
            .session()
            .is_some_and(|session| session.read(cx).review().target == Target::Staged)
    });
    shell.read_with(cx, |shell, _| {
        assert!(shell.pending_pick().is_none());
        assert!(shell.sheet().is_none());
        assert!(shell.banner().is_none(), "{:?}", shell.banner());
    });
}

#[gpui::test]
fn a_waiting_pick_leaves_a_reply_draft_alone_and_opens_once_it_is_cancelled(
    cx: &mut TestAppContext,
) {
    use dv_core::review::AnchorSide;

    let (_dir, shell, cx) = open_shell(cx);
    let repo = fixture();
    let (blocked, connection, location) = open_through_a_blockable_loopback(&shell, &repo, cx);
    // A thread on the deleted README's line, and a reply to it being written.
    let session = shell.read_with(cx, |shell, _| shell.session().cloned().unwrap());
    let readme = session.clone();
    settle(cx, &shell, "README.md's lines", move |_, cx| {
        readme
            .read(cx)
            .anchor_for("README.md", AnchorSide::Old, 1, 1)
            .is_ok()
    });
    let adding = session.update(cx, |session, cx| {
        let anchor = session
            .anchor_for("README.md", AnchorSide::Old, 1, 1)
            .expect("the lines are loaded");
        session.add_thread(anchor, "Why remove it?".to_string(), cx)
    });
    settle(cx, &shell, "the thread to land", |_, _| true);
    let thread = cx
        .foreground_executor()
        .block_test(adding)
        .expect("the thread is added");
    let pane = shell.read_with(cx, |shell, _| shell.diff_pane().clone());
    pane.update_in(cx, |pane, window, cx| pane.start_reply(&thread, window, cx));
    let input = pane.read_with(cx, |pane, _| {
        pane.composer().expect("the reply composer").input.clone()
    });
    cx.update(|window, cx| {
        input.update(cx, |input, cx| {
            input.set_value("An unsent reply draft", window, cx)
        })
    });
    let draft = |cx: &mut VisualTestContext| pane.read_with(cx, |pane, cx| pane.composer_text(cx));
    assert_eq!(draft(cx).as_deref(), Some("An unsent reply draft"));
    pick_behind_a_failed_connection(&shell, &blocked, &connection, &location, cx);

    // The connection comes back: the review under the draft stays, and the pick waits.
    bring_the_connection_back(&shell, &blocked, cx);
    assert_eq!(open_target(&shell, cx), Some(Target::Working));
    assert_eq!(draft(cx).as_deref(), Some("An unsent reply draft"));
    assert!(shell.read_with(cx, |shell, _| shell.pending_pick().is_some()));

    // The reply is cancelled: the pick opens now.
    pane.update_in(cx, |pane, window, cx| pane.cancel_composer(window, cx));
    settle(cx, &shell, "the pick to open", |shell, cx| {
        shell
            .session()
            .is_some_and(|session| session.read(cx).review().target == Target::Staged)
    });
    assert!(shell.read_with(cx, |shell, _| shell.pending_pick().is_none()));
}

#[gpui::test]
fn dismissing_another_banner_keeps_the_waiting_pick(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = open_shell(cx);
    let repo = fixture();
    let (blocked, connection, location) = open_through_a_blockable_loopback(&shell, &repo, cx);
    pick_behind_a_failed_connection(&shell, &blocked, &connection, &location, cx);

    // Open folder… on a folder that isn't a repository: a banner, and not the pick's.
    let scratch = tempfile::tempdir().unwrap();
    let missing = Location::Local(scratch.path().join("missing"));
    open_repo(&shell, missing, IfNotOpened::Banner, cx).expect_err("not a repository");
    shell.read_with(cx, |shell, _| {
        let banner = shell.banner().expect("its banner");
        assert!(banner.retry.is_none());
        assert!(shell.pending_pick().is_some());
    });
    // Dismissed, it goes alone: the pick still waits, and opens once the connection is back.
    shell.update(cx, |shell, cx| shell.dismiss_banner(cx));
    shell.read_with(cx, |shell, _| {
        assert!(shell.banner().is_none());
        assert!(shell.pending_pick().is_some(), "the pick still waits");
    });
    bring_the_connection_back(&shell, &blocked, cx);
    settle(cx, &shell, "the pick to open", |shell, cx| {
        shell
            .session()
            .is_some_and(|session| session.read(cx).review().target == Target::Staged)
    });
    assert!(shell.read_with(cx, |shell, _| shell.pending_pick().is_none()));
}

#[gpui::test]
fn a_waiting_pick_runs_through_a_connection_that_idled_out_meanwhile(cx: &mut TestAppContext) {
    let (_dir, shell, cx) = open_shell(cx);
    // The open review is a local repository's; the pick is another's, through a loopback that
    // closes its server once unused for a moment.
    let here = fixture();
    add_repo(&here, &shell, cx);
    open_review(
        &shell,
        Location::Local(here.path().to_path_buf()),
        OpenRequest::Target(Target::Working),
        cx,
    )
    .expect("the local review opens");
    let there = fixture();
    let blocked = Arc::new(AtomicBool::new(true));
    let launcher = Launcher::custom("an in-process server tests block", {
        let blocked = blocked.clone();
        move || {
            if blocked.load(Ordering::SeqCst) {
                return Err(std::io::Error::other("blocked by the test"));
            }
            Launcher::InProcess.start()
        }
    });
    let connection = Connection::with_options(
        "loopback",
        launcher,
        dv_backend::client::ConnectionOptions {
            backoff: vec![Duration::from_millis(10)],
            idle: Some(Duration::from_millis(300)),
        },
    );
    let locations = cx.update(|_, cx| cx.services().locations.clone());
    locations.update(cx, |locations, _| {
        locations.register(
            there.path().to_path_buf(),
            Route {
                env: there.env_overlay(),
                connection: Some(connection.clone()),
            },
        )
    });
    let location = Location::Local(there.path().to_path_buf());
    open_review(
        &shell,
        location.clone(),
        OpenRequest::Target(Target::Working),
        cx,
    )
    .expect_err("its server can't start");
    assert!(shell.read_with(cx, |shell, _| shell.pending_pick().is_some()));

    // A summary in the sheet holds the pick while its connection comes back, and then closes
    // for idleness.
    press(cx, "secondary-enter tab");
    cx.simulate_input("Unsaved");
    blocked.store(false, Ordering::SeqCst);
    connection.retry().expect("it starts now");
    let idled = connection.clone();
    settle(cx, &shell, "the connection to idle out", move |_, _| {
        idled.state() == ConnectionState::Idle
    });
    shell.read_with(cx, |shell, cx| {
        assert!(shell.pending_pick().is_some(), "the pick still waits");
        let session = shell.session().expect("the local review").read(cx);
        assert_eq!(
            session.origin(),
            &Location::Local(here.path().to_path_buf())
        );
    });

    // The sheet closes: the pick runs, starting the connection again.
    let sheet = shell.read_with(cx, |shell, _| shell.sheet().cloned().expect("the sheet"));
    sheet.update(cx, |sheet, cx| sheet.dismiss(cx));
    settle(cx, &shell, "the pick to open", |shell, cx| {
        shell
            .session()
            .is_some_and(|session| session.read(cx).origin() == &location)
    });
    shell.read_with(cx, |shell, _| {
        assert!(shell.pending_pick().is_none());
        assert!(shell.banner().is_none(), "{:?}", shell.banner());
    });
}
