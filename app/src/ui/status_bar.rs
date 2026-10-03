use gpui::{div, prelude::*, px, rgba, svg, FontWeight, IntoElement, SharedString};

use crate::theme::Colors;
use crate::ui::scale::rem;

#[derive(Clone, Debug, Default)]
pub(crate) struct LspIndicator {
    pub server: Option<&'static str>,
    pub state: Option<crate::lsp::ServerStatus>,
}

impl LspIndicator {
    /// Glyph + colour + tooltip-ish label for the current state.
    pub(crate) fn parts(&self, t: &Colors) -> (&'static str, u32, String) {
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
    // (ahead, behind) relative to the upstream, when either is non-zero.
    git_sync: Option<(u32, u32)>,
    cursor_pos: Option<(u32, u32)>,
    diagnostic_counts: Option<(usize, usize)>,
    lang: Option<&str>,
    lsp: LspIndicator,
    // Whether the right-hand terminal dock is open (drives the icon's
    // active styling, like Zed's dock toggles in the status bar).
    right_terminal_open: bool,
    t: &Colors,
) -> impl IntoElement {
    let (dot, dot_color, lsp_label) = lsp.parts(t);
    let lang_display = lang.map(crate::lang::language_name).unwrap_or("Plain Text");

    div()
        .h(rem(26.0))
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .px(rem(10.0))
        .bg(rgba(t.status_bar))
        .border_t_1()
        .border_color(rgba(t.border_variant))
        .text_size(rem(12.0))
        .text_color(rgba(t.text))
        .child(
            div()
                .flex()
                .items_center()
                .gap(rem(10.0))
                .min_w_0()
                .overflow_hidden()
                .when_some(git_branch, |bar, branch| {
                    bar.child(
                        div()
                            .id("status-git-branch")
                            .flex()
                            .items_center()
                            .gap(rem(4.0))
                            .px(rem(4.0))
                            .py(rem(1.0))
                            .rounded(px(3.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(t.ghost_hover)))
                            // Zed-style: the status-bar branch opens the
                            // branch switcher, not just the git panel.
                            .on_click(|_, window, cx| {
                                window
                                    .dispatch_action(Box::new(crate::actions::GitBranchPicker), cx);
                            })
                            .child(
                                svg()
                                    .path("ui_icons/git_branch.svg")
                                    .w(rem(13.0))
                                    .h(rem(13.0))
                                    .text_color(rgba(t.text)),
                            )
                            .child(SharedString::from(branch.to_string()))
                            .when_some(git_sync, |parent, (ahead, behind)| {
                                parent.child(
                                    div()
                                        .text_size(rem(10.5))
                                        .text_color(rgba(t.text_muted))
                                        .child(SharedString::from(format!("↑{ahead} ↓{behind}"))),
                                )
                            })
                            .when(git_changes > 0, |parent| {
                                parent.child(
                                    div()
                                        .text_size(rem(10.5))
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgba(t.vc_modified))
                                        .child(SharedString::from(git_changes.to_string())),
                                )
                            }),
                    )
                })
                .when_some(diagnostic_counts, |bar, (errors, warnings)| {
                    bar.child(
                        div()
                            .id("status-diagnostics-btn")
                            .flex()
                            .items_center()
                            .gap(rem(6.0))
                            .px(rem(4.0))
                            .py(rem(1.0))
                            .rounded(px(3.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(t.ghost_hover)))
                            .on_click(|_, window, cx| {
                                window
                                    .dispatch_action(Box::new(crate::actions::NextDiagnostic), cx);
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(rem(2.5))
                                    .text_color(rgba(if errors > 0 {
                                        t.vc_deleted
                                    } else {
                                        t.text_muted
                                    }))
                                    .child("ⓧ")
                                    .child(SharedString::from(errors.to_string())),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(rem(2.5))
                                    .text_color(rgba(if warnings > 0 {
                                        t.vc_modified
                                    } else {
                                        t.text_muted
                                    }))
                                    .child("▲")
                                    .child(SharedString::from(warnings.to_string())),
                            ),
                    )
                })
                .when(!status.is_empty(), |bar| {
                    bar.child(
                        div()
                            .text_color(rgba(t.text_muted))
                            .truncate()
                            .child(SharedString::from(status.to_string())),
                    )
                }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(rem(12.0))
                .flex_shrink_0()
                .when_some(cursor_pos, |bar, (line, col)| {
                    bar.child(
                        div()
                            .id("status-cursor-pos")
                            .px(rem(4.0))
                            .py(rem(1.0))
                            .rounded(px(3.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(t.ghost_hover)))
                            .on_click(|_, window, cx| {
                                window
                                    .dispatch_action(Box::new(crate::actions::ToggleGoToLine), cx);
                            })
                            .child(SharedString::from(format!("Ln {line}, Col {col}"))),
                    )
                })
                .child(SharedString::from("Spaces: 4"))
                .child(SharedString::from("UTF-8"))
                // Language Selector Button
                .child(
                    div()
                        .id("status-language-selector-btn")
                        .flex()
                        .items_center()
                        .gap(rem(5.0))
                        .px(rem(6.0))
                        .py(rem(2.0))
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgba(t.ghost_hover)))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(
                                Box::new(crate::actions::ToggleLanguageSelector),
                                cx,
                            );
                        })
                        .child(
                            div()
                                .text_size(rem(10.5))
                                .text_color(rgba(dot_color))
                                .child(SharedString::from(dot)),
                        )
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .child(SharedString::from(lang_display.to_string())),
                        )
                        .when(lsp.server.is_some(), |parent| {
                            parent.child(
                                div()
                                    .text_size(rem(11.0))
                                    .text_color(rgba(t.text_muted))
                                    .child(SharedString::from(format!("({lsp_label})"))),
                            )
                        }),
                )
                .child(
                    div()
                        .id("status-theme-btn")
                        .px(rem(4.0))
                        .py(rem(1.0))
                        .rounded(px(3.0))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgba(t.ghost_hover)))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(
                                Box::new(crate::actions::ToggleCommandPalette),
                                cx,
                            );
                        })
                        .child(SharedString::from(theme_name.to_string())),
                )
                .child(
                    div()
                        .id("status-settings-btn")
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .rounded(px(3.0))
                        .px(rem(2.0))
                        .py(rem(1.0))
                        .hover(|s| s.bg(rgba(t.ghost_hover)))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(crate::actions::OpenSettings), cx);
                        })
                        .child(
                            svg()
                                .path("ui_icons/settings-gear_tint.svg")
                                .w(rem(13.0))
                                .h(rem(13.0))
                                .text_color(rgba(t.text)),
                        ),
                )
                // Right terminal dock toggle — the rightmost control in the
                // bar, so it sits in the bottom-right corner like Zed's panel
                // toggles. Opens a *separate* terminal panel (own PTY
                // sessions) docked to the right edge of the workspace.
                .child(
                    div()
                        .id("status-terminal-right-btn")
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .rounded(px(3.0))
                        .px(rem(2.0))
                        .py(rem(1.0))
                        .hover(|s| s.bg(rgba(t.ghost_hover)))
                        .on_click(|_, window, cx| {
                            window
                                .dispatch_action(Box::new(crate::actions::ToggleTerminalRight), cx);
                        })
                        .child(
                            svg()
                                .path("ui_icons/terminal_panel_right.svg")
                                .w(rem(14.0))
                                .h(rem(14.0))
                                .text_color(rgba(if right_terminal_open {
                                    t.text
                                } else {
                                    t.text_muted
                                })),
                        ),
                ),
        )
}
