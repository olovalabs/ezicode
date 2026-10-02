use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutoSaveMode {
    #[serde(
        alias = "off",
        alias = "OFF",
        alias = "false",
        alias = "none",
        alias = "disabled"
    )]
    #[default]
    Off,
    #[serde(
        alias = "afterDelay",
        alias = "after_delay",
        alias = "on",
        alias = "ON",
        alias = "true",
        alias = "delay",
        alias = "auto",
        alias = "enabled"
    )]
    AfterDelay,
    #[serde(
        alias = "onFocusChange",
        alias = "on_focus_change",
        alias = "focusChange",
        alias = "focus"
    )]
    OnFocusChange,
}

impl AutoSaveMode {
    pub fn description(&self) -> &'static str {
        match self {
            AutoSaveMode::Off => "A dirty file is never automatically saved (Ctrl+S to save).",
            AutoSaveMode::AfterDelay => {
                "A dirty file is automatically saved after the configured delay."
            }
            AutoSaveMode::OnFocusChange => {
                "A dirty file is automatically saved when switching tabs or editor focus."
            }
        }
    }
}

/// Whether saving a file first asks its language server to format the buffer
/// — Zed's `format_on_save` setting. The manual command is always available
/// as Shift+Alt+F.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FormatOnSaveMode {
    #[serde(
        alias = "off",
        alias = "OFF",
        alias = "false",
        alias = "none",
        alias = "disabled"
    )]
    #[default]
    Off,
    #[serde(alias = "on", alias = "ON", alias = "true", alias = "enabled")]
    On,
}

impl FormatOnSaveMode {
    pub fn description(&self) -> &'static str {
        match self {
            FormatOnSaveMode::Off => {
                "Saving writes the buffer as-is (Shift+Alt+F formats on demand)."
            }
            FormatOnSaveMode::On => "Saving asks the language server to format the buffer first.",
        }
    }
}

/// The size the interface was designed at, in pixels.
///
/// `ui_font_size` starts here, and every scalable metric in the UI is a
/// fraction of it (see [`crate::ui::scale`]), so leaving the setting at its
/// default reproduces the original layout exactly.
pub const DEFAULT_UI_FONT_SIZE: f32 = 14.0;

/// Smallest `ui_font_size` accepted. The lower bound keeps the interface
/// usable; the pair is the same clamp Zed applies to font sizes
/// (`clamp_font_size` in `zed/crates/theme_settings`).
pub const MIN_UI_FONT_SIZE: f32 = 6.0;

/// Largest `ui_font_size` accepted, and the guard that stops a hand-edited
/// `settings.json` from asking for a 400px interface.
pub const MAX_UI_FONT_SIZE: f32 = 100.0;

/// Clamp a requested UI font size into [`MIN_UI_FONT_SIZE`]..=[`MAX_UI_FONT_SIZE`].
///
/// Out-of-range values are clamped rather than rejected, so editing the number
/// by hand can never wedge the UI. A non-finite value (a `NaN` typed into the
/// JSON, say) falls back to the default instead of poisoning every length it
/// would otherwise be multiplied by.
pub fn clamp_ui_font_size(size: f32) -> f32 {
    if size.is_finite() {
        size.clamp(MIN_UI_FONT_SIZE, MAX_UI_FONT_SIZE)
    } else {
        DEFAULT_UI_FONT_SIZE
    }
}

fn default_font_size() -> f32 {
    14.5
}

fn default_ui_font_size() -> f32 {
    DEFAULT_UI_FONT_SIZE
}

fn default_theme() -> String {
    // Must name a theme that actually ships: the value is written into a
    // fresh settings.json and matched by name at startup ("GitHub Dark" is
    // what `theme::default_index` falls back to).
    "GitHub Dark".to_string()
}

fn default_auto_save() -> AutoSaveMode {
    AutoSaveMode::Off
}

fn default_format_on_save() -> FormatOnSaveMode {
    FormatOnSaveMode::Off
}

fn default_auto_save_delay() -> u64 {
    1000
}

fn default_tab_size() -> usize {
    4
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(rename = "editor.fontSize", default = "default_font_size")]
    pub editor_font_size: f32,

    /// The size of every text in the interface — Zed's `ui_font_size`.
    ///
    /// It is also the size of `1rem`, the unit the sidebar's rows, icons,
    /// indents and gaps are expressed in, so changing it scales the whole
    /// interface the way zooming a web page does instead of only the editor.
    /// The key is intentionally spelled like Zed's (`ui_font_size`, not a
    /// `workbench.*`/`editor.*` pair) because, unlike the other settings here,
    /// it is not an editor or a workbench option: it is the UI's scale.
    #[serde(default = "default_ui_font_size")]
    pub ui_font_size: f32,

    #[serde(rename = "workbench.colorTheme", default = "default_theme")]
    pub workbench_color_theme: String,

    #[serde(rename = "editor.autoSave", default = "default_auto_save")]
    pub editor_auto_save: AutoSaveMode,

    #[serde(rename = "editor.autoSaveDelay", default = "default_auto_save_delay")]
    pub editor_auto_save_delay: u64,

    #[serde(rename = "editor.tabSize", default = "default_tab_size")]
    pub editor_tab_size: usize,

    #[serde(rename = "editor.formatOnSave", default = "default_format_on_save")]
    pub editor_format_on_save: FormatOnSaveMode,

    #[serde(
        rename = "terminal.integrated.shell",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub terminal_integrated_shell: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            editor_font_size: default_font_size(),
            ui_font_size: default_ui_font_size(),
            workbench_color_theme: default_theme(),
            editor_auto_save: default_auto_save(),
            editor_auto_save_delay: default_auto_save_delay(),
            editor_tab_size: default_tab_size(),
            editor_format_on_save: default_format_on_save(),
            terminal_integrated_shell: None,
        }
    }
}

/// Returns the platform-specific directory where `settings.json` lives:
/// - Windows: `%APPDATA%\ezicode` (falls back to legacy `%APPDATA%\html2gpui`)
/// - macOS: `~/Library/Application Support/ezicode`
/// - Linux / BSD: `$XDG_CONFIG_HOME/ezicode` or `~/.config/ezicode`
pub fn config_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let ezicode = PathBuf::from(&appdata).join("ezicode");
            let legacy = PathBuf::from(&appdata).join("html2gpui");
            if !ezicode.exists() && legacy.exists() {
                return legacy;
            }
            return ezicode;
        }
        if let Ok(userprofile) = std::env::var("USERPROFILE") {
            let ezicode = PathBuf::from(&userprofile)
                .join("AppData")
                .join("Roaming")
                .join("ezicode");
            let legacy = PathBuf::from(&userprofile)
                .join("AppData")
                .join("Roaming")
                .join("html2gpui");
            if !ezicode.exists() && legacy.exists() {
                return legacy;
            }
            return ezicode;
        }
        PathBuf::from(".").join("config")
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            let ezicode = PathBuf::from(&home)
                .join("Library")
                .join("Application Support")
                .join("ezicode");
            let legacy = PathBuf::from(&home)
                .join("Library")
                .join("Application Support")
                .join("html2gpui");
            if !ezicode.exists() && legacy.exists() {
                return legacy;
            }
            return ezicode;
        }
        PathBuf::from(".").join("config")
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            let ezicode = PathBuf::from(&xdg).join("ezicode");
            let legacy = PathBuf::from(&xdg).join("html2gpui");
            if !ezicode.exists() && legacy.exists() {
                return legacy;
            }
            return ezicode;
        }
        if let Ok(home) = std::env::var("HOME") {
            let ezicode = PathBuf::from(&home).join(".config").join("ezicode");
            let legacy = PathBuf::from(&home).join(".config").join("html2gpui");
            if !ezicode.exists() && legacy.exists() {
                return legacy;
            }
            return ezicode;
        }
        PathBuf::from(".").join("config")
    }
}

/// Returns the full path to `settings.json`.
pub fn settings_file_path() -> PathBuf {
    config_dir().join("settings.json")
}

impl Settings {
    /// Loads settings from `settings.json` on disk, creating a default file if none exists.
    pub fn load() -> Self {
        let path = settings_file_path();
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(mut settings) = serde_json::from_str::<Settings>(&content) {
                // Values a user typed into the JSON by hand are clamped on the
                // way in (the same place Zed clamps them), so one bad number
                // can neither shrink the UI into unreadability nor blow it up.
                settings.ui_font_size = clamp_ui_font_size(settings.ui_font_size);
                return settings;
            }
        }

        let defaults = Settings::default();
        let _ = defaults.save();
        defaults
    }

    /// Saves the current settings to `settings.json` with pretty JSON formatting.
    pub fn save(&self) -> Result<(), std::io::Error> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir)?;
        let path = settings_file_path();
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, json.as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The setting is spelled the way Zed spells it, and it is written back out
    /// under the same key, which is what makes a saved size survive a restart.
    #[test]
    fn ui_font_size_round_trips_through_its_zed_key() {
        let mut settings = Settings::default();
        assert_eq!(settings.ui_font_size, DEFAULT_UI_FONT_SIZE);

        settings.ui_font_size = 21.5;
        let json = serde_json::to_string(&settings).unwrap();
        assert!(
            json.contains(r#""ui_font_size":21.5"#),
            "saved settings must use Zed's key, got {json}"
        );

        let restored: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.ui_font_size, 21.5);
    }

    /// Existing `settings.json` files have no `ui_font_size`; they keep the
    /// look they had before the setting existed rather than jumping to a new
    /// size on upgrade.
    #[test]
    fn a_missing_ui_font_size_falls_back_to_the_design_size() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings.ui_font_size, DEFAULT_UI_FONT_SIZE);
        // The design size is also the rem base, so this is the one value at
        // which no metric in the UI moves.
        assert_eq!(settings.ui_font_size, crate::ui::scale::UI_FONT_BASE);
    }

    #[test]
    fn ui_font_size_is_clamped_instead_of_rejected() {
        assert_eq!(clamp_ui_font_size(0.0), MIN_UI_FONT_SIZE);
        assert_eq!(clamp_ui_font_size(6.0), MIN_UI_FONT_SIZE);
        assert_eq!(clamp_ui_font_size(42.0), 42.0);
        assert_eq!(clamp_ui_font_size(100.0), MAX_UI_FONT_SIZE);
        assert_eq!(clamp_ui_font_size(1e9), MAX_UI_FONT_SIZE);
        // A non-finite size would multiply into every length in the interface.
        assert_eq!(clamp_ui_font_size(f32::NAN), DEFAULT_UI_FONT_SIZE);
        assert_eq!(clamp_ui_font_size(f32::INFINITY), DEFAULT_UI_FONT_SIZE);
    }

    /// A hand-edited size is repaired while reading the file, so no code path
    /// has to remember to validate it again.
    #[test]
    fn clamping_happens_when_the_file_is_read() {
        let mut settings: Settings = serde_json::from_str(r#"{"ui_font_size": 900}"#).unwrap();
        assert_eq!(settings.ui_font_size, 900.0);
        settings.ui_font_size = clamp_ui_font_size(settings.ui_font_size);
        assert_eq!(settings.ui_font_size, MAX_UI_FONT_SIZE);
    }
}
