use gpui::prelude::*;
use gpui::{div, px, rgba, Context, FontWeight, IntoElement, MouseButton, SharedString};

use crate::file_icons;
use crate::git::{ChangeKind, RepoStatus};
use crate::theme::Colors;
use crate::ui::common::icon_img;
use crate::ui::scale::rem;
use crate::workspace::{OpenTab, Workspace};

const TAB_HEIGHT: f32 = 35.0;

fn kind_color(kind: ChangeKind, t: &Colors) -> u32 {
    match kind {
        ChangeKind::Modified => t.vc_modified,
        ChangeKind::Added | ChangeKind::Renamed | ChangeKind::Copied | ChangeKind::Untracked => {
            t.vc_added
        }
        ChangeKind::Deleted | ChangeKind::Conflicted => t.vc_deleted,
        ChangeKind::TypeChanged => t.icon_accent,
    }
}

fn render_tab_content(
    tab: &OpenTab,
    index: usize,
    is_active: bool,
    git_repo: Option<&RepoStatus>,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let label = if tab.is_settings {
        "settings.json".to_string()
    } else if let Some(diff) = &tab.diff {
        let name = diff
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| diff.rel.clone());
        format!("{name} — diff")
    } else {
        tab.path
            .as_ref()
            .map(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Untitled".to_string())
            })
            .unwrap_or_else(|| "Untitled".to_string())
    };

    let icon_path = if tab.is_settings {
        "file_icons/file_type_json.svg"
    } else if tab.diff.is_some() {
        "file_icons/file_type_git.svg"
    } else if let Some(override_lang) = &tab.language_override {
        crate::lang::language_icon(override_lang)
    } else if let Some(p) = &tab.path {
        file_icons::icon_for(p)
    } else if let Some(lang) = tab.language() {
        crate::lang::language_icon(lang)
    } else {
        "file_icons/default_file.svg"
    };

    let git_change = tab
        .path
        .as_ref()
        .and_then(|p| git_repo.and_then(|r| r.changes.iter().find(|c| &c.path == p)));

    let (git_letter, git_color) = if let Some(change) = git_change {
        if change.conflicted {
            (Some(ChangeKind::Conflicted.letter()), Some(t.vc_deleted))
        } else if change.untracked {
            (Some("U"), Some(t.vc_added))
        } else if let Some(worktree) = change.worktree {
            (Some(worktree.letter()), Some(kind_color(worktree, t)))
        } else if let Some(index) = change.index {
            (Some(index.letter()), Some(kind_color(index, t)))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let text_color = if is_active {
        t.tab_active_fg
    } else if let Some(color) = git_color {
        color
    } else {
        t.tab_inactive_fg
    };

    let group_name = SharedString::from(format!("tab-item-{index}"));

    let mut tab_div = div()
        .id(("tab", index))
        .group(group_name.clone())
        .relative()
        .h(rem(TAB_HEIGHT))
        .flex()
        .flex_row()
        .items_center()
        .pl(rem(10.0))
        .pr(rem(8.0))
        .gap(rem(6.0))
        .cursor_pointer()
        .text_size(rem(14.0))
        .text_color(rgba(text_color))
        .max_w(rem(220.0))
        .flex_shrink()
        .overflow_hidden()
        .border_r_1()
        .border_color(rgba(t.border_variant))
        .on_click(cx.listener(move |this, _, _window, cx| {
            this.switch_tab_to(index, cx);
        }))
        .on_mouse_down(
            MouseButton::Middle,
            cx.listener(move |this, _, _window, cx| {
                this.close_tab_at_index(index, cx);
            }),
        );

    if is_active {
        tab_div = tab_div.bg(rgba(t.tab_active_bg)).child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(2.0))
                .bg(rgba(t.text_accent)),
        );
    } else {
        tab_div = tab_div
            .bg(rgba(t.tab_inactive_bg))
            .border_b_1()
            .border_color(rgba(t.border_variant))
            .hover(|h| h.bg(rgba(t.element_hover)));
    }

    let content = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(rem(6.0))
        .flex_1()
        .min_w(px(0.0))
        .overflow_hidden()
        .child(icon_img(icon_path, rem(16.0)))
        .child(
            div()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .when(tab.preview, |d| d.italic())
                .child(SharedString::from(label)),
        );

    tab_div = tab_div.child(content);

    if let Some((letter, color)) = git_letter.zip(git_color) {
        tab_div = tab_div.child(
            div()
                .text_size(rem(11.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgba(color))
                .child(letter),
        );
    }

    let mut close_btn = div()
        .id(("close-tab", index))
        .relative()
        .size(rem(18.0))
        .rounded(px(3.0))
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .cursor_pointer()
        .hover(|h| h.bg(rgba(t.element_hover)))
        .on_click(cx.listener(move |this, _, _window, cx| {
            cx.stop_propagation();
            this.close_tab_at_index(index, cx);
        }));

    let icon_fg = if is_active {
        t.tab_active_fg
    } else {
        t.tab_inactive_fg
    };

    if tab.dirty {
        close_btn = close_btn
            .child(
                div()
                    .size(rem(8.0))
                    .rounded_full()
                    .bg(rgba(icon_fg))
                    .group_hover(group_name.clone(), |s| s.invisible()),
            )
            .child(
                div()
                    .absolute()
                    .text_size(rem(10.5))
                    .text_color(rgba(icon_fg))
                    .invisible()
                    .group_hover(group_name.clone(), |s| s.visible())
                    .child("✕"),
            );
    } else {
        if !is_active {
            close_btn = close_btn
                .invisible()
                .group_hover(group_name.clone(), |s| s.visible());
        }
        close_btn = close_btn.child(
            div()
                .text_size(rem(10.5))
                .text_color(rgba(icon_fg))
                .child("✕"),
        );
    }

    tab_div.child(close_btn)
}

pub fn render_tab_bar(
    tabs: &[OpenTab],
    active_tab: usize,
    git_repo: Option<&RepoStatus>,
    show_markdown_preview_button: bool,
    markdown_preview_open: bool,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    div()
        .id("tab-bar")
        .flex()
        .flex_row()
        .items_center()
        .h(rem(TAB_HEIGHT))
        .bg(rgba(t.tab_bar))
        .w_full()
        .overflow_hidden()
        .children(tabs.iter().enumerate().map(|(idx, tab)| {
            let is_active = idx == active_tab;
            render_tab_content(tab, idx, is_active, git_repo, t, cx)
        }))
        .child(
            div()
                .flex_1()
                .h_full()
                .border_b_1()
                .border_color(rgba(t.border_variant)),
        )
        .when(show_markdown_preview_button, |bar| {
            bar.child(
                div()
                    .id("toggle-markdown-preview")
                    .h_full()
                    .w(px(38.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .border_b_1()
                    .border_color(rgba(t.border_variant))
                    .when(markdown_preview_open, |button| {
                        button.bg(rgba(t.element_active))
                    })
                    .hover(|button| button.bg(rgba(t.element_hover)))
                    .on_click(cx.listener(|workspace, _, _, cx| {
                        workspace.toggle_markdown_preview(cx);
                    }))
                    .child(crate::markdown_preview::toolbar_icon(t)),
            )
        })
}
