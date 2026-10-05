//! The root view (PLAN §11 `shell`, DESIGN §2): the title bar, the panel, the main area and the
//! status bar, over the window material (DESIGN §3).
//!
//! The shell keeps its window in step with the theme: whenever [`crate::theme::install`] puts a
//! new [`DvTheme`] in place it applies that theme's material and light/dark appearance to the
//! window ([`crate::platform::apply_window_material`],
//! [`crate::platform::apply_window_appearance`]) and repaints. It also reports the OS's
//! light/dark changes to [`crate::appearance`] and has it re-probe the platform when the window
//! is activated.
//!
//! **Reviews.** The shell owns the open review's [`ReviewSession`] and hands it to the panel's
//! [`FileList`] and [`CommentsList`] and to the [`DiffPane`]; opening another review swaps it
//! ([`Shell::open`], which the switcher, the source picker and automation's `open_review` all
//! go through). Opening runs
//! in the background; until it lands the main area shows that it is opening, and a failure shows
//! as a banner at the top of the main area with Retry (DESIGN §4.4). With no review open the
//! main area shows "Pick a repo to start a review"; a review with nothing to show says so.
//!
//! **Keyboard.** It rests on the main area, which always renders with the diff pane's focus
//! handle and key context inside the shell's, so `Ctrl O` and `Ctrl B` work at once and the
//! stream's keys whenever a review shows (DESIGN §5). The chrome around it takes no focus, so a
//! click on the title bar leaves the keyboard where it was.
//!
//! **Overlays.** The switcher (`Ctrl O`) and the source picker are menus that hang under their
//! title-bar buttons over the scrim (DESIGN §3); the "Finish round" sheet (`Ctrl Enter`, DESIGN
//! §4.2) is centred over it. One menu is open at a time. The sheet holds the reviewer's unsaved
//! summary, so a menu opens over it rather than in its place (the scrim then covers the sheet
//! too), and closing the menu returns to it; the sheet closes only by its own Cancel or `Esc`, a
//! click on the scrim while it is the top overlay, finishing its round, or its review going
//! away, and never while its round is being finished. Each overlay takes the keyboard while open
//! and gives it back where it was when it closes (the main area, when that place is gone).
//!
//! **Focus.** Should the keyboard ever rest on an element that is no longer drawn (a hidden
//! panel's filter, an overlay's field), gpui would dispatch keys from the window's root, above
//! the shell's key context, and every key would go nowhere; the shell's focus-lost listener
//! hands it to the nearest drawn ancestor that takes the keyboard, else the main area.
//!
//! A finished review shows "Approved · this review is finished" at the top of the main area,
//! with "Start a new review" (DESIGN §4.4).
//!
//! **Panel.** `Ctrl B` shows or hides it; dragging its right edge resizes it within 220–420,
//! saved in `settings.json` (DESIGN §2).

mod render;
mod status_bar;
pub mod title_bar;

#[cfg(test)]
mod tests;

use anyhow::{Context as _, Result};
use dv_core::review::ReviewId;
use dv_core::{Location, Target};
use futures::channel::oneshot;
use gpui::{
    App, AppContext as _, Bounds, Context, Entity, FocusHandle, Focusable, Size, Subscription,
    Task, TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, point, px, size,
};
use gpui_component::Root;

use crate::actions::shell::{FinishReview, TogglePanel, ToggleSwitcher};
use crate::comments_list::CommentsList;
use crate::diff_pane::DiffPane;
use crate::failure::Failure;
use crate::file_list::{FileList, FileListEvent};
use crate::platform;
use crate::services::ActiveServices as _;
use crate::services::reviews::OpenRequest;
use crate::session::{ReviewSession, SessionEvent};
use crate::submit::{FinishSheet, SheetEvent};
use crate::switcher::source::{SourcePicker, SourcePickerEvent};
use crate::switcher::{Switcher, SwitcherEvent};
use crate::theme::{ActiveTheme as _, Appearance, DvTheme, Material};

/// What the main area shows when there is nothing to review (DESIGN §4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmptyState {
    /// No review is open: "Pick a repo to start a review".
    NoReview,
    /// A `working` review with no changes: "Working tree clean — nothing to review".
    WorkingTreeClean,
    /// Any other review whose target has no changes.
    NoChanges,
}

impl EmptyState {
    /// `"no_review"`, `"working_tree_clean"` or `"no_changes"`, as automation state spells it.
    pub fn key(self) -> &'static str {
        match self {
            Self::NoReview => "no_review",
            Self::WorkingTreeClean => "working_tree_clean",
            Self::NoChanges => "no_changes",
        }
    }
}

/// A review being opened.
#[derive(Clone, Debug)]
pub struct Opening {
    pub location: Location,
    pub request: OpenRequest,
    generation: u64,
}

/// A failure the main area's banner shows, and how to retry it.
#[derive(Clone, Debug)]
pub struct MainBanner {
    pub failure: Failure,
    /// The open that failed, which Retry runs again.
    pub retry: Option<(Location, OpenRequest)>,
    /// Whether its detail is disclosed.
    pub expanded: bool,
}

/// What [`Shell::open`] opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    pub review: ReviewId,
    /// A new review rather than a resumed one.
    pub created: bool,
}

/// An overlay while it is open.
struct Overlay<V> {
    view: Entity<V>,
    /// Where the keyboard goes back to when it closes.
    return_focus: Option<FocusHandle>,
    _subscription: Subscription,
}

/// The root view of the main window.
pub struct Shell {
    /// The main area's, which is the diff pane's: where the keyboard rests whenever no overlay
    /// or text field has it, so the shell's keys and the stream's both work.
    focus_handle: FocusHandle,
    /// The material last applied to the window, for `state.shell`.
    applied: Material,
    /// The open review, if any.
    session: Option<Entity<ReviewSession>>,
    file_list: Entity<FileList>,
    comments_list: Entity<CommentsList>,
    diff_pane: Entity<DiffPane>,
    switcher: Option<Overlay<Switcher>>,
    source_picker: Option<Overlay<SourcePicker>>,
    /// The "Finish round" sheet (DESIGN §4.2).
    sheet: Option<Overlay<FinishSheet>>,
    panel_visible: bool,
    opening: Option<Opening>,
    /// Bumped by every open: an open that lands after a newer one started is dropped.
    open_generation: u64,
    /// Folder-picker flows still running.
    picking: usize,
    banner: Option<MainBanner>,
    /// Whether the session's error banner shows its detail.
    session_banner_expanded: bool,
    _session_subscriptions: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let diff_pane = cx.new(DiffPane::new);
        let file_list = cx.new(|cx| FileList::new(diff_pane.clone(), window, cx));
        let comments_list = cx.new(|cx| CommentsList::new(diff_pane.clone(), cx));
        let settings = cx.services().settings.clone();
        let subscriptions = vec![
            cx.observe_global_in::<DvTheme>(window, |this, window, cx| {
                this.apply_window_theme(window, cx);
                cx.notify();
            }),
            cx.observe_window_appearance(window, |_, window, cx| {
                crate::appearance::system_changed(window.appearance(), cx);
                // gpui sets the window's light/dark to the OS's right after this observer runs;
                // dv's mode may differ, so it is put back on the next frame.
                window.on_next_frame(|window, cx| {
                    platform::apply_window_appearance(window, cx.dv_theme().appearance);
                });
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    crate::appearance::refresh_support(cx);
                    if let Some(session) = &this.session {
                        session.update(cx, |session, cx| session.window_activated(cx));
                    }
                }
            }),
            // The panel's width lives in the settings.
            cx.observe(&settings, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &file_list,
                window,
                |this, _, event, window, cx| match event {
                    FileListEvent::Done => this.focus_handle.focus(window, cx),
                },
            ),
            // The keyboard never stays on an element that is no longer drawn (module docs).
            cx.on_focus_lost(window, |this, window, cx| {
                let target = window
                    .focus_lost_restore_target(cx)
                    .unwrap_or_else(|| this.focus_handle.clone());
                target.focus(window, cx);
            }),
        ];
        let (appearance, material) = {
            let theme = cx.dv_theme();
            (theme.appearance, theme.material)
        };
        // gpui opened the window light or dark as the OS is; dv's mode may say otherwise.
        if appearance != Appearance::of_window(window.appearance()) {
            platform::apply_window_appearance(window, appearance);
        }
        let focus_handle = diff_pane.read(cx).focus_handle(cx);
        // The main area always renders, inside the shell's key context, so the shell's keys
        // (`Ctrl O`) work at once and the stream's as soon as a review shows.
        focus_handle.focus(window, cx);
        Self {
            focus_handle,
            // The window opened with the theme's material ([`window_options`]).
            applied: material,
            session: None,
            file_list,
            comments_list,
            diff_pane,
            switcher: None,
            source_picker: None,
            sheet: None,
            panel_visible: true,
            opening: None,
            open_generation: 0,
            picking: 0,
            banner: None,
            session_banner_expanded: false,
            _session_subscriptions: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    /// The open review, if any.
    pub fn session(&self) -> Option<&Entity<ReviewSession>> {
        self.session.as_ref()
    }

    pub fn file_list(&self) -> &Entity<FileList> {
        &self.file_list
    }

    pub fn diff_pane(&self) -> &Entity<DiffPane> {
        &self.diff_pane
    }

    pub fn switcher(&self) -> Option<&Entity<Switcher>> {
        self.switcher.as_ref().map(|overlay| &overlay.view)
    }

    pub fn source_picker(&self) -> Option<&Entity<SourcePicker>> {
        self.source_picker.as_ref().map(|overlay| &overlay.view)
    }

    pub fn comments_list(&self) -> &Entity<CommentsList> {
        &self.comments_list
    }

    /// The "Finish round" sheet, while it is open.
    pub fn sheet(&self) -> Option<&Entity<FinishSheet>> {
        self.sheet.as_ref().map(|overlay| &overlay.view)
    }

    /// The review being opened, if any.
    pub fn opening(&self) -> Option<&Opening> {
        self.opening.as_ref()
    }

    /// The main area's banner (a review that didn't open), if any.
    pub fn banner(&self) -> Option<&MainBanner> {
        self.banner.as_ref()
    }

    /// Shows `session`'s review, replacing (and so closing) the one open before.
    pub fn open_session(&mut self, session: Entity<ReviewSession>, cx: &mut Context<Self>) {
        let location = session.read(cx).backend().repo().location.clone();
        self._session_subscriptions = vec![
            cx.observe(&session, |_, _, cx| cx.notify()),
            // A review changing under an open switcher shows in its row at once.
            cx.subscribe(&session, move |this, _, event: &SessionEvent, cx| {
                if matches!(event, SessionEvent::ReviewChanged) && this.switcher.is_some() {
                    let reviews = cx.services().reviews.clone();
                    reviews.update(cx, |reviews, cx| reviews.refresh(location.clone(), cx));
                }
            }),
        ];
        self.session = Some(session.clone());
        self.session_banner_expanded = false;
        // A sheet over the review that was open goes with it.
        self.sheet = None;
        self.file_list
            .update(cx, |list, cx| list.set_session(Some(session.clone()), cx));
        self.comments_list
            .update(cx, |list, cx| list.set_session(Some(session.clone()), cx));
        self.diff_pane
            .update(cx, |pane, cx| pane.set_session(Some(session), cx));
        cx.notify();
    }

    /// Whether the panel column shows (`Ctrl B` toggles it, DESIGN §2).
    pub fn panel_visible(&self) -> bool {
        self.panel_visible
    }

    pub fn set_panel_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.panel_visible != visible {
            self.panel_visible = visible;
            cx.notify();
        }
    }

    /// The empty state the main area shows, if any.
    pub fn empty_state(&self, cx: &App) -> Option<EmptyState> {
        let Some(session) = &self.session else {
            return self.opening.is_none().then_some(EmptyState::NoReview);
        };
        let session = session.read(cx);
        // A refresh of an empty list keeps saying so until it lands (DESIGN §4.4).
        let empty = session.spec().is_some()
            && !session.is_first_load()
            && session.error().is_none()
            && session.files().is_empty();
        empty.then(|| match session.review().target {
            Target::Working => EmptyState::WorkingTreeClean,
            _ => EmptyState::NoChanges,
        })
    }

    /// The material applied to the window.
    pub fn applied_material(&self) -> Material {
        self.applied
    }

    /// Nothing is opening or being picked, and no round is being finished: automation's
    /// `settled`.
    pub fn is_settled(&self, cx: &App) -> bool {
        self.opening.is_none()
            && self.picking == 0
            && self
                .sheet
                .as_ref()
                .is_none_or(|sheet| !sheet.view.read(cx).is_submitting())
    }

    /// Opens or resumes a review (the switcher's rows, the source picker, `open_review`) and
    /// shows it; a review already open stays as it is, with its sheet if one is open. The work
    /// runs in the background and lands even if the returned task, which only carries the
    /// result, is dropped. A failure shows as the main area's banner.
    pub fn open(
        &mut self,
        location: Location,
        request: OpenRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<Opened, Failure>> {
        // A round being finished keeps its sheet (and its session) until it lands, to show how
        // it went; opening another review meanwhile would drop both mid-write.
        if self
            .sheet
            .as_ref()
            .is_some_and(|sheet| sheet.view.read(cx).is_submitting())
        {
            return Task::ready(Err(Failure::new(
                "a round is being finished; open the review once it lands",
            )));
        }
        self.close_menu(window, cx);
        self.open_generation += 1;
        let generation = self.open_generation;
        self.opening = Some(Opening {
            location: location.clone(),
            request: request.clone(),
            generation,
        });
        self.banner = None;
        let reviews = cx.services().reviews.clone();
        let opening = reviews.update(cx, |reviews, cx| {
            reviews.open(location.clone(), request.clone(), cx)
        });
        let (sender, receiver) = oneshot::channel();
        cx.spawn_in(window, async move |this, cx| {
            let opened = opening.await;
            let result = this
                .update_in(cx, |this, window, cx| {
                    this.finish_open(generation, location, request, opened, window, cx)
                })
                .unwrap_or_else(|_| Err(Failure::new("dv is closing")));
            let _ = sender.send(result);
        })
        .detach();
        cx.notify();
        cx.background_spawn(async move {
            receiver
                .await
                .unwrap_or_else(|_| Err(Failure::new("dv is closing")))
        })
    }

    fn finish_open(
        &mut self,
        generation: u64,
        location: Location,
        request: OpenRequest,
        opened: Result<crate::services::reviews::OpenedReview, Failure>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Opened, Failure> {
        if self
            .opening
            .as_ref()
            .is_none_or(|opening| opening.generation != generation)
        {
            return Err(Failure::new("another review was opened meanwhile"));
        }
        self.opening = None;
        cx.notify();
        match opened {
            Ok(opened) => {
                let result = Opened {
                    review: opened.loaded.review.id.clone(),
                    created: opened.created,
                };
                let same = self
                    .session
                    .as_ref()
                    .is_some_and(|session| session.read(cx).review_id() == &result.review);
                if same {
                    if let Some(session) = &self.session {
                        session.update(cx, |session, cx| session.reload_fold(cx));
                    }
                } else {
                    let session = cx.new(|cx| ReviewSession::new(opened, cx));
                    self.open_session(session, cx);
                }
                // A sheet still open (the same review, resumed) keeps the keyboard.
                if self.sheet.is_none() {
                    self.focus_handle.focus(window, cx);
                }
                Ok(result)
            }
            Err(failure) => {
                self.banner = Some(MainBanner {
                    failure: failure.clone(),
                    retry: Some((location, request)),
                    expanded: false,
                });
                Err(failure)
            }
        }
    }

    /// Opens the switcher, its selection on `focus`'s first row (`dv open <path>`) or on the
    /// open review. Every known repository's row is recomputed meanwhile. It replaces an open
    /// menu and opens over an open sheet.
    pub fn open_switcher(
        &mut self,
        focus: Option<&Location>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let return_focus = self.take_menu().unwrap_or_else(|| window.focused(cx));
        let reviews = cx.services().reviews.clone();
        reviews.update(cx, |reviews, cx| reviews.refresh_all(cx));
        let current = self
            .session
            .as_ref()
            .map(|session| session.read(cx).review_id().clone());
        let view = cx.new(|cx| Switcher::new(reviews, focus, current.as_ref(), window, cx));
        let subscription = cx.subscribe_in(&view, window, Self::on_switcher_event);
        view.update(cx, |switcher, cx| switcher.focus(window, cx));
        self.switcher = Some(Overlay {
            view,
            return_focus,
            _subscription: subscription,
        });
        cx.notify();
    }

    /// Opens the source picker over the open review's repository. Like the switcher, it
    /// replaces an open menu and opens over an open sheet.
    pub fn open_source_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        let (location, target) = {
            let session = session.read(cx);
            (
                session.backend().repo().location.clone(),
                session.review().target.clone(),
            )
        };
        let return_focus = self.take_menu().unwrap_or_else(|| window.focused(cx));
        let reviews = cx.services().reviews.clone();
        reviews.update(cx, |reviews, cx| reviews.refresh(location.clone(), cx));
        let view = cx.new(|cx| SourcePicker::new(reviews, location, target, cx));
        let subscription = cx.subscribe_in(&view, window, Self::on_source_picker_event);
        let handle = view.read(cx).focus_handle(cx);
        handle.focus(window, cx);
        self.source_picker = Some(Overlay {
            view,
            return_focus,
            _subscription: subscription,
        });
        cx.notify();
    }

    /// Opens the "Finish round" sheet over the open review (DESIGN §4.2), in place of an open
    /// menu; nothing happens with no review open, a finished one, or a sheet already open.
    pub fn open_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        if session.read(cx).is_read_only() || self.sheet.is_some() {
            return;
        }
        let return_focus = self.take_menu().unwrap_or_else(|| window.focused(cx));
        let view = cx.new(|cx| FinishSheet::new(session, window, cx));
        let subscription = cx.subscribe_in(&view, window, Self::on_sheet_event);
        view.update(cx, |sheet, cx| sheet.focus(window, cx));
        self.sheet = Some(Overlay {
            view,
            return_focus,
            _subscription: subscription,
        });
        cx.notify();
    }

    /// "Start a new review" on a finished one: a fresh review of the same target (PLAN §2.4:
    /// the finished one no longer counts as open).
    pub fn start_new_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        let (location, target) = {
            let session = session.read(cx);
            (
                session.backend().repo().location.clone(),
                session.review().target.clone(),
            )
        };
        self.open(location, OpenRequest::Target(target), window, cx)
            .detach();
    }

    fn on_sheet_event(
        &mut self,
        _: &Entity<FinishSheet>,
        event: &SheetEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SheetEvent::Dismiss => {
                self.close_sheet(window, cx);
            }
            SheetEvent::Finished(_) => {
                self.sheet = None;
                // The keyboard returns to the stream, which shows the new round: now, or when
                // a menu open over the sheet closes.
                let menu_focus = match (&mut self.switcher, &mut self.source_picker) {
                    (Some(menu), _) => Some(&mut menu.return_focus),
                    (None, Some(menu)) => Some(&mut menu.return_focus),
                    (None, None) => None,
                };
                match menu_focus {
                    Some(return_focus) => *return_focus = None,
                    None => self.focus_handle.focus(window, cx),
                }
                cx.notify();
            }
        }
    }

    fn finish_review(&mut self, _: &FinishReview, window: &mut Window, cx: &mut Context<Self>) {
        self.open_sheet(window, cx);
    }

    /// Takes the open menu (the switcher or the source picker) away without moving the
    /// keyboard: `Some` with where the keyboard was before it opened, `None` when no menu was
    /// open.
    fn take_menu(&mut self) -> Option<Option<FocusHandle>> {
        match (self.switcher.take(), self.source_picker.take()) {
            (Some(menu), _) => Some(menu.return_focus),
            (None, Some(menu)) => Some(menu.return_focus),
            (None, None) => None,
        }
    }

    /// Closes the open menu, the keyboard going back to where it was before the menu opened;
    /// `false` when no menu was open.
    pub fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(return_focus) = self.take_menu() else {
            return false;
        };
        self.restore_focus(return_focus, window, cx);
        cx.notify();
        true
    }

    /// Closes the sheet, the keyboard going back to where it was before it opened, unless its
    /// round is being finished: it then stays until the round lands, to show how it went.
    /// `false` when it didn't close.
    pub fn close_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(sheet) = self
            .sheet
            .take_if(|sheet| !sheet.view.read(cx).is_submitting())
        else {
            return false;
        };
        self.restore_focus(sheet.return_focus, window, cx);
        cx.notify();
        true
    }

    /// Closes the top overlay (`Esc`, a click on the scrim): the open menu, else the sheet
    /// ([`Shell::close_sheet`]). `false` when nothing closed.
    pub fn close_top_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.close_menu(window, cx) || self.close_sheet(window, cx)
    }

    /// Gives the keyboard back to `handle`, or to the main area when there is none or it went
    /// away meanwhile (the file list's filter while the panel is hidden).
    fn restore_focus(&self, handle: Option<FocusHandle>, window: &mut Window, cx: &mut App) {
        let filter = self.file_list.read(cx).filter_focus(cx);
        match handle {
            Some(handle) if self.panel_visible || handle != filter => handle.focus(window, cx),
            _ => self.focus_handle.focus(window, cx),
        }
    }

    /// "Open folder…": the folder picker (through the system seam), then the folder's
    /// repository added and selected in the switcher. A folder that isn't a repository shows
    /// why in the main area's banner.
    pub fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picking += 1;
        let picked = crate::system::pick_folder("Open repository", cx);
        cx.spawn_in(window, async move |this, cx| {
            let added = match picked.await {
                Ok(Some(path)) => {
                    // A folder picked through `\\wsl.localhost\<distro>\…` is a WSL location.
                    let text = path.to_string_lossy().into_owned();
                    let location = Location::parse(&text).unwrap_or(Location::Local(path));
                    let adding = this.update(cx, |_, cx| {
                        let reviews = cx.services().reviews.clone();
                        reviews.update(cx, |reviews, cx| reviews.add_repo(location, cx))
                    });
                    match adding {
                        Ok(adding) => Some(adding.await),
                        Err(_) => None,
                    }
                }
                Ok(None) => None,
                Err(error) => Some(Err(Failure::new(format!(
                    "the folder picker failed: {error}"
                )))),
            };
            this.update_in(cx, |this, window, cx| {
                this.picking -= 1;
                match added {
                    Some(Ok(location)) => this.open_switcher(Some(&location), window, cx),
                    Some(Err(failure)) => {
                        this.banner = Some(MainBanner {
                            failure,
                            retry: None,
                            expanded: false,
                        });
                    }
                    None => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens the review the banner's Retry names again.
    pub fn retry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((location, request)) = self.banner.as_ref().and_then(|b| b.retry.clone()) {
            self.open(location, request, window, cx).detach();
        }
    }

    fn on_switcher_event(
        &mut self,
        _: &Entity<Switcher>,
        event: &SwitcherEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SwitcherEvent::Open { location, request } => {
                self.open(location.clone(), request.clone(), window, cx)
                    .detach();
            }
            SwitcherEvent::OpenFolder => self.pick_folder(window, cx),
            SwitcherEvent::Dismiss => {
                self.close_menu(window, cx);
            }
        }
    }

    fn on_source_picker_event(
        &mut self,
        _: &Entity<SourcePicker>,
        event: &SourcePickerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SourcePickerEvent::Open { location, target } => {
                self.open(
                    location.clone(),
                    OpenRequest::Target(target.clone()),
                    window,
                    cx,
                )
                .detach();
            }
            SourcePickerEvent::Dismiss => {
                self.close_menu(window, cx);
            }
        }
    }

    fn toggle_switcher(&mut self, _: &ToggleSwitcher, window: &mut Window, cx: &mut Context<Self>) {
        if self.switcher.is_some() {
            self.close_menu(window, cx);
        } else {
            self.open_switcher(None, window, cx);
        }
    }

    fn toggle_panel(&mut self, _: &TogglePanel, window: &mut Window, cx: &mut Context<Self>) {
        // The keyboard leaves a filter field that is about to disappear.
        if self.panel_visible && self.file_list.read(cx).is_filter_focused(window, cx) {
            self.focus_handle.focus(window, cx);
        }
        self.set_panel_visible(!self.panel_visible, cx);
    }

    fn apply_window_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let theme = cx.dv_theme();
        platform::apply_window_appearance(window, theme.appearance);
        if theme.material != self.applied {
            platform::apply_window_material(window, theme.material);
            self.applied = theme.material;
        }
    }
}

impl Focusable for Shell {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// Options for the main window: centred at `window_size` (logical px), DESIGN's minimum size, a
/// custom title bar, and the installed theme's material.
pub fn window_options(window_size: Size<gpui::Pixels>, cx: &App) -> WindowOptions {
    let theme = cx.dv_theme();
    let min_size = size(theme.sizes.window_min_width, theme.sizes.window_min_height);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            window_size,
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("dv".into()),
            // Hides the system title bar; the shell draws its own (`title_bar`).
            appears_transparent: true,
            // macOS: centred on the 40px title bar.
            traffic_light_position: Some(point(px(12.), px(12.))),
        }),
        window_min_size: Some(min_size),
        window_background: theme.material.window_background(),
        app_id: Some("dv".into()),
        ..Default::default()
    }
}

/// The main window and its root view.
pub struct MainWindow {
    pub handle: WindowHandle<Root>,
    pub shell: Entity<Shell>,
}

/// Opens the main window, gpui-component's `Root` around a [`Shell`], through
/// [`crate::automation::open_window`] like every dv window. Call after
/// [`crate::appearance::init`], so the window opens in the resolved theme and material.
pub fn open_main_window(window_size: Size<gpui::Pixels>, cx: &mut App) -> Result<MainWindow> {
    let options = window_options(window_size, cx);
    let mut shell = None;
    let handle = crate::automation::open_window(cx, options, |window, cx| {
        let view = cx.new(|cx| Shell::new(window, cx));
        shell = Some(view.clone());
        cx.new(|cx| Root::new(view, window, cx))
    })
    .context("opening the main window")?;
    let shell = shell.context("the main window built no shell")?;
    Ok(MainWindow { handle, shell })
}
