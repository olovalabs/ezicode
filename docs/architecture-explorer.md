# The Explorer — Architecture & VS Code Parity

**Easy Code (ezicode) · Rust + GPUI**

The explorer is the panel people touch most, so it is built to behave like the
one they already know: VS Code's. This document describes how the panel is put
together, which VS Code behaviors are implemented and where, and the rules that
keep it fast on repositories with hundreds of thousands of files.

---

## 1. Layers

| Layer | File | Responsibility |
| --- | --- | --- |
| Tree model | `app/src/fs_tree.rs` | `TreeNode` (lazily loaded, per-folder expansion state), flattening to `VisibleTreeRow`, and the *pure* geometry helpers: ancestors, subtree bounds, sticky-scroll layout, type-ahead matching. All unit-tested. |
| State & behavior | `app/src/workspace/mod.rs` | Selection, focus, keyboard, drag and drop, file operations, persistence. Nothing here draws. |
| Rendering | `app/src/ui/sidebar/explorer.rs` | One `uniform_list` of rows plus the sticky-scroll widget, hover/selected/focused/cut styling, context menus. Nothing here mutates the tree directly; every interaction calls a `Workspace` method. |

The split matters for two reasons: the interesting logic (sticky scroll, range
selection, type-ahead) is testable without a GPU, and the render path stays a
pure function of state, which is what keeps repaints cheap.

## 2. Performance model

* The tree is flattened into `Arc<[VisibleTreeRow]>` **only** when its
  structure changes (`rebuild_explorer_rows`). Ordinary repaints reuse the
  `Arc`.
* `uniform_list` renders only the rows in the viewport, so cost is proportional
  to panel height, not project size.
* Directories are read lazily on expansion and off the UI thread
  (`load_directory_async`); a refresh re-scans only directories that were
  actually loaded.
* Sticky scroll is `O(depth)` per frame: it walks the ancestor chain of a
  single row, never the whole tree.

## 3. VS Code behaviors and where they live

### Sticky scroll (`fs_tree::sticky_layout` + `StickyFolders`)

VS Code pins the ancestor folders of whatever sits at the top of the viewport.
The *what to pin* half mirrors `StickyScrollController`:

1. Start from the row at the current scroll offset and take its ancestors.
2. Grow the stack one line at a time: a further ancestor is pinned only if it
   is an ancestor of the row that would sit just below the taller widget. The
   stack only ever grows during this refinement, which is what keeps it from
   oscillating between two states while scrolling.
3. A folder is pinned only while its own row is hidden behind the widget.
4. When the innermost pinned section ends inside the widget, that row — and
   only that row — drifts upwards behind the ones above it, so it slides away
   instead of blinking out. Zed's project panel drifts just the innermost item
   too; keeping the outer rows perfectly still is what makes the stack look
   calm.

The *how to draw it* half follows Zed's `ui::sticky_items`: the headers are a
**`UniformListDecoration`** attached to the explorer's `uniform_list`, not an
overlay element layered on top of it. That choice is load-bearing:

* A decoration is computed during the list's **prepaint**, so it sees the
  scroll offset the list has already clamped, the measured item height and the
  real visible range. An overlay has to read the offset from `render`, a frame
  late and *unclamped* at the ends of the list; the two disagree while
  overscrolling, the pinned set flips between two answers on consecutive
  frames, and the tree visibly shakes when you scroll into the bottom.
* The headers are prepainted inside the list's own hitbox, so the wheel keeps
  scrolling the tree while the pointer is over them — no hand-rolled scroll
  forwarding. The overlay needed that forwarding, and because GPUI hitboxes do
  not block by default the list handled the very same event as well: two
  deltas per tick, each clamped differently at the bottom.
* Being inside the list also means the content mask clips the drifting row for
  free, and no per-frame state outside the list changes, so scrolling no
  longer drags the whole panel through an extra layout pass.

Only `explorer_sticky_rows` is written back (during prepaint, without
notifying) so keyboard navigation can reveal rows past the headers. Headers are
clickable: click scrolls to the folder, the twistie collapses it, Alt+click
toggles the whole subtree. The feature follows
`workbench.tree.enableStickyScroll`, is toggled from the panel's context menu,
and is stored per workspace. It steps aside while the inline "new file" editor
is open, because that row does not exist in the flat model the indices come
from.

Anything that nudges the scroll position by hand — the drag auto-scroll near
the panel edges — clamps against the list's own `max_offset` rather than
recomputing a limit from row counts, for the same reason: a second opinion
about where the bottom is, is exactly what makes a list judder.

### Selection and focus

`selected_path` is the *focused* row; `explorer_selection` is the full
multi-selection. The set is only honoured while it still contains the focused
row, so the many places that simply assign `selected_path` (opening a file,
revealing a search hit, finishing a rename) collapse the selection instead of
leaving a stale highlight behind — no bookkeeping needed at those call sites.

* Click selects, Ctrl/Cmd-click toggles one row, Shift-click extends from the
  anchor, Shift+arrows extend while navigating, Ctrl/Cmd+A selects everything,
  Escape collapses back to the focused row.
* Selection colors follow VS Code: bright while the tree has focus, dimmed when
  it does not, plus a focus ring on the focused row.
* Cut entries are drawn at 50% opacity until the paste happens.
* Cut / Copy / Paste / Delete / drag all operate on the whole selection.

### Keyboard

Arrows (with VS Code's "left collapses, then walks to the parent" rule),
Home/End, PageUp/PageDown sized from the real viewport and the scaled row
height, Enter (open and pin),
Space (open as preview), F2, Delete, `*` to expand a subtree, and type-ahead:
typing letters jumps to the next matching row, repeating one letter cycles
through matches, and the buffer expires after a pause.

### Mouse, drag and drop

Single click opens files in a preview tab and toggles folders; double click
pins the preview tab. Alt+click on a folder expands or collapses its entire
subtree. Dragging shows a chip with the number of entries being moved; hovering
a collapsed folder for half a second expands it mid-drag; dragging near the top
or bottom edge scrolls the list; dropping on a file targets its folder; and
dropping on blank space targets the workspace root.

### Context menu

New File / New Folder (targeting the clicked folder, or the clicked file's
folder), Reveal in File Explorer, Open in Integrated Terminal, Find in Folder,
Cut / Copy / Paste, Copy Path / Copy Relative Path, Duplicate, Rename, Delete —
plus Refresh, Collapse All and the sticky-scroll toggle on the panel header and
on blank space.

### Small details that matter

* Rows are 22px with 8px indents, 13px labels and indent guides; the guide of
  the focused row's branch is highlighted.
* Those are *design* pixels: each one goes through `ui::scale::rem`, where 1 rem
  is `ui_font_size` (14px by default). Raising the UI font size therefore grows
  labels, file icons, chevrons, row height, gaps and the indent step together —
  the tree reads as zoomed rather than as text clipped inside a 22px row — and
  it needs no rebuild, because only the lengths change. Hairlines (indent
  guides, borders) and corner radii stay in px: a one pixel guide is one pixel
  at every size.
* The Git sidebar measures its rows, labels, badges and icons the same way, so
  the two panels that share the activity bar scale together instead of one
  growing inside a row that stayed 22px.
* Paste and Duplicate use VS Code's naming (`report copy.md`, then
  `report copy 2.md`).
* Deleting moves focus to the next surviving row.
* Expanded folders, the focused row and the sticky-scroll preference are saved
  per workspace and restored on the next session.
* The tree follows the active editor (`explorer.autoReveal`), scrolling the
  minimum amount needed rather than re-centering.

## 4. Testing

`app/src/fs_tree.rs` unit-tests the pure logic: ancestor chains, subtree
bounds, sticky-scroll pinning/capping/sliding, type-ahead wrap-around and
recursive expansion. Anything that needs a window belongs in the headless
render tests instead.
