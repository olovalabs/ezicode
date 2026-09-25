//! Installed-extension discovery.
//!
//! ezicode keeps everything an extension produces in one place:
//!
//! ```text
//! <data>/ezicode/extensions/<publisher>.<name>/   unpacked .vsix + .ezicode.json
//! <data>/ezicode/themes/<publisher>.<name>.json   themes converted at install time
//! ```
//!
//! The `ezicode-ext` CLI (see `extensions/` in the repo) writes both, this
//! module reads them. Themes are picked up by [`crate::theme::all`], so a VS
//! Code theme extension shows up in the theme menu without any further wiring.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

/// One entry of `<data>/ezicode/extensions/`.
#[derive(Clone, Debug)]
#[allow(dead_code)] // `publisher`/`path` are read by the host wiring (phase 2)
pub struct InstalledExtension {
    pub id: String,
    pub display_name: String,
    pub publisher: String,
    pub version: String,
    pub description: String,
    /// `false` when the user switched it off (`ezicode-ext disable <id>`).
    pub enabled: bool,
    /// Themes/snippets only — runs without starting the Node extension host.
    pub declarative_only: bool,
    /// Theme names this extension contributed, already converted.
    pub themes: Vec<String>,
    /// `works` | `partial` | `unsupported` — from the install-time report.
    pub compatibility: String,
    pub path: PathBuf,
}

impl InstalledExtension {
    /// Short line shown under the extension name in the sidebar.
    pub fn subtitle(&self) -> String {
        if self.version.is_empty() {
            self.id.clone()
        } else {
            format!("{}  v{}", self.id, self.version)
        }
    }
}

/// `%LOCALAPPDATA%` on Windows, `$XDG_DATA_HOME` or `~/.local/share` elsewhere.
/// Mirrors `lsp::node::language_servers_dir` and `extensions/src/paths.js`.
pub fn data_dir() -> PathBuf {
    if let Some(explicit) = std::env::var_os("EZICODE_DATA_DIR") {
        return PathBuf::from(explicit);
    }
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
    } else if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(data)
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".local/share")
    } else {
        std::env::temp_dir()
    };
    base.join("ezicode")
}

/// Where unpacked extensions live.
pub fn extensions_dir() -> PathBuf {
    data_dir().join("extensions")
}

/// Where converted theme families live.
pub fn themes_dir() -> PathBuf {
    data_dir().join("themes")
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Parse one extension directory. Prefers the installer's `.ezicode.json`
/// and falls back to the raw VS Code `package.json` for side-loaded folders.
fn read_extension(dir: &Path) -> Option<InstalledExtension> {
    let meta = read_json(&dir.join(".ezicode.json"));
    let manifest = read_json(&dir.join("package.json"));
    let source = meta.as_ref().or(manifest.as_ref())?;

    let name = string_field(source, "name");
    let publisher = {
        let p = string_field(source, "publisher");
        if p.is_empty() {
            "unknown".to_string()
        } else {
            p
        }
    };
    let id = {
        let explicit = string_field(source, "id");
        if explicit.is_empty() {
            if name.is_empty() {
                dir.file_name()?.to_string_lossy().to_string()
            } else {
                format!("{publisher}.{name}")
            }
        } else {
            explicit
        }
    };

    let display_name = {
        let d = string_field(source, "displayName");
        if d.is_empty() {
            if name.is_empty() {
                id.clone()
            } else {
                name.clone()
            }
        } else {
            d
        }
    };

    let themes = source
        .get("themes")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let declarative_only = source
        .get("declarativeOnly")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            manifest
                .as_ref()
                .map(|m| m.get("main").is_none())
                .unwrap_or(true)
        });

    Some(InstalledExtension {
        id,
        display_name,
        publisher,
        version: string_field(source, "version"),
        description: string_field(source, "description"),
        enabled: source
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        declarative_only,
        themes,
        compatibility: string_field(source, "compatibility"),
        path: dir.to_path_buf(),
    })
}

/// Read `<data>/ezicode/extensions/` from disk, sorted by id.
pub fn scan() -> Vec<InstalledExtension> {
    let dir = extensions_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let hidden = path
            .file_name()
            .map(|n| n.to_string_lossy().starts_with('.'))
            .unwrap_or(true);
        if hidden {
            continue;
        }
        if let Some(extension) = read_extension(&path) {
            found.push(extension);
        }
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found
}

fn cache() -> &'static Mutex<Option<Vec<InstalledExtension>>> {
    static CACHE: OnceLock<Mutex<Option<Vec<InstalledExtension>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Cached view of the installed extensions (scans disk on first use).
pub fn installed() -> Vec<InstalledExtension> {
    let mut guard = cache().lock().unwrap_or_else(|poison| poison.into_inner());
    if guard.is_none() {
        *guard = Some(scan());
    }
    (*guard).clone().unwrap_or_default()
}

/// Re-read the extensions directory (call after an install/uninstall).
#[allow(dead_code)]
pub fn refresh() -> Vec<InstalledExtension> {
    let fresh = scan();
    let mut guard = cache().lock().unwrap_or_else(|poison| poison.into_inner());
    *guard = Some(fresh.clone());
    fresh
}

/// Contents of every `<data>/ezicode/themes/*.json`, as `(file name, json)`.
///
/// These are already in ezicode's own theme format — the VS Code -> ezicode
/// conversion happens once, at install time, in `ezicode-ext`.
pub fn user_theme_sources() -> Vec<(String, String)> {
    let dir = themes_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .map(|ext| ext.eq_ignore_ascii_case("json"))
                    .unwrap_or(false)
        })
        .collect();
    files.sort();

    files
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_string_lossy().to_string();
            let text = std::fs::read_to_string(&path).ok()?;
            Some((name, text))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_is_namespaced() {
        assert!(data_dir().ends_with("ezicode") || std::env::var_os("EZICODE_DATA_DIR").is_some());
        assert!(extensions_dir().ends_with("extensions"));
        assert!(themes_dir().ends_with("themes"));
    }

    #[test]
    fn missing_directories_are_not_an_error() {
        // A fresh install has no extensions directory at all.
        assert!(user_theme_sources().len() < 10_000);
        let _ = installed();
    }

    #[test]
    fn reads_installer_metadata() {
        let dir = std::env::temp_dir().join(format!("ezicode-ext-test-{}", std::process::id()));
        let ext = dir.join("acme.sample-theme");
        std::fs::create_dir_all(&ext).expect("create temp extension dir");
        std::fs::write(
            ext.join(".ezicode.json"),
            r#"{
                "id": "acme.sample-theme",
                "name": "sample-theme",
                "publisher": "acme",
                "displayName": "Sample Theme",
                "version": "2.1.0",
                "description": "A colour theme",
                "enabled": true,
                "declarativeOnly": true,
                "themes": ["Sample Dark", "Sample Light"],
                "compatibility": "works"
            }"#,
        )
        .expect("write metadata");

        let parsed = read_extension(&ext).expect("metadata parses");
        assert_eq!(parsed.id, "acme.sample-theme");
        assert_eq!(parsed.display_name, "Sample Theme");
        assert_eq!(parsed.version, "2.1.0");
        assert!(parsed.enabled);
        assert!(parsed.declarative_only);
        assert_eq!(parsed.themes.len(), 2);
        assert_eq!(parsed.subtitle(), "acme.sample-theme  v2.1.0");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn falls_back_to_the_vscode_manifest() {
        let dir = std::env::temp_dir().join(format!("ezicode-ext-pkg-{}", std::process::id()));
        let ext = dir.join("side-loaded");
        std::fs::create_dir_all(&ext).expect("create temp extension dir");
        std::fs::write(
            ext.join("package.json"),
            r#"{"name":"hello","publisher":"ezicode","version":"0.1.0","main":"./extension.js"}"#,
        )
        .expect("write manifest");

        let parsed = read_extension(&ext).expect("manifest parses");
        assert_eq!(parsed.id, "ezicode.hello");
        assert!(parsed.enabled);
        assert!(!parsed.declarative_only, "an extension with `main` needs the host");

        std::fs::remove_dir_all(&dir).ok();
    }
}
