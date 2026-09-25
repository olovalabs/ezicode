use gpui::{div, prelude::*, px, rgba, AnyElement, Div, FontWeight, SharedString};

use crate::extension::{self, Capabilities, ExtensionSummary};
use crate::theme::Colors;
use crate::ui::common::{mock_input, panel_header, section_strip};

pub(crate) fn render_extensions_panel(t: &Colors) -> AnyElement {
    let installed = extension::summaries();

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
                .child(mock_input("Search Extensions", 30.0, t)),
        )
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(section_strip(
                    &format!("INSTALLED ({})", installed.len()),
                    t,
                ))
                .child(installed_section(installed, t))
                .child(section_strip("BUILT-IN", t))
                .child(builtin_section(t))
                .child(section_strip("ADD AN EXTENSION", t))
                .child(add_section(t)),
        )
        .into_any_element()
}

fn installed_section(installed: &[ExtensionSummary], t: &Colors) -> Div {
    if installed.is_empty() {
        return div().p(px(12.0)).child(
            div()
                .text_size(px(12.5))
                .text_color(rgba(t.text_muted))
                .child(SharedString::from(
                    "No extensions installed yet. Drop a Zed-compatible extension folder into the extensions directory below, or install one from a Git URL.",
                )),
        );
    }

    div()
        .flex()
        .flex_col()
        .children(installed.iter().map(|e| extension_item(e, t)))
}

fn extension_item(e: &ExtensionSummary, t: &Colors) -> Div {
    let author = if e.authors.is_empty() {
        e.id.clone()
    } else {
        format!("{} · {}", e.authors.join(", "), e.id)
    };
    let version = if e.version.is_empty() {
        String::new()
    } else {
        format!("v{}", e.version)
    };

    div()
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap(px(3.0))
        .border_b_1()
        .border_color(rgba(t.border_variant))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(t.ghost_hover)))
        .child(title_row(&e.name, &version, t))
        .child(cap_badges(e.capabilities, t))
        .child(meta_line(&author, t))
        .when(!e.description.is_empty(), |d| {
            d.child(meta_line(&e.description, t))
        })
}

fn title_row(name: &str, version: &str, t: &Colors) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(13.5))
                .font_weight(FontWeight::BOLD)
                .text_color(rgba(t.text))
                .child(SharedString::from(name.to_string())),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(rgba(t.text_muted))
                .child(SharedString::from(version.to_string())),
        )
}

fn cap_badges(caps: Capabilities, t: &Colors) -> Div {
    let mut row = div().flex().flex_wrap().gap(px(4.0)).py(px(2.0));
    let mut any = false;
    for (on, label) in [
        (caps.themes, "Theme"),
        (caps.icon_themes, "Icon Theme"),
        (caps.languages, "Language"),
        (caps.grammars, "Grammar"),
        (caps.language_servers, "LSP"),
        (caps.wasm, "WASM"),
    ] {
        if on {
            any = true;
            row = row.child(badge(label, t));
        }
    }
    if !any {
        row = row.child(badge("Extension", t));
    }
    row
}

fn badge(label: &str, t: &Colors) -> Div {
    div()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(px(3.0))
        .bg(rgba(t.element_active))
        .text_size(px(10.5))
        .text_color(rgba(t.text_muted))
        .child(SharedString::from(label.to_string()))
}

fn builtin_section(t: &Colors) -> Div {
    let theme_count = crate::theme::all().len();
    div()
        .flex()
        .flex_col()
        .child(builtin_item(
            "Zed Theme Engine",
            &format!("{theme_count} themes loaded — extension themes appear here too"),
            t,
        ))
        .child(builtin_item(
            "Tree-sitter Syntax",
            "High-performance semantic highlighting",
            t,
        ))
        .child(builtin_item(
            "Language Server Protocol",
            "Auto-provisioned diagnostics, completion & go-to-definition",
            t,
        ))
}

fn builtin_item(name: &str, desc: &str, t: &Colors) -> Div {
    div()
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .border_b_1()
        .border_color(rgba(t.border_variant))
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgba(t.text))
                .child(SharedString::from(name.to_string())),
        )
        .child(meta_line(desc, t))
}

fn add_section(t: &Colors) -> Div {
    let dir = extension::store::installed_dir().display().to_string();
    div()
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(meta_line(
            "Install a Zed-compatible extension from a Git URL:",
            t,
        ))
        .child(code_line("ezicode --install-extension owner/repo", t))
        .child(meta_line("…or from a local folder:", t))
        .child(code_line("ezicode --install-extension ./path/to/extension", t))
        .child(meta_line("Extensions are stored in:", t))
        .child(code_line(&dir, t))
        .child(meta_line(
            "Restart ezicode after installing to activate new extensions.",
            t,
        ))
}

fn code_line(text: &str, t: &Colors) -> Div {
    div()
        .px(px(8.0))
        .py(px(5.0))
        .rounded(px(4.0))
        .bg(rgba(t.element_bg))
        .border_1()
        .border_color(rgba(t.border))
        .text_size(px(11.5))
        .text_color(rgba(t.text))
        .child(SharedString::from(text.to_string()))
}

fn meta_line(text: &str, t: &Colors) -> Div {
    div()
        .text_size(px(12.0))
        .text_color(rgba(t.text_muted))
        .child(SharedString::from(text.to_string()))
}
