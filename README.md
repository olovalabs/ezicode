# Olova Editor

Native code editor: file tree + [gpui-component](https://github.com/longbridge/gpui-component) `Input` in **code editor** mode (rope buffer, tree-sitter highlighting, search).

```
cargo run -p app
```

Starts with **no project loaded** (VS Code-style welcome screen):

- **Open Folder** — pick a folder to browse in the explorer
- **Open File** (`Ctrl+O`) — open a single file
- **New File** (`Ctrl+N`) — untitled buffer; `Ctrl+S` opens Save As
- **Ctrl+S** save · **Ctrl+F** search (from the editor library)

## Syntax highlighting — Zed's exact palette

Default theme: **GitHub Dark** (from
[PyaeSoneAungRgn/github-zed-theme](https://github.com/PyaeSoneAungRgn/github-zed-theme)),
plus Ayu and Gruvbox families. The JSONs in `app/assets/themes/` are verbatim
Zed theme files consumed two ways:

1. UI chrome colors → `src/theme/colors.rs` tokens
2. the full `syntax` token table (46 entries) + editor colors → forwarded
   untouched into gpui-component's `HighlightTheme`, whose schema is
   explicitly Zed-compatible. Tree-sitter captures (`keyword`, `function`,
   `comment.doc`, …) are painted with exactly Zed's colors; switching themes
   via **View → Theme** re-skins chrome *and* syntax together.

## Language servers

Errors, warnings, completions, hover, go-to-definition and quick fixes are
live, and the Node-based servers **install themselves on first use** — no
`npm i -g`, no PATH setup. The design follows Zed's, split the same three ways:

| Zed | here | responsibility |
|---|---|---|
| `crates/node_runtime` | `src/lsp/node.rs` | find Node, `npm install` servers on demand |
| `crates/languages` (`LspAdapter` impls) | `src/lsp/adapter.rs` | per-server binary, init options, settings, language ids |
| `crates/lsp`, `crates/project/src/lsp_store.rs` | `src/lsp/client.rs` | JSON-RPC transport, document sync, diagnostics, providers |

Open `App.tsx` in a project with nothing installed and you get:

1. `lang.rs` → `tsx` → the `typescript-language-server` adapter.
2. Nothing installed, so a background `npm install typescript-language-server
   typescript@6` runs into a **private** container dir
   (`~/.local/share/olova-editor/language-servers/<server>`, mirroring Zed's
   `~/.local/share/zed/languages/`) — never a global prefix, never your
   project. The status bar shows `◌ installing…`.
3. Install finishes → the server starts and every already-open buffer it
   handles is attached, so you don't have to reopen the file you're looking at.
4. `initialize` → the client answers the server's `workspace/configuration`
   request from the adapter → diagnostics start flowing.
5. A version marker makes every later launch instant and offline-clean.

Auto-installed (Node): `typescript-language-server` (ts/js/tsx/jsx, one
process for all four so imports resolve across files), `vscode-css-language-server`,
`vscode-html-language-server`, `json-language-server`, `yaml-language-server`,
`bash-language-server`, `docker-langserver`.
Looked up on PATH (toolchain-owned): rust-analyzer, gopls, basedpyright,
clangd, zls, lua-language-server, taplo, intelephense, ruby-lsp.

### Two things that are easy to get wrong

Both were verified against the real servers rather than assumed:

* **`workspace/configuration` must be answered.** The VS Code-derived servers
  ask for their settings right after `initialized` and publish *nothing* until
  they get a reply. Replying `-32601` (the obvious "we implement no server
  requests" default) yields **0 diagnostics for CSS and HTML** while
  TypeScript still works — which is exactly the "works for some languages, not
  others" symptom. Answering with the adapter's config blob takes both from
  0 → 2 diagnostics on a broken fixture. This is why
  `capabilities.workspace.configuration = true` and `handle_server_request`
  exist.
* **Request ids are `integer | string`.** A client that only parses integer
  ids silently drops string-id requests and deadlocks the servers that use
  them.

Note that `vscode-html-language-server` validates *embedded* CSS/JS rather
than HTML tag nesting — it does not flag an unclosed `<div>`, matching VS Code.

## Fonts — Zed's shipped set

`app/assets/fonts/` contains the exact TTFs Zed ships: **IBM Plex Sans**
(UI, 4 styles) and **Lilex** (code buffer, 4 styles), with their licenses
(`LICENSE-IBMPlex.txt`, `LICENSE-Lilex-OFL.txt`). At startup every embedded
TTF is registered with GPUI's text system (`load_embedded_fonts`, mirroring
Zed's approach); the widget library is then pointed at them
(`mono_font_family = Lilex` for the editor, `font_family = IBM Plex Sans`
for UI) and re-applied after every theme switch.

## Explorer performance and navigation

The file tree follows the same shape as Zed's project panel: directory levels
are lazy-loaded, visible rows are cached in a flat snapshot, and GPUI's
`uniform_list` only lays out rows inside the viewport. Refreshes and filesystem
notifications merge one affected directory level; they do not recursively
rescan every expanded folder or block the UI while reading disk. Empty folders
are cached correctly, so repeatedly opening them does not trigger another scan.

The explorer also supports auto-reveal for opened files, arrow-key navigation
(Up/Down/Home/End), Left/Right folder navigation, Enter to open/toggle, and
quick New File, New Folder, Refresh, and Collapse All actions in the project
header. Right-click rows for the full file-operation menu. Files and folders
can be created inline in any expanded directory, renamed with F2, deleted with
Delete, copied/cut/pasted with Ctrl+C/Ctrl+X/Ctrl+V, and moved by dragging onto
a folder (including the project root). Moves and folder renames update open
tabs, diagnostics, and the selected path; invalid destinations, collisions, and
moves into a descendant are rejected without touching the filesystem.

## File icons

Explorer icons come from Zed's default icon theme — SVGs in
`app/assets/file_icons/` and the extension→icon tables in `src/file_icons.rs`,
ported from Zed's `crates/theme/src/icon_theme.rs`. Unknown extensions fall
back to Zed's default file icon. Activity-bar glyphs are Zed's own icons
(`app/assets/ui_icons/`).

Note: colorful file/folder icons are rendered with `gpui::img()` because
gpui's `svg()` element paints a single tint color; UI glyphs intentionally use
tinted `svg()` so they follow the theme.
