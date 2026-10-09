//! The inline error banner (DESIGN §4.4): drawn at the top of the region a failure affects,
//! never over another region. An icon, the failure's one-line headline (never raw stderr), the
//! region's fix actions (Retry, …), the failed command and its output in a disclosure, and
//! "Copy diagnostics", which goes through the clipboard seam (`crate::system`).
//!
//! The owner keeps whether the disclosure is open and hands it in with a toggle, so the banner
//! itself holds no state. Automation targets hang off the banner's own name: `<name>`,
//! `<name>/details`, `<name>/copy` and `<name>/<action key>`.

use std::rc::Rc;

use gpui::{
    App, ElementId, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};

use crate::automation::AutomationTargetExt as _;
use crate::components::{icon, small_button};
use crate::failure::Failure;
use crate::icons::DvIcon;
use crate::theme::{ActiveTheme as _, StyledText as _};

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// One fix action on a banner.
pub struct BannerAction {
    /// Names its automation target, `<banner>/<key>`.
    pub key: &'static str,
    pub label: SharedString,
    pub handler: Handler,
}

impl BannerAction {
    pub fn new(
        key: &'static str,
        label: impl Into<SharedString>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            key,
            label: label.into(),
            handler: Rc::new(handler),
        }
    }
}

/// The text "Copy diagnostics" writes: the headline, then the detail when there is one.
pub fn diagnostics(failure: &Failure) -> String {
    match &failure.detail {
        Some(detail) => format!("{}\n\n{detail}", failure.message),
        None => failure.message.clone(),
    }
}

/// An error banner (see the module docs).
#[derive(IntoElement)]
pub struct Banner {
    name: SharedString,
    failure: Failure,
    expanded: bool,
    on_toggle: Option<Handler>,
    actions: Vec<BannerAction>,
}

impl Banner {
    /// A banner for `failure`, its automation target `name` (`banner/main`).
    pub fn new(name: impl Into<SharedString>, failure: Failure) -> Self {
        Self {
            name: name.into(),
            failure,
            expanded: false,
            on_toggle: None,
            actions: Vec::new(),
        }
    }

    /// Whether the detail shows, and what flips it. Without a toggle the detail stays hidden.
    pub fn disclosure(
        mut self,
        expanded: bool,
        on_toggle: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.expanded = expanded;
        self.on_toggle = Some(Rc::new(on_toggle));
        self
    }

    pub fn action(mut self, action: BannerAction) -> Self {
        self.actions.push(action);
        self
    }
}

impl RenderOnce for Banner {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.dv_theme().clone();
        let c = &theme.colors;
        let name = self.name.clone();
        let id = |part: &str| ElementId::Name(format!("{name}/{part}").into());
        let has_detail = self.failure.detail.is_some() && self.on_toggle.is_some();
        let expanded = has_detail && self.expanded;
        let diagnostics = diagnostics(&self.failure);
        let toggle = self.on_toggle.clone();
        let header = div()
            .flex()
            .items_center()
            .gap(theme.spacing.md)
            .min_h(px(40.))
            .py(theme.spacing.sm)
            .pl(theme.spacing.lg)
            .pr(theme.spacing.md)
            .child(icon(DvIcon::TriangleAlert, px(15.), c.red))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_spec(&theme.text.body)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c.fg)
                    .child(self.failure.message.clone()),
            )
            .children(self.actions.into_iter().map(|action| {
                let handler = action.handler.clone();
                small_button(id(action.key), action.label, &theme)
                    .on_click(move |_, window, cx| handler(window, cx))
                    .automation_target(format!("{name}/{}", action.key))
            }))
            .when(has_detail, |this| {
                let hover = c.control_hover;
                this.child(
                    div()
                        .id(id("details"))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .h(theme.sizes.control_small_height)
                        .px(theme.spacing.sm)
                        .rounded(theme.radii.control)
                        .text_spec(&theme.text.secondary)
                        .text_color(c.fg2)
                        .cursor_pointer()
                        .hover(move |style| style.bg(hover))
                        .child(icon(
                            if expanded {
                                DvIcon::ChevronDown
                            } else {
                                DvIcon::ChevronRight
                            },
                            px(12.),
                            c.fg2,
                        ))
                        .child("Details")
                        .on_click(move |_, window, cx| {
                            if let Some(toggle) = &toggle {
                                toggle(window, cx);
                            }
                        })
                        .automation_target(format!("{name}/details")),
                )
            })
            .child(
                small_button(id("copy"), "Copy diagnostics", &theme)
                    .on_click(move |_, _, cx| {
                        crate::system::write_clipboard(diagnostics.clone(), cx)
                    })
                    .automation_target(format!("{name}/copy")),
            );
        let detail = self.failure.detail.clone().filter(|_| expanded);
        div()
            .flex_none()
            .flex()
            .flex_col()
            .m(theme.spacing.lg)
            .rounded(theme.radii.control)
            .border_1()
            .border_color(c.red.opacity(0.35))
            .bg(c.red_soft)
            .child(header)
            .children(detail.map(|detail| {
                div().px(theme.spacing.lg).pb(theme.spacing.lg).child(
                    div()
                        .id(id("detail-text"))
                        .max_h(px(180.))
                        .overflow_y_scroll()
                        .p(theme.spacing.md)
                        .rounded(theme.radii.small)
                        .bg(c.recess)
                        .font_family(theme.fonts.mono.clone())
                        .text_size(px(11.5))
                        .line_height(px(16.))
                        .text_color(c.fg2)
                        .child(detail),
                )
            }))
            .automation_target(self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_carry_the_headline_and_the_command() {
        let failure = Failure::with_detail(
            "git diff failed (exit code 128)",
            "$ git diff --raw\nfatal: bad object",
        );
        assert_eq!(
            diagnostics(&failure),
            "git diff failed (exit code 128)\n\n$ git diff --raw\nfatal: bad object"
        );
        assert_eq!(diagnostics(&Failure::new("gone")), "gone");
    }
}
