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

use crate::actions::{
    ExplorerCopyPath, ExplorerRevealInFinder, GitDiscardAll, GitDiscardFile, GitOpenDiff,
    GitOpenFile, GitRefresh, GitStageAll, GitStageFile, GitUnstageAll, GitUnstageFile,
};
use crate::file_icons;
use crate::git::{ChangeKind, GitChange, RepoStatus};
use crate::theme::Colors;
use crate::ui::common::icon_img;
use crate::workspace::Workspace;

const ROW_HEIGHT: f32 = 24.0;

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

pub(crate) fn render_git_panel(
    commit_input: Option<&Entity<InputState>>,
    repo: Option<&RepoStatus>,
    repo_section_expanded: bool,
    staged_expanded: bool,
    changes_expanded: bool,
    t: &Colors,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let branch = repo
        .and_then(|r| r.branch.clone())
        .unwrap_or_else(|| "main".to_string());

    let mut col = div()
        .size_full()
        .flex()
        .flex_col()
        .bg(rgba(t.panel))
        .overflow_hidden();

    col = col.child(header(t, window, cx));

    match repo {
        None => {
            col = col.child(empty_state(
                "Open a folder inside a Git repository\nto see your changes here.",
                t,
            ));
        }
        Some(repo) => {
            let mut body = div()
                .id("git-sidebar-scroll")
                .flex_1()
                .w_full()
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .overflow_y_scrollbar();

            let staged: Vec<GitChange> =
                repo.changes.iter().filter(|c| c.is_staged()).cloned().collect();
            let unstaged: Vec<GitChange> = repo
                .changes
                .iter()
                .filter(|c| !c.is_staged() || c.worktree.is_some() || c.is_untracked())
                .cloned()
                .collect();

            let repo_chevron = if repo_section_expanded {
                "ui_icons/chevron-down_tint.svg"
            } else {
                "ui_icons/chevron-right_tint.svg"
            };

            let repo_section_header = div()
                .id("git-repo-header")
                .h(px(24.0))
                .px(px(8.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .cursor_pointer()
                .hover(|s| s.bg(rgba(t.ghost_hover)))
                .child(
                    svg()
                        .path(repo_chevron)
                        .w(px(14.0))
                        .h(px(14.0))
                        .text_color(rgba(t.icon_muted)),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgba(t.text))
                        .child(SharedString::from("Changes")),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle_git_repo_section(cx);
                }));

            body = body.child(repo_section_header);

            if repo_section_expanded {
                body = body.child(commit_box(commit_input, &branch, t, window, cx));
            }

            if !staged.is_empty() {
                let staged_chevron = if staged_expanded {
                    "ui_icons/chevron-down_tint.svg"
                } else {
                    "ui_icons/chevron-right_tint.svg"
                };

                let staged_header = div()
                    .id("git-staged-header")
                    .group("git-staged-header")
                    .h(px(22.0))
                    .px(px(8.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgba(t.ghost_hover)))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                svg()
                                    .path(staged_chevron)
                                    .w(px(14.0))
                                    .h(px(14.0))
                                    .text_color(rgba(t.icon_muted)),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgba(t.text))
                                    .child(SharedString::from("Staged Changes")),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.0))
                            .child(
                                section_action(
                                    "git-unstage-all-btn",
                                    "ui_icons/minus_tint.svg",
                                    "Unstage All Changes",
                                    GitUnstageAll,
                                    t,
                                    cx,
                                )
                                .invisible()
                                .group_hover("git-staged-header", |s| s.visible()),
                            )
                            .child(badge(staged.len())),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_git_staged_section(cx);
                    }));

                body = body.child(staged_header);

                if staged_expanded {
                    body = body.child(
                        div()
                            .flex()
                            .flex_col()
                            .children(staged.iter().map(|c| {
                                change_row(c, true, "git-staged-row", t, cx)
                            })),
                    );
                }
            }

            let changes_chevron = if changes_expanded {
                "ui_icons/chevron-down_tint.svg"
            } else {
                "ui_icons/chevron-right_tint.svg"
            };

            let changes_header = div()
                .id("git-changes-header")
                .group("git-changes-header")
                .h(px(22.0))
                .px(px(8.0))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .cursor_pointer()
                .hover(|s| s.bg(rgba(t.ghost_hover)))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            svg()
                                .path(changes_chevron)
                                .w(px(14.0))
                                .h(px(14.0))
                                .text_color(rgba(t.icon_muted)),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgba(t.text))
                                .child(SharedString::from("Changes")),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(2.0))
                                .invisible()
                                .group_hover("git-changes-header", |s| s.visible())
                                .child(section_action(
                                    "git-discard-all-btn",
                                    "ui_icons/discard_tint.svg",
                                    "Discard All Changes",
                                    GitDiscardAll,
                                    t,
                                    cx,
                                ))
                                .child(section_action(
                                    "git-stage-all-btn",
                                    "ui_icons/plus_tint.svg",
                                    "Stage All Changes",
                                    GitStageAll,
                                    t,
                                    cx,
                                )),
                        )
                        .child(badge(unstaged.len())),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle_git_changes_section(cx);
                }));

            body = body.child(changes_header);

            if changes_expanded {
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .children(unstaged.iter().map(|c| {
                            change_row(c, false, "git-change-row", t, cx)
                        })),
                );
            }

            col = col.child(body);
        }
    }

    col.into_any_element()
}

fn header(
    t: &Colors,
    _window: &mut Window,
    _cx: &mut Context<Workspace>,
) -> impl IntoElement {
    div()
        .h(px(35.0))
        .px(px(12.0))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(11.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgba(t.text_muted))
                .child(SharedString::from("Source Control")),
        )
        .child(
            div()
                .id("git-header-more")
                .size(px(22.0))
                .rounded(px(4.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(rgba(t.ghost_hover)))
                .child(
                    svg()
                        .path("ui_icons/ellipsis_tint.svg")
                        .w(px(14.0))
                        .h(px(14.0))
                        .text_color(rgba(t.icon_muted)),
                )
                .context_menu(|menu, _window, _cx| {
                    menu.menu("Refresh", Box::new(GitRefresh))
                        .menu("Stage All Changes", Box::new(GitStageAll))
                        .menu("Unstage All Changes", Box::new(GitUnstageAll))
                        .menu("Discard All Changes", Box::new(GitDiscardAll))
                }),
        )
}

fn section_action(
    id: &'static str,
    icon_path: &'static str,
    tooltip: &'static str,
    action: impl gpui::Action + 'static,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(18.0))
        .rounded(px(3.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(rgba(t.element_hover)))
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .child(
            svg()
                .path(icon_path)
                .w(px(13.0))
                .h(px(13.0))
                .text_color(rgba(t.icon_muted)),
        )
        .on_click(cx.listener(move |_this, _, window, cx| {
            window.dispatch_action(action.boxed_clone(), cx);
        }))
}

fn badge(count: usize) -> impl IntoElement {
    div()
        .min_w(px(16.0))
        .h(px(16.0))
        .px(px(5.0))
        .rounded_full()
        .bg(rgba(0x0078d4ff))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.0))
        .font_weight(FontWeight::BOLD)
        .text_color(rgba(0xffffffff))
        .child(SharedString::from(count.to_string()))
}

fn commit_box(
    input: Option<&Entity<InputState>>,
    branch: &str,
    t: &Colors,
    _window: &mut Window,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let placeholder_text = format!("Message (Ctrl+Enter to commit on \"{branch}\"...)");

    let input_field = match input {
        Some(input) => div()
            .h(px(30.0))
            .px(px(8.0))
            .flex()
            .flex_row()
            .items_center()
            .bg(rgba(t.element_bg))
            .border_1()
            .border_color(rgba(t.border))
            .rounded(px(3.0))
            .child(
                Input::new(input)
                    .xsmall()
                    .text_size(px(12.5))
                    .appearance(false)
                    .bordered(false),
            )
            .into_any_element(),
        None => div()
            .h(px(30.0))
            .px(px(8.0))
            .flex()
            .flex_row()
            .items_center()
            .bg(rgba(t.element_bg))
            .border_1()
            .border_color(rgba(t.border))
            .rounded(px(3.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgba(t.text_muted))
                    .child(SharedString::from(placeholder_text)),
            )
            .into_any_element(),
    };

    let commit_split_btn = div()
        .h(px(28.0))
        .rounded(px(3.0))
        .bg(rgba(0x0078d4ff))
        .flex()
        .flex_row()
        .items_center()
        .overflow_hidden()
        .child(
            div()
                .id("git-commit-btn")
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(rgba(0x0086e6ff)))
                .text_size(px(12.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgba(0xffffffff))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            svg()
                                .path("ui_icons/check_tint.svg")
                                .w(px(14.0))
                                .h(px(14.0))
                                .text_color(rgba(0xffffffff)),
                        )
                        .child(SharedString::from("Commit")),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.git_commit(window, cx);
                })),
        )
        .child(
            div()
                .w(px(1.0))
                .h(px(18.0))
                .bg(rgba(0xffffff33)),
        )
        .child(
            div()
                .id("git-commit-dropdown-btn")
                .w(px(26.0))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(rgba(0x0086e6ff)))
                .child(
                    svg()
                        .path("ui_icons/chevron-down_tint.svg")
                        .w(px(14.0))
                        .h(px(14.0))
                        .text_color(rgba(0xffffffff)),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.git_commit(window, cx);
                })),
        );

    div()
        .px(px(12.0))
        .pt(px(2.0))
        .pb(px(8.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(input_field)
        .child(commit_split_btn)
}

fn change_row(
    change: &GitChange,
    staged_section: bool,
    id_prefix: &'static str,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let letter = if staged_section {
        change.staged_letter()
    } else {
        change.worktree_letter()
    };
    let kind = if staged_section {
        change.index.unwrap_or(ChangeKind::Modified)
    } else if change.is_untracked() {
        ChangeKind::Untracked
    } else {
        change.worktree.unwrap_or(ChangeKind::Modified)
    };
    let color = kind_color(kind, t);

    let rel_path = Path::new(&change.rel);
    let name = rel_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| change.rel.trim_end_matches('/').to_string());

    let parent_dir = rel_path.parent().and_then(|p| {
        let s = p.to_string_lossy();
        if s.is_empty() {
            None
        } else {
            Some(s.replace('/', "\\"))
        }
    });

    let old_name = change
        .old_rel
        .as_ref()
        .and_then(|old| old.rsplit('/').next())
        .map(|s| s.to_string());

    let path = change.path.clone();
    let file_icon_path = file_icons::icon_for(rel_path);

    let mut row = div()
        .id((ElementId::from(id_prefix), change.rel.clone()))
        .group("git-row")
        .w_full()
        .h(px(ROW_HEIGHT))
        .pl(px(20.0))
        .pr(px(12.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(t.ghost_hover)))
        .on_click(cx.listener({
            let path = path.clone();
            move |this, _, _, cx| {
                this.open_diff(&path, cx);
            }
        }));

    // Official File Type Icon
    row = row.child(icon_img(file_icon_path, 16.0));

    // File name and folder subpath
    row = row.child(
        div()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(12.5))
                    .text_color(rgba(t.text))
                    .child(SharedString::from(name)),
            )
            .when_some(parent_dir, |d, parent| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(px(11.5))
                        .text_color(rgba(t.text_muted))
                        .child(SharedString::from(parent)),
                )
            })
            .when_some(old_name, |d, old| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(px(11.0))
                        .text_color(rgba(t.text_muted))
                        .child(SharedString::from(format!("← {old}"))),
                )
            }),
    );

    // Per-file actions shown on hover:
    // In Changes section: Open File, Discard Changes (↺), Stage Changes (+)
    // In Staged section: Open File, Unstage Changes (−)
    let path_open = path.clone();
    let path_discard = path.clone();
    let path_stage = path.clone();
    let path_unstage = path.clone();
    let rel = change.rel.clone();

    if staged_section {
        row = row.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(2.0))
                .invisible()
                .group_hover("git-row", |s| s.visible())
                .child(
                    row_icon_btn(
                        SharedString::from(format!("git-open-file-staged-{rel}")),
                        "ui_icons/go-to-file_tint.svg",
                        "Open File",
                        t,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_file(path_open.clone(), window, cx);
                    })),
                )
                .child(
                    row_icon_btn(
                        SharedString::from(format!("git-unstage-file-{rel}")),
                        "ui_icons/minus_tint.svg",
                        "Unstage Changes",
                        t,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.git_unstage_path(&path_unstage, cx);
                    })),
                ),
        );
    } else {
        row = row.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(2.0))
                .invisible()
                .group_hover("git-row", |s| s.visible())
                .child(
                    row_icon_btn(
                        SharedString::from(format!("git-open-file-{rel}")),
                        "ui_icons/go-to-file_tint.svg",
                        "Open File",
                        t,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_file(path_open.clone(), window, cx);
                    })),
                )
                .child(
                    row_icon_btn(
                        SharedString::from(format!("git-discard-file-{rel}")),
                        "ui_icons/discard_tint.svg",
                        "Discard Changes",
                        t,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.git_discard_path(&path_discard, cx);
                    })),
                )
                .child(
                    row_icon_btn(
                        SharedString::from(format!("git-stage-file-{rel}")),
                        "ui_icons/plus_tint.svg",
                        "Stage Changes",
                        t,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.git_stage_path(&path_stage, cx);
                    })),
                ),
        );
    }

    // Status letter on far right (e.g. M, U, A, D)
    row = row.child(
        div()
            .w(px(14.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(11.5))
            .font_weight(FontWeight::BOLD)
            .text_color(rgba(color))
            .child(SharedString::from(letter)),
    );

    // Context menu: open / diff / stage / discard / copy.
    let path_c1 = change.path.clone();
    let path_c2 = change.path.clone();
    let path_c3 = change.path.clone();
    let path_c4 = change.path.clone();
    let path_c5 = change.path.clone();
    let is_staged = change.is_staged();
    let is_untracked = change.is_untracked();

    row.context_menu(move |menu, _window, _cx| {
        menu.menu("Open File", Box::new(GitOpenFile { path: path_c1.clone() }))
            .menu("Open Diff", Box::new(GitOpenDiff { path: path_c2.clone() }))
            .separator()
            .when(!is_staged && !is_untracked, |m| {
                m.menu("Stage Changes", Box::new(GitStageFile { path: path_c3.clone() }))
            })
            .when(is_staged, |m| {
                m.menu("Unstage Changes", Box::new(GitUnstageFile { path: path_c4.clone() }))
            })
            .when(!is_untracked, |m| {
                m.menu("Discard Changes", Box::new(GitDiscardFile { path: path_c5.clone() }))
            })
            .separator()
            .menu(
                "Reveal in File Explorer",
                Box::new(ExplorerRevealInFinder { path: path_c1.clone() }),
            )
            .menu("Copy Path", Box::new(ExplorerCopyPath { path: path_c2.clone() }))
    })
}

fn row_icon_btn(
    id: impl Into<ElementId>,
    icon_path: &'static str,
    tooltip: &'static str,
    t: &Colors,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(18.0))
        .rounded(px(3.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(rgba(t.element_hover)))
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .child(
            svg()
                .path(icon_path)
                .w(px(13.0))
                .h(px(13.0))
                .text_color(rgba(t.icon_muted)),
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
                .text_size(px(12.0))
                .text_color(rgba(t.text_muted))
                .child(SharedString::from(message.to_string())),
        )
}
