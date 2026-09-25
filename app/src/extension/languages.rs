//! Bridges language extensions into file-type detection.
//!
//! Zed language extensions describe a language in `languages/<lang>/config.toml`
//! with a `name`, a `grammar` reference and a `path_suffixes` list. We turn each
//! path suffix into a file-extension -> language-id association so ezicode can
//! label and (when a matching grammar is compiled in) highlight those files.

use std::collections::HashMap;
use std::sync::OnceLock;

use super::manifest::{slugify, LanguageConfigManifest};

static LANG_BY_EXT: OnceLock<HashMap<String, &'static str>> = OnceLock::new();

/// Language id for a file extension contributed by an installed extension, or
/// `None` if no extension claims it. The returned id is the grammar name (so it
/// lines up with the highlighter's grammar registry) falling back to a slug of
/// the language name.
pub fn language_for_extension(ext: &str) -> Option<&'static str> {
    if ext.is_empty() {
        return None;
    }
    LANG_BY_EXT.get_or_init(build_map).get(ext).copied()
}

/// The list of `(language name, extensions)` contributed by extensions — used
/// for the Extensions panel summary.
#[allow(dead_code)]
pub fn language_summaries() -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for e in super::installed() {
        for cfg_path in e.language_config_files() {
            if let Some(cfg) = read_config(&cfg_path) {
                if !cfg.name.is_empty() {
                    out.push((cfg.name, cfg.path_suffixes));
                }
            }
        }
    }
    out
}

fn build_map() -> HashMap<String, &'static str> {
    let mut map = HashMap::new();
    for e in super::installed() {
        for cfg_path in e.language_config_files() {
            let Some(cfg) = read_config(&cfg_path) else {
                continue;
            };
            let id = language_id(&cfg);
            // Leak the id so we can hand out `&'static str` like the built-in
            // detector does. There is a bounded, tiny number of extension
            // languages, so this one-time leak is inconsequential.
            let leaked: &'static str = Box::leak(id.into_boxed_str());
            for suffix in &cfg.path_suffixes {
                let key = suffix.trim_start_matches('.').to_ascii_lowercase();
                if !key.is_empty() {
                    map.entry(key).or_insert(leaked);
                }
            }
        }
    }
    map
}

fn language_id(cfg: &LanguageConfigManifest) -> String {
    if let Some(g) = &cfg.grammar {
        if !g.is_empty() {
            return g.to_ascii_lowercase();
        }
    }
    slugify(&cfg.name)
}

/// Maps a language *display name* (as used in `[language_servers].language`,
/// e.g. `"Gleam"`) to the ezicode language *id* used everywhere else. Falls
/// back to a slug of the name when no matching language config is installed.
pub fn language_id_for_name(name: &str) -> String {
    for e in super::installed() {
        for cfg_path in e.language_config_files() {
            if let Some(cfg) = read_config(&cfg_path) {
                if cfg.name.eq_ignore_ascii_case(name) {
                    return language_id(&cfg);
                }
            }
        }
    }
    slugify(name)
}

fn read_config(path: &std::path::Path) -> Option<LanguageConfigManifest> {
    let text = std::fs::read_to_string(path).ok()?;
    match toml::from_str::<LanguageConfigManifest>(&text) {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("extension language: cannot parse {}: {e}", path.display());
            None
        }
    }
}
