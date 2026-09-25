//! Bridges theme extensions into the built-in theme system.
//!
//! A Zed theme extension ships one or more theme *family* files under
//! `themes/*.json`. Each file has the shape ezicode's own bundled themes use
//! (`{ "name": ..., "themes": [ { "name", "appearance", "style" }, ... ] }`),
//! so the exact same parser handles both.

/// Every theme-family JSON document contributed by installed extensions.
/// The theme module parses these with the same `parse_family` used for the
/// bundled themes, so extension themes appear seamlessly in the picker.
pub fn theme_family_jsons() -> Vec<String> {
    let mut out = Vec::new();
    for ext in super::installed() {
        for file in ext.theme_files() {
            match std::fs::read_to_string(&file) {
                Ok(s) => out.push(s),
                Err(e) => eprintln!("extension theme: cannot read {}: {e}", file.display()),
            }
        }
    }
    out
}

/// Paths of every icon-theme JSON contributed by installed extensions, paired
/// with the owning extension directory (icons are resolved relative to it).
#[allow(dead_code)]
pub fn icon_theme_files() -> Vec<(std::path::PathBuf, std::path::PathBuf)> {
    let mut out = Vec::new();
    for ext in super::installed() {
        for file in ext.icon_theme_files() {
            out.push((ext.dir.clone(), file));
        }
    }
    out
}
