use gpui::{
    div, prelude::*, px, rgba, svg, Context, Entity, FontWeight, IntoElement, SharedString, Window,
};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement;

use crate::settings::{AutoSaveMode, FormatOnSaveMode, Settings};
use crate::theme::{self, Colors};
use crate::ui::scale::rem;
use crate::workspace::Workspace;

pub(crate) const SETTINGS_CATEGORIES: &[&str] = &[
    "Commonly Used",
    "Text Editor",
    "Workbench",
    "Window",
    "Features",
    "Application",
    "Security",
    "Extensions",
];
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_settings(
    settings: &Settings,
    t: &Colors,
    active_theme_ix: usize,
    font_size: f32,
    active_category: usize,
    active_scope: usize,
    search_input: Option<&Entity<InputState>>,
    search_query: &str,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    div()
        .id("settings-view")
        .flex_1()
        .min_h(px(0.0))
        .w_full()
        .h_full()
        .bg(rgba(t.editor_bg))
        .flex()
        .flex_col()
        .overflow_hidden()
        .child(render_vscode_header(active_scope, search_input, t, cx))
        .child(
            div()
                .flex_1()
                .min_h(px(0.0))
                .w_full()
                .flex()
                .flex_row()
                .overflow_hidden()
                .child(render_category_sidebar(active_category, t, cx))
                .child(render_settings_content(
                    settings,
                    t,
                    active_theme_ix,
                    font_size,
                    active_category,
                    search_query,
                    cx,
                )),
        )
}

/// Top header matching the VS Code settings screenshot:
/// - Search settings input box with blue focus border
/// - Tabs row: [User] | Workspace + Open Settings (JSON) button on the right
fn render_vscode_header(
    active_scope: usize,
    search_input: Option<&Entity<InputState>>,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    div()
        .w_full()
        .px(rem(24.0))
        .pt(rem(14.0))
        .pb(rem(10.0))
        .bg(rgba(t.editor_bg))
        .border_b_1()
        .border_color(rgba(t.border_variant))
        .flex()
        .flex_col()
        .gap(rem(10.0))
        .child(
            // Top Search Bar
            div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(rem(16.0))
                .child(
                    div()
                        .flex_1()
                        .max_w(rem(780.0))
                        .h(rem(32.0))
                        .px(rem(10.0))
                        .rounded(px(3.0))
                        .bg(rgba(t.element_bg))
                        .border_1()
                        .border_color(rgba(t.border_focused))
                        .flex()
                        .items_center()
                        .gap(rem(8.0))
                        .child(div().flex_1().min_w(px(0.0)).child(
                            if let Some(input) = search_input {
                                Input::new(input)
                                    .text_size(rem(13.0))
                                    .appearance(false)
                                    .cleanable(true)
                                    .into_any_element()
                            } else {
                                div()
                                    .text_size(rem(13.0))
                                    .text_color(rgba(t.text_muted))
                                    .child("Search settings")
                                    .into_any_element()
                            },
                        )),
                )
                .child(
                    div()
                        .id("open-settings-json-btn")
                        .flex()
                        .items_center()
                        .gap(rem(6.0))
                        .px(rem(10.0))
                        .py(rem(4.0))
                        .rounded(px(3.0))
                        .bg(rgba(t.element_bg))
                        .border_1()
                        .border_color(rgba(t.border))
                        .hover(|s| {
                            s.bg(rgba(t.element_hover))
                                .border_color(rgba(t.border_focused))
                        })
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_settings_json(window, cx);
                        }))
                        .child(
                            svg()
                                .path("ui_icons/file-code_tint.svg")
                                .w(rem(14.0))
                                .h(rem(14.0))
                                .text_color(rgba(t.text_accent)),
                        )
                        .child(
                            div()
                                .text_size(rem(12.0))
                                .text_color(rgba(t.text))
                                .child("Open Settings (JSON)"),
                        ),
                ),
        )
        .child(
            // Scope Tabs: [User] | Workspace
            div()
                .flex()
                .items_center()
                .gap(rem(16.0))
                .child(
                    div()
                        .id("scope-tab-user")
                        .px(rem(10.0))
                        .py(rem(3.0))
                        .rounded(px(3.0))
                        .cursor_pointer()
                        .when(active_scope == 0, |d| {
                            d.bg(rgba(0x37373dff))
                                .text_color(rgba(t.text))
                                .font_weight(FontWeight::SEMIBOLD)
                        })
                        .when(active_scope != 0, |d| {
                            d.text_color(rgba(t.text_muted))
                                .hover(|s| s.text_color(rgba(t.text)))
                        })
                        .text_size(rem(13.0))
                        .child("User")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_settings_scope(0, cx);
                        })),
                )
                .child(
                    div()
                        .id("scope-tab-workspace")
                        .px(rem(10.0))
                        .py(rem(3.0))
                        .rounded(px(3.0))
                        .cursor_pointer()
                        .when(active_scope == 1, |d| {
                            d.bg(rgba(0x37373dff))
                                .text_color(rgba(t.text))
                                .font_weight(FontWeight::SEMIBOLD)
                        })
                        .when(active_scope != 1, |d| {
                            d.text_color(rgba(t.text_muted))
                                .hover(|s| s.text_color(rgba(t.text)))
                        })
                        .text_size(rem(13.0))
                        .child("Workspace")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_settings_scope(1, cx);
                        })),
                ),
        )
}

/// Left Navigation Sidebar matching VS Code:
/// - Category list: Commonly Used, Text Editor, Workbench, Window, Features, Application, Security, Extensions
fn render_category_sidebar(
    active_category: usize,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    div()
        .w(rem(200.0))
        .flex_none()
        .h_full()
        .bg(rgba(t.editor_bg))
        .border_r_1()
        .border_color(rgba(t.border_variant))
        .py(rem(14.0))
        .px(rem(10.0))
        .flex()
        .flex_col()
        .gap(rem(2.0))
        .overflow_y_scrollbar()
        .children(SETTINGS_CATEGORIES.iter().enumerate().map(|(idx, name)| {
            let is_selected = idx == active_category;
            div()
                .id(SharedString::from(format!("cat-nav-{idx}")))
                .h(rem(28.0))
                .px(rem(8.0))
                .rounded(px(3.0))
                .flex()
                .items_center()
                .gap(rem(6.0))
                .cursor_pointer()
                .when(is_selected, |d| {
                    d.bg(rgba(t.element_selected))
                        .text_color(rgba(t.text))
                        .font_weight(FontWeight::SEMIBOLD)
                })
                .when(!is_selected, |d| {
                    d.text_color(rgba(t.text_muted))
                        .hover(|s| s.bg(rgba(t.element_hover)).text_color(rgba(t.text)))
                })
                .child(
                    div()
                        .w(rem(12.0))
                        .flex_none()
                        .text_size(rem(11.0))
                        .text_color(if is_selected {
                            rgba(t.text)
                        } else {
                            rgba(t.text_muted)
                        })
                        .child(if idx == 0 { "" } else { "›" }),
                )
                .child(
                    div()
                        .text_size(rem(13.0))
                        .line_clamp(1)
                        .child(SharedString::from(*name)),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_settings_category(idx, cx);
                }))
        }))
}

/// Right Content Area:
/// - Category Header
/// - Setting rows in exact VS Code style
fn render_settings_content(
    settings: &Settings,
    t: &Colors,
    active_theme_ix: usize,
    font_size: f32,
    active_category: usize,
    search_query: &str,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let query_lower = search_query.trim().to_lowercase();
    let has_query = !query_lower.is_empty();

    let category_name = SETTINGS_CATEGORIES
        .get(active_category)
        .copied()
        .unwrap_or("Commonly Used");

    let header_title = if has_query {
        format!("Search: \"{search_query}\"")
    } else {
        category_name.to_string()
    };

    let ui_font_size = settings.ui_font_size;
    let auto_save = settings.editor_auto_save;
    let auto_save_delay = settings.editor_auto_save_delay;
    let tab_size = settings.editor_tab_size;
    let format_on_save = settings.editor_format_on_save;
    let themes = theme::all();
    let current_theme = themes.get(active_theme_ix);
    let current_theme_name = current_theme
        .map(|th| th.name.as_str())
        .unwrap_or("GitHub Dark");

    let content = div()
        .id("settings-content-scroll")
        .flex_1()
        .w_full()
        .min_h(px(0.0))
        .h_full()
        .overflow_y_scrollbar()
        .px(rem(32.0))
        .py(rem(20.0))
        .child(
            div()
                .max_w(rem(780.0))
                .flex()
                .flex_col()
                .gap(rem(16.0))
                .child(
                    div()
                        .pb(rem(8.0))
                        .border_b_1()
                        .border_color(rgba(t.border_variant))
                        .child(
                            div()
                                .text_size(rem(22.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgba(t.text))
                                .child(header_title),
                        ),
                ),
        );

    // Filter helper: checks if a setting matches category or query
    let should_show = |cats: &[usize], title: &str, desc: &str| -> bool {
        if has_query {
            title.to_lowercase().contains(&query_lower)
                || desc.to_lowercase().contains(&query_lower)
        } else if active_category == 0 {
            true // Commonly Used shows all primary settings
        } else {
            cats.contains(&active_category)
        }
    };

    let mut rows = div().flex().flex_col().gap(rem(4.0));

    // 1. Editor: Font Size
    if should_show(
        &[0, 1],
        "Editor: Font Size",
        "Controls the font size in pixels.",
    ) {
        rows = rows.child(vscode_setting_row(
            "Editor: Font Size",
            None,
            "Controls the font size in pixels.",
            vscode_number_input(
                "input-editor-font-size",
                format!("{font_size:.1}"),
                t,
                cx.listener(|this, _, _, cx| this.decrease_font_size(cx)),
                cx.listener(|this, _, _, cx| this.increase_font_size(cx)),
                Some(cx.listener(|this, _, _, cx| this.reset_font_size(cx))),
            ),
            (font_size - 14.5).abs() > 0.01,
            t,
        ));
    }

    // 2. Editor: Font Family
    if should_show(&[0, 1], "Editor: Font Family", "Controls the font family.") {
        rows = rows.child(vscode_setting_row(
            "Editor: Font Family",
            None,
            "Controls the font family.",
            div()
                .w(rem(300.0))
                .h(rem(28.0))
                .px(rem(8.0))
                .rounded(px(2.0))
                .bg(rgba(t.element_bg))
                .border_1()
                .border_color(rgba(t.border))
                .flex()
                .items_center()
                .child(
                    div()
                        .text_size(rem(13.0))
                        .text_color(rgba(t.text))
                        .child("Lilex (bundled monospace)"),
                ),
            false,
            t,
        ));
    }

    // 3. Editor: Format On Save
    if should_show(
        &[0, 1],
        "Editor: Format On Save",
        "Format a file on save. A formatter must be available and the editor must not be formatted when saved explicitly.",
    ) {
        let is_on = format_on_save == FormatOnSaveMode::On;
        rows = rows.child(vscode_setting_row(
            "Editor: Format On Save",
            if is_on { Some("(Modified elsewhere)") } else { None },
            "Format a file on save. A formatter must be available and the editor must not be formatted when saved explicitly.",
            vscode_checkbox(
                "chk-format-on-save",
                is_on,
                "Format a file on save.",
                t,
                cx.listener(move |this, _, _, cx| {
                    this.settings.editor_format_on_save = if is_on {
                        FormatOnSaveMode::Off
                    } else {
                        FormatOnSaveMode::On
                    };
                    let _ = this.settings.save();
                    cx.notify();
                }),
            ),
            is_on,
            t,
        ));
    }

    // 4. Files: Auto Save
    if should_show(
        &[0, 5],
        "Files: Auto Save",
        "Controls auto save of editors that have unsaved changes.",
    ) {
        let is_modified = auto_save != AutoSaveMode::Off;
        let mode_str = match auto_save {
            AutoSaveMode::Off => "off",
            AutoSaveMode::AfterDelay => "afterDelay",
            AutoSaveMode::OnFocusChange => "onFocusChange",
        };
        rows = rows.child(vscode_setting_row(
            "Files: Auto Save",
            if is_modified {
                Some("(Modified elsewhere)")
            } else {
                None
            },
            "Controls auto save of editors that have unsaved changes.",
            vscode_dropdown(
                "dropdown-auto-save",
                mode_str.to_string(),
                t,
                cx.listener(move |this, _, _, cx| {
                    this.settings.editor_auto_save = match this.settings.editor_auto_save {
                        AutoSaveMode::Off => AutoSaveMode::AfterDelay,
                        AutoSaveMode::AfterDelay => AutoSaveMode::OnFocusChange,
                        AutoSaveMode::OnFocusChange => AutoSaveMode::Off,
                    };
                    let _ = this.settings.save();
                    cx.notify();
                }),
            ),
            is_modified,
            t,
        ));
    }

    // 5. Files: Auto Save Delay
    if should_show(
        &[0, 5],
        "Files: Auto Save Delay",
        "Controls the delay in milliseconds after which a dirty file is saved automatically.",
    ) && auto_save == AutoSaveMode::AfterDelay
    {
        let delay_str = format!("{auto_save_delay} ms");
        rows = rows.child(vscode_setting_row(
            "Files: Auto Save Delay",
            None,
            "Controls the delay in milliseconds after which a dirty file is saved automatically.",
            vscode_dropdown(
                "dropdown-auto-save-delay",
                delay_str,
                t,
                cx.listener(move |this, _, _, cx| {
                    this.settings.editor_auto_save_delay =
                        match this.settings.editor_auto_save_delay {
                            500 => 1000,
                            1000 => 2000,
                            2000 => 5000,
                            _ => 500,
                        };
                    let _ = this.settings.save();
                    cx.notify();
                }),
            ),
            auto_save_delay != 1000,
            t,
        ));
    }

    // 6. Editor: Tab Size
    if should_show(
        &[0, 1],
        "Editor: Tab Size",
        "The number of spaces a tab is equal to in code files.",
    ) {
        let tab_str = format!("{tab_size} spaces");
        rows = rows.child(vscode_setting_row(
            "Editor: Tab Size",
            None,
            "The number of spaces a tab is equal to in code files.",
            vscode_dropdown(
                "dropdown-tab-size",
                tab_str,
                t,
                cx.listener(move |this, _, _, cx| {
                    this.settings.editor_tab_size = match this.settings.editor_tab_size {
                        2 => 4,
                        4 => 8,
                        _ => 2,
                    };
                    let _ = this.settings.save();
                    cx.notify();
                }),
            ),
            tab_size != 4,
            t,
        ));
    }

    // 7. Workbench: Color Theme
    if should_show(
        &[0, 2],
        "Workbench: Color Theme",
        "Specifies the color theme used in the workbench.",
    ) {
        rows = rows.child(vscode_setting_row(
            "Workbench: Color Theme",
            None,
            "Specifies the color theme used in the workbench.",
            div()
                .flex()
                .flex_col()
                .gap(rem(8.0))
                .child(vscode_dropdown(
                    "dropdown-color-theme",
                    current_theme_name.to_string(),
                    t,
                    cx.listener(move |this, _, window, cx| {
                        let total = theme::all().len();
                        let next_ix = (this.theme_ix + 1) % total;
                        this.apply_theme(next_ix, window, cx);
                    }),
                ))
                .child(div().flex().flex_wrap().gap(rem(8.0)).children(
                    themes.iter().enumerate().map(|(idx, th)| {
                        let is_act = idx == active_theme_ix;
                        let name = th.name.clone();
                        div()
                            .id(SharedString::from(format!("theme-chip-{idx}")))
                            .px(rem(8.0))
                            .py(rem(3.0))
                            .rounded(px(3.0))
                            .cursor_pointer()
                            .border_1()
                            .border_color(if is_act {
                                rgba(t.border_focused)
                            } else {
                                rgba(t.border)
                            })
                            .bg(if is_act {
                                rgba(t.element_selected)
                            } else {
                                rgba(t.element_bg)
                            })
                            .text_size(rem(11.5))
                            .text_color(if is_act {
                                rgba(t.text)
                            } else {
                                rgba(t.text_muted)
                            })
                            .child(SharedString::from(name))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.apply_theme(idx, window, cx);
                            }))
                    }),
                )),
            current_theme_name != "GitHub Dark",
            t,
        ));
    }

    // 8. Window: UI Font Size / Zoom
    if should_show(
        &[0, 2, 3],
        "Window: Zoom / UI Font Size",
        "Controls the UI font size in pixels (Zed ui_font_size: scales the whole interface).",
    ) {
        rows = rows.child(vscode_setting_row(
            "Window: Zoom / UI Font Size",
            None,
            "Controls the UI font size in pixels (Zed ui_font_size: scales the whole interface).",
            vscode_number_input(
                "input-ui-font-size",
                format!("{ui_font_size:.1} px"),
                t,
                cx.listener(|this, _, _, cx| this.decrease_ui_font_size(cx)),
                cx.listener(|this, _, _, cx| this.increase_ui_font_size(cx)),
                Some(cx.listener(|this, _, _, cx| this.reset_ui_font_size(cx))),
            ),
            (ui_font_size - 14.0).abs() > 0.01,
            t,
        ));
    }

    // 9. Editor: Semantic Highlighting
    if should_show(
        &[1, 4],
        "Editor: Semantic Syntax Highlighting",
        "Tree-Sitter incremental syntax parsing and exact Zed theme tokens.",
    ) {
        rows = rows.child(vscode_setting_row(
            "Editor: Semantic Syntax Highlighting",
            None,
            "Tree-Sitter incremental syntax parsing and exact Zed theme tokens.",
            div()
                .px(rem(8.0))
                .py(rem(3.0))
                .rounded(px(3.0))
                .bg(rgba(t.border_focused))
                .text_size(rem(11.5))
                .font_weight(FontWeight::BOLD)
                .text_color(rgba(t.background))
                .child("✓ Enabled"),
            false,
            t,
        ));
    }

    // 10. Terminal: Default Shell Profile
    if should_show(
        &[4],
        "Terminal: Default Shell Profile",
        "The shell process launched when spawning new terminal tabs.",
    ) {
        let shell_desc = if cfg!(windows) {
            "PowerShell (Windows PTY)"
        } else {
            "Zsh / Bash (Unix PTY)"
        };
        rows = rows.child(vscode_setting_row(
            "Terminal: Default Shell Profile",
            None,
            "The shell process launched when spawning new terminal tabs.",
            div()
                .w(rem(260.0))
                .h(rem(28.0))
                .px(rem(8.0))
                .rounded(px(2.0))
                .bg(rgba(t.element_bg))
                .border_1()
                .border_color(rgba(t.border))
                .flex()
                .items_center()
                .child(
                    div()
                        .text_size(rem(12.5))
                        .text_color(rgba(t.text))
                        .child(shell_desc),
                ),
            false,
            t,
        ));
    }

    // 11. Files: Auto Watcher
    if should_show(
        &[5],
        "Files: Auto Watcher & Debouncing",
        "Monitors external directory modifications and updates the explorer tree automatically.",
    ) {
        rows = rows.child(vscode_setting_row(
            "Files: Auto Watcher & Debouncing",
            None,
            "Monitors external directory modifications and updates the explorer tree automatically.",
            div()
                .px(rem(8.0))
                .py(rem(3.0))
                .rounded(px(3.0))
                .bg(rgba(t.element_bg))
                .border_1()
                .border_color(rgba(t.border))
                .text_size(rem(11.5))
                .text_color(rgba(t.text_muted))
                .child("Active · 150ms debounce"),
            false,
            t,
        ));
    }

    // 12. Security: Workspace Trust
    if should_show(
        &[6],
        "Security: Workspace Trust",
        "Controls whether language servers and tools are allowed to execute in this workspace.",
    ) {
        rows = rows.child(vscode_setting_row(
            "Security: Workspace Trust",
            None,
            "Controls whether language servers and tools are allowed to execute in this workspace.",
            div()
                .px(rem(8.0))
                .py(rem(3.0))
                .rounded(px(3.0))
                .bg(rgba(t.border_focused))
                .text_size(rem(11.5))
                .font_weight(FontWeight::BOLD)
                .text_color(rgba(t.background))
                .child("✓ Trusted Workspace"),
            false,
            t,
        ));
    }

    // 13. Extensions: Language Servers
    if should_show(
        &[7],
        "Extensions: Language Server Protocol (LSP)",
        "Automatic sandboxed language server provisioning for TypeScript, CSS, HTML, JSON, and toolchain discovery.",
    ) {
        rows = rows.child(vscode_setting_row(
            "Extensions: Language Server Protocol (LSP)",
            None,
            "Automatic sandboxed language server provisioning for TypeScript, CSS, HTML, JSON, and toolchain discovery.",
            div()
                .px(rem(8.0))
                .py(rem(3.0))
                .rounded(px(3.0))
                .bg(rgba(t.element_bg))
                .border_1()
                .border_color(rgba(t.border))
                .text_size(rem(11.5))
                .text_color(rgba(t.text_accent))
                .child("Auto-Provisioned (Sandboxed Node)"),
            false,
            t,
        ));
    }

    content.child(div().max_w(rem(780.0)).child(rows))
}

/// A setting row matching VS Code:
/// - Bold title with optional tag
/// - Description in muted gray
/// - Modified indicator line on the left edge (amber/blue bar)
/// - Control below description
fn vscode_setting_row(
    title: &'static str,
    subtitle_tag: Option<&'static str>,
    desc: &'static str,
    control: impl IntoElement,
    is_modified: bool,
    t: &Colors,
) -> impl IntoElement {
    div()
        .w_full()
        .relative()
        .pl(rem(12.0))
        .py(rem(10.0))
        .border_b_1()
        .border_color(rgba(t.border_variant))
        .flex()
        .flex_col()
        .gap(rem(4.0))
        .when(is_modified, |d| {
            d.child(
                div()
                    .absolute()
                    .left_0()
                    .top(rem(10.0))
                    .bottom(rem(10.0))
                    .w(px(3.0))
                    .rounded(px(1.0))
                    .bg(rgba(0xe5a842ff)), // Amber/orange modified bar matching VS Code screenshot
            )
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(rem(6.0))
                .child(
                    div()
                        .text_size(rem(13.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgba(t.text))
                        .child(SharedString::from(title)),
                )
                .when_some(subtitle_tag, |parent, tag| {
                    parent.child(
                        div()
                            .text_size(rem(12.0))
                            .text_color(rgba(t.text_muted))
                            .italic()
                            .child(SharedString::from(tag)),
                    )
                }),
        )
        .child(
            div()
                .text_size(rem(12.5))
                .text_color(rgba(t.text_muted))
                .child(SharedString::from(desc)),
        )
        .child(div().pt(rem(4.0)).child(control))
}

/// VS Code style dropdown select box: dark box with chevron-down on right
fn vscode_dropdown(
    id: &'static str,
    value: String,
    t: &Colors,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .w(rem(260.0))
        .h(rem(28.0))
        .px(rem(8.0))
        .rounded(px(2.0))
        .bg(rgba(t.element_bg))
        .border_1()
        .border_color(rgba(t.border))
        .hover(|s| s.border_color(rgba(t.border_focused)))
        .flex()
        .items_center()
        .justify_between()
        .cursor_pointer()
        .child(
            div()
                .text_size(rem(13.0))
                .text_color(rgba(t.text))
                .child(SharedString::from(value)),
        )
        .child(
            svg()
                .path("ui_icons/chevron-down_tint.svg")
                .w(rem(12.0))
                .h(rem(12.0))
                .text_color(rgba(t.icon_muted)),
        )
        .on_click(on_click)
}

/// VS Code style number/stepper input: dark box with value + [-] [+] [Reset] buttons
fn vscode_number_input(
    id: &'static str,
    value: String,
    t: &Colors,
    on_dec: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    on_inc: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    on_reset: Option<impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static>,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(rem(8.0))
        .child(
            div()
                .id(id)
                .w(rem(180.0))
                .h(rem(28.0))
                .px(rem(8.0))
                .rounded(px(2.0))
                .bg(rgba(t.element_bg))
                .border_1()
                .border_color(rgba(t.border))
                .flex()
                .items_center()
                .child(
                    div()
                        .text_size(rem(13.0))
                        .text_color(rgba(t.text))
                        .child(SharedString::from(value)),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(rem(4.0))
                .child(btn_small("-", t, on_dec))
                .child(btn_small("+", t, on_inc))
                .when_some(on_reset, |parent, reset| {
                    parent.child(btn_small("Reset", t, reset))
                }),
        )
}

/// VS Code style checkbox: square box with checkmark + label
fn vscode_checkbox(
    id: &'static str,
    checked: bool,
    label: &'static str,
    t: &Colors,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(rem(8.0))
        .cursor_pointer()
        .child(
            div()
                .size(rem(16.0))
                .rounded(px(2.0))
                .border_1()
                .border_color(if checked {
                    rgba(t.text_accent)
                } else {
                    rgba(t.border)
                })
                .bg(if checked {
                    rgba(t.text_accent)
                } else {
                    rgba(t.element_bg)
                })
                .flex()
                .items_center()
                .justify_center()
                .child(if checked {
                    div()
                        .text_size(rem(11.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgba(t.background))
                        .child("✓")
                } else {
                    div().child("")
                }),
        )
        .child(
            div()
                .text_size(rem(12.5))
                .text_color(rgba(t.text))
                .child(SharedString::from(label)),
        )
        .on_click(on_click)
}

fn btn_small(
    label: &'static str,
    t: &Colors,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("btn-setting-{label}")))
        .px(rem(8.0))
        .py(rem(3.0))
        .rounded(px(2.0))
        .bg(rgba(t.element_bg))
        .border_1()
        .border_color(rgba(t.border))
        .hover(|s| {
            s.bg(rgba(t.element_hover))
                .border_color(rgba(t.border_focused))
        })
        .cursor_pointer()
        .text_size(rem(12.0))
        .text_color(rgba(t.text))
        .child(SharedString::from(label))
        .on_click(on_click)
}
