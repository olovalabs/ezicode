use std::path::Path;

use gpui::{
    div, prelude::*, px, rgba, AnyElement, Context, FontWeight, IntoElement, SharedString,
};
use gpui_component::scroll::ScrollableElement as _;

use crate::actions::{GitDiscardFile, GitStageFile, GitUnstageFile};
use crate::assets;
use crate::file_icons;
use crate::theme::Colors;
use crate::ui::common::icon_img;
use crate::workspace::{DiffTab, Workspace};

const MAX_RENDERED_LINES: usize = 12_000;

fn with_alpha(color: u32, alpha: u8) -> u32 {
    (color & 0xFFFF_FF00) | u32::from(alpha)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffCellKind {
    Context,
    Add,
    Remove,
    Empty,
}

#[derive(Clone, Debug)]
pub struct DiffCell {
    pub line_no: Option<u32>,
    pub text: String,
    pub kind: DiffCellKind,
    pub diff_spans: Vec<std::ops::Range<usize>>,
}

#[derive(Clone, Debug)]
pub enum SideBySideRow {
    Hunk {
        old_no: u32,
        new_no: u32,
        header: String,
    },
    Line {
        left: DiffCell,
        right: DiffCell,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct ParsedDiff {
    pub rows: Vec<SideBySideRow>,
    pub added: usize,
    pub removed: usize,
}

pub(crate) fn parse_diff(raw: &str) -> ParsedDiff {
    let (rows, added, removed) = parse_side_by_side_diff(raw);
    ParsedDiff { rows, added, removed }
}

pub(crate) fn render_diff_view(
    diff: &DiffTab,
    font_size: f32,
    split_diff: bool,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let rel_path = Path::new(&diff.rel);
    let file_name = diff
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| diff.rel.clone());

    let parent_dir = rel_path.parent().and_then(|p| {
        let s = p.to_string_lossy();
        if s.is_empty() {
            None
        } else {
            let mut normalized = s.replace('/', "\\");
            if !normalized.ends_with('\\') {
                normalized.push('\\');
            }
            Some(normalized)
        }
    });

    let path = diff.path.clone();
    let file_icon_path = file_icons::icon_for(rel_path);

    let (rows, total_added, total_removed) = match &diff.parsed {
        Some(parsed) => (parsed.rows.clone(), parsed.added, parsed.removed),
        None => (Vec::new(), 0, 0),
    };

    let is_staged = diff.staged;

    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(rgba(t.editor_bg))
        .child(

            div()
                .h(px(36.0))
                .w_full()
                .px(px(14.0))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .bg(rgba(t.panel))
                .border_b_1()
                .border_color(rgba(t.border_variant))
                .child(

                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(icon_img(file_icon_path, 16.0))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgba(t.text))
                                .child(SharedString::from(file_name)),
                        )
                        .when_some(parent_dir, |d, parent| {
                            d.child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgba(t.text_muted))
                                    .child(SharedString::from(parent)),
                            )
                        })
                        .child(

                            div()
                                .px(px(6.0))
                                .py(px(1.5))
                                .rounded(px(4.0))
                                .bg(rgba(t.element_bg))
                                .border_1()
                                .border_color(rgba(t.border))
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(4.0))
                                .text_size(px(10.5))
                                .font_weight(FontWeight::BOLD)
                                .child(
                                    div()
                                        .text_color(rgba(t.vc_added))
                                        .child(SharedString::from(format!("+{total_added}"))),
                                )
                                .child(
                                    div()
                                        .text_color(rgba(t.vc_deleted))
                                        .child(SharedString::from(format!("-{total_removed}"))),
                                ),
                        )
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(1.5))
                                .rounded_full()
                                .bg(rgba(if is_staged { t.vc_added } else { t.vc_modified }))
                                .text_size(px(9.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgba(t.background))
                                .child(SharedString::from(if is_staged {
                                    "STAGED"
                                } else {
                                    "WORKING TREE"
                                })),
                        ),
                )
                .child(

                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(tool_button(
                            if split_diff { "Split [||]" } else { "Inline [=]" },
                            t,
                            cx,
                            |this, _window, cx| {
                                this.toggle_split_diff(cx);
                            },
                        ))
                        .when(!is_staged, |d| {
                            let path_stage = path.clone();
                            d.child(tool_button("Stage", t, cx, move |_this, window, cx| {
                                window.dispatch_action(
                                    Box::new(GitStageFile {
                                        path: path_stage.clone(),
                                    }),
                                    cx,
                                );
                            }))
                        })
                        .when(is_staged, |d| {
                            let path_unstage = path.clone();
                            d.child(tool_button("Unstage", t, cx, move |_this, window, cx| {
                                window.dispatch_action(
                                    Box::new(GitUnstageFile {
                                        path: path_unstage.clone(),
                                    }),
                                    cx,
                                );
                            }))
                        })
                        .when(!is_staged, |d| {
                            let path_discard = path.clone();
                            d.child(tool_button("Restore", t, cx, move |_this, window, cx| {
                                window.dispatch_action(
                                    Box::new(GitDiscardFile {
                                        path: path_discard.clone(),
                                    }),
                                    cx,
                                );
                            }))
                        })
                        .child(tool_button("Open File", t, cx, {
                            let path_open = path.clone();
                            move |this, window, cx| {
                                this.open_file(path_open.clone(), window, cx);
                            }
                        })),
                ),
        )
        .child(diff_body(diff, rows, font_size, split_diff, t))
        .into_any_element()
}

fn tool_button(
    label: &'static str,
    t: &Colors,
    cx: &mut Context<Workspace>,
    on_click: impl Fn(&mut Workspace, &mut gpui::Window, &mut Context<Workspace>) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("diff-tool-{label}")))
        .h(px(24.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(t.element_bg))
        .border_1()
        .border_color(rgba(t.border))
        .rounded(px(3.0))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(t.element_hover)))
        .text_size(px(11.5))
        .font_weight(FontWeight::BOLD)
        .text_color(rgba(t.text))
        .child(SharedString::from(label))
        .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
}

fn diff_body(
    diff: &DiffTab,
    rows: Vec<SideBySideRow>,
    font_size: f32,
    split_diff: bool,
    t: &Colors,
) -> impl IntoElement {
    let line_height = font_size * 1.45;

    let Some(_text) = diff.text.as_ref() else {
        return div()
            .flex_1()
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(13.0))
            .text_color(rgba(t.text_muted))
            .child(SharedString::from("Loading diff…"))
            .into_any_element();
    };

    if let Some(error) = &diff.error {
        return div()
            .flex_1()
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(13.0))
            .text_color(rgba(t.vc_deleted))
            .child(SharedString::from(error.clone()))
            .into_any_element();
    }

    if rows.is_empty() {
        return div()
            .flex_1()
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(13.0))
            .text_color(rgba(t.text_muted))
            .child(SharedString::from("No changes in this file"))
            .into_any_element();
    }

    let truncated = rows.len() > MAX_RENDERED_LINES;
    let iter = rows.into_iter().take(MAX_RENDERED_LINES);

    let mut list = div()
        .id("diff-scroll")
        .flex_1()
        .w_full()
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .font_family(assets::MONO_FONT)
        .text_size(px(font_size))
        .overflow_y_scrollbar();

    for row in iter {
        match row {
            SideBySideRow::Hunk {
                old_no,
                new_no,
                header,
            } => {
                list = list.child(hunk_header_row(
                    old_no,
                    new_no,
                    &header,
                    line_height,
                    font_size,
                    split_diff,
                    t,
                ));
            }
            SideBySideRow::Line { left, right } => {
                if split_diff {
                    list = list.child(split_line_row(&left, &right, line_height, font_size, t));
                } else {
                    list = list.child(inline_line_row(&left, &right, line_height, font_size, t));
                }
            }
        }
    }

    if truncated {
        list = list.child(
            div()
                .w_full()
                .h(px(line_height))
                .px(px(12.0))
                .flex()
                .items_center()
                .text_size(px(12.0))
                .text_color(rgba(t.text_muted))
                .child(SharedString::from(format!(
                    "… diff truncated at {MAX_RENDERED_LINES} lines"
                ))),
        );
    }

    list.into_any_element()
}

fn hunk_header_row(
    old_no: u32,
    new_no: u32,
    header: &str,
    line_height: f32,
    font_size: f32,
    split_diff: bool,
    t: &Colors,
) -> impl IntoElement {
    if split_diff {
        div()
            .w_full()
            .h(px(line_height))
            .flex()
            .flex_row()
            .bg(rgba(with_alpha(t.icon_accent, 0x15)))
            .border_t_1()
            .border_b_1()
            .border_color(rgba(t.border_variant))
            .child(

                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .border_r_1()
                    .border_color(rgba(t.border_variant))
                    .child(
                        div()
                            .w(px(font_size * 3.4))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_end()
                            .pr(px(8.0))
                            .text_size(px(font_size * 0.82))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgba(t.text_accent))
                            .child(SharedString::from(format!("↑ {old_no}"))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .px(px(8.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(font_size * 0.88))
                            .text_color(rgba(t.text_muted))
                            .child(SharedString::from(header.to_string())),
                    ),
            )
            .child(

                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(
                        div()
                            .w(px(font_size * 3.4))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_end()
                            .pr(px(8.0))
                            .text_size(px(font_size * 0.82))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgba(t.text_accent))
                            .child(SharedString::from(format!("↑ {new_no}"))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .px(px(8.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(font_size * 0.88))
                            .text_color(rgba(t.text_muted))
                            .child(SharedString::from(header.to_string())),
                    ),
            )
    } else {
        div()
            .w_full()
            .h(px(line_height))
            .px(px(14.0))
            .flex()
            .flex_row()
            .items_center()
            .bg(rgba(with_alpha(t.icon_accent, 0x15)))
            .border_t_1()
            .border_b_1()
            .border_color(rgba(t.border_variant))
            .text_size(px(font_size * 0.88))
            .text_color(rgba(t.text_accent))
            .child(SharedString::from(header.to_string()))
    }
}

fn split_line_row(
    left: &DiffCell,
    right: &DiffCell,
    line_height: f32,
    font_size: f32,
    t: &Colors,
) -> impl IntoElement {
    div()
        .w_full()
        .h(px(line_height))
        .flex()
        .flex_row()
        .child(

            div()
                .flex_1()
                .min_w(px(0.0))
                .h_full()
                .flex()
                .flex_row()
                .border_r_1()
                .border_color(rgba(t.border_variant))
                .child(render_pane_cell(left, font_size, t)),
        )
        .child(

            div()
                .flex_1()
                .min_w(px(0.0))
                .h_full()
                .flex()
                .flex_row()
                .child(render_pane_cell(right, font_size, t)),
        )
}

fn render_pane_cell(
    cell: &DiffCell,
    font_size: f32,
    t: &Colors,
) -> impl IntoElement {
    let (bg, gutter_bg, bar_color) = match cell.kind {
        DiffCellKind::Remove => (
            Some(with_alpha(t.vc_deleted, 0x22)),
            Some(with_alpha(t.vc_deleted, 0x33)),
            Some(t.vc_deleted),
        ),
        DiffCellKind::Add => (
            Some(with_alpha(t.vc_added, 0x22)),
            Some(with_alpha(t.vc_added, 0x33)),
            Some(t.vc_added),
        ),
        DiffCellKind::Empty => (Some(0x00000035), Some(0x00000045), None),
        DiffCellKind::Context => (None, None, None),
    };

    let gutter_no = cell
        .line_no
        .map(|n| n.to_string())
        .unwrap_or_default();

    div()
        .size_full()
        .flex()
        .flex_row()
        .when_some(bg, |d, bg| d.bg(rgba(bg)))

        .child(
            div()
                .w(px(3.0))
                .h_full()
                .flex_none()
                .when_some(bar_color, |d, color| d.bg(rgba(color))),
        )

        .child(
            div()
                .w(px(font_size * 3.4))
                .flex_none()
                .h_full()
                .flex()
                .items_center()
                .justify_end()
                .pr(px(8.0))
                .when_some(gutter_bg, |d, gbg| d.bg(rgba(gbg)))
                .text_size(px(font_size * 0.82))
                .text_color(rgba(t.text_muted))
                .child(SharedString::from(gutter_no)),
        )

        .child(render_cell_content(cell, font_size, t))
}

fn render_cell_content(
    cell: &DiffCell,
    font_size: f32,
    t: &Colors,
) -> impl IntoElement {
    if cell.kind == DiffCellKind::Empty {
        return div()
            .flex_1()
            .h_full()
            .bg(rgba(0x00000028))
            .into_any_element();
    }

    if cell.diff_spans.is_empty() {
        let fg = match cell.kind {
            DiffCellKind::Add => t.vc_added,
            DiffCellKind::Remove => t.vc_deleted,
            DiffCellKind::Context | DiffCellKind::Empty => t.editor_fg,
        };
        return div()
            .flex_1()
            .min_w(px(0.0))
            .px(px(6.0))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_size(px(font_size))
            .text_color(rgba(fg))
            .child(SharedString::from(cell.text.clone()))
            .into_any_element();
    }

    let text = &cell.text;
    let mut fragments = Vec::new();
    let mut last_idx = 0;

    let span_bg = match cell.kind {
        DiffCellKind::Add => with_alpha(t.vc_added, 0x58),
        DiffCellKind::Remove => with_alpha(t.vc_deleted, 0x58),
        _ => with_alpha(t.icon_accent, 0x33),
    };
    let fg = match cell.kind {
        DiffCellKind::Add => t.vc_added,
        DiffCellKind::Remove => t.vc_deleted,
        _ => t.editor_fg,
    };

    for span in &cell.diff_spans {
        let start = span.start.min(text.len());
        let end = span.end.min(text.len());
        if start > last_idx {
            fragments.push(
                div()
                    .text_color(rgba(fg))
                    .child(SharedString::from(text[last_idx..start].to_string())),
            );
        }
        if start < end {
            fragments.push(
                div()
                    .bg(rgba(span_bg))
                    .rounded(px(2.0))
                    .px(px(1.5))
                    .text_color(rgba(fg))
                    .font_weight(FontWeight::BOLD)
                    .child(SharedString::from(text[start..end].to_string())),
            );
        }
        last_idx = end;
    }
    if last_idx < text.len() {
        fragments.push(
            div()
                .text_color(rgba(fg))
                .child(SharedString::from(text[last_idx..].to_string())),
        );
    }

    div()
        .flex_1()
        .min_w(px(0.0))
        .px(px(6.0))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_size(px(font_size))
        .flex()
        .flex_row()
        .items_center()
        .children(fragments)
        .into_any_element()
}

fn inline_line_row(
    left: &DiffCell,
    right: &DiffCell,
    line_height: f32,
    font_size: f32,
    t: &Colors,
) -> impl IntoElement {
    if left.kind == DiffCellKind::Remove {
        return div()
            .w_full()
            .h(px(line_height))
            .flex()
            .flex_row()
            .child(render_pane_cell(left, font_size, t))
            .into_any_element();
    }
    if right.kind == DiffCellKind::Add {
        return div()
            .w_full()
            .h(px(line_height))
            .flex()
            .flex_row()
            .child(render_pane_cell(right, font_size, t))
            .into_any_element();
    }
    div()
        .w_full()
        .h(px(line_height))
        .flex()
        .flex_row()
        .child(render_pane_cell(left, font_size, t))
        .into_any_element()
}

fn compute_word_diff(
    old: &str,
    new: &str,
) -> (Vec<std::ops::Range<usize>>, Vec<std::ops::Range<usize>>) {
    if old == new || old.is_empty() || new.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let old_chars: Vec<(usize, char)> = old.char_indices().collect();
    let new_chars: Vec<(usize, char)> = new.char_indices().collect();

    let mut prefix_chars = 0;
    while prefix_chars < old_chars.len()
        && prefix_chars < new_chars.len()
        && old_chars[prefix_chars].1 == new_chars[prefix_chars].1
    {
        prefix_chars += 1;
    }

    let old_rem = old_chars.len() - prefix_chars;
    let new_rem = new_chars.len() - prefix_chars;

    let mut suffix_chars = 0;
    while suffix_chars < old_rem
        && suffix_chars < new_rem
        && old_chars[old_chars.len() - 1 - suffix_chars].1
            == new_chars[new_chars.len() - 1 - suffix_chars].1
    {
        suffix_chars += 1;
    }

    let old_start_idx = if prefix_chars < old_chars.len() {
        old_chars[prefix_chars].0
    } else {
        old.len()
    };
    let old_end_idx = if suffix_chars > 0 {
        old_chars[old_chars.len() - suffix_chars].0
    } else {
        old.len()
    };

    let new_start_idx = if prefix_chars < new_chars.len() {
        new_chars[prefix_chars].0
    } else {
        new.len()
    };
    let new_end_idx = if suffix_chars > 0 {
        new_chars[new_chars.len() - suffix_chars].0
    } else {
        new.len()
    };

    let mut old_spans = Vec::new();
    if old_start_idx < old_end_idx {
        old_spans.push(old_start_idx..old_end_idx);
    }

    let mut new_spans = Vec::new();
    if new_start_idx < new_end_idx {
        new_spans.push(new_start_idx..new_end_idx);
    }

    (old_spans, new_spans)
}

pub fn parse_side_by_side_diff(raw: &str) -> (Vec<SideBySideRow>, usize, usize) {
    let mut rows = Vec::new();
    let mut old_no: Option<u32> = None;
    let mut new_no: Option<u32> = None;
    let mut total_added = 0;
    let mut total_removed = 0;

    let mut pending_removes: Vec<(u32, String)> = Vec::new();
    let mut pending_adds: Vec<(u32, String)> = Vec::new();

    let flush = |removes: &mut Vec<(u32, String)>,
                 adds: &mut Vec<(u32, String)>,
                 rows: &mut Vec<SideBySideRow>| {
        let n = removes.len();
        let m = adds.len();
        let count = n.max(m);
        for i in 0..count {
            let left = if i < n {
                let (lno, text) = &removes[i];
                let diff_spans = if i < m {
                    compute_word_diff(text, &adds[i].1).0
                } else {
                    Vec::new()
                };
                DiffCell {
                    line_no: Some(*lno),
                    text: text.clone(),
                    kind: DiffCellKind::Remove,
                    diff_spans,
                }
            } else {
                DiffCell {
                    line_no: None,
                    text: String::new(),
                    kind: DiffCellKind::Empty,
                    diff_spans: Vec::new(),
                }
            };

            let right = if i < m {
                let (rno, text) = &adds[i];
                let diff_spans = if i < n {
                    compute_word_diff(&removes[i].1, text).1
                } else {
                    Vec::new()
                };
                DiffCell {
                    line_no: Some(*rno),
                    text: text.clone(),
                    kind: DiffCellKind::Add,
                    diff_spans,
                }
            } else {
                DiffCell {
                    line_no: None,
                    text: String::new(),
                    kind: DiffCellKind::Empty,
                    diff_spans: Vec::new(),
                }
            };

            rows.push(SideBySideRow::Line { left, right });
        }
        removes.clear();
        adds.clear();
    };

    for line in raw.lines() {
        if line.starts_with("--- ")
            || line.starts_with("+++ ")
            || line.starts_with("--- /dev/null")
            || line.starts_with("+++ /dev/null")
            || line.starts_with("--- a/")
            || line.starts_with("+++ b/")
            || line.starts_with("diff --git")
            || line.starts_with("index ")
            || line.starts_with("new file mode")
            || line.starts_with("deleted file mode")
        {
            continue;
        }

        if let Some(rest) = line.strip_prefix("@@") {
            flush(&mut pending_removes, &mut pending_adds, &mut rows);
            let (old, new) = hunk_numbers(rest);
            old_no = old;
            new_no = new;
            rows.push(SideBySideRow::Hunk {
                old_no: old.unwrap_or(1),
                new_no: new.unwrap_or(1),
                header: line.to_string(),
            });
            continue;
        }

        let Some(first) = line.chars().next() else {
            flush(&mut pending_removes, &mut pending_adds, &mut rows);
            let cur_old = old_no;
            let cur_new = new_no;
            bump(&mut old_no, &mut new_no);
            rows.push(SideBySideRow::Line {
                left: DiffCell {
                    line_no: cur_old,
                    text: String::new(),
                    kind: DiffCellKind::Context,
                    diff_spans: Vec::new(),
                },
                right: DiffCell {
                    line_no: cur_new,
                    text: String::new(),
                    kind: DiffCellKind::Context,
                    diff_spans: Vec::new(),
                },
            });
            continue;
        };

        match first {
            ' ' => {
                flush(&mut pending_removes, &mut pending_adds, &mut rows);
                let cur_old = old_no;
                let cur_new = new_no;
                bump(&mut old_no, &mut new_no);
                rows.push(SideBySideRow::Line {
                    left: DiffCell {
                        line_no: cur_old,
                        text: line[1..].to_string(),
                        kind: DiffCellKind::Context,
                        diff_spans: Vec::new(),
                    },
                    right: DiffCell {
                        line_no: cur_new,
                        text: line[1..].to_string(),
                        kind: DiffCellKind::Context,
                        diff_spans: Vec::new(),
                    },
                });
            }
            '-' => {
                total_removed += 1;
                let cur_old = old_no.unwrap_or(1);
                pending_removes.push((cur_old, line[1..].to_string()));
                if let Some(n) = old_no.as_mut() {
                    *n += 1;
                }
            }
            '+' => {
                total_added += 1;
                let cur_new = new_no.unwrap_or(1);
                pending_adds.push((cur_new, line[1..].to_string()));
                if let Some(n) = new_no.as_mut() {
                    *n += 1;
                }
            }
            '\\' => {

            }
            _ => {}
        }
    }

    flush(&mut pending_removes, &mut pending_adds, &mut rows);
    (rows, total_added, total_removed)
}

fn bump(old: &mut Option<u32>, new: &mut Option<u32>) {
    if let Some(n) = old.as_mut() {
        *n += 1;
    }
    if let Some(n) = new.as_mut() {
        *n += 1;
    }
}

fn hunk_numbers(header: &str) -> (Option<u32>, Option<u32>) {
    let mut old = None;
    let mut new = None;
    for part in header.split_whitespace() {
        if let Some(rest) = part.strip_prefix('-') {
            old = rest.split(',').next().and_then(|s| s.parse().ok());
        } else if let Some(rest) = part.strip_prefix('+') {
            new = rest.split(',').next().and_then(|s| s.parse().ok());
        }
    }
    (old, new)
}
