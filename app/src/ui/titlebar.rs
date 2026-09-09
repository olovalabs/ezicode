use gpui::{div, prelude::*, px, rgba, Context, IntoElement, SharedString, Window};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    input::{Copy, Cut, Paste, Redo, SelectAll, Undo},
    menu::{DropdownMenu as _, PopupMenu},
    TitleBar,
};

use crate::actions::*;
use crate::theme::{self, Colors};
use crate::ui::app_icon;

pub(crate) fn render_titlebar(title: &str, t: &Colors, theme_ix: usize) -> impl IntoElement {
    TitleBar::new()
        .bg(rgba(t.title_bar))
        .border_color(rgba(t.border))
        .child(
            div()
                .w_full()
                .h_full()
                .flex()
                .flex_row()
                .items_center()
                .text_size(px(13.5))
                .child(

                    div()
                        .w(px(28.0))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(app_icon::render_app_icon(20.0, t)),
                )
                .child(menu_btn("m-file", "File", t, |menu, _, _| {
                    menu.menu("New File (Ctrl+N)", Box::new(NewFile))
                        .separator()
                        .menu("Open File… (Ctrl+O)", Box::new(OpenFile))
                        .menu("Open Folder…", Box::new(OpenFolder))
                        .menu("Quick Open File… (Ctrl+P)", Box::new(ToggleFileFinder))
                        .separator()
                        .menu("Save (Ctrl+S)", Box::new(Save))
                        .separator()
                        .menu("Preferences: Settings (Ctrl+,)", Box::new(OpenSettings))
                        .separator()
                        .menu("Exit", Box::new(Quit))
                }))
                .child(menu_btn("m-edit", "Edit", t, |menu, _, _| {
                    menu.menu("Undo", Box::new(Undo))
                        .menu("Redo", Box::new(Redo))
                        .separator()
                        .menu("Cut", Box::new(Cut))
                        .menu("Copy", Box::new(Copy))
                        .menu("Paste", Box::new(Paste))
                        .separator()
                        .menu("Select All", Box::new(SelectAll))
                        .separator()
                        .menu("Copy Problem / Error (Ctrl+Alt+C)", Box::new(CopyDiagnostic))
                }))
                .child(menu_btn("m-view", "View", t, move |menu, window, cx| {
                    menu.menu("Command Palette… (Ctrl+Shift+P)", Box::new(ToggleCommandPalette))
                        .menu("Go to Line… (Ctrl+G)", Box::new(ToggleGoToLine))
                        .separator()
                        .menu("Explorer (Ctrl+Shift+E)", Box::new(ShowExplorer))
                        .menu("Search (Ctrl+Shift+F)", Box::new(ShowSearch))
                        .menu("Source Control (Ctrl+Shift+G)", Box::new(ShowGit))
                        .menu("Extensions (Ctrl+Shift+X)", Box::new(ShowExtensions))
                        .separator()
                        .submenu("Theme", window, cx, move |menu, _, _| {
                            let mut m = menu;
                            for (i, th) in theme::all().iter().enumerate() {
                                m = m.menu_with_check(
                                    th.name.clone(),
                                    i == theme_ix,
                                    Box::new(SelectTheme { ix: i }),
                                );
                            }
                            m
                        })
                        .separator()
                        .menu("Toggle Primary Side Bar", Box::new(ToggleSidebar))
                        .menu("Toggle Terminal", Box::new(ToggleTerminal))
                        .separator()
                        .menu("Zoom In (Ctrl++)", Box::new(IncreaseFontSize))
                        .menu("Zoom Out (Ctrl+-)", Box::new(DecreaseFontSize))
                        .menu("Reset Zoom (Ctrl+0)", Box::new(ResetFontSize))
                }))
                .child(menu_btn("m-term", "Terminal", t, |menu, _, _| {
                    menu.menu("New Terminal (Ctrl+Shift+`)", Box::new(NewTerminal))
                        .menu("Toggle Terminal (Ctrl+J)", Box::new(ToggleTerminal))
                }))
                .child(menu_btn("m-help", "Help", t, |menu, _, _| {
                    menu.menu("About", Box::new(About))
                }))
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(13.0))
                        .text_color(rgba(t.text))
                        .child(SharedString::from(title.to_string())),
                ),
        )
}

fn menu_btn(
    id: &'static str,
    label: &'static str,
    t: &Colors,
    build: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
) -> impl IntoElement {
    Button::new(id)
        .ghost()
        .compact()
        .label(label)
        .text_color(rgba(t.text))
        .dropdown_menu(build)
}
