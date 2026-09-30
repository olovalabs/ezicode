use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    div, prelude::FluentBuilder as _, px, App, Bounds, ClipboardItem, Div, Entity,
    InteractiveElement as _, ParentElement as _, Point, StatefulInteractiveElement as _,
    Styled as _, Window,
};

use crate::{
    avatar::Avatar,
    input::{popovers::Popover, BlameDetail, InputEvent, InputState},
    ActiveTheme as _, Icon, IconName, Sizable as _, Size, StyledExt as _,
};

/// Build the Zed-style git blame hover popover for the annotation on `row`
/// (anchored to its `range`): author header, the commit message, and a
/// footer with the commit date plus short-SHA / copy-SHA actions.
///
/// `bounds_sink` receives the popover's laid-out screen bounds each frame, so
/// the editor can keep it open while the mouse is over it. `anchor` is the
/// hover position the popover is placed under (Zed's behaviour).
pub(crate) fn blame_popover(
    editor: Entity<InputState>,
    range: Range<usize>,
    detail: BlameDetail,
    bounds_sink: Rc<Cell<Option<Bounds<gpui::Pixels>>>>,
    anchor: Option<Point<gpui::Pixels>>,
) -> Popover {
    let popover = Popover::new(
        "blame-popover",
        editor.clone(),
        range,
        move |window: &mut Window, cx: &mut App| {
            blame_card(&detail, editor.clone(), window, cx)
        },
    )
    .track_bounds(bounds_sink);

    match anchor {
        Some(anchor) => popover.anchor(anchor),
        None => popover,
    }
}

/// The inner card. Kept separate so [`blame_popover`] only wires anchoring.
fn blame_card(
    detail: &BlameDetail,
    editor: Entity<InputState>,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    // Precompute theme colors: the `.hover`/`.when` builder closures below do not
    // receive `cx`, so nothing inside them may call `cx.theme()`.
    let theme = cx.theme();
    let fg = theme.foreground;
    let muted = theme.muted_foreground;
    let border = theme.border;
    let secondary = theme.secondary;

    let sha_for_copy = detail.sha.clone();
    let sha_for_open = detail.sha.clone();
    let has_sha = !detail.sha.is_empty();

    // Zed's blame popover shows the author's hosting-provider avatar,
    // falling back to a person glyph when the remote has none.
    let avatar = {
        let mut avatar = Avatar::new().with_size(Size::Small);
        if !detail.avatar_url.is_empty() {
            avatar = avatar.src(detail.avatar_url.clone());
        }
        avatar
    };

    // Header: avatar + author + email on one row, separated from the
    // message by a bottom border (Zed's blame popover layout).
    let header = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .pb(px(4.0))
        .border_b_1()
        .border_color(border)
        .child(avatar)
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

    // Message, capped so a long commit body scrolls instead of
    // stretching the popover (Zed caps it at 12 lines).
    let message = div()
        .id("blame-message")
        .py_1p5()
        .max_h(px(160.0))
        .overflow_y_scroll()
        .text_color(fg)
        .child(super::render_markdown(
            "blame-message",
            detail.message.clone(),
            window,
            cx,
        ));

    // Footer: commit date on the left, the short SHA (opens the commit)
    // and a copy button on the right, under a top border.
    let footer = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .pt(px(4.0))
        .border_t_1()
        .border_color(border)
        .text_color(muted)
        .text_size(px(11.0))
        .child(
            div()
                .flex_1()
                .child(detail.date.clone()),
        )
        .when(has_sha, |this| {
            this.child(
                div()
                    .id("blame-open-commit")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_0p5()
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(border)
                    .text_color(fg)
                    .hover(|s| s.bg(secondary))
                    .child(
                        Icon::new(IconName::File)
                            .path("ui_icons/git_branch.svg")
                            .size_3p5(),
                    )
                    .child(detail.short_sha.clone())
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
            .child(
                div()
                    .id("blame-copy-sha")
                    .p_1()
                    .rounded(px(4.0))
                    .text_color(fg)
                    .hover(|s| s.bg(secondary))
                    .child(Icon::new(IconName::Copy).size_3p5())
                    .on_click(move |_, _window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(
                            sha_for_copy.to_string(),
                        ));
                    }),
            )
        });

    div()
        .flex()
        .flex_col()
        .gap_1()
        .min_w(px(280.0))
        .max_w(px(480.0))
        .child(header)
        .child(message)
        .child(footer)
}
