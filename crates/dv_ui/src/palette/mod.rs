//! The palette (`Ctrl K`, or a click on the title bar's command field; PLAN §11 `palette`,
//! DESIGN §4.1 "Palette"): one filter field over commands, reviews and files, in an overlay hung
//! under the command field over the scrim, in the file finder's look.
//!
//! **Rows**, in sections and in this order ([`build`]):
//! - **Commands** ([`commands::COMMANDS`]): the shell's and the stream's actions by name, with
//!   their keycaps; the stream's only while a review is open.
//! - **Reviews**: every known repository's targets and open reviews as the switcher lists them
//!   (`crate::switcher::items::build`, the switcher's own filter: repository, label, status).
//! - **Files**: with a review open and something typed, the file finder's first
//!   [`FILE_ROWS`] matches (`crate::file_finder::filter`); the rest are `Ctrl P`'s.
//!
//! A command passes when every word typed is in its title or its other words, ignoring case.
//! An empty filter lists the commands and the reviews. The first row is selected; `↑`/`↓` move
//! the selection (wrapping) across the sections, `Enter` picks, `Esc` closes. The palette only
//! reports what was picked ([`PaletteEvent`]); the shell runs it. Rows refresh as the reviews
//! service recomputes them and as the review's side paths land, keeping the selected row.
//!
//! Automation targets: `palette`, `palette/input`, `palette/empty`, `palette/row/<key>` for
//! each row ([`Row::key`]).

pub mod commands;
mod render;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use dv_core::Location;
use dv_core::diff::FileStatus;
use gpui::{
    AppContext as _, Context, Entity, EventEmitter, Focusable as _, ScrollHandle, Subscription,
    Window,
};
use gpui_component::input::{InputEvent, InputState};

use crate::actions::palette::{Confirm, Dismiss, SelectNext, SelectPrevious};
use crate::file_finder::Candidate;
use crate::services::reviews::{OpenRequest, Reviews};
use crate::session::ReviewSession;
use crate::switcher::items::{self, ItemKind, ReviewBadge, TargetKind};
use commands::{COMMANDS, Command};

/// The panel's width (the switcher's and the file finder's).
pub const PALETTE_WIDTH: gpui::Pixels = gpui::px(560.);
/// The filter field's placeholder: the command field's words.
pub const PLACEHOLDER: &str = "Go to file, review or command";
/// How many files the palette lists at most.
pub const FILE_ROWS: usize = 5;
/// How many rows `state.palette` reports.
pub const STATE_ROWS: usize = 40;

/// The palette's sections, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Commands,
    Reviews,
    Files,
}

impl Section {
    /// Its label over its rows.
    pub fn title(self) -> &'static str {
        match self {
            Self::Commands => "Commands",
            Self::Reviews => "Reviews",
            Self::Files => "Files",
        }
    }

    /// `"commands"`, `"reviews"` or `"files"`, for keys and automation state.
    pub fn key(self) -> &'static str {
        match self {
            Self::Commands => "commands",
            Self::Reviews => "reviews",
            Self::Files => "files",
        }
    }
}

/// What a row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKind {
    Command(Command),
    /// Open one of a repository's targets, or resume an open review.
    Review {
        /// The repository's name.
        repo: String,
        /// "Uncommitted changes", "Branch feat vs main", a review's label.
        label: String,
        /// The target's kind; `None` for an open review of another target.
        target: Option<TargetKind>,
        /// Its open review, if any.
        badge: Option<ReviewBadge>,
        location: Location,
        request: OpenRequest,
    },
    /// Open this file of the review in the file view.
    File {
        path: String,
        status: Option<FileStatus>,
    },
}

/// One pickable row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// A stable name: `command/<action>` (`command/diff_pane::NextFile`), `review/<the
    /// switcher's key>` (`review/api-server/working`) or `file/<path>`.
    pub key: String,
    pub section: Section,
    pub kind: RowKind,
}

impl Row {
    /// What the row reads.
    pub fn label(&self) -> String {
        match &self.kind {
            RowKind::Command(command) => command.title.to_string(),
            RowKind::Review { repo, label, .. } => format!("{repo} · {label}"),
            RowKind::File { path, .. } => path.clone(),
        }
    }
}

/// The rows for `query` (module docs, "Rows"): `commands` filtered by it (those needing a review
/// left out unless `review_open`), the reviews `listing` lists for it, and the first of `files`
/// passing it (only with a review open and `query` non-empty).
pub fn build(
    query: &str,
    review_open: bool,
    listing: &items::Listing,
    files: &[Candidate],
) -> Vec<Row> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut rows: Vec<Row> = COMMANDS
        .iter()
        .filter(|command| review_open || !command.needs_review)
        .filter(|command| {
            let text = command.text();
            words.iter().all(|word| text.contains(word.as_str()))
        })
        .map(|command| Row {
            key: format!("command/{}", command.name()),
            section: Section::Commands,
            kind: RowKind::Command(*command),
        })
        .collect();
    for (ix, item) in listing.items.iter().enumerate() {
        let target = match &item.kind {
            ItemKind::Target(offer) => Some(offer.kind),
            ItemKind::Review { .. } => None,
            // The branch picker is the switcher's: the palette lists the reviews it opens.
            ItemKind::Start { .. }
            | ItemKind::Retry { .. }
            | ItemKind::OpenFolder
            | ItemKind::CompareBranches
            | ItemKind::Branch { .. }
            | ItemKind::Base { .. } => continue,
        };
        let (Some((location, request)), Some(repo)) = (
            listing.request(ix),
            item.repo.and_then(|repo| listing.repos.get(repo)),
        ) else {
            continue;
        };
        rows.push(Row {
            key: format!("review/{}", item.key),
            section: Section::Reviews,
            kind: RowKind::Review {
                repo: repo.name.clone(),
                label: item.label().to_string(),
                target,
                badge: item.review().cloned(),
                location,
                request,
            },
        });
    }
    if review_open && !words.is_empty() {
        rows.extend(
            crate::file_finder::filter(files, query)
                .into_iter()
                .take(FILE_ROWS)
                .map(|index| {
                    let file = &files[index];
                    Row {
                        key: format!("file/{}", file.path),
                        section: Section::Files,
                        kind: RowKind::File {
                            path: file.path.clone(),
                            status: file.status,
                        },
                    }
                }),
        );
    }
    rows
}

/// What was picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteEvent {
    /// Run this command.
    Run(Command),
    /// Open or resume this review.
    Open {
        location: Location,
        request: OpenRequest,
    },
    /// Show this file of the open review in the file view.
    OpenFile(String),
    /// Closed without picking (`Esc`).
    Dismiss,
}

/// The palette (see the module docs).
pub struct Palette {
    reviews: Entity<Reviews>,
    session: Option<Entity<ReviewSession>>,
    input: Entity<InputState>,
    query: String,
    /// The open review's files ([`crate::file_finder::candidates`]), and whether the side's
    /// paths were in them.
    files: Arc<Vec<Candidate>>,
    files_complete: bool,
    rows: Vec<Row>,
    /// Into `rows`.
    selected: usize,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PaletteEvent> for Palette {}

impl Palette {
    /// A palette over the known repositories' reviews and, with `session`, the open review's
    /// commands and files (its side's paths read if they aren't).
    pub fn new(
        reviews: Entity<Reviews>,
        session: Option<Entity<ReviewSession>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(PLACEHOLDER));
        let mut subscriptions = vec![
            cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.query_changed(cx);
                }
            }),
            cx.observe(&reviews, |this, _, cx| this.rebuild(cx)),
        ];
        if let Some(session) = &session {
            session.update(cx, |session, cx| session.ensure_side_paths(cx));
            // The side's paths land.
            subscriptions.push(cx.observe(session, |this, _, cx| {
                if !this.files_complete {
                    this.read_files(cx);
                    this.rebuild(cx);
                }
            }));
        }
        let mut this = Self {
            reviews,
            session,
            input,
            query: String::new(),
            files: Arc::new(Vec::new()),
            files_complete: false,
            rows: Vec::new(),
            selected: 0,
            scroll: ScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        this.read_files(cx);
        this.rows = this.build(cx);
        this
    }

    /// Puts the keyboard in the filter field.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Whether the filter field has the keyboard.
    pub fn is_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.input.read(cx).focus_handle(cx).is_focused(window)
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The selected row's key.
    pub fn selected_key(&self) -> Option<&str> {
        self.rows.get(self.selected).map(|row| row.key.as_str())
    }

    /// The open review's files, once its side's paths have landed (the review's own files
    /// meanwhile).
    fn read_files(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        let session = session.read(cx);
        let side = session.side_paths().cloned();
        let changed: Vec<(String, FileStatus)> = session
            .target_files()
            .iter()
            .map(|entry| (entry.path.clone(), entry.status))
            .collect();
        self.files_complete = side.is_some();
        self.files = Arc::new(crate::file_finder::candidates(
            side.as_deref().map_or(&[], Vec::as_slice),
            &changed,
        ));
    }

    fn build(&self, cx: &gpui::App) -> Vec<Row> {
        let listing = items::build(&self.reviews.read(cx).rows(), &self.query);
        build(&self.query, self.session.is_some(), &listing, &self.files)
    }

    /// Lists the rows again (the reviews or the files changed), keeping the selected row when
    /// it is still there.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let rows = self.build(cx);
        if rows == self.rows {
            return;
        }
        let keep = self.selected_key().map(str::to_string);
        self.rows = rows;
        self.selected = keep
            .and_then(|key| self.rows.iter().position(|row| row.key == key))
            .unwrap_or(0);
        self.scroll_to_selected();
        cx.notify();
    }

    fn query_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().to_string();
        if query == self.query {
            return;
        }
        self.query = query;
        self.rows = self.build(cx);
        self.selected = 0;
        self.scroll_to_selected();
        cx.notify();
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = ix.min(self.rows.len().saturating_sub(1));
        self.scroll_to_selected();
        cx.notify();
    }

    /// The list's line for row `ix`: each section's label is a line before its first row.
    fn line_of(&self, ix: usize) -> usize {
        let labels = self.rows[..=ix.min(self.rows.len().saturating_sub(1))]
            .iter()
            .enumerate()
            .filter(|(at, row)| *at == 0 || self.rows[at - 1].section != row.section)
            .count();
        ix + labels
    }

    fn scroll_to_selected(&self) {
        if !self.rows.is_empty() {
            self.scroll.scroll_to_item(self.line_of(self.selected));
        }
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.rows.len();
        if len > 0 {
            self.select((self.selected + 1) % len, cx);
        }
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.rows.len();
        if len > 0 {
            self.select((self.selected + len - 1) % len, cx);
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        self.pick(self.selected, cx);
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(PaletteEvent::Dismiss);
    }

    /// Picks row `ix` (Enter on it, or a click).
    fn pick(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else {
            return;
        };
        self.selected = ix;
        let event = match &row.kind {
            RowKind::Command(command) => PaletteEvent::Run(*command),
            RowKind::Review {
                location, request, ..
            } => PaletteEvent::Open {
                location: location.clone(),
                request: request.clone(),
            },
            RowKind::File { path, .. } => PaletteEvent::OpenFile(path.clone()),
        };
        cx.emit(event);
    }
}
