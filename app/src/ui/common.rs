use gpui::{div, img, prelude::*, px, rgba, Div, FontWeight, IntoElement, SharedString};

use crate::theme::Colors;

pub(crate) fn icon_img(path: &'static str, size: f32) -> impl IntoElement {
    div()
        .flex_none()
        .w(px(size))
        .h(px(size))
        .child(img(path).w(px(size)).h(px(size)))
}

/// Bold small-caps header row at the top of a sidebar panel.
pub(crate) fn panel_header(label: &'static str, t: &Colors) -> Div {
    div()
        .h(px(35.0))
        .px(px(16.0))
        .flex()
        .items_center()
        .text_size(px(11.0))
        .font_weight(FontWeight::BOLD)
        .text_color(rgba(t.text_muted))
        .child(SharedString::from(label))
}

pub(crate) fn mock_input(text: &'static str, h: f32, t: &Colors) -> Div {
    div()
        .h(px(h))
        .px(px(8.0))
        .flex()
        .items_center()
        .bg(rgba(t.element_bg))
        .border_1()
        .border_color(rgba(t.border))
        .rounded(px(4.0))
        .text_size(px(12.0))
        .text_color(rgba(t.text_muted))
        .child(SharedString::from(text))
}

/// Collapsible section title strip ("CHANGES", "INSTALLED", …).
pub(crate) fn section_strip(label: &str, t: &Colors) -> Div {
    div()
        .h(px(22.0))
        .px(px(12.0))
        .flex()
        .items_center()
        .justify_between()
        .bg(rgba(t.surface))
        .text_size(px(11.0))
        .font_weight(FontWeight::BOLD)
        .text_color(rgba(t.text))
        .child(SharedString::from(label.to_string()))
}
