# Performance Audit & Fix Record — html2gpui / `app`

Static audit of the Rust/GPUI code editor, plus the resolution status of each
finding. References use `app/src/...` by file:line at the time of the audit.
**Note:** this sandbox has no Rust toolchain (and no crates.io access), so the
fixes were written against the APIs already used in the codebase but were **not
compiled** here — run `cargo build` / `cargo clippy` before shipping.

---

## Summary

| # | Severity | Status | Issue |
|---|----------|--------|-------|
| 1 | 🔴 Critical | ✅ Fixed | O(n·log n) `stat()` syscalls + per-comparison allocs in `load_dir` sort |
| 2 | 🔴 High | ✅ Fixed | Explorer tree rebuilt every frame (no virtualization) |
| 3 | 🔴 High | 🟡 Partly fixed | Whole-file LSP `didChange` per keystroke + blocking writes on UI thread |
| 4 | 🟠 High | ✅ Fixed | Every fs event → full recursive tree rescan on the UI thread |
| 5 | 🟡 Medium | ✅ Fixed | Whole-state `clone()` per frame in `Workspace::render` |
| 6 | 🟡 Medium | ✅ Fixed | Diagnostics double-cloned on every LSP publish |
| 7 | 🔵 Low | ✅ Fixed | Per-frame double string allocations in status/tab helpers |
| 8 | 🔵 Low | ✅ Fixed | `params.clone()` on the LSP read path |
| 9 | 🔵 Low | ✅ Fixed | `title()` / explorer header re-derive the folder name every frame |

---

## 1. ✅ `load_dir` sort: O(n·log n) `stat()` → O(n) `file_type()`

**`app/src/fs_tree.rs`** — rewrote `load_dir`:

- Entries are materialized once into a private `Entry { name, sort_key, path, is_dir }`.
- `DirEntry::file_type()` runs **once per entry** (on Linux it's served by
  `d_type` from `readdir`, i.e. no syscall at all; it only falls back to
  `stat()` where the OS leaves the type unknown). The old comparator called
  `Path::is_dir()` twice per comparison — ~n·log₂n syscalls.
- The lowercase `sort_key` is computed once per entry; the comparator is now
  allocation-free.
- The extra `is_dir()` probe per entry after sorting was removed entirely.

Also fixed inside the same file: `reload_dir_preserving` now indexes previous
nodes in a `HashMap<&Path, &TreeNode>` instead of a linear `find` per node
(O(n) instead of O(n²) on wide directories).

## 2. ✅ Explorer rebuild — cached rows + GPUI virtualization

**`app/src/fs_tree.rs` / `app/src/ui/sidebar/explorer.rs` / `app/src/workspace/render.rs` / `app/src/workspace/mod.rs`**

- The visible tree is flattened into a cached `Arc<[VisibleTreeRow]>` only
  after an actual tree/expansion mutation. Ordinary workspace paints do not
  recursively walk the tree or allocate a row vector.
- The explorer now uses GPUI 0.2.2's `uniform_list`, the same primitive used
  by Zed's project panel: only the viewport (plus its measurement row) is
  laid out and painted. A project with tens of thousands of visible entries
  no longer creates tens of thousands of GPUI elements per frame.
- Directory children now track `children_loaded`, so an empty directory is
  scanned once rather than on every expand/collapse cycle.
- Root refreshes and watcher updates use a shallow, move-based merge. Loaded
  grandchildren are retained instead of recursively rescanning every expanded
  directory. Watcher `read_dir` work is performed on GPUI's background
  executor; the UI thread only merges the resulting snapshot.
- Added auto-reveal (expand only the ancestors of the opened file), explorer
  arrow-key navigation, and compact New File/New Folder/Refresh/Collapse All
  header actions. The project label keeps its original case.
- Row element ids remain allocation-free (`("tree-row", idx)`) and steady-state
  typing still avoids repainting the explorer chrome (see #3b).

## 3. 🟡 LSP sync — coalesced + off the UI thread; incremental ranges remain

**`app/src/lsp.rs` / `app/src/workspace/mod.rs`**

- **Non-blocking sends:** all outbound JSON-RPC messages are framed once and
  pushed onto an unbounded `mpsc` channel consumed by a **dedicated writer
  thread** per client. A language server that stops draining its stdin (full
  64 KB pipe) can no longer stall the UI thread — previously `write_all` ran
  synchronously on the UI thread per keystroke.
- **Coalescing/debounce:** `did_change` now stores the latest text per document
  in a `pending_changes` map; the writer thread flushes it as a full-document
  `didChange` roughly every 120 ms (recv_timeout-driven, no extra timers).
  Consequences: a fast typist costs one sync per 120 ms instead of one
  serialization + write per keypress; close-during-pending drops the stale
  edit; version numbers stay monotonic (computed at flush time).
- **No work when there is no server:** the workspace change handler checks
  `LspManager::has_client(lang)` before reading the whole buffer, so plain
  text / too-large / unsupported files skip the per-keystroke
  `value().to_string()` clone entirely.

**Still open:** sync is still full-document (`range: None`) rather than
incremental ranges — the editor's change-event API for edit ranges could not
be verified without the crate source. If your files are large, incremental
`didChange` (`range` + `range_length`) is the next step.

## 4. ✅ fs events: scoped, debounced reloads completely off the UI thread

**`app/src/workspace/mod.rs` (new/load_root/reload_dir) + `app/src/fs_tree.rs`**

- The watcher callback now forwards the **changed path** (not just `()`).
- Raw events are drained by a dedicated **OS thread** with a 120 ms debounce —
  the old handler ran two blocking `thread::sleep(150 ms)` calls *inside the
  foreground executor*, i.e. froze the UI ~300 ms per fs burst.
- The UI-side rescan is scoped: only the **parent directory** of each changed
  path is reloaded (the only level whose entry set can differ), via the new
  `Workspace::reload_dir`, preserving deeper expansion state. A full recursive
  rescan no longer happens per event; full reloads remain only for
  user-initiated actions (Refresh, Save As, create/delete/rename).

## 5. ✅ Removed per-frame deep clones in `Workspace::render`

**`app/src/workspace/render.rs`** — the render helpers only *read* state, so
the previous `tabs.clone()`, `terminal_tabs.clone()`, `status.clone()`,
`root.clone()`, `selected_path.clone()`, `inline_creating.clone()`,
`theme_name.clone()`, `active_path().cloned()`, `active_editor().cloned()`
per frame are replaced with borrows (`&self.tabs`, `self.status.as_str()`,
`self.root.as_ref()`, …). The panel-size clamps were reordered to run before
the borrows are taken.

## 6. ✅ Diagnostics: one storage site, `Arc`-shared payload

**`app/src/workspace/mod.rs`** — `diagnostics_by_path` now holds
`Arc<Vec<Diagnostic>>`; `apply_diagnostics` wraps the incoming vector once and
shares it (`Arc::clone`) instead of cloning the whole bundle into the map and
again into every editor. `copy_active_diagnostic` works unchanged through the
`Arc` deref. (Per-editor `DiagnosticSet` clones remain — bounded by the editor
API — and the open-tab scan is still a small linear list.)

## 7. ✅ Status bar double allocations removed

**`app/src/ui/status_bar.rs`** — `status.to_string()` / `theme_name.to_string()`
→ `SharedString::from(borrowed)`. Tab-bar and terminal-tab labels still build
one small `String` per tab, but only on frames where the workspace actually
re-renders now (real state changes), not per keystroke.

## 8. ✅ LSP read path no longer clones every JSON body

**`app/src/lsp.rs`** — `handle_incoming_message` takes `&mut Value` and
`serde_json::from_value(std::mem::take(params))` moves the params `Value`
instead of cloning it.

## 9. ✅ Folder-name re-derivation removed

**`app/src/workspace/mod.rs`** — cached `Workspace::root_display` (set in
`load_root`) is used by `title()` and passed to the explorer header instead of
calling `display_name(root)` per frame.

---

## What still needs human/CI verification

1. **Compile:** `cargo build -p app` and `cargo clippy -p app` — this sandbox
   has no Rust toolchain and no crates.io access, so the changes were
   hand-checked but not built. Areas most worth eyeballing in the build:
   `flush_pending_changes`/writer-thread code in `lsp.rs`, the borrowed
   locals in `render.rs`, and `reload_dir` in `workspace/mod.rs`.
2. **Explorer runtime check** — verify scroll-wheel behavior and keyboard
   focus with the exact desktop backend; the list now uses GPUI's verified
   `uniform_list` primitive.
3. **Incremental LSP sync** (issue #3) — ~~switch `content_changes` from
   `range: None` to edit ranges~~ **done in round 2** (see #13): sync is now
   incremental whenever the server advertises it.
4. Optional runtime check: `EZICODE_PERF=1 cargo run` exists for measuring the
   explorer reload cost against `perf::stat_count()`.

---
---

# Round 2 — deep Zed-inspired pass (2026-09-09)

A second full-codebase audit (`app/`, `components/`, `gpui-terminal/`),
implemented against the same constraint as round 1 (no toolchain in the
sandbox → every change uses only APIs already exercised elsewhere in this
tree). All round-1 fixes were found in place and intact; the incremental-sync
follow-up from round 1 had also already been completed (diffed
`didChange` with ranged edits, covered by tests in `lsp/client.rs`).

| # | Severity | Status | Issue |
|---|----------|--------|-------|
| 10 | 🔴 High | ✅ Fixed | Terminal re-shaped its font metrics glyph **every frame** |
| 11 | 🔴 High | ✅ Fixed | Diff view re-ran the word-level diff parser on **every repaint** |
| 12 | 🔴 High | ✅ Fixed | `open_file` / `save` did blocking disk I/O on the UI thread |
| 13 | 🟠 Medium | ✅ Fixed | Auto-save debounce parked a worker thread per keystroke |
| 14 | 🟠 Medium | ✅ Fixed | Git watcher spawned 2–3 processes per poll instead of 1 |
| 15 | 🟠 Medium | ✅ Fixed | 3 whole-buffer copies per keystroke on the LSP sync path |
| 16 | 🟡 Low | ✅ Fixed | Shebang detection read the *entire* file |
| 17 | 🟡 Low | ✅ Fixed | Per-token `String`→`SharedString` allocations in the highlighter |
| 18 | 🟡 Low | ✅ Fixed | Per-run font-family re-interning in the terminal painter |
| 19 | 🔵 Build | ✅ Fixed | Release profile had no LTO / default codegen units |

## 10. ✅ Terminal font metrics: measure once per font config, not per frame

**`gpui-terminal/src/view.rs`** — the canvas paint closure called
`measure_cell(window)` on *every frame*, shaping a reference glyph through
the text system even while the terminal was idle — the single biggest
steady-state cost of the terminal panel. `TerminalView` now carries a
`cell_metrics_valid` flag; metrics are (re)measured in `render()` only when
`update_config` saw a family/size/line-height change. Theme switches
(palette-only) no longer re-measure. This mirrors Zed, which caches font
metrics per font configuration in its terminal element.

## 11. ✅ Diff view: parse once on a background thread, share via `Arc`

**`app/src/ui/diff.rs` + `app/src/workspace/mod.rs`** — `render_diff_view`
ran `parse_side_by_side_diff` (which includes the expensive word-level
intra-line diffing) on **every workspace repaint** while a diff tab was
open: git status refreshes, theme switches, panel toggles all re-parsed the
entire diff. Now:

- `DiffTab` stores `parsed: Option<Arc<ParsedDiff>>` next to the raw text.
- Both loaders (`open_diff`, `refresh_active_diff`) parse on the same
  background task that produces the diff text.
- Repaints clone the cached row snapshot (cheap struct clones) instead of
  re-running the parser. Zed's diff view likewise keeps a parsed
  representation and re-renders from it.

## 12. ✅ File open/save off the UI thread (Zed-style async I/O)

**`app/src/workspace/mod.rs`** —

- `open_file` used to `std::fs::read` (up to 8 MB), null-scan, UTF-8 decode
  and language-detect **synchronously on the UI thread**, then cloned the
  whole buffer once more for `set_value`. Now `load_buffer_file` runs all of
  that on the background executor; `finish_open_file` creates the editor on
  the UI thread when the load completes, re-checks the file wasn't opened
  again in flight, and **moves** the decoded text into the editor (one fewer
  full-buffer copy per open).
- `save` / `save_tab_quiet` wrote the file synchronously (`std::fs::write`),
  freezing the window for the whole write on slow disks or network mounts.
  `write_file_async` now writes on the background executor and does the
  bookkeeping on completion: the dirty marker is cleared only when the
  buffer still matches the written snapshot (typing during an in-flight
  save correctly stays dirty), `didSave` goes to the language server, git is
  poked, and `settings.json` reload still fires. Zed performs buffer saves
  on its background executor for exactly this reason.
- `save_as` intentionally keeps its synchronous write: it already follows a
  blocking native dialog and must settle the tab path before the LSP attach.

## 13. ✅ Auto-save: async executor timer instead of sleeping worker threads

**`app/src/workspace/mod.rs`** — the debounce waited via
`background_spawn(thread::sleep(delay))`: every keystroke parked one of the
executor's worker threads for the whole delay (default 1 s), so a fast
typist accumulated a dozen sleeping threads that could starve the same pool
running LSP requests and directory scans. Now it awaits
`cx.background_executor().timer(delay)` (the same primitive the component
library already uses), which schedules the wake-up without holding a thread.

## 14. ✅ Git status: one process per poll

**`app/src/git.rs`** — the watcher thread polls every ~1.5 s and used to
spawn `git status` **plus** `git rev-parse --abbrev-ref` (and on a detached
HEAD a second `rev-parse --short`): up to 3 process spawns per poll.
`status()` now passes `--branch`, parses the `## <branch>` header record
(including `No commits yet on …` and upstream/tracking suffixes), and only
falls back to `rev-parse` in the rare detached-HEAD case — halving the
steady-state process churn. `parse_porcelain` skips the header record (its
third byte is a space too and would otherwise parse as a bogus `##` change);
new tests pin the header shapes.

## 15. ✅ LSP sync path: 3 → 2 whole-buffer copies per keystroke

**`app/src/lsp/client.rs`** — per keystroke the workspace materialized the
buffer once (`value().to_string()`), then `did_change` cloned it into
`last_texts` **and** again into `pending_changes`. `change_document` /
`did_change` now take an owned `String`: one clone feeds `last_texts`, the
original moves into `pending_changes`. On a 400 KB file that is ~400 KB less
memcpy per keystroke. (The workspace read itself can't go lower — the editor
rope is UI-thread-only and the LSP maps live behind `Mutex`.)

## 16. ✅ Shebang detection: read 256 bytes, not the whole file

**`app/src/lang.rs`** — `shebang_language` used `std::fs::read` on the entire
file just to look at its first line, so opening a huge extension-less file
(log, dump, minified bundle) paid a full read for language detection. It now
opens the file and reads one 256-byte block. Also removed the unconditional
`to_ascii_lowercase()` allocation in `language_for` (it runs on every render
for the status bar's language label): the extension is only folded when it
actually contains uppercase letters.

## 17. ✅ Highlighter: intern tree-sitter capture names per language

**`components/src/highlighter/highlighter.rs`** — the visible-range
highlight pass ran `SharedString::from(name.to_string())` for **every
capture on every repaint** — two allocations per visible token. Capture
names are fixed per compiled query, so `CompiledLanguageQuery` now interns
them once (`capture_names: Vec<SharedString>`); the hot loop clones a
refcount instead. Zed likewise interns its highlight-name strings.

## 18. ✅ Terminal painter: hoist font-family interning out of the run loop

**`gpui-terminal/src/render.rs`** — each batched text run converted the font
family `String` → `SharedString` (allocation) and rebuilt the
`FontFeatures`. The family is now converted once per frame and cloned by
refcount per run; the ligature-disable features value is built once and
cloned. Same visuals, fewer per-frame allocations on busy terminal output.

## 19. ✅ Release profile: LTO + single codegen unit

**`Cargo.toml`** — added `[profile.release] lto = "thin", codegen-units = 1`
(Zed's trade of build time for runtime: cross-crate inlining across the
GPUI/tree-sitter boundary).

## Verified-but-left-alone (why they are not problems)

- **Explorer rows** — `flatten_visible` only runs on tree mutations; the
  per-frame path is `Arc::clone` + `uniform_list` viewport rows. ✔
- **LSP writer thread** — 120 ms `recv_timeout` wake-ups are negligible, and
  the coalesced incremental flush is covered by unit tests. ✔
- **Editor element (vendored)** — visible-range shaping, `longest_row`
  shaping and line-number shaping all hit GPUI's `LineLayout` cache on
  unchanged text; incremental tree-sitter updates already happen per edit. ✔
- **Terminal palette lookups** — `palette.resolve` is an array index per
  cell, and background quads are already 2D-merged before `paint_quad`. ✔
- **`git.rs` run_git** — callers already run it on background threads. ✔

## Round-2 items that still need human/CI verification

1. `cargo build -p app && cargo test -p app && cargo clippy -p app` (still no
   toolchain in this sandbox). Highest-risk spots to eyeball: the
   `spawn_in`/`update_in` dance in `open_file`/`finish_open_file`, the borrow
   ordering in `save`/`save_tab_quiet`/`write_file_async`, and the terminal
   `cell_metrics_valid` invalidation paths.
2. Runtime check that the first terminal paint after a font-size zoom
   re-measures correctly (Ctrl+= / Ctrl-- is currently editor-only, so the
   practical path is a theme or config change).
3. Soak test: type fast with `editor.autoSave: "afterDelay"` enabled and a
   language server running — should no longer saturate the background pool.