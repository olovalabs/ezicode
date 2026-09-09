//! Language identification for open buffers plus LSP server selection.
//!
//! `language_for` returns the tree-sitter language id used by the highlighter
//! (gpui-component's `LanguageRegistry` — see its `Language` enum for the
//! exact id list) and by the LSP layer. Detection is extension + filename
//! based, with a shebang fallback for extension-less scripts.
//!
//! `lsp_server_for` maps a language to the language server that handles it;
//! the full per-server description (how to install it, how to configure it,
//! which LSP `languageId` to use) lives in `crate::lsp::adapter`.

use std::path::Path;

/// Tree-sitter language id used by the highlighter for this file
/// (`"rust"`, `"python"`, …), or `None` for unrecognized files.
pub fn language_for(path: &Path) -> Option<&'static str> {
    let ext_raw = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    // Extensions are almost always lowercase; only allocate when an
    // uppercase letter actually needs folding (this runs on every render
    // for the status bar's language label).
    let mut folded = String::new();
    let ext = if ext_raw.bytes().any(|b| b.is_ascii_uppercase()) {
        folded = ext_raw.to_ascii_lowercase();
        folded.as_str()
    } else {
        ext_raw
    };
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    match name {
        "Dockerfile" | "Containerfile" => return Some("dockerfile"),
        "Makefile" | "makefile" | "GNUmakefile" => return Some("make"),
        "CMakeLists.txt" => return Some("cmake"),
        "Justfile" | "justfile" => return Some("make"),
        ".bashrc" | ".bash_profile" | ".zshrc" | ".profile" => return Some("bash"),
        _ => {}
    }
    if let Some(lang) = by_extension(ext) {
        return Some(lang);
    }
    // Extension-less scripts: sniff the shebang from the first line.
    shebang_language(path)
}

fn by_extension(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "rs" => "rust",
        "js" | "mjs" | "cjs" => "javascript",
        // Kept distinct from plain JS: the server needs the
        // "javascriptreact" language id to parse JSX syntax.
        "jsx" => "jsx",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "py" | "pyw" => "python",
        "html" | "htm" | "xhtml" => "html",
        "css" => "css",
        "scss" | "sass" => "css",
        "json" => "json",
        "jsonc" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "md" | "markdown" | "mdown" => "markdown",
        "go" => "go",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "inl" => "cpp",
        "cs" => "csharp",
        "java" => "java",
        "rb" => "ruby",
        "sh" | "bash" | "zsh" => "bash",
        "php" => "php",
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        "scala" | "sc" => "scala",
        "lua" => "lua",
        "zig" => "zig",
        "sql" => "sql",
        "r" => "r",
        "xml" | "xsl" | "xsd" | "svg" => "xml",
        "proto" => "proto",
        "graphql" | "gql" => "graphql",
        "ex" | "exs" => "elixir",
        "ejs" => "ejs",
        "erb" => "erb",
        "diff" | "patch" => "diff",
        "cmake" => "cmake",
        "vue" | "svelte" | "astro" => "html",
        "tex" => "latex",
        "dart" => "dart",
        "hs" => "haskell",
        "ml" => "ocaml",
        "fs" | "fsx" => "fsharp",
        "erl" => "erlang",
        "clj" | "cljs" => "clojure",
        "ps1" | "psm1" => "powershell",
        _ => return None,
    })
}

/// Detect the language of an extension-less file from its `#!` shebang.
///
/// Only the first bytes of the file can contain a shebang, so this reads a
/// single 256-byte block instead of the whole file — detection stays O(1)
/// even for huge extension-less files (minified bundles, logs, dumps).
fn shebang_language(path: &Path) -> Option<&'static str> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 256];
    let n = file.read(&mut head).ok()?;
    let head = head.get(..n)?;
    if head.len() < 2 || head[0] != b'#' || head[1] != b'!' {
        return None;
    }
    let line = String::from_utf8_lossy(head).lines().next()?.to_string();
    let lower = line.to_ascii_lowercase();
    if lower.contains("python") {
        Some("python")
    } else if lower.contains("ruby") {
        Some("ruby")
    } else if lower.contains("node") || lower.contains("deno") || lower.contains("bun") {
        Some("javascript")
    } else if lower.contains("bash") || lower.contains("sh") || lower.contains("zsh") {
        Some("bash")
    } else if lower.contains("perl") {
        Some("perl")
    } else if lower.contains("php") {
        Some("php")
    } else if lower.contains("lua") {
        Some("lua")
    } else if lower.contains("elixir") {
        Some("elixir")
    } else if lower.contains("swift") {
        Some("swift")
    } else if lower.contains("go") && line.contains("go run") {
        Some("go")
    } else {
        None
    }
}

/// The name of the language server that handles this language, if any.
///
/// The authoritative table lives in [`crate::lsp::adapter::ADAPTERS`], which
/// also knows how to install and configure each server (Zed keeps the same
/// information in one `LspAdapter` per server rather than in a name map).
#[allow(dead_code)]
pub fn lsp_server_for(lang: &str) -> Option<&'static str> {
    crate::lsp::adapter::adapter_for_language(lang).map(|a| a.name)
}

const TSX_HIGHLIGHT_QUERY: &str = r#"
; Types & Variables
(identifier) @variable

; Properties
(property_identifier) @property
(shorthand_property_identifier) @property
(shorthand_property_identifier_pattern) @property
(private_property_identifier) @property

; Function and method definitions
(function_expression name: (identifier) @function)
(function_declaration name: (identifier) @function)
(method_definition name: (property_identifier) @function)
(pair
  key: (property_identifier) @function
  value: [(function_expression) (arrow_function)])
(assignment_expression
  left: (member_expression property: (property_identifier) @function)
  right: [(function_expression) (arrow_function)])
(variable_declarator
  name: (identifier) @function
  value: [(function_expression) (arrow_function)])
(assignment_expression
  left: (identifier) @function
  right: [(function_expression) (arrow_function)])

; Function and method calls
(call_expression function: (identifier) @function)
(call_expression
  function: (member_expression property: (property_identifier) @function))

; Special identifiers
((identifier) @type (#match? @type "^[A-Z]"))
([
  (identifier)
  (shorthand_property_identifier)
  (shorthand_property_identifier_pattern)
 ] @constant (#match? @constant "^_*[A-Z_][A-Z\\d_]*$"))

((identifier) @variable
 (#match? @variable "^(arguments|module|console|window|document|process)$"))

; Literals
(this) @variable
(super) @variable
[(true) (false) (null) (undefined)] @constant

(comment) @comment

[(string) (template_string)] @string
(regex) @string.special
(number) @number

; JSX Elements & Attributes
(jsx_opening_element name: (_) @tag)
(jsx_closing_element name: (_) @tag)
(jsx_self_closing_element name: (_) @tag)
(jsx_attribute (property_identifier) @attribute)

; Punctuation & Delimiters
[";" (optional_chain) "." ","] @punctuation.delimiter
["-" "--" "-=" "+" "++" "+=" "*" "*=" "**" "**=" "/" "/=" "%" "%="
 "<" "<=" "<<" "<<=" "=" "==" "===" "!" "!=" "!==" "=>"
 ">" ">=" ">>" ">>=" ">>>" ">>>=" "~" "^" "&" "|" "^=" "&=" "|="
 "&&" "||" "??" "&&=" "||=" "??="] @operator

["(" ")" "[" "]" "{" "}"] @punctuation.bracket
(template_substitution "${" @punctuation.special "}" @punctuation.special) @embedded

; Keywords
[
  "as" "async" "await" "break" "case" "catch" "class" "const"
  "continue" "debugger" "default" "delete" "do" "else" "export"
  "extends" "finally" "for" "from" "function" "get" "if" "import"
  "in" "instanceof" "let" "new" "of" "return" "set" "static"
  "switch" "target" "throw" "try" "typeof" "var" "void" "while"
  "with" "yield" "abstract" "declare" "enum" "implements"
  "interface" "keyof" "namespace" "private" "protected" "public"
  "type" "readonly" "override" "satisfies"
] @keyword

; Types
(type_identifier) @type
(predefined_type) @type
(type_arguments "<" @punctuation.bracket ">" @punctuation.bracket)
(required_parameter (identifier) @variable)
(optional_parameter (identifier) @variable)
"#;

/// Initializes and registers high-quality Tree-Sitter language definitions for TSX/TypeScript/JSX.
pub fn init_languages() {
    use gpui_component::highlighter::{LanguageConfig, LanguageRegistry};
    let registry = LanguageRegistry::singleton();

    let tsx_config = LanguageConfig::new(
        "tsx",
        tree_sitter_typescript::LANGUAGE_TSX.into(),
        vec!["html".into(), "css".into(), "javascript".into(), "typescript".into()],
        TSX_HIGHLIGHT_QUERY,
        "",
        tree_sitter_typescript::LOCALS_QUERY,
    );
    registry.register("tsx", &tsx_config);

    // `.jsx` is its own language id (so the LSP layer can send the
    // "javascriptreact" language id), but syntactically it is TSX minus the
    // type annotations — the TSX grammar highlights it correctly.
    registry.register("jsx", &tsx_config);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_extensions() {
        assert_eq!(language_for(Path::new("main.rs")), Some("rust"));
        assert_eq!(language_for(Path::new("app.tsx")), Some("tsx"));
        assert_eq!(language_for(Path::new("x.cs")), Some("csharp"));
        assert_eq!(language_for(Path::new("Dockerfile")), Some("dockerfile"));
        assert_eq!(language_for(Path::new("CMakeLists.txt")), Some("cmake"));
        assert_eq!(language_for(Path::new("Makefile")), Some("make"));
        assert_eq!(language_for(Path::new("unknown.xyz")), None);
    }

    #[test]
    fn test_init_languages() {
        init_languages();
        let registry = gpui_component::highlighter::LanguageRegistry::singleton();
        assert!(registry.language("tsx").is_some());
        assert!(registry.language("jsx").is_some());
    }

    #[test]
    fn jsx_is_its_own_language_id() {
        // So the LSP layer can map it to "javascriptreact".
        assert_eq!(language_for(Path::new("App.jsx")), Some("jsx"));
        assert_eq!(language_for(Path::new("app.js")), Some("javascript"));
    }

    #[test]
    fn web_languages_resolve_to_a_server() {
        assert_eq!(
            lsp_server_for("tsx"),
            Some("typescript-language-server")
        );
        assert_eq!(lsp_server_for("css"), Some("vscode-css-language-server"));
        assert_eq!(lsp_server_for("html"), Some("vscode-html-language-server"));
        assert_eq!(lsp_server_for("json"), Some("json-language-server"));
        assert_eq!(lsp_server_for("rust"), Some("rust-analyzer"));
        assert_eq!(lsp_server_for("plaintext"), None);
    }
}
