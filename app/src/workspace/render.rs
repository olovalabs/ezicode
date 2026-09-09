//! Root layout. Composes title bar → activity bar + sidebar + editor area
//! (+ terminal) → status bar into a single column, and wires every action to
//! its [`Workspace`] command.

use gpui::{
    div, prelude::*, px, rgba, Context, MouseButton, MouseMoveEvent, MouseUpEvent, Render, Window,
};
use gpui_component::input::Input;

use crate::actions::*;
use crate::lang;

use crate::theme::Colors;
use crate::ui;
use crate::workspace::CreatingKind;

use super::{Activity, PanelResizeDrag, ResizeKind, Workspace};

/// Pixels of chrome reserved above/below the terminal so it can be dragged to
/// full height: title bar (34) + status bar (26) = 60px.
/// Drag the divider up and the terminal expands to full height.
const TERMINAL_MAX_RESERVE: f32 = 60.0;

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(path) = self.pending_open.take() {
            self.open_file(path, window, cx);
        }
        // Enter in the commit box was pressed (the input's event callback has
        // no window handle) — run the commit now that the window is here.
        if self.git_commit_pending {
            self.git_commit_pending = false;
            self.git_commit(window, cx);
        }

        // Poll terminal processes for exit (Zed-style lifecycle monitoring).
        // When a shell exits, its tab shows a red dot and the exit status
        // without requiring the user to discover it by typing into a dead PTY.
        if self.show_terminal && !self.terminal_tabs.is_empty() {
            self.poll_terminal_processes(cx);
        }

        let th = self.theme();
        let t = th.colors;
        let welcome = self.welcome_visible();
        let title = self.title();

        // Clamp panel sizes against the current viewport so shrinking or
        // maximizing the window never leaves a panel overflowing — and so
        // the panels keep their absolute pixel size on window resize
        // (VS Code behavior) instead of scaling proportionally. (Must happen
        // before the field borrows below, since both mutate `self`.)
        let max_sidebar = f32::from(window.viewport_size().width - px(320.0)).max(220.0);
        let min_sidebar = if self.panel_resize.is_some() { 60.0 } else { 170.0 };
        self.sidebar_width = self.sidebar_width.clamp(min_sidebar, max_sidebar);
        let sidebar_w = self.sidebar_width;
        let max_terminal = f32::from(window.viewport_size().height - px(TERMINAL_MAX_RESERVE))
            .max(120.0);
        let min_terminal = if self.panel_resize.is_some() { 45.0 } else { 80.0 };
        self.terminal_height = self.terminal_height.clamp(min_terminal, max_terminal);
        let terminal_h = self.terminal_height;
        let panel_resize = self.panel_resize;

        // Borrowed views of state — the render helpers below only read these,
        // so the per-frame deep clones (tabs, terminal_tabs, paths, strings,
        // ...) are unnecessary and have been removed.
        let explorer_rows = &self.explorer_rows;
        let explorer_scroll_handle = self.explorer_scroll_handle.clone();
        let explorer_focus_handle = self.explorer_focus_handle.clone();
        // Get the currently open file from active tab
        let open = self.active_path();
        let selected_path = self.selected_path.as_ref();
        let explorer_section_expanded = self.explorer_section_expanded;
        let inline_creating = self.inline_creating.as_ref();
        let inline_renaming = self.inline_renaming.as_ref();
        let root_display_shared = &self.root_display_shared;
        let status = self.status.as_str();
        let activity = self.activity;
        let root_opt = self.root.as_ref();
        let show_sidebar = self.show_sidebar;
        let show_terminal = self.show_terminal;
        let theme_ix = self.theme_ix;
        let theme_name = th.name.as_str();
        let font_size = self.font_size;
        let terminal_tabs = &self.terminal_tabs;
        let active_terminal = self.active_terminal;
        let terminal_maximized = self.terminal_maximized && self.show_terminal && !self.terminal_tabs.is_empty();

        // Tab-related data
        let tabs = &self.tabs;
        let active_tab = self.active_tab;
        let is_settings = self.tabs.get(active_tab).map(|t| t.is_settings).unwrap_or(false);
        // The active tab may be a Git diff view instead of an editor.
        let active_diff = self.tabs.get(active_tab).and_then(|t| t.diff.as_ref());
        // Get the active editor from the current tab
        let editor = self.active_editor();
        // Git state for the activity-bar badge / status bar.
        let git_repo = self.git.as_ref();
        let git_changes = git_repo.map(|g| g.change_count()).unwrap_or(0);
        let git_branch = git_repo.and_then(|g| g.branch.clone());
        let git_repo_section_expanded = self.git_repo_section_expanded;
        let git_staged_expanded = self.git_staged_expanded;
        let git_changes_expanded = self.git_changes_expanded;
        let split_diff = self.split_diff;
        let git_commit_input = self.git_commit_input.clone();
        // Active file language + LSP readiness for the status bar.
        let lang_label = open.and_then(|p| lang::language_for(p));
        let lsp_indicator = {
            let lsp = self.lsp.lock().unwrap();
            ui::status_bar::LspIndicator {
                server: lang_label.and_then(|l| lsp.server_name_for_language(l)),
                state: lang_label.and_then(|l| lsp.status_for_language(l)),
            }
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgba(t.background))
            .text_color(rgba(t.text))
            .font_family(crate::assets::SANS_FONT)
            .cursor_default()
            // Commands handled directly by the workspace.
            .on_action(cx.listener(|this, _: &Save, window, cx| this.save(window, cx)))
            .on_action(cx.listener(|this, _: &Quit, _, cx| this.quit(cx)))
            .on_action(cx.listener(|this, _: &ShowExplorer, window, cx| {
                this.set_activity_explicit(Activity::Explorer, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowSearch, window, cx| {
                this.set_activity_explicit(Activity::Search, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowGit, window, cx| {
                this.set_activity_explicit(Activity::Git, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowExtensions, window, cx| {
                this.set_activity_explicit(Activity::Extensions, window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &ToggleSidebar, _, cx| {
                    this.show_sidebar = !this.show_sidebar;
                    cx.notify();
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleTerminal, window, cx| {
                this.toggle_terminal(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NewTerminal, window, cx| {
                this.new_terminal(window, cx);
            }))
            // Terminal navigation actions (Zed-style: Alt+arrows, Alt+1..5)
            .on_action(cx.listener(|this, _: &NextTerminal, window, cx| {
                this.next_terminal_tab(window, cx);
            }))
            .on_action(cx.listener(|this, _: &PrevTerminal, window, cx| {
                this.prev_terminal_tab(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseTerminal, window, cx| {
                this.close_active_terminal(window, cx);
            }))
            .on_action(cx.listener(|this, _: &TerminalTab1, window, cx| {
                this.switch_terminal_tab_to(0, window, cx);
            }))
            .on_action(cx.listener(|this, _: &TerminalTab2, window, cx| {
                this.switch_terminal_tab_to(1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &TerminalTab3, window, cx| {
                this.switch_terminal_tab_to(2, window, cx);
            }))
            .on_action(cx.listener(|this, _: &TerminalTab4, window, cx| {
                this.switch_terminal_tab_to(3, window, cx);
            }))
            .on_action(cx.listener(|this, _: &TerminalTab5, window, cx| {
                this.switch_terminal_tab_to(4, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ClearTerminal, _window, cx| {
                this.clear_active_terminal(cx);
            }))
            .on_action(cx.listener(|this, _: &NewFile, window, cx| this.new_file(window, cx)))
            .on_action(cx.listener(|this, _: &OpenFile, window, cx| {
                this.open_file_dialog(window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenFolder, window, cx| {
                this.open_folder_dialog(window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| {
                this.open_settings(cx);
            }))
            .on_action(cx.listener(|this, action: &SelectTheme, window, cx| {
                this.apply_theme(action.ix, window, cx);
            }))
            .on_action(cx.listener(|this, _: &About, _, cx| this.about(cx)))
            .on_action(cx.listener(|this, _: &ExplorerRefresh, _, cx| {
                this.refresh_explorer(cx);
            }))
            .on_action(cx.listener(|this, _: &ExplorerCollapseAll, _, cx| {
                this.collapse_all_folders(cx);
            }))
            .on_action(cx.listener(|this, action: &ExplorerNewFile, window, cx| {
                this.start_inline_create(CreatingKind::File, action.parent.clone(), window, cx);
            }))
            .on_action(cx.listener(|this, action: &ExplorerNewFolder, window, cx| {
                this.start_inline_create(CreatingKind::Folder, action.parent.clone(), window, cx);
            }))
            .on_action(cx.listener(|this, action: &ExplorerRevealInFinder, _, cx| {
                this.reveal_in_explorer(&action.path, cx);
            }))
            .on_action(cx.listener(|this, action: &ExplorerCopyPath, _, cx| {
                this.copy_path(&action.path, cx);
            }))
            .on_action(cx.listener(|this, action: &ExplorerCopyRelativePath, _, cx| {
                this.copy_relative_path(&action.path, cx);
            }))
            .on_action(cx.listener(|this, action: &ExplorerRename, window, cx| {
                this.start_inline_rename(action.path.clone(), window, cx);
            }))
            .on_action(cx.listener(|this, action: &ExplorerDelete, _, cx| {
                this.delete_entry(&action.path, cx);
            }))
            .on_action(cx.listener(|this, _: &ExplorerCut, _, cx| {
                this.explorer_cut(cx);
            }))
            .on_action(cx.listener(|this, _: &ExplorerCopy, _, cx| {
                this.explorer_copy(cx);
            }))
            .on_action(cx.listener(|this, _: &ExplorerPaste, _, cx| {
                this.explorer_paste(cx);
            }))
            // Tab action handlers
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                this.handle_close_tab(&CloseTab, window, cx);
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| {
                this.handle_next_tab(&NextTab, window, cx);
            }))
            .on_action(cx.listener(|this, _: &PrevTab, window, cx| {
                this.handle_prev_tab(&PrevTab, window, cx);
            }))
            .on_action(cx.listener(|this, action: &SwitchTab, window, cx| {
                this.handle_switch_tab(action, window, cx);
            }))
            .on_action(cx.listener(|this, action: &CloseTabAt, window, cx| {
                this.handle_close_tab_at(action, window, cx);
            }))
            // Font zoom actions
            .on_action(cx.listener(|this, _: &IncreaseFontSize, _, cx| {
                this.increase_font_size(cx);
            }))
            .on_action(cx.listener(|this, _: &DecreaseFontSize, _, cx| {
                this.decrease_font_size(cx);
            }))
            .on_action(cx.listener(|this, _: &ResetFontSize, _, cx| {
                this.reset_font_size(cx);
            }))
            .on_action(cx.listener(|this, _: &CopyDiagnostic, _, cx| {
                this.copy_active_diagnostic(cx);
            }))
            // Format the active document via its language server.
            .on_action(cx.listener(|this, _: &FormatDocument, window, cx| {
                this.format_document(window, cx);
            }))
            // Git: source-control panel actions.
            .on_action(cx.listener(|this, _: &GitRefresh, _, cx| {
                this.git_refresh(cx);
            }))
            .on_action(cx.listener(|this, _: &GitStageAll, _, cx| {
                this.git_stage_all(cx);
            }))
            .on_action(cx.listener(|this, _: &GitUnstageAll, _, cx| {
                this.git_unstage_all(cx);
            }))
            .on_action(cx.listener(|this, _: &GitDiscardAll, _, cx| {
                this.git_discard_all(cx);
            }))
            .on_action(cx.listener(|this, action: &GitStageFile, _, cx| {
                this.git_stage_path(&action.path, cx);
            }))
            .on_action(cx.listener(|this, action: &GitUnstageFile, _, cx| {
                this.git_unstage_path(&action.path, cx);
            }))
            .on_action(cx.listener(|this, action: &GitDiscardFile, _, cx| {
                this.git_discard_path(&action.path, cx);
            }))
            .on_action(cx.listener(|this, action: &GitOpenDiff, _, cx| {
                this.open_diff(&action.path, cx);
            }))
            .on_action(cx.listener(|this, action: &GitOpenFile, window, cx| {
                this.open_file(action.path.clone(), window, cx);
            }))
            .on_action(cx.listener(|this, _: &GitCommit, window, cx| {
                this.git_commit(window, cx);
            }))
            // Zed-style key context for workspace-level bindings. Note: do NOT
            // put `track_focus` on this full-window div — its hitbox breaks
            // the platform's client-decoration hit-testing, which kills
            // title-bar dragging on Windows. The focus target lives on the
            // tiny `workspace-focus-catcher` element below instead.
            .key_context("Workspace")
            // Invisible 1x1 focus target: when no editor or terminal is
            // focused, the workspace holds focus here so global keybindings
            // always have a dispatch path (Zed keeps an equivalent workspace
            // focus handle). 1x1px so its hitbox can't interfere with the
            // title bar or window controls.
            .child(
                div()
                    .id("workspace-focus-catcher")
                    .track_focus(&self.focus_handle)
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .size(px(1.0)),
            )
            .child(ui::titlebar::render_titlebar(&title, &t, theme_ix))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .child(ui::activity_bar::render_activity_bar(
                        activity, show_sidebar, git_changes, &t, cx,
                    ))
                    // Sidebar: fixed pixel width, resized by dragging the thin
                    // divider next to it. Not affected by window resize/maximize.
                    .when(show_sidebar, |row| {
                        row.child(
                            div()
                                .w(px(sidebar_w))
                                .flex_shrink_0()
                                .h_full()
                                .overflow_hidden()
                                .child(match activity {
                                    Activity::Explorer => match root_opt {
                                        Some(root) => ui::sidebar::explorer::render_tree(
                                            explorer_rows.clone(),
                                            explorer_scroll_handle.clone(),
                                            explorer_focus_handle.clone(),
                                            Some(root.as_path()),
                                            open,
                                            selected_path,
                                            explorer_section_expanded,
                                            inline_creating,
                                            inline_renaming,
                                            root_display_shared,
                                            &t,
                                            cx,
                                        ),
                                        None => ui::welcome::render_no_folder_panel(&t, cx),
                                    },
                                    Activity::Search => ui::sidebar::search::render_search_panel(&t),
                                    Activity::Git => ui::sidebar::git::render_git_panel(
                                        git_commit_input.as_ref(),
                                        git_repo,
                                        git_repo_section_expanded,
                                        git_staged_expanded,
                                        git_changes_expanded,
                                        &t,
                                        window,
                                        cx,
                                    ),
                                    Activity::Extensions => {
                                        ui::sidebar::extensions::render_extensions_panel(&t)
                                    }
                                }),
                        )
                    })
                    // Sidebar resizer: always visible right next to the sidebar (or
                    // activity bar when hidden), allowing click, drag, or double-click to
                    // reveal and resize, matching VS Code.
                    .child(resize_handle(ResizeKind::Sidebar, &t, cx))
                    .child(
                                // Editor column absorbs all remaining space; the
                        // terminal panel below it is also a fixed-pixel panel
                        // with its own drag divider.
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            // Editor area: hidden when terminal is full-screen maximized
                            .when(!terminal_maximized, |col| {
                                col.child(
                                div()
                                    .flex_1()
                                    .min_h(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .bg(rgba(t.editor_bg))
                                    // Tab bar - only show when tabs exist
                                    .when(!tabs.is_empty(), |d| {
                                        d.child(ui::tab_bar::render_tab_bar(tabs, active_tab, git_repo, &t, cx))
                                    })
                                    // Editor area / Settings area / Welcome
                                    .when(welcome, |d| d.child(ui::welcome::render_welcome(&t, cx)))
                                    .when(!welcome, |d| {
                                        if is_settings {
                                            d.child(ui::settings::render_settings(&self.settings, &t, theme_ix, font_size, cx))
                                        } else if let Some(diff) = active_diff {
                                            d.child(ui::diff::render_diff_view(
                                                diff, font_size, split_diff, &t, cx,
                                            ))
                                        } else if let Some(editor) = editor {
                                            d.child(
                                                div()
                                                    .flex_1()
                                                    .min_h(px(0.0))
                                                    .overflow_hidden()
                                                    .font_family(crate::assets::MONO_FONT)
                                                    .text_size(px(font_size))
                                                    .child(
                                                        Input::new(editor)
                                                            .text_size(px(font_size))
                                                            .h_full()
                                                            .appearance(false)
                                                            .bordered(false),
                                                    ),
                                            )
                                        } else {
                                            d.flex_1()
                                        }
                                    }),
                                )
                            })
                            // Terminal resizer: always visible above the status bar
                            // even when collapsed, allowing drag-to-reveal or click to open like VS Code.
                            .when(!terminal_tabs.is_empty(), |col| {
                                col.child(resize_handle(ResizeKind::Terminal, &t, cx))
                                    .when(show_terminal, |col| {
                                        if terminal_maximized {
                                            col.child(
                                                div()
                                                    .flex_1()
                                                    .size_full()
                                                    .overflow_hidden()
                                                    .child(crate::terminal::render_terminal_panel(
                                                        terminal_tabs,
                                                        active_terminal,
                                                        true,
                                                        &t,
                                                        cx,
                                                    )),
                                            )
                                        } else {
                                            col.child(
                                                div()
                                                    .h(px(terminal_h))
                                                    .flex_shrink_0()
                                                    .overflow_hidden()
                                                    .child(crate::terminal::render_terminal_panel(
                                                        terminal_tabs,
                                                        active_terminal,
                                                        false,
                                                        &t,
                                                        cx,
                                                    )),
                                            )
                                        }
                                    })
                            }),
                    ),
            )
            .child(ui::status_bar::render_status_bar(
                status,
                theme_name,
                git_branch.as_deref(),
                git_changes,
                lang_label,
                lsp_indicator,
                &t,
            ))
            // While a panel divider is being dragged, capture ALL mouse
            // movement with a transparent full-window overlay. Without this
            // the drag would freeze as soon as the cursor leaves the thin
            // divider or crosses a mouse-blocking element (editor input,
            // terminal, ...), because div listeners only fire while their
            // own hitbox is hovered.
            .when(panel_resize.is_some(), |root| {
                let kind = panel_resize.unwrap().kind;
                let overlay = div()
                    .id("panel-resize-overlay")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .occlude()
                    .on_mouse_move(cx.listener(
                        |this, ev: &MouseMoveEvent, window, cx| {
                            let Some(rz) = this.panel_resize else {
                                return;
                            };
                            match rz.kind {
                                ResizeKind::Sidebar => {
                                    // Direct coordinate tracking (Zed formula): sidebar width is exactly
                                    // distance from the right edge of activity bar (x=48px) to the cursor.
                                    let dist = f32::from(ev.position.x) - 48.0;
                                    if dist < 60.0 {
                                        this.show_sidebar = false;
                                    } else {
                                        this.show_sidebar = true;
                                        let max = f32::from(window.viewport_size().width - px(320.0)).max(220.0);
                                        this.sidebar_width = dist.clamp(60.0, max);
                                    }
                                }
                                ResizeKind::Terminal => {
                                    let bottom = f32::from(window.viewport_size().height) - 26.0;
                                    let dist = bottom - f32::from(ev.position.y);
                                    let titlebar_h = 34.0;
                                    let max_avail = (f32::from(window.viewport_size().height) - 26.0 - titlebar_h).max(120.0);

                                    if dist < 45.0 {
                                        // Dragged down near status bar -> collapse to hide
                                        this.show_terminal = false;
                                        this.terminal_maximized = false;
                                    } else if dist >= max_avail - 45.0 {
                                        // Dragged all the way up near the top titlebar -> totally full screen!
                                        this.show_terminal = true;
                                        this.terminal_maximized = true;
                                        this.terminal_height = max_avail;
                                    } else {
                                        // Normal resizing
                                        this.show_terminal = true;
                                        this.terminal_maximized = false;
                                        this.terminal_height = dist.clamp(45.0, max_avail - 45.0);
                                    }
                                }
                            }
                            cx.stop_propagation();
                            cx.notify();
                        },
                    ))
                    .on_mouse_up(MouseButton::Left, cx.listener(
                        |this, ev: &MouseUpEvent, _window, cx| {
                            if let Some(rz) = this.panel_resize.take() {
                                match rz.kind {
                                    ResizeKind::Sidebar => {
                                        if !this.show_sidebar {
                                            if (f32::from(ev.position.x) - rz.start_mouse).abs() < 5.0 {
                                                this.show_sidebar = true;
                                                this.sidebar_width = 300.0;
                                            }
                                        } else {
                                            this.sidebar_width = this.sidebar_width.max(170.0);
                                        }
                                    }
                                    ResizeKind::Terminal => {
                                        if !this.show_terminal {
                                            if (rz.start_mouse - f32::from(ev.position.y)).abs() < 5.0 {
                                                this.show_terminal = true;
                                                this.terminal_height = 320.0;
                                            }
                                        } else {
                                            this.terminal_height = this.terminal_height.max(80.0);
                                        }
                                    }
                                }
                                cx.notify();
                            }
                        },
                    ))
                    .on_mouse_up(MouseButton::Right, cx.listener(
                        |this, _: &MouseUpEvent, _, cx| {
                            if this.panel_resize.take().is_some() {
                                cx.notify();
                            }
                        },
                    ));
                let overlay = if kind == ResizeKind::Sidebar {
                    overlay.cursor_col_resize()
                } else {
                    overlay.cursor_row_resize()
                };
                root.child(overlay)
            })
    }
}

/// Zed-style panel resize handle: a 5px transparent hit zone with a centered
/// 1px border line that brightens on hover, with double-click to reset size.
fn resize_handle(kind: ResizeKind, t: &Colors, cx: &mut Context<Workspace>) -> impl IntoElement {
    let idle = rgba(t.border_variant);
    let hot = rgba(t.icon);
    let (id, vertical) = match kind {
        ResizeKind::Sidebar => ("sidebar-resize-handle", true),
        ResizeKind::Terminal => ("terminal-resize-handle", false),
    };

    let line = if vertical {
        div().w(px(1.0)).h_full()
    } else {
        div().h(px(1.0)).w_full()
    };

    let base = div()
        .id(id)
        .occlude()
        .flex_shrink_0()
        .group("resize-handle")
        .flex()
        .items_center()
        .justify_center();

    let base = if vertical {
        base.w(px(5.0)).h_full().cursor_col_resize()
    } else {
        base.h(px(5.0)).w_full().cursor_row_resize()
    };

        base.child(line.bg(idle).group_hover("resize-handle", |s| s.bg(hot)))
        .on_mouse_down(MouseButton::Left, cx.listener(
        move |this, ev: &gpui::MouseDownEvent, _, cx| {
            if ev.click_count == 2 {
                match kind {
                    ResizeKind::Sidebar => {
                        this.show_sidebar = !this.show_sidebar;
                        if this.show_sidebar && this.sidebar_width < 170.0 {
                            this.sidebar_width = 300.0;
                        }
                    }
                    ResizeKind::Terminal => {
                        this.show_terminal = !this.show_terminal;
                        if this.show_terminal {
                            if this.terminal_maximized {
                                // Restore from full screen
                                this.terminal_maximized = false;
                                this.terminal_height = 320.0;
                            } else if this.terminal_height >= 500.0 {
                                this.terminal_maximized = false;
                                this.terminal_height = 320.0;
                            } else {
                                // Maximize to full height
                                this.terminal_maximized = true;
                            }
                        } else {
                            this.terminal_maximized = false;
                            this.terminal_height = 320.0;
                        }
                    }
                }
                this.panel_resize = None;
                cx.stop_propagation();
                cx.notify();
                return;
            }
            let (start_mouse, start_size) = match kind {
                ResizeKind::Sidebar => {
                    let current_w = if this.show_sidebar { this.sidebar_width } else { 0.0 };
                    (f32::from(ev.position.x), current_w)
                }
                ResizeKind::Terminal => {
                    let current_h = if this.show_terminal { this.terminal_height } else { 0.0 };
                    (f32::from(ev.position.y), current_h)
                }
            };
            this.panel_resize = Some(PanelResizeDrag {
                kind,
                start_mouse,
                start_size,
            });
            cx.stop_propagation();
            cx.notify();
        },
    ))
}
