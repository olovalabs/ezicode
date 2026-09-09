use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, rgba, svg, uniform_list, AnyElement, Context, FocusHandle, FontWeight,
    IntoElement, MouseButton, Render, SharedString, UniformListScrollHandle, Window,
};
use gpui_component::{input::Input, menu::ContextMenuExt, tooltip::Tooltip, Sizable};

use crate::actions::{
    ExplorerCollapseAll, ExplorerCopy, ExplorerCopyPath, ExplorerCopyRelativePath, ExplorerCut,
    ExplorerDelete, ExplorerNewFile, ExplorerNewFolder, ExplorerPaste, ExplorerRefresh,
    ExplorerRename, ExplorerRevealInFinder,
    OpenFolder,
};
use crate::file_icons;
use crate::fs_tree::VisibleTreeRow;
use crate::theme::Colors;
use crate::ui::common::icon_img;
use crate::workspace::{
    CreatingKind, ExplorerDrag, InlineCreating, InlineRenaming, Workspace,
};

const INDENT_STEP: f32 = 14.0;
const BASE_PAD: f32 = 12.0;
const ROW_HEIGHT: f32 = 24.0;
const ICON_SIZE: f32 = 16.0;

pub(crate) fn render_tree(
    rows: Arc<[VisibleTreeRow]>,
    scroll_handle: UniformListScrollHandle,
    focus_handle: FocusHandle,
    root_path: Option<&Path>,
    open: Option<&PathBuf>,
    selected_path: Option<&PathBuf>,
    section_expanded: bool,
    inline_creating: Option<&InlineCreating>,
    inline_renaming: Option<&InlineRenaming>,
    folder: &SharedString,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let mut col = div()
        .size_full()
        .flex()
        .flex_col()
        .bg(rgba(t.panel))
        .overflow_hidden();

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

    let r_context = root_path.map(|path| path.to_path_buf());
    let drop_root = r_context.clone();
    let drop_color = t.element_selected;

    let header = div()
        .id("exp-root-header")
        .h(px(34.0))
        .px(px(8.0))
        .flex()
        .flex_row()
        .items_center()
        .cursor_pointer()
        .hover(|s| s.bg(rgba(t.ghost_hover)))
        .on_click(cx.listener(|this, _, _, cx| {
            this.toggle_explorer_section(cx);
        }))

        .drag_over::<ExplorerDrag>(move |this, _, _, _| {
            this.bg(rgba(drop_color))
        })
        .on_drop(cx.listener(move |this, drag: &ExplorerDrag, _window, cx| {
            if let Some(root) = drop_root.clone() {
                this.move_entry(&drag.path, &root, cx);
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
                .separator()
                .when(p3.is_some(), |m| {
                    m.menu(
                        "Reveal in File Explorer",
                        Box::new(ExplorerRevealInFinder { path: p3.unwrap() }),
                    )
                })
                .when(p4.is_some(), |m| {
                    m.menu("Copy Path", Box::new(ExplorerCopyPath { path: p4.unwrap() }))
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
                        .w(px(14.0))
                        .h(px(14.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_none()
                        .child(
                            svg()
                                .path(root_chevron)
                                .w(px(10.0))
                                .h(px(10.0))
                                .text_color(rgba(t.icon_muted)),
                        ),
                )
                .child(icon_img(root_folder_icon, ICON_SIZE))
                .child(
                    div()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgba(t.text))

                        .child(folder.clone()),
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
        ));
    col = col.child(header);

    if section_expanded {
        let workspace = cx.entity();
        let open = open.cloned();
        let selected_path = selected_path.cloned();
        let row_data = Arc::clone(&rows);
        let creating = inline_creating.cloned();
        let renaming = inline_renaming.cloned();
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
        let list_focus = focus_handle.clone();
        let colors = *t;
        let list = uniform_list("explorer-tree-list", item_count, move |range, _window, app| {
            let row_data = Arc::clone(&row_data);
            let open = open.clone();
            let selected_path = selected_path.clone();
            let creating = creating.clone();
            let renaming = renaming.clone();
            workspace.update(app, |_, cx| {
                range
                    .map(|idx| {
                        if inline_pos == Some(idx) {
                            return inline_create_row(
                                creating.as_ref().expect("inline row has creation state"),
                                inline_depth,
                                &colors,
                                cx,
                            )
                            .into_any_element();
                        }
                        let row_idx = if inline_pos.is_some_and(|inline_ix| idx > inline_ix) {
                            idx - 1
                        } else {
                            idx
                        };
                        tree_row(
                            idx,
                            &row_data[row_idx],
                            open.as_ref(),
                            selected_path.as_ref(),
                            renaming.as_ref(),
                            colors,
                            cx,
                        )
                    })
                    .collect::<Vec<AnyElement>>()
            })
        })
        .track_scroll(scroll_handle)
        .track_focus(&list_focus)
        .w_full()
        .flex_1()
        .min_h(px(0.0))
        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
            this.handle_explorer_key(event, window, cx);
        }));
        col = col.child(list);
    }

    col.into_any_element()
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
                .w(px(14.0))
                .h(px(14.0))
                .text_color(rgba(t.icon_muted)),
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            action(this, window, cx);

            cx.stop_propagation();
        }))
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

    for d in 0..depth {
        let guide_x = BASE_PAD + d as f32 * INDENT_STEP + 3.0;
        row = row.child(
            div()
                .absolute()
                .left(px(guide_x))
                .top(px(0.0))
                .bottom(px(0.0))
                .w(px(1.0))
                .bg(rgba(0xffffff1e)),
        );
    }

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
        .child(div().w(px(14.0)).h(px(14.0)).flex_none())
        .child(div().w(px(4.0)).flex_none())
        .child(icon_img(icon, ICON_SIZE))
        .child(div().w(px(6.0)).flex_none())
        .child(
            div()
                .flex_1()
                .h(px(22.0))
                .flex()
                .items_center()
                .bg(rgba(t.background))
                .border_1()
                .border_color(rgba(t.border_focused))
                .rounded(px(4.0))
                .px(px(2.0))
                .child(
                    Input::new(&creating.input)
                        .xsmall()
                        .text_size(px(13.5))
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
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> AnyElement {
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
            .w(px(14.0))
            .h(px(14.0))
            .flex()
            .items_center()
            .justify_center()
            .flex_none()
            .child(svg().path(path).w(px(10.0)).h(px(10.0)).text_color(rgba(t.icon_muted)))
    } else {
        div().w(px(14.0)).h(px(14.0)).flex_none()
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
    for d in 0..row_data.depth {
        let guide_x = BASE_PAD + d as f32 * INDENT_STEP + 3.0;
        row = row.child(
            div()
                .absolute()
                .left(px(guide_x))
                .top(px(0.0))
                .bottom(px(0.0))
                .w(px(1.0))
                .bg(rgba(0xffffff1e)),
        );
    }
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
            .child(div().w(px(4.0)).flex_none())
            .child(icon_img(icon_path, ICON_SIZE))
            .child(div().w(px(6.0)).flex_none())
            .child(
                div()
                    .flex_1()
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .bg(rgba(t.background))
                    .border_1()
                    .border_color(rgba(t.border_focused))
                    .rounded(px(4.0))
                    .px(px(2.0))
                    .child(
                        Input::new(&renaming.input)
                            .xsmall()
                            .text_size(px(13.5))
                            .appearance(false)
                            .bordered(false),
                    ),
            ),
    )
    .into_any_element()
}

fn tree_row(
    idx: usize,
    row_data: &VisibleTreeRow,
    open: Option<&PathBuf>,
    selected_path: Option<&PathBuf>,
    renaming: Option<&InlineRenaming>,
    t: Colors,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    if let Some(renaming) = renaming.filter(|rename| rename.path == row_data.path) {
        return inline_rename_row(idx, row_data, renaming, &t, cx);
    }
    let is_open = open.is_some_and(|p| p == &row_data.path);
    let is_selected = selected_path.is_some_and(|p| p == &row_data.path) || is_open;
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
        row = row.bg(rgba(t.element_selected));
    }

    for d in 0..row_data.depth {
        let guide_x = BASE_PAD + d as f32 * INDENT_STEP + 3.0;
        row = row.child(
            div()
                .absolute()
                .left(px(guide_x))
                .top(px(0.0))
                .bottom(px(0.0))
                .w(px(1.0))
                .bg(rgba(0xffffff1e)),
        );
    }

    let icon_path = if is_dir {
        file_icons::folder_icon_for(&row_data.path, expanded)
    } else {
        file_icons::icon_for(&row_data.path)
    };
    let text_color = t.text;

    let chevron_element = if is_dir {
        let chev_path = if expanded {
            "ui_icons/chevron-down_tint.svg"
        } else {
            "ui_icons/chevron-right_tint.svg"
        };
        div()
            .w(px(14.0))
            .h(px(14.0))
            .flex()
            .items_center()
            .justify_center()
            .flex_none()
            .child(
                svg()
                    .path(chev_path)
                    .w(px(10.0))
                    .h(px(10.0))
                    .text_color(rgba(t.icon_muted)),
            )
    } else {
        div().w(px(14.0)).h(px(14.0)).flex_none()
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
        .child(div().w(px(4.0)).flex_none())
        .child(icon_img(icon_path, ICON_SIZE))
        .child(div().w(px(6.0)).flex_none())
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .text_ellipsis()
                .text_size(px(13.5))
                .text_color(rgba(text_color))
                .child(SharedString::from(name)),
        );

    let path_click = path.clone();
    row = row
        .child(content)
        .on_click(cx.listener(move |this, _, window, cx| {
            window.focus(&this.explorer_focus_handle);
            this.selected_path = Some(path_click.clone());
            if is_dir {
                this.toggle_dir(&path_click, cx);
            } else {
                this.open_file(path_click.clone(), window, cx);
            }
        }))

        .on_drag(ExplorerDrag { path: path.clone() }, |drag, _, _, cx| {
            cx.stop_propagation();
            cx.new(|_| drag.clone())
        });

    if is_dir {
        let drop_path = path.clone();
        let drop_color = t.element_selected;
        row = row
            .drag_over::<ExplorerDrag>(move |this, _, _, _| {
                this.bg(rgba(drop_color))
            })
            .on_drop(cx.listener(move |this, drag: &ExplorerDrag, _window, cx| {
                this.move_entry(&drag.path, &drop_path, cx);
            }));
    }

    let context_select_path = path.clone();
    row = row.on_mouse_down(
        MouseButton::Right,
        cx.listener(move |this, _, _, cx| {
            this.selected_path = Some(context_select_path.clone());
            cx.notify();
        }),
    );

    let path_c1 = path.clone();
    let path_c2 = path.clone();
    let path_c3 = path.clone();
    let path_c4 = path.clone();
    let path_c5 = path.clone();
    let parent_for_new = if is_dir {
        Some(path.clone())
    } else {
        path.parent().map(|p| p.to_path_buf())
    };

    row.context_menu(move |menu, _window, _cx| {
        let p_new1 = parent_for_new.clone();
        let p_new2 = parent_for_new.clone();
        menu.menu("New File…", Box::new(ExplorerNewFile { parent: p_new1 }))
            .menu("New Folder…", Box::new(ExplorerNewFolder { parent: p_new2 }))
            .when(is_dir, |m| m.menu("Paste", Box::new(ExplorerPaste)))
            .separator()
            .menu("Cut", Box::new(ExplorerCut))
            .menu("Copy", Box::new(ExplorerCopy))
            .separator()
            .menu(
                "Reveal in File Explorer",
                Box::new(ExplorerRevealInFinder { path: path_c1.clone() }),
            )
            .separator()
            .menu(
                "Copy Path",
                Box::new(ExplorerCopyPath { path: path_c2.clone() }),
            )
            .menu(
                "Copy Relative Path",
                Box::new(ExplorerCopyRelativePath { path: path_c3.clone() }),
            )
            .separator()
            .menu("Rename…", Box::new(ExplorerRename { path: path_c4.clone() }))
            .menu("Delete", Box::new(ExplorerDelete { path: path_c5.clone() }))
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
            .text_size(px(13.0))
            .text_color(rgba(0xccccccff))
            .child(icon_img(icon_path, ICON_SIZE))
            .child(div().w(px(6.0)).flex_none())
            .child(SharedString::from(name.to_string()))
    }
}
