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
3. **Incremental LSP sync** (issue #3) — switch `content_changes` from
   `range: None` to edit ranges once the editor exposes them.
4. Optional runtime check: `EZICODE_PERF=1 cargo run` exists for measuring the
   explorer reload cost against `perf::stat_count()`.