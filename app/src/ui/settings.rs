use gpui::{
    div, prelude::*, px, rgba, svg, Context, FontWeight, IntoElement, SharedString, Window,
};
use gpui_component::scroll::ScrollableElement;

use crate::settings::{AutoSaveMode, Settings};
use crate::theme::{self, Colors};
use crate::workspace::Workspace;

pub(crate) fn render_settings(
    settings: &Settings,
    t: &Colors,
    active_theme_ix: usize,
    font_size: f32,
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

        .child(render_header(t, cx))

        .child(
            div()
                .id("settings-scroll")
                .flex_1()
                .w_full()
                .min_h(px(0.0))
                .overflow_y_scrollbar()
                .px(px(32.0))
                .py(px(24.0))
                .child(
                    div()
                        .max_w(px(860.0))
                        .flex()
                        .flex_col()
                        .gap(px(28.0))

                        .child(render_theme_section(t, active_theme_ix, cx))

                        .child(render_editor_section(settings, t, font_size, cx))

                        .child(render_terminal_section(t, cx))

                        .child(render_system_section(t)),
                ),
        )
}

fn render_header(t: &Colors, cx: &mut Context<Workspace>) -> impl IntoElement {
    let settings_path_display = crate::settings::settings_file_path().to_string_lossy().into_owned();
    div()
        .w_full()
        .px(px(32.0))
        .py(px(18.0))
        .bg(rgba(t.surface))
        .border_b_1()
        .border_color(rgba(t.border_variant))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(
                            svg()
                                .path("ui_icons/settings-gear_tint.svg")
                                .w(px(22.0))
                                .h(px(22.0))
                                .text_color(rgba(t.text_accent)),
                        )
                        .child(
                            div()
                                .text_size(px(20.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgba(t.text))
                                .child(SharedString::from("Settings")),
                        ),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgba(t.text_muted))
                        .child(SharedString::from(format!("Stored in {settings_path_display}"))),
                ),
        )
        .child(
            div()
                .id("open-settings-json-btn")
                .flex()
                .items_center()
                .gap(px(6.0))
                .px(px(12.0))
                .py(px(6.0))
                .rounded(px(4.0))
                .bg(rgba(t.element_bg))
                .border_1()
                .border_color(rgba(t.border))
                .cursor_pointer()
                .hover(|s| s.bg(rgba(t.element_hover)).border_color(rgba(t.border_focused)))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_settings_json(window, cx);
                }))
                .child(
                    svg()
                        .path("ui_icons/file-code_tint.svg")
                        .w(px(14.0))
                        .h(px(14.0))
                        .text_color(rgba(t.text_accent)),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgba(t.text))
                        .child(SharedString::from("Open Settings (JSON)")),
                ),
        )
}

fn render_theme_section(
    t: &Colors,
    active_theme_ix: usize,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let themes = theme::all();
    let current_theme = themes.get(active_theme_ix);
    let current_name = current_theme.map(|th| th.name.as_str()).unwrap_or("Default");
    let current_app = current_theme
        .map(|th| th.appearance.as_str())
        .unwrap_or("dark");

    div()
        .flex()
        .flex_col()
        .gap(px(14.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(16.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgba(t.text))
                                .child(SharedString::from("🎨 Color Theme")),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(rgba(t.text_muted))
                                .child(SharedString::from(
                                    "Select the workbench color theme. Theme and syntax tokens apply instantly.",
                                )),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(10.0))
                        .py(px(4.0))
                        .rounded(px(6.0))
                        .bg(rgba(t.element_bg))
                        .border_1()
                        .border_color(rgba(t.border))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(rgba(t.text_muted))
                                .child(SharedString::from("Current:")),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgba(t.text_accent))
                                .child(SharedString::from(current_name.to_string())),
                        )
                        .child(
                            div()
                                .px(px(5.0))
                                .py(px(1.0))
                                .rounded(px(3.0))
                                .bg(rgba(t.element_active))
                                .text_size(px(10.5))
                                .text_color(rgba(t.text))
                                .child(SharedString::from(current_app.to_uppercase())),
                        ),
                ),
        )

        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(10.0))
                .children(themes.iter().enumerate().map(|(idx, th)| {
                    let is_active = idx == active_theme_ix;
                    let th_name = th.name.clone();
                    let th_app = th.appearance.clone();
                    let th_bg = th.colors.background;
                    let th_panel = th.colors.panel;
                    let th_accent = th.colors.text_accent;
                    let th_text = th.colors.text;

                    div()
                        .id(SharedString::from(format!("theme-card-{idx}")))
                        .w(px(260.0))
                        .p(px(12.0))
                        .rounded(px(8.0))
                        .cursor_pointer()
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .when(is_active, |d| {
                            d.bg(rgba(t.element_selected))
                                .border_2()
                                .border_color(rgba(t.text_accent))
                        })
                        .when(!is_active, |d| {
                            d.bg(rgba(t.surface))
                                .border_1()
                                .border_color(rgba(t.border))
                                .hover(|s| {
                                    s.bg(rgba(t.element_hover))
                                        .border_color(rgba(t.border_focused))
                                })
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.apply_theme(idx, window, cx);
                        }))

                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(13.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgba(t.text))
                                        .child(SharedString::from(th_name)),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(4.0))
                                        .when(is_active, |d| {
                                            d.child(
                                                div()
                                                    .px(px(6.0))
                                                    .py(px(2.0))
                                                    .rounded(px(4.0))
                                                    .bg(rgba(t.border_focused))
                                                    .text_size(px(10.5))
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(rgba(t.background))
                                                    .child(SharedString::from("✓ Active")),
                                            )
                                        })
                                        .when(!is_active, |d| {
                                            d.child(
                                                div()
                                                    .px(px(5.0))
                                                    .py(px(1.0))
                                                    .rounded(px(3.0))
                                                    .bg(rgba(t.element_active))
                                                    .text_size(px(10.0))
                                                    .text_color(rgba(t.text_muted))
                                                    .child(SharedString::from(th_app.to_uppercase())),
                                            )
                                        }),
                                ),
                        )

                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(color_swatch(th_bg, "Background", t))
                                .child(color_swatch(th_panel, "Panel", t))
                                .child(color_swatch(th_accent, "Accent", t))
                                .child(color_swatch(th_text, "Text", t))
                                .child(
                                    div()
                                        .ml_auto()
                                        .text_size(px(11.0))
                                        .text_color(rgba(t.text_muted))
                                        .child(SharedString::from("Preview")),
                                ),
                        )
                })),
        )
}

fn color_swatch(color_hex: u32, _label: &'static str, t: &Colors) -> impl IntoElement {
    div()
        .w(px(18.0))
        .h(px(18.0))
        .rounded_full()
        .bg(rgba(color_hex))
        .border_1()
        .border_color(rgba(t.border_variant))
}

/// Section 2: Text Editor Settings
fn render_editor_section(
    settings: &Settings,
    t: &Colors,
    font_size: f32,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let auto_save = settings.editor_auto_save;
    let auto_save_delay = settings.editor_auto_save_delay;
    let tab_size = settings.editor_tab_size;

    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(16.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgba(t.text))
                        .child(SharedString::from("📝 Text Editor & Auto Save")),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgba(t.text_muted))
                        .child(SharedString::from(
                            "Auto-save triggers, font configuration, indentation and editor preferences",
                        )),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                // Auto Save Mode
                .child(
                    setting_row(
                        "Files: Auto Save",
                        auto_save.description(),
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                btn_pill(
                                    "autosave-off",
                                    "off",
                                    auto_save == AutoSaveMode::Off,
                                    t,
                                    cx.listener(|this, _, _, cx| {
                                        this.settings.editor_auto_save = AutoSaveMode::Off;
                                        let _ = this.settings.save();
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(
                                btn_pill(
                                    "autosave-after-delay",
                                    "afterDelay",
                                    auto_save == AutoSaveMode::AfterDelay,
                                    t,
                                    cx.listener(|this, _, _, cx| {
                                        this.settings.editor_auto_save = AutoSaveMode::AfterDelay;
                                        let _ = this.settings.save();
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(
                                btn_pill(
                                    "autosave-on-focus-change",
                                    "onFocusChange",
                                    auto_save == AutoSaveMode::OnFocusChange,
                                    t,
                                    cx.listener(|this, _, _, cx| {
                                        this.settings.editor_auto_save = AutoSaveMode::OnFocusChange;
                                        let _ = this.settings.save();
                                        cx.notify();
                                    }),
                                ),
                            ),
                        t,
                    ),
                )
                // Auto Save Delay (only visible/active when afterDelay is selected)
                .when(auto_save == AutoSaveMode::AfterDelay, |d| {
                    d.child(
                        setting_row(
                            "Files: Auto Save Delay",
                            "Controls the delay in milliseconds after which a dirty file is saved automatically",
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    btn_pill(
                                        "delay-500",
                                        "500ms",
                                        auto_save_delay == 500,
                                        t,
                                        cx.listener(|this, _, _, cx| {
                                            this.settings.editor_auto_save_delay = 500;
                                            let _ = this.settings.save();
                                            cx.notify();
                                        }),
                                    ),
                                )
                                .child(
                                    btn_pill(
                                        "delay-1000",
                                        "1000ms (1s)",
                                        auto_save_delay == 1000,
                                        t,
                                        cx.listener(|this, _, _, cx| {
                                            this.settings.editor_auto_save_delay = 1000;
                                            let _ = this.settings.save();
                                            cx.notify();
                                        }),
                                    ),
                                )
                                .child(
                                    btn_pill(
                                        "delay-2000",
                                        "2000ms (2s)",
                                        auto_save_delay == 2000,
                                        t,
                                        cx.listener(|this, _, _, cx| {
                                            this.settings.editor_auto_save_delay = 2000;
                                            let _ = this.settings.save();
                                            cx.notify();
                                        }),
                                    ),
                                )
                                .child(
                                    btn_pill(
                                        "delay-5000",
                                        "5000ms (5s)",
                                        auto_save_delay == 5000,
                                        t,
                                        cx.listener(|this, _, _, cx| {
                                            this.settings.editor_auto_save_delay = 5000;
                                            let _ = this.settings.save();
                                            cx.notify();
                                        }),
                                    ),
                                ),
                            t,
                        ),
                    )
                })
                // Font size item
                .child(
                    setting_row(
                        "Editor: Font Size",
                        "Controls the font size in pixels for the editor buffer (Zoom: Ctrl++ / Ctrl+-)",
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                btn_small("-", t, cx.listener(|this, _, _, cx| {
                                    this.decrease_font_size(cx);
                                })),
                            )
                            .child(
                                div()
                                    .min_w(px(55.0))
                                    .text_center()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgba(t.text))
                                    .child(SharedString::from(format!("{font_size:.1} px"))),
                            )
                            .child(
                                btn_small("+", t, cx.listener(|this, _, _, cx| {
                                    this.increase_font_size(cx);
                                })),
                            )
                            .child(
                                btn_small("Reset (14.5px)", t, cx.listener(|this, _, _, cx| {
                                    this.reset_font_size(cx);
                                })),
                            ),
                        t,
                    ),
                )
                // Tab size item
                .child(
                    setting_row(
                        "Editor: Tab Size",
                        "The number of spaces a tab is equal to in code files",
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                btn_pill(
                                    "tab-size-2",
                                    "2 spaces",
                                    tab_size == 2,
                                    t,
                                    cx.listener(|this, _, _, cx| {
                                        this.settings.editor_tab_size = 2;
                                        let _ = this.settings.save();
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(
                                btn_pill(
                                    "tab-size-4",
                                    "4 spaces",
                                    tab_size == 4,
                                    t,
                                    cx.listener(|this, _, _, cx| {
                                        this.settings.editor_tab_size = 4;
                                        let _ = this.settings.save();
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(
                                btn_pill(
                                    "tab-size-8",
                                    "8 spaces",
                                    tab_size == 8,
                                    t,
                                    cx.listener(|this, _, _, cx| {
                                        this.settings.editor_tab_size = 8;
                                        let _ = this.settings.save();
                                        cx.notify();
                                    }),
                                ),
                            ),
                        t,
                    ),
                )
                // Syntax Highlighting item
                .child(
                    setting_row(
                        "Editor: Semantic Syntax Highlighting",
                        "Tree-Sitter incremental syntax parsing and exact Zed theme tokens",
                        div()
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded(px(4.0))
                            .bg(rgba(t.border_focused))
                            .text_size(px(11.5))
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgba(t.background))
                            .child(SharedString::from("Enabled")),
                        t,
                    ),
                ),
        )
}

/// Section 3: Terminal settings
fn render_terminal_section(t: &Colors, cx: &mut Context<Workspace>) -> impl IntoElement {
    let shell_label = if cfg!(windows) {
        "PowerShell (Windows PTY)"
    } else {
        "Default Shell ($SHELL / PTY)"
    };

    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(16.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgba(t.text))
                        .child(SharedString::from("⚡ Integrated Terminal")),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgba(t.text_muted))
                        .child(SharedString::from(
                            "Embedded terminal shell emulation and execution environment",
                        )),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    setting_row(
                        "Terminal: Default Shell Profile",
                        "The shell process launched when spawning new terminal tabs",
                        div()
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(rgba(t.element_bg))
                            .border_1()
                            .border_color(rgba(t.border))
                            .text_size(px(12.5))
                            .text_color(rgba(t.text))
                            .child(SharedString::from(shell_label)),
                        t,
                    ),
                )
                .child(
                    setting_row(
                        "Terminal: Quick Actions",
                        "Create new shells or toggle visibility of the bottom terminal panel",
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                btn_small("New Terminal (Ctrl+Shift+`)", t, cx.listener(|this, _, window, cx| {
                                    this.new_terminal(window, cx);
                                })),
                            )
                            .child(
                                btn_small("Toggle Panel (Ctrl+J)", t, cx.listener(|this, _, window, cx| {
                                    this.toggle_terminal(window, cx);
                                })),
                            ),
                        t,
                    ),
                ),
        )
}

/// Section 4: System & Workspace
fn render_system_section(t: &Colors) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(16.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgba(t.text))
                        .child(SharedString::from("📁 Files & System")),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgba(t.text_muted))
                        .child(SharedString::from(
                            "File system watchers, buffers and background tasks",
                        )),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(
                    setting_row(
                        "Files: Auto Watcher & Debouncing",
                        "Monitors external directory modifications and updates the explorer tree automatically",
                        div()
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded(px(4.0))
                            .bg(rgba(t.element_bg))
                            .border_1()
                            .border_color(rgba(t.border))
                            .text_size(px(11.5))
                            .text_color(rgba(t.text_muted))
                            .child(SharedString::from("Active · 150ms debounce")),
                        t,
                    ),
                )
                .child(
                    setting_row(
                        "Files: Max Buffer Limit",
                        "Large file safety guard to prevent out-of-memory lockups",
                        div()
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(px(4.0))
                            .bg(rgba(t.element_bg))
                            .border_1()
                            .border_color(rgba(t.border))
                            .text_size(px(12.5))
                            .text_color(rgba(t.text))
                            .child(SharedString::from("8 MB Limit")),
                        t,
                    ),
                ),
        )
}

/// Generic setting row
fn setting_row(
    title: &'static str,
    desc: &'static str,
    control: impl IntoElement,
    t: &Colors,
) -> impl IntoElement {
    div()
        .p(px(14.0))
        .rounded(px(6.0))
        .bg(rgba(t.surface))
        .border_1()
        .border_color(rgba(t.border))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgba(t.text))
                        .child(SharedString::from(title)),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgba(t.text_muted))
                        .child(SharedString::from(desc)),
                ),
        )
        .child(control)
}

fn btn_small(
    label: &'static str,
    t: &Colors,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("btn-setting-{label}")))
        .px(px(10.0))
        .py(px(4.0))
        .rounded(px(4.0))
        .bg(rgba(t.element_bg))
        .border_1()
        .border_color(rgba(t.border))
        .hover(|s| s.bg(rgba(t.element_hover)).border_color(rgba(t.border_focused)))
        .cursor_pointer()
        .text_size(px(12.0))
        .text_color(rgba(t.text))
        .child(SharedString::from(label))
        .on_click(on_click)
}

fn btn_pill(
    id: &'static str,
    label: &'static str,
    is_active: bool,
    t: &Colors,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px(px(10.0))
        .py(px(4.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .text_size(px(12.0))
        .font_weight(if is_active {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        })
        .when(is_active, |d| {
            d.bg(rgba(t.border_focused))
                .text_color(rgba(t.background))
                .border_1()
                .border_color(rgba(t.border_focused))
        })
        .when(!is_active, |d| {
            d.bg(rgba(t.element_bg))
                .text_color(rgba(t.text))
                .border_1()
                .border_color(rgba(t.border))
                .hover(|s| s.bg(rgba(t.element_hover)).border_color(rgba(t.border_focused)))
        })
        .child(SharedString::from(label))
        .on_click(on_click)
}
