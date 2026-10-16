//! The stream's and the file view's side of code intelligence (PLAN §18.1, `crate::intel`):
//! which symbol the pointer rests on or the caret is at, and the hover card under it.
//!
//! **Hover.** The pointer resting [`HOVER_DELAY`] on an identifier in a line's code (found
//! through the lines' painted geometry, `select`) asks the review's [`CodeIntel`] for its hover;
//! `Ctrl I` asks for the symbol at the caret at once. The card stays while the pointer is over
//! its token or over the card itself, and closes [`CLOSE_GRACE`] after it left both (time to
//! cross from one to the other), at once on a press elsewhere in the code, on a scroll, a
//! drag, on `Esc` (first in the pane's chain) and when its file's diff is replaced. A hover
//! asked for and not answered yet is cancelled as soon as the pointer leaves its token. The
//! grace only ever closes the card: a wait for the token the pointer moved onto goes on. Text
//! selected in the card takes the keyboard; once the card goes, the keyboard goes back where it
//! was before the press in the card.
//!
//! **Definition and references.** `Ctrl`+click on an identifier, or `F12` at the caret, asks
//! for its definition; `Shift F12` at the caret, or the card's buttons, for its references. The
//! shell shows what they find (`crate::intel::IntelEvent`).

use std::sync::Arc;
use std::time::Duration;

use dv_core::diff::FileDiff;
use dv_core::review::AnchorSide;
use gpui::{
    AnyElement, Bounds, Context, Entity, IntoElement as _, MouseDownEvent, ParentElement as _,
    Pixels, Point, anchored, deferred, point, px,
};

use super::DiffPane;
use super::rows::side_number;
use super::select::line_index;
use super::text::unit_at;
use crate::intel::hover::{CardActions, render_card};
use crate::intel::{CodeIntel, HOVER_DELAY, Pane, SymbolAt, identifier_at};
use crate::theme::DvTheme;

/// The card paints over the rows and the pinned header, as the file menu does.
const CARD_PRIORITY: usize = 2;

/// How long a card stays after the pointer left its token and the card.
pub const CLOSE_GRACE: Duration = Duration::from_millis(300);

/// A symbol under the pointer or at the caret: where it is, its token's bounds in the window
/// and the diff its file is shown from.
pub(super) struct Pointed {
    at: SymbolAt,
    token: Bounds<Pixels>,
    diff: Arc<FileDiff>,
}

/// The pane's hover state (module docs).
#[derive(Default)]
pub(super) struct HoverUi {
    /// The symbol the pointer rests on, and the wait before it is asked about.
    waiting: Option<(SymbolAt, gpui::Task<()>)>,
    /// Closes the card once the grace after the pointer left it is over.
    closing: Option<gpui::Task<()>>,
    /// The pointer is over the card.
    over_card: bool,
    /// The card this pane last saw (`CodeIntel`'s id for it): a new one, or none, starts the
    /// pointer's state over (gpui tells no hover-out to an element it no longer paints).
    card: Option<u64>,
    /// Where the keyboard was before a press in the card, and where it went after it (the
    /// card's text), to give it back once the card goes (module docs).
    return_focus: Option<gpui::FocusHandle>,
    card_focus: Option<gpui::FocusHandle>,
    /// Files whose diff was replaced since the last frame: a card on one goes.
    replaced: Vec<String>,
}

impl HoverUi {
    /// Whether a hover is waiting for the pointer to rest (automation's `settled`).
    pub fn is_waiting(&self) -> bool {
        self.waiting.is_some() || self.closing.is_some()
    }

    /// Whether the pointer is over the card.
    pub fn is_over_card(&self) -> bool {
        self.over_card
    }

    /// `path`'s diff was replaced (`select::forget_text_in`).
    pub fn file_replaced(&mut self, path: &str) {
        self.replaced.push(path.to_string());
    }
}

impl DiffPane {
    /// Uses `intel` (the open review's) for hover, definition and references; `None` with no
    /// review open.
    pub fn set_intel(&mut self, intel: Option<Entity<CodeIntel>>, cx: &mut Context<Self>) {
        self._intel_events = intel
            .as_ref()
            .map(|intel| cx.observe(intel, |this, _, cx| this.intel_changed(cx)));
        self.intel = intel;
        self.hover_ui = HoverUi::default();
        cx.notify();
    }

    /// The review's code intelligence changed: a card that went, or another one, starts the
    /// pointer's state over (module docs).
    fn intel_changed(&mut self, cx: &mut Context<Self>) {
        let card = self.shown_card(cx).map(|card| card.id);
        if card != self.hover_ui.card {
            self.hover_ui.card = card;
            self.hover_ui.over_card = false;
            if card.is_none() {
                self.hover_ui.closing = None;
            }
        }
        cx.notify();
    }

    /// The card this pane shows, if any.
    fn shown_card<'a>(&self, cx: &'a gpui::App) -> Option<&'a crate::intel::HoverCard> {
        let pane = self.intel_pane();
        self.intel
            .as_ref()?
            .read(cx)
            .hover_card()
            .filter(|card| card.pane == pane)
    }

    /// The symbol whose hover this pane asked for and that hasn't answered, if any.
    fn asking(&self, cx: &gpui::App) -> Option<SymbolAt> {
        let pane = self.intel_pane();
        let intel = self.intel.as_ref()?.read(cx);
        intel
            .hovering()
            .filter(|(_, on)| *on == pane)
            .map(|(at, _)| at.clone())
    }

    /// Cancels this pane's hover still asked for (the pointer left its token).
    fn cancel_asking(&mut self, cx: &mut Context<Self>) {
        if self.asking(cx).is_some()
            && self.shown_card(cx).is_none()
            && let Some(intel) = self.intel.clone()
        {
            intel.update(cx, |intel, cx| intel.close_hover(cx));
        }
    }

    /// Which pane this is, as code intelligence names it.
    fn intel_pane(&self) -> Pane {
        if self.is_file_view() {
            Pane::FileView
        } else {
            Pane::Stream
        }
    }

    /// The symbol drawn at `position` (window coordinates), through the lines this frame drew.
    fn symbol_under(&self, position: Point<Pixels>) -> Option<Pointed> {
        let (path, side, line, text) = {
            let geometry = self.geometry.borrow();
            let ((path, side, line), _) = geometry
                .iter()
                .find(|(_, drawn)| drawn.bounds.contains(&position))?;
            let text = self.line(path, *line)?.text.clone();
            (path.clone(), *side, *line, text)
        };
        let geometry = self.geometry.borrow();
        let drawn = geometry.get(&(path.clone(), side, line))?;
        let x = f32::from(position.x);
        let x_of = |at| f32::from(drawn.x_of(at));
        let unit = unit_at(&text, x, x_of)?;
        // Past the line's end, or before its start, is no symbol.
        if x < x_of(unit.start) || x >= x_of(unit.end) {
            return None;
        }
        let range = identifier_at(&text, unit.start)?;
        let token = Bounds::from_corners(
            point(drawn.x_of(range.start), drawn.bounds.top()),
            point(drawn.x_of(range.end), drawn.bounds.bottom()),
        );
        drop(geometry);
        self.pointed(&path, side, line, range, token)
    }

    /// The symbol at `range` of `path`'s line `line` (an index) on `side`.
    fn pointed(
        &self,
        path: &str,
        side: AnchorSide,
        line: usize,
        range: std::ops::Range<usize>,
        token: Bounds<Pixels>,
    ) -> Option<Pointed> {
        let diff_line = self.line(path, line)?;
        let number = side_number(diff_line, side)?;
        let diff = self.files.get(path)?.diff.clone()?;
        Some(Pointed {
            at: SymbolAt {
                path: path.to_string(),
                side,
                line: number,
                name: diff_line.text.get(range.clone())?.to_string(),
                range,
            },
            token,
            diff,
        })
    }

    /// The symbol at the caret, its token's bounds from the line as last drawn (else at the
    /// pane's top left).
    fn symbol_at_caret(&self) -> Option<Pointed> {
        let caret = self.caret.as_ref()?;
        let lines = self.lines(&caret.path)?;
        let line = line_index(lines, caret.side, caret.line)?;
        let range = identifier_at(&lines[line].text, caret.offset)?;
        let token = self
            .geometry
            .borrow()
            .get(&(caret.path.clone(), caret.side, line))
            .map(|drawn| {
                Bounds::from_corners(
                    point(drawn.x_of(range.start), drawn.bounds.top()),
                    point(drawn.x_of(range.end), drawn.bounds.bottom()),
                )
            })
            .unwrap_or_else(|| {
                let origin = self.list.viewport_bounds().origin;
                Bounds::new(origin, gpui::size(px(1.), px(20.)))
            });
        self.pointed(&caret.path.clone(), caret.side, line, range, token)
    }

    /// The pointer moved over the pane without a button held (module docs, "Hover").
    pub(super) fn hover_pointer_moved(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.intel.is_none() || self.text_drag.is_some() || self.line_drag.is_some() {
            return;
        }
        let shown = self.shown_card(cx).map(|card| card.token);
        // Over the card's token, or over the card: it stays.
        if shown.is_some_and(|token| token.contains(&position)) || self.hover_ui.over_card {
            self.hover_ui.closing = None;
            self.hover_ui.waiting = None;
            return;
        }
        match self.symbol_under(position) {
            Some(pointed) => {
                let same = self
                    .hover_ui
                    .waiting
                    .as_ref()
                    .is_some_and(|(at, _)| *at == pointed.at)
                    || self.asking(cx).is_some_and(|at| at == pointed.at);
                if same {
                    return;
                }
                // Another token: a hover asked for the one before is no longer wanted.
                self.cancel_asking(cx);
                let at = pointed.at.clone();
                let wait = cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(HOVER_DELAY).await;
                    this.update(cx, |this, cx| {
                        this.hover_ui.waiting = None;
                        this.ask_hover(pointed, cx);
                    })
                    .ok();
                });
                self.hover_ui.waiting = Some((at, wait));
                if shown.is_some() {
                    self.close_hover_soon(cx);
                }
            }
            None => {
                self.hover_ui.waiting = None;
                self.cancel_asking(cx);
                if shown.is_some() {
                    self.close_hover_soon(cx);
                }
            }
        }
        cx.notify();
    }

    /// The pointer left the pane: a hover waiting for it to rest, or asked for and not
    /// answered, is dropped, and the card closes after its grace unless the pointer went onto
    /// it.
    pub(super) fn pointer_left_pane(&mut self, cx: &mut Context<Self>) {
        self.hover_ui.waiting = None;
        self.cancel_asking(cx);
        if self.shown_card(cx).is_some() {
            self.close_hover_soon(cx);
        }
    }

    /// Asks for `pointed`'s hover.
    fn ask_hover(&mut self, pointed: Pointed, cx: &mut Context<Self>) {
        let Some(intel) = self.intel.clone() else {
            return;
        };
        let pane = self.intel_pane();
        self.hover_ui.closing = None;
        intel.update(cx, |intel, cx| {
            intel.hover(pointed.at, pane, pointed.token, &pointed.diff, cx)
        });
    }

    /// Closes the card once [`CLOSE_GRACE`] has passed, unless the pointer comes back.
    fn close_hover_soon(&mut self, cx: &mut Context<Self>) {
        if self.hover_ui.closing.is_some() {
            return;
        }
        self.hover_ui.closing = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CLOSE_GRACE).await;
            this.update(cx, |this, cx| {
                this.hover_ui.closing = None;
                // The card only: a wait for the token the pointer moved onto goes on.
                if !this.hover_ui.over_card
                    && this.shown_card(cx).is_some()
                    && let Some(intel) = this.intel.clone()
                {
                    intel.update(cx, |intel, cx| intel.close_card(cx));
                }
            })
            .ok();
        }));
    }

    /// Closes this pane's hover card (and a hover still waiting); `false` when there was none.
    pub(super) fn close_hover(&mut self, cx: &mut Context<Self>) -> bool {
        self.hover_ui.waiting = None;
        self.hover_ui.closing = None;
        self.hover_ui.over_card = false;
        let Some(intel) = self.intel.clone() else {
            return false;
        };
        let pane = self.intel_pane();
        let open = {
            let intel = intel.read(cx);
            intel.hover_card().is_some_and(|card| card.pane == pane)
                || intel.hovering().is_some_and(|(_, on)| on == pane)
        };
        if open {
            intel.update(cx, |intel, cx| intel.close_hover(cx));
        }
        open
    }

    /// `Ctrl I`: the hover card for the symbol at the caret, at once.
    pub(super) fn hover_at_caret(&mut self, cx: &mut Context<Self>) {
        if let Some(pointed) = self.symbol_at_caret() {
            self.ask_hover(pointed, cx);
        }
    }

    /// `F12`: go to the definition of the symbol at the caret.
    pub(super) fn definition_at_caret(&mut self, cx: &mut Context<Self>) {
        if let (Some(pointed), Some(intel)) = (self.symbol_at_caret(), self.intel.clone()) {
            let pane = self.intel_pane();
            intel.update(cx, |intel, cx| {
                intel.definition(pointed.at, pane, &pointed.diff, cx)
            });
        }
    }

    /// `Shift F12`: find the references of the symbol at the caret.
    pub(super) fn references_at_caret(&mut self, cx: &mut Context<Self>) {
        if let (Some(pointed), Some(intel)) = (self.symbol_at_caret(), self.intel.clone()) {
            let pane = self.intel_pane();
            intel.update(cx, |intel, cx| {
                intel.references_at(pointed.at, pane, &pointed.diff, cx)
            });
        }
    }

    /// A `Ctrl`+click in code: go to the definition of the identifier under the pointer, the
    /// caret put at it. `false` when there is none there (the press then selects as usual).
    pub(super) fn definition_click(
        &mut self,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(intel) = self.intel.clone() else {
            return false;
        };
        let Some(pointed) = self.symbol_under(event.position) else {
            return false;
        };
        self.close_hover(cx);
        self.caret = Some(super::select::TextPoint {
            path: pointed.at.path.clone(),
            side: pointed.at.side,
            line: pointed.at.line,
            offset: pointed.at.range.start,
        });
        let pane = self.intel_pane();
        intel.update(cx, |intel, cx| {
            intel.definition(pointed.at, pane, &pointed.diff, cx)
        });
        cx.notify();
        true
    }

    /// Shows line `line` of `side` of `path` (go to definition, a references row) with the
    /// cursor on it and the caret at byte `offset`: in the stream the gap hiding it opens; in
    /// the file view (opened on `path` first, by the shell) once its text is there.
    pub fn show_place(
        &mut self,
        path: &str,
        side: AnchorSide,
        line: u32,
        offset: usize,
        cx: &mut Context<Self>,
    ) {
        if !self.is_file_view() {
            self.show_line(path, side, line, cx);
        }
        self.pending_caret = Some(super::select::TextPoint {
            path: path.to_string(),
            side,
            line,
            offset,
        });
        self.place_pending_caret();
        cx.notify();
    }

    /// Puts the caret asked for by [`DiffPane::show_place`] once its line is there.
    pub(super) fn place_pending_caret(&mut self) {
        let Some(point) = self.pending_caret.as_ref() else {
            return;
        };
        let Some(lines) = self.lines(&point.path) else {
            return;
        };
        let Some(line) = line_index(lines, point.side, point.line) else {
            return;
        };
        let text = &lines[line].text;
        let mut offset = point.offset.min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        let mut point = point.clone();
        point.offset = offset;
        self.pending_caret = None;
        self.caret = Some(point);
    }

    /// The card's buttons, for the card this pane shows.
    fn card_definition(&mut self, cx: &mut Context<Self>) {
        let pane = self.intel_pane();
        self.card_request(cx, |intel, at, diff, cx| {
            intel.definition(at, pane, diff, cx)
        });
    }

    fn card_references(&mut self, cx: &mut Context<Self>) {
        let pane = self.intel_pane();
        self.card_request(cx, |intel, at, diff, cx| {
            intel.references_at(at, pane, diff, cx)
        });
    }

    fn card_request(
        &mut self,
        cx: &mut Context<Self>,
        ask: impl FnOnce(&mut CodeIntel, SymbolAt, &Arc<FileDiff>, &mut Context<CodeIntel>),
    ) {
        let Some(intel) = self.intel.clone() else {
            return;
        };
        let Some(at) = intel.read(cx).hover_card().map(|card| card.at.clone()) else {
            return;
        };
        let Some(diff) = self.files.get(&at.path).and_then(|ui| ui.diff.clone()) else {
            return;
        };
        self.hover_ui.waiting = None;
        self.hover_ui.closing = None;
        self.hover_ui.over_card = false;
        intel.update(cx, |intel, cx| ask(intel, at, &diff, cx));
    }

    /// Before a frame: a card on a file whose diff was replaced goes, and once the card has
    /// gone the keyboard comes back from its text (module docs).
    pub(super) fn hover_before_frame(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        let replaced = std::mem::take(&mut self.hover_ui.replaced);
        if let Some(card) = self.shown_card(cx)
            && replaced.contains(&card.at.path)
            && let Some(intel) = self.intel.clone()
        {
            intel.update(cx, |intel, cx| intel.close_hover(cx));
        }
        if self.shown_card(cx).is_none() && self.hover_ui.return_focus.is_some() {
            let back = self.hover_ui.return_focus.take();
            let left = self.hover_ui.card_focus.take();
            // Only when the keyboard is still where the press in the card put it.
            if let (Some(back), Some(left)) = (back, left)
                && window.focused(cx).as_ref() == Some(&left)
            {
                back.focus(window, cx);
            }
        }
    }

    /// A press in the card: where the keyboard was before, and (next frame) where it went.
    fn pressed_card(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        if self.hover_ui.return_focus.is_none() {
            self.hover_ui.return_focus = window.focused(cx);
        }
        let view = cx.entity();
        window.on_next_frame(move |window, cx| {
            view.update(cx, |this, cx| {
                this.hover_ui.card_focus = window.focused(cx);
            });
        });
    }

    /// The hover card this pane shows, hung under its token (module docs).
    pub(super) fn render_hover(
        &self,
        theme: &DvTheme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let intel = self.intel.clone()?;
        let card = self.shown_card(cx)?.clone();
        let view = cx.entity();
        let actions = CardActions {
            definition: std::rc::Rc::new({
                let view = view.clone();
                move |_, cx| view.update(cx, |this, cx| this.card_definition(cx))
            }),
            references: std::rc::Rc::new({
                let view = view.clone();
                move |_, cx| view.update(cx, |this, cx| this.card_references(cx))
            }),
            pressed: std::rc::Rc::new({
                let view = view.clone();
                move |window, cx| view.update(cx, |this, cx| this.pressed_card(window, cx))
            }),
            hovered: std::rc::Rc::new(move |over, _, cx| {
                view.update(cx, |this, cx| {
                    this.hover_ui.over_card = over;
                    if over {
                        this.hover_ui.closing = None;
                    } else {
                        this.close_hover_soon(cx);
                    }
                })
            }),
        };
        let element = render_card(&card, &intel, actions, theme, cx);
        Some(
            deferred(
                anchored()
                    .position(point(card.token.left(), card.token.bottom() + px(2.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(element),
            )
            .with_priority(CARD_PRIORITY)
            .into_any_element(),
        )
    }
}
