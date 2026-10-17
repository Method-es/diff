//! The hover card (PLAN §18.1 "Hover", DESIGN §4.1 "Hover card"): what a language server says
//! about the symbol under the pointer (or at the caret), hung under the token on the `overlay`
//! surface.
//!
//! Several servers can serve a file (a `.tsx`: TypeScript and Tailwind CSS): their answers
//! stack in the registry's order, each as it lands, a hairline between them and, when more than
//! one answered, each headed by its server's name. Each answer's parts stack in the server's
//! order: Markdown through [`crate::markdown`] (sanitised, its fenced signature highlighted as
//! code), code in a named language as a fenced block, plain text as it is. Its text can be
//! selected (the copy goes through the clipboard seam,
//! `crate::shell`). A link to a file opens it (the stream or the file view, as go to definition
//! does; `#L12` names a line); any other link goes through [`crate::system::open_url`]. Under
//! the parts, "Read against the working tree" when the server read another side's text, then
//! **Go to definition** (`F12`) and **Find references** (`Shift F12`).
//!
//! Automation targets: `hover` (the card), `hover/definition`, `hover/references`.

use std::rc::Rc;

use dv_lsp::{HoverInfo, HoverPart, SymbolLocation, TextPosition, TextRange, WorkspacePaths};
use gpui::{
    AnyElement, App, Bounds, Display, Element, Entity, GlobalElementId, InspectorElementId,
    InteractiveElement as _, IntoElement, LayoutId, ParentElement as _, Pixels, Position,
    SharedString, StatefulInteractiveElement as _, Style, Styled as _, Window, div, point,
    prelude::FluentBuilder as _, px,
};

use super::{CodeIntel, HoverCard};
use crate::automation::AutomationTargetExt as _;
use crate::components::{kbd, small_button};
use crate::keymap;
use crate::theme::{DvTheme, StyledText as _};

/// How far below (or above) what it is about the card hangs, and how far in from the main
/// area's edges it stays (DESIGN §4.1 "Hover card").
pub const CARD_GAP: f32 = 2.;
pub const CARD_MARGIN: f32 = 8.;

/// The card's width bounds and its tallest before it scrolls.
pub const CARD_MIN_WIDTH: f32 = 280.;
pub const CARD_MAX_WIDTH: f32 = 560.;
pub const CARD_MAX_HEIGHT: f32 = 360.;

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// Told whether the pointer is over the card.
type HoverHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// What the card's parts and buttons do.
pub struct CardActions {
    pub definition: Handler,
    pub references: Handler,
    /// The pointer entered (`true`) or left the card.
    pub hovered: HoverHandler,
    /// A press in the card, before anything in it hears it (its text takes the keyboard).
    pub pressed: Handler,
}

/// One answer's parts as one Markdown source: Markdown as it is, code fenced in its
/// language, plain text as a paragraph of its own (its Markdown characters escaped).
pub fn markdown_of(info: &HoverInfo) -> String {
    info.parts
        .iter()
        .map(|part| match part {
            HoverPart::Markdown(text) => text.trim().to_string(),
            HoverPart::Code { language, text } => {
                format!("```{language}\n{}\n```", text.trim_end())
            }
            HoverPart::Plain(text) => escape_markdown(text.trim()),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// `text` with every character Markdown would read as markup escaped.
fn escape_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if "\\`*_{}[]()#+-.!<>|~".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Where a link in a hover card goes: a file of the repository (at `#L<line>` or
/// `#L<line>,<column>`, 1-based, when the link names one), else `None` for a web link.
pub fn link_target(url: &str, paths: &WorkspacePaths) -> Option<SymbolLocation> {
    if !url.starts_with("file:") {
        return None;
    }
    let (uri, fragment) = url.split_once('#').unwrap_or((url, ""));
    let fragment = fragment.trim_start_matches('L');
    let mut numbers = fragment
        .split([',', ':', 'C', 'c'])
        .map(|part| part.trim_start_matches('L').parse::<u32>().ok());
    let line = numbers.next().flatten().unwrap_or(1).max(1);
    let column = numbers.next().flatten().unwrap_or(1).max(1);
    let at = TextPosition::new(line - 1, column - 1);
    Some(SymbolLocation {
        target: paths.target(uri),
        range: TextRange { start: at, end: at },
    })
}

/// The card for `card` (module docs), with `actions`.
pub fn render_card(
    card: &HoverCard,
    intel: &Entity<CodeIntel>,
    actions: CardActions,
    theme: &DvTheme,
    cx: &App,
) -> AnyElement {
    let c = &theme.colors;
    if let Some(note) = card.note.filter(|_| card.sections.is_empty()) {
        // Nothing to say, and why: one quiet line.
        let hovered = actions.hovered.clone();
        return div()
            .id(("hover-card", card.id as usize))
            .occlude()
            .max_w(px(CARD_MAX_WIDTH))
            .px(theme.spacing.lg)
            .py(theme.spacing.md)
            .rounded(theme.radii.card)
            .border_1()
            .border_color(c.overlay_line)
            .bg(c.popover)
            .shadow(theme.shadows.overlay.clone())
            .text_spec(&theme.text.secondary)
            .text_color(c.fg3)
            .on_hover(move |over, window, cx| hovered(*over, window, cx))
            .child(note)
            .automation_target("hover")
            .into_any_element();
    }
    let paths = intel.read(cx).paths(cx);
    let link_intel = intel.clone();
    let asked = super::Asked {
        at: card.at.clone(),
        pane: card.pane,
        worktree: !card.against_worktree,
        definition: false,
    };
    let labelled = card.sections.len() > 1;
    let sections = card.sections.iter().enumerate().map(|(index, section)| {
        let (paths, link_intel, asked) = (paths.clone(), link_intel.clone(), asked.clone());
        let body = crate::markdown::view_with_links(
            SharedString::from(format!("hover-{}-{}", card.id, section.spec_id)),
            &markdown_of(&section.info),
            move |url, cx| match link_target(url, &paths) {
                Some(location) => link_intel.update(cx, |intel, cx| {
                    intel.go_to(location, Some(asked.clone()), cx)
                }),
                None => crate::system::open_url(url, cx),
            },
            cx,
        )
        .selectable(true);
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.xs)
            .when(index > 0, |this| {
                this.mt(theme.spacing.md)
                    .pt(theme.spacing.md)
                    .border_t_1()
                    .border_color(c.line)
            })
            .when(labelled, |this| {
                this.child(
                    div()
                        .text_spec(&theme.text.secondary)
                        .text_color(c.fg3)
                        .child(section.server),
                )
            })
            .child(body)
            .automation_target(format!("hover/section/{}", section.spec_id))
    });
    let hovered = actions.hovered.clone();
    let pressed = actions.pressed.clone();
    let definition = actions.definition.clone();
    let references = actions.references.clone();
    let button = |id: &'static str, label: &'static str, keys: &'static str, handler: Handler| {
        small_button(id, label, theme)
            .child(kbd(keymap::label(keys), theme))
            .on_click(move |_, window, cx| handler(window, cx))
            .automation_target(format!("hover/{id}"))
    };
    div()
        // Its own id per answer, so the pointer over a new card is heard as entering it.
        .id(("hover-card", card.id as usize))
        .occlude()
        .min_w(px(CARD_MIN_WIDTH))
        .max_w(px(CARD_MAX_WIDTH))
        .flex()
        .flex_col()
        .rounded(theme.radii.card)
        .border_1()
        .border_color(c.overlay_line)
        .bg(c.popover)
        .shadow(theme.shadows.overlay.clone())
        .text_spec(&theme.text.body)
        .text_color(c.fg)
        .on_hover(move |over, window, cx| hovered(*over, window, cx))
        .capture_any_mouse_down(move |_, window, cx| pressed(window, cx))
        .child(
            div()
                .id("hover-card-body")
                .max_h(px(CARD_MAX_HEIGHT))
                .overflow_y_scroll()
                .px(theme.spacing.lg)
                .py(theme.spacing.md)
                .children(sections),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(theme.spacing.md)
                .px(theme.spacing.lg)
                .py(theme.spacing.md)
                .border_t_1()
                .border_color(c.line)
                .child(button("definition", "Go to definition", "f12", definition))
                .child(button(
                    "references",
                    "Find references",
                    "shift-f12",
                    references,
                ))
                .when(card.against_worktree, |this| {
                    this.child(div().flex_1()).child(
                        div()
                            .text_spec(&theme.text.secondary)
                            .text_color(c.fg3)
                            .child("Read against the working tree"),
                    )
                }),
        )
        .automation_target("hover")
        .into_any_element()
}

/// Where a card of `size` hangs off `target` (window coordinates) within `area` (DESIGN §4.1
/// "Hover card"): [`CARD_GAP`] below it, else, when it doesn't fit below, as far above its top;
/// when it fits neither way, on the side with more room, kept inside; its left edge at the
/// target's, moved in to stay [`CARD_MARGIN`] inside the area's sides and bottom and top.
pub fn hang_origin(
    size: gpui::Size<Pixels>,
    target: Bounds<Pixels>,
    area: Bounds<Pixels>,
) -> gpui::Point<Pixels> {
    let (gap, margin) = (px(CARD_GAP), px(CARD_MARGIN));
    let (top, bottom) = (area.top() + margin, area.bottom() - margin);
    let below = target.bottom() + gap;
    let above = target.top() - gap - size.height;
    let y = if below + size.height <= bottom {
        below
    } else if above >= top {
        above
    } else if bottom - target.bottom() >= target.top() - top {
        (bottom - size.height).max(top)
    } else {
        top
    };
    let right = area.right() - margin - size.width;
    let x = target.left().min(right).max(area.left() + margin);
    point(x, y)
}

/// Hangs its child off what `place` names (the bounds the card is about, and the area it
/// stays in), worked out as the frame is laid out ([`hang_origin`]): after every line of the
/// frame put its geometry back (`deferred` lays this out last), so the card hangs under the
/// servers' range as drawn now, never under last frame's.
pub struct Hang {
    child: AnyElement,
    place: Box<dyn Fn() -> (Bounds<Pixels>, Bounds<Pixels>)>,
}

/// [`Hang`] for `child`.
pub fn hang(
    child: impl IntoElement,
    place: impl Fn() -> (Bounds<Pixels>, Bounds<Pixels>) + 'static,
) -> Hang {
    Hang {
        child: child.into_any_element(),
        place: Box::new(place),
    }
}

impl IntoElement for Hang {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Hang {
    type RequestLayoutState = LayoutId;
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, LayoutId) {
        let child = self.child.request_layout(window, cx);
        let style = Style {
            position: Position::Absolute,
            display: Display::Flex,
            ..Style::default()
        };
        (window.request_layout(style, [child], cx), child)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child: &mut LayoutId,
        window: &mut Window,
        cx: &mut App,
    ) {
        let size = window.layout_bounds(*child).size;
        let (target, area) = (self.place)();
        let origin = hang_origin(size, target, area);
        let offset = origin - bounds.origin;
        let offset = point(offset.x.round(), offset.y.round());
        window.with_element_offset(offset, |window| self.child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _child: &mut LayoutId,
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use dv_lsp::UriTarget;

    use super::*;

    #[test]
    fn file_links_name_a_line_and_web_links_none() {
        let paths = WorkspacePaths::new("/home/kyle/web");
        let at = |line, column| {
            let at = TextPosition::new(line, column);
            TextRange { start: at, end: at }
        };
        assert_eq!(
            link_target("file:///home/kyle/web/src/b.ts#L12", &paths),
            Some(SymbolLocation {
                target: UriTarget::Worktree("src/b.ts".into()),
                range: at(11, 0),
            })
        );
        assert_eq!(
            link_target("file:///home/kyle/web/src/b.ts#L3,7", &paths).map(|l| l.range),
            Some(at(2, 6))
        );
        assert_eq!(
            link_target("file:///home/kyle/web/src/b.ts", &paths).map(|l| l.range),
            Some(at(0, 0))
        );
        assert_eq!(
            link_target("file:///usr/lib/x.d.ts#L1", &paths).map(|l| l.target),
            Some(UriTarget::Outside {
                uri: "file:///usr/lib/x.d.ts".into(),
                path: Some("/usr/lib/x.d.ts".into()),
            })
        );
        assert_eq!(link_target("https://example.com/docs#L3", &paths), None);
    }

    #[test]
    fn the_card_hangs_below_flips_above_and_stays_inside() {
        let area = Bounds::from_corners(point(px(300.), px(40.)), point(px(1440.), px(872.)));
        let token = |x: f32, y: f32| {
            Bounds::from_corners(point(px(x), px(y)), point(px(x + 60.), px(y + 20.)))
        };
        let size = gpui::size(px(400.), px(200.));
        // Room below: 2 under the token, at its left edge.
        assert_eq!(
            hang_origin(size, token(500., 100.), area),
            point(px(500.), px(122.))
        );
        // At the bottom: above the token's top, never over the status bar below the area.
        assert_eq!(
            hang_origin(size, token(500., 800.), area),
            point(px(500.), px(598.))
        );
        // At the right edge: moved in to stay 8 inside.
        assert_eq!(hang_origin(size, token(1400., 100.), area).x, px(1032.));
        // Too tall either way: the roomier side, inside the area.
        let tall = gpui::size(px(400.), px(700.));
        let origin = hang_origin(tall, token(500., 300.), area);
        assert_eq!(origin.y, px(864.) - px(700.));
        let origin = hang_origin(tall, token(500., 700.), area);
        assert_eq!(origin.y, px(48.));
    }

    #[test]
    fn parts_become_one_markdown_source() {
        let card = HoverCard {
            at: super::super::SymbolAt {
                path: "a.ts".into(),
                side: dv_core::review::AnchorSide::New,
                line: 1,
                range: 0..1,
                name: "a".into(),
                offset: 0,
                worktree: false,
            },
            span: 0..1,
            pane: super::super::Pane::Stream,
            token: Default::default(),
            sections: vec![super::super::HoverSection {
                order: 0,
                spec_id: "typescript",
                server: "TypeScript",
                info: dv_lsp::HoverInfo {
                    parts: vec![
                        HoverPart::Code {
                            language: "typescript".into(),
                            text: "const a: number\n".into(),
                        },
                        HoverPart::Markdown("The *answer*.".into()),
                        HoverPart::Plain("a_b [c]".into()),
                    ],
                    range: None,
                },
            }],
            note: None,
            against_worktree: false,
            id: 1,
        };
        assert_eq!(
            markdown_of(&card.sections[0].info),
            "```typescript\nconst a: number\n```\n\nThe *answer*.\n\na\\_b \\[c\\]"
        );
    }
}
