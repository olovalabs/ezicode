# Themes, Syntax Highlighting & Code Formatting — Architecture

**Easy Code (ezicode) · Rust + GPUI + Tree-sitter**

This document is the system architecture for the three subsystems that make an
editor *feel* like Zed: the theme engine, syntax highlighting, and code
formatting. It describes how Zed itself architected each system, how ezicode
implements the same model, what is already in this repository, and the ordered
roadmap for the rest. Code snippets are real — they are either lifted from this
codebase or written to drop into it.

---

## Table of contents

1. [Design philosophy](#1-design-philosophy)
2. [Theme system architecture](#2-theme-system-architecture)
3. [Syntax highlighting](#3-syntax-highlighting)
4. [Code formatting](#4-code-formatting)
5. [Visual & UI fidelity](#5-visual--ui-fidelity)
6. [Step-by-step implementation guide](#6-step-by-step-implementation-guide)
7. [Testing strategy](#7-testing-strategy)

---

## 1. Design philosophy

Five rules, borrowed from Zed, that every decision below follows:

| Rule | Consequence in this codebase |
| --- | --- |
| **The editor core never knows about themes.** | `InputState` renders whatever `HighlightStyle` values it is handed. Color decisions live in `app/src/theme/`; application of colors lives in `Workspace::apply_theme`. The buffer, gutter, and completion UI are theme *consumers*, nothing more. |
| **Data files over code.** | Themes are Zed-schema JSON, not Rust enums. Adding a theme must never require a recompile. |
| **Parsing is cheap, so do it lazily and cache it.** | Syntax styles are deserialized once per `Theme` (`OnceLock`), not on every switch. Tree-sitter trees are incrementally repaired, never re-parsed from scratch. |
| **Style is layered, never absolute.** | A theme file may define 3 colors or 300. Missing tokens fall back through a derivation chain (theme file → contextual derivation → base palette), mirroring Zed's `refine_theme_style`. |
| **The UI thread is sacred.** | Formatting requests and file writes run on the background executor; the UI thread only ever applies results. |

---

## 2. Theme system architecture

### 2.1 How Zed does it

Understanding Zed's model is a prerequisite for copying its ergonomics:

- **Theme files are JSON** adhering to the schema at
  `https://zed.dev/schema/themes/v0.2.0.json`. A file is a *theme family*: a
  `name`, an `author`, and a `themes` array, so one file can bundle a dark and
  a light variant [1](https://zed.dev/blog/user-themes-now-in-preview).
- **User themes live in a config directory** — `~/.config/zed/themes` on
  Linux — and "will be available in the theme selector the next time Zed
  loads" [1](https://zed.dev/blog/user-themes-now-in-preview). Same-named user
  themes override built-ins.
- Each theme has an **`appearance`** (`light` | `dark`) which also drives
  native widget styling, and a **`style` object** split into: UI chrome
  colors (`border`, `surface.background`, `text.muted`, `panel.background`, …),
  editor colors (`editor.background`, `editor.line_number`, …), terminal ANSI
  colors, collaborator ("player") cursor colors, and a **`syntax` object**
  mapping Tree-sitter capture names to `{ color, font_style, font_weight }` [2](https://zed.dev/docs/extensions/themes).
- Internally, Zed keeps themes in a global `ThemeRegistry`, and a theme is
  resolved into two distinct data structures: **`ThemeStyle`** (UI chrome) and
  **`SyntaxStyle`** (the syntax token map) — the same split this codebase uses.
  User `ThemeContent` JSON is *refined* against a base light/dark palette, so
  partial themes are completed rather than rejected.

### 2.2 ezicode's architecture

The theme system is three layers with a one-way dependency arrow. The editor
core is at the bottom and knows nothing about the layers above it:

```text
┌──────────────────────────────────────────────────────────────────────┐
│ L3 · CONSUMERS            Workspace::apply_theme, Settings UI,       │
│                           terminal tabs, status bar, welcome screen  │
│                           (read resolved Colors + HighlightTheme)    │
├──────────────────────────────────────────────────────────────────────┤
│ L2 · REGISTRY             theme::all() — OnceLock<Vec<Theme>>         │
│                           · embedded families (rust-embed)           │
│                           · user dir merge (~/.config/ezicode/themes) │
│                           · override-by-name, deterministic order    │
├──────────────────────────────────────────────────────────────────────┤
│ L1 · RESOLUTION           theme::parse_theme_file / parse_family     │
│                           · KEY_MAP: Zed style key → Colors field    │
│                           · contextual derivation chain              │
│                           · refine_from_base (light/dark base)       │
│                           · HighlightTheme (syntax token styles)     │
│                           · terminal ANSI palette derivation         │
├──────────────────────────────────────────────────────────────────────┤
│ L0 · EDITOR CORE          InputState, Rope, GPUI text system          │
│                           (renders styles; has no color logic)       │
└──────────────────────────────────────────────────────────────────────┘
```

One `Theme` value is the *resolved* form of one theme entry — everything any
consumer needs, computed once at load:

```rust
// app/src/theme/mod.rs
#[derive(Clone, Debug)]
pub struct Theme {
    pub name: String,          // selection key (settings.json, command palette)
    pub appearance: String,    // "light" | "dark" → widget theme mode
    pub colors: Colors,        // 33 UI-chrome tokens (u32 RGBA, Copy)
    pub terminal_palette: gpui_terminal::ColorPalette, // 16 ANSI + 256 ext

    hl_json: String,           // Zed syntax JSON for the highlighter
    hl_cache: OnceLock<HighlightTheme>, // parsed once, cloned thereafter
}
```

### 2.3 The resolution pipeline (L1)

Every theme file — embedded or user — goes through the same four stages:

```text
Zed theme JSON
   │
   ① EXTRACTION      KEY_MAP maps 33 Zed style keys onto Colors fields.
   │                 ("tab.active_background" → colors.tab_active_bg, …)
   ② DERIVATION      Contextual fallbacks for related tokens:
   │                 tab_bar      ← toolbar | panel | surface | background
   │                 tab_active_bg← editor_bg | background
   │                 terminal_bg  ← background | tab_bar …
   ③ REFINEMENT      refine_from_base(appearance): any token still missing
   │                 is filled from the base light/dark palette (Zed's
   │                 refine_theme_style). A 3-color theme file still
   │                 renders every surface correctly — and never leaks
   │                 the magenta MISSING sentinel into the UI.
   ④ SATELLITES      · build_highlight_theme → HighlightTheme JSON
                     · build_terminal_palette → ANSI-16 + cursor + fg/bg
                       (terminal.* keys, falling back to editor.*/text)
```

The refinement step is the piece that makes third-party themes safe. Zed
behaves the same way: the JSON `ThemeContent` is refined against base colors
before it becomes an active theme. In ezicode the base palettes are the exact
token sets of the shipped GitHub Dark / GitHub Light themes, with a test
(`base_palettes_match_the_shipped_github_themes`) that fails if the shipped
themes and the base constants ever drift apart.

### 2.4 Theme sources & precedence (L2)

```rust
// app/src/theme/mod.rs — the registry, in full
pub fn all() -> &'static [Theme] {
    static THEMES: OnceLock<Vec<Theme>> = OnceLock::new();
    THEMES.get_or_init(|| {
        let mut out = Vec::new();
        // 1. Embedded families (compile-time, zero disk I/O)
        for file in THEME_FILES { /* parse_family(...) into `out` */ }
        // 2. User families: ~/.config/ezicode/themes/*.json
        //    (created on first run so it is discoverable)
        let user_dir = user_themes_dir();
        let _ = std::fs::create_dir_all(&user_dir);
        load_user_themes_from(&user_dir, &mut out);
        out
    })
}
```

`load_user_themes_from` gives user themes **Zed's exact semantics**:

- Files are scanned sorted by name → deterministic ordering between runs.
- A user theme with the same **name** as a built-in *replaces* it in place
  (override), new names are appended.
- Both file layouts are accepted: a full family
  (`{"name": …, "themes": […]}`) and a bare single theme
  (`{"name": …, "style": …}`), which is wrapped into a one-entry family so
  both layouts share the identical ①–④ pipeline.
- Malformed files are skipped silently — a bad user file can never take the
  editor down or break the embedded themes.

Because `THEME_FILES` is a *list of names* next to a rust-embed *folder* asset
(`app/assets/themes/`), shipping a new built-in family is: drop the JSON in the
folder, add its name to the list, extend the count assertion in the tests.
No Rust code changes.

### 2.5 Dynamic switching (L3)

Switching is a single function, `Workspace::apply_theme`, driven by the
Settings page cards, the command palette (`theme.github_dark`, …), and startup
(`settings.json → workbench.colorTheme` matched by name). It performs, in
order:

```rust
// app/src/workspace/mod.rs (annotated excerpt)
pub(crate) fn apply_theme(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
    // 1. Resolve + persist the choice by name (index is internal only).
    self.settings.workbench_color_theme = th.name.to_string();
    let _ = self.settings.save();

    // 2. Flip the widget library's appearance mode (drives native widgets).
    gpui_component::Theme::change(mode, Some(window), cx);

    // 3. Swap the syntax token table every editor reads.
    gpui_component::Theme::global_mut(cx).highlight_theme = Arc::new(th.highlight_theme());

    // 4. Re-point the widget library at the bundled fonts.
    crate::assets::sync_component_fonts(cx);

    // 5. Push the derived ANSI palette into every live terminal tab.
    for tab in &self.terminal_tabs {
        tab.update(cx, |term, cx| term.set_theme(&th.terminal_palette, cx));
    }
    cx.notify();  // one re-render; GPUI repaints GPU-side
}
```

Every consumer reads theme data through indirections that `apply_theme`
rewrites (the `gpui_component::Theme` global and `Workspace::theme()`), so a
switch is O(1) beyond the repaint — no per-editor recomputation, no restart.

### 2.6 Sample theme file

A complete, annotated sample family ships in [`docs/sample-theme.json`](sample-theme.json)
(two variants: **Ezi Dusk** dark and **Ezi Dawn** light). It is a valid Zed
theme — drop it in `~/.config/zed/themes/` and Zed will load it too. The
skeleton it demonstrates:

```jsonc
{
  "$schema": "https://zed.dev/schema/themes/v0.2.0.json",
  "name": "My Theme Family",          // family name (shown in Zed's UI)
  "author": "you",
  "themes": [
    {
      "name": "My Theme Dark",        // the selection key — must be unique
      "appearance": "dark",           // "light" | "dark" (widgets follow)
      "style": {
        // ── UI chrome (33 tokens consumed by ezicode; Zed consumes more) ──
        "background": "#101418ff",
        "surface.background": "#0b0e12ff",
        "border": "#2a323cff", "border.variant": "#1f262eff",
        "text": "#d6dde8ff", "text.muted": "#8a94a6ff", "text.accent": "#6cb6ffff",
        "panel.background": "#0b0e12ff", "title_bar.background": "#0b0e12ff",
        "status_bar.background": "#0b0e12ff", "tab_bar.background": "#0b0e12ff",
        "tab.active_background": "#101418ff", "tab.active_foreground": "#d6dde8ff",
        // ── Editor ────────────────────────────────────────────────────────
        "editor.background": "#101418ff", "editor.foreground": "#d6dde8ff",
        "editor.active_line.background": "#1a2028ff",
        "editor.line_number": "#414b58ff", "editor.active_line_number": "#8a94a6ff",
        // ── Terminal (falls back to editor.* if omitted) ──────────────────
        "terminal.background": "#0b0e12", "terminal.foreground": "#d6dde8",
        "terminal.ansi.black": "#0b0e12",  /* … 15 more ANSI colors … */
        // ── Syntax: Tree-sitter capture name → style ─────────────────────
        "syntax": {
          "comment": { "color": "#5c6773ff", "font_style": "italic" },
          "keyword": { "color": "#c792eaff" },
          "string":  { "color": "#c3e88dff" },
          "function": { "color": "#82aaffff" },
          "type":    { "color": "#ffcb6bff" },
          "number":  { "color": "#f78c6cff" }
          // … 40 more tokens, see docs/sample-theme.json for the full list
        }
      }
    }
  ]
}
```

**The minimum viable theme is three keys** (`background`,
`editor.background`, `editor.foreground`) — everything else is refined from
the base palette for the theme's `appearance`.

### 2.7 Roadmap: live reload & name-keyed state (Phase 2)

Two architectural upgrades, deliberately sequenced *after* the current
milestone because both touch the same invariant (`theme_ix` is an index into a
list built once per process):

1. **File-watcher hot reload.** The `notify` crate is already a dependency
   (the file explorer uses it). Watch `user_themes_dir()`, rebuild the list,
   re-resolve the active theme *by name*, and re-run `apply_theme`. Requires
   changing `theme::all()` from `OnceLock<Vec<Theme>>` to
   `RwLock<Arc<[Theme]>>` — callers keep compiling because `Arc<[Theme]>`
   derefs to `[Theme]`, but `Workspace::theme()` must stop returning
   `&'static Theme`.
2. **Name-keyed active theme.** Replace `theme_ix: usize` with
   `theme_name: SharedString`. This makes hot reload, settings round-trips and
   override replacement all index-stable, and is the prerequisite for a Zed-style
   theme *selector modal* with live preview-on-hover.

---

## 3. Syntax highlighting

### 3.1 Why Tree-sitter (and what it buys over regex highlighting)

Zed uses Tree-sitter grammars, and so does ezicode — with the same three
payoffs:

1. **Accuracy.** Highlights come from an incremental parse *tree*, so a
   `string` spanning 40 lines is one node — no regex can do that. Errors in
   one region don't cascade: Tree-sitter produces error-recovery nodes and
   keeps the rest of the tree valid.
2. **Incrementality.** After an edit, the tree is *repaired* with
   `InputEdit` (byte ranges: start/end positions + new end) in time
   proportional to the change, not the file. A keystroke in a 100k-line file
   costs microseconds of re-parse.
3. **Query-driven, data-oriented styling.** A language is a grammar crate +
   a **highlight query** (an S-expression pattern file mapping tree nodes to
   capture names like `@keyword`). Styling lives in the *theme*, matching
   lives in the *query* — the same separation as the theme system.

### 3.2 ezicode's pipeline

```text
 keystroke ──► InputState (Rope buffer)
                 │  edit → tree_sitter::InputEdit(byte ranges)
                 ▼
         SyntaxHighlighter (components/src/highlighter/highlighter.rs)
                 │  ① parser.parse(.., Some(&old_tree))   — incremental repair
                 │  ② QueryCursor::matches(HIGHLIGHTS_QUERY, tree, rope)
                 │     captures: (node, "@function") …
                 │  ③ Injection queries: markdown code fences re-parse with
                 │     the embedded language's grammar (html/css/js inside md)
                 │  ④ LOCALS_QUERY distinguishes definitions vs references,
                 │     so `foo` at a definition site can render differently
                 ▼
         sum_tree<HighlightItem>   — ordered by byte range, O(log n) queries
                 │  styles(range) → Vec<(Range<usize>, HighlightStyle)>
                 ▼
         GPUI text system — runs of styled glyphs, shaped & drawn GPU-side
```

Performance-critical details already in the implementation:

- The buffer is a **rope** (`Rope` from the `ropey` crate via gpui-component);
  the query engine reads text through a `ChunkCursor` without copying the file
  into a contiguous string.
- Highlight ranges accumulate in a **`sum_tree`** keyed by byte range —
  viewport queries are `O(log n + k)` for `k` visible tokens, so scrolling a
  huge file never re-queries the whole tree.
- The highlighter is created lazily per editor and *reset*, not recreated,
  when the buffer's language changes (`set_highlighter`).

### 3.3 The language registry

`app/src/lang.rs` is the single source of truth for language identity:

```rust
pub struct LanguageInfo {
    pub id: &'static str,                    // "tsx", "rust", …
    pub name: &'static str,                  // status-bar label
    pub extensions: &'static [&'static str], // detection
    pub lsp_server: Option<&'static str>,    // LSP adapter mapping
    pub icon: &'static str,                  // file_icons/file_type_*.svg
}
```

Detection order for a path: exact filename matches (`Dockerfile`,
`CMakeLists.txt`, rc-files) → extension (ASCII-case-folded, allocation-free
for the common lowercase case) → **shebang sniffing** (one 256-byte read, so
it is O(1) even for multi-gigabyte extension-less files). Grammar resolution
happens in `gpui_component::highlighter::LanguageRegistry`; `lang::init_languages`
registers TSX/JSX with a hand-tuned highlight query (see §3.5).

### 3.4 Adding a language — the full recipe

To add language `X` (example: Python, which ships via gpui-component):

1. **Grammar crate.** Add `tree-sitter-python = "…"`, or use a language the
   vendored `gpui-component` already bundles (`components/src/highlighter/languages/`
   has rust, go, javascript, typescript, tsx, json, html, css, markdown, zig,
   elixir, …).
2. **Identity.** One `LanguageInfo` entry in `LANGUAGES` (id, name,
   extensions, LSP server, icon). Detection, status bar, file icons, and the
   LSP adapter table all read from this single row.
3. **Register the grammar.** If not pre-registered by gpui-component:

   ```rust
   // app/src/lang.rs
   let config = LanguageConfig::new(
       "python",
       tree_sitter_python::LANGUAGE.into(),
       vec![],                                    // injection languages
       tree_sitter_python::HIGHLIGHTS_QUERY,      // capture patterns
       tree_sitter_python::INJECTIONS_QUERY,      // e.g. f-string → …
       "",                                       // locals query (optional)
   );
   LanguageRegistry::singleton().register("python", &config);
   ```

4. **Syntax mapping.** Nothing to do — the theme's `syntax` object already
   styles captures by *name* (`@keyword`, `@string`…). This is the payoff of
   the query/theme split: languages and themes are added orthogonally.
5. **Tests.** Extend `detects_extensions` and the LSP mapping test.

### 3.5 Authoring highlight queries

The TSX query in `app/src/lang.rs` (`TSX_HIGHLIGHT_QUERY`) is the worked
example in this repo. Patterns worth copying:

```scheme
; Definition-site vs call-site functions (needs the tree, not regex):
(function_declaration name: (identifier) @function)
(call_expression function: (identifier) @function)
(call_expression
  function: (member_expression property: (property_identifier) @function))

; Predicates on captured text — "capitalized identifier = type":
((identifier) @type (#match? @type "^[A-Z]"))
; SCREAMING_CASE = constant:
((identifier) @constant (#match? @constant "^_*[A-Z_][A-Z\\d_]*$"))

; JSX tags, template substitutions, every operator — see the full query.
```

Two rules of thumb: prefer *few, structural* patterns over many textual ones,
and test queries against real code (Tree-sitter's `query` CLI or a scratch
test) — a pattern that silently fails is worse than a missing one.

### 3.6 Performance budget

| Operation | Budget | Mechanism |
| --- | --- | --- |
| Keystroke → repainted highlights | < 2 ms | incremental `InputEdit` repair + `sum_tree` range query |
| Theme switch → repaint | one frame | swap `Arc<HighlightTheme>`, single `cx.notify()` |
| Open 8 MB file | no highlight | the open path caps at 8 MB and falls back to plain text |
| First paint of viewport | O(visible) | lazy highlighter creation, viewport-driven query |

Anti-patterns to avoid: re-running queries on the full tree per frame
(use the `sum_tree`), copying the rope into a `String` per query (use the
chunk cursor), and creating a `Query` per editor (the registry compiles
queries once and shares them behind an `Arc`).

---

## 4. Code formatting

### 4.1 How Zed does it

Zed's formatting model, from its configuration docs [3](https://zed.dev/docs/configuring-languages):

- **`format_on_save`: `"on" | "off"`** — per-language or global.
- **`formatter`** — either `"language_server"` (LSP
  `textDocument/formatting`) or an **external command** with argument
  templating, e.g. `prettier --stdin-filepath {buffer_path}`; a formatter may
  also be a *chain* (a code action like `source.fixAll.eslint` followed by an
  external formatter), all applied on save.
- Formatting runs *before* the file hits disk, and the manual command
  (`editor: format`) is always available.

The essential UX properties: the save is never blocked or lost if a formatter
is slow or absent, formatting must not clobber concurrent typing, and the
mechanism is pluggable per language without the editor hard-coding any tool.

### 4.2 ezicode's architecture

Formatting is layered exactly like Zed's, with LSP as the (current) provider
layer and a clean seam for external formatters:

```text
                    ┌──────────────────────────────────────────┐
  Ctrl+S ──────────►│ Workspace::save                          │
                    │  format_on_save == On?                    │
                    │   └─ client_for(language)?                │
                    │        └─ format_then_save(path, editor,  │
                    │             client, window, cx)           │
                    │   (any miss ⇒ plain write_file_async)     │
                    └───────────────┬──────────────────────────┘
                                    ▼
                 background executor (UI never blocks)
                    LspClient::format_document(path, text, tab_size)
                        · syncs the buffer via didChange
                        · textDocument/formatting
                        · FormattingOptions { tab_size: editor.tabSize }
                                    ▼
                    apply_lsp_edits (guarded, see §4.3)
                                    ▼
                    write_file_async — the normal save path:
                    dirty-flag clear, LSP didSave, git poke,
                    settings reload if settings.json
```

The three entry points:

| Trigger | Path | Setting |
| --- | --- | --- |
| **Ctrl+S** | `save` → optionally formats first | `editor.formatOnSave` |
| **Shift+Alt+F** | `format_document` (explicit command) | always available |
| **Auto-save** (`afterDelay`/`onFocusChange`) | `save_tab_quiet` — deliberately *no* formatting | — |

Auto-save skips formatting on purpose: with `afterDelay` the save fires every
second *mid-typing*, and applying formatter rewrites while the user types
fights them for the buffer (a known hazard in editors that do this). When the
async format-then-save path is unified for the quiet path (Phase 3), the
`onFocusChange` mode can opt in — the guard in §4.3 already makes it safe.

### 4.3 The stale-snapshot guard

`format_then_save` snapshots the buffer, asks for edits off-thread, then
applies them. Between snapshot and response the user may keep typing. The
guard makes that race harmless:

```rust
// app/src/workspace/mod.rs (excerpt)
let saved_text: Option<String> = editor_weak
    .update_in(cx, |state, window, cx| {
        let current = state.value().to_string();
        match edits {
            // Apply only if the buffer is still the snapshot we formatted.
            Some(edits) if !edits.is_empty() && current == text => {
                state.apply_lsp_edits(&edits, window, cx);
                Some(state.value().to_string())
            }
            // Stale (user typed) or nothing to do: save what's there now.
            _ => Some(current),
        }
    })
    .ok()      // tab closed mid-flight → nothing to save
    .flatten();
```

Invariants: **a save is never lost** (the current text is always written),
**formatting never clobbers typing** (stale edits are dropped), and **a closed
tab never resurrects** (the `WeakEntity` update fails softly).

### 4.4 Configuration

```jsonc
// settings.json
{
  "editor.formatOnSave": "on",   // "off" | "on"  (aliases: true/false, enabled/…)
  "editor.tabSize": 4            // forwarded as LSP FormattingOptions.tab_size
}
```

`editor.tabSize` matters: servers like rust-analyzer, clangd and
typescript-language-server honor it, so the formatter and the editor's own
indentation now agree. Both the manual command and format-on-save pass it.

### 4.5 Roadmap (Phase 3): external formatters & code actions

Zed's `formatter: { external: { command, arguments } }` maps cleanly onto the
existing seam. Design sketch — a provider chain resolved per language:

```rust
enum Formatter {
    LanguageServer,                       // current behavior
    External { command: String, arguments: Vec<String> },  // prettier, rustfmt…
    CodeAction(&'static str),             // "source.fixAll", "source.organizeImports"
}

// settings.json (proposed):
// "formatter": {
//   "default": "languageServer",
//   "overrides": { "JavaScript": { "external": { "command": "prettier",
//                    "arguments": ["--stdin-filepath", "{buffer_path}"] } } }
// }
```

Evaluation rules: run on the background executor with a timeout (the LSP
client already enforces `REQUEST_TIMEOUT`); feed the buffer on stdin and take
stdout (the `{buffer_path}` placeholder preserves file-type inference);
discover binaries on `PATH` once and cache; on failure, fall through to the
next provider — never block the save. Range formatting
(`textDocument/rangeFormatting`) slots into the same pipeline for
format-selection workflows.

---

## 5. Visual & UI fidelity

What makes Zed *look* like Zed is a small set of disciplined rules. Each maps
to a concrete token or value in ezicode:

### 5.1 Palette structure — the elevation ladder

Zed's chrome is built from a strict **background elevation hierarchy**, and
every theme in this repo encodes it:

```text
background  <  surface.background  <  elevated_surface.background  <  element.*
   └─ editor + toolbar sit on `background`/`editor.background`
   └─ sidebars, panels, tab bars sit on `surface`/`panel` (darker in dark themes)
   └─ popovers, menus sit on `elevated_surface`
   └─ hovered/selected rows add `element.hover`/`element.selected` on top
```

Text follows the same discipline: `text` → `text.muted` →
`text.placeholder`/`text.disabled`; borders: `border` → `border.variant` →
`border.transparent`. **Accent color (`text.accent`) is spent sparingly** —
active states, focused borders, links — never for large fills.

### 5.2 Typography

| Property | Zed | ezicode |
| --- | --- | --- |
| Code font | Zed Plex Mono | **Lilex** (bundled, 4 styles) |
| UI font | Zed Plex Sans | **IBM Plex Sans** (bundled) |
| Code size | ~15 px | `14.5` px default (`editor.fontSize`, Ctrl+=/- live zoom) |
| UI size | 16 px (`ui_font_size`) | `14` px default (`ui_font_size`, ±1 px live) |
| Line height | 1.5 | GPUI default ~1.5 line_height |

`ui_font_size` is the interface's scale, not just a text size: it becomes the
window's `rem` (`ui::scale`), and everything scalable — sidebar rows, file
icons, indentation, gaps — is expressed in rems, so one setting resizes the
whole UI at once. Font *families* stay with `sync_component_fonts`, which
deliberately leaves `font_size` alone so the two paths cannot fight over it.

Fonts are embedded TTFs registered with the GPUI text system at launch
(`assets::load_embedded_fonts`) — no system-font dependency, identical
rendering everywhere.

### 5.3 Editor surface & gutter

- **Active line highlight**: a barely-there tint (`editor.active_line.background`,
  e.g. `#1a2028` on `#101418`) — present, never noisy.
- **Line numbers**: dim (`editor.line_number`), with the cursor line promoted
  to `editor.active_line_number` (brighter, often the muted text color).
- **Gutter = line numbers only.** No permanent badges, no icons in the gutter;
  diagnostics render as squiggles *in* the text, git status as color in the
  explorer/diff, never as gutter clutter.
- **Cursor**: a narrow bar (2 px) in `players[0].cursor`; selections are the
  accent color at low alpha (e.g. `#6cb6ff40`) — tinted, never opaque.

### 5.4 Chrome minimalism

- **Flat surfaces, hairline borders.** 1 px `border.variant` between panes;
  no gradients, no drop shadows (GPUI has them; Zed almost never uses them).
- **Small radii** (4–6 px) on interactive elements; nothing pill-shaped
  except tiny status chips.
- **Active tab melts into the editor**: `tab.active_background ==
  editor.background` with no bottom border — the classic Zed/VS Code signal
  for "this surface continues into the buffer".
- **Icon-only activity bar** with muted icons that accent on hover/active.
- **One status bar, left/right justified**, muted text, tiny chips — it
  reports (git branch, LSP state, language, cursor position), it does not
  decorate.
- **Density over air**: 12–14 px paddings, 6–10 px gaps; Zed packs
  information tightly rather than spreading it out.

### 5.5 Syntax color principles

- **Hue-separated roles**: keyword/string/number/type/function each own a
  distinct hue band; *variables and punctuation stay near-neutral* so the
  structure pops and identifiers stay calm (see the palettes in
  `app/assets/themes/*.json` or `docs/sample-theme.json`).
- **At most one emphasis per token** — italic comments, bold titles; never
  bold-italic-color soup.
- **Both appearances are first-class**: every family ships dark *and* light
  variants with independently tuned palettes (not inverted dark).

---

## 6. Step-by-step implementation guide

The order below is the dependency order — each step only uses machinery from
the steps above it. Status reflects this repository.

### Phase A — Theme engine foundation ✅ (in this repo)

1. **Define the token model.** `theme::Colors` — 33 `u32` RGBA fields, `Copy`,
   one field per consumed Zed style key (`theme/colors.rs`).
2. **Write the extractor.** `KEY_MAP` (Zed key → field) + `parse_hex`
   (6- and 8-digit hex). *Ship a test that every embedded theme extracts
   completely.*
3. **Write the derivation chain.** Tab bar ← toolbar/panel/surface/background;
   active tab ← editor; terminal ← background. These encode *taste* as code.
4. **Add base-palette refinement.** `BASE_DARK`/`BASE_LIGHT` +
   `refine_from_base(appearance)`; test that shipped themes are a no-op
   through it. This is what makes arbitrary third-party themes safe.
5. **Derive satellites.** `build_highlight_theme` (map Zed style keys → the
   highlighter's `HighlightTheme` JSON) and `build_terminal_palette`
   (ANSI-16 + cursor + fg/bg with editor.* fallbacks).
6. **Cache the parsed highlight theme.** `OnceLock<HighlightTheme>` inside
   `Theme`; `highlight_theme()` clones from cache.

### Phase B — Theme delivery & switching ✅ (in this repo)

7. **Embed default family** (`app/assets/themes/github.json` via rust-embed)
   — GitHub (9 themes, default GitHub Dark).
8. **Load user themes** from `~/.config/ezicode/themes/*.json`:
   sorted scan, both file layouts, override-by-name, silent skip on malformed
   input (`parse_theme_file`, `load_user_themes_from`).
9. **Apply a theme at runtime** (`Workspace::apply_theme`): persist by name,
   flip widget mode, swap `HighlightTheme`, re-point fonts, push ANSI palette
   to terminals, one `cx.notify()`.
10. **Expose selection surfaces**: settings-page theme cards with color
    swatches, command-palette entries, startup resolution from
    `settings.json → workbench.colorTheme` (name-matched, index-free).

### Phase C — Syntax highlighting ✅ (in this repo)

11. **Adopt a rope-backed, tree-sitter-driven highlighter** with incremental
    `InputEdit` repairs and a range-keyed `sum_tree` of highlight runs (this
    is gpui-component's `SyntaxHighlighter`, already vendored and patched).
12. **Centralize language identity** in `lang.rs` (extensions, filenames,
    shebangs, LSP mapping, icons) — one row per language, everything reads it.
13. **Register grammars + queries.** Built-ins via the
    `tree-sitter-languages` feature; custom quality queries where the
    upstream one is weak (TSX/JSX in `lang.rs`).
14. **Wire language switching**: `set_highlighter(lang_id)` on open/save-as,
    reset-not-recreate.

### Phase D — Formatting ✅ (in this repo)

15. **LSP formatting command** — `textDocument/formatting` via the background
    executor, `apply_lsp_edits` on the main thread, status-bar feedback.
16. **Honor editor settings** in `FormattingOptions` (`tab_size` from
    `editor.tabSize`).
17. **Format-on-save** — `editor.formatOnSave` setting + `format_then_save`
    with the stale-snapshot guard, degrading to a plain save on every miss.
18. **Settings UI** — a Format on Save on/off row alongside auto-save.

### Phase E — Next (designed, not yet built)

19. **Theme hot reload** (§2.7): `RwLock<Arc<[Theme]>>` registry +
    `notify` watcher on the user themes dir + name-keyed active theme.
20. **Theme selector modal** with preview-on-hover (needs 19).
21. **External formatter chain** (§4.5): `Formatter` enum, per-language
    overrides, `PATH` discovery cache, timeouts, save never blocked.
22. **Code actions on save** (`source.organizeImports`, `source.fixAll`)
    chained before the formatter, as in Zed.
23. **Semantic token overlay** (LSP `textDocument/semanticTokens`) to refine
    tree-sitter captures with type info — layered *over* the tree-sitter
    baseline, exactly like the theme is layered over the editor.

---

## 7. Testing strategy

The repo's tests double as the contract for this architecture:

| Test | Guarantees |
| --- | --- |
| `loads_all_themes` / `every_theme_has_every_token` | embedded families load; *every* theme — user ones included — resolves to a complete token set (refinement works) |
| `base_palettes_match_the_shipped_github_themes` | base constants can't drift from shipped themes (refinement is a no-op for them) |
| `single_theme_file_is_wrapped_into_a_family` / `malformed_theme_files_are_skipped` | both file layouts work; bad files never panic |
| `user_themes_override_embedded_by_name` | override *replaces*, addition *appends* — Zed semantics, deterministic order |
| `highlighter_uses_zed_syntax_palette` | syntax extraction byte-exact against the source JSON |
| `terminal_palette_matches_theme` | every theme yields a full 16-ANSI + 256-color palette |
| `lang::detects_extensions` / `web_languages_resolve_to_a_server` | detection and LSP mapping stay correct as languages are added |

When extending: any new theme token gets a `KEY_MAP` row + a completeness
assertion; any new language gets a detection case; any formatter gets a
stale-snapshot case. The pattern to follow is *test the resolved output*, not
the internals — that keeps the pipeline refactorable (Phase E) without
rewriting tests.

---

*References: [1] [Zed — User themes now in Preview](https://zed.dev/blog/user-themes-now-in-preview) ·
[2] [Zed — Theme Extensions](https://zed.dev/docs/extensions/themes) ·
[3] [Zed — Configuring Languages](https://zed.dev/docs/configuring-languages)*
