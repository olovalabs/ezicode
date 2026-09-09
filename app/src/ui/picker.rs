use std::path::{Path, PathBuf};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, img, px, rgba, svg, Context, Entity, InteractiveElement, IntoElement,
    MouseButton, ParentElement, SharedString, StatefulInteractiveElement, Styled,
};
use gpui_component::input::{Input, InputState};

use crate::theme::Colors;
use crate::workspace::Workspace;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickerKind {
    FileFinder,
    CommandPalette,
    GoToLine,
}

#[derive(Clone, Debug)]
pub struct PickerItem {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub icon: Option<String>,
    pub shortcut: Option<&'static str>,
    pub is_recent: bool,
    pub score: i64,
}

pub struct PickerState {
    pub kind: PickerKind,
    pub input: Entity<InputState>,
    pub raw_items: Vec<PickerItem>,
    pub filtered_items: Vec<PickerItem>,
    pub selected_index: usize,
}

impl PickerState {
    pub fn new(
        kind: PickerKind,
        input: Entity<InputState>,
        raw_items: Vec<PickerItem>,
    ) -> Self {
        let filtered_items = if kind == PickerKind::GoToLine {
            Vec::new()
        } else {
            raw_items.iter().take(40).cloned().collect()
        };
        Self {
            kind,
            input,
            raw_items,
            filtered_items,
            selected_index: 0,
        }
    }

    pub fn filter(&mut self, query: &str) {
        if self.kind == PickerKind::GoToLine {
            self.filtered_items.clear();
            self.selected_index = 0;
            return;
        }

        let query = query.trim();
        if query.is_empty() {
            self.filtered_items = self.raw_items.iter().take(40).cloned().collect();
            self.selected_index = 0;
            return;
        }

        // If user appends :line or :col, match against the file part
        let query_match = if let Some((f, _)) = query.split_once(':') {
            f.trim()
        } else {
            query
        };

        let matcher = SkimMatcherV2::default();
        let mut scored = Vec::new();

        for item in &self.raw_items {
            let target = match &item.subtitle {
                Some(sub) => format!("{} {}", item.title, sub),
                None => item.title.clone(),
            };
            if let Some(score) = matcher.fuzzy_match(&target, query_match) {
                let mut it = item.clone();
                it.score = score;
                // Boost recently opened files slightly
                if it.is_recent {
                    it.score += 50;
                }
                scored.push(it);
            }
        }

        scored.sort_by(|a, b| b.score.cmp(&a.score));
        self.filtered_items = scored.into_iter().take(40).collect();
        self.selected_index = 0;
    }

    pub fn select_next(&mut self) {
        if !self.filtered_items.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.filtered_items.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.filtered_items.is_empty() {
            if self.selected_index == 0 {
                self.selected_index = self.filtered_items.len() - 1;
            } else {
                self.selected_index -= 1;
            }
        }
    }

    pub fn selected_item(&self) -> Option<&PickerItem> {
        self.filtered_items.get(self.selected_index)
    }
}

pub fn scan_workspace_files(root: &Path, recent_files: &[PathBuf]) -> Vec<PickerItem> {
    use ignore::WalkBuilder;
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // 1. Add recent files first
    for path in recent_files {
        if path.is_file() {
            let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
            if !seen.insert(canonical) {
                continue;
            }
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let relative_dir = path
                .strip_prefix(root)
                .ok()
                .and_then(|p| p.parent())
                .map(|p| {
                    let s = p.to_string_lossy();
                    #[cfg(windows)]
                    { s.replace('/', "\\") }
                    #[cfg(not(windows))]
                    { s.to_string() }
                })
                .filter(|s| !s.is_empty());
            let icon = crate::file_icons::icon_for(path).to_string();
            items.push(PickerItem {
                id: path.to_string_lossy().to_string(),
                title: file_name,
                subtitle: relative_dir,
                icon: Some(icon),
                shortcut: None,
                is_recent: true,
                score: 0,
            });
        }
    }

    // 2. Scan remaining workspace files
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            name != ".git" && name != "target" && name != "node_modules"
        })
        .build();

    for result in walker {
        if let Ok(entry) = result {
            if entry.file_type().map_or(false, |ft| ft.is_file()) {
                let path = entry.path();
                let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
                if !seen.insert(canonical) {
                    continue;
                }
                let file_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                let relative_dir = path
                    .strip_prefix(root)
                    .ok()
                    .and_then(|p| p.parent())
                    .map(|p| {
                        let s = p.to_string_lossy();
                        #[cfg(windows)]
                        { s.replace('/', "\\") }
                        #[cfg(not(windows))]
                        { s.to_string() }
                    })
                    .filter(|s| !s.is_empty());
                let icon = crate::file_icons::icon_for(path).to_string();
                items.push(PickerItem {
                    id: path.to_string_lossy().to_string(),
                    title: file_name,
                    subtitle: relative_dir,
                    icon: Some(icon),
                    shortcut: None,
                    is_recent: false,
                    score: 0,
                });
            }
        }
    }
    items
}

pub fn command_palette_items() -> Vec<PickerItem> {
    vec![
        // File
        PickerItem {
            id: "file.new".into(),
            title: "New File".into(),
            subtitle: Some("File".into()),
            icon: Some("ui_icons/new_file.svg".into()),
            shortcut: Some("Ctrl+N"),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "file.open".into(),
            title: "Open File...".into(),
            subtitle: Some("File".into()),
            icon: Some("ui_icons/file.svg".into()),
            shortcut: Some("Ctrl+O"),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "file.open_folder".into(),
            title: "Open Folder...".into(),
            subtitle: Some("File".into()),
            icon: Some("ui_icons/folder.svg".into()),
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "file.save".into(),
            title: "Save File".into(),
            subtitle: Some("File".into()),
            icon: None,
            shortcut: Some("Ctrl+S"),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "file.quick_open".into(),
            title: "Quick Open File (File Finder)".into(),
            subtitle: Some("Navigation".into()),
            icon: Some("ui_icons/go-to-file.svg".into()),
            shortcut: Some("Ctrl+P"),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "view.goto_line".into(),
            title: "Go to Line...".into(),
            subtitle: Some("Navigation".into()),
            icon: None,
            shortcut: Some("Ctrl+G"),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "tab.close".into(),
            title: "Close Active Tab".into(),
            subtitle: Some("View".into()),
            icon: None,
            shortcut: Some("Ctrl+W"),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "tab.next".into(),
            title: "Next Tab".into(),
            subtitle: Some("View".into()),
            icon: None,
            shortcut: Some("Ctrl+Tab"),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "tab.prev".into(),
            title: "Previous Tab".into(),
            subtitle: Some("View".into()),
            icon: None,
            shortcut: Some("Ctrl+Shift+Tab"),
            is_recent: false,
            score: 0,
        },
        // Terminal
        PickerItem {
            id: "terminal.toggle".into(),
            title: "Toggle Integrated Terminal".into(),
            subtitle: Some("Terminal".into()),
            icon: Some("ui_icons/terminal.svg".into()),
            shortcut: Some("Ctrl+`"),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "terminal.new".into(),
            title: "New Terminal Session".into(),
            subtitle: Some("Terminal".into()),
            icon: Some("ui_icons/add.svg".into()),
            shortcut: Some("Ctrl+Shift+`"),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "terminal.close".into(),
            title: "Close Active Terminal".into(),
            subtitle: Some("Terminal".into()),
            icon: None,
            shortcut: Some("Ctrl+Shift+W"),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "terminal.clear".into(),
            title: "Clear Terminal Buffer".into(),
            subtitle: Some("Terminal".into()),
            icon: None,
            shortcut: Some("Ctrl+Shift+K"),
            is_recent: false,
            score: 0,
        },
        // Sidebar Views
        PickerItem {
            id: "sidebar.toggle".into(),
            title: "Toggle Primary Sidebar".into(),
            subtitle: Some("View".into()),
            icon: None,
            shortcut: Some("Ctrl+B"),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "view.explorer".into(),
            title: "Focus File Explorer".into(),
            subtitle: Some("View".into()),
            icon: Some("ui_icons/files.svg".into()),
            shortcut: Some("Ctrl+Shift+E"),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "view.search".into(),
            title: "Focus Project Search".into(),
            subtitle: Some("View".into()),
            icon: Some("ui_icons/search.svg".into()),
            shortcut: Some("Ctrl+Shift+F"),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "view.git".into(),
            title: "Focus Source Control (Git)".into(),
            subtitle: Some("View".into()),
            icon: Some("ui_icons/source-control.svg".into()),
            shortcut: Some("Ctrl+Shift+G"),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "view.extensions".into(),
            title: "Focus Extensions".into(),
            subtitle: Some("View".into()),
            icon: Some("ui_icons/extensions.svg".into()),
            shortcut: Some("Ctrl+Shift+X"),
            is_recent: false,
            score: 0,
        },
        // Preferences & Theme
        PickerItem {
            id: "preferences.settings".into(),
            title: "Open Preferences / Settings".into(),
            subtitle: Some("Preferences".into()),
            icon: Some("ui_icons/settings.svg".into()),
            shortcut: Some("Ctrl+,"),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "theme.github_dark".into(),
            title: "Color Theme: GitHub Dark".into(),
            subtitle: Some("Preferences".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "theme.ayu_dark".into(),
            title: "Color Theme: Ayu Dark".into(),
            subtitle: Some("Preferences".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "theme.ayu_mirage".into(),
            title: "Color Theme: Ayu Mirage".into(),
            subtitle: Some("Preferences".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "theme.ayu_light".into(),
            title: "Color Theme: Ayu Light".into(),
            subtitle: Some("Preferences".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "theme.gruvbox_dark".into(),
            title: "Color Theme: Gruvbox Dark".into(),
            subtitle: Some("Preferences".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        // Editor
        PickerItem {
            id: "editor.format".into(),
            title: "Format Document (LSP)".into(),
            subtitle: Some("Editor".into()),
            icon: None,
            shortcut: Some("Shift+Alt+F"),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "editor.font_increase".into(),
            title: "Increase Editor Font Size".into(),
            subtitle: Some("View".into()),
            icon: None,
            shortcut: Some("Ctrl+="),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "editor.font_decrease".into(),
            title: "Decrease Editor Font Size".into(),
            subtitle: Some("View".into()),
            icon: None,
            shortcut: Some("Ctrl+-"),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "editor.font_reset".into(),
            title: "Reset Editor Font Size".into(),
            subtitle: Some("View".into()),
            icon: None,
            shortcut: Some("Ctrl+0"),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "editor.copy_diagnostic".into(),
            title: "Copy Diagnostic Error".into(),
            subtitle: Some("Editor".into()),
            icon: None,
            shortcut: Some("Ctrl+Alt+C"),
            is_recent: false,
            score: 0,
        },
        // Git
        PickerItem {
            id: "git.refresh".into(),
            title: "Git: Refresh Changes".into(),
            subtitle: Some("Git".into()),
            icon: Some("ui_icons/refresh.svg".into()),
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "git.stage_all".into(),
            title: "Git: Stage All Changes".into(),
            subtitle: Some("Git".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "git.unstage_all".into(),
            title: "Git: Unstage All Changes".into(),
            subtitle: Some("Git".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "git.discard_all".into(),
            title: "Git: Discard All Changes".into(),
            subtitle: Some("Git".into()),
            icon: Some("ui_icons/discard.svg".into()),
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "git.commit".into(),
            title: "Git: Commit Changes".into(),
            subtitle: Some("Git".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        // Help
        PickerItem {
            id: "help.about".into(),
            title: "About ezicode".into(),
            subtitle: Some("Help".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "app.quit".into(),
            title: "Quit ezicode".into(),
            subtitle: Some("Application".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
    ]
}

pub fn render_picker(
    picker: &PickerState,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let kind = picker.kind;
    let selected_index = picker.selected_index;
    let filtered_items = &picker.filtered_items;

    let items_view = if kind == PickerKind::GoToLine {
        div()
            .px(px(12.0))
            .py(px(12.0))
            .text_size(px(13.5))
            .text_color(rgba(0x8b949eff))
            .child("Type a line number (and optional :column) and press Enter to jump.")
            .into_any_element()
    } else if filtered_items.is_empty() {
        div()
            .px(px(12.0))
            .py(px(16.0))
            .flex()
            .justify_center()
            .text_size(px(13.5))
            .text_color(rgba(0x8b949eff))
            .child("No matching results")
            .into_any_element()
    } else {
        let mut list_col = div()
            .id("picker-results-list")
            .w_full()
            .max_h(px(460.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .mt(px(6.0))
            .gap(px(1.0));

        for (ix, item) in filtered_items.iter().enumerate() {
            let is_selected = ix == selected_index;
            let icon = item.icon.as_deref();
            let title = item.title.clone();
            let subtitle = item.subtitle.clone();
            let shortcut = item.shortcut;

            let row_bg = if is_selected {
                rgba(0x283344ff)
            } else {
                rgba(0x00000000)
            };

            let title_color = if is_selected {
                rgba(0xf0f6fcff)
            } else {
                rgba(0xc9d1d9ff)
            };

            let mut row = div()
                .id(SharedString::from(format!("picker-item-{ix}")))
                .w_full()
                .h(px(32.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .justify_between()
                .rounded(px(4.0))
                .bg(row_bg)
                .cursor_pointer()
                .hover(|s| {
                    if !is_selected {
                        s.bg(rgba(0x1c212c88))
                    } else {
                        s
                    }
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_picker_item(ix, window, cx);
                    cx.stop_propagation();
                }));

            let mut left = div().flex().items_center().gap(px(10.0)).min_w(px(0.0));

            if let Some(ic) = icon {
                let icon_el = if ic.starts_with("file_icons/") {
                    img(SharedString::from(ic.to_string()))
                        .w(px(16.0))
                        .h(px(16.0))
                        .into_any_element()
                } else if ic.starts_with("ui_icons/") {
                    svg()
                        .path(SharedString::from(ic.to_string()))
                        .w(px(16.0))
                        .h(px(16.0))
                        .text_color(rgba(0x8b949eff))
                        .into_any_element()
                } else {
                    img(SharedString::from(ic.to_string()))
                        .w(px(16.0))
                        .h(px(16.0))
                        .into_any_element()
                };
                left = left.child(
                    div()
                        .size(px(16.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon_el),
                );
            }

            left = left.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(13.5))
                            .font_weight(gpui::FontWeight::NORMAL)
                            .text_color(title_color)
                            .line_clamp(1)
                            .child(title),
                    )
                    .when_some(subtitle, |parent, sub| {
                        parent.child(
                            div()
                                .text_size(px(12.5))
                                .text_color(rgba(0x8b949eff))
                                .line_clamp(1)
                                .child(sub),
                        )
                    }),
            );

            row = row.child(left);

            if is_selected && kind == PickerKind::FileFinder {
                row = row.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            // Split editor icon [ | ]
                            div()
                                .w(px(14.0))
                                .h(px(12.0))
                                .rounded(px(2.0))
                                .border_1()
                                .border_color(rgba(0x8b949eff))
                                .flex()
                                .child(
                                    div()
                                        .w(px(6.0))
                                        .h_full()
                                        .border_r_1()
                                        .border_color(rgba(0x8b949eff)),
                                ),
                        )
                        .child(
                            // Close icon ✕
                            div()
                                .text_size(px(12.0))
                                .text_color(rgba(0x8b949eff))
                                .cursor_pointer()
                                .child("✕"),
                        )
                        .when(item.is_recent, |parent| {
                            parent.child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgba(0x8b949eff))
                                    .child("recently opened"),
                            )
                        }),
                );
            } else if let Some(sc) = shortcut {
                row = row.child(
                    div()
                        .px(px(6.0))
                        .py(px(1.0))
                        .rounded(px(3.0))
                        .bg(rgba(t.element_bg))
                        .border_1()
                        .border_color(rgba(t.border))
                        .text_size(px(12.0))
                        .text_color(rgba(t.text_muted))
                        .font_family(crate::assets::MONO_FONT)
                        .child(sc),
                );
            }

            list_col = list_col.child(row);
        }

        list_col.into_any_element()
    };

    // Full screen backdrop
    div()
        .id("picker-backdrop")
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .occlude()
        .bg(rgba(0x00000088))
        .flex()
        .flex_col()
        .items_center()
        .pt(px(50.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                this.close_modal(window, cx);
                cx.stop_propagation();
            }),
        )
        // Dialog Card exactly matching reference
        .child(
            div()
                .id("picker-card")
                .w(px(680.0))
                .max_w(px(740.0))
                .max_h(px(520.0))
                .bg(rgba(0x161922ff))
                .rounded(px(8.0))
                .border_1()
                .border_color(rgba(0x282c37ff))
                .shadow_2xl()
                .overflow_hidden()
                .p(px(8.0))
                .flex()
                .flex_col()
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                // Search Input Box: dark background + bright blue focus border
                .child(
                    div()
                        .w_full()
                        .h(px(38.0))
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .rounded(px(6.0))
                        .bg(rgba(0x0e1117ff))
                        .border_1()
                        .border_color(rgba(0x388bfdff))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .text_size(px(13.5))
                                .child(
                                    Input::new(&picker.input)
                                        .text_size(px(13.5))
                                        .appearance(false)
                                        .cleanable(false),
                                ),
                        ),
                )
                // Results list
                .child(items_view),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_palette_items_populated() {
        let items = command_palette_items();
        assert!(!items.is_empty(), "command palette should have commands");
        assert!(items.iter().any(|i| i.id == "file.save"));
        assert!(items.iter().any(|i| i.id == "terminal.toggle"));
        assert!(items.iter().any(|i| i.id == "file.quick_open"));
        assert!(items.iter().any(|i| i.id == "view.goto_line"));
    }

    #[test]
    fn test_fuzzy_filtering() {
        let items = vec![
            PickerItem {
                id: "1".into(),
                title: "settings.rs".into(),
                subtitle: Some("app/src".into()),
                icon: None,
                shortcut: None,
                is_recent: false,
                score: 0,
            },
            PickerItem {
                id: "2".into(),
                title: "main.rs".into(),
                subtitle: Some("app/src".into()),
                icon: None,
                shortcut: None,
                is_recent: false,
                score: 0,
            },
            PickerItem {
                id: "3".into(),
                title: "Cargo.toml".into(),
                subtitle: None,
                icon: None,
                shortcut: None,
                is_recent: false,
                score: 0,
            },
        ];

        let matcher = SkimMatcherV2::default();
        let mut scored = Vec::new();
        for item in &items {
            let target = match &item.subtitle {
                Some(sub) => format!("{} {}", item.title, sub),
                None => item.title.clone(),
            };
            if let Some(score) = matcher.fuzzy_match(&target, "sett") {
                let mut it = item.clone();
                it.score = score;
                scored.push(it);
            }
        }
        assert_eq!(scored.len(), 1);
        assert_eq!(scored[0].title, "settings.rs");
    }

    #[test]
    fn test_scan_workspace_files_ignores_target_and_git() {
        let root = Path::new(".");
        let files = scan_workspace_files(root, &[]);
        for f in &files {
            assert!(!f.id.contains(".git"), "Should not contain .git files: {}", f.id);
            assert!(!f.id.contains("target"), "Should not contain target files: {}", f.id);
        }
    }
}
