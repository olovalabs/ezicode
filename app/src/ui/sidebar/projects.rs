//! The project switcher is a thin view over the workspace's real session
//! store. This app has one active project root per window; the current root is
//! shown separately from folders previously opened in this window/session.

use gpui::{
    div, ease_in_out, prelude::*, px, rgba, svg, Animation, AnimationExt, Context, ElementId,
    FontWeight, IntoElement, SharedString,
};
use gpui_component::{scroll::ScrollableElement as _, tooltip::Tooltip};

use crate::fs_tree::display_name;
use crate::theme::Colors;
use crate::ui::common::section_strip;
use crate::workspace::Workspace;

/// Paints the outside-click target and the sliding panel. Both are absolute
/// children of the main workspace row, so showing the switcher never changes
/// the editor/sidebar layout or causes a horizontal relayout.
pub(crate) fn render_overlay(
    workspace: &Workspace,
    closing: bool,
    panel_width: f32,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let backdrop = div()
        .id("project-switcher-backdrop")
        .absolute()
        .top_0()
        .bottom_0()
        .left(px(50.0))
        .right_0()
        .occlude()
        .on_click(cx.listener(|this, _, window, cx| {
            cx.stop_propagation();
            this.close_project_switcher(cx);
            this.focus_active_editor_or_self(window, cx);
        }));

    let panel = render_panel(workspace, closing, panel_width, t, cx);

    div()
        .id("project-switcher-overlay")
        .absolute()
        .top_0()
        .bottom_0()
        .left_0()
        .right_0()
        .child(backdrop)
        .child(panel)
        .into_any_element()
}

fn render_panel(
    workspace: &Workspace,
    closing: bool,
    panel_width: f32,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let current = workspace.root.as_ref();
    let paths = workspace.project_switcher_paths();
    let recent: Vec<_> = paths
        .into_iter()
        .filter(|path| current != Some(path))
        .collect();
    let selected = workspace.project_switcher_selection.as_ref();
    let scroll_handle = workspace.project_switcher_scroll_handle.clone();

    let mut contents = div()
        .id("project-switcher-list")
        .flex_1()
        .min_h(px(0.0))
        .track_scroll(&scroll_handle)
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .gap(px(2.0))
        .px(px(8.0))
        .pb(px(8.0));

    if let Some(current) = current {
        contents = contents
            .child(section_strip("CURRENT PROJECT", t))
            .child(project_row(
                current,
                workspace.root.as_ref() == Some(current),
                false,
                selected == Some(current),
                1,
                t,
                cx,
            ));
    }

    contents = contents.child(section_strip("RECENT PROJECTS", t));
    if recent.is_empty() {
        contents = contents.child(
            div()
                .w_full()
                .px(px(10.0))
                .py(px(12.0))
                .text_size(px(12.0))
                .text_color(rgba(t.text_muted))
                .child(SharedString::from(if current.is_some() {
                    "No other recent projects".to_string()
                } else {
                    "No recent projects yet".to_string()
                })),
        );
    } else {
        let first_item_index = if current.is_some() { 3 } else { 1 };
        contents = contents.children(recent.iter().enumerate().map(|(index, path)| {
            let row_index = first_item_index + index;
            project_row(
                path,
                false,
                workspace.loading_root.as_ref() == Some(path),
                selected == Some(path),
                row_index,
                t,
                cx,
            )
        }));
    }

    let scroll_area = div()
        .id("project-switcher-scroll-area")
        .relative()
        .flex_1()
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(contents)
        .vertical_scrollbar(&scroll_handle);

    let open_folder = div()
        .id("project-switcher-open-folder")
        .w_full()
        .h(px(34.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(rgba(t.border))
        .bg(rgba(t.element_bg))
        .text_size(px(12.0))
        .text_color(rgba(t.text))
        .cursor_pointer()
        .hover(|style| style.bg(rgba(t.element_hover)))
        .child(
            svg()
                .path("ui_icons/add_tint.svg")
                .w(px(15.0))
                .h(px(15.0))
                .text_color(rgba(t.icon_muted)),
        )
        .child(SharedString::from("Open Folder…"))
        .on_click(cx.listener(|this, _, window, cx| {
            cx.stop_propagation();
            this.close_project_switcher(cx);
            this.focus_active_editor_or_self(window, cx);
            this.open_folder_dialog(window, cx);
        }));

    div()
        .id("project-switcher-panel")
        .absolute()
        .top_0()
        .bottom_0()
        .w(px(panel_width))
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(rgba(t.panel))
        .border_r_1()
        .border_color(rgba(t.border_variant))
        .key_context("ProjectSwitcher")
        .track_focus(&workspace.project_switcher_focus_handle)
        .child(
            div()
                .h(px(42.0))
                .flex_shrink_0()
                .px(px(14.0))
                .flex()
                .items_center()
                .justify_between()
                .border_b_1()
                .border_color(rgba(t.border_variant))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_size(px(12.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgba(t.text_muted))
                        .child(SharedString::from("PROJECTS")),
                )
                .child(close_button(t, cx)),
        )
        .child(scroll_area)
        .child(
            div()
                .flex_shrink_0()
                .px(px(10.0))
                .py(px(9.0))
                .border_t_1()
                .border_color(rgba(t.border_variant))
                .child(open_folder),
        )
        .with_animation(
            ElementId::NamedInteger("project-switcher-slide".into(), closing as u64),
            Animation::new(crate::workspace::PROJECT_SWITCHER_ANIMATION_DURATION)
                .with_easing(ease_in_out),
            move |panel, delta| {
                let left = if closing {
                    px(50.0) - delta * px(panel_width)
                } else {
                    px(50.0 - panel_width) + delta * px(panel_width)
                };
                panel.left(left)
            },
        )
}

fn close_button(t: &Colors, cx: &mut Context<Workspace>) -> impl IntoElement {
    div()
        .id("project-switcher-close")
        .size(px(28.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .cursor_pointer()
        .hover(|style| style.bg(rgba(t.ghost_hover)))
        .child(
            svg()
                .path("ui_icons/panel_close.svg")
                .w(px(16.0))
                .h(px(16.0))
                .text_color(rgba(t.icon_muted)),
        )
        .tooltip(|window, cx| Tooltip::new("Close project switcher").build(window, cx))
        .on_click(cx.listener(|this, _, window, cx| {
            cx.stop_propagation();
            this.close_project_switcher(cx);
            this.focus_active_editor_or_self(window, cx);
        }))
}

fn project_row(
    path: &std::path::Path,
    active: bool,
    opening: bool,
    selected: bool,
    row_index: usize,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let label = display_name(path);
    let full_path = path.to_string_lossy().into_owned();
    let click_path = path.to_path_buf();
    let background = if active {
        t.element_selected
    } else if selected {
        t.ghost_hover
    } else {
        0x00000000
    };
    let border = if active {
        t.border_focused
    } else if selected {
        t.border_variant
    } else {
        0x00000000
    };
    let status = if opening {
        Some("Opening…")
    } else if active {
        Some("Active")
    } else {
        None
    };

    div()
        .id(SharedString::from(format!("project-switcher-row-{row_index}")))
        .w_full()
        .h(px(50.0))
        .min_w(px(0.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(rgba(border))
        .bg(rgba(background))
        .cursor_pointer()
        .when(!active && !selected, |row| {
            row.hover(|style| style.bg(rgba(t.ghost_hover)))
        })
        .child(
            svg()
                .path("ui_icons/folder_tint.svg")
                .w(px(16.0))
                .h(px(16.0))
                .text_color(rgba(t.icon_muted)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .justify_center()
                .gap(px(2.0))
                .child(
                    div()
                        .w_full()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgba(t.text))
                        .child(SharedString::from(label)),
                )
                .child(
                    div()
                        .w_full()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(px(10.5))
                        .text_color(rgba(t.text_muted))
                        .child(SharedString::from(full_path.clone())),
                ),
        )
        .when_some(status, |row, status| {
            row.child(
                div()
                    .flex_none()
                    .text_size(px(10.0))
                    .text_color(rgba(if opening {
                        t.text_accent
                    } else {
                        t.text_muted
                    }))
                    .child(SharedString::from(status)),
            )
        })
        .tooltip(move |window, cx| Tooltip::new(full_path.clone()).build(window, cx))
        .on_click(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            this.select_project_switcher_project(click_path.clone(), window, cx);
        }))
}
