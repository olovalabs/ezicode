use gpui::{
    div, prelude::*, px, rgba, App, Context, FontWeight, IntoElement, MouseButton, SharedString,
    Window,
};

use crate::theme::Colors;
use crate::ui::app_icon;
use crate::workspace::Workspace;

pub(crate) fn render_welcome(t: &Colors, cx: &mut Context<Workspace>) -> impl IntoElement {
    div()
        .flex_1()
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.0))
        .bg(rgba(t.editor_bg))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, _| {
                window.focus(&this.focus_handle);
            }),
        )
        .child(app_icon::render_app_icon(96.0, t))
        .child(div().h(px(16.0)))
        .child(welcome_button(
            "Quick Open File (Ctrl+P)",
            true,
            Some((0x222222ff, 0x2f2f2fff)),
            t,
            cx.listener(|this, _, window, cx| this.toggle_file_finder(window, cx)),
        ))
}

fn welcome_button(
    label: &'static str,
    primary: bool,
    bg_override: Option<(u32, u32)>,
    t: &Colors,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("wb-{label}")))
        .w(px(220.0))
        .h(px(34.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .cursor_pointer()
        .text_size(px(13.0))
        .when_some(bg_override, |d, (bg, hover)| {
            d.bg(rgba(bg))
                .border_1()
                .border_color(rgba(0x3e3e3eff))
                .text_color(rgba(0xffffffff))
                .font_weight(FontWeight::MEDIUM)
                .hover(move |s| s.bg(rgba(hover)).border_color(rgba(0x4e4e4eff)))
        })
        .when(bg_override.is_none() && primary, |d| {
            d.bg(rgba(t.border_focused))
                .text_color(rgba(t.background))
                .hover(|s| s.bg(rgba(t.icon_accent)))
        })
        .when(bg_override.is_none() && !primary, |d| {
            d.bg(rgba(t.element_bg))
                .border_1()
                .border_color(rgba(t.border))
                .text_color(rgba(t.text))
                .hover(|s| s.bg(rgba(t.element_hover)))
        })
        .child(SharedString::from(label))
        .on_click(on_click)
}

pub(crate) fn render_no_folder_panel(
    t: &Colors,
    workspace: &Workspace,
    cx: &mut Context<Workspace>,
) -> gpui::AnyElement {
    // Read via the borrowed workspace: `cx.entity().read(cx)` would re-lease
    // the Workspace while `Render::render` already holds it and panic.
    let global_state = workspace.storage.recent();
    let recent_folders: Vec<_> = global_state.recent_folders.into_iter().collect();

    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(10.0))
        .pt(px(48.0))
        .bg(rgba(t.panel))
        .child(super::common::panel_header("EXPLORER", t))
        .child(
            div()
                .px(px(12.0))
                .text_size(px(13.0))
                .text_color(rgba(t.text_muted))
                .child(SharedString::from("No folder opened")),
        )
        .child(welcome_button(
            "Open Folder",
            true,
            None,
            t,
            cx.listener(|this, _, window, cx| this.open_folder_dialog(window, cx)),
        ))
        .when(!recent_folders.is_empty(), |el| {
            el.child(
                div()
                    .w_full()
                    .px(px(16.0))
                    .pt(px(16.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgba(t.text_muted))
                            .child(SharedString::from("RECENT")),
                    )
                    .children(recent_folders.into_iter().take(4).map(|path| {
                        let path_clone = path.clone();
                        let folder_name = path
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("Folder")
                            .to_string();
                        let display_path = path.to_string_lossy().to_string();

                        div()
                            .id(SharedString::from(format!(
                                "sidebar-recent-{}",
                                display_path
                            )))
                            .w_full()
                            .flex()
                            .flex_col()
                            .px(px(6.0))
                            .py(px(4.0))
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgba(t.element_hover)))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.load_root(path_clone.clone(), cx);
                            }))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgba(t.text))
                                    .child(SharedString::from(folder_name)),
                            )
                            .child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(rgba(t.text_muted))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(SharedString::from(display_path)),
                            )
                    })),
            )
        })
        .into_any_element()
}
