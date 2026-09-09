use gpui::{
    div, prelude::*, px, rgba, App, Context, IntoElement, MouseButton, SharedString, Window,
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
            t,
            cx.listener(|this, _, window, cx| this.toggle_file_finder(window, cx)),
        ))
        .child(welcome_button(
            "Open Folder",
            false,
            t,
            cx.listener(|this, _, window, cx| this.open_folder_dialog(window, cx)),
        ))
        .child(welcome_button(
            "Open File",
            false,
            t,
            cx.listener(|this, _, window, cx| this.open_file_dialog(window, cx)),
        ))
        .child(welcome_button(
            "New File",
            false,
            t,
            cx.listener(|this, _, window, cx| this.new_file(window, cx)),
        ))
}

fn welcome_button(
    label: &'static str,
    primary: bool,
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
        .when(primary, |d| {
            d.bg(rgba(t.border_focused))
                .text_color(rgba(t.background))
                .hover(|s| s.bg(rgba(t.icon_accent)))
        })
        .when(!primary, |d| {
            d.bg(rgba(t.element_bg))
                .border_1()
                .border_color(rgba(t.border))
                .text_color(rgba(t.text))
                .hover(|s| s.bg(rgba(t.element_hover)))
        })
        .child(SharedString::from(label))
        .on_click(on_click)
}

pub(crate) fn render_no_folder_panel(t: &Colors, cx: &mut Context<Workspace>) -> gpui::AnyElement {
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
                .text_size(px(12.0))
                .text_color(rgba(t.text_muted))
                .child(SharedString::from("No folder opened")),
        )
        .child(welcome_button(
            "Open Folder",
            true,
            t,
            cx.listener(|this, _, window, cx| this.open_folder_dialog(window, cx)),
        ))
        .into_any_element()
}
