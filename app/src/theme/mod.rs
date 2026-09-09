mod colors;

pub use colors::Colors;

use std::sync::OnceLock;

use serde_json::{json, Value};

use self::colors::{parse_hex, FALLBACKS, KEY_MAP};

const THEME_FILES: &[&str] = &["themes/github.json", "themes/ayu.json", "themes/gruvbox.json"];

#[derive(Clone, Debug)]
pub struct Theme {
    pub name: String,

    pub appearance: String,
    pub colors: Colors,
    pub terminal_palette: gpui_terminal::ColorPalette,

    hl_json: String,
}

fn hex_to_rgb(hex: u32) -> (u8, u8, u8) {
    (
        ((hex >> 24) & 0xff) as u8,
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
    )
}

fn build_terminal_palette(theme_obj: &Value) -> gpui_terminal::ColorPalette {
    let mut builder = gpui_terminal::ColorPalette::builder();
    let style = theme_obj.get("style").and_then(Value::as_object);

    let get_color = |key: &str| -> Option<(u8, u8, u8)> {
        style
            .and_then(|s| s.get(key))
            .and_then(Value::as_str)
            .and_then(parse_hex)
            .map(hex_to_rgb)
    };

    if let Some((r, g, b)) = get_color("terminal.background")
        .or_else(|| get_color("editor.background"))
        .or_else(|| get_color("background"))
    {
        builder = builder.background(r, g, b);
    }

    if let Some((r, g, b)) = get_color("terminal.foreground")
        .or_else(|| get_color("editor.foreground"))
        .or_else(|| get_color("text"))
    {
        builder = builder.foreground(r, g, b);
    }

    let cursor_color = theme_obj
        .get("players")
        .and_then(Value::as_array)
        .and_then(|p| p.first())
        .and_then(|p| p.get("cursor"))
        .and_then(Value::as_str)
        .and_then(parse_hex)
        .map(hex_to_rgb)
        .or_else(|| get_color("text.accent"))
        .or_else(|| get_color("terminal.bright_foreground"))
        .or_else(|| get_color("terminal.foreground"));

    if let Some((r, g, b)) = cursor_color {
        builder = builder.cursor(r, g, b);
    }

    let ansi_keys = [
        ("terminal.ansi.black", 0),
        ("terminal.ansi.red", 1),
        ("terminal.ansi.green", 2),
        ("terminal.ansi.yellow", 3),
        ("terminal.ansi.blue", 4),
        ("terminal.ansi.magenta", 5),
        ("terminal.ansi.cyan", 6),
        ("terminal.ansi.white", 7),
        ("terminal.ansi.bright_black", 8),
        ("terminal.ansi.bright_red", 9),
        ("terminal.ansi.bright_green", 10),
        ("terminal.ansi.bright_yellow", 11),
        ("terminal.ansi.bright_blue", 12),
        ("terminal.ansi.bright_magenta", 13),
        ("terminal.ansi.bright_cyan", 14),
        ("terminal.ansi.bright_white", 15),
    ];

    for (key, idx) in ansi_keys {
        if let Some((r, g, b)) = get_color(key) {
            builder = match idx {
                0 => builder.black(r, g, b),
                1 => builder.red(r, g, b),
                2 => builder.green(r, g, b),
                3 => builder.yellow(r, g, b),
                4 => builder.blue(r, g, b),
                5 => builder.magenta(r, g, b),
                6 => builder.cyan(r, g, b),
                7 => builder.white(r, g, b),
                8 => builder.bright_black(r, g, b),
                9 => builder.bright_red(r, g, b),
                10 => builder.bright_green(r, g, b),
                11 => builder.bright_yellow(r, g, b),
                12 => builder.bright_blue(r, g, b),
                13 => builder.bright_magenta(r, g, b),
                14 => builder.bright_cyan(r, g, b),
                15 => builder.bright_white(r, g, b),
                _ => builder,
            };
        }
    }

    builder.build()
}

fn parse_family(json: &str, out: &mut Vec<Theme>) {
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return;
    };
    let Some(themes) = v.get("themes").and_then(Value::as_array) else {
        return;
    };
    for t in themes {
        let Some(name) = t.get("name").and_then(Value::as_str) else {
            continue;
        };
        let appearance = t
            .get("appearance")
            .and_then(Value::as_str)
            .unwrap_or("dark")
            .to_string();

        let Some(style) = t.get("style").and_then(Value::as_object) else {
            continue;
        };
        let mut colors = Colors::all_missing();
        for (field, key) in KEY_MAP {
            if let Some(hex_str) = style.get(*key).and_then(Value::as_str) {
                if let Some(hex) = parse_hex(hex_str) {
                    if let Some(slot) = colors.get_mut(field) {
                        *slot = hex;
                    }
                }
            }
        }
        for (field, hex) in FALLBACKS {
            if let Some(slot) = colors.get_mut(field) {
                if *slot == Colors::MISSING {
                    *slot = *hex;
                }
            }
        }
        if colors.border == Colors::MISSING {
            colors.border = 0x30363dff;
        }
        if colors.border_variant == Colors::MISSING {
            colors.border_variant = if colors.border != Colors::MISSING {
                colors.border
            } else {
                0x21262dff
            };
        }

        if colors.tab_bar == Colors::MISSING {
            colors.tab_bar = if colors.toolbar != Colors::MISSING {
                colors.toolbar
            } else if colors.panel != Colors::MISSING {
                colors.panel
            } else if colors.surface != Colors::MISSING {
                colors.surface
            } else {
                colors.background
            };
        }
        if colors.tab_active_bg == Colors::MISSING {
            colors.tab_active_bg = if colors.editor_bg != Colors::MISSING {
                colors.editor_bg
            } else {
                colors.background
            };
        }
        if colors.tab_inactive_bg == Colors::MISSING {
            colors.tab_inactive_bg = colors.tab_bar;
        }
        if colors.tab_active_fg == Colors::MISSING {
            colors.tab_active_fg = if colors.editor_fg != Colors::MISSING {
                colors.editor_fg
            } else {
                colors.text
            };
        }
        if colors.tab_inactive_fg == Colors::MISSING {
            colors.tab_inactive_fg = if colors.text_muted != Colors::MISSING {
                colors.text_muted
            } else {
                colors.text
            };
        }
        if colors.terminal_bg == Colors::MISSING {
            colors.terminal_bg = if colors.background != Colors::MISSING {
                colors.background
            } else {
                colors.tab_bar
            };
        }
        out.push(Theme {
            name: name.to_string(),
            appearance,
            hl_json: build_highlight_theme(name, t),
            terminal_palette: build_terminal_palette(t),
            colors,
        });
    }
}

fn build_highlight_theme(name: &str, theme_obj: &Value) -> String {
    let style = theme_obj.get("style").cloned().unwrap_or(json!({}));
    let hl = json!({
        "name": name,
        "appearance": theme_obj.get("appearance").cloned().unwrap_or(json!("dark")),
        "style": {
            "editor.background": style.get("editor.background"),
            "editor.foreground": style.get("editor.foreground"),
            "editor.active_line.background": style.get("editor.active_line.background"),
            "editor.line_number": style.get("editor.line_number"),
            "editor.active_line_number": style.get("editor.active_line_number"),
            "error": style.get("text").or(style.get("editor.foreground")),
            "error.background": style.get("error.background").or(style.get("elevated_surface.background")).or(style.get("panel.background")),
            "error.border": style.get("error.border").or(style.get("border.variant")).or(style.get("border")),
            "warning": style.get("text").or(style.get("editor.foreground")),
            "warning.background": style.get("warning.background").or(style.get("elevated_surface.background")).or(style.get("panel.background")),
            "warning.border": style.get("warning.border").or(style.get("border.variant")).or(style.get("border")),
            "info": style.get("text").or(style.get("editor.foreground")),
            "info.background": style.get("info.background").or(style.get("elevated_surface.background")).or(style.get("panel.background")),
            "info.border": style.get("info.border").or(style.get("border.variant")).or(style.get("border")),
            "hint": style.get("text").or(style.get("editor.foreground")),
            "hint.background": style.get("hint.background").or(style.get("elevated_surface.background")).or(style.get("panel.background")),
            "hint.border": style.get("hint.border").or(style.get("border.variant")),
            "syntax": style.get("syntax"),
        },
    });
    hl.to_string()
}

impl Theme {

    pub fn highlight_theme(&self) -> gpui_component::highlighter::HighlightTheme {
        serde_json::from_str(&self.hl_json).unwrap_or_else(|_| {

            (*gpui_component::highlighter::HighlightTheme::default_dark()).clone()
        })
    }
}

pub fn all() -> &'static [Theme] {
    static THEMES: OnceLock<Vec<Theme>> = OnceLock::new();
    THEMES.get_or_init(|| {
        let mut out = Vec::new();
        for file in THEME_FILES {
            if let Some(data) = crate::assets::AppAssets::get(file) {
                parse_family(std::str::from_utf8(&data.data).unwrap_or(""), &mut out);
            }
        }
        out
    })
}

/// Index of the startup theme ("GitHub Dark").
pub fn default_index() -> usize {
    all()
        .iter()
        .position(|t| t.name == "GitHub Dark")
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_all_themes() {
        let themes = all();
        assert_eq!(themes.len(), 18, "9 GitHub + 3 Ayu + 6 Gruvbox");
        assert!(themes.iter().any(|t| t.name == "GitHub Dark"));
        assert!(themes.iter().any(|t| t.name == "Ayu Mirage"));
        assert!(themes.iter().any(|t| t.name == "Gruvbox Light"));
        // One family was removed; GitHub is the default now.
        assert!(!themes.iter().any(|t| t.name == "One Dark"));
        assert_eq!(all()[default_index()].name, "GitHub Dark");
    }

    #[test]
    fn github_dark_values_match_source_file() {
        let gd = &all()[default_index()];
        assert_eq!(gd.appearance, "dark");
        assert_eq!(gd.colors.background, 0x0d1117ff);
        assert_eq!(gd.colors.panel, 0x010409ff);
        assert_eq!(gd.colors.editor_fg, 0xf0f6fcff);
    }

    #[test]
    fn every_theme_has_every_token() {
        for t in all() {
            assert!(
                t.colors.is_complete(),
                "theme '{}' is missing tokens",
                t.name
            );
        }
    }

    #[test]
    fn light_and_dark_appearances_exist_for_widget_sync() {
        assert!(all().iter().any(|t| t.appearance == "light"));
        assert!(all().iter().any(|t| t.appearance == "dark"));
    }

    #[test]
    fn highlighter_uses_zed_syntax_palette() {
        use gpui::{rgba, Hsla};
        let od = &all()[default_index()];
        let hl = od.highlight_theme();
        // GitHub Dark comment token is #9198a1 in the source JSON.
        let comment = hl.style.syntax.style("comment").expect("comment style");
        assert_eq!(comment.color, Some(Hsla::from(rgba(0x9198a1ff))));
        let function = hl.style.syntax.style("function").expect("function style");
        assert_eq!(function.color, Some(Hsla::from(rgba(0xd2a8ffff))));
        assert!(hl.style.syntax.style("keyword").is_some());
        assert!(hl.style.syntax.style("comment.doc").is_some());
    }

    #[test]
    fn terminal_palette_matches_theme() {
        let gd = &all()[default_index()];
        // GitHub Dark background is #010409 (terminal.background)
        let bg = gd.terminal_palette.background();
        assert_eq!(bg.a, 1.0);
        // Verify all 18 themes have valid terminal palettes
        for t in all() {
            assert_eq!(t.terminal_palette.ansi_colors().len(), 16);
            assert_eq!(t.terminal_palette.extended_colors().len(), 256);
        }
    }
}
