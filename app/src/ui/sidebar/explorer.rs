//! The Explorer panel.
//!
//! This is modelled on VS Code's explorer tree rather than a generic file
//! list: 22px rows with 8px indents and indent guides, a virtualized list that
//! stays cheap on projects with thousands of files, multi-selection with
//! Ctrl/Cmd and Shift, drag and drop with hover-to-expand, a full context
//! menu, and sticky scroll — the parent folders of the rows you are looking
//! at stay pinned to the top of the panel.
//!
//! Sticky headers are a `UniformListDecoration` (see `StickyFolders`), the way
//! Zed's project panel does it, rather than an element floating over the list:
//! that is what keeps them exact and jitter-free at the ends of the scroll
//! range, and keeps the wheel scrolling the tree while the pointer is over
//! them.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    div, point, prelude::*, px, rgba, size, svg, uniform_list, AnyElement, App, AvailableSpace,
    Bounds, Context, DragMoveEvent, Element, ElementId, Entity, FocusHandle, FontWeight,
    GlobalElementId, InspectorElementId, IntoElement, LayoutId, MouseButton, Pixels, Point, Render,
    SharedString, Style, UniformListDecoration, UniformListScrollHandle, Window,
};
use gpui_component::{input::Input, menu::ContextMenuExt, tooltip::Tooltip, Sizable};

use crate::actions::{
    ExplorerCollapseAll, ExplorerCopy, ExplorerCopyPath, ExplorerCopyRelativePath, ExplorerCut,
    ExplorerDelete, ExplorerDuplicate, ExplorerFindInFolder, ExplorerNewFile, ExplorerNewFolder,
    ExplorerOpenInTerminal, ExplorerPaste, ExplorerRefresh, ExplorerRename, ExplorerRevealInFinder,
    ExplorerToggleStickyScroll, OpenFolder,
};
use crate::file_icons;
use crate::fs_tree::{ancestor_chain, sticky_layout, subtree_end, VisibleTreeRow};
use crate::git::ChangeKind;
use crate::theme::Colors;
use crate::ui::common::icon_img;
use crate::workspace::{CreatingKind, ExplorerDrag, InlineCreating, InlineRenaming, Workspace};

/// VS Code's `workbench.tree.indent`.
const INDENT_STEP: f32 = 8.0;
const BASE_PAD: f32 = 8.0;
/// VS Code's list row height.
pub(crate) const ROW_HEIGHT: f32 = 22.0;
const ICON_SIZE: f32 = 16.0;
const TEXT_SIZE: f32 = 13.0;
/// `workbench.tree.stickyScrollMaxItemCount`.
pub(crate) const STICKY_MAX_ROWS: usize = 7;
/// How close to an edge a drag has to get before the list scrolls itself.
const DRAG_SCROLL_MARGIN: f32 = 24.0;

/// VS Code-style git tint for tree entries (files and their parent dirs).
fn git_kind_color(kind: ChangeKind, t: &Colors) -> u32 {
    match kind {
        ChangeKind::Modified => t.vc_modified,
        ChangeKind::Added | ChangeKind::Renamed | ChangeKind::Copied | ChangeKind::Untracked => {
            t.vc_added
        }
        ChangeKind::Deleted | ChangeKind::Conflicted => t.vc_deleted,
        ChangeKind::TypeChanged => t.icon_accent,
    }
}

/// Everything the panel needs for one frame. Bundled into a struct because the
/// tree needs a lot of context and positional arguments stopped being
/// readable.
pub(crate) struct ExplorerView<'a> {
    pub rows: Arc<[VisibleTreeRow]>,
    pub scroll_handle: UniformListScrollHandle,
    pub focus_handle: FocusHandle,
    pub root_path: Option<&'a Path>,
    /// The file shown in the active editor tab.
    pub open: Option<&'a PathBuf>,
    /// The row with keyboard focus.
    pub focused: Option<&'a PathBuf>,
    /// Every selected row (multi-selection).
    pub selection: &'a [PathBuf],
    /// Entries sitting on the explorer clipboard after a Cut.
    pub cut_paths: &'a [PathBuf],
    /// Folder currently hovered by a drag.
    pub drag_target: Option<&'a PathBuf>,
    /// Whether the tree itself holds keyboard focus, which decides between
    /// VS Code's active and inactive selection colors.
    pub tree_focused: bool,
    pub sticky_enabled: bool,
    pub section_expanded: bool,
    pub inline_creating: Option<&'a InlineCreating>,
    pub inline_renaming: Option<&'a InlineRenaming>,
    pub folder: &'a SharedString,
    pub git_map: Arc<HashMap<PathBuf, ChangeKind>>,
}

/// Per-row drawing state that does not depend on the row itself.
#[derive(Clone, Copy)]
struct RowStyle {
    colors: Colors,
    tree_focused: bool,
    /// Depth of the indent guide to highlight, plus the row range it spans.
    active_guide: Option<(usize, usize, usize)>,
}

pub(crate) fn render_tree(
    view: ExplorerView<'_>,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let mut col = div()
        .size_full()
        .flex()
        .flex_col()
        .bg(rgba(t.panel))
        .overflow_hidden();

    col = col.child(root_header(&view, t, cx));

    if view.section_expanded {
        col = col.child(tree_body(view, t, cx));
    }

    col.into_any_element()
}

fn root_header(view: &ExplorerView<'_>, t: &Colors, cx: &mut Context<Workspace>) -> AnyElement {
    let section_expanded = view.section_expanded;
    let root_folder_icon = if section_expanded {
        file_icons::FOLDER_EXPANDED
    } else {
        file_icons::FOLDER_COLLAPSED
    };
    let root_chevron = if section_expanded {
        "ui_icons/chevron-down_tint.svg"
    } else {
        "ui_icons/chevron-right_tint.svg"
    };

    let r_context = view.root_path.map(|path| path.to_path_buf());
    let drop_root = r_context.clone();
    let drop_color = t.element_selected;
    let sticky_enabled = view.sticky_enabled;

    div()
        .id("exp-root-header")
        .h(px(35.0))
        .px(px(8.0))
        .flex()
        .flex_row()
        .items_center()
        .cursor_pointer()
        .hover(|s| s.bg(rgba(t.ghost_hover)))
        .on_click(cx.listener(|this, _, _, cx| {
            this.toggle_explorer_section(cx);
        }))
        .drag_over::<ExplorerDrag>(move |this, _, _, _| this.bg(rgba(drop_color)))
        .on_drop(cx.listener(move |this, drag: &ExplorerDrag, _window, cx| {
            if let Some(root) = drop_root.clone() {
                this.explorer_drop(&drag.path, &root, cx);
            }
        }))
        .context_menu(move |menu, _window, _cx| {
            let p1 = r_context.clone();
            let p2 = r_context.clone();
            let p3 = r_context.clone();
            let p4 = r_context.clone();
            menu.menu("New File…", Box::new(ExplorerNewFile { parent: p1 }))
                .menu("New Folder…", Box::new(ExplorerNewFolder { parent: p2 }))
                .separator()
                .menu("Refresh Explorer", Box::new(ExplorerRefresh))
                .menu("Collapse All Folders", Box::new(ExplorerCollapseAll))
                .menu(
                    if sticky_enabled {
                        "Disable Sticky Scroll"
                    } else {
                        "Enable Sticky Scroll"
                    },
                    Box::new(ExplorerToggleStickyScroll),
                )
                .separator()
                .when(p3.is_some(), |m| {
                    m.menu(
                        "Reveal in File Explorer",
                        Box::new(ExplorerRevealInFinder { path: p3.unwrap() }),
                    )
                })
                .when(p4.is_some(), |m| {
                    m.menu(
                        "Copy Path",
                        Box::new(ExplorerCopyPath { path: p4.unwrap() }),
                    )
                })
                .separator()
                .menu("Open Folder…", Box::new(OpenFolder))
        })
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .child(
                    div()
                        .w(px(16.0))
                        .h(px(16.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_none()
                        .child(
                            svg()
                                .path(root_chevron)
                                .w(px(12.0))
                                .h(px(12.0))
                                .text_color(rgba(t.icon_muted)),
                        ),
                )
                .child(icon_img(root_folder_icon, ICON_SIZE))
                .child(
                    div()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgba(t.text))
                        .child(view.folder.clone()),
                ),
        )
        .child(header_action_button(
            "exp-new-file",
            "ui_icons/new-file_tint.svg",
            "New File...",
            t,
            |this, window, cx| this.start_inline_create_at_root(CreatingKind::File, window, cx),
            cx,
        ))
        .child(header_action_button(
            "exp-new-folder",
            "ui_icons/new-folder_tint.svg",
            "New Folder...",
            t,
            |this, window, cx| this.start_inline_create_at_root(CreatingKind::Folder, window, cx),
            cx,
        ))
        .child(header_action_button(
            "exp-refresh",
            "ui_icons/refresh_tint.svg",
            "Refresh Explorer",
            t,
            |this, _window, cx| this.refresh_explorer(cx),
            cx,
        ))
        .child(header_action_button(
            "exp-collapse-all",
            "ui_icons/collapse-all_tint.svg",
            "Collapse Folders in Explorer",
            t,
            |this, _window, cx| this.collapse_all_folders(cx),
            cx,
        ))
        .into_any_element()
}

/// The virtualized list plus the sticky-scroll widget layered over it.
fn tree_body(view: ExplorerView<'_>, t: &Colors, cx: &mut Context<Workspace>) -> AnyElement {
    let ExplorerView {
        rows,
        scroll_handle,
        focus_handle,
        root_path,
        open,
        focused,
        selection,
        cut_paths,
        drag_target,
        tree_focused,
        sticky_enabled,
        git_map,
        inline_creating,
        inline_renaming,
        ..
    } = view;

    let workspace = cx.entity();
    let open = open.cloned();
    let focused_path = focused.cloned();
    let selection: Vec<PathBuf> = selection.to_vec();
    let cut_paths: Vec<PathBuf> = cut_paths.to_vec();
    let drag_target = drag_target.cloned();
    let creating = inline_creating.cloned();
    let renaming = inline_renaming.cloned();

    // Highlight the indent guide of the focused row's parent, like VS Code.
    let active_guide = focused_path.as_ref().and_then(|path| {
        let index = rows.iter().position(|row| &row.path == path)?;
        let parent = *ancestor_chain(&rows, index).last()?;
        Some((rows[parent].depth, parent + 1, subtree_end(&rows, parent)))
    });
    let style = RowStyle {
        colors: *t,
        tree_focused,
        active_guide,
    };

    let row_data = Arc::clone(&rows);
    let inline_pos = creating.as_ref().map(|creating| {
        rows.iter()
            .enumerate()
            .find(|(_, row)| row.path == creating.parent_dir)
            .map(|(ix, _)| ix + 1)
            .unwrap_or(0)
    });
    let inline_depth = creating
        .as_ref()
        .and_then(|creating| {
            rows.iter()
                .find(|row| row.path == creating.parent_dir)
                .map(|row| row.depth + 1)
        })
        .unwrap_or(0);
    let item_count = rows.len() + if inline_pos.is_some() { 1 } else { 0 };

    let list = uniform_list(
        "explorer-tree-list",
        item_count,
        move |range, _window, app| {
            let row_data = Arc::clone(&row_data);
            let open = open.clone();
            let focused_path = focused_path.clone();
            let selection = selection.clone();
            let cut_paths = cut_paths.clone();
            let drag_target = drag_target.clone();
            let creating = creating.clone();
            let renaming = renaming.clone();
            let git_map = Arc::clone(&git_map);
            workspace.update(app, |_, cx| {
                range
                    .map(|idx| {
                        if inline_pos == Some(idx) {
                            return inline_create_row(
                                creating.as_ref().expect("inline row has creation state"),
                                inline_depth,
                                &style.colors,
                                cx,
                            )
                            .into_any_element();
                        }
                        let row_idx = if inline_pos.is_some_and(|inline_ix| idx > inline_ix) {
                            idx - 1
                        } else {
                            idx
                        };
                        let row = &row_data[row_idx];
                        let git_kind = git_map.get(&row.path).copied();
                        tree_row(
                            row_idx,
                            row,
                            RowState {
                                is_open: open.as_ref() == Some(&row.path),
                                is_focused: focused_path.as_ref() == Some(&row.path),
                                is_selected: selection.contains(&row.path),
                                is_cut: cut_paths.contains(&row.path),
                                is_drop_target: drag_target.as_ref() == Some(&row.path),
                                selection_size: selection.len().max(1),
                                git_kind,
                            },
                            renaming.as_ref(),
                            style,
                            cx,
                        )
                    })
                    .collect::<Vec<AnyElement>>()
            })
        },
    )
    .track_scroll(scroll_handle.clone())
    .track_focus(&focus_handle)
    .w_full()
    .h_full()
    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
        this.handle_explorer_key(event, window, cx);
    }));

    // Sticky headers are a decoration *of the list*, not an overlay on top of
    // it (see `StickyFolders`). Attaching them here is what keeps scrolling
    // smooth and keeps the wheel working over the headers.
    let list = if sticky_enabled {
        list.with_decoration(StickyFolders {
            workspace: cx.entity(),
            style,
        })
    } else {
        list
    };

    let root_for_blank = root_path.map(|path| path.to_path_buf());
    let empty_area_root = root_for_blank.clone();
    let scroll_for_drag = scroll_handle.clone();

    let container = div()
        .id("explorer-tree-body")
        .relative()
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        // Clicking empty space below the tree clears the selection and drops
        // a dropped file into the workspace root, like VS Code.
        .on_click(cx.listener(|this, _, window, cx| {
            window.focus(&this.explorer_focus_handle);
            this.clear_explorer_selection(cx);
        }))
        // Dropping on blank space below the tree moves into the workspace
        // root. No highlight here: rows highlight themselves, and a panel
        // wide flash whenever a drag passes over a child looks wrong.
        .on_drop(cx.listener(move |this, drag: &ExplorerDrag, _window, cx| {
            if let Some(root) = empty_area_root.clone() {
                this.explorer_drop(&drag.path, &root, cx);
            }
        }))
        // Dragging near the top or bottom edge scrolls the list, so an item
        // can be dropped outside the current viewport.
        .on_drag_move(cx.listener(
            move |_this, event: &DragMoveEvent<ExplorerDrag>, window, _cx| {
                if auto_scroll_during_drag(&scroll_for_drag, event.bounds, event.event.position) {
                    window.refresh();
                }
            },
        ))
        .child(list);

    container
        .context_menu(move |menu, _window, _cx| {
            let parent = root_for_blank.clone();
            menu.menu(
                "New File…",
                Box::new(ExplorerNewFile {
                    parent: parent.clone(),
                }),
            )
            .menu("New Folder…", Box::new(ExplorerNewFolder { parent }))
            .separator()
            .menu("Paste", Box::new(ExplorerPaste))
            .separator()
            .menu("Refresh Explorer", Box::new(ExplorerRefresh))
            .menu("Collapse All Folders", Box::new(ExplorerCollapseAll))
        })
        .into_any_element()
}

/// Scroll the tree while a drag hovers near one of its edges.
fn auto_scroll_during_drag(
    scroll_handle: &UniformListScrollHandle,
    bounds: Bounds<Pixels>,
    position: Point<Pixels>,
) -> bool {
    if !bounds.contains(&position) {
        return false;
    }
    let from_top = f32::from(position.y - bounds.origin.y);
    let from_bottom = f32::from(bounds.origin.y + bounds.size.height - position.y);
    let delta = if from_top < DRAG_SCROLL_MARGIN {
        ROW_HEIGHT
    } else if from_bottom < DRAG_SCROLL_MARGIN {
        -ROW_HEIGHT
    } else {
        return false;
    };
    scroll_by(scroll_handle, px(delta));
    true
}

/// Shift the list by `delta` pixels.
///
/// The limit comes from the list's own `max_offset`, which its prepaint keeps
/// up to date — computing a limit here from a row count and a row height would
/// be a second opinion, and the two disagreeing at the end of the list is
/// exactly what makes a tree judder.
fn scroll_by(scroll_handle: &UniformListScrollHandle, delta: Pixels) {
    let base = scroll_handle.0.borrow().base_handle.clone();
    let min_y = -base.max_offset().height;
    let mut offset = base.offset();
    offset.y += delta;
    if offset.y > px(0.0) {
        offset.y = px(0.0);
    }
    if offset.y < min_y {
        offset.y = min_y;
    }
    base.set_offset(offset);
}

/// Sticky scroll: the ancestor folders of whatever is at the top of the
/// viewport, pinned to the top of the list and drifting out as their section
/// ends.
///
/// This is a `UniformListDecoration` rather than an overlay element, which is
/// how Zed's project panel does it, and the distinction is what makes it feel
/// right:
///
/// * A decoration is computed during the list's *prepaint*, so it sees the
///   scroll offset the list has already clamped, the measured item height and
///   the real visible range. An overlay has to read the offset a frame late
///   during `render`, and at the ends of the list — where the offset is still
///   overscrolled when read but clamped when painted — the two disagree and
///   the pinned rows visibly shake.
/// * It is prepainted inside the list's own hitbox, so the wheel keeps
///   scrolling the tree while the pointer is over a header. The overlay had to
///   forward wheel events by hand, and because GPUI hitboxes do not block by
///   default the list handled the same event *as well*, scrolling twice per
///   tick and fighting its own clamp at the bottom.
/// * Nothing outside the list is mutated per frame, so scrolling no longer
///   drags the whole panel through an extra layout pass.
struct StickyFolders {
    workspace: Entity<Workspace>,
    style: RowStyle,
}

impl UniformListDecoration for StickyFolders {
    fn compute(
        &self,
        _visible_range: Range<usize>,
        bounds: Bounds<Pixels>,
        scroll_offset: Point<Pixels>,
        item_height: Pixels,
        _item_count: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let row_height = f32::from(item_height);
        let scroll_top = f32::from(-scroll_offset.y).max(0.0);
        let style = self.style;

        let (mut rest, shift) = self.workspace.update(cx, |workspace, cx| {
            // The inline "new file" editor inserts a row the flat model does
            // not know about. Rather than shift every index for a state that
            // lasts a couple of seconds, the headers step aside for it.
            if !workspace.explorer_sticky_scroll || workspace.inline_creating.is_some() {
                workspace.explorer_sticky_rows = 0;
                return (Vec::new(), 0.0);
            }
            let rows = Arc::clone(&workspace.explorer_rows);
            let layout = sticky_layout(&rows, scroll_top, row_height, STICKY_MAX_ROWS);
            // Keyboard navigation reveals rows past the headers; it reads this
            // back. Deliberately no `notify` — this runs inside prepaint.
            workspace.explorer_sticky_rows = layout.rows.len();
            let elements = layout
                .rows
                .iter()
                .enumerate()
                .filter_map(|(slot, &ix)| {
                    let is_last = slot + 1 == layout.rows.len();
                    rows.get(ix)
                        .map(|row| sticky_row(ix, row, is_last, style, cx))
                })
                .collect::<Vec<AnyElement>>();
            (elements, layout.shift)
        });

        if rest.is_empty() {
            return StickyFoldersElement {
                drifting: None,
                rest: Vec::new(),
            }
            .into_any_element();
        }

        // `bounds.origin` is the scrolled content origin; undo the scroll to
        // get back to the top edge of the viewport.
        let base_origin = bounds.origin - point(px(0.0), scroll_offset.y);
        let available = size(
            AvailableSpace::Definite(bounds.size.width),
            AvailableSpace::Definite(item_height),
        );

        // Only the innermost header drifts; the outer ones never move, so the
        // stack stays rock steady while its last row slides away behind them.
        let mut drifting = if shift > 0.0 { rest.pop() } else { None };

        for (slot, element) in rest.iter_mut().enumerate() {
            element.layout_as_root(available, window, cx);
            element.prepaint_at(base_origin + point(px(0.0), item_height * slot), window, cx);
        }
        if let Some(element) = drifting.as_mut() {
            let y = item_height * rest.len() - px(shift);
            element.layout_as_root(available, window, cx);
            element.prepaint_at(base_origin + point(px(0.0), y), window, cx);
        }

        StickyFoldersElement { drifting, rest }.into_any_element()
    }
}

/// Paints the pinned headers. Painting order is the z-order: the drifting row
/// goes down first so it slides *behind* the headers above it, and the rest are
/// painted bottom-up so the outermost folder ends up on top.
struct StickyFoldersElement {
    drifting: Option<AnyElement>,
    rest: Vec<AnyElement>,
}

impl IntoElement for StickyFoldersElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for StickyFoldersElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (window.request_layout(Style::default(), [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(drifting) = self.drifting.as_mut() {
            drifting.paint(window, cx);
        }
        for element in self.rest.iter_mut().rev() {
            element.paint(window, cx);
        }
    }
}

fn sticky_row(
    index: usize,
    row_data: &VisibleTreeRow,
    is_last: bool,
    style: RowStyle,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let t = style.colors;
    let pad = BASE_PAD + row_data.depth as f32 * INDENT_STEP;
    let path = row_data.path.clone();
    let name = row_data.name.clone();
    let icon_path = file_icons::folder_icon_for(&row_data.path, row_data.expanded);
    let chevron = if row_data.expanded {
        "ui_icons/chevron-down_tint.svg"
    } else {
        "ui_icons/chevron-right_tint.svg"
    };

    let toggle_path = path.clone();
    // Its own ancestors stay pinned above it once we scroll to it.
    let own_ancestors = row_data.depth.min(STICKY_MAX_ROWS);
    div()
        .id(("sticky-row", index))
        .w_full()
        .h(px(ROW_HEIGHT))
        .flex()
        .flex_row()
        .items_center()
        .pl(px(pad))
        .pr(px(8.0))
        .cursor_pointer()
        // Opaque, so the rows scrolling underneath stay hidden, with a hairline
        // under the innermost header to separate the stack from the tree.
        .bg(rgba(t.panel))
        .when(is_last, |row| {
            row.border_b_1().border_color(rgba(t.border_variant))
        })
        .hover(|s| s.bg(rgba(t.ghost_hover)))
        // Clicking a sticky header jumps to that folder, as in VS Code.
        .on_click(
            cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                window.focus(&this.explorer_focus_handle);
                this.set_explorer_selection(path.clone());
                if event.modifiers().alt {
                    this.toggle_dir_recursive(&path, cx);
                } else if let Some(ix) = this
                    .explorer_rows
                    .iter()
                    .position(|candidate| candidate.path == path)
                {
                    // Scroll just far enough that this folder stops being sticky:
                    // it lands directly under its own pinned ancestors, which is
                    // what Zed's project panel does with a sticky item click.
                    this.explorer_scroll_handle
                        .scroll_to_item_strict_with_offset(
                            ix,
                            gpui::ScrollStrategy::Top,
                            own_ancestors,
                        );
                }
                cx.notify();
                cx.stop_propagation();
            }),
        )
        .child(
            div()
                .id(("sticky-chevron", index))
                .w(px(16.0))
                .h(px(16.0))
                .flex()
                .items_center()
                .justify_center()
                .flex_none()
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.toggle_dir(&toggle_path, cx);
                    cx.stop_propagation();
                }))
                .child(
                    svg()
                        .path(chevron)
                        .w(px(12.0))
                        .h(px(12.0))
                        .text_color(rgba(t.icon_muted)),
                ),
        )
        .child(div().w(px(2.0)).flex_none())
        .child(icon_img(icon_path, ICON_SIZE))
        .child(div().w(px(6.0)).flex_none())
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .text_ellipsis()
                .text_size(px(TEXT_SIZE))
                .text_color(rgba(t.text))
                .child(SharedString::from(name)),
        )
        .into_any_element()
}

fn header_action_button(
    id: &'static str,
    icon_path: &'static str,
    tooltip: &'static str,
    t: &Colors,
    action: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    div()
        .id(id)
        .size(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(t.element_hover)))
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .child(
            svg()
                .path(icon_path)
                .w(px(16.0))
                .h(px(16.0))
                .text_color(rgba(t.icon_muted)),
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            action(this, window, cx);

            cx.stop_propagation();
        }))
}

/// Indent guides for one row: a thin rule per ancestor level, with the guide
/// belonging to the focused branch highlighted.
fn indent_guides(
    mut row: gpui::Stateful<gpui::Div>,
    idx: usize,
    depth: usize,
    style: RowStyle,
) -> gpui::Stateful<gpui::Div> {
    for d in 0..depth {
        let active = style.active_guide.is_some_and(|(guide_depth, start, end)| {
            guide_depth == d && idx >= start && idx <= end
        });
        let color = if active { 0xffffff55 } else { 0xffffff1e };
        let guide_x = BASE_PAD + d as f32 * INDENT_STEP + 7.0;
        row = row.child(
            div()
                .absolute()
                .left(px(guide_x))
                .top(px(0.0))
                .bottom(px(0.0))
                .w(px(1.0))
                .bg(rgba(color)),
        );
    }
    row
}

fn inline_create_row(
    creating: &InlineCreating,
    depth: usize,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let pad = BASE_PAD + depth as f32 * INDENT_STEP;

    let mut row = div()
        .id("inline-create-row")
        .w_full()
        .h(px(ROW_HEIGHT))
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
            if event.keystroke.key == "escape" {
                this.cancel_inline_create(cx);
            } else if event.keystroke.key == "enter" {
                this.confirm_inline_create(cx);
            }
        }));

    row = indent_guides(
        row,
        0,
        depth,
        RowStyle {
            colors: *t,
            tree_focused: true,
            active_guide: None,
        },
    );

    let icon = match creating.kind {
        CreatingKind::File => "file_icons/default_file.svg",
        CreatingKind::Folder => file_icons::FOLDER_COLLAPSED,
    };

    let content = div()
        .w_full()
        .h_full()
        .flex()
        .flex_row()
        .items_center()
        .pl(px(pad))
        .pr(px(10.0))
        .child(div().w(px(16.0)).h(px(16.0)).flex_none())
        .child(div().w(px(2.0)).flex_none())
        .child(icon_img(icon, ICON_SIZE))
        .child(div().w(px(6.0)).flex_none())
        .child(
            div()
                .flex_1()
                .h(px(20.0))
                .flex()
                .items_center()
                .bg(rgba(t.background))
                .border_1()
                .border_color(rgba(t.border_focused))
                .rounded(px(3.0))
                .px(px(2.0))
                .child(
                    Input::new(&creating.input)
                        .xsmall()
                        .text_size(px(TEXT_SIZE))
                        .appearance(false)
                        .bordered(false),
                ),
        );

    row.child(content)
}

fn inline_rename_row(
    idx: usize,
    row_data: &VisibleTreeRow,
    renaming: &InlineRenaming,
    style: RowStyle,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let t = style.colors;
    let pad = BASE_PAD + row_data.depth as f32 * INDENT_STEP;
    let icon_path = if row_data.is_dir {
        file_icons::folder_icon_for(&row_data.path, row_data.expanded)
    } else {
        file_icons::icon_for(&row_data.path)
    };
    let chev = if row_data.is_dir {
        let path = if row_data.expanded {
            "ui_icons/chevron-down_tint.svg"
        } else {
            "ui_icons/chevron-right_tint.svg"
        };
        div()
            .w(px(16.0))
            .h(px(16.0))
            .flex()
            .items_center()
            .justify_center()
            .flex_none()
            .child(
                svg()
                    .path(path)
                    .w(px(12.0))
                    .h(px(12.0))
                    .text_color(rgba(t.icon_muted)),
            )
    } else {
        div().w(px(16.0)).h(px(16.0)).flex_none()
    };

    let mut row = div()
        .id(("tree-row-rename", idx))
        .w_full()
        .h(px(ROW_HEIGHT))
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .bg(rgba(t.element_selected))
        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
            if event.keystroke.key == "escape" {
                this.cancel_inline_rename(cx);
            } else if event.keystroke.key == "enter" {
                this.confirm_inline_rename(cx);
            }
        }));
    row = indent_guides(row, idx, row_data.depth, style);
    row.child(
        div()
            .w_full()
            .h_full()
            .flex()
            .flex_row()
            .items_center()
            .pl(px(pad))
            .pr(px(8.0))
            .child(chev)
            .child(div().w(px(2.0)).flex_none())
            .child(icon_img(icon_path, ICON_SIZE))
            .child(div().w(px(6.0)).flex_none())
            .child(
                div()
                    .flex_1()
                    .h(px(20.0))
                    .flex()
                    .items_center()
                    .bg(rgba(t.background))
                    .border_1()
                    .border_color(rgba(t.border_focused))
                    .rounded(px(3.0))
                    .px(px(2.0))
                    .child(
                        Input::new(&renaming.input)
                            .xsmall()
                            .text_size(px(TEXT_SIZE))
                            .appearance(false)
                            .bordered(false),
                    ),
            ),
    )
    .into_any_element()
}

/// What makes one row look different from its neighbours.
#[derive(Clone, Copy)]
struct RowState {
    is_open: bool,
    is_focused: bool,
    is_selected: bool,
    is_cut: bool,
    is_drop_target: bool,
    selection_size: usize,
    git_kind: Option<ChangeKind>,
}

fn tree_row(
    idx: usize,
    row_data: &VisibleTreeRow,
    state: RowState,
    renaming: Option<&InlineRenaming>,
    style: RowStyle,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    if let Some(renaming) = renaming.filter(|rename| rename.path == row_data.path) {
        return inline_rename_row(idx, row_data, renaming, style, cx);
    }
    let t = style.colors;
    let is_selected = state.is_selected || state.is_focused || state.is_open;
    let path = row_data.path.clone();
    let is_dir = row_data.is_dir;
    let expanded = row_data.expanded;
    let name = row_data.name.clone();
    let pad = BASE_PAD + row_data.depth as f32 * INDENT_STEP;

    let mut row = div()
        .id(("tree-row", idx))
        .w_full()
        .h(px(ROW_HEIGHT))
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .cursor_pointer()
        .hover(|s| s.bg(rgba(t.ghost_hover)));

    if is_selected {
        // VS Code dims the selection while the tree does not have focus.
        let bg = if style.tree_focused {
            t.element_selected
        } else {
            t.element_hover
        };
        row = row.bg(rgba(bg));
    }
    if state.is_drop_target {
        row = row.bg(rgba(t.element_selected));
    }
    if state.is_cut {
        row = row.opacity(0.5);
    }

    row = indent_guides(row, idx, row_data.depth, style);

    // Focus ring, drawn as an overlay so it never shifts the row's layout.
    if state.is_focused && style.tree_focused {
        row = row.child(
            div()
                .absolute()
                .top(px(0.0))
                .bottom(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .border_1()
                .border_color(rgba(t.border_focused)),
        );
    }

    let icon_path = if is_dir {
        file_icons::folder_icon_for(&row_data.path, expanded)
    } else {
        file_icons::icon_for(&row_data.path)
    };
    let text_color = state
        .git_kind
        .map(|kind| git_kind_color(kind, &t))
        .unwrap_or(t.text);
    let chevron_element = if is_dir {
        let chev_path = if expanded {
            "ui_icons/chevron-down_tint.svg"
        } else {
            "ui_icons/chevron-right_tint.svg"
        };
        div()
            .w(px(16.0))
            .h(px(16.0))
            .flex()
            .items_center()
            .justify_center()
            .flex_none()
            .child(
                svg()
                    .path(chev_path)
                    .w(px(12.0))
                    .h(px(12.0))
                    .text_color(rgba(t.icon_muted)),
            )
    } else {
        div().w(px(16.0)).h(px(16.0)).flex_none()
    };

    let content = div()
        .w_full()
        .h_full()
        .flex()
        .flex_row()
        .items_center()
        .pl(px(pad))
        .pr(px(8.0))
        .child(chevron_element)
        .child(div().w(px(2.0)).flex_none())
        .child(icon_img(icon_path, ICON_SIZE))
        .child(div().w(px(6.0)).flex_none())
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .text_ellipsis()
                .text_size(px(TEXT_SIZE))
                .text_color(rgba(text_color))
                .child(SharedString::from(name)),
        )
        // Git status letter (files only; directories just get the tint).
        .when_some(state.git_kind.filter(|_| !is_dir), |d, kind| {
            d.child(
                div()
                    .flex_none()
                    .pl(px(4.0))
                    .text_size(px(11.0))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgba(git_kind_color(kind, &t)))
                    .child(SharedString::from(kind.letter())),
            )
        });

    let tooltip_text = SharedString::from(path.to_string_lossy().into_owned());
    let path_click = path.clone();
    let drag_count = if state.is_selected {
        state.selection_size
    } else {
        1
    };
    row = row
        .child(content)
        .tooltip(move |window, cx| Tooltip::new(tooltip_text.clone()).build(window, cx))
        .on_click(
            cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                this.explorer_row_click(
                    path_click.clone(),
                    is_dir,
                    event.modifiers(),
                    event.click_count(),
                    window,
                    cx,
                );
                cx.stop_propagation();
            }),
        )
        .on_drag(
            ExplorerDrag {
                path: path.clone(),
                count: drag_count,
            },
            |drag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            },
        );

    // Files accept drops too — they redirect into the containing folder, and
    // folders expand when a drag hovers over them for a moment.
    let drop_path = path.clone();
    let hover_dir = if is_dir {
        Some(path.clone())
    } else {
        path.parent().map(Path::to_path_buf)
    };
    let leave_dir = hover_dir.clone();
    let drop_color = t.element_selected;
    row = row
        .drag_over::<ExplorerDrag>(move |this, _, _, _| this.bg(rgba(drop_color)))
        .on_drag_move(cx.listener(
            move |this, event: &DragMoveEvent<ExplorerDrag>, _window, cx| {
                if event.bounds.contains(&event.event.position) {
                    if let Some(dir) = hover_dir.clone() {
                        this.explorer_drag_over(dir, cx);
                    }
                } else if let Some(dir) = leave_dir.as_ref() {
                    this.explorer_drag_leave(dir, cx);
                }
            },
        ))
        .on_drop(cx.listener(move |this, drag: &ExplorerDrag, _window, cx| {
            this.explorer_drop(&drag.path, &drop_path, cx);
            cx.stop_propagation();
        }));

    // Right-click selects the row unless it is already part of the selection,
    // so "Delete" on a multi-selection keeps acting on all of it.
    let context_select_path = path.clone();
    row = row.on_mouse_down(
        MouseButton::Right,
        cx.listener(move |this, _, window, cx| {
            window.focus(&this.explorer_focus_handle);
            let selection = this.explorer_selected_entries();
            if !selection.contains(&context_select_path) {
                this.set_explorer_selection(context_select_path.clone());
            }
            cx.notify();
        }),
    );

    row.context_menu(move |menu, _window, _cx| {
        let parent_dir = if is_dir {
            Some(path.clone())
        } else {
            path.parent().map(Path::to_path_buf)
        };
        menu.menu(
            "New File…",
            Box::new(ExplorerNewFile {
                parent: parent_dir.clone(),
            }),
        )
        .menu(
            "New Folder…",
            Box::new(ExplorerNewFolder {
                parent: parent_dir.clone(),
            }),
        )
        .separator()
        .menu(
            "Reveal in File Explorer",
            Box::new(ExplorerRevealInFinder { path: path.clone() }),
        )
        .menu(
            "Open in Integrated Terminal",
            Box::new(ExplorerOpenInTerminal { path: path.clone() }),
        )
        .menu(
            "Find in Folder…",
            Box::new(ExplorerFindInFolder { path: path.clone() }),
        )
        .separator()
        .menu("Cut", Box::new(ExplorerCut))
        .menu("Copy", Box::new(ExplorerCopy))
        .menu("Paste", Box::new(ExplorerPaste))
        .separator()
        .menu(
            "Copy Path",
            Box::new(ExplorerCopyPath { path: path.clone() }),
        )
        .menu(
            "Copy Relative Path",
            Box::new(ExplorerCopyRelativePath { path: path.clone() }),
        )
        .separator()
        .menu(
            "Duplicate",
            Box::new(ExplorerDuplicate { path: path.clone() }),
        )
        .menu("Rename…", Box::new(ExplorerRename { path: path.clone() }))
        .menu("Delete", Box::new(ExplorerDelete { path: path.clone() }))
    })
    .into_any_element()
}

impl Render for ExplorerDrag {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let is_dir = self.path.is_dir();
        let icon_path = if is_dir {
            file_icons::folder_icon_for(&self.path, false)
        } else {
            file_icons::icon_for(&self.path)
        };
        let name = self
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file");
        let label = if self.count > 1 {
            format!("{} +{}", name, self.count - 1)
        } else {
            name.to_string()
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(4.0))
            .bg(rgba(0x252526f0))
            .border_1()
            .border_color(rgba(0x454545ff))
            .text_size(px(TEXT_SIZE))
            .text_color(rgba(0xccccccff))
            .child(icon_img(icon_path, ICON_SIZE))
            .child(div().w(px(6.0)).flex_none())
            .child(SharedString::from(label))
    }
}
