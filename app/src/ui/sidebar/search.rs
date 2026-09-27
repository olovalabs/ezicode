//! VS Code-style project search sidebar: query / replace / include inputs,
//! `Aa` whole-word `.*` toggles, and grouped results with match highlight.
//!
//! The panel is a thin view over [`crate::workspace::Workspace`] search state.
//! Searching itself runs on a background thread via [`crate::search`] (parallel
//! `ignore` walk + ripgrep `grep-searcher` matching), so typing never blocks.

use std::path::Path;

use gpui::{
    div, prelude::*, px, rgba, svg, AnyElement, Context, ElementId, Entity, FontWeight,
    IntoElement, SharedString, Window,
};
use gpui_component::{
    input::{Input, InputState},
    menu::ContextMenuExt,
    scroll::ScrollableElement as _,
    tooltip::Tooltip,
    Sizable,
};

use crate::file_icons;
use crate::search::{SearchFileResult, SearchMatch};
use crate::theme::Colors;
use crate::workspace::Workspace;

pub(crate) fn render_search_panel(
    workspace: &Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let t = workspace.theme().colors;
    let t = &t;

    let has_root = workspace.root.is_some();
    let query_input = workspace.search_query_input.clone();
    let replace_input = workspace.search_replace_input.clone();
    let include_input = workspace.search_include_input.clone();
    // Read before the entities are moved into the input rows below.
    let query_text = query_input
        .as_ref()
        .map(|i| i.read(cx).value().to_string())
        .unwrap_or_default();

    let mut col = div()
        .size_full()
        .flex()
        .flex_col()
        .bg(rgba(t.panel))
        .overflow_hidden();

    col = col.child(header(t, cx));

    if !has_root {
        col = col.child(empty_state(
            "Open a folder to search across your project.",
            t,
        ));
        return col.into_any_element();
    }

    // ---- Query row ----
    col = col.child(
        div()
            .px(px(12.0))
            .pt(px(6.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(search_input_row(
                "search-query-input",
                query_input,
                "Search",
                t,
                window,
                cx,
            ))
            // Toggles + replace-mode chevron
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .child(toggle_badge(
                        "search-toggle-case",
                        "Aa",
                        "Match Case",
                        workspace.search_case_sensitive,
                        t,
                        cx,
                        |this, cx| this.toggle_search_case(cx),
                    ))
                    .child(toggle_badge(
                        "search-toggle-word",
                        "ab",
                        "Match Whole Word",
                        workspace.search_whole_word,
                        t,
                        cx,
                        |this, cx| this.toggle_search_whole_word(cx),
                    ))
                    .child(toggle_badge(
                        "search-toggle-regex",
                        ".*",
                        "Use Regular Expression",
                        workspace.search_use_regex,
                        t,
                        cx,
                        |this, cx| this.toggle_search_regex(cx),
                    ))
                    .child(div().flex_1())
                    .child(replace_toggle_btn(workspace.search_replace_open, t, cx)),
            )
            // Replace row (collapsible, like VS Code's second input line)
            .when(workspace.search_replace_open, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div().flex_1().min_w(px(0.0)).child(search_input_row(
                                "search-replace-input",
                                replace_input,
                                "Replace",
                                t,
                                window,
                                cx,
                            )),
                        )
                        .child(replace_all_btn(
                            workspace.search_total_matches,
                            t,
                            cx,
                        )),
                )
            })
            .child(search_input_row(
                "search-include-input",
                include_input,
                "files to include",
                t,
                window,
                cx,
            )),
    );

    // ---- Status line ----
    col = col.child(status_line(workspace, &query_text, t));

    // ---- Results ----
    col = col.child(
        div()
            .id("search-results-scroll")
            .flex_1()
            .w_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .overflow_y_scrollbar()
            .children(
                workspace
                    .search_results
                    .iter()
                    .map(|f| file_group(workspace, f, t, cx)),
            ),
    );

    col.into_any_element()
}

fn header(t: &Colors, cx: &mut Context<Workspace>) -> impl IntoElement {
    div()
        .h(px(36.0))
        .px(px(12.0))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgba(t.text_muted))
                .child(SharedString::from("Search")),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(2.0))
                .child(header_btn(
                    "search-refresh-btn",
                    "ui_icons/refresh_tint.svg",
                    "Refresh Search",
                    t,
                    cx,
                    |this, _, cx| this.run_search_now(cx),
                ))
                .child(header_btn(
                    "search-expand-all-btn",
                    "ui_icons/chevron-down_tint.svg",
                    "Expand All",
                    t,
                    cx,
                    |this, _, cx| this.expand_all_search(cx),
                ))
                .child(header_btn(
                    "search-collapse-all-btn",
                    "ui_icons/collapse-all_tint.svg",
                    "Collapse All",
                    t,
                    cx,
                    |this, _, cx| this.collapse_all_search(cx),
                ))
                .child(header_btn(
                    "search-clear-btn",
                    "ui_icons/discard_tint.svg",
                    "Clear Search Results",
                    t,
                    cx,
                    |this, window, cx| this.clear_search(window, cx),
                ))
                .child(
                    div()
                        .id("search-header-more")
                        .size(px(24.0))
                        .rounded(px(4.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgba(t.ghost_hover)))
                        .child(
                            svg()
                                .path("ui_icons/ellipsis_tint.svg")
                                .w(px(15.0))
                                .h(px(15.0))
                                .text_color(rgba(t.icon_muted)),
                        )
                        .context_menu(|menu, _window, _cx| {
                            menu.menu("Refresh", Box::new(crate::actions::ExplorerRefresh))
                        }),
                ),
        )
}

fn header_btn(
    id: &'static str,
    icon_path: &'static str,
    tooltip: &'static str,
    t: &Colors,
    cx: &mut Context<Workspace>,
    action: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(24.0))
        .rounded(px(4.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(rgba(t.ghost_hover)))
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .child(
            svg()
                .path(icon_path)
                .w(px(15.0))
                .h(px(15.0))
                .text_color(rgba(t.icon_muted)),
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            action(this, window, cx);
        }))
}

fn search_input_row(
    id: &'static str,
    input: Option<Entity<InputState>>,
    placeholder: &str,
    t: &Colors,
    _window: &mut Window,
    _cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let body: AnyElement = match input {
        Some(input) => div()
            .flex_1()
            .min_w(px(0.0))
            .text_size(px(13.0))
            .child(
                Input::new(&input)
                    .xsmall()
                    .text_size(px(13.0))
                    .appearance(false)
                    .bordered(false),
            )
            .into_any_element(),
        None => div()
            .text_size(px(12.5))
            .text_color(rgba(t.text_muted))
            .child(SharedString::from(placeholder.to_string()))
            .into_any_element(),
    };
    div()
        .id(id)
        .w_full()
        .min_h(px(30.0))
        .py(px(2.0))
        .px(px(8.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .bg(rgba(t.element_bg))
        .border_1()
        .border_color(rgba(t.border))
        .rounded(px(4.0))
        .child(
            svg()
                .path("ui_icons/search_tint.svg")
                .w(px(14.0))
                .h(px(14.0))
                .flex_none()
                .text_color(rgba(t.icon_muted)),
        )
        .child(body)
}

fn toggle_badge(
    id: &'static str,
    label: &'static str,
    tooltip: &'static str,
    active: bool,
    t: &Colors,
    cx: &mut Context<Workspace>,
    action: impl Fn(&mut Workspace, &mut Context<Workspace>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let (bg, fg, border) = if active {
        (t.text_accent, 0xffffffff, t.text_accent)
    } else {
        (t.element_bg, t.text_muted, t.border)
    };
    div()
        .id(id)
        .h(px(24.0))
        .min_w(px(28.0))
        .px(px(7.0))
        .rounded(px(3.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .bg(rgba(bg))
        .border_1()
        .border_color(rgba(border))
        .hover(|s| {
            if active {
                s
            } else {
                s.bg(rgba(t.element_hover))
            }
        })
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .text_size(px(12.0))
        .font_weight(FontWeight::BOLD)
        .text_color(rgba(fg))
        .child(SharedString::from(label))
        .on_click(cx.listener(move |this, _, _, cx| {
            action(this, cx);
        }))
}

fn replace_toggle_btn(
    open: bool,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> gpui::Stateful<gpui::Div> {
    let icon = if open {
        "ui_icons/chevron-up_tint.svg"
    } else {
        "ui_icons/chevron-down_tint.svg"
    };
    div()
        .id("search-replace-toggle")
        .h(px(24.0))
        .px(px(6.0))
        .rounded(px(3.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(t.ghost_hover)))
        .tooltip(|window, cx| Tooltip::new("Toggle Replace").build(window, cx))
        .child(
            svg()
                .path(icon)
                .w(px(13.0))
                .h(px(13.0))
                .text_color(rgba(t.icon_muted)),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(rgba(t.text_muted))
                .child(SharedString::from("Replace")),
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.toggle_search_replace_open(cx);
        }))
}

fn replace_all_btn(
    total: usize,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let label = if total == 0 {
        "Replace All".to_string()
    } else {
        format!("Replace All ({total})")
    };
    // No results → visibly inert instead of a live-looking blue button.
    let (bg, fg) = if total == 0 {
        (t.element_active, t.text_muted)
    } else {
        (t.text_accent, 0xffffffff)
    };
    div()
        .id("search-replace-all-btn")
        .h(px(30.0))
        .px(px(10.0))
        .flex_none()
        .rounded(px(3.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .bg(rgba(bg))
        .hover(|s| {
            if total == 0 {
                s
            } else {
                s.bg(rgba(0x0086e6ff))
            }
        })
        .tooltip(|window, cx| Tooltip::new("Replace All (all files)").build(window, cx))
        .text_size(px(12.5))
        .font_weight(FontWeight::BOLD)
        .text_color(rgba(fg))
        .child(SharedString::from(label))
        .on_click(cx.listener(|this, _, _, cx| {
            this.replace_all_in_search(cx);
        }))
}

fn status_line(workspace: &Workspace, query: &str, t: &Colors) -> impl IntoElement {
    let text: SharedString = if workspace.search_in_progress {
        "Searching…".into()
    } else if let Some(err) = &workspace.search_error {
        err.clone().into()
    } else if workspace.search_total_matches > 0 {
        let mut s = format!(
            "{} {} in {} {} ({}ms)",
            workspace.search_total_matches,
            if workspace.search_total_matches == 1 {
                "result"
            } else {
                "results"
            },
            workspace.search_results.len(),
            if workspace.search_results.len() == 1 {
                "file"
            } else {
                "files"
            },
            workspace.search_elapsed_ms,
        );
        if workspace.search_truncated {
            s.push_str(" — showing first 10,000");
        }
        s.into()
    } else if query.trim().is_empty() {
        "Type to search — results update as you type".into()
    } else {
        "No results".into()
    };
    let color = if workspace.search_error.is_some() {
        t.vc_deleted
    } else {
        t.text_muted
    };
    div()
        .px(px(12.0))
        .py(px(6.0))
        .text_size(px(12.0))
        .text_color(rgba(color))
        .child(text)
}

fn file_group(
    workspace: &Workspace,
    file: &SearchFileResult,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let collapsed = workspace.search_collapsed.contains(&file.path);
    let chevron = if collapsed {
        "ui_icons/chevron-right_tint.svg"
    } else {
        "ui_icons/chevron-down_tint.svg"
    };
    let rel_path = Path::new(&file.rel);
    let name = rel_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.rel.clone());
    let parent = rel_path.parent().and_then(|p| {
        let s = p.to_string_lossy().replace('/', "\\");
        if s.is_empty() { None } else { Some(s) }
    });
    let icon_path = file_icons::icon_for(rel_path).to_string();
    let path_toggle = file.path.clone();
    let path_replace = file.path.clone();
    let count = file.matches.len();

    let mut group = div()
        .w_full()
        .flex()
        .flex_col()
        .child(
            div()
                .id((ElementId::from("search-file"), file.rel.clone()))
                .group("search-file-row")
                .w_full()
                .h(px(26.0))
                .pl(px(6.0))
                .pr(px(8.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .cursor_pointer()
                .hover(|s| s.bg(rgba(t.ghost_hover)))
                .child(
                    div()
                        .size(px(18.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            svg()
                                .path(chevron)
                                .w(px(12.0))
                                .h(px(12.0))
                                .text_color(rgba(t.icon_muted)),
                        ),
                )
                .child(
                    div()
                        .size(px(18.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            gpui::img(SharedString::from(icon_path))
                                .w(px(16.0))
                                .h(px(16.0)),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgba(t.text))
                                .text_ellipsis()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(SharedString::from(name)),
                        )
                        .when_some(parent, |d, parent| {
                            d.child(
                                div()
                                    .flex_none()
                                    .text_size(px(11.5))
                                    .text_color(rgba(t.text_muted))
                                    .text_ellipsis()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(SharedString::from(parent)),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div()
                                .min_w(px(20.0))
                                .h(px(18.0))
                                .px(px(5.0))
                                .rounded_full()
                                .bg(rgba(t.element_active))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgba(t.text))
                                .child(SharedString::from(count.to_string())),
                        )
                        .child(
                            div()
                                .id((ElementId::from("search-replace-file"), file.rel.clone()))
                                .size(px(22.0))
                                .rounded(px(3.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .invisible()
                                .group_hover("search-file-row", |s| s.visible())
                                .hover(|s| s.bg(rgba(t.element_hover)))
                                .tooltip(|window, cx| {
                                    Tooltip::new("Replace All in this file").build(window, cx)
                                })
                                .child(
                                    svg()
                                        .path("ui_icons/check_tint.svg")
                                        .w(px(13.0))
                                        .h(px(13.0))
                                        .text_color(rgba(t.icon_muted)),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.replace_in_search_file(&path_replace, cx);
                                    cx.stop_propagation();
                                })),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_search_file_collapsed(&path_toggle, cx);
                })),
        );

    if !collapsed {
        group = group.child(
            div()
                .flex()
                .flex_col()
                .children(file.matches.iter().map(|m| hit_row(file, m, t, cx))),
        );
    }
    group
}

fn hit_row(
    file: &SearchFileResult,
    m: &SearchMatch,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let path = file.path.clone();
    let line = m.line_number;
    let col = m.col_start;
    let preview = highlight_line(&m.line_text, m.col_start, m.col_end);

    div()
        .id(SharedString::from(format!(
            "search-hit-{}-{}-{}",
            file.rel,
            m.line_number,
            m.col_start
        )))
        .w_full()
        .min_h(px(22.0))
        .py(px(1.0))
        .pl(px(28.0))
        .pr(px(8.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(t.ghost_hover)))
        .child(
            div()
                .w(px(34.0))
                .flex_none()
                .flex()
                .justify_end()
                .text_size(px(11.5))
                .font_family(crate::assets::MONO_FONT)
                .text_color(rgba(t.text_muted))
                .child(SharedString::from(line.to_string())),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_row()
                .items_center()
                .overflow_hidden()
                .whitespace_nowrap()
                .font_family(crate::assets::MONO_FONT)
                .text_size(px(12.5))
                .child(
                    div()
                        .flex_none()
                        .text_color(rgba(t.text_muted))
                        .child(SharedString::from(preview.0)),
                )
                .child(
                    div()
                        .flex_none()
                        .bg(rgba(t.text_accent))
                        .rounded(px(2.0))
                        .px(px(1.0))
                        .text_color(rgba(0xffffffff))
                        .child(SharedString::from(preview.1)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_color(rgba(t.text))
                        .child(SharedString::from(preview.2)),
                ),
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            this.open_search_result(path.clone(), line, col, window, cx);
        }))
}

/// Split a matched line into (before, hit, after), clamped to the preview.
fn highlight_line(line: &str, start: usize, end: usize) -> (String, String, String) {
    let len = line.len();
    let mut s = start.min(len);
    let mut e = end.min(len).max(s);
    while s < len && !line.is_char_boundary(s) {
        s += 1;
    }
    while e < len && !line.is_char_boundary(e) {
        e += 1;
    }
    if s >= e {
        return (line.to_string(), String::new(), String::new());
    }
    // Keep the row readable: trim long context on both sides like VS Code.
    const KEEP_BEFORE: usize = 80;
    const KEEP_AFTER: usize = 160;
    let line_start = if s > KEEP_BEFORE {
        let mut cut = s - KEEP_BEFORE;
        while cut < s && !line.is_char_boundary(cut) {
            cut += 1;
        }
        cut
    } else {
        0
    };
    let line_end = if len - e > KEEP_AFTER {
        let mut cut = e + KEEP_AFTER;
        while cut > e && !line.is_char_boundary(cut) {
            cut -= 1;
        }
        cut.min(len)
    } else {
        len
    };
    let prefix = if line_start > 0 { "… " } else { "" };
    let suffix = if line_end < len { " …" } else { "" };
    (
        format!("{prefix}{}", &line[line_start..s]),
        line[s..e].to_string(),
        format!("{}{suffix}", &line[e..line_end]),
    )
}

fn empty_state(message: &str, t: &Colors) -> impl IntoElement {
    div()
        .flex_1()
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .px(px(20.0))
        .child(
            div()
                .text_size(px(13.0))
                .text_color(rgba(t.text_muted))
                .child(SharedString::from(message.to_string())),
        )
}
