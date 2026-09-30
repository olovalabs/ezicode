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

/// Git integration settings, shaped exactly like Zed's `git` block so the
/// same `settings.json` works in both editors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GitSettings {
    #[serde(default)]
    pub inline_blame: InlineBlameSettings,
}

impl Default for GitSettings {
    fn default() -> Self {
        Self {
            inline_blame: InlineBlameSettings::default(),
        }
    }
}

/// Zed's `git.inline_blame` object. Ghost-text blame shown at the end of the
/// current line.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InlineBlameSettings {
    /// Whether inline blame is shown at all.
    #[serde(default = "default_blame_enabled")]
    pub enabled: bool,
    /// How long the cursor must rest on a line before the annotation appears.
    /// `0` shows it immediately (Zed's default).
    #[serde(default)]
    pub delay_ms: u64,
    /// Never render the annotation before this column, so short lines don't get
    /// a hint jammed right against the text.
    #[serde(default)]
    pub min_column: u32,
    /// Columns between the end of the line and the annotation.
    #[serde(default = "default_blame_padding")]
    pub padding: u32,
    /// Append the commit summary after "Author, <relative date>".
    #[serde(default)]
    pub show_commit_summary: bool,
}

fn default_blame_enabled() -> bool {
    true
}

fn default_blame_padding() -> u32 {
    7
}

impl Default for InlineBlameSettings {
    fn default() -> Self {
        // Matches Zed's defaults.
        Self {
            enabled: true,
            delay_ms: 0,
            min_column: 0,
            padding: 7,
            show_commit_summary: false,
        }
    }
}

fn default_git() -> GitSettings {
    GitSettings::default()
}

fn default_font_size() -> f32 {
    14.5
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

    /// Nested `git` block (Zed-compatible), home of `inline_blame`.
    #[serde(rename = "git", default = "default_git")]
    pub git: GitSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            editor_font_size: default_font_size(),
            workbench_color_theme: default_theme(),
            editor_auto_save: default_auto_save(),
            editor_auto_save_delay: default_auto_save_delay(),
            editor_tab_size: default_tab_size(),
            editor_format_on_save: default_format_on_save(),
            terminal_integrated_shell: None,
            git: default_git(),
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
            if let Ok(settings) = serde_json::from_str::<Settings>(&content) {
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

    #[test]
    fn inline_blame_defaults_match_zed() {
        let d = InlineBlameSettings::default();
        assert!(d.enabled);
        assert_eq!(d.delay_ms, 0);
        assert_eq!(d.min_column, 0);
        assert_eq!(d.padding, 7);
        assert!(!d.show_commit_summary);
    }

    #[test]
    fn parses_zed_shaped_git_block() {
        let json = r#"{
            "editor.fontSize": 14.5,
            "git": {
                "inline_blame": {
                    "enabled": true,
                    "delay_ms": 300,
                    "min_column": 40,
                    "padding": 12,
                    "show_commit_summary": true
                }
            }
        }"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert!(s.git.inline_blame.enabled);
        assert_eq!(s.git.inline_blame.delay_ms, 300);
        assert_eq!(s.git.inline_blame.min_column, 40);
        assert_eq!(s.git.inline_blame.padding, 12);
        assert!(s.git.inline_blame.show_commit_summary);
    }

    #[test]
    fn git_block_absent_uses_defaults() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert!(s.git.inline_blame.enabled);
        assert_eq!(s.git.inline_blame.delay_ms, 0);
    }

    #[test]
    fn partial_inline_blame_fills_missing_fields() {
        let json = r#"{ "git": { "inline_blame": { "enabled": false } } }"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert!(!s.git.inline_blame.enabled);
        // Unspecified fields fall back to defaults.
        assert_eq!(s.git.inline_blame.min_column, 0);
    }
}
