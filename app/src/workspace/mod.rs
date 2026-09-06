//! Application state. The single [`Workspace`] entity owns the open buffer,
//! the explorer tree and all UI flags; this module holds the struct plus the
//! commands that mutate it. Rendering lives in [`render`].

mod render;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    AppContext, Context, Entity, FocusHandle, ScrollStrategy, SharedString,
    UniformListScrollHandle, Window,
};
use gpui_component::input::{InputEvent, InputState, TabSize};
use notify::Watcher as _;

use crate::fs_tree::{
    collapse_all, display_name, flatten_visible, is_same_or_descendant, load_dir,
    merge_loaded_dir, path_after_move, valid_entry_name, TreeNode, VisibleTreeRow,
};
use crate::git::{self, GitChange, RepoStatus};
use crate::lang;
use crate::lsp::{LspEvent, LspManager};
use crate::theme;

/// Represents a single open tab with its own editor buffer or special view
#[derive(Clone)]
pub struct OpenTab {
    pub path: Option<PathBuf>,
    pub editor: Option<Entity<InputState>>,
    pub dirty: bool,
    pub untitled: bool,
    /// True if this is a preview tab (will be replaced when opening another file)
    /// VS Code behavior: clicking a file opens it in preview mode,
    /// editing promotes it to a permanent tab
    pub preview: bool,
    /// True if this tab represents the VS Code Settings page
    pub is_settings: bool,
    /// True when this tab is a Git diff view (of `diff.path`).
    pub diff: Option<DiffTab>,
}

/// A Git diff view opened from the source-control panel. The raw unified
/// diff text is produced on a background thread and cached here.
#[derive(Clone)]
pub(crate) struct DiffTab {
    /// Absolute path of the changed file.
    pub path: PathBuf,
    /// Repo-relative path used for the git CLI.
    pub rel: String,
    /// True when the diff shows the staged (index) version.
    pub staged: bool,
    /// Cached unified-diff text, `None` while loading.
    pub text: Option<String>,
    /// Non-fatal load error (shown inside the diff view).
    pub error: Option<String>,
}

/// Which sidebar panel is active.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Activity {
    Explorer,
    Search,
    Git,
    Extensions,
}

impl Activity {
    /// Label shown in the status bar when this panel is selected.
    pub(crate) fn status_label(self) -> &'static str {
        match self {
            Activity::Explorer => "EXPLORER",
            Activity::Search => "SEARCH",
            Activity::Git => "SOURCE CONTROL",
            Activity::Extensions => "EXTENSIONS",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CreatingKind {
    File,
    Folder,
}

#[derive(Clone)]
pub(crate) struct InlineCreating {
    pub(crate) kind: CreatingKind,
    pub(crate) parent_dir: PathBuf,
    pub(crate) input: Entity<InputState>,
}

/// The inline editor used by F2 / Rename. Keeping it in the workspace rather
/// than opening a native save dialog makes rename work for both files and
/// folders and preserves the tree focus/selection just like VS Code.
#[derive(Clone)]
pub(crate) struct InlineRenaming {
    pub(crate) path: PathBuf,
    pub(crate) input: Entity<InputState>,
}

/// Payload carried by GPUI while an explorer row is being dragged.
#[derive(Clone, Debug)]
pub(crate) struct ExplorerDrag {
    pub(crate) path: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) struct ExplorerClipboard {
    pub(crate) path: PathBuf,
    pub(crate) cut: bool,
}

pub(crate) struct Workspace {
    /// None until the user opens a folder (VS Code-style start state).
    pub(crate) root: Option<PathBuf>,
    pub(crate) tree: Vec<TreeNode>,
    /// Flat, render-ready rows for the explorer. The tree is flattened only
    /// after a structural change; normal paints reuse this Arc and let
    /// `uniform_list` render just the rows in the viewport.
    pub(crate) explorer_rows: Arc<[VisibleTreeRow]>,
    /// Persistent scroll position for the virtualized explorer list.
    pub(crate) explorer_scroll_handle: UniformListScrollHandle,
    /// Focus target used by explorer keyboard navigation.
    pub(crate) explorer_focus_handle: FocusHandle,
    /// Currently selected path in the explorer.
    pub(crate) selected_path: Option<PathBuf>,
    pub(crate) explorer_section_expanded: bool,
    pub(crate) git_repo_section_expanded: bool,
    pub(crate) git_staged_expanded: bool,
    pub(crate) git_changes_expanded: bool,
    pub(crate) split_diff: bool,
    pub(crate) inline_creating: Option<InlineCreating>,
    pub(crate) inline_renaming: Option<InlineRenaming>,
    /// Internal explorer clipboard used by Ctrl+C/X/V. It intentionally stores
    /// paths, not file contents, so large files and folders stay cheap.
    pub(crate) explorer_clipboard: Option<ExplorerClipboard>,
    /// A file to open once the next render cycle runs.
    pub(crate) pending_open: Option<PathBuf>,
    pub(crate) status: String,
    pub(crate) activity: Activity,
    pub(crate) show_sidebar: bool,
    pub(crate) show_terminal: bool,
    pub(crate) theme_ix: usize,
    /// Editor buffer font size in pixels (supports Ctrl++/Ctrl-- zoom like Zed)
    pub(crate) font_size: f32,
    /// Language Server Protocol client manager
    pub(crate) lsp: Arc<Mutex<LspManager>>,
    /// Active diagnostics received from language servers, shared with the
    /// editors via `Arc` so an LSP publish clones the payload once (per
    /// editor) instead of once per storage site plus once per editor.
    pub(crate) diagnostics_by_path: HashMap<PathBuf, Arc<Vec<lsp_types::Diagnostic>>>,
    /// Integrated Terminal tabs (VS Code-style: multiple shells, one active).
    pub(crate) terminal_tabs: Vec<Entity<crate::terminal::Terminal>>,
    /// Index of the currently active terminal tab.
    pub(crate) active_terminal: usize,
    /// Monotonic counter for labeling new terminals (PowerShell 1, PowerShell 2, ...).
    pub(crate) next_terminal_id: usize,
    /// File system change notification sender: the changed path, so reloads
    /// can be scoped to the affected directory instead of rescanning the
    /// whole tree on every event.
    pub(crate) fs_event_tx: async_channel::Sender<PathBuf>,
    /// Cached `display_name(root)` so the title bar and explorer header don't
    /// re-derive (and re-allocate) the folder name on every frame.
    pub(crate) root_display: String,
    /// GPUI's shared form of the same label; cloning it for a render is cheap
    /// and avoids converting the borrowed String into a new allocation.
    pub(crate) root_display_shared: SharedString,
    /// Background file system watcher
    pub(crate) _watcher: Option<notify::RecommendedWatcher>,
    /// Open tabs
    pub(crate) tabs: Vec<OpenTab>,
    /// Index of the currently active tab
    pub(crate) active_tab: usize,
    /// Workspace-level focus target (Zed-style). The workspace always holds
    /// focus when no editor/terminal does, so global keybindings always have
    /// a dispatch path and shortcuts never go dead.
    pub(crate) focus_handle: FocusHandle,
    /// Draggable sidebar width in pixels. Kept at an absolute pixel value
    /// when the window resizes (VS Code behavior): the editor area absorbs
    /// the change instead of the sidebar scaling with the window.
    pub(crate) sidebar_width: f32,
    /// Draggable terminal panel height in pixels (same behavior as above).
    pub(crate) terminal_height: f32,
    /// Active panel resize drag: which handle was grabbed, where the mouse
    /// was at grab time and how big the panel was.
    pub(crate) panel_resize: Option<PanelResizeDrag>,
    /// Git repository state of the opened folder (`None` when it isn't a
    /// git repository). Refreshed in the background by a polling thread.
    pub(crate) git: Option<RepoStatus>,
    /// Pokes the git polling thread to re-run `git status` immediately
    /// (after saves, commits, staging, …).
    pub(crate) git_poke_tx: Option<std::sync::mpsc::Sender<()>>,
    /// The commit-message input of the source-control panel (created lazily
    /// when the panel is opened).
    pub(crate) git_commit_input: Option<Entity<InputState>>,
    /// Set by the commit input's Enter handler (which has no window handle);
    /// render() runs the commit on the next frame, where the window exists.
    pub(crate) git_commit_pending: bool,
    /// User preferences loaded from platform settings.json
    pub(crate) settings: crate::settings::Settings,
    /// Debounce generation counter for auto-save
    pub(crate) auto_save_generation: usize,
}

/// Which divider is being dragged.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum ResizeKind {
    Sidebar,
    Terminal,
}

/// State of an in-progress panel resize drag.
#[derive(Clone, Copy)]
pub(crate) struct PanelResizeDrag {
    pub(crate) kind: ResizeKind,
    /// Mouse position along the drag axis at grab time.
    pub(crate) start_mouse: f32,
    /// Panel size at grab time.
    pub(crate) start_size: f32,
}

impl Workspace {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Zed focuses the workspace on activation so keybindings always have
        // a focused element to dispatch through, even on the welcome screen.
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        let explorer_focus_handle = cx.focus_handle();
        let explorer_scroll_handle = UniformListScrollHandle::new();

        let lsp_mgr = LspManager::new();
        let rx = lsp_mgr.event_receiver();
        let lsp = Arc::new(Mutex::new(lsp_mgr));

        cx.spawn({
            let rx = rx.clone();
            async move |this, cx| {
                while let Ok(event) = rx.recv().await {
                    match event {
                        LspEvent::Diagnostics { path, diagnostics } => {
                            let _ = this.update(cx, |workspace, cx| {
                                workspace.apply_diagnostics(&path, diagnostics, cx);
                            });
                        }
                        LspEvent::Status { lang, message } => {
                            let _ = this.update(cx, |workspace, cx| {
                                workspace.status = format!("{lang}: {message}");
                                cx.notify();
                            });
                        }
                        // A background npm install finished: start the
                        // server and hand it every buffer it can analyse.
                        LspEvent::ServerReady { server } => {
                            let _ = this.update(cx, |workspace, cx| {
                                workspace.lsp.lock().unwrap().finish_install(&server);
                                workspace.status = format!("{server} ready");
                                workspace.start_server_for_open_buffers(&server, cx);
                                cx.notify();
                            });
                        }
                        LspEvent::ServerFailed { server, reason } => {
                            let _ = this.update(cx, |workspace, cx| {
                                workspace
                                    .lsp
                                    .lock()
                                    .unwrap()
                                    .set_failed(&server, reason.clone());
                                workspace.status = format!("{server}: {reason}");
                                cx.notify();
                            });
                        }
                        LspEvent::Initialized { server } => {
                            let _ = this.update(cx, |workspace, cx| {
                                workspace.lsp.lock().unwrap().set_running(&server);
                                cx.notify();
                            });
                        }
                        // The server process died (crash, OOM). Forget the
                        // cached client and respawn it for every open buffer
                        // — the same supervision Zed applies. Skip when the
                        // exit was a deliberate drop (app quit / root
                        // change): respawning then would fight the teardown.
                        LspEvent::ServerExited { server } => {
                            let _ = this.update(cx, |workspace, cx| {
                                let was_running =
                                    workspace.lsp.lock().unwrap().drop_client(&server);
                                if was_running {
                                    workspace.status = format!("{server} exited — restarting…");
                                    workspace.start_server_for_open_buffers(&server, cx);
                                    cx.notify();
                                }
                            });
                        }
                    }
                }
            }
        })
        .detach();

        let (fs_event_tx, fs_event_rx) = async_channel::unbounded::<PathBuf>();
        // Resolved "directories to reload" channel, produced off the UI
        // thread and consumed on it.
        let (fs_reload_tx, fs_reload_rx) = async_channel::unbounded::<Vec<PathBuf>>();

        // Debounce + coalesce raw filesystem events on a dedicated OS thread
        // (never the UI thread — the old handler slept a blocking 150 ms
        // twice inside the foregound executor). Only the *changed path* is
        // forwarded; the UI-side reload is then scoped to the affected
        // directory instead of rescanning the whole tree.
        std::thread::spawn(move || {
            let rx = fs_event_rx;
            loop {
                let Ok(first) = rx.recv_blocking() else { break };
                let mut paths = vec![first];
                while let Ok(more) = rx.try_recv() {
                    paths.push(more);
                }
                std::thread::sleep(std::time::Duration::from_millis(120));
                while let Ok(more) = rx.try_recv() {
                    paths.push(more);
                }

                // The parent always needs a refresh because a create/delete
                // changes its entry set. A directory's own level is included
                // too so a newly-created folder is visible as soon as it is
                // expanded. Never walk the whole project tree for one event.
                let mut dirs: HashSet<PathBuf> = HashSet::new();
                for p in paths {
                    if let Some(parent) = p.parent() {
                        dirs.insert(parent.to_path_buf());
                    }
                    if p.is_dir() {
                        dirs.insert(p);
                    }
                }
                if !dirs.is_empty() {
                    let _ = fs_reload_tx.try_send(dirs.into_iter().collect());
                }
            }
        });

        // Scan the affected directory levels off the UI thread. The UI only
        // merges the finished snapshots, so even a watcher burst in a large
        // folder cannot block input, scrolling, or painting.
        cx.spawn({
            let rx = fs_reload_rx.clone();
            async move |this, cx| {
                while let Ok(dirs) = rx.recv().await {
                    let scanned = cx
                        .background_spawn(async move {
                            dirs.into_iter()
                                .map(|dir| {
                                    let entries = load_dir(&dir);
                                    (dir, entries)
                                })
                                .collect::<Vec<_>>()
                        })
                        .await;
                    let _ = this.update(cx, |workspace, cx| {
                        let mut changed = false;
                        for (dir, entries) in scanned {
                            changed |= workspace.apply_loaded_dir_inner(&dir, entries);
                        }
                        if changed {
                            workspace.rebuild_explorer_rows();
                            cx.notify();
                        }
                    });
                }
            }
        })
        .detach();

        let settings = crate::settings::Settings::load();
        let themes = theme::all();
        let theme_ix = themes
            .iter()
            .position(|t| t.name == settings.workbench_color_theme)
            .unwrap_or_else(theme::default_index);
        let font_size = settings.editor_font_size;

        // Start with NO project loaded — the welcome screen offers
        // Open Folder / Open File / New File (VS Code-style).
        Self {
            root: None,
            tree: Vec::new(),
            explorer_rows: Arc::from(Vec::<VisibleTreeRow>::new()),
            explorer_scroll_handle,
            explorer_focus_handle,
            root_display: String::new(),
            root_display_shared: SharedString::new_static(""),
            selected_path: None,
            explorer_section_expanded: true,
            git_repo_section_expanded: true,
            git_staged_expanded: true,
            git_changes_expanded: true,
            split_diff: true,
            inline_creating: None,
            inline_renaming: None,
            explorer_clipboard: None,
            pending_open: None,
            status: "Welcome — open a folder or create a file to begin".into(),
            activity: Activity::Explorer,
            show_sidebar: true,
            show_terminal: false,
            theme_ix,
            font_size,
            settings,
            auto_save_generation: 0,
            lsp,
            diagnostics_by_path: HashMap::new(),
            terminal_tabs: Vec::new(),
            active_terminal: 0,
            next_terminal_id: 1,
            fs_event_tx,
            _watcher: None,
            tabs: Vec::new(),
            active_tab: 0,
            focus_handle,
            sidebar_width: 300.0,
            terminal_height: 320.0,
            panel_resize: None,
            git: None,
            git_poke_tx: None,
            git_commit_input: None,
            git_commit_pending: false,
        }
    }

    /// The git change entry for an absolute path, if this workspace is a
    /// git repository and the path is changed.
    pub(crate) fn git_change_for(&self, path: &Path) -> Option<(PathBuf, GitChange)> {
        let repo = self.git.as_ref()?;
        let change = repo.changes.iter().find(|c| c.path == path).cloned()?;
        Some((repo.root.clone(), change))
    }

    pub(crate) fn theme(&self) -> &'static theme::Theme {
        let themes = theme::all();
        &themes[self.theme_ix.min(themes.len() - 1)]
    }

    /// Window/title-bar text: "file ● — folder", "folder", "Settings", or the app name.
    pub(crate) fn title(&self) -> String {
        if let Some(tab) = self.tabs.get(self.active_tab) {
            if tab.is_settings {
                if self.root.is_some() {
                    format!("Settings — {}", self.root_display)
                } else {
                    "Settings — Olova Editor".to_string()
                }
            } else if let Some(path) = &tab.path {
                let star = if tab.dirty { " ●" } else { "" };
                let name = display_name(path);
                match &self.root {
                    Some(_) => format!("{name}{star} — {}", self.root_display),
                    None => format!("{name}{star}"),
                }
            } else if let Some(diff) = &tab.diff {
                let name = display_name(&diff.path);
                let label = if diff.staged { " (staged diff)" } else { " (diff)" };
                match &self.root {
                    Some(_) => format!("{name}{label} — {}", self.root_display),
                    None => format!("{name}{label}"),
                }
            } else if tab.untitled {
                let star = if tab.dirty { " ●" } else { "" };
                if self.root.is_some() {
                    format!("untitled{star} — {}", self.root_display)
                } else {
                    format!("untitled{star} — Olova Editor")
                }
            } else if self.root.is_some() {
                self.root_display.clone()
            } else {
                "Olova Editor".to_string()
            }
        } else if self.root.is_some() {
            self.root_display.clone()
        } else {
            "Olova Editor".to_string()
        }
    }

    /// Returns true if welcome screen should be shown
    pub(crate) fn welcome_visible(&self) -> bool {
        // Show welcome when no files are open
        self.tabs.is_empty()
    }

    /// Get the active tab's editor
    pub fn active_editor(&self) -> Option<&Entity<InputState>> {
        self.tabs.get(self.active_tab).and_then(|t| t.editor.as_ref())
    }

    /// Get the active tab's file path
    pub fn active_path(&self) -> Option<&PathBuf> {
        self.tabs.get(self.active_tab)?.path.as_ref()
    }

    /// Check if active tab has unsaved changes
    #[allow(dead_code)]
    pub fn is_dirty(&self) -> bool {
        self.tabs.get(self.active_tab).map(|t| t.dirty).unwrap_or(false)
    }

    // -- commands -----------------------------------------------------------

    pub(crate) fn apply_theme(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let themes = theme::all();
        let Some(th) = themes.get(ix) else {
            return;
        };
        self.theme_ix = ix;
        self.settings.workbench_color_theme = th.name.to_string();
        let _ = self.settings.save();
        // Keep the widget library (inputs, menus, scrollbars…) in sync.
        let mode = if th.appearance == "light" {
            gpui_component::ThemeMode::Light
        } else {
            gpui_component::ThemeMode::Dark
        };
        gpui_component::Theme::change(mode, Some(window), cx);
        // Paint tree-sitter captures with this theme's exact syntax palette.
        gpui_component::Theme::global_mut(cx).highlight_theme = Arc::new(th.highlight_theme());
        // Re-apply Zed fonts — Theme::change resets families from its config.
        crate::assets::sync_component_fonts(cx);
        // Keep all open terminal tabs in sync with the new theme colors.
        let palette = &th.terminal_palette;
        for tab in &self.terminal_tabs {
            tab.update(cx, |term, cx| {
                term.set_theme(palette, cx);
            });
        }
        self.status = format!("Theme: {}", th.name);
        cx.notify();
    }

    /// Rebuild the flat row cache after a tree or expansion change. This is
    /// intentionally separate from rendering: the recursive walk is paid only
    /// on actual explorer mutations, never while the editor is typing.
    fn rebuild_explorer_rows(&mut self) {
        let mut rows = Vec::new();
        if self.explorer_section_expanded {
            flatten_visible(&self.tree, 0, &mut rows);
        }
        self.explorer_rows = Arc::from(rows);
    }

    pub(crate) fn load_root(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.root = Some(path.clone());
        self.root_display = display_name(&path);
        self.root_display_shared = SharedString::from(self.root_display.clone());
        // Re-root the language servers. A server's project graph (tsconfig,
        // package.json, node_modules) is bound to the directory it was
        // started in, so opening a different folder has to restart them.
        self.lsp.lock().unwrap().set_root(Some(path.clone()));
        // Do not make opening a folder wait on a huge root directory. The
        // first level is scanned on the background executor and installed only
        // if this is still the active root when it completes.
        self.tree.clear();
        self.explorer_section_expanded = true;
        self.rebuild_explorer_rows();
        self.selected_path = None;
        self.inline_creating = None;
        self.inline_renaming = None;
        self.explorer_clipboard = None;
        self.status = format!("Loading folder {}…", self.root_display);
        let scan_root = path.clone();
        cx.spawn(async move |this, cx| {
            let entries = cx
                .background_spawn({
                    let scan_root = scan_root.clone();
                    async move { load_dir(&scan_root) }
                })
                .await;
            let _ = this.update(cx, |workspace, cx| {
                if workspace.root.as_ref() == Some(&scan_root) {
                    let previous = std::mem::take(&mut workspace.tree);
                    workspace.tree = merge_loaded_dir(&scan_root, previous, entries);
                    workspace.rebuild_explorer_rows();
                    workspace.status = format!("Opened folder {}", workspace.root_display);
                    cx.notify();
                }
            });
        })
        .detach();

        // Start filesystem watcher on the root directory. Each relevant
        // changed path is forwarded (not just a "something changed" flag) so
        // the reload on the UI side can be scoped to the affected directory.
        let tx = self.fs_event_tx.clone();
        let watcher = notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                for p in event.paths {
                    let path_str = p.to_string_lossy();
                    let relevant = !path_str.contains("target")
                        && !path_str.contains(".git")
                        && !path_str.contains(".DS_Store");
                    if relevant {
                        let _ = tx.try_send(p);
                    }
                }
            }
        });

        if let Ok(mut w) = watcher {
            let _ = w.watch(&path, notify::RecursiveMode::Recursive);
            self._watcher = Some(w);
        }

        // Git integration: when the folder is inside a repository, watch its
        // status in the background and show real changes in the source
        // control panel.
        self.start_git_watcher(&path, cx);

        cx.notify();
    }

    /// Detect a git repository for `root` and spawn a background thread that
    /// polls `git status` every ~1.5 s, forwarding snapshots to the UI only
    /// when they actually changed. `git_poke()` forces an immediate poll.
    pub(crate) fn start_git_watcher(&mut self, root: &Path, cx: &mut Context<Self>) {
        let Some(repo_root) = git::find_repo_root(root) else {
            self.git = None;
            return;
        };
        let (poke_tx, poke_rx) = std::sync::mpsc::channel::<()>();
        let (status_tx, status_rx) = async_channel::unbounded::<RepoStatus>();

        // First snapshot synchronously so the panel is populated before the
        // first frame (the watcher would otherwise be ~1.5 s late).
        if let Some(status) = git::status(&repo_root) {
            self.git = Some(status.clone());
            let _ = status_tx.try_send(status);
        }

        std::thread::spawn(move || {
            let mut last: Option<RepoStatus> = None;
            loop {
                if let Some(status) = git::status(&repo_root) {
                    if last.as_ref() != Some(&status) {
                        if status_tx.try_send(status.clone()).is_err() {
                            break; // workspace is gone
                        }
                        last = Some(status);
                    }
                }
                // Sleep until the next poll, but wake up immediately when the
                // workspace pokes us (save / commit / stage / …).
                match poke_rx.recv_timeout(Duration::from_millis(1500)) {
                    Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        self.git_poke_tx = Some(poke_tx);

        cx.spawn({
            let rx = status_rx.clone();
            async move |this, cx| {
                while let Ok(status) = rx.recv().await {
                    let _ = this.update(cx, |workspace, cx| {
                        workspace.git = Some(status);
                        // Diff tabs show a snapshot; refresh the active one so
                        // external changes (checkout, discard…) are visible.
                        workspace.refresh_active_diff(cx);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// Ask the git polling thread to re-run `git status` right now.
    pub(crate) fn git_poke(&self) {
        if let Some(tx) = &self.git_poke_tx {
            let _ = tx.send(());
        }
    }

    pub(crate) fn open_folder_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            self.load_root(path, cx);
        }
    }

    pub(crate) fn open_file_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = rfd::FileDialog::new().pick_file() {
            self.open_file(path, window, cx);
        }
    }

    pub(crate) fn new_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Create a new untitled tab
        let editor = cx.new(|cx| {
            InputState::new(window, cx)
                .code_editor("text")
                .line_number(true)
                .indent_guides(false)
                .soft_wrap(false)
                .searchable(true)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                })
                .placeholder("Start typing...")
        });

        // Subscribe to change events for the new editor
        cx.subscribe(&editor, move |this, _state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                if let Some(tab) = this.tabs.get_mut(this.active_tab) {
                    if !tab.dirty {
                        tab.dirty = true;
                        cx.notify();
                    }
                }
                let tab_idx = this.active_tab;
                this.trigger_auto_save_after_delay(tab_idx, cx);
            }
        })
        .detach();

        self.tabs.push(OpenTab {
            path: None,
            editor: Some(editor),
            dirty: false,
            untitled: true,
            preview: false, // New file tabs are permanent
            is_settings: false,
            diff: None,
        });
        self.active_tab = self.tabs.len() - 1;
        self.status = "Untitled file — Ctrl+S to save".into();
        cx.notify();
    }

    pub(crate) fn toggle_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_terminal {
            self.hide_terminal(window, cx);
            return;
        }

        self.show_terminal = true;
        // Toggle only hides once at least one terminal already exists; if the
        // user never opened one, lazily create the first tab (VS Code behavior).
        if self.terminal_tabs.is_empty() {
            self.new_terminal(window, cx);
            return;
        }
        self.focus_active_terminal(window, cx);
        self.status = "Terminal active".into();
        cx.notify();
    }

    /// Create a brand-new terminal tab (spawns a fresh shell/PTY), make it the
    /// active tab, show the panel and give it focus. Labeled `PowerShell N` so
    /// tabs stay distinguishably unique across opens/closes.
    pub(crate) fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let id = self.next_terminal_id;
        self.next_terminal_id += 1;
        let label = if cfg!(windows) {
            format!("PowerShell {id}")
        } else {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
            let base = if shell.ends_with("zsh") {
                "zsh"
            } else if shell.ends_with("fish") {
                "fish"
            } else {
                "bash"
            };
            format!("{base} {id}")
        };
        let palette = self.theme().terminal_palette.clone();
        let term = cx.new(|cx| crate::terminal::Terminal::new(root.as_deref(), label, palette, window, cx));
        self.terminal_tabs.push(term);
        self.active_terminal = self.terminal_tabs.len() - 1;
        self.show_terminal = true;
        self.focus_active_terminal(window, cx);
        self.status = format!(
            "Terminal {} created",
            self.active_terminal + 1
        );
        cx.notify();
    }

    /// Switch the active terminal tab and hand it focus (VS Code tab click).
    pub(crate) fn activate_terminal(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.terminal_tabs.len() {
            return;
        }
        self.active_terminal = index;
        if let Some(term) = self.terminal_tabs.get(index).cloned() {
            term.read(cx).focus_handle(cx).focus(window);
        }
        self.status = format!("Terminal {} active", index + 1);
        cx.notify();
    }

    /// Focus the active terminal tab's view.
    fn focus_active_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(term) = self.terminal_tabs.get(self.active_terminal).cloned() {
            term.read(cx).focus_handle(cx).focus(window);
        }
    }

    /// Hide the terminal panel and hand focus back to the active editor (or
    /// the workspace itself). Like Zed's dock toggle, focus is never left on
    /// a panel that is about to disappear, otherwise keybindings go dead.
    /// Sessions in all tabs stay alive; [`Self::close_terminal`] kills them.
    pub(crate) fn hide_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_terminal = false;
        self.status = "Terminal hidden".into();
        self.focus_active_editor_or_self(window, cx);
        cx.notify();
    }

    /// Close one terminal tab (VS Code tab ×): drops that terminal's entity,
    /// which closes its PTY and kills the child shell. Adjusts the active tab
    /// and hides the whole panel when the last tab is closed. Unlike
    /// [`Self::hide_terminal`] this fully exits the closed shell, not just the
    /// panel.
    pub(crate) fn close_terminal(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.terminal_tabs.len() {
            return;
        }
        // Dropping the entity tears down the terminal view and closes the PTY
        // master, which terminates the running shell process.
        self.terminal_tabs.remove(index);

        if self.terminal_tabs.is_empty() {
            self.active_terminal = 0;
            self.show_terminal = false;
            self.status = "Terminal closed".into();
            self.focus_active_editor_or_self(window, cx);
            cx.notify();
            return;
        }

        if self.active_terminal >= self.terminal_tabs.len() {
            self.active_terminal = self.terminal_tabs.len() - 1;
        } else if index < self.active_terminal {
            self.active_terminal -= 1;
        }
        self.focus_active_terminal(window, cx);
        self.status = format!(
            "Terminal {} closed — {} terminal(s) remain",
            index + 1,
            self.terminal_tabs.len()
        );
        cx.notify();
    }

    /// Focus the active tab's editor, falling back to the workspace focus
    /// handle when no editor is open. Keeps the dispatch path alive for
    /// global keybindings at all times.
    pub(crate) fn focus_active_editor_or_self(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self.tabs.get(self.active_tab) {
            if let Some(editor) = &tab.editor {
                let editor = editor.clone();
                editor.update(cx, |state, cx| state.focus(window, cx));
                return;
            }
        }
        window.focus(&self.focus_handle);
    }

    pub(crate) fn toggle_activity(
        &mut self,
        activity: Activity,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.show_sidebar && self.activity == activity {
            self.show_sidebar = false;
        } else {
            self.show_sidebar = true;
            self.activity = activity;
            // The source-control panel owns a commit-message input; create it
            // when the panel is first opened (needs the window handle).
            if activity == Activity::Git {
                self.ensure_git_commit_input(window, cx);
            }
        }
        self.status = self.activity.status_label().into();
        cx.notify();
    }

    pub(crate) fn set_activity_explicit(
        &mut self,
        activity: Activity,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_sidebar = true;
        self.activity = activity;
        if activity == Activity::Git {
            self.ensure_git_commit_input(window, cx);
        }
        self.status = activity.status_label().into();
        cx.notify();
    }

    /// Expands only the ancestors needed to make a path visible and queues a
    /// centered scroll. This is the same "auto reveal" behavior users expect
    /// from VS Code/Zed, without expanding unrelated branches.
    fn reveal_tree_path(&mut self, path: &Path) {
        let Some(root) = self.root.clone() else {
            return;
        };
        if !path.starts_with(&root) {
            return;
        }
        fn expand_to(nodes: &mut [TreeNode], target: &Path) {
            for node in nodes {
                if target.starts_with(&node.path) {
                    if node.is_dir {
                        node.expanded = true;
                        if !node.children_loaded {
                            node.children = load_dir(&node.path);
                            node.children_loaded = true;
                        }
                        if target != node.path {
                            expand_to(&mut node.children, target);
                        }
                    }
                    return;
                }
            }
        }
        expand_to(&mut self.tree, path);
        self.explorer_section_expanded = true;
        self.rebuild_explorer_rows();
        if let Some(index) = self
            .explorer_rows
            .iter()
            .position(|row| row.path == path)
        {
            self.explorer_scroll_handle
                .scroll_to_item(index, ScrollStrategy::Center);
        }
    }

    pub(crate) fn open_file(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_path = Some(path.clone());
        self.reveal_tree_path(&path);

        // Check if file is already open in a tab
        if let Some(idx) = self.tabs.iter().position(|t| t.path.as_ref() == Some(&path)) {
            // File already open - switch to its tab and promote preview to permanent
            self.active_tab = idx;
            // Remove preview status - it's now a permanent tab
            if let Some(tab) = self.tabs.get_mut(idx) {
                tab.preview = false;
            }
            cx.notify();
            return;
        }

        // VS Code behavior: Check if current active tab is a preview tab
        // If yes, REPLACE it with the new file (don't add a new tab)
        // If no, ADD a new tab
        let replace_preview = if let Some(active_idx) = self.tabs.get(self.active_tab) {
            // Active tab is preview AND not dirty AND is the only tab or current view
            active_idx.preview && !active_idx.dirty
        } else {
            false
        };

        // Read file content
        match std::fs::read(&path) {
            Ok(bytes) => {
                if bytes.len() > 8_000_000 {
                    self.status = format!("{} is too large (>8MB)", display_name(&path));
                    cx.notify();
                    return;
                }
                if bytes.iter().take(8000).any(|&b| b == 0) {
                    self.status = format!("{} looks binary", display_name(&path));
                    cx.notify();
                    return;
                }
                let newline_count = bytes.iter().filter(|&&b| b == b'\n').count();
                let highlight = bytes.len() <= 400_000 && newline_count <= 8_000;
                let lang_id = if highlight {
                    lang::language_for(&path).unwrap_or("text")
                } else {
                    "text"
                };
                let text = String::from_utf8_lossy(&bytes).into_owned();

                // Create a new editor for this tab
                let editor = cx.new(|cx| {
                    let mut state = InputState::new(window, cx)
                        .code_editor(lang_id)
                        .line_number(true)
                        .indent_guides(false)
                        .soft_wrap(false)
                        .searchable(true)
                        .tab_size(TabSize {
                            tab_size: 4,
                            hard_tabs: false,
                        });
                    state.set_value(text.clone(), window, cx);
                    state
                });

                // Notify LSP of document open and wire the editor up to the
                // server: completions, hover, go-to-definition and code
                // actions are driven by gpui-component's `InputState::lsp`
                // provider hooks (the same surface Zed uses). If the server
                // still has to be installed this returns false and the buffer
                // is attached automatically once the install finishes.
                self.attach_language_server(&path, lang_id, &editor, cx);

                // Subscribe to change events - also handles promoting preview to permanent
                let path_clone = path.clone();
                let lang_str = lang_id.to_string();
                let editor_ent = editor.clone();

                cx.subscribe(&editor, move |this, _state, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        let mut ui_changed = false;
                        if let Some(tab) = this.tabs.get_mut(this.active_tab) {
                            if !tab.dirty {
                                tab.dirty = true;
                                ui_changed = true;
                            }
                            // VS Code: editing a preview tab promotes it to permanent
                            if tab.preview {
                                tab.preview = false;
                                ui_changed = true;
                            }
                        }
                        {
                            let mut lsp = this.lsp.lock().unwrap();
                            if lsp.has_client(&lang_str) {
                                let text = editor_ent.read(cx).value().to_string();
                                lsp.change_document(&path_clone, &lang_str, &text);
                            }
                        }
                        // The editor view repaints itself. Only repaint the
                        // workspace chrome (dirty dot / preview promotion)
                        // when it actually changed, so steady-state typing
                        // doesn't rebuild the whole window (explorer, tab
                        // bar, status bar) on every keystroke.
                        if ui_changed {
                            cx.notify();
                        }
                        let tab_idx = this.active_tab;
                        this.trigger_auto_save_after_delay(tab_idx, cx);
                    }
                })
                .detach();

                if replace_preview {
                    // REPLACE the current preview tab with the new file
                    if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                        tab.path = Some(path.clone());
                        tab.dirty = false;
                        tab.untitled = false;
                        tab.preview = true; // New file is still in preview mode
                        tab.is_settings = false;
                        tab.editor = Some(editor);
                    }
                } else {
                    // ADD a new tab in preview mode (VS Code style)
                    self.tabs.push(OpenTab {
                        path: Some(path.clone()),
                        editor: Some(editor),
                        dirty: false,
                        untitled: false,
                        preview: true, // New tabs start as preview
                        is_settings: false,
                        diff: None,
                    });
                    self.active_tab = self.tabs.len() - 1;
                }
                self.status = if highlight {
                    path.display().to_string()
                } else {
                    format!("{} (plain text — large file)", display_name(&path))
                };
            }
            Err(e) => self.status = format!("open failed: {e}"),
        }
        cx.notify();
    }

    pub(crate) fn save(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // Get active tab
        let tab = match self.active_tab_mut() {
            Some(t) => t,
            None => {
                self.status = "No file open".into();
                cx.notify();
                return;
            }
        };

        // If settings tab, nothing to save
        if tab.is_settings {
            return;
        }

        // If untitled, fall back to Save As
        if tab.path.is_none() {
            self.save_as(cx);
            return;
        }

        // Save the file
        let path = tab.path.clone().unwrap();
        let Some(editor) = &tab.editor else {
            return;
        };
        let text = editor.read(cx).value().to_string();
        match std::fs::write(&path, text.as_bytes()) {
            Ok(()) => {
                tab.dirty = false;
                // Tell the server the file hit disk: ESLint, gopls and
                // rust-analyzer run their heavier checks on save.
                if let Some(lang_id) = lang::language_for(&path) {
                    self.lsp
                        .lock()
                        .unwrap()
                        .save_document(&path, lang_id, &text);
                }
                self.git_poke();
                if path == crate::settings::settings_file_path() {
                    self.reload_settings(cx);
                }
                self.status = format!("Saved {}", display_name(&path));
            }
            Err(e) => self.status = format!("save failed: {e}"),
        }
        cx.notify();
    }

    fn save_as(&mut self, cx: &mut Context<Self>) {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name("untitled.txt")
            .save_file()
        else {
            return;
        };

        // Get text first before mutable borrow
        let active_idx = self.active_tab;
        let text = {
            let tab = match self.tabs.get(active_idx) {
                Some(t) => t,
                None => return,
            };
            if tab.is_settings {
                return;
            }
            let Some(editor) = &tab.editor else {
                return;
            };
            editor.read(cx).value().to_string()
        };

        match std::fs::write(&path, text.as_bytes()) {
            Ok(()) => {
                let lang_id = lang::language_for(&path).unwrap_or("text");
                // Now update tab
                if let Some(tab) = self.tabs.get_mut(active_idx) {
                    tab.path = Some(path.clone());
                    tab.untitled = false;
                    tab.dirty = false;
                    tab.is_settings = false;
                    if let Some(editor) = &tab.editor {
                        editor.update(cx, |state, cx| {
                            state.set_highlighter(lang_id, cx);
                        });
                    }
                }
                // Bring the language server up for the new file and connect
                // the editor's LSP providers to it.
                if let Some(editor) = self.tabs.get(active_idx).and_then(|t| t.editor.clone()) {
                    self.attach_language_server(&path, lang_id, &editor, cx);
                }
                self.selected_path = Some(path.clone());
                if let Some(parent) = path.parent().map(|path| path.to_path_buf()) {
                    self.reload_dir(&parent, cx);
                }
                self.git_poke();
                self.status = format!("Saved {}", display_name(&path));
            }
            Err(e) => self.status = format!("save failed: {e}"),
        }
        cx.notify();
    }

    /// Get mutable reference to the active tab
    fn active_tab_mut(&mut self) -> Option<&mut OpenTab> {
        self.tabs.get_mut(self.active_tab)
    }

    pub(crate) fn toggle_dir(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.selected_path = Some(path.to_path_buf());
        fn rec(nodes: &mut [TreeNode], path: &Path) -> (bool, Option<PathBuf>) {
            for n in nodes {
                if n.path == path {
                    n.expanded = !n.expanded;
                    // Do not use `children.is_empty()` as the loaded marker:
                    // an empty folder is a valid (and cached) result.
                    let needs_load = n.expanded && !n.children_loaded;
                    return (true, needs_load.then(|| n.path.clone()));
                }
                if n.is_dir && path.starts_with(&n.path) {
                    let (found, needs_load) = rec(&mut n.children, path);
                    if found {
                        return (true, needs_load);
                    }
                }
            }
            (false, None)
        }
        let (_, dir_to_load) = rec(&mut self.tree, path);
        self.rebuild_explorer_rows();
        cx.notify();
        if let Some(dir) = dir_to_load {
            self.load_directory_async(dir, cx);
        }
    }

    pub(crate) fn refresh_explorer(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.status = "Refreshing explorer…".into();
        let mut scan_dirs = vec![root.clone()];
        fn collect_loaded_dirs(nodes: &[TreeNode], out: &mut Vec<PathBuf>) {
            for node in nodes {
                if node.is_dir && node.children_loaded {
                    out.push(node.path.clone());
                    collect_loaded_dirs(&node.children, out);
                }
            }
        }
        collect_loaded_dirs(&self.tree, &mut scan_dirs);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let scanned = cx
                .background_spawn(async move {
                    scan_dirs
                        .into_iter()
                        .map(|dir| {
                            let entries = load_dir(&dir);
                            (dir, entries)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |workspace, cx| {
                if workspace.root.as_ref() == Some(&root) {
                    let mut changed = false;
                    for (dir, entries) in scanned {
                        changed |= workspace.apply_loaded_dir_inner(&dir, entries);
                    }
                    if changed {
                        workspace.rebuild_explorer_rows();
                    }
                    workspace.status = "Explorer refreshed".into();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Merge a directory snapshot produced by the background scanner.
    fn apply_loaded_dir_inner(&mut self, dir: &Path, entries: Vec<TreeNode>) -> bool {
        let Some(root) = self.root.clone() else {
            return false;
        };
        if dir == root.as_path() {
            let previous = std::mem::take(&mut self.tree);
            self.tree = merge_loaded_dir(dir, previous, entries);
            return true;
        }

        fn apply(
            nodes: &mut [TreeNode],
            dir: &Path,
            entries: &mut Option<Vec<TreeNode>>,
        ) -> bool {
            for node in nodes {
                if node.path == dir {
                    if node.is_dir && node.expanded {
                        let previous = std::mem::take(&mut node.children);
                        node.children = merge_loaded_dir(
                            dir,
                            previous,
                            entries.take().unwrap_or_default(),
                        );
                        node.children_loaded = true;
                    } else if node.is_dir && node.children_loaded {
                        node.children.clear();
                        node.children_loaded = false;
                    }
                    return true;
                }
                if node.is_dir
                    && dir.starts_with(&node.path)
                    && apply(&mut node.children, dir, entries)
                {
                    return true;
                }
            }
            false
        }

        let mut entries = Some(entries);
        apply(&mut self.tree, dir, &mut entries)
    }

    /// Scan a directory off the UI thread and merge it if it still belongs to
    /// the active workspace. This is shared by lazy expansion and explicit
    /// refreshes/mutations so a large directory never blocks a click.
    fn load_directory_async(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let entries = cx
                .background_spawn({
                    let dir = dir.clone();
                    async move { load_dir(&dir) }
                })
                .await;
            let _ = this.update(cx, |workspace, cx| {
                if workspace.root.as_ref() == Some(&root)
                    && workspace.apply_loaded_dir_inner(&dir, entries)
                {
                    workspace.rebuild_explorer_rows();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Reload one affected directory level after a filesystem change without
    /// blocking the UI thread. A collapsed directory is intentionally not
    /// scanned into the tree; it is marked stale and will be loaded fresh when
    /// the user expands it. An expanded directory gets a shallow merge, so
    /// loaded grandchildren move across unchanged entries without being
    /// rescanned.
    pub(crate) fn reload_dir(&mut self, dir: &Path, cx: &mut Context<Self>) {
        self.load_directory_async(dir.to_path_buf(), cx);
    }

    pub(crate) fn collapse_all_folders(&mut self, cx: &mut Context<Self>) {
        collapse_all(&mut self.tree);
        self.rebuild_explorer_rows();
        self.status = "Collapsed all folders".into();
        cx.notify();
    }

    pub(crate) fn toggle_explorer_section(&mut self, cx: &mut Context<Self>) {
        self.explorer_section_expanded = !self.explorer_section_expanded;
        self.rebuild_explorer_rows();
        cx.notify();
    }

    /// Implements the small, high-value part of Zed/VS Code explorer
    /// navigation without forcing the editor to take focus: arrows move the
    /// cached visible-row selection, right/left expand and collapse, and
    /// Enter opens/toggles. Selection is always scrolled into view by the
    /// same uniform-list handle used by mouse navigation.
    pub(crate) fn handle_explorer_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        // The input entity owns text editing while an inline editor is active.
        // Do not let the list interpret Enter, arrows, or Delete underneath it.
        if self.inline_creating.is_some() || self.inline_renaming.is_some() {
            return;
        }

        if event.keystroke.modifiers.control {
            match (key, event.keystroke.modifiers.shift) {
                ("c", false) => self.explorer_copy(cx),
                ("x", false) => self.explorer_cut(cx),
                ("v", false) => self.explorer_paste(cx),
                ("n", true) => {
                    let selected_folder = self
                        .selected_path
                        .clone()
                        .filter(|path| path.is_dir());
                    self.start_inline_create(CreatingKind::Folder, selected_folder, window, cx);
                },
                _ => return,
            }
            cx.stop_propagation();
            return;
        }

        let rows = Arc::clone(&self.explorer_rows);
        let current = self
            .selected_path
            .as_ref()
            .and_then(|selected| rows.iter().position(|row| &row.path == selected));
        if rows.is_empty() {
            return;
        }
        if matches!(key, "f2") {
            if let Some(path) = self.selected_path.clone() {
                self.start_inline_rename(path, window, cx);
                cx.stop_propagation();
            }
            return;
        }
        if matches!(key, "delete" | "backspace") {
            if let Some(path) = self.selected_path.clone() {
                self.delete_entry(&path, cx);
                cx.stop_propagation();
            }
            return;
        }
        let current_ix = current.unwrap_or(0);

        match key {
            "arrowdown" | "down" => {
                let next = (current_ix + 1).min(rows.len() - 1);
                self.select_explorer_index(next, cx);
            }
            "arrowup" | "up" => {
                self.select_explorer_index(current_ix.saturating_sub(1), cx);
            }
            "home" => self.select_explorer_index(0, cx),
            "end" => self.select_explorer_index(rows.len() - 1, cx),
            "arrowright" | "right" => {
                let row = &rows[current_ix];
                if row.is_dir && !row.expanded {
                    let path = row.path.clone();
                    self.toggle_dir(&path, cx);
                } else if row.is_dir {
                    if let Some(next) = rows.get(current_ix + 1) {
                        if next.depth > row.depth {
                            self.select_explorer_index(current_ix + 1, cx);
                        }
                    }
                }
            }
            "arrowleft" | "left" => {
                let row = &rows[current_ix];
                if row.is_dir && row.expanded {
                    let path = row.path.clone();
                    self.toggle_dir(&path, cx);
                } else if row.depth > 0 {
                    if let Some(parent_ix) = (0..current_ix)
                        .rev()
                        .find(|&ix| rows[ix].depth < row.depth)
                    {
                        self.select_explorer_index(parent_ix, cx);
                    }
                }
            }
            "enter" => {
                let path = rows[current_ix].path.clone();
                if rows[current_ix].is_dir {
                    self.toggle_dir(&path, cx);
                } else {
                    self.open_file(path, window, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn select_explorer_index(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.explorer_rows.get(index) else {
            return;
        };
        self.selected_path = Some(row.path.clone());
        self.explorer_scroll_handle
            .scroll_to_item(index, ScrollStrategy::Center);
        cx.notify();
    }

    fn path_in_workspace(&self, path: &Path) -> bool {
        self.root
            .as_deref()
            .is_some_and(|root| is_same_or_descendant(root, path))
    }

    /// Expand all loaded ancestors of a directory. If an ancestor has not
    /// been opened yet, load its first level now so a new item has a visible
    /// insertion point. Directory scans are shallow and are only performed for
    /// the path the user is actively editing.
    fn ensure_directory_visible(&mut self, dir: &Path) {
        let Some(root) = self.root.clone() else {
            return;
        };
        if dir == root {
            return;
        }

        fn expand(nodes: &mut [TreeNode], target: &Path) {
            for node in nodes {
                if target.starts_with(&node.path) {
                    if node.is_dir {
                        node.expanded = true;
                        if !node.children_loaded {
                            node.children = load_dir(&node.path);
                            node.children_loaded = true;
                        }
                        expand(&mut node.children, target);
                    }
                    return;
                }
            }
        }
        expand(&mut self.tree, dir);
    }

    pub(crate) fn start_inline_create_at_root(
        &mut self,
        kind: CreatingKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = self.root.clone();
        self.start_inline_create(kind, root, window, cx);
    }

    pub(crate) fn start_inline_create(
        &mut self,
        kind: CreatingKind,
        parent: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Starting another edit commits neither operation. This mirrors the
        // explorer's one-inline-editor rule and avoids two recycled list rows
        // sharing focus.
        self.inline_creating = None;
        self.inline_renaming = None;

        let target_dir = parent
            .or_else(|| {
                self.selected_path.as_ref().map(|p| {
                    if p.is_dir() {
                        p.clone()
                    } else {
                        p.parent().unwrap_or(p).to_path_buf()
                    }
                })
            })
            .or_else(|| self.root.clone());
        let Some(dir) = target_dir else {
            if kind == CreatingKind::File {
                self.new_file(window, cx);
            } else {
                self.status = "Open a folder before creating a directory".into();
                cx.notify();
            }
            return;
        };
        if !dir.is_dir() || !self.path_in_workspace(&dir) {
            self.status = "The selected destination folder is unavailable".into();
            cx.notify();
            return;
        }

        self.selected_path = Some(dir.clone());
        self.ensure_directory_visible(&dir);
        self.explorer_section_expanded = true;
        self.rebuild_explorer_rows();

        let input = cx.new(|cx| {
            let state = InputState::new(window, cx);
            state.focus(window, cx);
            state
        });
        cx.subscribe(&input, |this, _state, event: &InputEvent, cx| {
            match event {
                InputEvent::PressEnter { .. } => this.confirm_inline_create(cx),
                InputEvent::Blur => this.cancel_inline_create(cx),
                _ => {}
            }
        })
        .detach();

        self.inline_creating = Some(InlineCreating {
            kind,
            parent_dir: dir,
            input,
        });
        cx.notify();
    }

    pub(crate) fn confirm_inline_create(&mut self, cx: &mut Context<Self>) {
        let Some(creating) = self.inline_creating.take() else {
            return;
        };
        let raw_name = creating.input.read(cx).value().to_string();
        let name = raw_name.trim();
        let target_path = creating.parent_dir.join(name);
        let name_is_valid = valid_entry_name(name);
        let target_exists = target_path.exists();
        let invalid = !name_is_valid
            || !self.path_in_workspace(&target_path)
            || target_exists;
        if invalid {
            self.status = if !name_is_valid || !self.path_in_workspace(&target_path) {
                "Enter a valid name (without path separators)".into()
            } else if target_exists {
                format!("{} already exists", display_name(&target_path))
            } else {
                "Could not create that item here".into()
            };
            self.inline_creating = Some(creating);
            cx.notify();
            return;
        }

        let result = match creating.kind {
            CreatingKind::File => std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target_path)
                .map(|_| ()),
            CreatingKind::Folder => std::fs::create_dir(&target_path),
        };
        match result {
            Ok(()) => {
                self.selected_path = Some(target_path.clone());
                self.ensure_directory_visible(&creating.parent_dir);
                self.rebuild_explorer_rows();
                self.reload_dir(&creating.parent_dir, cx);
                if creating.kind == CreatingKind::File {
                    self.pending_open = Some(target_path.clone());
                }
                self.status = if creating.kind == CreatingKind::File {
                    format!("Created {}", display_name(&target_path))
                } else {
                    format!("Created folder {}", display_name(&target_path))
                };
            }
            Err(error) => {
                self.status = format!("Could not create {}: {error}", display_name(&target_path));
                self.inline_creating = Some(creating);
            }
        }
        cx.notify();
    }

    pub(crate) fn cancel_inline_create(&mut self, cx: &mut Context<Self>) {
        if self.inline_creating.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn start_inline_rename(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.path_in_workspace(&path) || !path.exists() {
            return;
        }
        self.inline_creating = None;
        self.inline_renaming = None;
        self.selected_path = Some(path.clone());
        self.reveal_tree_path(&path);

        let name = display_name(&path);
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).default_value(name);
            state.select_all_text(cx);
            state.focus(window, cx);
            state
        });
        cx.subscribe(&input, |this, _state, event: &InputEvent, cx| {
            match event {
                InputEvent::PressEnter { .. } => this.confirm_inline_rename(cx),
                InputEvent::Blur => this.cancel_inline_rename(cx),
                _ => {}
            }
        })
        .detach();
        self.inline_renaming = Some(InlineRenaming { path, input });
        cx.notify();
    }

    pub(crate) fn confirm_inline_rename(&mut self, cx: &mut Context<Self>) {
        let Some(renaming) = self.inline_renaming.take() else {
            return;
        };
        let raw_name = renaming.input.read(cx).value().to_string();
        let name = raw_name.trim();
        let Some(parent) = renaming.path.parent() else {
            return;
        };
        let destination = parent.join(name);
        if !valid_entry_name(name) || !self.path_in_workspace(&destination) {
            self.status = "Enter a valid name (without path separators)".into();
            self.inline_renaming = Some(renaming);
            cx.notify();
            return;
        }
        if destination == renaming.path {
            self.selected_path = Some(destination);
            cx.notify();
            return;
        }
        if destination.exists() {
            self.status = format!("{} already exists", display_name(&destination));
            self.inline_renaming = Some(renaming);
            cx.notify();
            return;
        }

        match std::fs::rename(&renaming.path, &destination) {
            Ok(()) => {
                self.update_paths_after_move(&renaming.path, &destination);
                if renaming.path.parent() == destination.parent() {
                    self.rewrite_tree_path(&renaming.path, &destination);
                }
                if let Some(old_parent) = renaming.path.parent().map(Path::to_path_buf) {
                    self.reload_dir(&old_parent, cx);
                }
                self.status = format!("Renamed to {}", display_name(&destination));
                self.git_poke();
            }
            Err(error) => {
                self.status = format!("Could not rename: {error}");
                self.inline_renaming = Some(renaming);
            }
        }
        cx.notify();
    }

    pub(crate) fn cancel_inline_rename(&mut self, cx: &mut Context<Self>) {
        if self.inline_renaming.take().is_some() {
            cx.notify();
        }
    }

    fn rewrite_tree_path(&mut self, source: &Path, destination: &Path) {
        fn rewrite(nodes: &mut [TreeNode], source: &Path, destination: &Path) {
            for node in nodes {
                if let Some(updated) = path_after_move(&node.path, source, destination) {
                    node.path = updated;
                    node.name = display_name(&node.path);
                    rewrite(&mut node.children, source, destination);
                }
            }
        }
        rewrite(&mut self.tree, source, destination);
        self.rebuild_explorer_rows();
    }

    /// Update editor tabs, diagnostics and selection for a move or directory
    /// rename. `path_after_move` is component-aware, so `src2/file` is not
    /// rewritten when only `src` moved.
    fn update_paths_after_move(&mut self, source: &Path, destination: &Path) {
        let workspace_root = self.root.clone();
        for tab in &mut self.tabs {
            if let Some(path) = tab.path.as_ref() {
                if let Some(updated) = path_after_move(path, source, destination) {
                    tab.path = Some(updated);
                }
            }
            if let Some(diff) = tab.diff.as_mut() {
                if let Some(updated) = path_after_move(&diff.path, source, destination) {
                    diff.path = updated;
                    if let Some(root) = workspace_root.as_ref() {
                        diff.rel = diff
                            .path
                            .strip_prefix(root)
                            .unwrap_or(&diff.path)
                            .to_string_lossy()
                            .into_owned();
                    }
                }
            }
        }
        let diagnostics = std::mem::take(&mut self.diagnostics_by_path);
        self.diagnostics_by_path = diagnostics
            .into_iter()
            .map(|(path, value)| {
                (path_after_move(&path, source, destination).unwrap_or(path), value)
            })
            .collect();
        if let Some(path) = self.selected_path.as_ref() {
            if let Some(updated) = path_after_move(path, source, destination) {
                self.selected_path = Some(updated);
            }
        }
        if let Some(path) = self.pending_open.as_ref() {
            if let Some(updated) = path_after_move(path, source, destination) {
                self.pending_open = Some(updated);
            }
        }
    }

    /// Move a dropped explorer entry into `destination_dir`.
    pub(crate) fn move_entry(
        &mut self,
        source: &Path,
        destination_dir: &Path,
        cx: &mut Context<Self>,
    ) -> bool {
        let source = source.to_path_buf();
        let destination_dir = destination_dir.to_path_buf();
        if !self.path_in_workspace(&source)
            || !self.path_in_workspace(&destination_dir)
            || !destination_dir.is_dir()
            || !source.exists()
        {
            self.status = "The drag source or destination is unavailable".into();
            cx.notify();
            return false;
        }
        if is_same_or_descendant(&source, &destination_dir) {
            self.status = "A folder cannot be moved into itself".into();
            cx.notify();
            return false;
        }
        let Some(name) = source.file_name() else {
            return false;
        };
        let destination = destination_dir.join(name);
        if destination == source {
            self.status = "The item is already in that folder".into();
            cx.notify();
            return false;
        }
        if destination.exists() {
            self.status = format!("{} already exists there", display_name(&destination));
            cx.notify();
            return false;
        }
        let old_parent = source.parent().map(Path::to_path_buf);
        let moved = match std::fs::rename(&source, &destination) {
            Ok(()) => {
                self.update_paths_after_move(&source, &destination);
                self.selected_path = Some(destination.clone());
                if let Some(parent) = old_parent {
                    self.reload_dir(&parent, cx);
                }
                self.reload_dir(&destination_dir, cx);
                self.status = format!("Moved {}", display_name(&destination));
                self.git_poke();
                true
            }
            Err(error) => {
                self.status = format!("Could not move: {error}");
                false
            }
        };
        cx.notify();
        moved
    }

    fn copy_entry_recursive(source: &Path, destination: &Path) -> std::io::Result<()> {
        if source.is_dir() {
            std::fs::create_dir(destination)?;
            for entry in std::fs::read_dir(source)? {
                let entry = entry?;
                Self::copy_entry_recursive(&entry.path(), &destination.join(entry.file_name()))?;
            }
        } else {
            std::fs::copy(source, destination)?;
        }
        Ok(())
    }

    pub(crate) fn explorer_copy(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected_path.clone() else {
            return;
        };
        self.explorer_clipboard = Some(ExplorerClipboard { path, cut: false });
        self.status = "Copied explorer item".into();
        cx.notify();
    }

    pub(crate) fn explorer_cut(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.selected_path.clone() else {
            return;
        };
        self.explorer_clipboard = Some(ExplorerClipboard { path, cut: true });
        self.status = "Cut explorer item".into();
        cx.notify();
    }

    pub(crate) fn explorer_paste(&mut self, cx: &mut Context<Self>) {
        let Some(clipboard) = self.explorer_clipboard.clone() else {
            return;
        };
        let destination_dir = self
            .selected_path
            .as_ref()
            .filter(|path| path.is_dir())
            .cloned()
            .or_else(|| {
                self.selected_path
                    .as_ref()
                    .and_then(|path| path.parent().map(Path::to_path_buf))
            })
            .or_else(|| self.root.clone());
        let Some(destination_dir) = destination_dir else {
            return;
        };
        if clipboard.cut {
            if self.move_entry(&clipboard.path, &destination_dir, cx) {
                self.explorer_clipboard = None;
            }
            return;
        }
        if !self.path_in_workspace(&clipboard.path) || !clipboard.path.exists() {
            self.status = "Cannot paste: the copied item is no longer available".into();
            cx.notify();
            return;
        }
        let Some(name) = clipboard.path.file_name() else {
            return;
        };
        let destination = destination_dir.join(name);
        if destination.exists() || is_same_or_descendant(&clipboard.path, &destination_dir) {
            self.status = "Cannot paste: the destination already contains that item".into();
            cx.notify();
            return;
        }
        match Self::copy_entry_recursive(&clipboard.path, &destination) {
            Ok(()) => {
                self.reload_dir(&destination_dir, cx);
                self.selected_path = Some(destination);
                self.status = "Pasted explorer item".into();
            }
            Err(error) => {
                // Do not leave a half-copied folder in the tree after a
                // permissions or I/O failure.
                if destination.is_dir() {
                    let _ = std::fs::remove_dir_all(&destination);
                } else {
                    let _ = std::fs::remove_file(&destination);
                }
                self.status = format!("Could not paste: {error}");
            }
        }
        cx.notify();
    }

    pub(crate) fn reveal_in_explorer(&mut self, path: &Path, cx: &mut Context<Self>) {
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("explorer")
                .arg(format!("/select,{}", path.display()))
                .spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open")
                .arg("-R")
                .arg(path)
                .spawn();
        }
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("xdg-open")
                .arg(path.parent().unwrap_or(path))
                .spawn();
        }
        self.status = format!("Revealed {}", display_name(path));
        cx.notify();
    }

    pub(crate) fn copy_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.selected_path = Some(path.to_path_buf());
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(path.to_string_lossy().to_string()));
        self.status = format!("Copied path: {}", path.display());
        cx.notify();
    }

    pub(crate) fn copy_relative_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        let rel = if let Some(root) = &self.root {
            path.strip_prefix(root).unwrap_or(path)
        } else {
            path
        };
        self.selected_path = Some(path.to_path_buf());
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(rel.to_string_lossy().to_string()));
        self.status = format!("Copied relative path: {}", rel.display());
        cx.notify();
    }

    pub(crate) fn delete_entry(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.path_in_workspace(path) || !path.exists() {
            return;
        }
        let is_dir = path.is_dir();
        let result = if is_dir {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        match result {
            Ok(()) => {
                let active_editor_path = self
                    .tabs
                    .get(self.active_tab)
                    .and_then(|tab| tab.path.clone());
                let active_diff_path = self
                    .tabs
                    .get(self.active_tab)
                    .and_then(|tab| tab.diff.as_ref().map(|diff| diff.path.clone()));
                self.tabs.retain(|tab| {
                    let editor_path_is_alive = tab
                        .path
                        .as_ref()
                        .map_or(true, |tab_path| !is_same_or_descendant(path, tab_path));
                    let diff_path_is_alive = tab
                        .diff
                        .as_ref()
                        .map_or(true, |diff| !is_same_or_descendant(path, &diff.path));
                    editor_path_is_alive && diff_path_is_alive
                });
                self.diagnostics_by_path
                    .retain(|tab_path, _| !is_same_or_descendant(path, tab_path));
                if self.selected_path.as_ref().is_some_and(|selected| is_same_or_descendant(path, selected)) {
                    self.selected_path = path.parent().map(Path::to_path_buf);
                }
                if self.pending_open.as_ref().is_some_and(|pending| is_same_or_descendant(path, pending)) {
                    self.pending_open = None;
                }
                self.active_tab = active_editor_path
                    .as_ref()
                    .and_then(|active| {
                        self.tabs
                            .iter()
                            .position(|tab| tab.path.as_ref() == Some(active))
                    })
                    .or_else(|| {
                        active_diff_path.as_ref().and_then(|active| {
                            self.tabs.iter().position(|tab| {
                                tab.diff.as_ref().is_some_and(|diff| &diff.path == active)
                            })
                        })
                    })
                    .unwrap_or_else(|| self.active_tab.min(self.tabs.len().saturating_sub(1)));
                if let Some(parent) = path.parent().map(Path::to_path_buf) {
                    self.reload_dir(&parent, cx);
                }
                self.git_poke();
                self.status = format!("Deleted {}", display_name(path));
            }
            Err(error) => self.status = format!("Failed to delete: {error}"),
        }
        cx.notify();
    }

    /// Bring a language server up for `path` and connect `editor` to it.
    ///
    /// This is the single entry point every code path uses (open file, save
    /// as, post-install retry), so a buffer can never end up half-wired —
    /// e.g. with diagnostics arriving but no completion provider attached.
    ///
    /// Returns `false` when no server is available *yet*; the manager will
    /// have started an install if one is possible, and the buffer gets picked
    /// up by [`Self::start_server_for_open_buffers`] once it finishes.
    pub(crate) fn attach_language_server(
        &mut self,
        path: &Path,
        lang_id: &str,
        editor: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) -> bool {
        let root = self.root.clone();
        let client = {
            let mut lsp = self.lsp.lock().unwrap();
            lsp.ensure_server(lang_id, root.as_deref())
        };
        let Some(client) = client else {
            return false;
        };

        let text = editor.read(cx).value().to_string();
        client.did_open(path, lang_id, &text);

        let attach_client = client.clone();
        let lsp_path = path.to_path_buf();
        editor.update(cx, move |state, _cx| {
            crate::lsp::attach_lsp_providers(state, attach_client, lsp_path);
        });
        true
    }

    /// After a server finishes installing, open every buffer it handles.
    ///
    /// Without this the user would have to close and reopen the file they
    /// were already looking at to get diagnostics — the exact papercut Zed
    /// avoids by re-registering buffers when a server starts.
    pub(crate) fn start_server_for_open_buffers(
        &mut self,
        server: &str,
        cx: &mut Context<Self>,
    ) {
        let languages: Vec<&'static str> = self
            .lsp
            .lock()
            .unwrap()
            .languages_for_server(server)
            .to_vec();

        // Collect first: attaching borrows `self` mutably.
        let targets: Vec<(PathBuf, &'static str, Entity<InputState>)> = self
            .tabs
            .iter()
            .filter_map(|tab| {
                let path = tab.path.clone()?;
                let editor = tab.editor.clone()?;
                let lang = lang::language_for(&path)?;
                languages.contains(&lang).then_some((path, lang, editor))
            })
            .collect();

        for (path, lang, editor) in targets {
            self.attach_language_server(&path, lang, &editor, cx);
        }
    }

    pub(crate) fn apply_diagnostics(
        &mut self,
        path: &Path,
        diagnostics: Vec<lsp_types::Diagnostic>,
        cx: &mut Context<Self>,
    ) {
        // Wrap once: storing the bundle and copying it into each matching
        // editor both borrow from the same shared `Arc` payload, so an LSP
        // publish no longer full-clones the vector at two independent sites.
        let shared = Arc::new(diagnostics);

        self.diagnostics_by_path
            .insert(path.to_path_buf(), Arc::clone(&shared));

        let mut updated = false;
        let mut active_msg = None;
        let active_tab_idx = self.active_tab;

        for (idx, tab) in self.tabs.iter_mut().enumerate() {
            if let Some(tab_path) = &tab.path {
                if crate::lsp::paths_match(tab_path, path) {
                    if let Some(editor) = &tab.editor {
                        editor.update(cx, |state, _cx| {
                            if let Some(diag_set) = state.diagnostics_mut() {
                                diag_set.clear();
                                for d in shared.iter() {
                                    diag_set.push(d.clone());
                                }
                                updated = true;
                                if idx == active_tab_idx {
                                    if let Some(first) = shared.first() {
                                        active_msg = Some(first.message.clone());
                                    }
                                }
                            }
                        });
                    }
                }
            }
        }

        if let Some(msg) = active_msg {
            self.status = format!("Problem: {} (Ctrl+Alt+C to copy)", msg);
        }

        if updated {
            cx.notify();
        }
    }

    pub(crate) fn copy_active_diagnostic(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(self.active_tab) {
            if let Some(tab_path) = &tab.path {
                for (p, diags) in &self.diagnostics_by_path {
                    if crate::lsp::paths_match(p, tab_path) && !diags.is_empty() {
                        let msg = diags
                            .iter()
                            .map(|d| format!("{}: {}", d.source.as_deref().unwrap_or("error"), d.message))
                            .collect::<Vec<_>>()
                            .join("\n");
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(msg));
                        self.status = format!("Copied: {}", diags[0].message);
                        cx.notify();
                        return;
                    }
                }
            }
        }
        self.status = "No active problem to copy".into();
        cx.notify();
    }

    /// Open or focus the VS Code-style Settings tab
    // -- Git -----------------------------------------------------------------

    /// Refresh the git status immediately (used by the panel's refresh
    /// button and after external git operations).
    pub(crate) fn git_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.git.as_ref().map(|g| g.root.clone()) else {
            self.status = "Not a git repository".into();
            cx.notify();
            return;
        };
        self.status = "Refreshing source control…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let status = cx.background_spawn(async move { git::status(&root) }).await;
            let _ = this.update(cx, |workspace, cx| {
                match status {
                    Some(status) => {
                        workspace.git = Some(status);
                        workspace.status = "Source control refreshed".into();
                    }
                    None => workspace.status = "git status failed".into(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Stage one changed file.
    pub(crate) fn git_stage_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some((root, change)) = self.git_change_for(path) else {
            self.status = "Not a changed file".into();
            cx.notify();
            return;
        };
        self.run_git_op(
            root,
            vec![change.rel],
            move |root, rels| git::stage(&root, &rels),
            "Staged {}",
            cx,
        );
    }

    /// Unstage one file.
    pub(crate) fn git_unstage_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some((root, change)) = self.git_change_for(path) else {
            self.status = "Not a changed file".into();
            cx.notify();
            return;
        };
        self.run_git_op(
            root,
            vec![change.rel],
            move |root, rels| git::unstage(&root, &rels),
            "Unstaged {}",
            cx,
        );
    }

    /// Discard all worktree changes (or delete, when untracked) of one file.
    pub(crate) fn git_discard_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some((root, change)) = self.git_change_for(path) else {
            self.status = "Not a changed file".into();
            cx.notify();
            return;
        };
        let untracked = change.is_untracked();
        self.run_git_op(
            root,
            vec![change.rel],
            move |root, rels| {
                if untracked {
                    git::discard_untracked(&root, &rels)
                } else {
                    git::discard(&root, &rels)
                }
            },
            "Discarded changes in {}",
            cx,
        );
    }

    /// Stage every change.
    pub(crate) fn git_stage_all(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.git.as_ref().map(|g| g.root.clone()) else {
            self.status = "Not a git repository".into();
            cx.notify();
            return;
        };
        self.run_git_op(root, Vec::new(), |root, _| git::stage_all(&root), "Staged all changes", cx);
    }

    /// Unstage every change.
    pub(crate) fn git_unstage_all(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.git.as_ref().map(|g| g.root.clone()) else {
            self.status = "Not a git repository".into();
            cx.notify();
            return;
        };
        let rels: Vec<String> = self
            .git
            .as_ref()
            .map(|g| g.changes.iter().filter(|c| c.is_staged()).map(|c| c.rel.clone()).collect())
            .unwrap_or_default();
        if rels.is_empty() {
            self.status = "Nothing staged".into();
            cx.notify();
            return;
        }
        self.run_git_op(
            root,
            rels,
            |root, rels| git::unstage(&root, &rels),
            "Unstaged all changes",
            cx,
        );
    }

    /// Discard every worktree change (untracked files are deleted).
    pub(crate) fn git_discard_all(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.git.as_ref().map(|g| g.root.clone()) else {
            self.status = "Not a git repository".into();
            cx.notify();
            return;
        };
        let (tracked, untracked) = self
            .git
            .as_ref()
            .map(|g| {
                g.changes
                    .iter()
                    .filter(|c| !c.is_staged() || c.worktree.is_some() || c.is_untracked())
                    .partition::<Vec<_>, _>(|c| !c.is_untracked())
            })
            .unwrap_or_default();
        let tracked: Vec<String> = tracked.iter().map(|c| c.rel.clone()).collect();
        let untracked: Vec<String> = untracked.iter().map(|c| c.rel.clone()).collect();
        self.run_git_op(
            root,
            tracked,
            move |root, rels| {
                if !rels.is_empty() && !git::discard(&root, &rels) {
                    return false;
                }
                if !untracked.is_empty() {
                    return git::discard_untracked(&root, &untracked);
                }
                true
            },
            "Discarded all changes",
            cx,
        );
    }

    /// Run one git mutation on a background thread, then poke the watcher so
    /// the panel reflects the new status immediately.
    fn run_git_op(
        &mut self,
        root: PathBuf,
        rels: Vec<String>,
        op: impl FnOnce(PathBuf, Vec<String>) -> bool + Send + 'static,
        success: &'static str,
        cx: &mut Context<Self>,
    ) {
        let rel_name = rels.first().cloned().unwrap_or_default();
        self.status = "Working…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let ok = cx.background_spawn(async move { op(root, rels) }).await;
            let _ = this.update(cx, |workspace, cx| {
                if ok {
                    if success.contains("{}") {
                        workspace.status = success.replace("{}", &rel_name);
                    } else {
                        workspace.status = success.to_string();
                    }
                    workspace.git_poke();
                } else {
                    workspace.status = "Git operation failed".into();
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn toggle_git_repo_section(&mut self, cx: &mut Context<Self>) {
        self.git_repo_section_expanded = !self.git_repo_section_expanded;
        cx.notify();
    }

    pub(crate) fn toggle_git_staged_section(&mut self, cx: &mut Context<Self>) {
        self.git_staged_expanded = !self.git_staged_expanded;
        cx.notify();
    }

    pub(crate) fn toggle_git_changes_section(&mut self, cx: &mut Context<Self>) {
        self.git_changes_expanded = !self.git_changes_expanded;
        cx.notify();
    }

    pub(crate) fn toggle_split_diff(&mut self, cx: &mut Context<Self>) {
        self.split_diff = !self.split_diff;
        cx.notify();
    }

    /// The commit-message input of the source-control panel. Created once on
    /// first use; Enter (or the Commit button) commits the staged changes.
    pub(crate) fn ensure_git_commit_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        let branch = self
            .git
            .as_ref()
            .and_then(|g| g.branch.as_deref())
            .unwrap_or("main");
        let placeholder_text = format!("Message (Ctrl+Enter to commit on \"{branch}\"...)");
        if let Some(input) = &self.git_commit_input {
            return input.clone();
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder_text)
        });
        // Enter in the commit box commits. The subscribe callback has no
        // window handle, so it only flags the request; render() (which does
        // have the window) runs the commit on the next frame.
        cx.subscribe(&input, |this, _state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.git_commit_pending = true;
                cx.notify();
            }
        })
        .detach();
        self.git_commit_input = Some(input.clone());
        input
    }

    /// Commit the staged changes with the message from the commit box.
    pub(crate) fn git_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.git.as_ref().map(|g| g.root.clone()) else {
            self.status = "Not a git repository — open a folder to commit".into();
            cx.notify();
            return;
        };
        let staged_count = self.git.as_ref().map(|g| g.staged_count()).unwrap_or(0);
        if staged_count == 0 {
            let changed = self.git.as_ref().map(|g| g.change_count()).unwrap_or(0);
            self.status = if changed > 0 {
                "Nothing staged — use + on a file or 'stage all' first".into()
            } else {
                "Nothing to commit".into()
            };
            cx.notify();
            return;
        }
        let message = self
            .git_commit_input
            .as_ref()
            .map(|i| i.read(cx).value().trim().to_string())
            .unwrap_or_default();
        if message.is_empty() {
            self.status = "Commit message is empty".into();
            cx.notify();
            return;
        }

        self.status = "Committing…".into();
        cx.notify();
        let root = root.clone();
        let message = message.clone();
        let commit_input = self.git_commit_input.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { git::commit(&root, &message) })
                .await;
            let is_ok = result.is_ok();
            let status = match result {
                Ok(summary) => format!("Committed: {summary}"),
                Err(e) => format!("Commit failed: {e}"),
            };
            let _ = this.update(cx, |workspace, cx| {
                workspace.status = status.into();
                if is_ok {
                    workspace.git_poke();
                }
                cx.notify();
            });
            // Clear the commit input on success. `set_value` needs the window
            // handle, so this runs through the async window context.
            if is_ok {
                if let Some(input) = &commit_input {
                    let _ = input.downgrade().update_in(cx, |state, window, cx| {
                        state.set_value("", window, cx);
                    });
                }
            }
        })
        .detach();
    }

    /// Open the diff of a changed file in a new editor tab. The diff text is
    /// produced on a background thread; the tab shows a spinner meanwhile.
    pub(crate) fn open_diff(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some((root, change)) = self.git_change_for(path) else {
            self.status = "Not a changed file".into();
            cx.notify();
            return;
        };
        let staged = change.is_staged();

        // Already open? Just switch to it.
        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| t.diff.as_ref().map(|d| d.path == path && d.staged == staged) == Some(true))
        {
            self.active_tab = idx;
            cx.notify();
            return;
        }

        let rel = change.rel.clone();
        let diff_tab = DiffTab {
            path: path.to_path_buf(),
            rel: rel.clone(),
            staged,
            text: None,
            error: None,
        };
        self.tabs.push(OpenTab {
            path: None,
            editor: None,
            dirty: false,
            untitled: false,
            preview: false,
            is_settings: false,
            diff: Some(diff_tab),
        });
        self.active_tab = self.tabs.len() - 1;
        self.status = if staged {
            format!("Diff (staged): {}", display_name(path))
        } else {
            format!("Diff: {}", display_name(path))
        };

        // Load the diff text in the background.
        let tab_path = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let tab_path_bg = tab_path.clone();
            let text = cx
                .background_spawn(async move {
                    let raw = git::diff(&root, &rel, staged).unwrap_or_default();
                    if !raw.trim().is_empty() {
                        Some(raw)
                    } else {
                        // Untracked files have no git diff yet — show the
                        // full content as one big addition.
                        std::fs::read_to_string(&tab_path_bg)
                            .ok()
                            .map(|content| git::new_file_diff(&rel, &content))
                    }
                })
                .await;
            let _ = this.update(cx, |workspace, cx| {
                if let Some(tab) = workspace
                    .tabs
                    .iter_mut()
                    .find(|t| t.diff.as_ref().map(|d| d.path == tab_path) == Some(true))
                {
                    if let Some(diff) = &mut tab.diff {
                        diff.text = text;
                        diff.error = None;
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Re-load the active diff tab's text (after git status changes).
    fn refresh_active_diff(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let Some(diff) = tab.diff.as_ref() else {
            return;
        };
        let Some(root) = self.git.as_ref().map(|g| g.root.clone()) else {
            return;
        };
        let rel = diff.rel.clone();
        let staged = diff.staged;
        let tab_path = diff.path.clone();
        cx.spawn(async move |this, cx| {
            let tab_path_bg = tab_path.clone();
            let text = cx
                .background_spawn(async move {
                    let raw = git::diff(&root, &rel, staged).unwrap_or_default();
                    if !raw.trim().is_empty() {
                        Some(raw)
                    } else {
                        std::fs::read_to_string(&tab_path_bg)
                            .ok()
                            .map(|content| git::new_file_diff(&rel, &content))
                    }
                })
                .await;
            let _ = this.update(cx, |workspace, cx| {
                if let Some(tab) = workspace
                    .tabs
                    .iter_mut()
                    .find(|t| t.diff.as_ref().map(|d| d.path == tab_path) == Some(true))
                {
                    if let Some(diff) = &mut tab.diff {
                        diff.text = text;
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    // -- LSP -----------------------------------------------------------------

    /// Format the active document with its language server
    /// (`textDocument/formatting`), then apply the returned edits.
    pub(crate) fn format_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (path, editor, lang_id) = {
            let Some(tab) = self.tabs.get(self.active_tab) else {
                return;
            };
            let Some(path) = tab.path.clone() else {
                return;
            };
            let Some(editor) = tab.editor.clone() else {
                return;
            };
            let Some(lang_id) = lang::language_for(&path) else {
                self.status = format!("{} has no language server", display_name(&path));
                cx.notify();
                return;
            };
            (path, editor, lang_id)
        };
        let Some(client) = self.lsp.lock().unwrap().client_for(lang_id) else {
            self.status = format!("No language server running for {lang_id}");
            cx.notify();
            return;
        };
        let text = editor.read(cx).value().to_string();

        self.status = "Formatting…".into();
        cx.notify();
        let editor_weak = editor.downgrade();
        let display = display_name(&path);
        cx.spawn_in(window, async move |this, cx| {
            let edits = cx
                .background_spawn(async move { client.format_document(&path, &text) })
                .await;
            match edits {
                Some(edits) if !edits.is_empty() => {
                    // Apply on the UI thread; the window handle comes from
                    // the async window context.
                    let _ = editor_weak.update_in(cx, |state, window, cx| {
                        state.apply_lsp_edits(&edits, window, cx);
                    });
                    let _ = this.update(cx, |workspace, cx| {
                        workspace.status = format!("Formatted {display}");
                        cx.notify();
                    });
                }
                Some(_) => {
                    let _ = this.update(cx, |workspace, cx| {
                        workspace.status = "Document already formatted".into();
                        cx.notify();
                    });
                }
                None => {
                    let _ = this.update(cx, |workspace, cx| {
                        workspace.status =
                            "Formatting not supported by the language server".into();
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    pub(crate) fn open_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(idx) = self.tabs.iter().position(|t| t.is_settings) {
            self.active_tab = idx;
        } else {
            self.tabs.push(OpenTab {
                path: None,
                editor: None,
                dirty: false,
                untitled: false,
                preview: false,
                is_settings: true,
                diff: None,
            });
            self.active_tab = self.tabs.len() - 1;
        }
        self.status = "Settings".into();
        cx.notify();
    }

    /// Open user settings.json directly in an editor tab.
    pub(crate) fn open_settings_json(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = crate::settings::settings_file_path();
        if !path.exists() {
            let _ = self.settings.save();
        }
        self.open_file(path, window, cx);
    }

    /// Reloads settings from disk (e.g. after user edits settings.json).
    pub(crate) fn reload_settings(&mut self, cx: &mut Context<Self>) {
        self.settings = crate::settings::Settings::load();
        let themes = theme::all();
        if let Some(pos) = themes.iter().position(|t| t.name == self.settings.workbench_color_theme) {
            self.theme_ix = pos;
            let palette = &themes[pos].terminal_palette;
            for tab in &self.terminal_tabs {
                tab.update(cx, |term, cx| {
                    term.set_theme(palette, cx);
                });
            }
        }
        self.font_size = self.settings.editor_font_size;
        gpui_component::Theme::global_mut(cx).mono_font_size = gpui::px(self.font_size);
        self.status = "Settings reloaded from settings.json".into();
        cx.notify();
    }

    /// Quietly saves a single tab by index if dirty and has a valid path.
    pub(crate) fn save_tab_quiet(&mut self, idx: usize, cx: &mut Context<Self>) -> bool {
        let Some(tab) = self.tabs.get_mut(idx) else {
            return false;
        };
        if tab.is_settings || !tab.dirty || tab.path.is_none() {
            return false;
        }
        let path = tab.path.clone().unwrap();
        let Some(editor) = &tab.editor else {
            return false;
        };
        let text = editor.read(cx).value().to_string();
        if std::fs::write(&path, text.as_bytes()).is_ok() {
            tab.dirty = false;
            if let Some(lang_id) = lang::language_for(&path) {
                self.lsp
                    .lock()
                    .unwrap()
                    .save_document(&path, lang_id, &text);
            }
            self.git_poke();
            if path == crate::settings::settings_file_path() {
                self.reload_settings(cx);
            }
            self.status = format!("Auto-saved {}", display_name(&path));
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Saves all dirty tabs that have a file path on disk.
    pub(crate) fn save_all_dirty_quiet(&mut self, cx: &mut Context<Self>) {
        let mut saved_any = false;
        for i in 0..self.tabs.len() {
            if self.save_tab_quiet(i, cx) {
                saved_any = true;
            }
        }
        if saved_any {
            cx.notify();
        }
    }

    /// Triggers auto-save after debounce delay when typing stops.
    pub(crate) fn trigger_auto_save_after_delay(&mut self, _tab_idx: usize, cx: &mut Context<Self>) {
        if self.settings.editor_auto_save != crate::settings::AutoSaveMode::AfterDelay {
            return;
        }
        self.auto_save_generation = self.auto_save_generation.wrapping_add(1);
        let current_gen = self.auto_save_generation;
        let delay = std::time::Duration::from_millis(self.settings.editor_auto_save_delay);

        cx.spawn(async move |this, cx| {
            cx.background_spawn(async move {
                std::thread::sleep(delay);
            })
            .await;

            let _ = this.update(cx, |workspace, cx| {
                if workspace.auto_save_generation == current_gen {
                    workspace.save_all_dirty_quiet(cx);
                }
            });
        })
        .detach();
    }

    /// Triggers auto-save on focus change / tab switch.
    pub(crate) fn trigger_auto_save_on_focus_change(&mut self, cx: &mut Context<Self>) {
        if self.settings.editor_auto_save != crate::settings::AutoSaveMode::OnFocusChange {
            return;
        }
        self.save_all_dirty_quiet(cx);
    }

    // Tab management methods

    /// Switch to a specific tab by index
    #[allow(dead_code)]
    pub(crate) fn switch_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active_tab = index;
        }
    }

    /// Close a tab by index
    pub(crate) fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(closed_tab) = self.tabs.get(index) {
            if let Some(p) = &closed_tab.path {
                if let Some(lang_id) = lang::language_for(p) {
                    self.lsp.lock().unwrap().close_document(p, lang_id);
                }
            }
        }

        // Don't close if only one tab and it's clean (just show welcome)
        if self.tabs.len() == 1 {
            self.tabs.remove(0);
            self.active_tab = 0;
            // The closed editor held focus — restore it so keybindings keep
            // working on the welcome screen.
            window.focus(&self.focus_handle);
            cx.notify();
            return;
        }

        // Remove the tab
        self.tabs.remove(index);

        // Adjust active tab index
        if index <= self.active_tab && self.active_tab > 0 {
            self.active_tab -= 1;
        }
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len().saturating_sub(1);
        }

        // Hand keyboard focus to the editor that is now active.
        self.focus_active_editor_or_self(window, cx);
        cx.notify();
    }

    /// Switch to a specific tab by index (called from tab bar click)
    pub(crate) fn switch_tab_to(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            self.trigger_auto_save_on_focus_change(cx);
            self.active_tab = index;
            // Clicking a tab also promotes it from preview to permanent (VS Code behavior)
            if let Some(tab) = self.tabs.get_mut(index) {
                tab.preview = false;
            }
            cx.notify();
        }
    }

    /// Close a tab at a specific index (called from tab close button click)
    pub(crate) fn close_tab_at_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            if let Some(closed_tab) = self.tabs.get(index) {
                if let Some(p) = &closed_tab.path {
                    if let Some(lang_id) = lang::language_for(p) {
                        self.lsp.lock().unwrap().close_document(p, lang_id);
                    }
                }
            }

            // Don't close if only one tab - just show welcome
            if self.tabs.len() == 1 {
                self.tabs.remove(0);
                self.active_tab = 0;
            } else {
                // Remove the tab
                self.tabs.remove(index);
                // Adjust active tab index
                if index <= self.active_tab && self.active_tab > 0 {
                    self.active_tab -= 1;
                }
                if self.active_tab >= self.tabs.len() {
                    self.active_tab = self.tabs.len().saturating_sub(1);
                }
            }
            cx.notify();
        }
    }

    /// Action handlers for tab operations

    pub(crate) fn handle_close_tab(&mut self, _: &crate::actions::CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab(self.active_tab, window, cx);
    }

    pub(crate) fn handle_next_tab(&mut self, _: &crate::actions::NextTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.len() > 1 {
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
            self.focus_active_editor_or_self(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn handle_prev_tab(&mut self, _: &crate::actions::PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.len() > 1 {
            self.active_tab = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
            self.focus_active_editor_or_self(window, cx);
            cx.notify();
        }
    }

    /// Switch to a specific tab by index (called when tab is clicked)
    pub(crate) fn handle_switch_tab(&mut self, action: &crate::actions::SwitchTab, window: &mut Window, cx: &mut Context<Self>) {
        if action.index < self.tabs.len() {
            self.active_tab = action.index;
            self.focus_active_editor_or_self(window, cx);
            cx.notify();
        }
    }

    /// Close a specific tab by index (called when close button is clicked)
    pub(crate) fn handle_close_tab_at(&mut self, action: &crate::actions::CloseTabAt, window: &mut Window, cx: &mut Context<Self>) {
        if action.index < self.tabs.len() {
            self.close_tab(action.index, window, cx);
        }
    }

    pub(crate) fn increase_font_size(&mut self, cx: &mut Context<Self>) {
        self.font_size = (self.font_size + 1.0).min(36.0);
        self.settings.editor_font_size = self.font_size;
        let _ = self.settings.save();
        gpui_component::Theme::global_mut(cx).mono_font_size = gpui::px(self.font_size);
        self.status = format!("Editor font size: {:.1}px", self.font_size);
        cx.notify();
    }

    pub(crate) fn decrease_font_size(&mut self, cx: &mut Context<Self>) {
        self.font_size = (self.font_size - 1.0).max(9.0);
        self.settings.editor_font_size = self.font_size;
        let _ = self.settings.save();
        gpui_component::Theme::global_mut(cx).mono_font_size = gpui::px(self.font_size);
        self.status = format!("Editor font size: {:.1}px", self.font_size);
        cx.notify();
    }

    pub(crate) fn reset_font_size(&mut self, cx: &mut Context<Self>) {
        self.font_size = 14.5;
        self.settings.editor_font_size = self.font_size;
        let _ = self.settings.save();
        gpui_component::Theme::global_mut(cx).mono_font_size = gpui::px(self.font_size);
        self.status = format!("Editor font size reset: {:.1}px", self.font_size);
        cx.notify();
    }

    pub(crate) fn quit(&mut self, cx: &mut Context<Self>) {
        cx.quit();
    }

    pub(crate) fn about(&mut self, cx: &mut Context<Self>) {
        self.status = format!("Olova Editor — {} (Zed theme system)", self.theme().name);
        cx.notify();
    }
}
