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
use dv_lsp::{HoverInfo, LspClient, RequestError, SymbolLocation, TextPosition, Timeouts};
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

/// A symbol the reviewer points at: an identifier in a line of a file as a pane shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolAt {
    pub path: String,
    pub side: AnchorSide,
    /// Its line's number on `side`.
    pub line: u32,
    /// The identifier's bytes in the line's text.
    pub range: Range<usize>,
    /// The identifier.
    pub name: String,
}

impl SymbolAt {
    /// Where a server is asked about it: its line (0-based) and its first byte.
    fn position(&self) -> TextPosition {
        TextPosition::new(self.line.saturating_sub(1), self.range.start as u32)
    }
}

/// An open hover card (PLAN §18.1 "Hover").
#[derive(Clone, Debug)]
pub struct HoverCard {
    pub at: SymbolAt,
    pub pane: Pane,
    /// The token's bounds in the window when it was asked for; the card hangs under it.
    pub token: Bounds<Pixels>,
    pub info: HoverInfo,
    /// The server read the file's text in place of the disk's, and the rest of the project
    /// from the working tree (PLAN §18.2).
    pub against_worktree: bool,
    /// Tells this answer's Markdown views from another's.
    pub id: u64,
}

impl HoverCard {
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

/// The text a pane shows of one file at one side, as a server reads it (module docs).
#[derive(Clone)]
struct Document {
    /// The diff it was cut from, by identity.
    diff: Arc<FileDiff>,
    text: Arc<str>,
}

/// A review's code intelligence (module docs).
pub struct CodeIntel {
    session: Entity<ReviewSession>,
    servers: Entity<LanguageServers>,
    worktree: Location,
    /// The servers for the review's files were asked for (once its list landed).
    served: bool,
    /// The hover being asked for, and the card shown.
    hovering: Option<(SymbolAt, Pane, InFlight)>,
    hover: Option<HoverCard>,
    next_hover: u64,
    definition: Option<InFlight>,
    /// How many places the last go to definition found (`state.intel.definitions`).
    last_definitions: Option<usize>,
    references: Option<References>,
    references_request: Option<InFlight>,
    /// Why the last navigation couldn't show its place (a definition outside the repository).
    notice: Option<Failure>,
    documents: HashMap<(String, AnchorSide), Document>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<IntelEvent> for CodeIntel {}

impl CodeIntel {
    /// The code intelligence of `session`'s review: its worktree's servers are in use until
    /// this is dropped.
    pub fn new(session: Entity<ReviewSession>, cx: &mut Context<Self>) -> Self {
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
            next_hover: 0,
            definition: None,
            last_definitions: None,
            references: None,
            references_request: None,
            notice: None,
            documents: HashMap::new(),
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
        self.hovering.as_ref().map(|(at, pane, _)| (at, *pane))
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
            && self.definition.is_none()
            && self.references_request.is_none()
            && self
                .references
                .as_ref()
                .is_none_or(|references| !references.is_loading())
            && self.servers.read(cx).is_settled()
    }

    fn on_session_event(&mut self, event: &SessionEvent, cx: &mut Context<Self>) {
        match event {
            SessionEvent::FilesChanged => self.serve(cx),
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

    /// Whether a server reads `side`'s text as the disk holds it: the new side of a review
    /// whose new side is the working tree (module docs).
    pub fn reads_worktree(&self, side: AnchorSide, cx: &App) -> bool {
        side == AnchorSide::New && self.session.read(cx).new_side() == Some(&Side::Worktree)
    }

    /// `at`'s file's text on its side, from `diff` (the diff the pane shows the file from),
    /// built once per diff.
    fn document(&mut self, at: &SymbolAt, diff: &Arc<FileDiff>) -> Option<Arc<str>> {
        let key = (at.path.clone(), at.side);
        if let Some(known) = self.documents.get(&key)
            && Arc::ptr_eq(&known.diff, diff)
        {
            return Some(known.text.clone());
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
            },
        );
        Some(text)
    }

    /// Asks for `at`'s hover (the pointer rested on it, or `Ctrl I`), shown in `pane` under
    /// `token`; the hover asked for before is abandoned. The same symbol again does nothing.
    pub fn hover(
        &mut self,
        at: SymbolAt,
        pane: Pane,
        token: Bounds<Pixels>,
        diff: &Arc<FileDiff>,
        cx: &mut Context<Self>,
    ) {
        if self
            .hover
            .as_ref()
            .is_some_and(|card| card.at == at && card.pane == pane)
            || self
                .hovering
                .as_ref()
                .is_some_and(|(asked, on, _)| *asked == at && *on == pane)
        {
            return;
        }
        self.hovering = None;
        if self.hover.take().is_some() {
            cx.notify();
        }
        let Some(text) = self.document(&at, diff) else {
            return;
        };
        let against_worktree = !self.reads_worktree(at.side, cx);
        let asked = at.clone();
        let request = self.request(
            &at,
            text,
            against_worktree,
            Timeouts::default().hover,
            |client, path, position| client.hover(path, position),
            move |this, answer, cx| {
                if !this
                    .hovering
                    .as_ref()
                    .is_some_and(|(at, on, _)| *at == asked && *on == pane)
                {
                    return;
                }
                this.hovering = None;
                match answer {
                    Some(Ok(Some(info))) => {
                        this.next_hover += 1;
                        this.hover = Some(HoverCard {
                            at: asked,
                            pane,
                            token,
                            info,
                            against_worktree,
                            id: this.next_hover,
                        });
                    }
                    // Nothing to show: no answer, no server, or a request that failed
                    // quietly (PLAN §18.5).
                    Some(Err(error)) => log::info!("hover: {error}"),
                    _ => {}
                }
                cx.notify();
            },
            cx,
        );
        self.hovering = Some((at, pane, request));
        cx.notify();
    }

    /// Closes the hover card, leaving a hover asked for since (another token's) alone.
    pub fn close_card(&mut self, cx: &mut Context<Self>) {
        if self.hover.take().is_some() {
            cx.notify();
        }
    }

    /// Closes the hover card, abandoning a hover still asked for.
    pub fn close_hover(&mut self, cx: &mut Context<Self>) {
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
        let against_worktree = !self.reads_worktree(at.side, cx);
        let from = at.clone();
        let request = self.request(
            &at,
            text,
            against_worktree,
            Timeouts::default().definition,
            |client, path, position| client.definition(path, position),
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
                            against_worktree,
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
        let against_worktree = !self.reads_worktree(at.side, cx);
        self.references = Some(References::loading(
            ReferencesKind::References,
            at.clone(),
            pane,
            against_worktree,
            self.references.as_ref().is_some_and(|r| r.in_review),
        ));
        cx.emit(IntelEvent::ShowReferences);
        let from = at.clone();
        let request = self.request(
            &at,
            text,
            against_worktree,
            Timeouts::default().references,
            |client, path, position| client.references(path, position, true),
            move |this, answer, cx| {
                this.references_request = None;
                let found = match answer {
                    Some(Ok(locations)) => Ok(locations),
                    Some(Err(error)) => Err(error.to_string()),
                    None => Err("no language server serves this file".to_string()),
                };
                this.show_list(
                    ReferencesKind::References,
                    from,
                    pane,
                    against_worktree,
                    found,
                    cx,
                );
            },
            cx,
        );
        self.references_request = Some(request);
        cx.notify();
    }

    /// The References tab's list: `found`'s places, their lines read in the background.
    fn show_list(
        &mut self,
        kind: ReferencesKind,
        from: SymbolAt,
        pane: Pane,
        against_worktree: bool,
        found: Result<Vec<SymbolLocation>, String>,
        cx: &mut Context<Self>,
    ) {
        let in_review = self.references.as_ref().is_some_and(|r| r.in_review);
        let mut list = References::loading(kind, from, pane, against_worktree, in_review);
        match found {
            Ok(locations) => {
                let backend = self.session.read(cx).backend().clone();
                let job = cx.background_spawn(async move {
                    references::read_rows(backend.as_ref(), locations)
                });
                list.reading = Some(cx.spawn(async move |this, cx| {
                    let files = job.await;
                    this.update(cx, |this, cx| {
                        if let Some(list) = this.references.as_mut() {
                            list.reading = None;
                            list.files = Some(files);
                        }
                        this.load_referenced(cx);
                        cx.notify();
                    })
                    .ok();
                }));
            }
            Err(error) => list.error = Some(error),
        }
        self.references = Some(list);
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

    /// One request about `at`, whose file's text is `text` (module docs): waits for its
    /// file's server up to `timeout`, then asks `ask` on a background thread and hands the
    /// answer (`None` without a server) to `done` on the UI thread. A document that isn't the
    /// working tree's (`close_after`) is closed once answered, so the server reads the file from
    /// the working tree again for every other request (PLAN §18.2).
    #[allow(clippy::too_many_arguments)]
    fn request<T: Send + 'static>(
        &mut self,
        at: &SymbolAt,
        text: Arc<str>,
        close_after: bool,
        timeout: Duration,
        ask: fn(&LspClient, &str, TextPosition) -> dv_lsp::Pending<T>,
        done: impl FnOnce(&mut Self, Option<Result<T, RequestError>>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> InFlight {
        let path = at.path.clone();
        let position = at.position();
        let servers = self.servers.clone();
        let worktree = self.worktree.clone();
        InFlight::new(move |arming: Arming| {
            cx.spawn(async move |this, cx| {
                let deadline = Instant::now() + timeout;
                let found = loop {
                    let lookup = this
                        .update(cx, |_, cx| {
                            servers.update(cx, |servers, cx| servers.lookup(&worktree, &path, cx))
                        })
                        .unwrap_or(Lookup::Unavailable);
                    match lookup {
                        Lookup::Ready {
                            client,
                            language_id,
                            ..
                        } => break Some((client, language_id)),
                        Lookup::Unavailable => break None,
                        Lookup::Pending if Instant::now() >= deadline => break None,
                        Lookup::Pending => cx.background_executor().timer(SERVER_POLL).await,
                    }
                };
                let answer = match found {
                    None => None,
                    Some((client, language_id)) => {
                        let job = cx.background_spawn(async move {
                            if arming.is_abandoned() {
                                return None;
                            }
                            if let Err(error) = client.open_document(&path, language_id, text) {
                                return Some(Err(error));
                            }
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
                        });
                        job.await
                    }
                };
                this.update(cx, |this, cx| done(this, answer, cx)).ok();
            })
        })
    }
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
