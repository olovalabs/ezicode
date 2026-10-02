use gpui::{
    div, prelude::*, px, rgba, svg, Context, FontWeight, IntoElement, SharedString, Window,
};

use gpui_component::tooltip::Tooltip;

use crate::workspace::{Activity, Workspace};

pub(crate) const ACTIVITY_BAR_WIDTH: f32 = 50.0;

pub(crate) fn render_activity_bar(
    activity: Activity,
    show_sidebar: bool,
    git_change_count: usize,
    project_switcher_open: bool,
    t: &crate::theme::Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    div()
        .id("activity-bar")
        .w(px(ACTIVITY_BAR_WIDTH))
        .h_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_between()
        .bg(rgba(t.background))
        .border_r_1()
        .border_color(rgba(t.border_variant))
        .py(px(8.0))
        .child(
            div()
                .id("activity-pill")
                .w(px(42.0))
                .rounded_full()
                .bg(rgba(t.elevated_surface))
                .border_1()
                .border_color(rgba(t.border_variant))
                .py(px(6.0))
                .px(px(2.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(4.0))
                .child(activity_icon(
                    "act-explorer",
                    "ui_icons/files_tint.svg",
                    show_sidebar && activity == Activity::Explorer,
                    None,
                    Activity::Explorer,
                    t,
                    cx,
                ))
                .child(activity_icon(
                    "act-search",
                    "ui_icons/search_tint.svg",
                    show_sidebar && activity == Activity::Search,
                    None,
                    Activity::Search,
                    t,
                    cx,
                ))
                .child(activity_icon(
                    "act-git",
                    "ui_icons/source-control_tint.svg",
                    show_sidebar && activity == Activity::Git,
                    (git_change_count > 0).then(|| git_change_count.to_string()),
                    Activity::Git,
                    t,
                    cx,
                ))
                .child(activity_icon(
                    "act-ext",
                    "ui_icons/extensions_tint.svg",
                    show_sidebar && activity == Activity::Extensions,
                    None,
                    Activity::Extensions,
                    t,
                    cx,
                )),
        )
        .child(
            div()
                .id("activity-bottom-actions")
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .id("activity-settings-pill")
                        .size(px(42.0))
                        .rounded_full()
                        .bg(rgba(t.elevated_surface))
                        .border_1()
                        .border_color(rgba(t.border_variant))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(activity_static_icon(
                            "act-settings",
                            "ui_icons/settings-gear_tint.svg",
                            t,
                            move |this, _, _window, cx| {
                                this.open_settings(cx);
                            },
                            cx,
                        )),
                )
                .child(project_switcher_icon(project_switcher_open, t, cx)),
        )
}

fn activity_icon(
    id: &'static str,
    svg_path: &'static str,
    selected: bool,
    badge: Option<String>,
    which: Activity,
    t: &crate::theme::Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let icon_color = if selected { t.text } else { t.icon_muted };

    let mut item = div()
        .id(SharedString::from(id))
        .group(SharedString::from(id))
        .size(px(38.0))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .relative();

    if selected {
        item = item
            .bg(rgba(t.element_selected))
            .border_1()
            .border_color(rgba(t.border_focused));
    } else {
        item = item.hover(|s| s.bg(rgba(t.ghost_hover)));
    }

    item = item.child(
        svg()
            .path(svg_path)
            .w(px(22.0))
            .h(px(22.0))
            .text_color(rgba(icon_color))
            .group_hover(SharedString::from(id), |s| s.text_color(rgba(t.text))),
    );

    if let Some(b) = badge {
        item = item.child(
            div()
                .absolute()
                .top(px(-1.0))
                .right(px(-1.0))
                .min_w(px(16.0))
                .h(px(16.0))
                .px(px(3.0))
                .rounded_full()
                .bg(rgba(t.text_accent))
                .border_1()
                .border_color(rgba(t.elevated_surface))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(9.5))
                .font_weight(FontWeight::BOLD)
                .text_color(rgba(t.background))
                .child(SharedString::from(b)),
        );
    }

    item.on_click(cx.listener(move |this, _, window, cx| {
        this.toggle_activity(which, window, cx);
    }))
}

fn activity_static_icon(
    id: &'static str,
    svg_path: &'static str,
    t: &crate::theme::Colors,
    action: impl Fn(&mut Workspace, &gpui::ClickEvent, &mut Window, &mut Context<Workspace>) + 'static,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id))
        .group(SharedString::from(id))
        .size(px(38.0))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|s| s.bg(rgba(t.ghost_hover)))
        .child(
            svg()
                .path(svg_path)
                .w(px(22.0))
                .h(px(22.0))
                .text_color(rgba(t.icon_muted))
                .group_hover(SharedString::from(id), |s| s.text_color(rgba(t.text))),
        )
        .on_click(cx.listener(action))
}

fn project_switcher_icon(
    selected: bool,
    t: &crate::theme::Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let icon_color = if selected { t.text } else { t.icon_muted };
    let mut item = div()
        .id("act-project-switcher")
        .group("act-project-switcher")
        .size(px(42.0))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer();

    if selected {
        item = item
            .bg(rgba(t.element_selected))
            .border_1()
            .border_color(rgba(t.border_focused));
    } else {
        item = item.hover(|style| style.bg(rgba(t.ghost_hover)));
    }

    item.child(
        svg()
            .path("ui_icons/project-switcher_tint.svg")
            .w(px(24.0))
            .h(px(24.0))
            .text_color(rgba(icon_color))
            .group_hover("act-project-switcher", |style| {
                style.text_color(rgba(t.text))
            }),
    )
    .tooltip(|window, cx| Tooltip::new("Switch Project (Ctrl+Shift+O)").build(window, cx))
    .on_click(cx.listener(|this, _, window, cx| {
        this.toggle_project_switcher(window, cx);
    }))
}
