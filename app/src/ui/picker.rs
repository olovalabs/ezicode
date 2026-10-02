use crate::cancellation::Cancellation;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    div, img, px, rgba, svg, Context, Entity, InteractiveElement, IntoElement, MouseButton,
    ParentElement, SharedString, StatefulInteractiveElement, Styled,
};
use gpui_component::input::{Input, InputState};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::theme::Colors;
use crate::ui::scale::rem;
use crate::workspace::Workspace;
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickerKind {
    FileFinder,
    CommandPalette,
    GoToLine,
    LanguageSelector,
    /// Branch switcher: Enter checks out the selection, or creates a branch
    /// named after the query when nothing matches.
    GitBranch,
    /// Branch deletion: Enter deletes the selected branch (`git branch -d`).
    GitBranchDelete,
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
    pub raw_items: Arc<Vec<PickerItem>>,
    pub loading: bool,
    pub indexing: bool,
    pub filtered_items: Vec<PickerItem>,
    pub selected_index: usize,
}

impl PickerState {
    pub fn new(kind: PickerKind, input: Entity<InputState>, raw_items: Vec<PickerItem>) -> Self {
        let filtered_items = if kind == PickerKind::GoToLine {
            Vec::new()
        } else {
            raw_items.iter().take(40).cloned().collect()
        };
        Self {
            kind,
            input,
            raw_items: Arc::new(raw_items),
            loading: false,
            indexing: false,
            filtered_items,
            selected_index: 0,
        }
    }

    pub fn filter(&mut self, query: &str) {
        self.filtered_items = if self.kind == PickerKind::GoToLine {
            Vec::new()
        } else {
            filter_items(&self.raw_items, query, &Cancellation::default())
        };
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

/// Pure filtering. Score indices, not cloned items: only the best forty
/// results allocate strings, even when every project file matches a query.
pub fn filter_items(
    items: &[PickerItem],
    query: &str,
    cancelled: &Cancellation,
) -> Vec<PickerItem> {
    filter_items_inner(items, query, cancelled, None)
}

pub fn filter_items_with_recents(
    items: &[PickerItem],
    query: &str,
    cancelled: &Cancellation,
    recent: &[PathBuf],
) -> Vec<PickerItem> {
    let ranks = recent
        .iter()
        .enumerate()
        .map(|(index, path)| (path.to_string_lossy().into_owned(), index))
        .collect();
    filter_items_inner(items, query, cancelled, Some(&ranks))
}

fn filter_items_inner(
    items: &[PickerItem],
    query: &str,
    cancelled: &Cancellation,
    recent: Option<&std::collections::HashMap<String, usize>>,
) -> Vec<PickerItem> {
    if cancelled.is_cancelled() {
        return Vec::new();
    }
    let query = query.trim();
    if query.is_empty() && recent.is_none() {
        return items.iter().take(40).cloned().collect();
    }
    // If user appends :line or :col, match against the file part.
    let query_match = query.split_once(':').map_or(query, |(file, _)| file.trim());
    let matcher = SkimMatcherV2::default();
    let mut scored = Vec::new();
    for (index, item) in items.iter().enumerate() {
        if cancelled.is_cancelled() {
            return Vec::new();
        }
        let rank = recent.and_then(|ranks| ranks.get(&item.id)).copied();
        let is_recent = recent.map_or(item.is_recent, |_| rank.is_some());
        let score = if query_match.is_empty() {
            Some(0)
        } else if let Some(subtitle) = &item.subtitle {
            matcher
                .fuzzy_match(&item.title, query_match)
                .map(|score| score + 30)
                .or_else(|| matcher.fuzzy_match(subtitle, query_match))
        } else {
            matcher.fuzzy_match(&item.title, query_match)
        };
        if let Some(score) = score {
            scored.push((
                score + if is_recent { 50 } else { 0 },
                rank.unwrap_or(usize::MAX),
                index,
                is_recent,
            ));
        }
    }
    scored.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
    });
    scored
        .into_iter()
        .take(40)
        .map(|(score, _, index, is_recent)| {
            let mut item = items[index].clone();
            item.score = score;
            item.is_recent = is_recent;
            item
        })
        .collect()
}

fn is_ignored_scan_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "target"
            | "node_modules"
            | ".pnpm-store"
            | ".turbo"
            | ".next"
            | "dist"
            | "build"
            | "out"
            | ".cache"
            | "test-results"
            | ".vscode"
            | ".idea"
            | ".gradle"
    )
}

#[cfg(test)]
pub fn scan_workspace_files(root: &Path, recent_files: &[PathBuf]) -> Vec<PickerItem> {
    scan_workspace_files_cancellable(root, recent_files, &Cancellation::default(), None)
}

pub fn scan_workspace_files_cancellable(
    root: &Path,
    recent_files: &[PathBuf],
    cancelled: &Cancellation,
    watches: Option<std::sync::mpsc::Sender<PathBuf>>,
) -> Vec<PickerItem> {
    let _span = crate::perf::span("workspace.file_index.background");
    if cancelled.is_cancelled() {
        return Vec::new();
    }
    use ignore::WalkBuilder;
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // 1. Add recent files first
    for path in recent_files {
        if cancelled.is_cancelled() {
            return Vec::new();
        }
        if path.is_file() {
            let rel = path.strip_prefix(root).unwrap_or(path).to_path_buf();
            if !seen.insert(rel) {
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
                    {
                        s.replace('/', "\\")
                    }
                    #[cfg(not(windows))]
                    {
                        s.to_string()
                    }
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

    // 2. Scan remaining workspace files without expensive canonicalize syscalls
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .filter_entry(|entry| {
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                let name = entry.file_name().to_string_lossy();
                !is_ignored_scan_dir(&name)
            } else {
                true
            }
        })
        .build();

    for entry in walker.flatten() {
        if cancelled.is_cancelled() {
            return Vec::new();
        }
        if entry.file_type().is_some_and(|ft| ft.is_dir()) {
            if let Some(watches) = &watches {
                let _ = watches.send(entry.path().to_path_buf());
            }
        }
        if entry.file_type().is_some_and(|ft| ft.is_file()) {
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap_or(path).to_path_buf();
            if !seen.insert(rel) {
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
                    {
                        s.replace('/', "\\")
                    }
                    #[cfg(not(windows))]
                    {
                        s.to_string()
                    }
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

            if items.len() >= 15_000 {
                break;
            }
        }
    }
    items
}

pub fn language_selector_items(current_lang: Option<&str>) -> Vec<PickerItem> {
    crate::lang::all_languages()
        .iter()
        .map(|lang| {
            let is_current = current_lang.is_some_and(|c| c.eq_ignore_ascii_case(lang.id));
            let subtitle = if let Some(server) = lang.lsp_server {
                format!("Language Server: {server}")
            } else {
                "Built-in Syntax Highlighting".to_string()
            };
            PickerItem {
                id: lang.id.to_string(),
                title: lang.name.to_string(),
                subtitle: Some(subtitle),
                icon: Some(lang.icon.to_string()),
                shortcut: if is_current { Some("Current") } else { None },
                is_recent: is_current,
                score: 0,
            }
        })
        .collect()
}

pub fn command_palette_items() -> Vec<PickerItem> {
    vec![
        // Language Mode
        PickerItem {
            id: "language.change_mode".into(),
            title: "Change Language Mode".into(),
            subtitle: Some("Language / LSP / Syntax".into()),
            icon: Some("ui_icons/file-code.svg".into()),
            shortcut: Some("Ctrl+K M"),
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "project.switcher".into(),
            title: "Switch Project".into(),
            subtitle: Some("Navigation".into()),
            icon: Some("ui_icons/project-switcher_tint.svg".into()),
            shortcut: Some("Ctrl+Shift+O"),
            is_recent: false,
            score: 0,
        },
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
            id: "terminal.toggle_right".into(),
            title: "Toggle Right Terminal Panel".into(),
            subtitle: Some("Terminal".into()),
            icon: Some("ui_icons/terminal_panel_right.svg".into()),
            shortcut: None,
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
            id: "theme.github_light".into(),
            title: "Color Theme: GitHub Light".into(),
            subtitle: Some("Preferences".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "theme.github_dark_dimmed".into(),
            title: "Color Theme: GitHub Dark Dimmed".into(),
            subtitle: Some("Preferences".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "theme.github_dark_high_contrast".into(),
            title: "Color Theme: GitHub Dark High Contrast".into(),
            subtitle: Some("Preferences".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "theme.github_light_high_contrast".into(),
            title: "Color Theme: GitHub Light High Contrast".into(),
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
            id: "ui.font_size_increase".into(),
            title: "Increase UI Font Size".into(),
            subtitle: Some("View".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "ui.font_size_decrease".into(),
            title: "Decrease UI Font Size".into(),
            subtitle: Some("View".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "ui.font_size_reset".into(),
            title: "Reset UI Font Size".into(),
            subtitle: Some("View".into()),
            icon: None,
            shortcut: None,
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
        PickerItem {
            id: "git.commit_all".into(),
            title: "Git: Commit All (Tracked)".into(),
            subtitle: Some("Git".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "git.commit_amend".into(),
            title: "Git: Amend Last Commit".into(),
            subtitle: Some("Git".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "git.fetch".into(),
            title: "Git: Fetch".into(),
            subtitle: Some("Git".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "git.pull".into(),
            title: "Git: Pull".into(),
            subtitle: Some("Git".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "git.push".into(),
            title: "Git: Push".into(),
            subtitle: Some("Git".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "git.push_force".into(),
            title: "Git: Force Push (with lease)".into(),
            subtitle: Some("Git".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "git.stash".into(),
            title: "Git: Stash Changes (incl. untracked)".into(),
            subtitle: Some("Git".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "git.stash_pop".into(),
            title: "Git: Pop Latest Stash".into(),
            subtitle: Some("Git".into()),
            icon: None,
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "git.branch.checkout".into(),
            title: "Git: Checkout Branch… (create / switch)".into(),
            subtitle: Some("Git".into()),
            icon: Some("ui_icons/git_branch.svg".into()),
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "git.branch.delete".into(),
            title: "Git: Delete Branch…".into(),
            subtitle: Some("Git".into()),
            icon: Some("ui_icons/git_branch.svg".into()),
            shortcut: None,
            is_recent: false,
            score: 0,
        },
        PickerItem {
            id: "git.init".into(),
            title: "Git: Initialize Repository".into(),
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
            .text_size(rem(13.5))
            .text_color(rgba(0x8b949eff))
            .child("Type a line number (and optional :column) and press Enter to jump.")
            .into_any_element()
    } else if filtered_items.is_empty() {
        let empty_text = if picker.loading {
            "Loading files…"
        } else {
            match kind {
                PickerKind::GitBranch => {
                    "No matching branch — press Enter to create a branch with the typed name"
                }
                PickerKind::GitBranchDelete => "No matching branch",
                _ => "No matching results",
            }
        };
        div()
            .px(px(12.0))
            .py(px(16.0))
            .flex()
            .justify_center()
            .text_size(rem(13.5))
            .text_color(rgba(0x8b949eff))
            .child(empty_text)
            .into_any_element()
    } else {
        let mut list_col = div()
            .id("picker-results-list")
            .w_full()
            .max_h(rem(460.0))
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
                .h(rem(32.0))
                .px(rem(10.0))
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
                        .w(rem(16.0))
                        .h(rem(16.0))
                        .into_any_element()
                } else if ic.starts_with("ui_icons/") {
                    svg()
                        .path(SharedString::from(ic.to_string()))
                        .w(rem(16.0))
                        .h(rem(16.0))
                        .text_color(rgba(0x8b949eff))
                        .into_any_element()
                } else {
                    img(SharedString::from(ic.to_string()))
                        .w(rem(16.0))
                        .h(rem(16.0))
                        .into_any_element()
                };
                left = left.child(
                    div()
                        .size(rem(16.0))
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
                            .text_size(rem(13.5))
                            .font_weight(gpui::FontWeight::NORMAL)
                            .text_color(title_color)
                            .line_clamp(1)
                            .child(title),
                    )
                    .when_some(subtitle, |parent, sub| {
                        parent.child(
                            div()
                                .text_size(rem(12.5))
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
                                .w(rem(14.0))
                                .h(rem(12.0))
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
                                .text_size(rem(12.0))
                                .text_color(rgba(0x8b949eff))
                                .cursor_pointer()
                                .child("✕"),
                        )
                        .when(item.is_recent, |parent| {
                            parent.child(
                                div()
                                    .text_size(rem(12.0))
                                    .text_color(rgba(0x8b949eff))
                                    .child("recently opened"),
                            )
                        }),
                );
            } else if let Some(sc) = shortcut {
                let badge_color = if kind == PickerKind::LanguageSelector && item.is_recent {
                    rgba(t.vc_added)
                } else {
                    rgba(t.text_muted)
                };
                row = row.child(
                    div()
                        .px(px(6.0))
                        .py(px(1.0))
                        .rounded(px(3.0))
                        .bg(rgba(t.element_bg))
                        .border_1()
                        .border_color(rgba(t.border))
                        .text_size(rem(12.0))
                        .text_color(badge_color)
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
                        .h(rem(38.0))
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .rounded(px(6.0))
                        .bg(rgba(0x0e1117ff))
                        .border_1()
                        .border_color(rgba(0x388bfdff))
                        .child(
                            div().flex_1().min_w(px(0.0)).text_size(rem(13.5)).child(
                                Input::new(&picker.input)
                                    .text_size(rem(13.5))
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
    fn cached_indexes_use_current_recent_priority() {
        let items = ["/project/old.txt", "/project/new.txt"]
            .into_iter()
            .map(|id| PickerItem {
                id: id.into(),
                title: "file.txt".into(),
                subtitle: None,
                icon: None,
                shortcut: None,
                is_recent: id.ends_with("old.txt"),
                score: 0,
            })
            .collect::<Vec<_>>();
        let recent = vec![PathBuf::from("/project/new.txt")];
        for query in ["", "file", "file:12:3"] {
            let filtered =
                filter_items_with_recents(&items, query, &Cancellation::default(), &recent);
            assert_eq!(filtered[0].id, "/project/new.txt");
            assert!(filtered[0].is_recent);
            assert!(!filtered[1].is_recent);
        }
    }

    #[test]
    fn filtering_large_indexes_is_cancellable_and_limits_results() {
        let items = (0..15000)
            .map(|index| PickerItem {
                id: index.to_string(),
                title: format!("file-{index}.rs"),
                subtitle: None,
                icon: None,
                shortcut: None,
                is_recent: false,
                score: 0,
            })
            .collect::<Vec<_>>();
        let cancellation = Cancellation::default();
        assert_eq!(filter_items(&items, "file", &cancellation).len(), 40);
        cancellation.cancel();
        assert!(filter_items(&items, "file", &cancellation).is_empty());
    }

    #[test]
    fn cancelled_indexes_do_not_scan_old_workspaces() {
        let cancellation = Cancellation::default();
        cancellation.cancel();
        assert!(
            scan_workspace_files_cancellable(Path::new("."), &[], &cancellation, None).is_empty()
        );
    }

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
            assert!(
                !f.id.contains(".git"),
                "Should not contain .git files: {}",
                f.id
            );
            assert!(
                !f.id.contains("target"),
                "Should not contain target files: {}",
                f.id
            );
        }
    }

    #[test]
    fn test_language_selector_items() {
        let items = language_selector_items(Some("rust"));
        assert!(!items.is_empty());
        let rust = items.iter().find(|i| i.id == "rust").expect("rust item");
        assert_eq!(rust.title, "Rust");
        assert!(rust.is_recent);
        assert_eq!(rust.shortcut, Some("Current"));

        let py = items
            .iter()
            .find(|i| i.id == "python")
            .expect("python item");
        assert_eq!(py.title, "Python");
        assert!(!py.is_recent);
    }
}
