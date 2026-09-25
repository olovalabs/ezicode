use gpui::{div, prelude::*, px, rgba, AnyElement, Div, FontWeight, SharedString};

use crate::extensions::{self, InstalledExtension};
use crate::theme::Colors;
use crate::ui::common::{mock_input, panel_header, section_strip};

pub(crate) fn render_extensions_panel(t: &Colors) -> AnyElement {
    let installed = extensions::installed();
    let theme_count: usize = installed.iter().map(|e| e.themes.len()).sum();

    let body = if installed.is_empty() {
        empty_state(t)
    } else {
        extension_list(&installed, t)
    };

    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(rgba(t.panel))
        .overflow_hidden()
        .child(panel_header("EXTENSIONS", t))
        .child(
            div()
                .px(px(12.0))
                .py(px(8.0))
                .child(mock_input("Search Extensions in Marketplace", 30.0, t)),
        )
        .child(section_strip(
            &format!("INSTALLED ({})", installed.len()),
            t,
        ))
        .child(body)
        .child(footer(installed.len(), theme_count, t))
        .into_any_element()
}

fn extension_list(items: &[InstalledExtension], t: &Colors) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .overflow_hidden()
        .children(items.iter().map(|item| extension_item(item, t)))
        .into_any_element()
}

fn extension_item(item: &InstalledExtension, t: &Colors) -> Div {
    let mut tags = div().flex().flex_wrap().gap(px(4.0)).pt(px(2.0));

    if !item.themes.is_empty() {
        tags = tags.child(tag(
            format!("{} themes", item.themes.len()),
            t.icon_accent,
            t,
        ));
    }
    if item.declarative_only {
        tags = tags.child(tag("no runtime".to_string(), t.text_muted, t));
    } else {
        tags = tags.child(tag("extension host".to_string(), t.text_muted, t));
    }
    if item.compatibility == "partial" {
        tags = tags.child(tag("partial support".to_string(), t.vc_modified, t));
    }

    div()
        .p(px(10.0))
        .flex()
        .flex_col()
        .gap(px(3.0))
        .border_b_1()
        .border_color(rgba(t.border_variant))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(t.ghost_hover)))
        .child(title_row(&item.display_name, item.enabled, t))
        .child(meta_line(item.subtitle(), t))
        .when(!item.description.is_empty(), |row| {
            row.child(meta_line(item.description.clone(), t))
        })
        .child(tags)
}

fn title_row(name: &str, enabled: bool, t: &Colors) -> Div {
    let (label, fg, bg) = if enabled {
        ("Enabled", t.text_muted, t.element_active)
    } else {
        ("Disabled", t.background, t.border_focused)
    };

    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(6.0))
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .text_size(px(13.5))
                .font_weight(FontWeight::BOLD)
                .text_color(rgba(t.text))
                .child(SharedString::from(name.to_string())),
        )
        .child(
            div()
                .flex_none()
                .px(px(8.0))
                .py(px(2.0))
                .rounded(px(3.0))
                .bg(rgba(bg))
                .text_size(px(11.0))
                .text_color(rgba(fg))
                .child(SharedString::from(label)),
        )
}

fn tag(text: String, color: u32, t: &Colors) -> Div {
    div()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(px(3.0))
        .bg(rgba(t.element_bg))
        .text_size(px(10.5))
        .text_color(rgba(color))
        .child(SharedString::from(text))
}

fn meta_line(text: String, t: &Colors) -> Div {
    div()
        .text_size(px(12.0))
        .text_color(rgba(t.text_muted))
        .child(SharedString::from(text))
}

/// Shown on a fresh install — tells the user exactly how to get extensions.
fn empty_state(t: &Colors) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .px(px(12.0))
        .py(px(14.0))
        .child(
            div()
                .text_size(px(13.0))
                .text_color(rgba(t.text))
                .child(SharedString::from("No extensions installed yet.")),
        )
        .child(meta_line(
            "Install any VS Code-compatible extension from the Open VSX registry:".to_string(),
            t,
        ))
        .child(command_hint("ezicode-ext install dracula-theme.theme-dracula", t))
        .child(meta_line("Building your own? Point ezicode at a folder:".to_string(), t))
        .child(command_hint("ezicode-ext link ./my-extension", t))
        .into_any_element()
}

fn command_hint(command: &'static str, t: &Colors) -> Div {
    div()
        .px(px(8.0))
        .py(px(6.0))
        .rounded(px(4.0))
        .bg(rgba(t.element_bg))
        .border_1()
        .border_color(rgba(t.border))
        .text_size(px(11.5))
        .text_color(rgba(t.text_accent))
        .child(SharedString::from(command))
}

fn footer(count: usize, theme_count: usize, t: &Colors) -> Div {
    let summary = if count == 0 {
        extensions::extensions_dir().display().to_string()
    } else {
        format!("{count} installed · {theme_count} themes contributed")
    };

    div()
        .flex_none()
        .px(px(12.0))
        .py(px(6.0))
        .border_t_1()
        .border_color(rgba(t.border_variant))
        .text_size(px(11.0))
        .text_color(rgba(t.text_muted))
        .child(SharedString::from(summary))
}
