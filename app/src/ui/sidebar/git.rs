use std::path::Path;

use gpui::{
    div, prelude::*, px, rgba, svg, AnyElement, Context, ElementId, Entity, FontWeight,
    IntoElement, SharedString, Window,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
    menu::{ContextMenuExt, DropdownMenu as _},
    scroll::ScrollableElement as _,
    tooltip::Tooltip,
    Sizable,
};

use crate::actions::{
    ExplorerCopyPath, ExplorerRevealInFinder, GitBranchPicker, GitCommitAll, GitCommitAmend,
    GitDiscardAll, GitDiscardFile, GitFetch, GitForcePush, GitInit, GitOpenDiff, GitOpenFile,
    GitPull, GitPush, GitRefresh, GitStageAll, GitStageFile, GitStashPop, GitStashPush,
    GitUnstageAll, GitUnstageFile,
};
use crate::file_icons;
use crate::git::{ChangeKind, GitChange, RepoStatus};
use crate::theme::Colors;
use crate::ui::common::icon_img;
use crate::workspace::{GitConfirm, GitSection, Workspace};

const ROW_HEIGHT: f32 = 26.0;

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

/// Which panel section a row is rendered in — decides its letter, color and
/// hover actions.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowSection {
    Conflict,
    Staged,
    Unstaged,
    Untracked,
}

/// Everything the panel needs from the workspace, bundled so the render
/// call in `render.rs` stays readable as the parameter list grows.
pub(crate) struct GitPanelParams<'a> {
    pub commit_input: Option<&'a Entity<InputState>>,
    pub repo: Option<&'a RepoStatus>,
    /// Whether a folder is open at all (drives the `git init` empty state).
    pub has_root: bool,
    pub repo_section_expanded: bool,
    pub conflicts_expanded: bool,
    pub staged_expanded: bool,
    pub changes_expanded: bool,
    pub untracked_expanded: bool,
    /// Label of the remote operation in flight, e.g. "Push".
    pub op_running: Option<&'static str>,
}

pub(crate) fn render_git_panel(
    params: GitPanelParams,
    t: &Colors,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let mut col = div()
        .size_full()
        .flex()
        .flex_col()
        .bg(rgba(t.panel))
        .overflow_hidden();

    col = col.child(header(t, window, cx));

    match params.repo {
        None => {
            if params.has_root {
                col = col.child(init_repo_state(t, cx));
            } else {
                col = col.child(empty_state(
                    "Open a folder inside a Git repository\nto see your changes here.",
                    t,
                ));
            }
        }
        Some(repo) => {
            col = col.child(branch_row(repo, params.op_running, t, cx));

            let mut body = div()
                .id("git-sidebar-scroll")
                .flex_1()
                .w_full()
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .overflow_y_scrollbar();

            let conflicts: Vec<GitChange> = repo
                .changes
                .iter()
                .filter(|c| c.is_conflicted())
                .cloned()
                .collect();
            let staged: Vec<GitChange> = repo
                .changes
                .iter()
                .filter(|c| c.is_staged() && !c.is_conflicted())
                .cloned()
                .collect();
            let unstaged: Vec<GitChange> = repo
                .changes
                .iter()
                .filter(|c| !c.is_untracked() && !c.is_conflicted() && c.worktree.is_some())
                .cloned()
                .collect();
            let untracked: Vec<GitChange> = repo
                .changes
                .iter()
                .filter(|c| c.is_untracked())
                .cloned()
                .collect();

            let branch = repo.branch.clone().unwrap_or_else(|| "main".to_string());

            body = body.child(section_header(
                "git-repo-header",
                "Repository",
                params.repo_section_expanded,
                None,
                GitSection::Repo,
                Vec::new(),
                t,
                cx,
            ));

            if params.repo_section_expanded {
                body = body.child(commit_box(params.commit_input, &branch, t, window, cx));
            }

            if repo.changes.is_empty() {
                body = body.child(
                    div()
                        .px(px(20.0))
                        .py(px(10.0))
                        .text_size(px(12.5))
                        .text_color(rgba(t.text_muted))
                        .child(SharedString::from("No changes — working tree clean")),
                );
            }

            if !conflicts.is_empty() {
                let actions = vec![section_action(
                    "git-conflicts-stage-all-btn",
                    "ui_icons/plus_tint.svg",
                    "Stage All (Mark All Resolved)",
                    GitStageAll,
                    t,
                    cx,
                )
                .into_any_element()];
                body = body.child(section_header(
                    "git-conflicts-header",
                    "Merge Conflicts",
                    params.conflicts_expanded,
                    Some(conflicts.len()),
                    GitSection::Conflicts,
                    actions,
                    t,
                    cx,
                ));
                if params.conflicts_expanded {
                    body =
                        body.child(div().flex().flex_col().children(conflicts.iter().map(|c| {
                            change_row(c, RowSection::Conflict, "git-conflict-row", t, cx)
                        })));
                }
            }

            if !staged.is_empty() {
                let actions = vec![section_action(
                    "git-unstage-all-btn",
                    "ui_icons/minus_tint.svg",
                    "Unstage All Changes",
                    GitUnstageAll,
                    t,
                    cx,
                )
                .into_any_element()];
                body = body.child(section_header(
                    "git-staged-header",
                    "Staged Changes",
                    params.staged_expanded,
                    Some(staged.len()),
                    GitSection::Staged,
                    actions,
                    t,
                    cx,
                ));
                if params.staged_expanded {
                    body =
                        body.child(div().flex().flex_col().children(
                            staged.iter().map(|c| {
                                change_row(c, RowSection::Staged, "git-staged-row", t, cx)
                            }),
                        ));
                }
            }

            if !unstaged.is_empty() {
                let actions = vec![
                    section_action(
                        "git-discard-all-btn",
                        "ui_icons/discard_tint.svg",
                        "Discard All Changes",
                        GitDiscardAll,
                        t,
                        cx,
                    )
                    .into_any_element(),
                    section_action(
                        "git-stage-all-btn",
                        "ui_icons/plus_tint.svg",
                        "Stage All Changes",
                        GitStageAll,
                        t,
                        cx,
                    )
                    .into_any_element(),
                ];
                body = body.child(section_header(
                    "git-changes-header",
                    "Changes",
                    params.changes_expanded,
                    Some(unstaged.len()),
                    GitSection::Changes,
                    actions,
                    t,
                    cx,
                ));
                if params.changes_expanded {
                    body =
                        body.child(div().flex().flex_col().children(unstaged.iter().map(|c| {
                            change_row(c, RowSection::Unstaged, "git-change-row", t, cx)
                        })));
                }
            }

            if !untracked.is_empty() {
                let actions = vec![section_action(
                    "git-untracked-stage-all-btn",
                    "ui_icons/plus_tint.svg",
                    "Stage All Untracked Files",
                    GitStageAll,
                    t,
                    cx,
                )
                .into_any_element()];
                body = body.child(section_header(
                    "git-untracked-header",
                    "Untracked",
                    params.untracked_expanded,
                    Some(untracked.len()),
                    GitSection::Untracked,
                    actions,
                    t,
                    cx,
                ));
                if params.untracked_expanded {
                    body =
                        body.child(div().flex().flex_col().children(untracked.iter().map(|c| {
                            change_row(c, RowSection::Untracked, "git-untracked-row", t, cx)
                        })));
                }
            }

            col = col.child(body);
        }
    }

    col.into_any_element()
}

fn header(t: &Colors, _window: &mut Window, _cx: &mut Context<Workspace>) -> impl IntoElement {
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
                .child(SharedString::from("Source Control")),
        )
        .child(
            div()
                .id("git-header-more")
                .size(px(26.0))
                .rounded(px(4.0))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(rgba(t.ghost_hover)))
                .child(
                    svg()
                        .path("ui_icons/ellipsis_tint.svg")
                        .w(px(16.0))
                        .h(px(16.0))
                        .text_color(rgba(t.icon_muted)),
                )
                .context_menu(|menu, _window, _cx| {
                    menu.menu("Refresh", Box::new(GitRefresh))
                        .separator()
                        .menu("Fetch", Box::new(GitFetch))
                        .menu("Pull", Box::new(GitPull))
                        .menu("Push", Box::new(GitPush))
                        .menu("Force Push (with lease)", Box::new(GitForcePush))
                        .separator()
                        .menu("Checkout Branch…", Box::new(GitBranchPicker))
                        .separator()
                        .menu("Stash Changes", Box::new(GitStashPush))
                        .menu("Pop Stash", Box::new(GitStashPop))
                        .separator()
                        .menu("Commit All (Tracked)", Box::new(GitCommitAll))
                        .menu("Amend Last Commit", Box::new(GitCommitAmend))
                        .separator()
                        .menu("Stage All Changes", Box::new(GitStageAll))
                        .menu("Unstage All Changes", Box::new(GitUnstageAll))
                        .menu("Discard All Changes", Box::new(GitDiscardAll))
                }),
        )
}

/// Branch + sync strip: current branch (click to switch), ahead/behind
/// counts, and fetch / pull / push shortcuts. Mirrors the top strip of
/// Zed's git panel.
fn branch_row(
    repo: &RepoStatus,
    op_running: Option<&'static str>,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let branch = repo.branch.clone().unwrap_or_else(|| "(no branch)".into());
    let branch_label = if repo.detached {
        format!("{branch} (detached)")
    } else {
        branch
    };

    let mut row = div()
        .h(px(30.0))
        .px(px(12.0))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .border_b_1()
        .border_color(rgba(t.border_variant));

    let mut left = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .min_w(px(0.0))
        .child(
            div()
                .id("git-branch-switcher")
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .px(px(4.0))
                .py(px(2.0))
                .rounded(px(3.0))
                .cursor_pointer()
                .hover(|s| s.bg(rgba(t.ghost_hover)))
                .tooltip(|window, cx| {
                    Tooltip::new("Switch Branch (create, checkout)").build(window, cx)
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_branch_picker(window, cx);
                }))
                .child(
                    svg()
                        .path("ui_icons/git_branch.svg")
                        .w(px(13.0))
                        .h(px(13.0))
                        .text_color(rgba(t.text)),
                )
                .child(
                    div()
                        .max_w(px(150.0))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgba(t.text))
                        .child(SharedString::from(branch_label)),
                ),
        );

    if repo.ahead > 0 || repo.behind > 0 {
        left = left.child(
            div()
                .text_size(px(11.5))
                .text_color(rgba(t.text_muted))
                .child(SharedString::from(format!(
                    "↑{} ↓{}",
                    repo.ahead, repo.behind
                ))),
        );
    }

    row = row.child(left);

    let right = if let Some(op) = op_running {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .text_size(px(11.5))
            .text_color(rgba(t.text_accent))
            .child(SharedString::from(format!("{op}…")))
            .into_any_element()
    } else {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0))
            .child(
                row_icon_btn(
                    SharedString::from("git-fetch-btn"),
                    "ui_icons/refresh_tint.svg",
                    "Fetch (git fetch --all --prune)",
                    t,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.git_fetch(cx);
                })),
            )
            .child(
                row_icon_btn(
                    SharedString::from("git-pull-btn"),
                    "ui_icons/chevron-down_tint.svg",
                    "Pull",
                    t,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.git_pull(cx);
                })),
            )
            .child(
                row_icon_btn(
                    SharedString::from("git-push-btn"),
                    "ui_icons/chevron-up_tint.svg",
                    "Push (publishes the branch when needed)",
                    t,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.git_push(false, cx);
                })),
            )
            .into_any_element()
    };

    row.child(right)
}

/// A collapsible section header: chevron + bold label on the left, hover
/// actions + count badge on the right.
#[allow(clippy::too_many_arguments)]
fn section_header(
    id: &'static str,
    label: &'static str,
    expanded: bool,
    count: Option<usize>,
    section: GitSection,
    actions: Vec<AnyElement>,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let chevron = if expanded {
        "ui_icons/chevron-down_tint.svg"
    } else {
        "ui_icons/chevron-right_tint.svg"
    };

    let label_color = if section == GitSection::Conflicts {
        t.vc_deleted
    } else {
        t.text
    };

    let mut right = div().flex().flex_row().items_center().gap(px(4.0));
    if !actions.is_empty() {
        right = right.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(2.0))
                .invisible()
                .group_hover(id, |s| s.visible())
                .children(actions),
        );
    }
    if let Some(count) = count {
        right = right.child(badge(count, t));
    }

    div()
        .id(id)
        .group(id)
        .h(px(26.0))
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
                    div()
                        .w(px(16.0))
                        .h(px(16.0))
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
                        .text_size(px(12.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgba(label_color))
                        .child(SharedString::from(label)),
                ),
        )
        .child(right)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.toggle_git_section(section, cx);
        }))
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
        .size(px(22.0))
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
                .w(px(14.0))
                .h(px(14.0))
                .text_color(rgba(t.icon_muted)),
        )
        .on_click(cx.listener(move |_this, _, window, cx| {
            cx.stop_propagation();
            window.dispatch_action(action.boxed_clone(), cx);
        }))
}

fn badge(count: usize, t: &Colors) -> impl IntoElement {
    div()
        .min_w(px(18.0))
        .h(px(18.0))
        .px(px(5.0))
        .rounded_full()
        .bg(rgba(t.text_accent))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.5))
        .font_weight(FontWeight::BOLD)
        .text_color(rgba(t.background))
        .child(SharedString::from(count.to_string()))
}

fn commit_box(
    input: Option<&Entity<InputState>>,
    branch: &str,
    t: &Colors,
    _window: &mut Window,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let placeholder_text = format!("Message (Enter to commit on \"{branch}\")");

    let input_field = match input {
        Some(input) => div()
            .h(px(34.0))
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
                    .text_size(px(13.0))
                    .appearance(false)
                    .bordered(false),
            )
            .into_any_element(),
        None => div()
            .h(px(34.0))
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
                    .text_size(px(12.5))
                    .text_color(rgba(t.text_muted))
                    .child(SharedString::from(placeholder_text)),
            )
            .into_any_element(),
    };

    let commit_split_btn = div()
        .h(px(32.0))
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
                .text_size(px(13.0))
                .font_weight(FontWeight::BOLD)
                .text_color(rgba(0xffffffff))
                .tooltip(|window, cx| {
                    Tooltip::new("Commit staged changes (all tracked changes when nothing staged)")
                        .build(window, cx)
                })
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(5.0))
                        .child(
                            svg()
                                .path("ui_icons/check_tint.svg")
                                .w(px(16.0))
                                .h(px(16.0))
                                .text_color(rgba(0xffffffff)),
                        )
                        .child(SharedString::from("Commit")),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.git_commit(window, cx);
                })),
        )
        .child(div().w(px(1.0)).h(px(20.0)).bg(rgba(0xffffff33)))
        .child(
            Button::new("git-commit-dropdown-btn")
                .ghost()
                .compact()
                .label("▾")
                .text_color(rgba(0xffffffff))
                .dropdown_menu(|menu, _window, _cx| {
                    menu.menu("Commit All (Tracked)", Box::new(GitCommitAll))
                        .menu("Amend Last Commit", Box::new(GitCommitAmend))
                }),
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
    section: RowSection,
    id_prefix: &'static str,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let (letter, kind) = match section {
        RowSection::Conflict => ("!", ChangeKind::Conflicted),
        RowSection::Staged => (
            change.staged_letter(),
            change.index.unwrap_or(ChangeKind::Modified),
        ),
        RowSection::Unstaged => (
            change.worktree_letter(),
            change.worktree.unwrap_or(ChangeKind::Modified),
        ),
        RowSection::Untracked => ("U", ChangeKind::Untracked),
    };
    let color = kind_color(kind, t);

    let rel_path = Path::new(&change.rel);
    let name = rel_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| change.rel.trim_end_matches('/').to_string());

    // Platform separator, not a hard-coded backslash.
    let parent_dir = rel_path.parent().and_then(|p| {
        let s = p.to_string_lossy();
        if s.is_empty() {
            None
        } else {
            Some(s.replace(&['/', '\\'][..], std::path::MAIN_SEPARATOR_STR))
        }
    });

    let old_name = change
        .old_rel
        .as_ref()
        .and_then(|old| old.rsplit('/').next())
        .map(|s| s.to_string());

    let path = change.path.clone();
    let file_icon_path = file_icons::icon_for(rel_path);

    let is_conflict = section == RowSection::Conflict;
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
            move |this, _, window, cx| {
                // A conflicted file needs editing, not diffing: open the
                // buffer with its conflict markers.
                if is_conflict {
                    this.open_file(path.clone(), window, cx);
                } else {
                    this.open_diff(&path, cx);
                }
            }
        }));

    // Official file type icon.
    row = row.child(icon_img(file_icon_path, 18.0));

    // File name, folder subpath, rename source.
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
                    .text_size(px(13.5))
                    .text_color(rgba(if is_conflict { t.vc_deleted } else { t.text }))
                    .child(SharedString::from(name)),
            )
            .when_some(parent_dir, |d, parent| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(px(12.0))
                        .text_color(rgba(t.text_muted))
                        .child(SharedString::from(parent)),
                )
            })
            .when_some(old_name, |d, old| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(px(12.0))
                        .text_color(rgba(t.text_muted))
                        .child(SharedString::from(format!("← {old}"))),
                )
            }),
    );

    // Per-file actions shown on hover.
    let path_open = path.clone();
    let path_discard = path.clone();
    let path_stage = path.clone();
    let path_unstage = path.clone();
    let rel = change.rel.clone();

    let mut hover_actions = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(2.0))
        .invisible()
        .group_hover("git-row", |s| s.visible());

    hover_actions = hover_actions.child(
        row_icon_btn(
            SharedString::from(format!("git-open-file-{id_prefix}-{rel}")),
            "ui_icons/go-to-file_tint.svg",
            "Open File",
            t,
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            this.open_file(path_open.clone(), window, cx);
        })),
    );

    match section {
        RowSection::Staged => {
            hover_actions = hover_actions.child(
                row_icon_btn(
                    SharedString::from(format!("git-unstage-file-{rel}")),
                    "ui_icons/minus_tint.svg",
                    "Unstage Changes",
                    t,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.git_unstage_path(&path_unstage, cx);
                })),
            );
        }
        RowSection::Conflict => {
            hover_actions = hover_actions.child(
                row_icon_btn(
                    SharedString::from(format!("git-resolve-file-{rel}")),
                    "ui_icons/plus_tint.svg",
                    "Stage (Mark as Resolved)",
                    t,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.git_stage_path(&path_stage, cx);
                })),
            );
        }
        RowSection::Unstaged | RowSection::Untracked => {
            hover_actions = hover_actions
                .child(
                    row_icon_btn(
                        SharedString::from(format!("git-discard-file-{rel}")),
                        "ui_icons/discard_tint.svg",
                        if section == RowSection::Untracked {
                            "Delete File (untracked)"
                        } else {
                            "Discard Changes"
                        },
                        t,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.git_request_discard_path(&path_discard, cx);
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
                        cx.stop_propagation();
                        this.git_stage_path(&path_stage, cx);
                    })),
                );
        }
    }

    row = row.child(hover_actions);

    // Status letter on the far right (M, U, A, D, R, !).
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
    let is_staged_row = section == RowSection::Staged;
    let can_stage = matches!(
        section,
        RowSection::Unstaged | RowSection::Untracked | RowSection::Conflict
    );
    let can_discard = matches!(section, RowSection::Unstaged | RowSection::Untracked);

    row.context_menu(move |menu, _window, _cx| {
        menu.menu(
            "Open File",
            Box::new(GitOpenFile {
                path: path_c1.clone(),
            }),
        )
        .menu(
            "Open Diff",
            Box::new(GitOpenDiff {
                path: path_c2.clone(),
            }),
        )
        .separator()
        .when(can_stage, |m| {
            m.menu(
                "Stage Changes",
                Box::new(GitStageFile {
                    path: path_c3.clone(),
                }),
            )
        })
        .when(is_staged_row, |m| {
            m.menu(
                "Unstage Changes",
                Box::new(GitUnstageFile {
                    path: path_c4.clone(),
                }),
            )
        })
        .when(can_discard, |m| {
            m.menu(
                "Discard Changes",
                Box::new(GitDiscardFile {
                    path: path_c5.clone(),
                }),
            )
        })
        .separator()
        .menu(
            "Reveal in File Explorer",
            Box::new(ExplorerRevealInFinder {
                path: path_c1.clone(),
            }),
        )
        .menu(
            "Copy Path",
            Box::new(ExplorerCopyPath {
                path: path_c2.clone(),
            }),
        )
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
        .size(px(22.0))
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
                .w(px(14.0))
                .h(px(14.0))
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
                .text_size(px(13.0))
                .text_color(rgba(t.text_muted))
                .child(SharedString::from(message.to_string())),
        )
}

/// Empty state when a folder is open but not a repository: offer `git init`.
fn init_repo_state(t: &Colors, cx: &mut Context<Workspace>) -> impl IntoElement {
    div()
        .flex_1()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(12.0))
        .px(px(20.0))
        .child(
            div()
                .text_size(px(13.0))
                .text_color(rgba(t.text_muted))
                .child(SharedString::from(
                    "This folder is not a Git repository yet.",
                )),
        )
        .child(
            div()
                .id("git-init-btn")
                .h(px(30.0))
                .px(px(14.0))
                .rounded(px(4.0))
                .bg(rgba(0x0078d4ff))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(rgba(0x0086e6ff)))
                .text_size(px(12.5))
                .font_weight(FontWeight::BOLD)
                .text_color(rgba(0xffffffff))
                .child(SharedString::from("Initialize Repository"))
                .on_click(cx.listener(|_this, _, window, cx| {
                    window.dispatch_action(Box::new(GitInit), cx);
                })),
        )
}

/// Modal confirmation for destructive git actions (discard / delete).
pub(crate) fn render_git_confirm(
    confirm: &GitConfirm,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    div()
        .id("git-confirm-backdrop")
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .occlude()
        .bg(rgba(0x00000088))
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(|this, _, _, cx| {
                this.git_confirm_cancel(cx);
                cx.stop_propagation();
            }),
        )
        .child(
            div()
                .id("git-confirm-card")
                .w(px(440.0))
                .bg(rgba(t.panel))
                .rounded(px(8.0))
                .border_1()
                .border_color(rgba(t.border))
                .shadow_2xl()
                .overflow_hidden()
                .p(px(16.0))
                .flex()
                .flex_col()
                .gap(px(10.0))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgba(t.text))
                        .child(SharedString::from(confirm.title.clone())),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgba(t.text_muted))
                        .child(SharedString::from(confirm.detail.clone())),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(px(8.0))
                        .pt(px(6.0))
                        .child(
                            div()
                                .id("git-confirm-cancel")
                                .h(px(28.0))
                                .px(px(12.0))
                                .rounded(px(4.0))
                                .bg(rgba(t.element_bg))
                                .border_1()
                                .border_color(rgba(t.border))
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .hover(|s| s.bg(rgba(t.element_hover)))
                                .text_size(px(12.5))
                                .text_color(rgba(t.text))
                                .child(SharedString::from("Cancel"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.git_confirm_cancel(cx);
                                })),
                        )
                        .child(
                            div()
                                .id("git-confirm-accept")
                                .h(px(28.0))
                                .px(px(12.0))
                                .rounded(px(4.0))
                                .bg(rgba(t.vc_deleted))
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .hover(|s| s.opacity(0.9))
                                .text_size(px(12.5))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgba(0xffffffff))
                                .child(SharedString::from(confirm.confirm_label.clone()))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.git_confirm_accept(cx);
                                })),
                        ),
                ),
        )
        .into_any_element()
}
