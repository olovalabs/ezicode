//! Zed-compatible extension subsystem.
//!
//! ezicode loads extensions authored in Zed's exact format — the same
//! `extension.toml`, the same `themes/`, `icon_themes/` and `languages/`
//! directory layout — so an extension written for Zed can be installed here
//! unchanged. See `docs/EXTENSIONS.md` for the authoring guide.
//!
//! What is wired up natively today:
//! * **Theme extensions** — full support; Zed theme JSON feeds straight into
//!   the theme picker.
//! * **Icon theme extensions** — discovered and exposed.
//! * **Language extensions** — file-type associations from `config.toml`;
//!   syntax highlighting activates when a matching Tree-sitter grammar is built
//!   in.
//! * **Grammar / language-server / logic extensions** — recognised from the
//!   manifest and surfaced in the UI. Executing the compiled WASM component
//!   requires the optional `wasm-extensions` build feature (see [`wasm`]).

pub mod languages;
pub mod lsp;
pub mod manifest;
pub mod store;
pub mod themes;

#[cfg(feature = "wasm-extensions")]
pub mod wasm;

use std::sync::OnceLock;

pub use manifest::ExtensionManifest;
pub use store::{Capabilities, InstalledExtension};

static INSTALLED: OnceLock<Vec<InstalledExtension>> = OnceLock::new();
static SUMMARIES: OnceLock<Vec<ExtensionSummary>> = OnceLock::new();

/// Cheap, owned, per-extension display data for the Extensions panel. Computed
/// once (capability detection touches the disk) and memoized so rendering never
/// hits the filesystem.
#[derive(Debug, Clone)]
pub struct ExtensionSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub authors: Vec<String>,
    pub description: String,
    pub capabilities: Capabilities,
}

/// Memoized display summaries for every installed extension.
pub fn summaries() -> &'static [ExtensionSummary] {
    SUMMARIES.get_or_init(|| {
        installed()
            .iter()
            .map(|e| ExtensionSummary {
                id: e.manifest.effective_id(),
                name: if e.manifest.name.is_empty() {
                    e.manifest.effective_id()
                } else {
                    e.manifest.name.clone()
                },
                version: e.manifest.version.clone(),
                authors: e.manifest.authors.clone(),
                description: e.manifest.description.clone(),
                capabilities: e.capabilities(),
            })
            .collect()
    })
}

/// All installed extensions, scanned once and memoized for the process
/// lifetime. Extensions added while the app is running become active after a
/// restart — the same model Zed uses for reindexing installed extensions.
pub fn installed() -> &'static [InstalledExtension] {
    INSTALLED.get_or_init(store::scan)
}

/// Called once at startup. Logs a short summary and, when built with the
/// `wasm-extensions` feature, initializes the WASM host.
pub fn init() {
    let exts = installed();
    if exts.is_empty() {
        return;
    }
    let mut themes = 0;
    let mut langs = 0;
    let mut servers = 0;
    for e in exts {
        let c = e.capabilities();
        themes += c.themes as usize;
        langs += c.languages as usize;
        servers += c.language_servers as usize;
    }
    println!(
        "extensions: {} installed ({} with themes, {} with languages, {} with language servers)",
        exts.len(),
        themes,
        langs,
        servers,
    );

    #[cfg(feature = "wasm-extensions")]
    wasm::init(exts);
}
