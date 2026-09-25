# ezicode Extensions

ezicode loads extensions written in **[Zed](https://github.com/zed-industries/zed)'s
exact format**. An extension is a folder with an `extension.toml` manifest plus
some content directories. If it works in Zed, the declarative parts work here —
you can copy a Zed extension repository in and it just loads.

> **TL;DR** — Put a folder like the examples in
> [`examples/extensions/`](../examples/extensions) into your extensions
> directory (or install it from a Git URL), restart ezicode, and it's live.

---

## Compatibility matrix

| Capability | Zed format | ezicode support |
| --- | --- | --- |
| **Themes** (`themes/*.json`) | ✅ | ✅ Full — themes appear in the theme picker |
| **Icon themes** (`icon_themes/*.json`) | ✅ | ✅ Discovered and exposed |
| **Languages** (`languages/<lang>/config.toml`) | ✅ | ✅ File-type association; highlighting when a matching grammar is built in |
| **Grammars** (`[grammars.*]`) | ✅ | ⚠️ Recognised & listed; native execution of arbitrary WASM grammars is not yet wired |
| **Language servers** (`[language_servers.*]`) | ✅ | ✅ Native launch via the `command`/`args` keys (works today); WASM-driven command generation is the remaining piece |
| **Compiled logic** (`extension.wasm`) | ✅ | 🧪 Loaded, validated & instantiated with WASI behind the `wasm-extensions` feature; `zed:extension` host imports are the remaining piece |

Themes, icon themes and language file-associations are **fully declarative** and
need no compilation, so they are the most portable — the same file that ships in
a Zed theme/language extension works unchanged.

---

## Where extensions live

Installed extensions are stored next to `settings.json`, one folder per
extension:

```
<config-dir>/extensions/installed/<extension-id>/
```

`<config-dir>` is:

| OS | Path |
| --- | --- |
| Linux / BSD | `$XDG_CONFIG_HOME/ezicode` or `~/.config/ezicode` |
| macOS | `~/Library/Application Support/ezicode` |
| Windows | `%APPDATA%\ezicode` |

Print the exact path any time with:

```bash
ezicode --extensions-dir
```

---

## Installing extensions

### From a Git URL (or `owner/repo` shorthand)

```bash
ezicode --install-extension https://github.com/owner/repo
ezicode --install-extension owner/repo          # expands to GitHub
```

This shallow-clones the repository, reads its `extension.toml`, and copies it
into the extensions directory under its `id`.

### From a local folder ("dev extension")

```bash
ezicode --install-extension ./examples/extensions/ezicode-solarized-theme
```

Or just copy/symlink the folder into `.../extensions/installed/` yourself.

### Managing

```bash
ezicode --list-extensions
ezicode --uninstall-extension solarized
```

> **Restart ezicode** after installing/removing so themes and languages are
> re-indexed. The **Extensions** panel (activity bar, or `Ctrl+Shift+X`) lists
> everything currently installed with capability badges.

---

## Authoring an extension

### 1. The manifest — `extension.toml`

```toml
id = "my-extension"          # stable id; also the install folder name
name = "My Extension"
version = "1.0.0"
schema_version = 1
authors = ["Your Name <you@example.com>"]
description = "What it does."
repository = "https://github.com/you/my-extension"
```

Optional sections (same keys as Zed):

```toml
[grammars.mylang]
repository = "https://github.com/you/tree-sitter-mylang"
commit = "<git-sha>"

[language_servers.mylang]
name = "My Language Server"
language = "MyLang"          # or: languages = ["MyLang"]
```

Unknown/newer keys are ignored, so a manifest authored for a later Zed schema
still loads.

### 2. A theme extension

Drop one or more theme **family** files in `themes/`:

```
my-extension/
  extension.toml
  themes/
    my-theme.json
```

Each file is a standard Zed theme document:

```json
{
  "name": "My Theme Family",
  "themes": [
    {
      "name": "My Theme Dark",
      "appearance": "dark",
      "style": {
        "background": "#101317",
        "editor.background": "#101317",
        "editor.foreground": "#d5dae2",
        "text": "#d5dae2",
        "text.muted": "#8a919c",
        "syntax": {
          "comment": { "color": "#6b7280", "font_style": "italic" },
          "keyword": { "color": "#c792ea" },
          "string":  { "color": "#c3e88d" },
          "function":{ "color": "#82aaff" }
        }
      }
    }
  ]
}
```

ezicode understands the full Zed style token set (UI chrome, terminal ANSI
palette, `players[].cursor`, and the `syntax` tokens) and fills any missing
tokens with sensible fallbacks. See the complete working sample in
[`examples/extensions/ezicode-solarized-theme`](../examples/extensions/ezicode-solarized-theme).

### 3. A language extension

```
my-extension/
  extension.toml
  languages/
    mylang/
      config.toml
```

`languages/mylang/config.toml`:

```toml
name = "MyLang"
grammar = "mylang"
path_suffixes = ["ml2", "mylang"]
line_comments = ["// "]
```

Files with those suffixes are now recognised as *MyLang*. Syntax highlighting
lights up automatically **if a Tree-sitter grammar with that name is compiled
into ezicode** (the bundled grammars cover Rust, TS/JS/TSX, Go, HTML, JSON,
Markdown, Zig and more). See
[`examples/extensions/ezicode-nim-lang`](../examples/extensions/ezicode-nim-lang).

#### Wiring a language server (native path)

In Zed, the command that launches a language server is produced by the
extension's WASM component. ezicode also supports a **native** path that works
without WASM: add `command` (a binary resolved on `$PATH`) and optional `args`
to the server entry. These keys are **ignored by Zed**, so your extension stays
cross-compatible.

```toml
[language_servers.mylang]
name = "My Language Server"
language = "MyLang"
command = "mylang-lsp"   # ezicode: launched from $PATH
args = ["--stdio"]       # ezicode: extra arguments
```

When a *MyLang* file is opened, ezicode starts `mylang-lsp --stdio` (if found on
`$PATH`) and wires diagnostics, completion, hover and go-to-definition through
it — exactly like the built-in servers.

### 4. Compiled logic (experimental)

Extensions that ship a `extension.wasm` component (built against
`zed_extension_api`) can be loaded by a `wasmtime`-based host that is compiled in
only when you build with the feature:

```bash
cargo run --features wasm-extensions
```

The host (`app/src/extension/wasm.rs`) builds a `wasmtime` component-model
engine, sets up a **WASI** linker, and loads, validates and instantiates each
`extension.wasm`. Components that only need WASI instantiate cleanly; those that
also import the `zed:extension/*` host interfaces report exactly which imports
are still missing.

The complete, matching WIT for the `zed:extension` world (v0.6.0) is vendored at
[`app/wit/since_v0.6.0/`](../app/wit/since_v0.6.0) — the same files Zed ships —
ready to drive `wasmtime::component::bindgen!`. Implementing those host imports
(github, http-client, nodejs, platform, process, worktree, …) and calling the
guest exports (`language-server-command`, …) is the remaining step to run
WASM-driven extensions end-to-end. The default build does **not** pull in
`wasmtime`.

Until then, use the native `command`/`args` path described above to run language
servers that ship as ordinary binaries.

---

## Trying the bundled examples

```bash
# Theme extension — adds "Solarized Dark" and "Solarized Light" to the picker
ezicode --install-extension ./examples/extensions/ezicode-solarized-theme

# Language extension — associates .nim / .nims / .nimble files with Nim
ezicode --install-extension ./examples/extensions/ezicode-nim-lang

ezicode --list-extensions
# restart ezicode, then open Ctrl+Shift+X (Extensions) and the theme picker
```
