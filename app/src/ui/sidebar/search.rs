use gpui::{div, prelude::*, px, rgba, AnyElement, Div, SharedString};

use crate::theme::Colors;
use crate::ui::common::{mock_input, panel_header};

pub(crate) fn render_search_panel(t: &Colors) -> AnyElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(rgba(t.panel))
        .child(panel_header("SEARCH", t))
        .child(
            div()
                .px(px(12.0))
                .py(px(8.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(mock_input("Search (files, symbols)", 30.0, t))
                .child(mock_input("Replace", 30.0, t))
                .child(filter_badges(t)),
        )
        .child(
            div()
                .px(px(16.0))
                .pt(px(16.0))
                .text_size(px(13.0))
                .text_color(rgba(t.text_muted))
                .child(SharedString::from(
                    "Press Ctrl+F inside the editor to search active file",
                )),
        )
        .into_any_element()
}

fn filter_badges(t: &Colors) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(5.0))
        .child(filter_badge("Aa", t))
        .child(filter_badge("Ab", t))
        .child(filter_badge(".*", t))
}

fn filter_badge(label: &'static str, t: &Colors) -> impl IntoElement {
    div()
        .px(px(8.0))
        .py(px(3.0))
        .rounded(px(3.0))
        .bg(rgba(t.element_active))
        .hover(|s| s.bg(rgba(t.border_focused)))
        .cursor_pointer()
        .text_size(px(12.0))
        .text_color(rgba(t.text))
        .child(SharedString::from(label))
}
