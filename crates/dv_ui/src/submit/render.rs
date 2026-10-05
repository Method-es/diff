//! The sheet's layout (DESIGN §4.2, the board's `GlassSubmit` dialog: 980 wide, radius 12, the
//! `overlay` surface; a header, a 420px left column with the summary and the verdicts, the
//! round's comments on the right, and a footer with Cancel and the primary button).

use dv_core::review::{AuthorKind, ThreadState, Verdict};
use gpui::{
    AnyElement, Context, ElementId, Focusable as _, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Window,
    div, prelude::FluentBuilder as _, px,
};
use gpui_component::input::Textarea;

use super::{FinishSheet, SHEET_WIDTH, SummaryTab, VERDICTS, verdict_label};
use crate::automation::AutomationTargetExt as _;
use crate::components::{button, count_badge, icon, kbd, kbd_on_accent, primary_button, totals};
use crate::icons::DvIcon;
use crate::keymap;
use crate::theme::{ActiveTheme as _, DvTheme, StyledText as _};
use crate::thread::{first_line, location_label};

/// The left column's width (the board's `420px` grid column).
const LEFT_WIDTH: f32 = 420.;
/// The summary field's and its preview's height.
const SUMMARY_HEIGHT: f32 = 150.;
/// The tallest the comments list grows before it scrolls.
const COMMENTS_HEIGHT: f32 = 330.;

impl Render for FinishSheet {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.dv_theme().clone();
        let c = &theme.colors;
        let viewport = window.viewport_size();
        let max_height = (viewport.height
            - theme.sizes.title_bar_height
            - super::sheet_top(viewport.height)
            - px(24.))
        .max(px(320.));
        div()
            .id("sheet")
            .key_context(keymap::SHEET)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_submit))
            .on_action(cx.listener(Self::on_dismiss))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::pick_approve))
            .on_action(cx.listener(Self::pick_request_changes))
            .on_action(cx.listener(Self::switch_focus))
            .w(SHEET_WIDTH)
            .max_h(max_height)
            .flex()
            .flex_col()
            .rounded(theme.radii.sheet)
            .border_1()
            .border_color(c.overlay_line)
            .bg(c.overlay)
            .shadow(theme.shadows.overlay.clone())
            .overflow_hidden()
            .occlude()
            .text_spec(&theme.text.body)
            .text_color(c.fg)
            .child(self.render_header(&theme, cx))
            .child(
                div()
                    .flex()
                    .min_h_0()
                    .child(self.render_left(&theme, window, cx))
                    .child(self.render_comments(&theme, cx)),
            )
            .child(self.render_footer(&theme, cx))
            .automation_target("sheet")
    }
}

impl FinishSheet {
    fn render_header(&self, theme: &DvTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let session = self.session.read(cx);
        let files = session.files();
        let additions: u64 = files.iter().map(|entry| u64::from(entry.additions)).sum();
        let deletions: u64 = files.iter().map(|entry| u64::from(entry.deletions)).sum();
        let repo = session.backend().repo().location.short_name();
        let hover = c.control_hover;
        let closable = !self.submitting;
        let dot = || div().text_color(c.fg3).child("·");
        div()
            .flex_none()
            .pt(theme.spacing.xl)
            .pr(px(14.))
            .pb(px(14.))
            .pl(px(20.))
            .border_b_1()
            .border_color(c.line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .text_spec(&theme.text.heading)
                            .child(format!("Finish round {}", session.review().current_round())),
                    )
                    .child(div().flex_1())
                    .child(kbd(keymap::label("escape"), theme))
                    .child(
                        div()
                            .id("sheet-close")
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(theme.sizes.control_height)
                            .rounded(theme.radii.control)
                            .child(icon(DvIcon::X, px(15.), c.fg2))
                            // Inert while the round is being finished (module docs).
                            .map(|this| {
                                if closable {
                                    this.cursor_pointer()
                                        .hover(move |style| style.bg(hover))
                                        .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx)))
                                } else {
                                    this.opacity(0.5).cursor_default()
                                }
                            })
                            .automation_target("sheet/close"),
                    ),
            )
            .child(
                div()
                    .mt(theme.spacing.xs)
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .text_color(c.fg2)
                    .child(div().text_color(c.fg).child(repo))
                    .child(dot())
                    .child(
                        div()
                            .font_family(theme.fonts.mono.clone())
                            .text_size(px(12.))
                            .child(session.target_label()),
                    )
                    .child(dot())
                    .child(format!(
                        "{} {}",
                        files.len(),
                        if files.len() == 1 { "file" } else { "files" }
                    ))
                    .child(totals(additions, deletions, px(12.), theme)),
            )
            .into_any_element()
    }

    fn render_left(&self, theme: &DvTheme, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let verdicts: Vec<AnyElement> = VERDICTS
            .iter()
            .enumerate()
            .map(|(ix, verdict)| self.render_verdict(*verdict, ix + 1, theme, cx))
            .collect();
        let segment = |key: &'static str, label: &'static str, tab: SummaryTab| {
            let on = self.tab == tab;
            let hover = c.control_hover;
            div()
                .id(ElementId::Name(format!("sheet-{key}").into()))
                .flex()
                .items_center()
                .h(theme.sizes.control_xsmall_height)
                .px(theme.spacing.md)
                .rounded(theme.radii.chip)
                .text_spec(&theme.text.secondary)
                .font_weight(FontWeight::MEDIUM)
                .text_color(if on { c.fg } else { c.fg2 })
                .when(on, |this| {
                    this.bg(c.seg_on).shadow(theme.shadows.control.clone())
                })
                .when(!on, |this| {
                    this.cursor_pointer().hover(move |style| style.bg(hover))
                })
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| this.set_tab(tab, window, cx)))
                .automation_target(format!("sheet/{key}"))
        };
        let summary_focused = self.summary.read(cx).focus_handle(cx).is_focused(window);
        let summary: AnyElement = match self.tab {
            // The field pads its own text (gpui-component's multi-line paddings).
            SummaryTab::Write => div()
                .h(px(SUMMARY_HEIGHT))
                .rounded(px(8.))
                .border_1()
                .border_color(if summary_focused {
                    c.accent_ring
                } else {
                    c.ctrl_line
                })
                .bg(c.input_bg)
                .overflow_hidden()
                .child(
                    Textarea::new(&self.summary)
                        .appearance(false)
                        .aria_label("Round summary")
                        .font_family(theme.fonts.mono.clone())
                        .text_size(px(12.5))
                        .line_height(px(20.))
                        .text_color(c.fg),
                )
                .automation_target("sheet/summary")
                .into_any_element(),
            SummaryTab::Preview => {
                let text = self.summary.read(cx).value().to_string();
                let body = if text.trim().is_empty() {
                    div()
                        .text_color(c.fg3)
                        .child("Nothing to preview")
                        .into_any_element()
                } else {
                    crate::markdown::view("sheet-preview", &text, cx)
                        .text_size(theme.text.body.size)
                        .line_height(px(20.))
                        .text_color(c.fg)
                        .into_any_element()
                };
                div()
                    .id("sheet-preview-box")
                    .h(px(SUMMARY_HEIGHT))
                    .px(theme.spacing.lg)
                    .py(px(10.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(c.line)
                    .bg(c.layer)
                    .overflow_y_scroll()
                    .child(body)
                    .automation_target("sheet/summary")
                    .into_any_element()
            }
        };
        div()
            .flex_none()
            .w(px(LEFT_WIDTH))
            .flex()
            .flex_col()
            .pt(theme.spacing.xl)
            .px(px(20.))
            .pb(px(18.))
            .border_r_1()
            .border_color(c.line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mb(theme.spacing.md)
                    .child(div().text_spec(&theme.text.title).child("Summary"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .h(theme.sizes.control_height)
                            .p(px(2.))
                            .rounded(theme.radii.segmented)
                            .border_1()
                            .border_color(c.ctrl_line)
                            .bg(c.ctrl)
                            .child(segment("write", "Write", SummaryTab::Write))
                            .child(segment("preview", "Preview", SummaryTab::Preview)),
                    ),
            )
            .child(summary)
            .child(
                div()
                    .mt(theme.spacing.sm)
                    .flex()
                    .items_center()
                    .gap(theme.spacing.sm)
                    .text_spec(&theme.text.secondary)
                    .text_color(c.fg3)
                    .child("Markdown · optional")
                    .child(div().flex_1())
                    .child(kbd(keymap::label("tab"), theme))
                    .child(if summary_focused {
                        "to the verdict"
                    } else {
                        "to write"
                    }),
            )
            .child(
                div()
                    .mt(px(18.))
                    .mb(theme.spacing.md)
                    .text_spec(&theme.text.title)
                    .child("Verdict"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.sm)
                    .children(verdicts),
            )
            .into_any_element()
    }

    /// A verdict's radio card (the board's: a 16px dot, the label with its glyph, a line of
    /// description and the key that picks it).
    fn render_verdict(
        &self,
        verdict: Verdict,
        key: usize,
        theme: &DvTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let c = &theme.colors;
        let on = self.verdict == verdict;
        let (glyph, color, description, name) = match verdict {
            Verdict::RequestChanges => (
                DvIcon::CircleMinus,
                c.red,
                "The agent has more to do; the round goes back to it",
                "request_changes",
            ),
            _ => (
                DvIcon::CircleCheck,
                c.green,
                "The work is done; the review closes",
                "approve",
            ),
        };
        div()
            .id(ElementId::Name(format!("sheet-verdict-{name}").into()))
            .flex()
            .items_center()
            .gap(theme.spacing.lg)
            .w_full()
            .pt(px(9.))
            .pr(px(10.))
            .pb(px(9.))
            .pl(theme.spacing.lg)
            .rounded(px(8.))
            .border_1()
            .border_color(if on { c.accent_ring } else { c.line })
            .when(on, |this| this.bg(c.accent_soft))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.set_verdict(verdict, cx)))
            .child(div().flex_none().size(px(16.)).rounded_full().map(|this| {
                if on {
                    this.border(px(5.)).border_color(c.accent).bg(c.accent_fg)
                } else {
                    this.border(px(1.5)).border_color(c.line2)
                }
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.spacing.sm)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(icon(glyph, px(14.), color))
                            .child(verdict_label(verdict)),
                    )
                    .child(
                        div()
                            .text_spec(&theme.text.secondary)
                            .text_color(c.fg2)
                            .child(description),
                    ),
            )
            .child(kbd(key.to_string(), theme))
            .automation_target(format!("sheet/verdict/{name}"))
            .into_any_element()
    }

    /// The round's comments: the threads opened in it and those of earlier rounds that aren't
    /// resolved yet (the agent gets both with the round), each with its state, `path:line`, its
    /// round when it is an earlier one, and its first line.
    fn render_comments(&self, theme: &DvTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let session = self.session.read(cx);
        let review = session.review();
        let round = review.current_round();
        let threads: Vec<_> = review
            .live_threads()
            .filter(|thread| thread.anchor.round == round || thread.state != ThreadState::Resolved)
            .collect();
        let new = threads
            .iter()
            .filter(|thread| thread.anchor.round == round)
            .count();
        let earlier = threads.len() - new;
        let rows: Vec<AnyElement> = threads
            .iter()
            .enumerate()
            .map(|(ix, thread)| {
                let (glyph, color) = match thread.state {
                    ThreadState::Open => (DvIcon::MessageSquare, c.accent_text),
                    ThreadState::Addressed => (DvIcon::Check, c.blue),
                    ThreadState::Resolved => (DvIcon::CircleCheck, c.green),
                };
                let location = location_label(thread);
                let (path, line) = match location.rsplit_once(':') {
                    Some((path, line)) => (path.to_string(), format!(":{line}")),
                    None => (location.clone(), String::new()),
                };
                let from = (thread.anchor.round != round).then(|| {
                    div()
                        .flex_none()
                        .ml_auto()
                        .font_family(theme.fonts.ui.clone())
                        .text_size(px(11.))
                        .text_color(c.fg3)
                        .child(format!("round {}", thread.anchor.round))
                });
                div()
                    .flex()
                    .gap(px(10.))
                    .px(theme.spacing.lg)
                    .py(px(10.))
                    .when(ix > 0, |this| this.border_t_1().border_color(c.line))
                    .child(div().pt(px(2.)).child(icon(glyph, px(14.), color)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .font_family(theme.fonts.mono.clone())
                                    .text_size(px(12.))
                                    .line_height(px(16.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(c.fg2)
                                    // At its start, so the file's name stays.
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis_start()
                                            .text_right()
                                            .child(path),
                                    )
                                    .child(div().flex_none().text_color(c.fg3).child(line))
                                    .children(from),
                            )
                            .child(
                                div()
                                    .mt(px(3.))
                                    .line_height(px(19.))
                                    .child(first_line(&thread.body)),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        let list: AnyElement = if rows.is_empty() {
            div()
                .py(theme.spacing.xl)
                .flex()
                .justify_center()
                .text_color(c.fg3)
                .child("No comments")
                .into_any_element()
        } else {
            div()
                .id("sheet-comments")
                .max_h(px(COMMENTS_HEIGHT))
                .overflow_y_scroll()
                .rounded(px(8.))
                .border_1()
                .border_color(c.line2)
                .children(rows)
                .into_any_element()
        };
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .pt(theme.spacing.xl)
            .px(px(20.))
            .pb(px(18.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.md)
                    .mb(theme.spacing.xs)
                    .child(div().text_spec(&theme.text.title).child("Comments"))
                    .child(count_badge(theme).child(threads.len().to_string())),
            )
            .child(
                div()
                    .mb(px(10.))
                    .text_spec(&theme.text.secondary)
                    .text_color(c.fg2)
                    .child(if earlier > 0 {
                        format!("{new} new in this round · {earlier} from earlier rounds")
                    } else {
                        "New in this round; the agent gets them with it".to_string()
                    }),
            )
            .child(list)
            .into_any_element()
    }

    fn render_footer(&self, theme: &DvTheme, cx: &mut Context<Self>) -> AnyElement {
        let c = &theme.colors;
        let session = self.session.read(cx);
        let reviewer = session.reviewer();
        let round = session.review().current_round();
        let can_submit = self.can_submit(cx);
        let avatar = div()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .size(px(22.))
            .rounded_full()
            .bg(if reviewer.kind == AuthorKind::Reviewer {
                c.accent_soft
            } else {
                c.layer_strong
            })
            .text_color(c.accent_text)
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .child(crate::thread::initial(&reviewer.name));
        let label = if self.submitting {
            "Finishing…".to_string()
        } else {
            verdict_label(self.verdict).to_string()
        };
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.md)
            .pt(theme.spacing.lg)
            .pr(px(14.))
            .pb(theme.spacing.lg)
            .pl(px(20.))
            .border_t_1()
            .border_color(c.line)
            .child(avatar)
            .child(
                div()
                    .flex_none()
                    .text_color(c.fg2)
                    .child(format!("Finishing round {round} as ")),
            )
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .child(reviewer.name.clone()),
            )
            .child(div().flex_1())
            .children(self.error.as_ref().map(|error| {
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.sm)
                    .text_spec(&theme.text.secondary)
                    .text_color(c.red)
                    .child(icon(DvIcon::TriangleAlert, px(13.), c.red))
                    .child(div().min_w_0().truncate().child(error.message.clone()))
                    .automation_target("sheet/error")
            }))
            .child(
                button("sheet-cancel", theme)
                    .child("Cancel")
                    .map(|this| {
                        if self.submitting {
                            this.opacity(0.5).cursor_default()
                        } else {
                            this.on_click(cx.listener(|this, _, _, cx| this.dismiss(cx)))
                        }
                    })
                    .automation_target("sheet/cancel"),
            )
            .child(
                primary_button("sheet-submit", can_submit, theme)
                    .child(label)
                    .child(kbd_on_accent(keymap::label("secondary-enter"), theme))
                    .when(can_submit, |this| {
                        this.on_click(cx.listener(|this, _, _, cx| this.submit(cx)))
                    })
                    .automation_target("sheet/submit"),
            )
            .into_any_element()
    }
}
