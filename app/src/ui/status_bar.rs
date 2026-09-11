use gpui::{div, prelude::*, px, rgba, svg, FontWeight, IntoElement, SharedString};

use crate::theme::Colors;

#[allow(clippy::too_many_arguments)]
#[derive(Clone, Debug, Default)]
pub(crate) struct LspIndicator {
    pub server: Option<&'static str>,
    pub state: Option<crate::lsp::ServerStatus>,
}

impl LspIndicator {
    /// Glyph + colour + tooltip-ish label for the current state.
    fn parts(&self, t: &Colors) -> (&'static str, u32, String) {
        use crate::lsp::ServerStatus::*;
        match (&self.state, self.server) {
            (Some(Running), Some(name)) => ("●", t.vc_added, name.to_string()),
            (Some(Starting), Some(name)) => ("◐", t.vc_modified, format!("{name}: starting…")),
            (Some(Installing), Some(name)) => ("◌", t.vc_modified, format!("{name}: installing…")),
            (Some(Failed(reason)), _) => ("○", t.vc_deleted, reason.clone()),
            (None, Some(name)) => ("○", t.text_muted, name.to_string()),
            _ => ("○", t.text_muted, "no language server".to_string()),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_status_bar(
    status: &str,
    theme_name: &str,
    git_branch: Option<&str>,
    git_changes: usize,
    lang: Option<&str>,
    lsp: LspIndicator,
    t: &Colors,
) -> impl IntoElement {
    div()
        .h(px(26.0))
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .px(px(10.0))
        .bg(rgba(t.status_bar))
        .border_t_1()
        .border_color(rgba(t.border_variant))
        .text_size(px(12.5))
        .text_color(rgba(t.text))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(SharedString::from(status.to_string())),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .when_some(git_branch, |bar, branch| {
                    bar.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                svg()
                                    .path("ui_icons/git_branch.svg")
                                    .w(px(13.0))
                                    .h(px(13.0))
                                    .text_color(rgba(t.text)),
                            )
                            .child(SharedString::from(branch.to_string()))
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgba(if git_changes > 0 {
                                        t.vc_modified
                                    } else {
                                        t.text_muted
                                    }))
                                    .child(SharedString::from(git_changes.to_string())),
                            ),
                    )
                })
                .when_some(lang, |bar, lang| {
                    let (dot, dot_color, label) = lsp.parts(t);
                    bar.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgba(dot_color))
                                    .child(SharedString::from(dot)),
                            )
                            .child(SharedString::from(lang.to_string()))
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgba(t.text_muted))
                                    .child(SharedString::from(label)),
                            ),
                    )
                })
                .child(
                    div()
                        .id("status-settings-btn")
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .rounded(px(3.0))
                        .px(px(2.0))
                        .py(px(1.0))
                        .hover(|s| s.bg(rgba(t.ghost_hover)))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(crate::actions::OpenSettings), cx);
                        })
                        .child(
                            svg()
                                .path("ui_icons/settings-gear_tint.svg")
                                .w(px(13.0))
                                .h(px(13.0))
                                .text_color(rgba(t.text)),
                        ),
                )
                .child(SharedString::from(theme_name.to_string()))
                .child(SharedString::from("UTF-8"))
                .child(SharedString::from("Spaces: 4")),
        )
}
