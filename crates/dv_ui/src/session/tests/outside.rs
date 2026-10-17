//! A file outside the repository in the file view (PLAN §18.1 "Go to definition"): read
//! once, looked at (no bytes) when the file view comes back to it, and read again only once its
//! length changed.

use pretty_assertions::assert_eq;

use super::*;
use crate::session::{SideFileState, ViewFrom};

#[gpui::test]
fn an_outside_file_is_read_again_only_when_its_length_changed(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture();
    let outside = repo.outside_dir("lib");
    let file = outside.join("x.d.ts");
    std::fs::write(
        &file,
        "declare const x: number;
",
    )
    .unwrap();
    let path = file.display().to_string();
    let probe = Probe::new(&repo, |_| Ok(()));
    let session = session_over(probe.clone(), Target::Working, cx);
    wait(cx, &session, "the list", |session| {
        session.files().len() == 3
    });
    let lines = |session: &ReviewSession| match session.outside_file(&path) {
        SideFileState::Loaded(shown) => shown.diff.as_text().map(|text| text.lines.len()),
        _ => None,
    };
    let open = |cx: &mut TestAppContext| {
        session.update(cx, |session, cx| {
            session.set_file_view_at(Some(path.clone()), ViewFrom::Outside, cx)
        })
    };
    open(cx);
    wait(cx, &session, "the read", |session| {
        lines(session) == Some(1)
    });
    assert_eq!(probe.reads.load(Ordering::SeqCst), 1);
    // Back to it, unchanged: looked at, not read.
    open(cx);
    session.update(cx, |session, cx| session.window_activated(cx));
    wait(cx, &session, "the look", |_| true);
    assert_eq!(probe.reads.load(Ordering::SeqCst), 1);
    // Its length changed: read again.
    std::fs::write(
        &file,
        "declare const x: number;
declare const y: number;
",
    )
    .unwrap();
    session.update(cx, |session, cx| session.window_activated(cx));
    wait(cx, &session, "the second read", |session| {
        lines(session) == Some(2)
    });
    assert_eq!(probe.reads.load(Ordering::SeqCst), 2);
}
