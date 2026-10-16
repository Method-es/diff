//! Code intelligence in the shell (PLAN §18.1, §18.5, DESIGN §4.1): the open review's
//! [`CodeIntel`], where its places go, the panel's References tab and the status bar's servers.
//!
//! **Going to a place** (go to definition, a references row, a hover card's file link). A place
//! the stream shows — a line of a file the review lists, within the changes' context
//! ([`stream_shows`]) — scrolls the stream there (the gap around it opened); any other place in
//! the repository opens the file view there, a file git ignores included (a library's `.d.ts`
//! under `node_modules`, read as an untracked file reads); a place outside the repository can't
//! be shown, and the main area's notice says where it is. Each is a jump the history keeps
//! (`Alt ←` comes back). A draft in the file view's composer keeps the file view: it takes the
//! keyboard instead, as for every jump.
//!
//! **The panel's tabs.** While a references list (or several definitions) is open the panel
//! heads its column with two tabs, **Review** (the file list and the comments) and
//! **References**; closing the list takes the tabs away.
//!
//! **The status bar** names each server of the review's worktree and its state; a click opens
//! its details (the command line, where it came from, its last stderr lines, **Retry** for a
//! failed one), and a missing server's say why and how to get it.

use dv_core::diff::{FileDiff, LineKind, TextDiff};
use std::collections::HashMap;

use dv_core::Side;
use dv_core::review::AnchorSide;
use dv_lsp::{SymbolLocation, UriTarget};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _, Window,
    anchored, deferred, div, point, prelude::FluentBuilder as _, px,
};

use super::Shell;
use crate::automation::AutomationTargetExt as _;
use crate::components::{icon, menu_panel, small_button};
use crate::diff_pane::rows::CONTEXT_LINES;
use crate::failure::Failure;
use crate::icons::DvIcon;
use crate::intel::{Asked, CodeIntel, IntelEvent, Pane, ServerInfo, ServerState};
use crate::session::ReviewSession;
use crate::theme::{DvTheme, StyledText as _};

/// Where a place shows (`Shell::go_to_location`).
enum Place {
    /// The stream, on this side.
    Stream(AnchorSide),
    /// The file view, reading the working tree as it stands (`worktree`), at this side (else
    /// the side the file view shows the file at).
    FileView {
        worktree: bool,
        side: Option<AnchorSide>,
    },
}

/// Which of the panel's tabs shows (module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PanelTab {
    /// The file list and the comments.
    #[default]
    Review,
    References,
}

/// The status bar item whose details are open: a server, or a spec that is missing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerDetails {
    Server(u64),
    Missing(&'static str),
}

/// Whether the stream shows line `line` (new side) of `path`: the review lists the file and the
/// line is within [`CONTEXT_LINES`] of a change on the new side, so it is in view once its file
/// is (module docs).
pub fn stream_shows(session: &ReviewSession, path: &str, line: u32) -> bool {
    session.file(path).is_some()
        && session
            .shown_diff(path)
            .and_then(|diff| FileDiff::as_text(diff))
            .is_some_and(|text| near_change(text, line))
}

/// Whether new-side line `line` of `text` is within [`CONTEXT_LINES`] of an added or removed
/// line.
pub fn near_change(text: &TextDiff, line: u32) -> bool {
    let Some(at) = text
        .lines
        .iter()
        .position(|diff_line| diff_line.new_no == Some(line))
    else {
        return false;
    };
    let from = at.saturating_sub(CONTEXT_LINES);
    let to = (at + CONTEXT_LINES + 1).min(text.lines.len());
    text.lines[from..to]
        .iter()
        .any(|diff_line| diff_line.kind != LineKind::Context)
}

impl Shell {
    /// The open review's code intelligence, if a review is open.
    pub fn intel(&self) -> Option<&Entity<CodeIntel>> {
        self.intel.as_ref()
    }

    /// Which panel tab shows (`Review` while there is no references list).
    pub fn panel_tab(&self, cx: &gpui::App) -> PanelTab {
        if self.has_references(cx) {
            self.panel_tab
        } else {
            PanelTab::Review
        }
    }

    fn has_references(&self, cx: &gpui::App) -> bool {
        self.intel
            .as_ref()
            .is_some_and(|intel| intel.read(cx).references().is_some())
    }

    pub fn set_panel_tab(&mut self, tab: PanelTab, cx: &mut Context<Self>) {
        if self.panel_tab != tab {
            self.panel_tab = tab;
            cx.notify();
        }
    }

    /// Makes `session`'s code intelligence (the review just opened), handing it to both panes
    /// and the References tab.
    pub(super) fn start_intel(
        &mut self,
        session: &Entity<ReviewSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let intel = cx.new(|cx| CodeIntel::new(session.clone(), cx));
        self._intel_subscriptions = vec![
            cx.subscribe_in(&intel, window, |this, _, event: &IntelEvent, window, cx| {
                this.on_intel_event(event, window, cx)
            }),
            cx.observe(&intel, |_, _, cx| cx.notify()),
        ];
        for pane in [&self.diff_pane, &self.file_view] {
            pane.update(cx, |pane, cx| pane.set_intel(Some(intel.clone()), cx));
        }
        self.references_panel
            .update(cx, |panel, cx| panel.set_intel(Some(intel.clone()), cx));
        self.panel_tab = PanelTab::Review;
        self.server_details = None;
        self.definition_from = None;
        self.pending_goto = None;
        self.intel = Some(intel);
    }

    /// Closes the open review, its code intelligence with it (tests: what happens once no
    /// review uses a worktree's servers).
    #[cfg(test)]
    pub fn close_review_for_tests(&mut self, cx: &mut Context<Self>) {
        for pane in [&self.diff_pane, &self.file_view] {
            pane.update(cx, |pane, cx| {
                pane.set_intel(None, cx);
                pane.set_session(None, cx);
            });
        }
        self.references_panel
            .update(cx, |panel, cx| panel.set_intel(None, cx));
        self._intel_subscriptions.clear();
        self.intel = None;
        self.session = None;
        self._session_subscriptions.clear();
        cx.notify();
    }

    /// What the review's code intelligence asks for.
    fn on_intel_event(&mut self, event: &IntelEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            IntelEvent::GoTo { location, asked } => {
                // A definition that answers after the reviewer moved on is dropped.
                if asked.as_ref().is_some_and(|asked| asked.definition) {
                    let from = self.definition_from.take();
                    if from.is_some_and(|from| from != self.position(cx)) {
                        log::info!("go to definition answered after the reviewer moved on");
                        return;
                    }
                }
                self.go_to_location(location.clone(), asked.clone(), window, cx)
            }
            IntelEvent::DefinitionAsked => {
                self.definition_from = Some(self.position(cx));
                self.pending_goto = None;
            }
            IntelEvent::ShowReferences => {
                self.panel_tab = PanelTab::References;
                self.set_panel_visible(true, cx);
                cx.notify();
            }
        }
    }

    /// A file of the review landed: a go to definition that waited for it goes on.
    pub(super) fn file_loaded(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let waiting = self
            .pending_goto
            .as_ref()
            .is_some_and(|(location, _)| location.target == UriTarget::Worktree(path.to_string()));
        let loaded = self
            .session
            .as_ref()
            .is_some_and(|session| session.read(cx).shown_diff(path).is_some());
        if waiting
            && loaded
            && let Some((location, asked)) = self.pending_goto.take()
        {
            if let Some(session) = &self.session {
                session.update(cx, |session, cx| session.load_for_definition(None, cx));
            }
            self.go_to_location(location, asked, window, cx);
        }
    }

    /// Shows `location`, asked from `asked` when that is known (module docs, "Going to a
    /// place").
    pub fn go_to_location(
        &mut self,
        location: SymbolLocation,
        asked: Option<Asked>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(session), Some(intel)) = (self.session.clone(), self.intel.clone()) else {
            return;
        };
        let start = location.range.start;
        let (line, offset) = (start.line + 1, start.column as usize);
        let path = match &location.target {
            UriTarget::Worktree(path) => path.clone(),
            UriTarget::Outside { uri, path } => {
                let place = path.clone().unwrap_or_else(|| uri.clone());
                intel.update(cx, |intel, cx| {
                    intel.set_notice(
                        Failure::with_detail(
                            format!("Defined outside the repository: {place}:{line}"),
                            format!(
                                "dv shows the repository's own files. The language server \
                                 answered with {uri}, line {line}."
                            ),
                        ),
                        cx,
                    )
                });
                return;
            }
        };
        intel.update(cx, |intel, cx| intel.dismiss_notice(cx));
        let reads_worktree = session.read(cx).new_side() == Some(&Side::Worktree);
        let listed = session.read(cx).file(&path).is_some();
        let place = match asked {
            // The same file, answered against the text dv opened for it (another side): that
            // side's line, where it was asked.
            Some(asked) if asked.at.path == path && !asked.worktree => {
                if asked.pane == Pane::FileView || !listed {
                    Place::FileView {
                        worktree: false,
                        side: Some(asked.at.side),
                    }
                } else {
                    Place::Stream(asked.at.side)
                }
            }
            // Read from the working tree, and the review's new side isn't it: the file as it
            // stands now (PLAN §18.2).
            _ if !reads_worktree => Place::FileView {
                worktree: true,
                side: None,
            },
            _ if listed => {
                if session.read(cx).shown_diff(&path).is_none() {
                    // Listed but not loaded yet: decided once its diff lands.
                    self.pending_goto = Some((location, asked));
                    session.update(cx, |session, cx| {
                        session.load_for_definition(Some(path.clone()), cx)
                    });
                    return;
                }
                if stream_shows(session.read(cx), &path, line) {
                    Place::Stream(AnchorSide::New)
                } else {
                    Place::FileView {
                        worktree: false,
                        side: None,
                    }
                }
            }
            _ => Place::FileView {
                worktree: false,
                side: None,
            },
        };
        match place {
            Place::Stream(side) => {
                if self.file_view_open {
                    self.close_file_view(true, window, cx);
                    if self.file_view_open {
                        // A draft in the file view took the keyboard instead.
                        return;
                    }
                } else if let Some(from) = self.position(cx) {
                    self.history.jump(from);
                }
                self.diff_pane.update(cx, |pane, cx| {
                    pane.show_place(&path, side, line, offset, cx)
                });
                self.diff_pane.read(cx).focus_handle(cx).focus(window, cx);
            }
            Place::FileView { worktree, side } => {
                let shown = self.file_view_open && {
                    let view = self.file_view.read(cx);
                    view.file_view_path() == Some(path.as_str())
                        && view.file_view_worktree() == worktree
                };
                if shown && let Some(from) = self.position(cx) {
                    // Within the file shown: still a jump.
                    self.history.jump(from);
                }
                self.open_file_view_at(&path, Some(line), worktree, !shown, window, cx);
                if self.file_view_open
                    && self.file_view.read(cx).file_view_path() == Some(path.as_str())
                {
                    let side = side
                        .or(self.file_view.read(cx).file_view_side())
                        .unwrap_or(AnchorSide::New);
                    self.file_view.update(cx, |pane, cx| {
                        pane.show_place(&path, side, line, offset, cx)
                    });
                }
            }
        }
        cx.notify();
    }

    /// The panel's tabs, while a references list is open (module docs).
    pub(super) fn render_panel_tabs(
        &self,
        theme: &DvTheme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.has_references(cx) {
            return None;
        }
        let c = &theme.colors;
        let current = self.panel_tab(cx);
        let count = self
            .intel
            .as_ref()
            .and_then(|intel| intel.read(cx).references())
            .filter(|list| !list.is_loading())
            .map(|list| list.count());
        let tab = |tab: PanelTab, label: String, key: &'static str| {
            let on = current == tab;
            div()
                .id(key)
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .h(px(26.))
                .px(theme.spacing.md)
                .rounded(theme.radii.control)
                .cursor_pointer()
                .text_spec(&theme.text.control)
                .text_color(if on { c.fg } else { c.fg2 })
                .when(on, |this| this.bg(c.seg_on))
                .when(!on, |this| this.hover(|style| style.bg(c.control_hover)))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.set_panel_tab(tab, cx)))
                .automation_target(format!("panel/tab/{}", key.trim_start_matches("tab-")))
        };
        let references = match count {
            Some(count) => format!("References {count}"),
            None => "References".to_string(),
        };
        Some(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(theme.spacing.xs)
                .mx(theme.spacing.lg)
                .mt(theme.spacing.md)
                .p(px(3.))
                .rounded(theme.radii.segmented)
                .bg(c.ctrl)
                .border_1()
                .border_color(c.ctrl_line)
                .child(tab(PanelTab::Review, "Review".to_string(), "tab-review"))
                .child(tab(PanelTab::References, references, "tab-references"))
                .into_any_element(),
        )
    }

    /// The status bar's servers (module docs): each server's name and state, and each missing
    /// one's.
    pub(super) fn render_server_items(
        &self,
        theme: &DvTheme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(intel) = self.intel.as_ref() else {
            return Vec::new();
        };
        let c = &theme.colors;
        let intel = intel.read(cx);
        let servers = intel.servers(cx);
        let missing = intel.missing(cx);
        let mut items: Vec<AnyElement> = Vec::new();
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for server in servers {
            let (dot, state) = server_line(&server, theme);
            let details = ServerDetails::Server(server.id);
            // The first server of a spec is `statusbar/lsp/<id>`, another (two TypeScript
            // versions in one monorepo) `statusbar/lsp/<id>/<n>`, n from 2.
            let nth = seen.entry(server.spec_id).or_default();
            *nth += 1;
            let name = match *nth {
                1 => format!("statusbar/lsp/{}", server.spec_id),
                n => format!("statusbar/lsp/{}/{n}", server.spec_id),
            };
            items.push(
                status_item(
                    format!("statusbar-lsp-{}", server.id),
                    dot,
                    format!("{} {state}", server.name),
                    theme,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_details(&details, cx)))
                .automation_target(name),
            );
        }
        for spec in missing {
            let details = ServerDetails::Missing(spec.spec_id);
            let name = dv_lsp::languages::by_id(spec.spec_id).map_or(spec.spec_id, |s| s.name);
            let target = format!("statusbar/lsp/{}", spec.spec_id);
            items.push(
                status_item(
                    format!("statusbar-lsp-missing-{}", spec.spec_id),
                    c.fg3,
                    format!("{name} missing"),
                    theme,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_details(&details, cx)))
                .automation_target(target),
            );
        }
        items
    }

    fn toggle_details(&mut self, details: &ServerDetails, cx: &mut Context<Self>) {
        self.server_details = if self.server_details.as_ref() == Some(details) {
            None
        } else {
            Some(details.clone())
        };
        cx.notify();
    }

    /// The details of the status bar item clicked: over the status bar at its right.
    pub(super) fn render_server_details(
        &self,
        window: &Window,
        theme: &DvTheme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let details = self.server_details.clone()?;
        let intel = self.intel.clone()?;
        let c = &theme.colors;
        let read = intel.read(cx);
        let mono = |text: String| {
            div()
                .w_full()
                .px(theme.spacing.sm)
                .py(theme.spacing.xs)
                .rounded(theme.radii.small)
                .bg(c.icode_bg)
                .text_spec(&theme.text.inline_code)
                .text_color(c.fg2)
                .child(text)
        };
        let label = |text: &'static str| {
            div()
                .mt(theme.spacing.sm)
                .text_spec(&theme.text.secondary)
                .text_color(c.fg3)
                .child(text)
        };
        let panel = menu_panel("server-details", theme)
            .w(px(420.))
            .p(theme.spacing.lg)
            .flex()
            .flex_col()
            .gap(theme.spacing.xs)
            .text_spec(&theme.text.body);
        let panel = match &details {
            ServerDetails::Server(id) => {
                let server = read.servers(cx).into_iter().find(|s| s.id == *id)?;
                let (dot, state) = server_line(&server, theme);
                let retry = matches!(server.state, ServerState::Failed(_)).then(|| {
                    let intel = intel.clone();
                    let id = *id;
                    small_button("server-retry", "Retry", theme)
                        .child(icon(DvIcon::RotateCw, px(12.), c.fg2))
                        .on_click(move |_, _, cx| {
                            intel.update(cx, |intel, cx| intel.retry(id, cx));
                        })
                        .automation_target(format!("statusbar/lsp/{}/retry", server.spec_id))
                });
                let why = match &server.state {
                    ServerState::Failed(why) => Some(why.clone()),
                    _ => None,
                };
                panel
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.spacing.sm)
                            .child(div().size(px(7.)).rounded_full().bg(dot))
                            .child(
                                div()
                                    .text_spec(&theme.text.title)
                                    .child(server.name.to_string()),
                            )
                            .child(div().text_color(c.fg2).child(state))
                            .child(div().flex_1())
                            .children(retry),
                    )
                    .children(why.map(|why| div().text_color(c.red).child(why)))
                    .child(label("Command"))
                    .child(mono(server.command_line.clone()))
                    .child(
                        div()
                            .text_spec(&theme.text.secondary)
                            .text_color(c.fg3)
                            .child(format!("From {}", server.source)),
                    )
                    .when(!server.stderr.is_empty(), |this| {
                        this.child(label("Last output")).child(mono(
                            server
                                .stderr
                                .iter()
                                .rev()
                                .take(8)
                                .rev()
                                .cloned()
                                .collect::<Vec<_>>()
                                .join("\n"),
                        ))
                    })
            }
            ServerDetails::Missing(spec_id) => {
                let missing = read
                    .missing(cx)
                    .into_iter()
                    .find(|m| m.spec_id == *spec_id)?;
                let name = dv_lsp::languages::by_id(spec_id).map_or(*spec_id, |s| s.name);
                panel
                    .child(
                        div()
                            .text_spec(&theme.text.title)
                            .child(format!("{name} isn't available")),
                    )
                    .map(|this| {
                        // "… isn't installed (where it looked)": the places, quieter.
                        let (why, looked) = match missing.reason.split_once(" (") {
                            Some((why, rest)) if rest.ends_with(')') => (
                                why.to_string(),
                                Some(rest.trim_end_matches(')').to_string()),
                            ),
                            _ => (missing.reason.clone(), None),
                        };
                        this.child(div().text_color(c.fg2).child(why))
                            .children(looked.map(|looked| {
                                div()
                                    .text_spec(&theme.text.secondary)
                                    .text_color(c.fg3)
                                    .child(looked)
                            }))
                    })
                    .child(label("How to get it"))
                    .child(div().text_color(c.fg2).child(missing.hint.clone()))
                    .child(
                        div().mt(theme.spacing.sm).flex().child(
                            small_button("server-check-again", "Check again", theme)
                                .child(icon(DvIcon::RotateCw, px(12.), c.fg2))
                                .on_click({
                                    let intel = intel.clone();
                                    move |_, _, cx| {
                                        intel.update(cx, |intel, cx| intel.check_again(cx));
                                    }
                                })
                                .automation_target(format!("statusbar/lsp/{spec_id}/check_again")),
                        ),
                    )
            }
        };
        let bottom = window.viewport_size().height - theme.sizes.status_bar_height;
        let right = window.viewport_size().width - theme.spacing.lg;
        Some(
            deferred(
                anchored()
                    .position(point(right, bottom - px(6.)))
                    .anchor(gpui::Anchor::BottomRight)
                    .snap_to_window_with_margin(px(8.))
                    .child(panel.automation_target("statusbar/lsp/details")),
            )
            .with_priority(super::render::MENU_PRIORITY)
            .into_any_element(),
        )
    }

    /// The main area's notice for a place that can't be shown (module docs).
    pub(super) fn intel_banner(&self, cx: &mut Context<Self>) -> Option<crate::banner::Banner> {
        let intel = self.intel.clone()?;
        let failure = intel.read(cx).notice()?.clone();
        Some(crate::banner::Banner::new("banner/intel", failure).action(
            crate::banner::BannerAction::new("dismiss", "Dismiss", move |_, cx| {
                intel.update(cx, |intel, cx| intel.dismiss_notice(cx))
            }),
        ))
    }
}

/// A server's dot and how its state reads ("ready", "loading 40%", "restarting…", "failed").
fn server_line(server: &ServerInfo, theme: &DvTheme) -> (gpui::Hsla, String) {
    let c = &theme.colors;
    match &server.state {
        ServerState::Ready => match server.progress.first() {
            Some(progress) => (
                c.yellow,
                match progress.percentage {
                    Some(percent) => format!("{} {percent}%", progress.title.to_lowercase()),
                    None => format!("{}…", progress.title.to_lowercase()),
                },
            ),
            None => (c.green, "ready".to_string()),
        },
        ServerState::Starting => (c.fg3, "starting…".to_string()),
        ServerState::Restarting => (c.yellow, "restarting…".to_string()),
        ServerState::Waiting => (c.yellow, "waiting for the connection".to_string()),
        ServerState::Failed(_) => (c.red, "failed".to_string()),
    }
}

/// One status bar item: a dot and its text, a click opening its details.
fn status_item(
    name: String,
    dot: gpui::Hsla,
    text: String,
    theme: &DvTheme,
) -> gpui::Stateful<gpui::Div> {
    let hover = theme.colors.control_hover;
    div()
        .id(gpui::ElementId::Name(name.clone().into()))
        .flex_none()
        .flex()
        .items_center()
        .gap(theme.spacing.sm)
        .h(px(22.))
        .px(theme.spacing.sm)
        .rounded(theme.radii.control)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .child(div().size(px(7.)).rounded_full().bg(dot))
        .child(text)
        .debug_selector(move || name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stream_shows_lines_near_a_change() {
        use dv_core::diff::{DiffLine, TextDiff};
        let line = |kind, old: Option<u32>, new: Option<u32>| DiffLine {
            kind,
            old_no: old,
            new_no: new,
            text: String::new(),
            words: Vec::new(),
        };
        let mut lines: Vec<DiffLine> = (1..=20)
            .map(|n| line(LineKind::Context, Some(n), Some(n)))
            .collect();
        lines[9] = line(LineKind::Added, None, Some(10));
        let text = TextDiff {
            lines,
            old_eof_newline: true,
            new_eof_newline: true,
            crlf: false,
        };
        let near = |n: u32| near_change(&text, n);
        assert!(near(10) && near(7) && near(13));
        assert!(!near(6) && !near(14) && !near(1) && !near(99));
    }
}
