use std::path::PathBuf;

use gpui::prelude::*;
use gpui::{div, img, px, rgba, AnyElement, FontWeight, IntoElement, Styled};
use gpui_component::IconName;

use crate::theme::Colors;

fn logo_path() -> Option<PathBuf> {

    let candidates = [
        "assets/logo/ezicode.png",
        "app/assets/logo/ezicode.png",
        "ezicode.png",
        "logo.png",
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

pub fn render_app_icon(size: f32, t: &Colors) -> AnyElement {
    if let Some(path) = logo_path() {
        return img(path)
            .w(px(size))
            .h(px(size))
            .into_any_element();
    }

    if crate::assets::AppAssets::get("logo/ezicode.png").is_some() {
        return img("logo/ezicode.png")
            .w(px(size))
            .h(px(size))
            .into_any_element();
    }

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

#[allow(dead_code)]
pub fn app_icon_name() -> IconName {
    IconName::Star
}
