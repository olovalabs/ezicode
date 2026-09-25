//! Parsing of the Zed-compatible `extension.toml` manifest and the per-language
//! `languages/<lang>/config.toml` files.
//!
//! The field layout intentionally mirrors Zed's own extension schema
//! (<https://github.com/zed-industries/zed>, `crates/extension`), so an
//! extension authored for Zed can be dropped into ezicode unchanged. Unknown
//! keys are ignored on purpose — newer Zed schema fields simply pass through
//! without breaking loading.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Top-level `extension.toml` manifest, matching Zed's format.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ExtensionManifest {
    /// Stable identifier, e.g. `"gleam"`. Used as the on-disk folder name.
    #[serde(default)]
    pub id: String,
    /// Human readable display name.
    #[serde(default)]
    pub name: String,
    /// SemVer version string.
    #[serde(default)]
    pub version: String,
    /// Manifest schema version. Zed currently ships schema `1`.
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub repository: String,
    #[serde(default)]
    pub authors: Vec<String>,

    /// Present when the extension ships compiled logic (a WASM component built
    /// against `zed_extension_api`).
    #[serde(default)]
    pub lib: Option<LibManifest>,

    /// Tree-sitter grammars the extension provides, keyed by grammar name.
    #[serde(default)]
    pub grammars: BTreeMap<String, GrammarManifest>,

    /// Language servers the extension registers, keyed by server id.
    #[serde(default)]
    pub language_servers: BTreeMap<String, LanguageServerManifest>,
}

/// The `[lib]` table — describes the compiled WASM component, if any.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LibManifest {
    /// Typically `"rust"`.
    #[serde(default)]
    pub kind: Option<String>,
    /// Version of `zed_extension_api` the component was built against.
    #[serde(default)]
    pub version: Option<String>,
}

/// A `[grammars.<name>]` entry describing a Tree-sitter grammar to fetch/build.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct GrammarManifest {
    #[serde(default)]
    pub repository: String,
    /// Git commit to pin. Zed historically used both `commit` and `rev`.
    #[serde(default, alias = "rev")]
    pub commit: String,
    /// Optional sub-path inside the grammar repository.
    #[serde(default)]
    pub path: Option<String>,
}

/// A `[language_servers.<id>]` entry.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LanguageServerManifest {
    #[serde(default)]
    pub name: Option<String>,
    /// Single language this server attaches to (older schema).
    #[serde(default)]
    pub language: Option<String>,
    /// Multiple languages this server attaches to (newer schema).
    #[serde(default)]
    pub languages: Vec<String>,
    /// Optional map of language name -> language id override.
    #[serde(default)]
    pub language_ids: BTreeMap<String, String>,

    /// ezicode extension (ignored by Zed): the binary to launch for this
    /// language server, resolved on `$PATH`. When present, ezicode can start
    /// the server natively without executing the extension's WASM component —
    /// the pragmatic path for language servers that ship as a normal binary.
    #[serde(default)]
    pub command: Option<String>,
    /// ezicode extension: arguments passed to `command`.
    #[serde(default)]
    pub args: Vec<String>,
}

impl LanguageServerManifest {
    /// Every language name this server handles, merging both schema styles.
    #[allow(dead_code)]
    pub fn all_languages(&self) -> Vec<String> {
        let mut out = self.languages.clone();
        if let Some(l) = &self.language {
            if !out.iter().any(|x| x == l) {
                out.push(l.clone());
            }
        }
        out
    }
}

impl ExtensionManifest {
    /// Parse an `extension.toml` string.
    pub fn parse(toml_str: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(toml_str)
    }

    /// A best-effort id: falls back to the name (slugified) when `id` is blank,
    /// which keeps very old manifests loadable.
    pub fn effective_id(&self) -> String {
        if !self.id.is_empty() {
            return self.id.clone();
        }
        slugify(&self.name)
    }
}

/// The per-language `languages/<lang>/config.toml` file (Zed format).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LanguageConfigManifest {
    #[serde(default)]
    pub name: String,
    /// Name of the Tree-sitter grammar this language uses.
    #[serde(default)]
    pub grammar: Option<String>,
    /// File extensions (without the dot) associated with this language.
    #[serde(default)]
    pub path_suffixes: Vec<String>,
    /// Line-comment token(s), used for editor comment toggling.
    #[serde(default)]
    pub line_comments: Vec<String>,
}

/// Turn an arbitrary display name into a filesystem/id friendly slug.
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_zed_manifest() {
        let src = r#"
            id = "gleam"
            name = "Gleam"
            version = "0.2.0"
            schema_version = 1
            authors = ["Jane Doe <jane@example.com>"]
            description = "Gleam language support"
            repository = "https://github.com/gleam-lang/zed-gleam"

            [lib]
            kind = "rust"
            version = "0.1.0"

            [grammars.gleam]
            repository = "https://github.com/gleam-lang/tree-sitter-gleam"
            commit = "deadbeef"

            [language_servers.gleam]
            name = "Gleam LSP"
            language = "Gleam"
        "#;
        let m = ExtensionManifest::parse(src).expect("parse");
        assert_eq!(m.id, "gleam");
        assert_eq!(m.name, "Gleam");
        assert_eq!(m.schema_version, 1);
        assert_eq!(m.authors.len(), 1);
        assert!(m.lib.is_some());
        assert_eq!(m.grammars.len(), 1);
        assert_eq!(m.grammars["gleam"].repository, "https://github.com/gleam-lang/tree-sitter-gleam");
        assert_eq!(m.language_servers["gleam"].all_languages(), vec!["Gleam"]);
    }

    #[test]
    fn parses_native_language_server_command() {
        let src = r#"
            id = "nim"
            name = "Nim"
            [language_servers.nimlangserver]
            name = "Nim Language Server"
            language = "Nim"
            command = "nimlangserver"
            args = ["--stdio"]
        "#;
        let m = ExtensionManifest::parse(src).unwrap();
        let ls = &m.language_servers["nimlangserver"];
        assert_eq!(ls.command.as_deref(), Some("nimlangserver"));
        assert_eq!(ls.args, vec!["--stdio"]);
        assert_eq!(ls.all_languages(), vec!["Nim"]);
    }

    #[test]
    fn accepts_rev_alias_for_commit() {
        let src = r#"
            id = "x"
            name = "X"
            [grammars.x]
            repository = "https://example.com/x"
            rev = "abc123"
        "#;
        let m = ExtensionManifest::parse(src).unwrap();
        assert_eq!(m.grammars["x"].commit, "abc123");
    }

    #[test]
    fn ignores_unknown_future_fields() {
        let src = r#"
            id = "x"
            name = "X"
            some_future_field = "ok"
            [capabilities]
            anything = true
        "#;
        let m = ExtensionManifest::parse(src).unwrap();
        assert_eq!(m.id, "x");
    }

    #[test]
    fn parses_language_config() {
        let src = r#"
            name = "Gleam"
            grammar = "gleam"
            path_suffixes = ["gleam"]
            line_comments = ["// "]
        "#;
        let c: LanguageConfigManifest = toml::from_str(src).unwrap();
        assert_eq!(c.name, "Gleam");
        assert_eq!(c.grammar.as_deref(), Some("gleam"));
        assert_eq!(c.path_suffixes, vec!["gleam"]);
    }

    #[test]
    fn slugify_makes_ids() {
        assert_eq!(slugify("My Cool Theme!"), "my-cool-theme");
        assert_eq!(slugify("Gleam"), "gleam");
        assert_eq!(slugify("  spaced  out  "), "spaced-out");
    }
}
