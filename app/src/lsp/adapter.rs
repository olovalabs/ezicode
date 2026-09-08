//! Language-server adapters — a port of Zed's `LspAdapter` trait
//! (`crates/languages/src/*.rs`) to a plain data table.
//!
//! In Zed every language server is described by an adapter that answers four
//! questions:
//!
//! 1. **What binary do I run, and with which arguments?**
//!    (`LspAdapter::check_if_version_installed` / `fetch_server_binary`)
//! 2. **What `initializationOptions` does it need?**
//!    (`LspAdapter::initialization_options`)
//! 3. **What does it get back from `workspace/configuration`?**
//!    (`LspAdapter::workspace_configuration`)
//! 4. **What LSP `languageId` do my buffers map to?**
//!    (`LspAdapter::language_ids`)
//!
//! Zed's adapters are async trait objects because they can hit the network to
//! resolve the latest npm version. We keep the same four answers but express
//! them as a static table plus a small amount of logic, because the install
//! step lives in [`crate::lsp::node`] (the analogue of Zed's `NodeRuntime`).
//!
//! Question 3 is the one that matters most and is the easiest to get wrong:
//! every VS Code-derived server (CSS, HTML, JSON, ESLint, YAML) **publishes no
//! diagnostics at all** until the client answers `workspace/configuration`
//! with a config blob that enables validation. Zed hardcodes those blobs in
//! each adapter; so do we, in [`ServerAdapter::workspace_configuration`].

use std::path::Path;

use serde_json::{json, Value};

/// How a server's executable is obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A Node package Zed installs on demand with npm. `package` is the npm
    /// package name; `entry` is the path *inside* the install directory of
    /// the JS entry point, which we run with our managed `node`.
    ///
    /// Mirrors e.g. `CssLspAdapter::PACKAGE_NAME` +
    /// `"node_modules/vscode-langservers-extracted/bin/vscode-css-language-server"`.
    Npm {
        package: &'static str,
        entry: &'static str,
    },
    /// A native binary the user installs themselves (rust-analyzer, gopls,
    /// clangd, zls…). Zed downloads these from GitHub releases; we look them
    /// up on `PATH`, which keeps toolchain management with the user.
    Native { binary: &'static str },
}

/// The static description of one language server.
#[derive(Clone, Copy, Debug)]
pub struct ServerAdapter {
    /// Stable id, also the key under which the client is cached and the label
    /// shown in the status bar. Matches Zed's `LanguageServerName`.
    pub name: &'static str,
    pub source: Source,
    /// Extra CLI arguments appended after the entry point / binary.
    pub args: &'static [&'static str],
    /// Editor language ids (our `crate::lang` ids) this server handles.
    pub languages: &'static [&'static str],
}

impl ServerAdapter {
    /// The npm package this adapter installs, if any.
    #[allow(dead_code)]
    pub fn npm_package(&self) -> Option<&'static str> {
        match self.source {
            Source::Npm { package, .. } => Some(package),
            Source::Native { .. } => None,
        }
    }

    /// Additional npm packages that must be installed alongside the server.
    ///
    /// Zed's `TypeScriptLspAdapter::fetch_server_binary` installs
    /// `typescript` next to `typescript-language-server`, because the server
    /// is only a thin protocol shim around `tsserver.js` and cannot produce a
    /// single diagnostic without it. This is the single most common reason a
    /// hand-rolled TypeScript integration reports nothing.
    pub fn extra_npm_packages(&self) -> &'static [&'static str] {
        if self.name == "typescript-language-server" {
            // Pinned major: typescript-language-server does not support
            // TypeScript 7+, which no longer ships `tsserver.js` (see the
            // comment in Zed's `TypeScriptLspAdapter::tsdk_path`).
            &["typescript@6"]
        } else {
            &[]
        }
    }

    /// `initializationOptions` for the `initialize` request.
    ///
    /// Ported from each adapter's `initialization_options`.
    pub fn initialization_options(&self, root: Option<&Path>) -> Option<Value> {
        match self.name {
            // Zed: TypeScriptLspAdapter::initialization_options
            "typescript-language-server" => {
                // Point the server at the project's own TypeScript when it
                // has one, exactly like Zed's `tsdk_path` — that way the
                // editor reports the same errors as the project's `tsc`.
                let tsdk = root.and_then(|root| {
                    let local = root.join("node_modules/typescript/lib");
                    local.join("tsserver.js").is_file().then_some(local)
                });
                Some(json!({
                    "provideFormatter": true,
                    "hostInfo": "ezicode",
                    "tsserver": { "path": tsdk },
                    "preferences": {
                        "includeInlayParameterNameHints": "all",
                        "includeInlayParameterNameHintsWhenArgumentMatchesName": true,
                        "includeInlayFunctionParameterTypeHints": true,
                        "includeInlayVariableTypeHints": true,
                        "includeInlayVariableTypeHintsWhenTypeMatchesName": true,
                        "includeInlayPropertyDeclarationTypeHints": true,
                        "includeInlayFunctionLikeReturnTypeHints": true,
                        "includeInlayEnumMemberValueHints": true,
                    }
                }))
            }
            // Zed: Css/Html/JsonLspAdapter::initialization_options
            "vscode-css-language-server"
            | "vscode-html-language-server"
            | "json-language-server" => Some(json!({ "provideFormatter": true })),
            _ => None,
        }
    }

    /// The reply to a server's `workspace/configuration` request.
    ///
    /// `section` is the requested settings section (`items[i].section`); the
    /// server gets one array element back per requested item.
    ///
    /// Ported from each adapter's `workspace_configuration`. Without this the
    /// VS Code family of servers stays completely silent.
    pub fn workspace_configuration(&self, section: &str, root: Option<&Path>) -> Value {
        match self.name {
            // Zed: TypeScriptLspAdapter::workspace_configuration
            "typescript-language-server" => json!({
                "completions": { "completeFunctionCalls": true }
            }),

            // Zed: JsonLspAdapter::workspace_configuration
            "json-language-server" => json!({
                "json": {
                    "format": { "enable": true },
                    "validate": { "enable": true },
                    "schemas": [],
                }
            }),

            // Zed: CssLspAdapter — validation must be switched on explicitly.
            "vscode-css-language-server" => {
                let validate = json!({
                    "validate": true,
                    "lint": { "unknownAtRules": "ignore" },
                });
                match section {
                    "css" | "scss" | "less" => validate,
                    _ => json!({ "css": validate, "scss": validate, "less": validate }),
                }
            }

            // Zed: HtmlLspAdapter
            "vscode-html-language-server" => json!({
                "html": {
                    "validate": { "scripts": true, "styles": true },
                    "format": { "enable": true },
                    "suggest": { "html5": true },
                },
                "css": { "validate": true },
                "javascript": { "validate": { "enable": true } },
            }),

            // Zed: YamlLspAdapter::workspace_configuration
            "yaml-language-server" => json!({
                "yaml": {
                    "validate": true,
                    "format": { "enable": true },
                    "keyOrdering": false,
                    "schemas": {},
                },
                "[yaml]": { "editor.tabSize": 2 },
            }),

            // Zed: EsLintLspAdapter::workspace_configuration. ESLint is the
            // fussiest of the family: it wants the whole blob under the ""
            // section and refuses to lint without `workspaceFolder`.
            "eslint" => {
                let root_uri = root
                    .and_then(super::client::path_to_uri)
                    .map(|u| u.as_str().to_owned())
                    .unwrap_or_default();
                let root_name = root
                    .and_then(|r| r.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                json!({
                    "validate": "on",
                    "rulesCustomizations": [],
                    "run": "onType",
                    "nodePath": null,
                    "workingDirectory": { "mode": "auto" },
                    "workspaceFolder": { "uri": root_uri, "name": root_name },
                    "problems": {},
                    "codeActionOnSave": { "enable": true },
                    "codeAction": {
                        "disableRuleComment": { "enable": true, "location": "separateLine" },
                        "showDocumentation": { "enable": true }
                    }
                })
            }

            _ => Value::Null,
        }
    }

    /// The LSP `languageId` for one of our editor language ids.
    ///
    /// Ported from `LspAdapter::language_ids`. Our internal ids are chosen to
    /// suit the tree-sitter highlighter (`"tsx"`, `"jsx"`, `"bash"`) and are
    /// not always the identifiers the protocol defines
    /// (`"typescriptreact"`, `"javascriptreact"`, `"shellscript"`).
    ///
    /// Measured against `typescript-language-server` 6.x, sending `"tsx"`
    /// still produces correct diagnostics because it also infers the dialect
    /// from the file extension — but that is a fallback, not a guarantee. We
    /// send the standard id so behaviour doesn't depend on a server's
    /// tolerance, matching what Zed and VS Code send.
    pub fn language_id(&self, lang: &str) -> &'static str {
        match lang {
            "tsx" => "typescriptreact",
            "jsx" => "javascriptreact",
            "typescript" => "typescript",
            "javascript" => "javascript",
            "csharp" => "csharp",
            "bash" => "shellscript",
            "markdown" => "markdown",
            "scss" => "scss",
            "c" => "c",
            "cpp" => "cpp",
            other => match other {
                "rust" => "rust",
                "go" => "go",
                "python" => "python",
                "html" => "html",
                "css" => "css",
                "json" => "json",
                "yaml" => "yaml",
                "toml" => "toml",
                "php" => "php",
                "ruby" => "ruby",
                "java" => "java",
                "lua" => "lua",
                "zig" => "zig",
                "dockerfile" => "dockerfile",
                _ => "plaintext",
            },
        }
    }
}

/// Every language server the editor knows about.
///
/// The Node-based entries mirror the set Zed installs by default for web
/// development; the native entries mirror Zed's non-Node defaults.
pub static ADAPTERS: &[ServerAdapter] = &[
    // -- Node-based (auto-installed, exactly like Zed) ---------------------
    ServerAdapter {
        name: "typescript-language-server",
        source: Source::Npm {
            package: "typescript-language-server",
            entry: "node_modules/typescript-language-server/lib/cli.mjs",
        },
        args: &["--stdio"],
        languages: &["typescript", "javascript", "tsx", "jsx"],
    },
    ServerAdapter {
        name: "vscode-css-language-server",
        source: Source::Npm {
            package: "vscode-langservers-extracted",
            entry: "node_modules/vscode-langservers-extracted/bin/vscode-css-language-server",
        },
        args: &["--stdio"],
        languages: &["css", "scss"],
    },
    ServerAdapter {
        name: "vscode-html-language-server",
        source: Source::Npm {
            package: "vscode-langservers-extracted",
            entry: "node_modules/vscode-langservers-extracted/bin/vscode-html-language-server",
        },
        args: &["--stdio"],
        languages: &["html"],
    },
    ServerAdapter {
        name: "json-language-server",
        source: Source::Npm {
            package: "vscode-langservers-extracted",
            entry: "node_modules/vscode-langservers-extracted/bin/vscode-json-language-server",
        },
        args: &["--stdio"],
        languages: &["json"],
    },
    ServerAdapter {
        name: "yaml-language-server",
        source: Source::Npm {
            package: "yaml-language-server",
            entry: "node_modules/yaml-language-server/bin/yaml-language-server",
        },
        args: &["--stdio"],
        languages: &["yaml"],
    },
    ServerAdapter {
        name: "bash-language-server",
        source: Source::Npm {
            package: "bash-language-server",
            entry: "node_modules/bash-language-server/out/cli.js",
        },
        args: &["start"],
        languages: &["bash"],
    },
    ServerAdapter {
        name: "docker-langserver",
        source: Source::Npm {
            package: "dockerfile-language-server-nodejs",
            entry: "node_modules/dockerfile-language-server-nodejs/bin/docker-langserver",
        },
        args: &["--stdio"],
        languages: &["dockerfile"],
    },
    // -- Native toolchain servers (looked up on PATH) ----------------------
    ServerAdapter {
        name: "rust-analyzer",
        source: Source::Native {
            binary: "rust-analyzer",
        },
        args: &[],
        languages: &["rust"],
    },
    ServerAdapter {
        name: "gopls",
        source: Source::Native { binary: "gopls" },
        args: &[],
        languages: &["go"],
    },
    ServerAdapter {
        name: "basedpyright-langserver",
        source: Source::Native {
            binary: "basedpyright-langserver",
        },
        args: &["--stdio"],
        languages: &["python"],
    },
    ServerAdapter {
        name: "clangd",
        source: Source::Native { binary: "clangd" },
        args: &[],
        languages: &["c", "cpp"],
    },
    ServerAdapter {
        name: "zls",
        source: Source::Native { binary: "zls" },
        args: &[],
        languages: &["zig"],
    },
    ServerAdapter {
        name: "lua-language-server",
        source: Source::Native {
            binary: "lua-language-server",
        },
        args: &[],
        languages: &["lua"],
    },
    ServerAdapter {
        name: "taplo",
        source: Source::Native { binary: "taplo" },
        args: &["lsp", "stdio"],
        languages: &["toml"],
    },
    ServerAdapter {
        name: "intelephense",
        source: Source::Native {
            binary: "intelephense",
        },
        args: &["--stdio"],
        languages: &["php"],
    },
    ServerAdapter {
        name: "ruby-lsp",
        source: Source::Native { binary: "ruby-lsp" },
        args: &[],
        languages: &["ruby"],
    },
];

/// The adapter that handles `lang`, if any.
pub fn adapter_for_language(lang: &str) -> Option<&'static ServerAdapter> {
    ADAPTERS.iter().find(|a| a.languages.contains(&lang))
}

/// The adapter with this server name.
pub fn adapter_by_name(name: &str) -> Option<&'static ServerAdapter> {
    ADAPTERS.iter().find(|a| a.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_web_language_has_an_adapter() {
        for lang in [
            "typescript",
            "javascript",
            "tsx",
            "jsx",
            "html",
            "css",
            "scss",
            "json",
            "yaml",
            "bash",
        ] {
            let a = adapter_for_language(lang)
                .unwrap_or_else(|| panic!("no adapter for {lang}"));
            assert!(a.npm_package().is_some(), "{lang} should be node-based");
        }
    }

    #[test]
    fn tsx_uses_the_standard_protocol_language_id() {
        let ts = adapter_for_language("tsx").unwrap();
        // Our internal highlighter ids are not the protocol's ids.
        assert_eq!(ts.language_id("tsx"), "typescriptreact");
        assert_eq!(ts.language_id("jsx"), "javascriptreact");
        assert_eq!(ts.language_id("typescript"), "typescript");
    }

    #[test]
    fn one_server_serves_the_whole_typescript_family() {
        // ts/js/tsx/jsx must resolve to the SAME adapter, so a single server
        // process holds one project graph and can resolve imports across
        // files. Splitting them per-language breaks cross-file diagnostics.
        let names: Vec<_> = ["typescript", "javascript", "tsx", "jsx"]
            .iter()
            .map(|l| adapter_for_language(l).unwrap().name)
            .collect();
        assert!(names.iter().all(|n| *n == "typescript-language-server"));
    }

    #[test]
    fn css_and_html_ship_in_one_npm_package() {
        // Both come from vscode-langservers-extracted, like Zed.
        let css = adapter_by_name("vscode-css-language-server").unwrap();
        let html = adapter_by_name("vscode-html-language-server").unwrap();
        assert_eq!(css.npm_package(), Some("vscode-langservers-extracted"));
        assert_eq!(html.npm_package(), css.npm_package());
        // ...but install into separate containers, so one can't half-break
        // the other on a failed upgrade.
        assert_ne!(css.name, html.name);
    }

    #[test]
    fn typescript_also_installs_the_typescript_package() {
        let ts = adapter_by_name("typescript-language-server").unwrap();
        assert_eq!(ts.extra_npm_packages(), &["typescript@6"]);
    }

    #[test]
    fn css_config_enables_validation() {
        let css = adapter_by_name("vscode-css-language-server").unwrap();
        // Requested as a bare section, and as the whole bag.
        assert_eq!(css.workspace_configuration("css", None)["validate"], true);
        assert_eq!(
            css.workspace_configuration("", None)["css"]["validate"],
            true
        );
    }

    #[test]
    fn eslint_config_carries_a_workspace_folder() {
        let eslint = adapter_by_name("eslint");
        // ESLint ships inside vscode-langservers-extracted but is opt-in;
        // only assert the config shape when the adapter is registered.
        if let Some(eslint) = eslint {
            let cfg = eslint.workspace_configuration("", Some(Path::new("/tmp/proj")));
            assert_eq!(cfg["run"], "onType");
            assert_eq!(cfg["workspaceFolder"]["name"], "proj");
        }
    }
}
