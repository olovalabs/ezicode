use gpui::{px, App};
use rust_embed::RustEmbed;

pub const SANS_FONT: &str = "IBM Plex Sans";
pub const MONO_FONT: &str = "Lilex";

#[derive(RustEmbed)]
#[folder = "assets/"]
pub struct AppAssets;

pub struct CombinedAssets;

impl gpui::AssetSource for CombinedAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        let clean = path
            .trim_start_matches("icons/")
            .trim_start_matches("assets/")
            .trim_start_matches('/');
        if let Some(file) = AppAssets::get(clean) {
            return Ok(Some(file.data));
        }
        if let Some(file) = AppAssets::get(path) {
            return Ok(Some(file.data));
        }
        gpui_component_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<gpui::SharedString>> {
        let mut list = Vec::new();
        for file in AppAssets::iter() {
            list.push(gpui::SharedString::from(file.to_string()));
        }
        if let Ok(other) = gpui_component_assets::Assets.list(path) {
            list.extend(other);
        }
        Ok(list)
    }
}

/// Register every embedded TTF under `assets/fonts/` with GPUI's text

pub fn load_embedded_fonts(cx: &App) {
    let fonts: Vec<std::borrow::Cow<'static, [u8]>> = AppAssets::iter()
        .filter(|p| p.starts_with("fonts/") && p.ends_with(".ttf"))
        .filter_map(|p| AppAssets::get(&p).map(|f| f.data))
        .collect();
    let count = fonts.len();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        eprintln!("failed to register embedded fonts: {e}");
    } else {
        println!("registered {count} embedded font files");
    }
}

/// Point the widget library at Zed's fonts (called after every

pub fn sync_component_fonts(cx: &mut App) {
    let theme = gpui_component::Theme::global_mut(cx);
    theme.font_family = SANS_FONT.into();
    theme.mono_font_family = MONO_FONT.into();
    theme.font_size = px(14.0);
    theme.mono_font_size = px(14.5);
}
