# Native Markdown preview

Easy Code renders a live Markdown preview as a native GPUI pane. It does not
use a browser or webview.

## User interface

For files whose extension is `.md` or `.markdown` (case-insensitive), the tab
bar shows an **M↓** button at its right edge. The icon is
`app/assets/ui_icons/markdown-preview.svg`; its paths use `currentColor`, and
the GPUI SVG element supplies the active theme's icon color. Clicking the
button calls `Workspace::toggle_markdown_preview`. The same button, or the
close button in the preview header, removes the right-hand split.

The source and preview share the editor area equally. The preview uses
`gpui_component::text::TextView`, which produces GPUI elements for headings,
paragraphs, emphasis, links, images, lists and task lists, quotes, rules,
tables, and highlighted fenced code. `TextViewStyle` receives the current
highlight theme and light/dark mode, so the result follows theme changes.

## Architecture

The implementation follows the layering documented in
`architecture-explorer.md`:

| Layer | Location | Responsibility |
| --- | --- | --- |
| Pure model | `app/src/markdown_preview.rs` | `pulldown-cmark` parsing, owned blocks/inline runs, nesting, source byte ranges and source-line mapping. |
| State and behavior | `app/src/workspace/mod.rs` | Toggle/close operations, buffer observation, generation-based debounce and background parsing. |
| GPUI adapter | `markdown_preview::render_panel` and `ui/tab_bar.rs` | Theme-aware native elements only; interactions are routed back through `Workspace`. |

`pulldown-cmark` 0.13 is the same parser/version family used by Zed. Easy Code
enables tables, task lists, strikethrough, and footnotes. Parse results own all
text so they can safely cross executor boundaries. Every edit increments a
preview generation, waits 75 ms, and parses on GPUI's background executor. A
result is installed only if its path and generation are still current; stale
large-file parses therefore cannot overwrite newer text.

The GPUI component renderer is a thin materialization adapter. Easy Code's
pulldown model remains the source of block structure and source mappings, while
the existing native text nodes provide image loading, selection, table layout,
and tree-sitter syntax highlighting without duplicating those facilities or
introducing a webview.

## Relationship to Zed

Before implementation, the relevant Zed crates were reviewed:

- `crates/markdown/src/parser.rs` wraps `pulldown-cmark`, enables GFM options,
  retains source ranges, and turns borrowed parser events into an owned model.
  Zed's `Markdown` entity parses asynchronously and renders that model as GPUI
  blocks with language-backed code highlighting.
- `crates/markdown_preview/src/markdown_preview_view.rs` binds a preview item to
  an editor, subscribes to buffer/selection events, debounces reparsing, and
  calls `Markdown::reset`. It attaches the item to a workspace pane. Source
  offsets are mapped to rendered root blocks for scroll synchronization.

Easy Code reimplements that design against its smaller `Workspace`, `OpenTab`,
and `InputState` APIs. No Zed source was copied. This is important because the
Zed repository includes GPL-licensed editor code (alongside its Apache
license); the implementation uses the public architectural ideas and the
independently licensed `pulldown-cmark` crate rather than importing editor
code.

Unlike Zed, Easy Code currently has one editor pane rather than a general pane
tree, so the preview is represented by an optional right-hand child in the
editor row instead of a workspace item. The pure model already exposes
`block_for_source_line`, the source-to-root-block primitive required for scroll
sync. GPUI Component's `TextView` does not currently expose its internal list
scroll handle, so editor-driven scroll sync is intentionally deferred rather
than reaching through private renderer state.

## Tests

Unit tests in `app/src/markdown_preview.rs` cover:

- headings, tables, task items, inline code, strong text, and strikethrough;
- nested list depth and source-line mapping;
- fenced-code languages, links, images, and footnotes; and
- Markdown extension detection.
