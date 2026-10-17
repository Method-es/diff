//! Code intelligence in the app (PLAN §18.1, §18.5, §18.6): hover, go to definition and find
//! references from the project's own language servers.
//!
//! - [`servers`]: the app-wide [`LanguageServers`] service, one server per worktree, spec and
//!   executable, started through the review's backend (inside the distro for a WSL repository),
//!   restarted when it crashes, stopped [`servers::LINGER`] after the worktree's last review
//!   closed.
//! - [`CodeIntel`] (this module), one per open review: the requests the stream and the file
//!   view make, the hover card they show, the references list the panel's References tab shows
//!   ([`references`]), and the notice for a definition outside the repository. The shell owns
//!   it with the review's session, hands it to both panes and to the panel, and does the
//!   navigation its events ask for ([`IntelEvent`]).
//! - [`hover`]: the hover card; [`references`]: the References tab.
//!
//! **Which text a server reads** (PLAN §18.2). A request opens its file's text as the pane
//! shows it (`didOpen`, one version of a path at a time): the working tree's for a review whose
//! new side is the working tree, so every answer is exact; any other side's text (an old side, a
//! round's snapshot, the index, a commit) overrides the disk for that one file, and the answer
//! says it was "read against the working tree" ([`HoverCard::against_worktree`]).
//!
//! **Nothing blocks the UI thread.** A request waits for its file's server to be ready (it may
//! still be resolving or starting: up to the request's own timeout), then opens the document,
//! sends the request and waits for the answer on a background thread; dropping the request
//! ([`request::InFlight`]) cancels it at once. Hover asks after the pointer rested 350 ms
//! ([`HOVER_DELAY`], the pane's), definition and references when asked.

pub mod hover;
pub mod references;
pub mod request;
pub mod servers;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dv_core::diff::FileDiff;
use dv_core::review::{AnchorSide, side_lines};
use dv_core::{Location, Side};
use dv_lsp::{
    HoverInfo, LanguageServerSpec, LspClient, RequestError, SymbolLocation, TextPosition, Timeouts,
};
use gpui::{App, AppContext as _, Bounds, Context, Entity, EventEmitter, Pixels, Subscription};

use crate::failure::Failure;
use crate::services::ActiveServices as _;
use crate::session::{ReviewSession, SessionEvent};
use references::{References, ReferencesKind};
use request::{Arming, InFlight};
pub use servers::{LanguageServers, Lookup, ServerInfo, ServerState};

/// How long the pointer rests on an identifier before its hover is asked for (PLAN §18.1).
pub const HOVER_DELAY: Duration = Duration::from_millis(350);

/// How often a request looks again for its file's server while that is being found or starts.
const SERVER_POLL: Duration = Duration::from_millis(25);

/// Which pane a symbol is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pane {
    Stream,
    FileView,
}

/// What the reviewer points at in a line of a file as a pane shows it: an identifier (the
/// caret's, a `Ctrl`+click's), or for a hover the token under the pointer
/// ([`hover_token_at`]: a class name in a string is one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolAt {
    pub path: String,
    pub side: AnchorSide,
    /// Its line's number on `side`.
    pub line: u32,
    /// The token's bytes in the line's text.
    pub range: Range<usize>,
    /// The token.
    pub name: String,
    /// The byte a server is asked at: the pointer's character for a hover, else the token's
    /// first.
    pub offset: usize,
    /// The pane shows the file as the working tree holds it now (the file view at the working
    /// tree, PLAN §18.1 "Go to definition"), whatever the review's sides are: a server reads
    /// that text from the disk (PLAN §18.2).
    pub worktree: bool,
}

impl SymbolAt {
    /// Where a server is asked about it: its line (0-based) and [`SymbolAt::offset`].
    fn position(&self) -> TextPosition {
        TextPosition::new(self.line.saturating_sub(1), self.offset as u32)
    }

    /// Whether `other` is in the same line of the same file and side.
    pub fn same_line(&self, other: &SymbolAt) -> bool {
        self.path == other.path && self.side == other.side && self.line == other.line
    }
}

/// An open hover card (PLAN §18.1 "Hover"): the answers of every server of the file that said
/// anything, in the registry's order, each added as it lands.
#[derive(Clone, Debug)]
pub struct HoverCard {
    pub at: SymbolAt,
    pub pane: Pane,
    /// The token's bounds in the window when it was asked for; the card hangs under it.
    pub token: Bounds<Pixels>,
    /// Empty only for a card that says there is nothing ([`HoverCard::note`]).
    pub sections: Vec<HoverSection>,
    /// What a card without sections says: every server answered with nothing for text that
    /// wasn't the working tree's (a removed line's symbol), which they read against the
    /// working tree, so the reviewer knows why (PLAN §18.1 "Hover").
    pub note: Option<&'static str>,
    /// The bytes of the line the card is about: the union of the ranges the servers answered
    /// for (Tailwind's is the whole class, `bg-red-500`), else the token asked about. The
    /// card hangs under it, and the pointer anywhere in it keeps the card without asking again.
    pub span: Range<usize>,
    /// The server read the file's text in place of the disk's, and the rest of the project
    /// from the working tree (PLAN §18.2).
    pub against_worktree: bool,
    /// Tells this card's Markdown views from another's; it stays as sections are added.
    pub id: u64,
}

impl HoverCard {
    /// Its text as automation reads it: the sections' texts joined by blank lines (its note,
    /// for a card without sections).
    pub fn text(&self) -> String {
        if self.sections.is_empty() {
            return self.note.unwrap_or_default().to_string();
        }
        self.sections
            .iter()
            .map(HoverSection::text)
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Adds `section` in its place in the registry's order.
    fn add(&mut self, section: HoverSection) {
        let at = self
            .sections
            .iter()
            .position(|known| known.order > section.order)
            .unwrap_or(self.sections.len());
        self.sections.insert(at, section);
        self.span = span_of(&self.at, &self.sections);
    }

    /// Whether the card is about `at`: the same line, and its asked byte inside the span.
    pub fn covers(&self, at: &SymbolAt) -> bool {
        self.at.same_line(at) && self.span.contains(&at.offset)
    }
}

/// The bytes of `at`'s line the `sections` answered for (module docs, [`HoverCard::span`]):
/// the union of the ranges on that line that hold the asked byte, else the token.
fn span_of(at: &SymbolAt, sections: &[HoverSection]) -> Range<usize> {
    let line = at.line.saturating_sub(1);
    sections
        .iter()
        .filter_map(|section| section.info.range)
        .filter(|range| range.start.line == line && range.end.line == line)
        .map(|range| range.start.column as usize..range.end.column as usize)
        .filter(|range| range.contains(&at.offset))
        .reduce(|a, b| a.start.min(b.start)..a.end.max(b.end))
        .unwrap_or_else(|| at.range.clone())
}

/// One server's answer in a hover card.
#[derive(Clone, Debug)]
pub struct HoverSection {
    /// Its spec's place in the registry (`languages::ALL`): sections stack in that order.
    pub order: usize,
    pub spec_id: &'static str,
    /// What the card labels it with when more than one server answered.
    pub server: &'static str,
    pub info: HoverInfo,
}

impl HoverSection {
    /// Its text as automation reads it: the parts joined by blank lines.
    pub fn text(&self) -> String {
        self.info
            .parts
            .iter()
            .map(|part| match part {
                dv_lsp::HoverPart::Markdown(text) | dv_lsp::HoverPart::Plain(text) => text.clone(),
                dv_lsp::HoverPart::Code { text, .. } => text.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// A hover being asked for, before any server answered with anything: one request per server
/// of the file, by its registry place.
struct Hovering {
    at: SymbolAt,
    pane: Pane,
    requests: Vec<(usize, InFlight)>,
    /// Every server that answered so far answered, with nothing (none failed or was missing).
    all_empty: bool,
}

/// One server's answer to a hover, as the card takes it.
enum Answered {
    Something(HoverSection),
    /// It answered, with nothing.
    Empty,
    /// No server, or a request that failed quietly.
    Nothing,
}

/// What a card says when every server answered with nothing for another side's text.
pub const NO_INFORMATION: &str = "No information: read against the working tree";

/// Where a place was asked from: the symbol, the pane, and whether the server read that
/// file's text from the working tree (`worktree`) or from the text dv opened for it. A place in
/// the same file answered against another side's text is on that side (`crate::shell::intel`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asked {
    pub at: SymbolAt,
    pub pane: Pane,
    pub worktree: bool,
    /// A go to definition's answer: dropped when the reviewer moved on while it was asked for.
    pub definition: bool,
}

/// What the views around a review's code intelligence are asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntelEvent {
    /// Show this place (go to definition, a references row, a link in a hover card), asked
    /// from `asked` when that is known.
    GoTo {
        location: SymbolLocation,
        asked: Option<Asked>,
    },
    /// A go to definition was asked for: the shell notes where the reviewer is, and drops an
    /// answer that lands after they moved on.
    DefinitionAsked,
    /// Show the panel's References tab: a references list (or several definitions) is there.
    ShowReferences,
}

/// How many files' texts [`CodeIntel`] keeps for its requests: the ones asked about last.
pub const DOCUMENTS_KEPT: usize = 32;

/// The text a pane shows of one file at one side, as a server reads it (module docs).
#[derive(Clone)]
struct Document {
    /// The diff it was cut from, by identity.
    diff: Arc<FileDiff>,
    text: Arc<str>,
    /// When it was last asked about (a count of requests): the least recent goes first.
    used: u64,
}

/// A review's code intelligence (module docs).
pub struct CodeIntel {
    session: Entity<ReviewSession>,
    servers: Entity<LanguageServers>,
    worktree: Location,
    /// The servers for the review's files were asked for (once its list landed).
    served: bool,
    /// The hover being asked for, and the card shown.
    hovering: Option<Hovering>,
    hover: Option<HoverCard>,
    /// The requests of the card shown whose servers haven't answered yet, by registry place.
    hover_more: Vec<(usize, InFlight)>,
    next_hover: u64,
    definition: Option<InFlight>,
    /// How many places the last go to definition found (`state.intel.definitions`).
    last_definitions: Option<usize>,
    references: Option<References>,
    references_request: Option<InFlight>,
    /// Tells each references list from the one before.
    next_list: u64,
    /// Why the last navigation couldn't show its place (a definition outside the repository).
    notice: Option<Failure>,
    /// The texts requests were asked about, at most [`DOCUMENTS_KEPT`].
    documents: HashMap<(String, AnchorSide), Document>,
    documents_used: u64,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<IntelEvent> for CodeIntel {}

impl CodeIntel {
    /// The code intelligence of `session`'s review: its worktree's servers are in use until
    /// this is dropped.
    pub fn new(session: Entity<ReviewSession>, cx: &mut Context<Self>) -> Self {
        // A review opening: `settings.json`'s `lsp` as it is now, before its servers are found.
        reload_lsp_settings(cx);
        let servers = cx.services().language_servers.clone();
        let (backend, connection) = {
            let session = session.read(cx);
            (session.backend().clone(), session.connection().cloned())
        };
        let worktree = servers.update(cx, |servers, cx| servers.acquire(backend, connection, cx));
        let subscriptions = vec![
            cx.subscribe(&session, |this, _, event: &SessionEvent, cx| {
                this.on_session_event(event, cx)
            }),
            cx.observe(&servers, |_, _, cx| cx.notify()),
            cx.on_release({
                let servers = servers.clone();
                let worktree = worktree.clone();
                move |_, cx| {
                    servers.update(cx, |servers, cx| servers.release(&worktree, cx));
                }
            }),
        ];
        let mut intel = Self {
            session,
            servers,
            worktree,
            served: false,
            hovering: None,
            hover: None,
            hover_more: Vec::new(),
            next_hover: 0,
            definition: None,
            last_definitions: None,
            references: None,
            references_request: None,
            next_list: 0,
            notice: None,
            documents: HashMap::new(),
            documents_used: 0,
            _subscriptions: subscriptions,
        };
        intel.serve(cx);
        intel
    }

    pub fn session(&self) -> &Entity<ReviewSession> {
        &self.session
    }

    /// The worktree whose servers it uses.
    pub fn worktree(&self) -> &Location {
        &self.worktree
    }

    /// The servers of the review's worktree.
    pub fn servers(&self, cx: &App) -> Vec<ServerInfo> {
        self.servers.read(cx).servers(&self.worktree)
    }

    /// Why each server the review's files need is missing.
    pub fn missing(&self, cx: &App) -> Vec<dv_lsp::Missing> {
        self.servers.read(cx).missing(&self.worktree)
    }

    /// **Retry** on a failed server.
    pub fn retry(&mut self, server: u64, cx: &mut Context<Self>) {
        let worktree = self.worktree.clone();
        self.servers
            .update(cx, |servers, cx| servers.retry(&worktree, server, cx));
    }

    /// **Check again** on a missing server (PLAN §18.5): `settings.json`'s `lsp` is read
    /// again first, so a command written there meanwhile is used.
    pub fn check_again(&mut self, cx: &mut Context<Self>) {
        self.reload_lsp_settings(cx);
        let worktree = self.worktree.clone();
        self.servers
            .update(cx, |servers, cx| servers.check_again(&worktree, cx));
    }

    /// The window came back to the front: a missing server is looked for again (throttled;
    /// the shell has read `settings.json`'s `lsp` again first).
    pub fn window_activated(&mut self, cx: &mut Context<Self>) {
        let worktree = self.worktree.clone();
        self.servers
            .update(cx, |servers, cx| servers.recheck_missing(&worktree, cx));
    }

    /// Reads `settings.json`'s `lsp` again and hands it to the servers ([`reload_lsp_settings`]).
    fn reload_lsp_settings(&mut self, cx: &mut Context<Self>) {
        reload_lsp_settings(cx);
    }

    /// How many times the worktree's servers were looked for again.
    pub fn rechecks(&self, cx: &App) -> u32 {
        self.servers.read(cx).rechecks(&self.worktree)
    }

    /// How the review's worktree names its files, for links a server answers with.
    pub fn paths(&self, cx: &App) -> dv_lsp::WorkspacePaths {
        dv_lsp::WorkspacePaths::new(&self.session.read(cx).backend().repo().toplevel)
    }

    /// Shows `location` (a references row, a link in a hover card): the shell does the
    /// navigation ([`IntelEvent::GoTo`]); the hover card closes.
    pub fn go_to(
        &mut self,
        location: SymbolLocation,
        asked: Option<Asked>,
        cx: &mut Context<Self>,
    ) {
        self.close_hover(cx);
        cx.emit(IntelEvent::GoTo { location, asked });
    }

    /// The hover card shown, if any.
    pub fn hover_card(&self) -> Option<&HoverCard> {
        self.hover.as_ref()
    }

    /// The symbol whose hover is being asked for, if any.
    pub fn hovering(&self) -> Option<(&SymbolAt, Pane)> {
        self.hovering
            .as_ref()
            .map(|asking| (&asking.at, asking.pane))
    }

    /// The references list (or several definitions) the References tab shows, if any.
    pub fn references(&self) -> Option<&References> {
        self.references.as_ref()
    }

    /// How many places the last go to definition found, once it answered.
    pub fn last_definitions(&self) -> Option<usize> {
        self.last_definitions
    }

    /// Why the last place asked for can't be shown, until dismissed.
    pub fn notice(&self) -> Option<&Failure> {
        self.notice.as_ref()
    }

    pub fn dismiss_notice(&mut self, cx: &mut Context<Self>) {
        if self.notice.take().is_some() {
            cx.notify();
        }
    }

    /// Shows `notice` (the shell, for a place it can't show).
    pub fn set_notice(&mut self, notice: Failure, cx: &mut Context<Self>) {
        self.notice = Some(notice);
        cx.notify();
    }

    /// No request is waiting and no server of its worktree is being found or starting:
    /// automation's `settled`.
    pub fn is_settled(&self, cx: &App) -> bool {
        self.hovering.is_none()
            && self.hover_more.is_empty()
            && self.definition.is_none()
            && self.references_request.is_none()
            && self
                .references
                .as_ref()
                .is_none_or(|references| !references.is_loading() && !references.is_reading())
            && self.servers.read(cx).is_settled()
    }

    fn on_session_event(&mut self, event: &SessionEvent, cx: &mut Context<Self>) {
        match event {
            SessionEvent::FilesChanged => {
                self.serve(cx);
                self.refresh_changed(None, cx);
            }
            SessionEvent::FileLoaded(path) => self.refresh_changed(Some(path), cx),
            SessionEvent::WorktreeChanged { paths, rescan } => {
                let worktree = self.worktree.clone();
                let (paths, rescan) = (paths.clone(), *rescan);
                self.servers.update(cx, |servers, cx| {
                    servers.files_changed(&worktree, &paths, rescan, cx)
                });
                cx.notify();
            }
            _ => {}
        }
    }

    /// The references rows of `path` (every file's, `None`) work out again whether the review
    /// changed their lines: the session's diff of it landed, or its list changed.
    fn refresh_changed(&mut self, path: Option<&str>, cx: &mut Context<Self>) {
        let Some(list) = self.references.as_mut() else {
            return;
        };
        if path.is_some_and(|path| !list.has_file(path)) {
            return;
        }
        let shape = list.shape;
        list.refresh_changed(path, self.session.read(cx));
        if list.shape != shape {
            cx.notify();
        }
    }

    /// Asks for the servers of the review's files, once its list landed (PLAN §18.5).
    fn serve(&mut self, cx: &mut Context<Self>) {
        let session = self.session.read(cx);
        if self.served || session.spec().is_none() || session.is_first_load() {
            return;
        }
        self.served = true;
        let paths: Vec<String> = session
            .target_files()
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        let worktree = self.worktree.clone();
        self.servers
            .update(cx, |servers, cx| servers.serve(&worktree, paths, cx));
    }

    /// Whether a server reads `at`'s text as the disk holds it: a pane showing the working
    /// tree, or the new side of a review whose new side is the working tree (module docs).
    pub fn reads_worktree(&self, at: &SymbolAt, cx: &App) -> bool {
        at.worktree
            || (at.side == AnchorSide::New
                && self.session.read(cx).new_side() == Some(&Side::Worktree))
    }

    /// `at`'s file's text on its side, from `diff` (the diff the pane shows the file from),
    /// built once per diff.
    fn document(&mut self, at: &SymbolAt, diff: &Arc<FileDiff>) -> Option<Arc<str>> {
        let key = (at.path.clone(), at.side);
        self.documents_used += 1;
        if let Some(known) = self.documents.get_mut(&key)
            && Arc::ptr_eq(&known.diff, diff)
        {
            known.used = self.documents_used;
            return Some(known.text.clone());
        }
        // Room for one more: the least recently asked about goes.
        if self.documents.len() >= DOCUMENTS_KEPT
            && !self.documents.contains_key(&key)
            && let Some(oldest) = self
                .documents
                .iter()
                .min_by_key(|(_, document)| document.used)
                .map(|(key, _)| key.clone())
        {
            self.documents.remove(&oldest);
        }
        let text_diff = diff.as_text()?;
        let mut text = side_lines(text_diff, at.side).join("\n");
        let eof = match at.side {
            AnchorSide::Old => text_diff.old_eof_newline,
            AnchorSide::New => text_diff.new_eof_newline,
        };
        if eof && !text.is_empty() {
            text.push('\n');
        }
        let text: Arc<str> = Arc::from(text);
        self.documents.insert(
            key,
            Document {
                diff: diff.clone(),
                text: text.clone(),
                used: self.documents_used,
            },
        );
        Some(text)
    }

    /// Asks for `at`'s hover (the pointer rested on it, or `Ctrl I`), shown in `pane` under
    /// `token`, of every server of the file at once (PLAN §18.4): the card opens with the first
    /// answer that says anything and takes the others in the registry's order as they land.
    /// The hover asked for before is abandoned; the same symbol again does nothing.
    pub fn hover(
        &mut self,
        at: SymbolAt,
        pane: Pane,
        token: Bounds<Pixels>,
        diff: &Arc<FileDiff>,
        cx: &mut Context<Self>,
    ) {
        // The card's span, or the token being asked about: nothing new to ask.
        if self
            .hover
            .as_ref()
            .is_some_and(|card| card.pane == pane && card.covers(&at))
            || self.hovering.as_ref().is_some_and(|asking| {
                asking.pane == pane && asking.at.same_line(&at) && asking.at.range == at.range
            })
        {
            return;
        }
        self.hovering = None;
        self.hover_more.clear();
        if self.hover.take().is_some() {
            cx.notify();
        }
        let Some(text) = self.document(&at, diff) else {
            return;
        };
        let against_worktree = !self.reads_worktree(&at, cx);
        let mut requests = Vec::new();
        for (order, spec) in dv_lsp::languages::ALL.iter().copied().enumerate() {
            if spec.language_id(&at.path).is_none() {
                continue;
            }
            let asked = at.clone();
            let request = self.request(
                &at,
                text.clone(),
                against_worktree,
                Timeouts::default().hover,
                vec![spec],
                "hoverProvider",
                |client, path, position| client.hover(path, position),
                |_| true,
                move |this, answer, cx| {
                    let answered = match answer {
                        Some(Ok(Some(info))) if !info.parts.is_empty() => {
                            Answered::Something(HoverSection {
                                order,
                                spec_id: spec.id,
                                server: spec.name,
                                info,
                            })
                        }
                        Some(Ok(_)) => Answered::Empty,
                        // Nothing to show: no server, or a request that failed quietly (PLAN
                        // §18.5).
                        Some(Err(error)) => {
                            log::info!("hover ({}): {error}", spec.id);
                            Answered::Nothing
                        }
                        None => Answered::Nothing,
                    };
                    this.hover_answered(order, asked, pane, token, against_worktree, answered, cx);
                },
                cx,
            );
            requests.push((order, request));
        }
        if requests.is_empty() {
            return;
        }
        self.hovering = Some(Hovering {
            at,
            pane,
            requests,
            all_empty: true,
        });
        cx.notify();
    }

    /// One server's answer to the hover asked for `at` in `pane`: the first that says anything
    /// opens the card, a later one joins it in its place; when every server answered with
    /// nothing for another side's text, a card says so ([`NO_INFORMATION`]).
    #[allow(clippy::too_many_arguments)]
    fn hover_answered(
        &mut self,
        order: usize,
        at: SymbolAt,
        pane: Pane,
        token: Bounds<Pixels>,
        against_worktree: bool,
        answered: Answered,
        cx: &mut Context<Self>,
    ) {
        if let Some(asking) = self
            .hovering
            .as_mut()
            .filter(|asking| asking.at == at && asking.pane == pane)
        {
            asking.requests.retain(|(known, _)| *known != order);
            asking.all_empty &= matches!(answered, Answered::Empty);
            match answered {
                Answered::Something(section) => {
                    let rest = self
                        .hovering
                        .take()
                        .map(|asking| asking.requests)
                        .unwrap_or_default();
                    self.next_hover += 1;
                    let sections = vec![section];
                    self.hover = Some(HoverCard {
                        span: span_of(&at, &sections),
                        at,
                        pane,
                        token,
                        sections,
                        note: None,
                        against_worktree,
                        id: self.next_hover,
                    });
                    self.hover_more = rest;
                }
                _ if asking.requests.is_empty() => {
                    let note = against_worktree && asking.all_empty;
                    self.hovering = None;
                    if note {
                        self.next_hover += 1;
                        self.hover = Some(HoverCard {
                            span: at.range.clone(),
                            at,
                            pane,
                            token,
                            sections: Vec::new(),
                            note: Some(NO_INFORMATION),
                            against_worktree,
                            id: self.next_hover,
                        });
                    }
                }
                _ => {}
            }
        } else if let Some(card) = self
            .hover
            .as_mut()
            .filter(|card| card.at == at && card.pane == pane)
        {
            self.hover_more.retain(|(known, _)| *known != order);
            if let Answered::Something(section) = answered {
                card.add(section);
            }
        }
        cx.notify();
    }

    /// Closes the hover card, leaving a hover asked for since (another token's) alone.
    pub fn close_card(&mut self, cx: &mut Context<Self>) {
        self.hover_more.clear();
        if self.hover.take().is_some() {
            cx.notify();
        }
    }

    /// Closes the hover card, abandoning a hover still asked for.
    pub fn close_hover(&mut self, cx: &mut Context<Self>) {
        self.hover_more.clear();
        if self.hovering.take().is_some() | self.hover.take().is_some() {
            cx.notify();
        }
    }

    /// Go to definition at `at` (`F12`, `Ctrl`+click, the hover card's button): one place is
    /// shown ([`IntelEvent::GoTo`]), several list in the References tab, none does nothing.
    pub fn definition(
        &mut self,
        at: SymbolAt,
        pane: Pane,
        diff: &Arc<FileDiff>,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self.document(&at, diff) else {
            return;
        };
        self.close_hover(cx);
        cx.emit(IntelEvent::DefinitionAsked);
        let against_worktree = !self.reads_worktree(&at, cx);
        // The text the server reads in place of the disk's, kept for a list's rows of it.
        let own = against_worktree.then(|| text.clone());
        let from = at.clone();
        let request = self.request(
            &at,
            text,
            against_worktree,
            Timeouts::default().definition,
            servers_of(&at.path),
            "definitionProvider",
            |client, path, position| client.definition(path, position),
            |locations| !locations.is_empty(),
            move |this, answer, cx| {
                this.definition = None;
                let mut locations = match answer {
                    Some(Ok(locations)) => locations,
                    Some(Err(error)) => {
                        log::info!("definition: {error}");
                        Vec::new()
                    }
                    None => Vec::new(),
                };
                locations.dedup();
                this.last_definitions = Some(locations.len());
                match locations.len() {
                    0 => {}
                    1 => cx.emit(IntelEvent::GoTo {
                        location: locations.remove(0),
                        asked: Some(Asked {
                            at: from,
                            pane,
                            worktree: !against_worktree,
                            definition: true,
                        }),
                    }),
                    _ => {
                        this.show_list(
                            ReferencesKind::Definitions,
                            from,
                            pane,
                            own,
                            Ok(locations),
                            cx,
                        );
                    }
                }
                cx.notify();
            },
            cx,
        );
        self.last_definitions = None;
        self.definition = Some(request);
        cx.notify();
    }

    /// Find references at `at` (`Shift F12`, the hover card's button): the References tab
    /// shows them once they land, loading meanwhile.
    pub fn references_at(
        &mut self,
        at: SymbolAt,
        pane: Pane,
        diff: &Arc<FileDiff>,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self.document(&at, diff) else {
            return;
        };
        self.close_hover(cx);
        let against_worktree = !self.reads_worktree(&at, cx);
        self.next_list += 1;
        self.references = Some(References::loading(
            ReferencesKind::References,
            at.clone(),
            pane,
            against_worktree,
            self.references.as_ref().is_some_and(|r| r.in_review),
            self.next_list,
        ));
        cx.emit(IntelEvent::ShowReferences);
        let from = at.clone();
        let own = against_worktree.then(|| text.clone());
        let request = self.request(
            &at,
            text,
            against_worktree,
            Timeouts::default().references,
            servers_of(&at.path),
            "referencesProvider",
            |client, path, position| client.references(path, position, true),
            |locations| !locations.is_empty(),
            move |this, answer, cx| {
                this.references_request = None;
                let found = match answer {
                    Some(Ok(locations)) => Ok(locations),
                    Some(Err(error)) => Err(error.to_string()),
                    None => Err("no language server serves this file".to_string()),
                };
                this.show_list(ReferencesKind::References, from, pane, own, found, cx);
            },
            cx,
        );
        self.references_request = Some(request);
        cx.notify();
    }

    /// The References tab's list: `found`'s places, shown at once, their lines read in the
    /// background ([`references::read_lines`]); the asking file's from `own`, the text the
    /// server read in place of the disk's when it was another side's (PLAN §18.2), as it was
    /// sent with the request (whatever the documents kept since).
    fn show_list(
        &mut self,
        kind: ReferencesKind,
        from: SymbolAt,
        pane: Pane,
        own: Option<Arc<str>>,
        found: Result<Vec<SymbolLocation>, String>,
        cx: &mut Context<Self>,
    ) {
        let in_review = self.references.as_ref().is_some_and(|r| r.in_review);
        self.next_list += 1;
        let id = self.next_list;
        let against_worktree = own.is_some();
        let own = own.map(|text| (from.side, text));
        let mut list = References::loading(kind, from, pane, against_worktree, in_review, id);
        match found {
            Ok(locations) => {
                let files = references::group(locations, &list.from, own, self.session.read(cx));
                let jobs = references::ReadJob::for_files(&files);
                list.files = Some(files);
                let backend = self.session.read(cx).backend().clone();
                let (send, mut landed) = futures::channel::mpsc::unbounded();
                let job = cx.background_spawn(async move {
                    references::read_lines(backend.as_ref(), jobs, send)
                });
                list.reading = Some(cx.spawn(async move |this, cx| {
                    use futures::StreamExt as _;
                    while let Some(landed) = landed.next().await {
                        let applied = this.update(cx, |this, cx| {
                            let session = this.session.clone();
                            let Some(list) = this.references.as_mut().filter(|l| l.id == id) else {
                                return false;
                            };
                            match landed {
                                references::Landed::Text { index, texts } => {
                                    list.text_landed(index, texts, session.read(cx))
                                }
                                references::Landed::Tokens { index, tokens } => {
                                    list.tokens_landed(index, tokens)
                                }
                            }
                            cx.notify();
                            true
                        });
                        if !matches!(applied, Ok(true)) {
                            return;
                        }
                    }
                    job.await;
                    this.update(cx, |this, cx| {
                        if let Some(list) = this.references.as_mut().filter(|l| l.id == id) {
                            list.reading = None;
                        }
                        cx.notify();
                    })
                    .ok();
                }));
            }
            Err(error) => list.error = Some(error),
        }
        self.references = Some(list);
        self.load_referenced(cx);
        cx.emit(IntelEvent::ShowReferences);
        cx.notify();
    }

    /// Has the session load the diffs of the review's files the references are in, so their
    /// rows can say whether the review changed them.
    fn load_referenced(&mut self, cx: &mut Context<Self>) {
        let Some(paths) = self.references.as_ref().map(References::worktree_paths) else {
            return;
        };
        self.session
            .update(cx, |session, cx| session.load_for_references(paths, cx));
    }

    /// The References tab's **In this review**.
    pub fn set_in_review(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Some(list) = self.references.as_mut()
            && list.in_review != on
        {
            list.in_review = on;
            cx.notify();
        }
    }

    /// Closes the references list (its tab goes).
    pub fn close_references(&mut self, cx: &mut Context<Self>) {
        self.references_request = None;
        if self.references.take().is_some() {
            self.session.update(cx, |session, cx| {
                session.load_for_references(Vec::new(), cx)
            });
            cx.notify();
        }
    }

    /// One request about `at`, whose file's text is `text` (module docs), to `specs`' servers
    /// (PLAN §18.4): each is waited for while it is found or starts, skipped when its
    /// capabilities lack `provider`, and asked by `ask` on a background thread, all at once.
    /// The answer is the first that says something (`is_something`) in `specs`' order: a
    /// server earlier in the order that hasn't answered yet holds a later one's answer back
    /// until it answers or `timeout` (one deadline for all) passes. It goes to `done` on the UI
    /// thread (`None` without a server; with nothing useful, an empty answer over an error). A
    /// document that isn't the working tree's (`close_after`) is closed once answered, so the
    /// server reads the file from the working tree again for every other request (PLAN §18.2).
    #[allow(clippy::too_many_arguments)]
    fn request<T: Send + 'static>(
        &mut self,
        at: &SymbolAt,
        text: Arc<str>,
        close_after: bool,
        timeout: Duration,
        specs: Vec<&'static LanguageServerSpec>,
        provider: &'static str,
        ask: fn(&LspClient, &str, TextPosition) -> dv_lsp::Pending<T>,
        is_something: fn(&T) -> bool,
        done: impl FnOnce(&mut Self, Option<Result<T, RequestError>>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> InFlight {
        let path = at.path.clone();
        let position = at.position();
        let servers = self.servers.clone();
        let worktree = self.worktree.clone();
        InFlight::new(move |arming: Arming| {
            cx.spawn(async move |this, cx| {
                /// Where one server's part of the request is.
                enum Part<T> {
                    /// Its server is being found or starts.
                    Waiting,
                    /// Asked; the answer comes through the channel.
                    Asked,
                    /// Answered (`None`: no server, or one that doesn't do this).
                    Done(Option<Result<T, RequestError>>),
                }
                let deadline = Instant::now() + timeout;
                let (answers, answered) = std::sync::mpsc::channel();
                let mut parts: Vec<Part<T>> = specs.iter().map(|_| Part::Waiting).collect();
                loop {
                    for (index, spec) in specs.iter().copied().enumerate() {
                        if !matches!(parts[index], Part::Waiting) {
                            continue;
                        }
                        let lookup = this
                            .update(cx, |_, cx| {
                                servers.update(cx, |servers, cx| {
                                    servers.lookup(&worktree, &path, spec, cx)
                                })
                            })
                            .unwrap_or(Lookup::Unavailable);
                        parts[index] = match lookup {
                            Lookup::Ready { client, .. } if !provides(&client, provider) => {
                                Part::Done(None)
                            }
                            Lookup::Ready {
                                client,
                                language_id,
                                ..
                            } => {
                                let (path, text, arming) =
                                    (path.clone(), text.clone(), arming.clone());
                                let answers = answers.clone();
                                cx.background_spawn(async move {
                                    let answer = if arming.is_abandoned() {
                                        None
                                    } else if let Err(error) =
                                        client.open_document(&path, language_id, text)
                                    {
                                        Some(Err(error))
                                    } else {
                                        let pending = ask(&client, &path, position);
                                        let answer = if arming.arm(&pending) {
                                            Some(pending.wait())
                                        } else {
                                            drop(pending);
                                            None
                                        };
                                        if close_after {
                                            client.close_document(&path);
                                        }
                                        answer
                                    };
                                    let _ = answers.send((index, answer));
                                })
                                .detach();
                                Part::Asked
                            }
                            Lookup::Unavailable => Part::Done(None),
                            Lookup::Pending if Instant::now() >= deadline => Part::Done(None),
                            Lookup::Pending => Part::Waiting,
                        };
                    }
                    while let Ok((index, answer)) = answered.try_recv() {
                        parts[index] = Part::Done(answer);
                    }
                    let late = Instant::now() >= deadline;
                    // The first useful answer in order, unless an earlier server may still
                    // give one (until the deadline).
                    let mut winner = None;
                    let mut open = false;
                    for (index, part) in parts.iter().enumerate() {
                        match part {
                            Part::Done(Some(Ok(found))) if is_something(found) => {
                                winner = Some(index);
                                break;
                            }
                            Part::Done(_) => {}
                            Part::Waiting | Part::Asked if late => {}
                            Part::Waiting | Part::Asked => {
                                open = true;
                                break;
                            }
                        }
                    }
                    if winner.is_none() && open {
                        cx.background_executor().timer(SERVER_POLL).await;
                        continue;
                    }
                    let mut answers: Vec<Option<Result<T, RequestError>>> = parts
                        .into_iter()
                        .map(|part| match part {
                            Part::Done(answer) => answer,
                            Part::Waiting | Part::Asked => None,
                        })
                        .collect();
                    let best = match winner {
                        Some(index) => answers[index].take(),
                        None => {
                            let empty = answers.iter().position(|a| matches!(a, Some(Ok(_))));
                            let failed = answers.iter().position(Option::is_some);
                            empty.or(failed).and_then(|index| answers[index].take())
                        }
                    };
                    this.update(cx, |this, cx| done(this, best, cx)).ok();
                    return;
                }
            })
        })
    }
}

/// Reads `settings.json`'s `lsp` again (`Settings::reload_lsp`) and hands it to the language
/// servers, which change nothing when it is what they have (PLAN §18.5): on **Check again**, a
/// review opening and the window coming back to the front, a review open or not.
pub fn reload_lsp_settings(cx: &mut App) {
    let settings = cx.services().settings.clone();
    if let Some(lsp) = settings.update(cx, |settings, _| settings.reload_lsp()) {
        let servers = cx.services().language_servers.clone();
        servers.update(cx, |servers, cx| servers.set_settings(lsp, cx));
    }
}

/// Whether `client`'s server does `provider` (`hoverProvider`, `definitionProvider`,
/// `referencesProvider`): its capabilities say so with anything but `false` or `null`.
fn provides(client: &LspClient, provider: &str) -> bool {
    let capabilities = client.capabilities();
    match capabilities.get(provider) {
        Some(serde_json::Value::Bool(false)) | Some(serde_json::Value::Null) => false,
        Some(_) => true,
        // A server that said nothing at all about its capabilities is asked anyway.
        None => capabilities.as_object().is_none_or(|map| map.is_empty()),
    }
}

/// The specs that serve `path`, in the registry's order: who a definition or references asks.
fn servers_of(path: &str) -> Vec<&'static LanguageServerSpec> {
    dv_lsp::languages::for_path(path)
        .map(|(spec, _)| spec)
        .collect()
}

/// The token a hover at byte `offset` of `text` is about (PLAN §18.1 "Hover"): `None` on
/// whitespace, a quote or past the end; else the identifier the character there is part of
/// (alphanumerics, `_`, `$`; a number too, `500` of `bg-red-500`), else the run of characters
/// around it that aren't whitespace or quotes (`-` of `bg-red-500`, `[--x:1px]`). The server is
/// asked at the pointer's own character, and its answer's range takes over the card's place
/// ([`HoverCard::span`]).
pub fn hover_token_at(text: &str, offset: usize) -> Option<Range<usize>> {
    let ch = text.get(offset..)?.chars().next()?;
    let breaks = |ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | '`');
    if breaks(ch) {
        return None;
    }
    let within: fn(char) -> bool = if is_identifier_char(ch) {
        is_identifier_char
    } else {
        |ch| !(ch.is_whitespace() || matches!(ch, '"' | '\'' | '`'))
    };
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|&(_, ch)| within(ch))
        .last()
        .map_or(offset, |(at, _)| at);
    let end = text[offset..]
        .char_indices()
        .find(|&(_, ch)| !within(ch))
        .map_or(text.len(), |(at, _)| offset + at);
    Some(start..end)
}

/// Whether `ch` can be part of an identifier a language server answers about: alphanumerics,
/// `_` and `$` (JavaScript's).
pub fn is_identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '$'
}

/// The identifier in `text` at byte `offset`: the one the character there is part of, else the
/// one ending just before it (a caret after a word's last letter); `None` when there is none.
pub fn identifier_at(text: &str, offset: usize) -> Option<Range<usize>> {
    let offset = offset.min(text.len());
    let within = |at: usize| {
        text.get(at..)
            .and_then(|rest| rest.chars().next())
            .is_some_and(is_identifier_char)
    };
    let start_at = if within(offset) {
        offset
    } else {
        let before = text.get(..offset)?.char_indices().next_back()?;
        if !is_identifier_char(before.1) {
            return None;
        }
        before.0
    };
    let start = text[..start_at]
        .char_indices()
        .rev()
        .take_while(|&(_, ch)| is_identifier_char(ch))
        .last()
        .map_or(start_at, |(at, _)| at);
    let end = text[start_at..]
        .char_indices()
        .find(|&(_, ch)| !is_identifier_char(ch))
        .map_or(text.len(), |(at, _)| start_at + at);
    // A number isn't a symbol.
    let word = &text[start..end];
    (!word.chars().next().is_some_and(|ch| ch.is_ascii_digit())).then_some(start..end)
}
