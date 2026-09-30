use std::ops::Range;

use gpui::{
    div, prelude::FluentBuilder as _, px, App, ClipboardItem, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, StatefulInteractiveElement as _, Styled as _, Window,
};

use crate::{
    input::{popovers::Popover, BlameDetail, InputEvent, InputState},
    ActiveTheme as _, Colorize as _, StyledExt as _,
};

/// Build the Zed-style git blame hover popover for the annotation on `row`
/// (anchored to its `range`), showing the author, full date, message, short
/// SHA, and copy-SHA / open-commit actions.
pub(crate) fn blame_popover(
    editor: Entity<InputState>,
    range: Range<usize>,
    detail: BlameDetail,
) -> Popover {
    Popover::new(
        "blame-popover",
        editor.clone(),
        range,
        move |_window: &mut Window, cx: &mut App| blame_card(&detail, editor.clone(), cx),
    )
}

/// The inner card. Kept separate so [`blame_popover`] only wires anchoring.
fn blame_card(detail: &BlameDetail, editor: Entity<InputState>, cx: &mut App) -> impl IntoElement {
    // Precompute theme colors: the `.hover`/`.when` builder closures below do not
    // receive `cx`, so nothing inside them may call `cx.theme()`.
    let theme = cx.theme();
    let fg = theme.foreground;
    let muted = theme.muted_foreground;
    let border = theme.border;
    let accent = theme.accent;
    let accent_fg = theme.accent_foreground;
    let secondary = theme.secondary;

    let initials = author_initials(detail.author.as_ref());
    let sha_for_copy = detail.sha.clone();
    let sha_for_open = detail.sha.clone();
    let has_sha = !detail.sha.is_empty();

    let avatar = div()
        .flex_none()
        .size(px(28.0))
        .rounded_full()
        .bg(accent)
        .text_color(accent_fg)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.0))
        .child(initials);

    let identity = div()
        .flex()
        .flex_col()
        .child(
            div()
                .font_semibold()
                .text_color(fg)
                .child(detail.author.clone()),
        )
        .when(!detail.author_email.is_empty(), |this| {
            this.child(
                div()
                    .text_color(muted)
                    .text_size(px(11.0))
                    .child(detail.author_email.clone()),
            )
        });

    let footer = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .pt_1()
        .child(
            div()
                .flex_1()
                .text_color(muted)
                .text_size(px(11.0))
                .child(detail.short_sha.clone()),
        )
        .when(has_sha, |this| {
            this.child(
                div()
                    .id("blame-copy-sha")
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .text_color(fg)
                    .hover(|s| s.bg(secondary))
                    .child("Copy SHA")
                    .on_click(move |_, _window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(sha_for_copy.to_string()));
                    }),
            )
            .child(
                div()
                    .id("blame-open-commit")
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .bg(accent)
                    .text_color(accent_fg)
                    .hover(|s| s.bg(accent.opacity(0.85)))
                    .child("Open commit")
                    .on_click({
                        let editor = editor.clone();
                        move |_, _window, cx| {
                            let sha = sha_for_open.clone();
                            editor.update(cx, |_, cx| {
                                cx.emit(InputEvent::BlameOpenCommit { sha });
                            });
                        }
                    }),
            )
        });

    div()
        .flex()
        .flex_col()
        .gap_1()
        .min_w(px(260.0))
        .max_w(px(460.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(avatar)
                .child(identity),
        )
        .when(!detail.date.is_empty(), |this| {
            this.child(
                div()
                    .text_color(muted)
                    .text_size(px(11.0))
                    .child(detail.date.clone()),
            )
        })
        .child(div().my_1().h(px(1.0)).w_full().bg(border))
        .child(
            div()
                .whitespace_normal()
                .text_color(fg)
                .child(detail.message.clone()),
        )
        .child(footer)
}

/// Up to two uppercase initials from an author display name.
fn author_initials(name: &str) -> String {
    let mut initials = String::new();
    for word in name.split_whitespace().take(2) {
        if let Some(c) = word.chars().next() {
            initials.extend(c.to_uppercase());
        }
    }
    if initials.is_empty() {
        initials.push('?');
    }
    initials
}
