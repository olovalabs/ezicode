use gpui::{div, img, prelude::*, px, rgba, Div, FontWeight, IntoElement, Length, SharedString};

use crate::theme::Colors;
use crate::ui::scale::rem;

/// An icon drawn from an SVG asset at `size`.
///
/// Callers hand over a length rather than a number so they can say which kind
/// of size they mean: `px(15.)` for one that is fixed (the tab strip's file
/// icons), `rem(15.)` for one that follows `ui_font_size` the way the file
/// tree's do — an icon that is 16 design pixels is 24 on screen when the UI is
/// scaled up to 24.
pub(crate) fn icon_img(path: &'static str, size: impl Into<Length>) -> impl IntoElement {
    let size = size.into();
    div()
        .flex_none()
        .w(size)
        .h(size)
        .child(img(path).w(size).h(size))
}

/// Bold small-caps header row at the top of a sidebar panel.
pub(crate) fn panel_header(label: &'static str, t: &Colors) -> Div {
    div()
        .h(rem(36.0))
        .px(rem(14.0))
        .flex()
        .items_center()
        .text_size(rem(12.0))
        .font_weight(FontWeight::BOLD)
        .text_color(rgba(t.text_muted))
        .child(SharedString::from(label))
}

/// A text field that only pretends to be one (search boxes that are not wired
/// up yet). Its height is a design-pixel value too, so the row never clips the
/// label inside it.
pub(crate) fn mock_input(text: &'static str, h: f32, t: &Colors) -> Div {
    div()
        .h(rem(h))
        .px(rem(10.0))
        .flex()
        .items_center()
        .bg(rgba(t.element_bg))
        .border_1()
        .border_color(rgba(t.border))
        .rounded(px(4.0))
        .text_size(rem(13.0))
        .text_color(rgba(t.text_muted))
        .child(SharedString::from(text))
}

/// Collapsible section title strip ("CHANGES", "INSTALLED", …).
pub(crate) fn section_strip(label: &str, t: &Colors) -> Div {
    div()
        .h(rem(26.0))
        .px(rem(12.0))
        .flex()
        .items_center()
        .justify_between()
        .bg(rgba(t.surface))
        .text_size(rem(12.0))
        .font_weight(FontWeight::BOLD)
        .text_color(rgba(t.text))
        .child(SharedString::from(label.to_string()))
}
