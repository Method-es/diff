//! The References tab's view (`super`, DESIGN §4.1 "References tab"): a header (the title, the
//! symbol, the count, **In this review**, a close button), then a virtualised list of the
//! files' path rows and their places.
//!
//! **Fitting a row.** A place's line shows without its indentation; when the symbol starts past
//! what the row has room for, the line's start gives way to "…" so the symbol stays in view
//! ([`elide_line`]). A file's path drops whole leading segments for "…/" rather than being cut
//! mid-word (`crate::components::elide_path_start`).

use std::collections::HashSet;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, Entity, FontWeight, HighlightStyle, Hsla, InteractiveElement as _,
    IntoElement, ListAlignment, ListState, ParentElement as _, Pixels, Render,
    StatefulInteractiveElement as _, Styled as _, StyledText, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};

use super::{RefFile, RefRow, References, floor_boundary};
use crate::automation::AutomationTargetExt as _;
use crate::components::{checkbox, elide_path_start, icon, skeleton, text_width};
use crate::icons::DvIcon;
use crate::intel::{Asked, CodeIntel};
use crate::services::ActiveServices as _;
use crate::theme::{ActiveTheme as _, DvTheme, StyledText as _};

/// A path row's height and the space above it, and a place's row's (DESIGN §4.1).
const FILE_ROW: f32 = 26.;
const PLACE_ROW: f32 = 24.;

/// The room a place's row takes before its text: the changed marker and its margin, the line
/// number's column (DESIGN §4.1).
const ROW_LEAD: f32 = 3. + 9. + 36.;

/// How far the list lays rows out past what is in view.
const OVERDRAW: f32 = 200.;

/// One item of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    /// File `file`'s path row, `shown` of its rows showing.
    File { file: usize, shown: usize },
    /// Row `row` of file `file`; `first` when it is the first row on its line.
    Place {
        file: usize,
        row: usize,
        changed: bool,
        first: bool,
    },
}

/// The References tab's body (module docs).
pub struct ReferencesPanel {
    intel: Option<Entity<CodeIntel>>,
    list: ListState,
    items: Rc<Vec<Item>>,
    /// What `items` were laid out for: the list's id, its shape and **In this review**.
    laid_out: Option<(u64, u64, bool)>,
    _subscription: Option<Subscription>,
}

impl ReferencesPanel {
    pub fn new() -> Self {
        Self {
            intel: None,
            list: ListState::new(0, ListAlignment::Top, px(OVERDRAW)),
            items: Rc::new(Vec::new()),
            laid_out: None,
            _subscription: None,
        }
    }

    /// Shows `intel`'s list (the open review's), or none.
    pub fn set_intel(&mut self, intel: Option<Entity<CodeIntel>>, cx: &mut Context<Self>) {
        self._subscription = intel
            .as_ref()
            .map(|intel| cx.observe(intel, |_, _, cx| cx.notify()));
        self.intel = intel;
        self.laid_out = None;
        self.items = Rc::new(Vec::new());
        self.list.reset(0);
        cx.notify();
    }

    /// Lays the list's items out again when the rows that show changed.
    fn lay_out(&mut self, list: &References) {
        let key = (list.id, list.shape, list.in_review);
        if self.laid_out == Some(key) {
            return;
        }
        let same_list = self.laid_out.is_some_and(|(id, _, _)| id == list.id);
        let files = list.files.as_deref().unwrap_or_default();
        let mut items = Vec::new();
        for (index, file) in files.iter().enumerate() {
            let rows: Vec<(usize, bool)> = file
                .rows
                .iter()
                .enumerate()
                .map(|(at, row)| (at, file.changed.contains(&row.line)))
                .filter(|(_, changed)| !list.in_review || *changed)
                .collect();
            if rows.is_empty() {
                continue;
            }
            items.push(Item::File {
                file: index,
                shown: rows.len(),
            });
            let mut seen = HashSet::new();
            for (row, changed) in rows {
                items.push(Item::Place {
                    file: index,
                    row,
                    changed,
                    first: seen.insert(file.rows[row].line),
                });
            }
        }
        if same_list && self.items.len() == items.len() {
            // The same rows, some now marked (or not): the place in the list stays.
            self.list.remeasure();
        } else {
            self.list.reset(items.len());
        }
        self.items = Rc::new(items);
        self.laid_out = Some(key);
    }

    /// One item of the list (module docs).
    fn render_item(
        &mut self,
        at: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.dv_theme().clone();
        let (Some(intel), Some(item)) = (self.intel.clone(), self.items.get(at).copied()) else {
            return div().into_any_element();
        };
        let width = px(cx.services().settings.read(cx).panel_width());
        let read = intel.read(cx);
        let Some(list) = read.references() else {
            return div().into_any_element();
        };
        let files = list.files.as_deref().unwrap_or_default();
        match item {
            Item::File { file, shown } => match files.get(file) {
                Some(file) => render_file(file, shown, width, &theme, window),
                None => div().into_any_element(),
            },
            Item::Place {
                file,
                row,
                changed,
                first,
            } => match files
                .get(file)
                .and_then(|f| f.rows.get(row).map(|r| (f, r)))
            {
                Some((file, row)) => {
                    let asked = Asked {
                        at: list.from.clone(),
                        pane: list.pane,
                        worktree: !list.against_worktree,
                        definition: false,
                    };
                    render_place(file, row, changed, first, asked, &intel, width, &theme, cx)
                }
                None => div().into_any_element(),
            },
        }
    }
}

impl Default for ReferencesPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for ReferencesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.dv_theme().clone();
        let body = div().id("references").flex_1().min_h_0().flex().flex_col();
        let Some(intel) = self.intel.clone() else {
            return body.automation_target("references");
        };
        let intel_ref = intel.read(cx);
        let Some(list) = intel_ref.references() else {
            return body.automation_target("references");
        };
        let header = render_header(list, &intel, &theme);
        let content: AnyElement = if list.is_loading() {
            div()
                .flex()
                .flex_col()
                .gap(theme.spacing.md)
                .px(theme.spacing.lg)
                .py(theme.spacing.md)
                .children((0..4).map(|_| skeleton(px(180.), px(10.), &theme)))
                .into_any_element()
        } else if let Some(error) = &list.error {
            note(error.clone(), &theme)
        } else {
            self.lay_out(list);
            if self.items.is_empty() {
                note(
                    if list.in_review && list.count() > 0 {
                        "None on lines this review changed".to_string()
                    } else {
                        format!("No {} found", list.kind.title().to_lowercase())
                    },
                    &theme,
                )
            } else {
                gpui::list(
                    self.list.clone(),
                    cx.processor(|this, at: usize, window, cx| this.render_item(at, window, cx)),
                )
                .size_full()
                .pb(theme.spacing.md)
                .into_any_element()
            }
        };
        body.child(header)
            .child(
                div()
                    .id("references-scroll")
                    .flex_1()
                    .min_h_0()
                    .child(content),
            )
            .automation_target("references")
    }
}

/// A quiet line in the tab's body.
fn note(text: String, theme: &DvTheme) -> AnyElement {
    div()
        .px(theme.spacing.lg)
        .py(theme.spacing.md)
        .text_spec(&theme.text.secondary)
        .text_color(theme.colors.fg3)
        .child(text)
        .into_any_element()
}

/// The tab's header: its title and the symbol with the count, then **In this review** and the
/// close button; "Read against the working tree" under it when that is so.
fn render_header(list: &References, intel: &Entity<CodeIntel>, theme: &DvTheme) -> AnyElement {
    let c = &theme.colors;
    // The count shows as soon as the server answered, before the rows' lines are read.
    let count = (!list.is_loading() && list.error.is_none()).then(|| list.count());
    let in_review = list.in_review;
    let toggle = {
        let intel = intel.clone();
        div()
            .id("references-in-review")
            .flex()
            .items_center()
            .gap(theme.spacing.sm)
            .cursor_pointer()
            .text_spec(&theme.text.secondary)
            .text_color(c.fg2)
            .child(checkbox(in_review, theme))
            .child("In this review")
            .on_click(move |_, _, cx| {
                intel.update(cx, |intel, cx| intel.set_in_review(!in_review, cx));
            })
            .automation_target("references/in_review")
    };
    let close = {
        let intel = intel.clone();
        div()
            .id("references-close")
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .size(theme.sizes.control_xsmall_height)
            .rounded(theme.radii.control)
            .cursor_pointer()
            .hover(|style| style.bg(c.control_hover))
            .child(icon(DvIcon::X, px(14.), c.fg2))
            .tooltip(crate::components::tooltip("Close the references"))
            .on_click(move |_, _, cx| {
                intel.update(cx, |intel, cx| intel.close_references(cx));
            })
            .automation_target("references/close")
    };
    div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(theme.spacing.sm)
        .pt(theme.spacing.md)
        .px(theme.spacing.lg)
        .pb(theme.spacing.md)
        .border_b_1()
        .border_color(c.line)
        .child(
            div()
                .flex()
                .items_center()
                .gap(theme.spacing.md)
                .child(
                    div()
                        .text_spec(&theme.text.title)
                        .text_color(c.fg)
                        .child(list.kind.title()),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_spec(&theme.text.inline_code)
                        .text_color(c.accent_text)
                        .child(list.from.name.clone()),
                )
                .children(count.map(|count| div().text_color(c.fg3).child(count.to_string())))
                .child(div().flex_1())
                .child(close),
        )
        .child(toggle)
        .when(list.against_worktree, |this| {
            this.child(
                div()
                    .text_spec(&theme.text.secondary)
                    .text_color(c.fg3)
                    .child("Read against the working tree"),
            )
        })
        .into_any_element()
}

/// One file's path row: its glyph, its path (whole leading segments giving way to "…/" when
/// it doesn't fit), "outside the repository" for one outside it, and how many of its rows show.
fn render_file(
    file: &RefFile,
    shown: usize,
    width: Pixels,
    theme: &DvTheme,
    window: &mut Window,
) -> AnyElement {
    let c = &theme.colors;
    let outside = file.worktree_path().is_none();
    let spec = &theme.text.secondary;
    let count = shown.to_string();
    let outside_label = "outside the repository";
    let gap = theme.spacing.sm;
    // The row's own room less its padding, the glyph, the count and the gaps between them.
    let mut room = width - theme.spacing.lg * 2. - px(13.) - gap * 3. - px(2.);
    room -= text_width(window, &count, spec);
    if outside {
        room -= text_width(window, outside_label, spec) + gap;
    }
    // The name is drawn at 600, a little wider than measured at 400.
    let label = elide_path_start(file.label(), |text| {
        text_width(window, text, spec) * 1.06 <= room
    });
    // Either separator: a file outside the repository may be a Windows path.
    let (dir, name) = match label.rfind(['/', '\\']) {
        Some(at) => label.split_at(at + 1),
        None => ("", label.as_str()),
    };
    div()
        .flex()
        .items_center()
        .gap(gap)
        .h(px(FILE_ROW))
        .mt(theme.spacing.sm)
        .px(theme.spacing.lg)
        .child(icon(DvIcon::File, px(13.), c.fg3))
        .child(
            div()
                .min_w_0()
                .flex()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_spec(spec)
                .child(div().flex_none().text_color(c.fg3).child(dir.to_string()))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(c.fg)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name.to_string()),
                ),
        )
        .when(outside, |this| {
            this.child(
                div()
                    .flex_none()
                    .text_spec(spec)
                    .text_color(c.fg3)
                    .child(outside_label),
            )
        })
        .child(div().flex_1())
        .child(
            div()
                .flex_none()
                .text_spec(spec)
                .text_color(c.fg3)
                .child(count),
        )
        .into_any_element()
}

/// One place: the changed marker, the line number, the line's text with the symbol marked
/// (module docs, "Fitting a row").
#[allow(clippy::too_many_arguments)]
fn render_place(
    file: &RefFile,
    row: &RefRow,
    changed: bool,
    first_on_line: bool,
    asked: Asked,
    intel: &Entity<CodeIntel>,
    width: Pixels,
    theme: &DvTheme,
    cx: &App,
) -> AnyElement {
    let c = &theme.colors;
    let label = file.label();
    let text = row.text.clone().unwrap_or_default();
    let indent = text.len() - text.trim_start().len();
    let symbol = row.symbol();
    // The text's room in characters of the code font.
    let room = width - px(ROW_LEAD) - theme.spacing.lg;
    let advance = {
        let system = cx.text_system();
        let font = system.resolve_font(&gpui::font(theme.text.inline_code.family.clone()));
        system
            .ch_advance(font, theme.text.inline_code.size)
            .unwrap_or(px(7.))
    };
    let columns = (f32::from(room) / f32::from(advance).max(1.))
        .floor()
        .max(8.) as usize;
    let mark = (symbol.end > symbol.start && symbol.start >= indent).then_some(symbol.clone());
    let shown = elide_line(&text, indent, mark.clone(), columns);
    let colors: Vec<(Range<usize>, Hsla)> = row
        .tokens
        .iter()
        .filter_map(|token| {
            let range = shown.map(token.range.start as usize..token.range.end as usize)?;
            Some((range, token.kind.color(&c.syntax)))
        })
        .collect();
    let mark = mark.and_then(|mark| shown.map(mark));
    let highlights = runs(&shown.text, &colors, mark, c.find_current);
    let styled = StyledText::new(shown.text.clone()).with_highlights(highlights);
    let location = row.location.clone();
    let intel = intel.clone();
    let hover = c.control_hover;
    let id = format!("references/row/{label}/{}", row.line);
    // One element per place, two on one line apart by their column.
    let element_id = format!("{id}/{}", row.location.range.start.column);
    let element = div()
        .id(gpui::ElementId::Name(element_id.into()))
        .flex()
        .items_center()
        .h(px(PLACE_ROW))
        .pr(theme.spacing.lg)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .child(
            div()
                .flex_none()
                .w(px(3.))
                .h(px(16.))
                .mr(px(9.))
                .rounded(px(1.5))
                .when(changed, |this| this.bg(c.green)),
        )
        .child(
            div()
                .flex_none()
                .w(px(36.))
                .pr(theme.spacing.md)
                .flex()
                .justify_end()
                .text_spec(&theme.text.kbd)
                .text_color(c.fg3)
                .child(row.line.to_string()),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_spec(&theme.text.inline_code)
                .text_color(c.syntax.plain)
                .child(styled),
        )
        .on_click(move |_, _, cx| {
            let location = location.clone();
            let asked = asked.clone();
            intel.update(cx, |intel, cx| intel.go_to(location, Some(asked), cx));
        });
    if first_on_line {
        element.automation_target(id)
    } else {
        element.into_any_element()
    }
}

/// A line as a row shows it: its text from `from` (a byte of the source line) on, after
/// `prefix` ("…" when its start gave way), and how a range of the source line maps into it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShownLine {
    pub text: String,
    from: usize,
    prefix: usize,
}

impl ShownLine {
    /// `range` (bytes of the source line) in the shown text; `None` once nothing of it shows.
    pub fn map(&self, range: Range<usize>) -> Option<Range<usize>> {
        let start = range.start.max(self.from);
        if range.end <= start {
            return None;
        }
        let shift = |at: usize| at - self.from + self.prefix;
        let (start, end) = (shift(start), shift(range.end).min(self.text.len()));
        (end > start).then_some(start..end)
    }
}

/// `text` as a row of `columns` characters shows it (module docs, "Fitting a row"): without
/// its first `indent` bytes, and when `symbol` (bytes of `text`) would end past the row, from a
/// little before the symbol on, after "…".
pub fn elide_line(
    text: &str,
    indent: usize,
    symbol: Option<Range<usize>>,
    columns: usize,
) -> ShownLine {
    let indent = floor_boundary(text, indent);
    let plain = ShownLine {
        text: text[indent..].to_string(),
        from: indent,
        prefix: 0,
    };
    let Some(symbol) = symbol else {
        return plain;
    };
    let chars = |range: Range<usize>| text[range].chars().count();
    let (start, end) = (
        floor_boundary(text, symbol.start.max(indent)),
        floor_boundary(text, symbol.end.max(indent)),
    );
    let before = chars(indent..start);
    let length = chars(start..end);
    if before + length <= columns {
        return plain;
    }
    // A third of what the symbol leaves of the row, at least a few characters, before it.
    let lead = (columns.saturating_sub(length + 1) / 3).max(4).min(before);
    let from = text[indent..start]
        .char_indices()
        .nth(before - lead)
        .map_or(start, |(at, _)| indent + at);
    ShownLine {
        text: format!("…{}", &text[from..]),
        from,
        prefix: '…'.len_utf8(),
    }
}

/// The highlight runs of `text`: each token's colour (`colors`, sorted, without overlaps), and
/// over `mark` (the symbol) a `mark_bg` background at 600, whatever token it is in. Every cut
/// falls on a character boundary of `text`, whatever ranges it is given.
pub fn runs(
    text: &str,
    colors: &[(Range<usize>, Hsla)],
    mark: Option<Range<usize>>,
    mark_bg: Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let on = |at: usize| floor_boundary(text, at);
    let colors: Vec<(Range<usize>, Hsla)> = colors
        .iter()
        .map(|(range, color)| (on(range.start)..on(range.end), *color))
        .filter(|(range, _)| range.end > range.start)
        .collect();
    let mark = mark
        .map(|mark| on(mark.start)..on(mark.end))
        .filter(|mark| mark.end > mark.start);
    let len = text.len();
    let mut cuts: Vec<usize> = vec![0, len];
    for (range, _) in &colors {
        cuts.extend([range.start, range.end]);
    }
    if let Some(mark) = &mark {
        cuts.extend([mark.start, mark.end]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .filter_map(|pair| {
            let piece = pair[0]..pair[1];
            let color = colors
                .iter()
                .find(|(range, _)| range.start <= piece.start && piece.end <= range.end)
                .map(|(_, color)| *color);
            let marked = mark
                .as_ref()
                .is_some_and(|mark| mark.start <= piece.start && piece.end <= mark.end);
            (color.is_some() || marked).then(|| {
                (
                    piece,
                    HighlightStyle {
                        color,
                        background_color: marked.then_some(mark_bg),
                        font_weight: marked.then_some(FontWeight::SEMIBOLD),
                        ..Default::default()
                    },
                )
            })
        })
        .collect()
}
