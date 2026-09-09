use gpui::{div, prelude::*, px, rgba, AnyElement, Div, FontWeight, SharedString};

use crate::theme::Colors;
use crate::ui::common::{mock_input, panel_header, section_strip};

pub(crate) fn render_extensions_panel(t: &Colors) -> AnyElement {
    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(rgba(t.panel))
        .child(panel_header("EXTENSIONS", t))
        .child(
            div()
                .px(px(12.0))
                .py(px(8.0))
                .child(mock_input("Search Extensions in Marketplace", 26.0, t)),
        )
        .child(section_strip("INSTALLED", t))
        .child(
            div()
                .flex()
                .flex_col()
                .child(extension_item(
                    "Rust Analyzer",
                    "rust-lang.rust-analyzer",
                    "Rust language support",
                    true,
                    t,
                ))
                .child(extension_item(
                    "Tree-sitter Syntax",
                    "gpui.treesitter",
                    "High performance syntax coloring",
                    true,
                    t,
                ))
                .child(extension_item(
                    "HTML to GPUI Preview",
                    "ezicode.preview",
                    "Live GPUI element previewer",
                    true,
                    t,
                )),
        )
        .into_any_element()
}

fn extension_item(
    name: &'static str,
    author: &'static str,
    desc: &'static str,
    installed: bool,
    t: &Colors,
) -> impl IntoElement {
    div()
        .p(px(8.0))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .border_b_1()
        .border_color(rgba(t.border_variant))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(t.ghost_hover)))
        .child(title_row(name, installed, t))
        .child(meta_line(author, t))
        .child(meta_line(desc, t))
}

fn title_row(name: &'static str, installed: bool, t: &Colors) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgba(t.text))
                .child(SharedString::from(name)),
        )
        .child(
            div()
                .px(px(6.0))
                .py(px(1.0))
                .rounded(px(3.0))
                .bg(rgba(if installed { t.element_active } else { t.border_focused }))
                .text_size(px(10.0))
                .text_color(rgba(if installed { t.text_muted } else { t.background }))
                .child(SharedString::from(if installed { "Installed" } else { "Install" })),
        )
}

fn meta_line(text: &'static str, t: &Colors) -> Div {
    div()
        .text_size(px(11.0))
        .text_color(rgba(t.text_muted))
        .child(SharedString::from(text))
}
