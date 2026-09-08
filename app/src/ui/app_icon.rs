//! App icon (ezicode logo) rendering utilities.
//!
//! Renders the embedded logo wherever the app needs branding — welcome
//! screen, title bar, about dialog, etc.

use std::path::PathBuf;

use gpui::prelude::*;
use gpui::{div, img, px, rgba, AnyElement, FontWeight, IntoElement, Styled};
use gpui_component::IconName;

use crate::theme::Colors;

/// Get the path to the app logo PNG (if it exists on disk).
/// We use a path-based image so the GPUI image cache can handle it.
fn logo_path() -> Option<PathBuf> {
    // Check a few common locations
    let candidates = [
        "assets/logo/ezicode.png",
        "app/assets/logo/ezicode.png",
        "assets/logo/olova.png",
        "app/assets/logo/olova.png",
    ];
    for c in &candidates {
        let p = PathBuf::from(c);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Render the app icon at a specific size.
///
/// Uses the embedded logo if available. If not, falls back to a
/// colored circle with a stylized "E" — still recognizable as branding.
pub fn render_app_icon(size: f32, t: &Colors) -> AnyElement {
    if let Some(path) = logo_path() {
        return img(path)
            .w(px(size))
            .h(px(size))
            .into_any_element();
    }

    // Fallback: colored circle with "E"
    div()
        .w(px(size))
        .h(px(size))
        .rounded(px(size / 2.0))
        .bg(rgba(t.text_accent))
        .flex()
        .items_center()
        .justify_center()
        .text_color(rgba(t.background))
        .font_weight(FontWeight::BOLD)
        .text_size(px(size * 0.55))
        .child("E")
        .into_any_element()
}

/// Render the app icon next to a title — for use in headers/badges.
#[allow(dead_code)]
pub fn render_app_icon_with_label(
    size: f32,
    title: &str,
    t: &Colors,
) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(render_app_icon(size, t))
        .child(
            div()
                .text_size(px(size * 0.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgba(t.text))
                .child(title.to_string()),
        )
        .into_any_element()
}

/// Just the icon glyph name (for menu items / buttons).
/// This makes the app identifiable in dropdowns.
#[allow(dead_code)]
pub fn app_icon_name() -> IconName {
    IconName::Star  // Placeholder; will use a custom ezicode glyph if added
}
