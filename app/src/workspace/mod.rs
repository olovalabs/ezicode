mod blame;
mod history;
mod render;
mod search;
mod terminal_tabs;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, ScrollHandle, ScrollStrategy, SharedString,
    UniformListScrollHandle, Window,
};
use gpui_component::input::{InputEvent, InputState, RopeExt as _, TabSize};
use notify::Watcher as _;

use crate::fs_tree::{
    collapse_all, display_name, flatten_visible, is_same_or_descendant, load_dir, merge_loaded_dir,
    path_after_move, valid_entry_name, TreeNode, VisibleTreeRow,
};
use crate::git::{self, ChangeKind, GitChange, RepoStatus};
use crate::lang;
use crate::lsp::{LspEvent, LspManager};
use crate::theme;

#[derive(Clone)]
pub struct OpenTab {
    pub path: Option<PathBuf>,
    pub editor: Option<Entity<InputState>>,
    pub dirty: bool,
    pub untitled: bool,

    pub preview: bool,

    pub is_settings: bool,

    pub diff: Option<DiffTab>,

    pub language_override: Option<String>,
}

impl OpenTab {
    pub fn language(&self) -> Option<&str> {
        if let Some(override_lang) = &self.language_override {
            return Some(override_lang.as_str());
        }
        if let Some(path) = &self.path {
            return lang::language_for(path);
        }
        if self.untitled {
            return Some("text");
        }
        None
    }
}

#[derive(Clone)]
pub(crate) struct DiffTab {
    pub path: PathBuf,

    pub rel: String,

    pub staged: bool,

    pub text: Option<String>,

    pub parsed: Option<Arc<crate::ui::diff::ParsedDiff>>,

    pub error: Option<String>,

    /// When set, this tab shows `git show <sha>` (a commit's full diff opened
    /// from the History graph) instead of a working-tree/index diff.
    pub commit: Option<String>,
}

/// Collapsible sections of the Source Control panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum GitSection {
    Repo,
    Conflicts,
    Staged,
    Changes,
    Untracked,
}

/// What a pending git confirmation dialog will do when accepted.
#[derive(Clone, Debug)]
pub(crate) enum GitConfirmAction {
    DiscardPath(PathBuf),
    DiscardAll,
}

/// A modal "are you sure?" prompt for destructive git actions, mirroring
/// Zed's confirmation before a restore/trash. Rendered as an overlay by
/// `render.rs`; Escape or Cancel clears it without acting.
#[derive(Clone, Debug)]
pub(crate) struct GitConfirm {
    pub(crate) title: String,
    pub(crate) detail: String,
    pub(crate) confirm_label: String,
    pub(crate) action: GitConfirmAction,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Activity {
    Explorer,
    Search,
    Git,
    Extensions,
}

impl Activity {
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
    /// How many entries are being dragged, so the drag chip can say "+2".
    pub(crate) count: usize,
}

/// Where a dragged terminal tab would land if dropped right now. Drives the
/// insertion caret in the tab strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerminalDragTarget {
    pub(crate) dock: crate::terminal::TerminalDock,
    /// Insertion index in the target dock's tab list (`0..=len`).
    pub(crate) insert_ix: usize,
    /// Which element claimed the target (tab index, or `usize::MAX` for the
    /// tail drop zone). Prevents adjacent tabs from clearing each other's
    /// indicator: a tab only clears a target it set itself.
    pub(crate) owner_ix: usize,
}

/// The inline editor shown in a terminal tab while it is being renamed.
/// Keyed by entity, not index, so the rename survives tab reordering.
#[derive(Clone)]
pub(crate) struct TerminalRenaming {
    pub(crate) dock: crate::terminal::TerminalDock,
    pub(crate) terminal: Entity<crate::terminal::Terminal>,
    pub(crate) input: Entity<InputState>,
}

#[derive(Clone, Debug)]
pub(crate) struct ExplorerClipboard {
    /// Every entry that was cut or copied — the explorer is multi-select, so
    /// Ctrl+C on three files pastes three files.
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) cut: bool,
}

/// How often the terminal child processes are probed for exit. Short enough
/// that a tab flips to "(exit 0)" promptly, long enough that the per-frame
/// `kill(2)` cost stays negligible no matter how many tabs are open.
const TERMINAL_EXIT_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Default width of the right terminal dock, and the smallest width the
/// tab bar stays usable at. Dragging narrower than the minimum hides the
/// dock, mirroring how the bottom panel behaves when dragged flat.
pub(crate) const TERMINAL_RIGHT_DEFAULT_WIDTH: f32 = 420.0;
pub(crate) const TERMINAL_RIGHT_MIN_WIDTH: f32 = 120.0;

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
    /// The explorer's *focused* entry: the row keyboard navigation moves, the
    /// anchor new selections grow from, and the row drawn with a focus ring.
    pub(crate) selected_path: Option<PathBuf>,
    /// Every selected row. VS Code's explorer is a multi-select tree: Ctrl or
    /// Cmd click toggles, Shift click extends from the anchor. The set is only
    /// honoured while it still contains `selected_path`, so the many places
    /// that just assign `selected_path` implicitly collapse it back to a
    /// single selection instead of leaving a stale highlight behind.
    pub(crate) explorer_selection: Vec<PathBuf>,
    /// Anchor row for Shift+click / Shift+arrow range selection.
    pub(crate) explorer_selection_anchor: Option<PathBuf>,
    /// VS Code's `workbench.tree.enableStickyScroll`.
    pub(crate) explorer_sticky_scroll: bool,
    /// Number of sticky rows painted on the last frame. Keyboard navigation
    /// scrolls past them so the focused row never hides under the widget.
    pub(crate) explorer_sticky_rows: usize,
    /// Buffered type-ahead query and the generation that will clear it.
    pub(crate) explorer_typeahead: String,
    pub(crate) explorer_typeahead_generation: u64,
    /// Folder currently hovered during a drag, and the generation used to
    /// debounce VS Code's "hover a collapsed folder to expand it".
    pub(crate) explorer_drag_target: Option<PathBuf>,
    pub(crate) explorer_drag_generation: u64,
    pub(crate) explorer_section_expanded: bool,
    pub(crate) git_repo_section_expanded: bool,
    pub(crate) git_conflicts_expanded: bool,
    pub(crate) git_staged_expanded: bool,
    pub(crate) git_changes_expanded: bool,
    pub(crate) git_untracked_expanded: bool,
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
    pub(crate) terminal_maximized: bool,
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
    /// Horizontal scroll position of the terminal tab strip, so the active tab
    /// can be scrolled into view once there are more tabs than fit.
    pub(crate) terminal_tab_scroll: ScrollHandle,
    /// When the terminal child processes were last probed for exit. See
    /// [`Workspace::poll_terminal_processes`].
    pub(crate) last_terminal_poll: std::time::Instant,
    /// Monotonic counter for labeling new terminals (PowerShell 1, PowerShell 2, ...).
    pub(crate) next_terminal_id: usize,
    /// Number of pinned tabs in the bottom dock. Pinned tabs are always a
    /// prefix of `terminal_tabs`, exactly like `pinned_tab_count` on a Zed
    /// pane.
    pub(crate) terminal_pinned_count: usize,
    // ---- Right-dock terminal panel (Zed-style) ----
    // A second, fully independent terminal surface: its own tab list and its
    // own PTY sessions. Nothing here is shared with `terminal_tabs` above —
    // closing, hiding or resizing one dock never affects the other.
    pub(crate) show_terminal_right: bool,
    /// Current width of the right terminal dock in px (drag-resizable).
    pub(crate) terminal_right_width: f32,
    /// Terminals living in the right dock. Each entry owns a private PTY.
    pub(crate) terminal_right_tabs: Vec<Entity<crate::terminal::Terminal>>,
    /// Index of the active right-dock terminal tab.
    pub(crate) active_terminal_right: usize,
    /// Horizontal scroll position of the right dock's tab strip.
    pub(crate) terminal_right_tab_scroll: ScrollHandle,
    /// Monotonic counter for right-dock terminals.
    pub(crate) next_terminal_right_id: usize,
    /// Number of pinned tabs in the right dock (see `terminal_pinned_count`).
    pub(crate) terminal_right_pinned_count: usize,
    /// Live drop target while a terminal tab drag is in flight, `None`
    /// otherwise. Shared by both docks — only one drag can exist at a time.
    pub(crate) terminal_drag_target: Option<TerminalDragTarget>,
    /// In-flight inline rename of a terminal tab, if any.
    pub(crate) terminal_renaming: Option<TerminalRenaming>,
    /// File system change notification sender: the changed path, so reloads
    /// can be scoped to the affected directory instead of rescanning the
    /// whole tree on every event.
    pub(crate) fs_event_tx: async_channel::Sender<PathBuf>,
    /// Cached `display_name(root)` so the title bar and explorer header don't
    pub(crate) root_display: String,

    pub(crate) root_display_shared: SharedString,

    pub(crate) _watcher: Option<notify::RecommendedWatcher>,

    pub(crate) tabs: Vec<OpenTab>,

    /// Right-hand native preview bound to a Markdown buffer. The parsed model
    /// and source are replaced atomically after the background debounce.
    pub(crate) markdown_preview: Option<crate::markdown_preview::MarkdownPreviewState>,

    pub(crate) active_tab: usize,

    pub(crate) focus_handle: FocusHandle,

    pub(crate) sidebar_width: f32,

    pub(crate) terminal_height: f32,

    pub(crate) panel_resize: Option<PanelResizeDrag>,

    pub(crate) git: Option<RepoStatus>,

    pub(crate) git_poke_tx: Option<std::sync::mpsc::Sender<()>>,

    /// Bumped every time a new git watcher is started. Status snapshots
    /// carry the generation they were produced under, and stale ones (from a
    /// previous project's watcher still winding down) are dropped instead of
    /// overwriting the current repo's state.
    pub(crate) git_watch_generation: u64,

    /// Lookup table derived from `git` on every status update: change kind
    /// per absolute path, plus a Modified marker for every ancestor
    /// directory. Shared as an `Arc` so the virtualized explorer list can
    /// clone it per frame for pennies.
    pub(crate) git_path_kinds: Arc<HashMap<PathBuf, ChangeKind>>,

    /// Label of the remote git operation in flight (fetch/pull/push/…).
    /// Doubles as a lock so two remote operations can't interleave.
    pub(crate) git_op_running: Option<&'static str>,

    /// Pending destructive-action confirmation (discard file / discard all).
    pub(crate) git_confirm: Option<GitConfirm>,

    pub(crate) git_commit_input: Option<Entity<InputState>>,

    pub(crate) git_commit_pending: bool,

    // ---- Inline git blame (Feature 2, Zed-style) ----
    /// Cached blame per absolute file path. Computed off-thread, shifted on
    /// edits, invalidated on save / branch switch / commit / external change.
    pub(crate) git_blame_cache: HashMap<PathBuf, git::FileBlame>,
    /// Bumped whenever a blame request is superseded (file closed/changed), so
    /// stale background results are dropped instead of applied.
    pub(crate) git_blame_generation: u64,
    /// The "Toggle Git Blame" state (per-line blame in the gutter for all
    /// lines). Applies to every editor, like Zed's editor-wide toggle.
    pub(crate) git_blame_gutter: bool,
    /// Lazily-fetched commit details, cached by SHA and shared with background
    /// tasks (used by the history view and blame hover data).
    pub(crate) commit_detail_cache: Arc<Mutex<HashMap<String, git::CommitDetails>>>,

    // ---- Source Control "History" graph (Feature 1) ----
    /// Whether the Source Control panel shows the History graph (vs. changes).
    pub(crate) git_history_view: bool,
    /// Loaded commits (newest first), lazily paginated.
    pub(crate) git_history: Vec<git::Commit>,
    /// Precomputed lane/merge graph rows, one per commit in `git_history`.
    pub(crate) git_history_graph: Vec<git::GraphRow>,
    /// Invalidation counter for history loads (refs/HEAD change → reload).
    pub(crate) git_history_generation: u64,
    /// A page load is in flight.
    pub(crate) git_history_loading: bool,
    /// The last page returned fewer than a full page → no more commits.
    pub(crate) git_history_complete: bool,
    /// The commit whose details are open, if any.
    pub(crate) git_history_selected: Option<String>,
    /// Virtualized scroll handle for the history list.
    pub(crate) git_history_scroll: UniformListScrollHandle,

    pub(crate) settings: crate::settings::Settings,

    pub(crate) auto_save_generation: usize,

    pub(crate) picker: Option<crate::ui::picker::PickerState>,

    pub(crate) picker_confirm_pending: bool,

    pub(crate) workspace_files_cache: Option<(PathBuf, Vec<crate::ui::picker::PickerItem>)>,

    pub(crate) cached_breadcrumbs: Option<(
        PathBuf,
        usize,
        usize,
        Vec<crate::ui::breadcrumbs::BreadcrumbItem>,
    )>,

    // ---- Project search (VS Code-style Search view, ripgrep engine) ----
    pub(crate) search_query_input: Option<Entity<InputState>>,
    pub(crate) search_replace_input: Option<Entity<InputState>>,
    pub(crate) search_include_input: Option<Entity<InputState>>,
    pub(crate) search_case_sensitive: bool,
    pub(crate) search_whole_word: bool,
    pub(crate) search_use_regex: bool,
    pub(crate) search_results: Vec<crate::search::SearchFileResult>,
    pub(crate) search_total_matches: usize,
    pub(crate) search_truncated: bool,
    pub(crate) search_in_progress: bool,
    pub(crate) search_error: Option<String>,
    pub(crate) search_elapsed_ms: u64,
    pub(crate) search_generation: u64,
    pub(crate) search_collapsed: HashSet<PathBuf>,
    pub(crate) search_replace_open: bool,
    /// Jump applied once an async `open_file` finishes (search result click
    /// on a file that is not open yet).
    pub(crate) pending_search_jump: Option<(PathBuf, usize, usize)>,
    pub(crate) pending_restore_tabs: Vec<crate::storage::OpenTabState>,
    pub(crate) pending_restore_active_tab: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum ResizeKind {
    Sidebar,
    Terminal,
    /// The vertical handle on the left edge of the right terminal dock.
    TerminalRight,
}

#[derive(Clone, Copy)]
pub(crate) struct PanelResizeDrag {
    pub(crate) kind: ResizeKind,

    pub(crate) start_mouse: f32,

    #[allow(dead_code)]
    pub(crate) start_size: f32,
}

pub(crate) struct LoadedBuffer {
    pub text: String,

    pub lang_id: &'static str,
    /// False when the file is too big for tree-sitter highlighting.
    pub highlight: bool,
}

/// Read and classify one file for opening in an editor. Pure I/O + pure
/// computation, no GPUI handles — safe (and intended) to run on the
/// background executor. The `Err` strings are user-facing status messages,
/// worded exactly like the old synchronous open path produced them.
fn load_buffer_file(path: &std::path::Path) -> Result<LoadedBuffer, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("open failed: {e}"))?;
    if bytes.len() > 8_000_000 {
        return Err(format!("{} is too large (>8MB)", display_name(path)));
    }
    if bytes.iter().take(8000).any(|&b| b == 0) {
        return Err(format!("{} looks binary", display_name(path)));
    }
    let newline_count = bytes.iter().filter(|&&b| b == b'\n').count();
    let highlight = bytes.len() <= 400_000 && newline_count <= 8_000;
    let lang_id = if highlight {
        lang::language_for(path).unwrap_or("text")
    } else {
        "text"
    };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(LoadedBuffer {
        text,
        lang_id,
        highlight,
    })
}

fn expand_saved_folders(nodes: &mut [TreeNode], expanded: &[PathBuf]) {
    let set: HashSet<&PathBuf> = expanded.iter().collect();
    for n in nodes {
        if n.is_dir && set.contains(&n.path) {
            n.expanded = true;
            if !n.children_loaded {
                n.children = load_dir(&n.path);
                n.children_loaded = true;
            }
            expand_saved_folders(&mut n.children, expanded);
        }
    }
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
        let fs_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

        std::thread::spawn(move || {
            let rx = fs_event_rx;
            loop {
                if fs_stop.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
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

        let mut workspace = Self {
            root: None,
            tree: Vec::new(),
            explorer_rows: Arc::from(Vec::<VisibleTreeRow>::new()),
            explorer_scroll_handle,
            explorer_focus_handle,
            root_display: String::new(),
            root_display_shared: SharedString::new_static(""),
            selected_path: None,
            explorer_selection: Vec::new(),
            explorer_selection_anchor: None,
            explorer_sticky_scroll: true,
            explorer_sticky_rows: 0,
            explorer_typeahead: String::new(),
            explorer_typeahead_generation: 0,
            explorer_drag_target: None,
            explorer_drag_generation: 0,
            explorer_section_expanded: true,
            git_repo_section_expanded: true,
            git_conflicts_expanded: true,
            git_staged_expanded: true,
            git_changes_expanded: true,
            git_untracked_expanded: true,
            split_diff: true,
            inline_creating: None,
            inline_renaming: None,
            explorer_clipboard: None,
            pending_open: None,
            status: "Welcome — open a folder or create a file to begin".into(),
            activity: Activity::Explorer,
            show_sidebar: true,
            show_terminal: false,
            terminal_maximized: false,
            theme_ix,
            font_size,
            settings,
            auto_save_generation: 0,
            lsp,
            diagnostics_by_path: HashMap::new(),
            terminal_tabs: Vec::new(),
            active_terminal: 0,
            terminal_tab_scroll: ScrollHandle::new(),
            last_terminal_poll: std::time::Instant::now(),
            next_terminal_id: 1,
            terminal_pinned_count: 0,
            show_terminal_right: false,
            terminal_right_width: TERMINAL_RIGHT_DEFAULT_WIDTH,
            terminal_right_tabs: Vec::new(),
            active_terminal_right: 0,
            terminal_right_tab_scroll: ScrollHandle::new(),
            next_terminal_right_id: 1,
            terminal_right_pinned_count: 0,
            terminal_drag_target: None,
            terminal_renaming: None,
            fs_event_tx,
            _watcher: None,
            tabs: Vec::new(),
            markdown_preview: None,
            active_tab: 0,
            focus_handle,
            sidebar_width: 300.0,
            terminal_height: 320.0,
            panel_resize: None,
            git: None,
            git_poke_tx: None,
            git_watch_generation: 0,
            git_path_kinds: Arc::new(HashMap::new()),
            git_op_running: None,
            git_confirm: None,
            git_commit_input: None,
            git_commit_pending: false,
            git_blame_cache: HashMap::new(),
            git_blame_generation: 0,
            git_blame_gutter: false,
            commit_detail_cache: Arc::new(Mutex::new(HashMap::new())),
            git_history_view: false,
            git_history: Vec::new(),
            git_history_graph: Vec::new(),
            git_history_generation: 0,
            git_history_loading: false,
            git_history_complete: false,
            git_history_selected: None,
            git_history_scroll: UniformListScrollHandle::new(),
            picker: None,
            picker_confirm_pending: false,
            workspace_files_cache: None,
            cached_breadcrumbs: None,
            search_query_input: None,
            search_replace_input: None,
            search_include_input: None,
            search_case_sensitive: false,
            search_whole_word: false,
            search_use_regex: false,
            search_results: Vec::new(),
            search_total_matches: 0,
            search_truncated: false,
            search_in_progress: false,
            search_error: None,
            search_elapsed_ms: 0,
            search_generation: 0,
            search_collapsed: HashSet::new(),
            search_replace_open: false,
            pending_search_jump: None,
            pending_restore_tabs: Vec::new(),
            pending_restore_active_tab: None,
        };

        let global_state = crate::storage::GlobalState::load();
        if let Some(root) = global_state.last_workspace_root {
            if root.is_dir() {
                workspace.load_root(root, cx);
            }
        }

        workspace
    }

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
                    "Settings — ezicode".to_string()
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
                let label = if diff.staged {
                    " (staged diff)"
                } else {
                    " (diff)"
                };
                match &self.root {
                    Some(_) => format!("{name}{label} — {}", self.root_display),
                    None => format!("{name}{label}"),
                }
            } else if tab.untitled {
                let star = if tab.dirty { " ●" } else { "" };
                if self.root.is_some() {
                    format!("untitled{star} — {}", self.root_display)
                } else {
                    format!("untitled{star} — ezicode")
                }
            } else if self.root.is_some() {
                self.root_display.clone()
            } else {
                "ezicode".to_string()
            }
        } else if self.root.is_some() {
            self.root_display.clone()
        } else {
            "ezicode".to_string()
        }
    }

    /// Returns true if welcome screen should be shown
    pub(crate) fn welcome_visible(&self) -> bool {
        // Show welcome when no files are open
        self.tabs.is_empty()
    }

    /// Get the active tab's editor
    pub fn active_editor(&self) -> Option<&Entity<InputState>> {
        self.tabs
            .get(self.active_tab)
            .and_then(|t| t.editor.as_ref())
    }

    pub fn active_path(&self) -> Option<&PathBuf> {
        self.tabs.get(self.active_tab)?.path.as_ref()
    }

    pub(crate) fn toggle_markdown_preview(&mut self, cx: &mut Context<Self>) {
        let Some((path, source)) = self.active_markdown_buffer(cx) else {
            return;
        };
        if self
            .markdown_preview
            .as_ref()
            .is_some_and(|preview| preview.path == path)
        {
            self.close_markdown_preview(cx);
            return;
        }

        self.open_markdown_preview(path, source, cx);
        self.status = "Markdown preview opened".into();
        cx.notify();
    }

    fn active_markdown_buffer(&self, cx: &mut Context<Self>) -> Option<(PathBuf, Arc<str>)> {
        let tab = self.tabs.get(self.active_tab)?;
        let path = tab.path.clone()?;
        if !crate::markdown_preview::is_markdown_path(&path) {
            return None;
        }
        let editor = tab.editor.clone()?;
        let source: Arc<str> = editor.read(cx).value().to_string().into();
        Some((path, source))
    }

    fn open_markdown_preview(&mut self, path: PathBuf, source: Arc<str>, cx: &mut Context<Self>) {
        // Give the user an immediate pane, then replace its model through the
        // same debounced background path used for edits.
        self.markdown_preview = Some(crate::markdown_preview::MarkdownPreviewState {
            path: path.clone(),
            source: source.clone(),
            document: Arc::new(crate::markdown_preview::MarkdownDocument::default()),
            generation: 0,
        });
        self.schedule_markdown_preview_update(path, source, cx);
    }

    pub(crate) fn sync_markdown_preview_with_active_tab(&mut self, cx: &mut Context<Self>) {
        let Some(preview) = self.markdown_preview.as_ref() else {
            return;
        };
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let Some(path) = tab.path.clone() else {
            return;
        };
        if !crate::markdown_preview::is_markdown_path(&path) || preview.path == path {
            return;
        }
        let Some(editor) = tab.editor.clone() else {
            return;
        };
        let source: Arc<str> = editor.read(cx).value().to_string().into();
        self.open_markdown_preview(path, source, cx);
    }

    pub(crate) fn close_markdown_preview(&mut self, cx: &mut Context<Self>) {
        if self.markdown_preview.take().is_some() {
            self.status = "Markdown preview closed".into();
            cx.notify();
        }
    }

    fn schedule_markdown_preview_update(
        &mut self,
        path: PathBuf,
        source: Arc<str>,
        cx: &mut Context<Self>,
    ) {
        let Some(preview) = self.markdown_preview.as_mut() else {
            return;
        };
        if preview.path != path {
            return;
        }
        preview.generation = preview.generation.wrapping_add(1);
        let generation = preview.generation;

        cx.spawn(async move |workspace, cx| {
            cx.background_executor()
                .timer(crate::markdown_preview::REPARSE_DEBOUNCE)
                .await;
            let parse_source = source.clone();
            let parsed = cx
                .background_executor()
                .spawn(async move { crate::markdown_preview::parse(&parse_source) })
                .await;
            let _ = workspace.update(cx, |workspace, cx| {
                let Some(preview) = workspace.markdown_preview.as_mut() else {
                    return;
                };
                if preview.path == path && preview.generation == generation {
                    preview.source = source;
                    preview.document = Arc::new(parsed);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn markdown_buffer_changed(
        &mut self,
        path: &Path,
        editor: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) {
        if self
            .markdown_preview
            .as_ref()
            .is_some_and(|preview| preview.path == path)
        {
            let source: Arc<str> = editor.read(cx).value().to_string().into();
            self.schedule_markdown_preview_update(path.to_path_buf(), source, cx);
        }
    }

    #[allow(dead_code)]
    pub fn is_dirty(&self) -> bool {
        self.tabs
            .get(self.active_tab)
            .map(|t| t.dirty)
            .unwrap_or(false)
    }

    pub(crate) fn apply_theme(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let themes = theme::all();
        let Some(th) = themes.get(ix) else {
            return;
        };
        self.theme_ix = ix;
        self.settings.workbench_color_theme = th.name.to_string();
        let _ = self.settings.save();

        let mode = if th.appearance == "light" {
            gpui_component::ThemeMode::Light
        } else {
            gpui_component::ThemeMode::Dark
        };
        gpui_component::Theme::change(mode, Some(window), cx);

        gpui_component::Theme::global_mut(cx).highlight_theme = Arc::new(th.highlight_theme());

        crate::assets::sync_component_fonts(cx);

        let palette = &th.terminal_palette;
        for tab in self
            .terminal_tabs
            .iter()
            .chain(self.terminal_right_tabs.iter())
        {
            tab.update(cx, |term, cx| {
                term.set_theme(palette, cx);
            });
        }
        self.status = format!("Theme: {}", th.name);
        cx.notify();
    }

    fn rebuild_explorer_rows(&mut self) {
        let mut rows = Vec::new();
        if self.explorer_section_expanded {
            flatten_visible(&self.tree, 0, &mut rows);
        }
        self.explorer_rows = Arc::from(rows);
    }

    pub(crate) fn load_root(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let switching_project = self.root.as_ref() != Some(&path);

        if switching_project && self.root.is_some() {
            self.persist_workspace_state(cx);
        }

        let mut global_state = crate::storage::GlobalState::load();
        global_state.add_recent_folder(path.clone());

        self.workspace_files_cache = None;
        self.root = Some(path.clone());
        self.root_display = display_name(&path);
        self.root_display_shared = SharedString::from(self.root_display.clone());

        self.lsp.lock().unwrap().set_root(Some(path.clone()));

        // Opening a different folder must drop the previous project's editors.
        if switching_project {
            self.close_all_project_tabs(cx);
        }

        let saved_state = crate::storage::WorkspaceState::load(&path);
        let saved_expanded = saved_state
            .as_ref()
            .map(|s| s.expanded_folders.clone())
            .unwrap_or_default();
        let saved_selection = saved_state
            .as_ref()
            .and_then(|s| s.explorer_selected.clone())
            .filter(|path| path.exists());

        if let Some(saved) = &saved_state {
            let max_w = 800.0f32;
            let min_w = 170.0f32;
            self.sidebar_width = saved.layout.sidebar_width.clamp(min_w, max_w);
            self.terminal_height = saved.layout.terminal_height.clamp(80.0, 800.0);
            self.show_sidebar = saved.layout.show_sidebar;
            self.show_terminal = saved.layout.show_terminal;
            self.terminal_maximized = saved.layout.terminal_maximized;
            self.terminal_right_width = saved
                .layout
                .terminal_right_width
                .clamp(TERMINAL_RIGHT_MIN_WIDTH, 800.0);
            // PTY sessions die with the process, so after a restart the right
            // dock always starts closed — one click on the status-bar icon
            // spawns a fresh shell. Mid-session folder switches keep live
            // terminals, and there the saved visibility is honored.
            self.show_terminal_right =
                saved.layout.show_terminal_right && !self.terminal_right_tabs.is_empty();
            self.explorer_sticky_scroll = saved.layout.explorer_sticky_scroll;
            self.activity = match saved.layout.activity.as_str() {
                "Search" => Activity::Search,
                "Git" => Activity::Git,
                "Extensions" => Activity::Extensions,
                _ => Activity::Explorer,
            };
            self.pending_restore_tabs = saved.tabs.clone();
            self.pending_restore_active_tab = Some(saved.active_tab);
        }

        self.tree.clear();
        self.explorer_section_expanded = true;
        self.rebuild_explorer_rows();
        self.selected_path = None;
        self.explorer_selection.clear();
        self.explorer_selection_anchor = None;
        self.explorer_typeahead.clear();
        self.inline_creating = None;
        self.inline_renaming = None;
        self.explorer_clipboard = None;
        self.clear_search_results();
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
                    let mut merged = merge_loaded_dir(&scan_root, previous, entries);
                    if !saved_expanded.is_empty() {
                        expand_saved_folders(&mut merged, &saved_expanded);
                    }
                    workspace.tree = merged;
                    workspace.rebuild_explorer_rows();
                    // Restore the previously focused row and scroll it into
                    // view, the way VS Code reopens an explorer.
                    if let Some(selected) = saved_selection.clone() {
                        workspace.reveal_tree_path(&selected);
                        workspace.set_explorer_selection(selected);
                    }
                    workspace.status = format!("Opened folder {}", workspace.root_display);
                    cx.notify();
                }
            });
        })
        .detach();

        let tx = self.fs_event_tx.clone();
        let watcher =
            notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
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

        self.start_git_watcher(&path, cx);

        cx.notify();
    }

    /// Close every open editor when leaving a project so the next folder
    /// starts clean. Unsaved buffers are flushed first.
    fn close_all_project_tabs(&mut self, cx: &mut Context<Self>) {
        self.save_all_dirty_quiet(cx);

        for tab in &self.tabs {
            if let Some(p) = &tab.path {
                if let Some(lang_id) = tab.language() {
                    self.lsp.lock().unwrap().close_document(p, lang_id);
                }
            }
        }

        self.tabs.clear();
        self.active_tab = 0;
        self.diagnostics_by_path.clear();
        self.cached_breadcrumbs = None;
        self.pending_open = None;
        self.picker = None;
        self.workspace_files_cache = None;
        self.git_commit_input = None;
        self.git_commit_pending = false;
        self.pending_search_jump = None;
        self.clear_search_results();

        // Leaving the project invalidates every piece of repo state, not just
        // the poll thread: without this a project switch could briefly show
        // (or keep showing) the previous repo's changes.
        self.stop_git_watcher();
    }

    /// Stop the git poll thread and clear all repo-derived state. The thread
    /// exits within ~1.5 s once its poke sender is dropped; the generation
    /// bump makes any snapshot it still manages to deliver a no-op.
    pub(crate) fn stop_git_watcher(&mut self) {
        self.git_poke_tx = None;
        self.git_watch_generation = self.git_watch_generation.wrapping_add(1);
        self.git = None;
        self.git_path_kinds = Arc::new(HashMap::new());
        self.git_confirm = None;
    }

    pub(crate) fn start_git_watcher(&mut self, root: &Path, cx: &mut Context<Self>) {
        // Invalidate whatever watcher may still be running (re-opening the
        // same folder, or a folder that is not a repository) before starting
        // a new one, so stale snapshots can never race the fresh ones.
        self.stop_git_watcher();
        let Some(repo_root) = git::find_repo_root(root) else {
            cx.notify();
            return;
        };
        let generation = self.git_watch_generation;
        let (poke_tx, poke_rx) = std::sync::mpsc::channel::<()>();
        let (status_tx, status_rx) = async_channel::unbounded::<RepoStatus>();

        if let Some(status) = git::status(&repo_root) {
            self.apply_git_status(status, cx);
        }

        std::thread::spawn(move || {
            let mut last: Option<RepoStatus> = None;
            loop {
                if let Some(status) = git::status(&repo_root) {
                    if last.as_ref() != Some(&status) {
                        if status_tx.try_send(status.clone()).is_err() {
                            break;
                        }
                        last = Some(status);
                    }
                }

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
                    let done = this
                        .update(cx, |workspace, cx| {
                            // A snapshot from a superseded watcher must not
                            // clobber the current repository's state.
                            if workspace.git_watch_generation != generation {
                                return true;
                            }
                            workspace.apply_git_status(status, cx);
                            false
                        })
                        .unwrap_or(true);
                    if done {
                        break;
                    }
                }
            }
        })
        .detach();
    }

    /// Install a fresh status snapshot and refresh everything derived from
    /// it: the path→kind lookup used by the explorer, and any open diff tabs.
    fn apply_git_status(&mut self, status: RepoStatus, cx: &mut Context<Self>) {
        let mut kinds: HashMap<PathBuf, ChangeKind> = HashMap::new();
        for change in &status.changes {
            let kind = if change.is_conflicted() {
                ChangeKind::Conflicted
            } else if change.is_untracked() {
                ChangeKind::Untracked
            } else if let Some(worktree) = change.worktree {
                worktree
            } else if let Some(index) = change.index {
                index
            } else {
                continue;
            };
            kinds.insert(change.path.clone(), kind);
            // Tint ancestor directories like VS Code/Zed do; conflicts win
            // over the generic Modified marker so red propagates upward.
            let mut dir = change.path.parent();
            while let Some(d) = dir {
                if !d.starts_with(&status.root) || d == status.root {
                    break;
                }
                let entry = kinds.entry(d.to_path_buf()).or_insert(ChangeKind::Modified);
                if kind == ChangeKind::Conflicted {
                    *entry = ChangeKind::Conflicted;
                }
                dir = d.parent();
            }
        }
        self.git_path_kinds = Arc::new(kinds);
        self.git = Some(status);
        self.refresh_diff_tabs(cx);
        // A repo change (commit, branch switch, pull, external edit) invalidates
        // cached blame; recompute for the active file. Also refresh the History
        // graph if it is currently shown.
        self.invalidate_blame(cx);
        if self.git_history_view {
            self.reload_git_history(cx);
        }
        cx.notify();
    }

    pub(crate) fn git_poke(&self) {
        if let Some(tx) = &self.git_poke_tx {
            let _ = tx.send(());
        }
    }

    pub(crate) fn open_folder_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.status = "Choose folder…".into();
        cx.notify();
        // Native dialogs pump Windows messages; run them outside the App borrow.
        cx.spawn(async move |this, cx| {
            let path = rfd::FileDialog::new().pick_folder();
            let _ = this.update(cx, |workspace, cx| match path {
                Some(path) => workspace.load_root(path, cx),
                None => {
                    workspace.status = "Open folder cancelled".into();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn open_file_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.status = "Choose file…".into();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let path = rfd::FileDialog::new().pick_file();
            let _ = this.update_in(cx, |workspace, window, cx| match path {
                Some(path) => workspace.open_file(path, window, cx),
                None => {
                    workspace.status = "Open file cancelled".into();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn new_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            preview: false,
            is_settings: false,
            diff: None,
            language_override: None,
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

        if self.terminal_tabs.is_empty() {
            self.new_terminal(window, cx);
            return;
        }
        self.focus_active_terminal(window, cx);
        self.status = "Terminal active".into();
        cx.notify();
    }

    pub(crate) fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_terminal_in(None, window, cx);
    }

    /// Create beside the terminal that owns focus. This keeps the global
    /// shortcut and the context-menu action useful in either terminal dock.
    pub(crate) fn new_terminal_for_focused_dock(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .focused_terminal(window, cx)
            .is_some_and(|(dock, _, _)| dock == crate::terminal::TerminalDock::Right)
        {
            self.new_terminal_right(window, cx);
        } else {
            self.new_terminal(window, cx);
        }
    }

    /// Open a terminal, optionally rooted at a specific directory (the
    /// explorer's "Open in Integrated Terminal").
    pub(crate) fn new_terminal_in(
        &mut self,
        directory: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let working_dir = directory.or_else(|| {
            self.terminal_tabs
                .get(self.active_terminal)
                .and_then(|term| term.read(cx).working_dir.clone())
                .or_else(|| self.root.clone())
        });

        let _id = self.next_terminal_id;
        self.next_terminal_id += 1;

        let shell_name = crate::terminal::Terminal::detect_shell_name();
        let folder = working_dir
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .or_else(|| {
                self.root
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .and_then(|s| s.to_str())
            })
            .unwrap_or("app");
        let label = format!("{folder} \u{2013} {shell_name}");
        let palette = self.theme().terminal_palette.clone();
        let term = cx.new(|cx| {
            crate::terminal::Terminal::new(working_dir.as_deref(), label, palette, window, cx)
        });
        self.watch_terminal_title(&term, cx);
        self.terminal_tabs.push(term);
        self.active_terminal = self.terminal_tabs.len() - 1;
        self.show_terminal = true;
        self.reveal_active_terminal_tab();
        self.focus_active_terminal(window, cx);
        self.status = format!("Terminal {} created", self.active_terminal + 1);
        cx.notify();
    }

    /// Keep a tab's cached title in sync with the `OSC 0 / 2` title its child
    /// process reports.
    ///
    /// The view *pushes* the new title into this entity rather than the tab
    /// strip reading it back out of the view: reading a `TerminalView` from the
    /// strip subscribes the strip to that view, and views notify on *every* PTY
    /// write, so the entire tab strip was re-rendered on every keystroke no
    /// matter how many terminals were open. Now only the tab whose title really
    /// changed re-renders.
    fn watch_terminal_title(
        &mut self,
        term: &Entity<crate::terminal::Terminal>,
        cx: &mut Context<Self>,
    ) {
        let weak = term.downgrade();
        term.update(cx, |term, cx| {
            term.view.update(cx, |view, _cx| {
                view.set_title_callback(move |_window, cx, title| {
                    let _ = weak.update(cx, |term, cx| term.set_osc_title(title, cx));
                });
            });
        });
    }

    /// Scroll the terminal tab strip so the active tab is visible. A no-op (and
    /// free) while every tab still fits.
    fn reveal_active_terminal_tab(&mut self) {
        if !self.terminal_tabs.is_empty() {
            self.terminal_tab_scroll
                .scroll_to_item(self.active_terminal);
        }
    }

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
        self.reveal_active_terminal_tab();
        self.status = format!("Terminal {} active", index + 1);
        cx.notify();
    }

    fn focus_active_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(term) = self.terminal_tabs.get(self.active_terminal).cloned() {
            term.read(cx).focus_handle(cx).focus(window);
        }
    }

    pub(crate) fn hide_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_terminal = false;
        self.terminal_maximized = false;
        self.status = "Terminal hidden".into();
        self.focus_active_editor_or_self(window, cx);
        cx.notify();
    }

    pub(crate) fn toggle_terminal_maximized(&mut self, cx: &mut Context<Self>) {
        if !self.show_terminal {
            self.show_terminal = true;
            self.terminal_maximized = true;
        } else {
            self.terminal_maximized = !self.terminal_maximized;
        }
        cx.notify();
    }

    pub(crate) fn close_terminal(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Shared multi-close core: keeps the pinned prefix, the active tab
        // and the inline rename consistent. See workspace/terminal_tabs.rs.
        self.close_terminal_tabs_at(
            crate::terminal::TerminalDock::Bottom,
            vec![index],
            window,
            cx,
        );
    }

    pub(crate) fn next_terminal_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_tabs.len() <= 1 {
            return;
        }
        self.active_terminal = (self.active_terminal + 1) % self.terminal_tabs.len();
        self.reveal_active_terminal_tab();
        self.focus_active_terminal(window, cx);
        self.status = format!("Terminal {} active", self.active_terminal + 1);
        cx.notify();
    }

    pub(crate) fn prev_terminal_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_tabs.len() <= 1 {
            return;
        }
        self.active_terminal =
            (self.active_terminal + self.terminal_tabs.len() - 1) % self.terminal_tabs.len();
        self.reveal_active_terminal_tab();
        self.focus_active_terminal(window, cx);
        self.status = format!("Terminal {} active", self.active_terminal + 1);
        cx.notify();
    }

    pub(crate) fn close_active_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.terminal_tabs.is_empty() {
            return;
        }
        let idx = self.active_terminal;
        self.close_terminal(idx, window, cx);
    }

    /// Return the terminal whose view currently owns focus. Context menus put
    /// focus back on their action context before dispatch, so this also routes
    /// menu commands to the exact dock from which the menu was opened.
    fn focused_terminal(
        &self,
        window: &Window,
        cx: &gpui::App,
    ) -> Option<(
        crate::terminal::TerminalDock,
        usize,
        Entity<crate::terminal::Terminal>,
    )> {
        for (index, terminal) in self.terminal_right_tabs.iter().enumerate() {
            if terminal
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx)
            {
                return Some((
                    crate::terminal::TerminalDock::Right,
                    index,
                    terminal.clone(),
                ));
            }
        }
        for (index, terminal) in self.terminal_tabs.iter().enumerate() {
            if terminal
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx)
            {
                return Some((
                    crate::terminal::TerminalDock::Bottom,
                    index,
                    terminal.clone(),
                ));
            }
        }
        None
    }

    fn focused_or_active_terminal(
        &self,
        window: &Window,
        cx: &gpui::App,
    ) -> Option<(
        crate::terminal::TerminalDock,
        usize,
        Entity<crate::terminal::Terminal>,
    )> {
        self.focused_terminal(window, cx).or_else(|| {
            self.terminal_tabs
                .get(self.active_terminal)
                .cloned()
                .map(|term| {
                    (
                        crate::terminal::TerminalDock::Bottom,
                        self.active_terminal,
                        term,
                    )
                })
        })
    }

    pub(crate) fn close_focused_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((dock, index, _)) = self.focused_or_active_terminal(window, cx) else {
            return;
        };
        // Zed parity: close shortcuts spare pinned tabs; only the tab's own
        // context menu (or unpinning first) closes a pinned terminal.
        if self.terminal_tab_is_pinned(dock, index) {
            self.status = "Terminal tab is pinned — unpin it to close".into();
            cx.notify();
            return;
        }
        match dock {
            crate::terminal::TerminalDock::Bottom => self.close_terminal(index, window, cx),
            crate::terminal::TerminalDock::Right => self.close_terminal_right(index, window, cx),
        }
    }

    pub(crate) fn copy_focused_terminal(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some((_, _, terminal)) = self.focused_terminal(window, cx) else {
            return;
        };
        let view = terminal.read(cx).view.clone();
        let copied = view.update(cx, |view, cx| view.copy_selection(cx));
        self.status = if copied {
            "Terminal selection copied".into()
        } else {
            "No terminal selection to copy".into()
        };
        cx.notify();
    }

    pub(crate) fn paste_focused_terminal(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some((_, _, terminal)) = self.focused_terminal(window, cx) else {
            return;
        };
        if terminal.read(cx).state != crate::terminal::TerminalState::Running {
            self.status = "Cannot paste into an exited terminal".into();
            cx.notify();
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.status = "Clipboard does not contain text".into();
            cx.notify();
            return;
        };
        let view = terminal.read(cx).view.clone();
        view.update(cx, |view, cx| view.paste_text(&text, cx));
        self.status = "Pasted into terminal".into();
        cx.notify();
    }

    pub(crate) fn select_all_focused_terminal(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some((_, _, terminal)) = self.focused_terminal(window, cx) else {
            return;
        };
        let view = terminal.read(cx).view.clone();
        view.update(cx, |view, cx| view.select_all(cx));
        self.status = "Terminal contents selected".into();
        cx.notify();
    }

    pub(crate) fn clear_focused_terminal(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some((_, _, terminal)) = self.focused_terminal(window, cx) else {
            return;
        };
        let view = terminal.read(cx).view.clone();
        view.update(cx, |view, cx| view.clear(cx));
        self.status = "Terminal cleared".into();
        cx.notify();
    }

    pub(crate) fn switch_terminal_tab_to(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index < self.terminal_tabs.len() {
            self.active_terminal = index;
            self.reveal_active_terminal_tab();
            self.focus_active_terminal(window, cx);
            self.status = format!("Terminal {} active", index + 1);
            cx.notify();
        }
    }

    pub(crate) fn clear_active_terminal(&mut self, cx: &mut Context<Self>) {
        if let Some(term_entity) = self.terminal_tabs.get(self.active_terminal).cloned() {
            let term = term_entity.read(cx);

            let clear_seq = b"\x1b[2J\x1b[H\x1b[3J";
            if term.send_bytes(clear_seq) {
                self.status = "Terminal cleared".into();
            } else {
                self.status = "Failed to clear terminal".into();
            }
            cx.notify();
        }
    }

    // ------------------------------------------------------------------
    // Right-dock terminal panel (Zed-style): a second terminal surface
    // docked to the right edge, with its own PTY sessions. Nothing is
    // shared with the bottom panel above.
    // ------------------------------------------------------------------

    /// Toggle the right terminal dock. Opening it spawns a fresh PTY when
    /// the dock is still empty (PTYs cannot be restored across restarts).
    pub(crate) fn toggle_terminal_right(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_terminal_right {
            self.hide_terminal_right(window, cx);
            return;
        }

        self.show_terminal_right = true;

        if self.terminal_right_tabs.is_empty() {
            self.new_terminal_right(window, cx);
            return;
        }
        self.focus_active_terminal_right(window, cx);
        self.status = "Right terminal panel active".into();
        cx.notify();
    }

    /// Spawn a new PTY session inside the right dock.
    ///
    /// Deliberately independent of the bottom dock: no shared tabs, no
    /// inherited working directory, and `Terminal::new` always opens a
    /// private PTY pair, so the two panels can never talk to the same shell.
    pub(crate) fn new_terminal_right(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Always start from the project root. The bottom dock inherits the
        // active terminal's cwd; doing the same here would couple this dock
        // to a session it is designed to know nothing about.
        let working_dir = self.root.clone();

        let _id = self.next_terminal_right_id;
        self.next_terminal_right_id += 1;

        let shell_name = crate::terminal::Terminal::detect_shell_name();
        let folder = working_dir
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("app");
        let label = format!("{folder} \u{2013} {shell_name}");
        let palette = self.theme().terminal_palette.clone();
        let term = cx.new(|cx| {
            crate::terminal::Terminal::new(working_dir.as_deref(), label, palette, window, cx)
        });
        self.watch_terminal_title(&term, cx);
        self.terminal_right_tabs.push(term);
        self.active_terminal_right = self.terminal_right_tabs.len() - 1;
        self.show_terminal_right = true;
        self.reveal_active_terminal_right_tab();
        self.focus_active_terminal_right(window, cx);
        self.status = format!("Right terminal {} created", self.active_terminal_right + 1);
        cx.notify();
    }

    /// Scroll the right dock's tab strip so the active tab is visible.
    fn reveal_active_terminal_right_tab(&mut self) {
        if !self.terminal_right_tabs.is_empty() {
            self.terminal_right_tab_scroll
                .scroll_to_item(self.active_terminal_right);
        }
    }

    pub(crate) fn activate_terminal_right(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.terminal_right_tabs.len() {
            return;
        }
        self.active_terminal_right = index;
        if let Some(term) = self.terminal_right_tabs.get(index).cloned() {
            term.read(cx).focus_handle(cx).focus(window);
        }
        self.reveal_active_terminal_right_tab();
        self.status = format!("Right terminal {} active", index + 1);
        cx.notify();
    }

    fn focus_active_terminal_right(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(term) = self
            .terminal_right_tabs
            .get(self.active_terminal_right)
            .cloned()
        {
            term.read(cx).focus_handle(cx).focus(window);
        }
    }

    pub(crate) fn hide_terminal_right(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_terminal_right = false;
        self.status = "Right terminal panel hidden".into();
        self.focus_active_editor_or_self(window, cx);
        cx.notify();
    }

    pub(crate) fn close_terminal_right(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Shared multi-close core: keeps the pinned prefix, the active tab
        // and the inline rename consistent. See workspace/terminal_tabs.rs.
        self.close_terminal_tabs_at(
            crate::terminal::TerminalDock::Right,
            vec![index],
            window,
            cx,
        );
    }

    /// Probe each terminal's child process for exit.
    ///
    /// Called from `render`, so it is throttled: the probe is a syscall per
    /// live terminal and nothing on screen needs sub-frame accuracy. Without
    /// the throttle, ten terminals meant ten `kill(2)` calls on every frame.
    pub(crate) fn poll_terminal_processes(&mut self, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        if now.duration_since(self.last_terminal_poll) < TERMINAL_EXIT_POLL_INTERVAL {
            return;
        }
        self.last_terminal_poll = now;

        // Both docks are polled in one pass: the right dock's terminals are
        // independent sessions, but they exit exactly like the bottom ones.
        let mut terminals = self
            .terminal_tabs
            .iter()
            .chain(self.terminal_right_tabs.iter());

        // Nothing to probe: skip the entity updates entirely so an empty or
        // fully-exited terminal list costs nothing per frame.
        if !terminals.any(|term| term.read(cx).state == crate::terminal::TerminalState::Running) {
            return;
        }

        let mut any_changed = false;
        for term_entity in self
            .terminal_tabs
            .iter()
            .chain(self.terminal_right_tabs.iter())
        {
            let changed = term_entity.update(cx, |term, _cx| term.check_process_exit());
            if changed {
                any_changed = true;
            }
        }
        if any_changed {
            cx.notify();
        }
    }

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

            if activity == Activity::Git {
                self.ensure_git_commit_input(window, cx);
            }
            if activity == Activity::Search {
                self.ensure_search_inputs(window, cx);
                self.focus_search_query(window, cx);
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
        if activity == Activity::Search {
            self.ensure_search_inputs(window, cx);
            self.focus_search_query(window, cx);
        }
        self.status = activity.status_label().into();
        cx.notify();
    }

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
        if let Some(index) = self.explorer_rows.iter().position(|row| row.path == path) {
            // Minimal reveal, clear of the sticky headers — VS Code only
            // scrolls when the row is actually out of view.
            self.explorer_reveal_index(index);
        }
    }

    pub(crate) fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.set_explorer_selection(path.clone());
        self.reveal_tree_path(&path);

        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| t.path.as_ref() == Some(&path))
        {
            self.active_tab = idx;

            if let Some(tab) = self.tabs.get_mut(idx) {
                tab.preview = false;
            }
            cx.notify();
            return;
        }

        self.status = format!("Opening {}…", display_name(&path));
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let load_path = path.clone();
            let loaded = cx
                .background_spawn(async move { load_buffer_file(&load_path) })
                .await;
            let _ = this.update_in(cx, move |workspace, window, cx| {
                workspace.finish_open_file(path, loaded, window, cx);
            });
        })
        .detach();
    }

    fn finish_open_file(
        &mut self,
        path: PathBuf,
        loaded: Result<LoadedBuffer, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let loaded = match loaded {
            Ok(loaded) => loaded,
            Err(message) => {
                self.status = message;
                // A failed open can never satisfy a queued search jump.
                if self
                    .pending_search_jump
                    .as_ref()
                    .map(|(p, _, _)| p == &path)
                    .unwrap_or(false)
                {
                    self.pending_search_jump = None;
                }
                cx.notify();
                return;
            }
        };

        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| t.path.as_ref() == Some(&path))
        {
            self.active_tab = idx;
            if let Some(tab) = self.tabs.get_mut(idx) {
                tab.preview = false;
            }
            cx.notify();
            return;
        }

        let replace_preview = if let Some(active_idx) = self.tabs.get(self.active_tab) {
            active_idx.preview && !active_idx.dirty
        } else {
            false
        };

        let lang_id = loaded.lang_id;
        let highlight = loaded.highlight;
        let text = loaded.text;

        let editor = cx.new(move |cx| {
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
            state.set_value(text, window, cx);
            state
        });

        self.attach_language_server(&path, lang_id, &editor, cx);

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

                    if tab.preview {
                        tab.preview = false;
                        ui_changed = true;
                    }
                }
                {
                    let current_lang = this
                        .tabs
                        .iter()
                        .find(|t| t.path.as_ref() == Some(&path_clone))
                        .and_then(|t| t.language())
                        .unwrap_or(lang_str.as_str());
                    let mut lsp = this.lsp.lock().unwrap();
                    if lsp.has_client(current_lang) {
                        let text = editor_ent.read(cx).value().to_string();
                        lsp.change_document(&path_clone, current_lang, text);
                    }
                }

                if ui_changed {
                    cx.notify();
                }
                this.markdown_buffer_changed(&path_clone, &editor_ent, cx);
                this.on_editor_blame_change(&path_clone, &editor_ent, cx);
                let tab_idx = this.active_tab;
                this.trigger_auto_save_after_delay(tab_idx, cx);
            }
            match event {
                InputEvent::BlameHover { sha, .. } => {
                    this.on_blame_hover(sha.as_ref(), editor_ent.clone(), cx)
                }
                InputEvent::BlameHoverEnd => this.on_blame_hover_end(editor_ent.clone(), cx),
                InputEvent::BlameOpenCommit { sha } => {
                    this.git_view_commit_diff(sha.to_string(), cx)
                }
                _ => {}
            }
        })
        .detach();

        if replace_preview {
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                tab.path = Some(path.clone());
                tab.dirty = false;
                tab.untitled = false;
                tab.preview = true;
                tab.is_settings = false;
                tab.diff = None;
                tab.language_override = None;
                tab.editor = Some(editor);
            }
        } else {
            self.tabs.push(OpenTab {
                path: Some(path.clone()),
                editor: Some(editor),
                dirty: false,
                untitled: false,
                preview: true,
                is_settings: false,
                diff: None,
                language_override: None,
            });
            self.active_tab = self.tabs.len() - 1;
        }
        self.status = if highlight {
            path.display().to_string()
        } else {
            format!("{} (plain text — large file)", display_name(&path))
        };

        let mut global_state = crate::storage::GlobalState::load();
        global_state.add_recent_file(path.clone());
        self.persist_workspace_state(cx);

        // Kick off inline git blame for the freshly opened buffer (off-thread).
        self.recompute_active_blame(cx);

        // A search-result click on a closed file queued `pending_search_jump`:
        // it is consumed in `render` (which owns a `&mut Window` for cursor
        // placement) once this tab is the active one.
        cx.notify();
    }

    pub(crate) fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = match self.active_tab_mut() {
            Some(t) => t,
            None => {
                self.status = "No file open".into();
                cx.notify();
                return;
            }
        };

        if tab.is_settings {
            return;
        }

        if tab.path.is_none() {
            self.save_as(cx);
            return;
        }

        let path = tab.path.clone().unwrap();
        let Some(editor) = tab.editor.clone() else {
            return;
        };
        let language = tab.language().map(|lang| lang.to_string());

        // Format-on-save (Zed's `format_on_save`, `editor.formatOnSave` in
        // settings.json): when enabled and a language server is attached,
        // apply its `textDocument/formatting` edits to the buffer before
        // writing to disk. Anything missing — no language, no server, no
        // formatting capability — falls through to a plain save, so Ctrl+S
        // is never blocked on a formatter.
        let formatter =
            if self.settings.editor_format_on_save == crate::settings::FormatOnSaveMode::On {
                language.and_then(|lang| self.lsp.lock().unwrap().client_for(&lang))
            } else {
                None
            };
        if let Some(client) = formatter {
            self.format_then_save(path, editor, client, window, cx);
            cx.notify();
            return;
        }

        let text = editor.read(cx).value().to_string();
        self.write_file_async(path, text, "Saved", cx);
        cx.notify();
    }

    /// Format-on-save backend: request `textDocument/formatting` for a
    /// snapshot of the buffer, apply the edits, then write the result to
    /// disk via the normal [`Workspace::write_file_async`] path (so dirty
    /// tracking, LSP `didSave`, git status and settings reloading all behave
    /// exactly like a plain save).
    ///
    /// The request runs on the background executor; if the user kept typing
    /// while it was in flight, the snapshot is stale and the edits are
    /// dropped — the *current* text is saved unformatted instead. Formatting
    /// must never clobber concurrent edits, and a save must never be lost.
    fn format_then_save(
        &mut self,
        path: PathBuf,
        editor: Entity<InputState>,
        client: Arc<crate::lsp::client::LspClient>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = editor.read(cx).value().to_string();
        let tab_size = self.settings.editor_tab_size as u32;
        let display = display_name(&path);
        self.status = format!("Formatting {display}…");
        cx.notify();

        let editor_weak = editor.downgrade();
        cx.spawn_in(window, async move |this, cx| {
            // `background_spawn` needs a 'static future, so the request gets
            // its own copies of the snapshot and path; `text` and `path`
            // stay owned by this task for the stale-check and the write.
            let req_text = text.clone();
            let req_path = path.clone();
            let edits = cx
                .background_spawn(
                    async move { client.format_document(&req_path, &req_text, tab_size) },
                )
                .await;
            // Apply the edits to the live buffer (guarded against a stale
            // snapshot), then persist whatever the buffer now contains.
            let saved_text: Option<String> = editor_weak
                .update_in(cx, |state, window, cx| {
                    let current = state.value().to_string();
                    match edits {
                        Some(edits) if !edits.is_empty() && current == text => {
                            state.apply_lsp_edits(&edits, window, cx);
                            Some(state.value().to_string())
                        }
                        _ => Some(current),
                    }
                })
                .ok()
                .flatten();
            let _ = this.update(cx, |workspace, cx| {
                match saved_text {
                    Some(text) => workspace.write_file_async(path, text, "Saved", cx),
                    // The tab was closed while formatting; nothing to save.
                    None => workspace.status = format!("{display} was closed while formatting"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn write_file_async(
        &mut self,
        path: PathBuf,
        text: String,
        done_label: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.status = format!("Saving {}…", display_name(&path));
        cx.spawn(async move |this, cx| {
            let write_path = path.clone();
            let write_text = text.clone();
            let result = cx
                .background_spawn(async move { std::fs::write(&write_path, write_text.as_bytes()) })
                .await;
            let _ = this.update(cx, |workspace, cx| {
                match result {
                    Ok(()) => {
                        if let Some(tab) = workspace
                            .tabs
                            .iter_mut()
                            .find(|t| t.path.as_ref() == Some(&path))
                        {
                            if let Some(editor) = &tab.editor {
                                if editor.read(cx).value() == text {
                                    tab.dirty = false;
                                }
                            }
                        }
                        // Tell the server the file hit disk.
                        let lang_id = workspace
                            .tabs
                            .iter()
                            .find(|t| t.path.as_ref() == Some(&path))
                            .and_then(|t| t.language())
                            .or_else(|| lang::language_for(&path));
                        if let Some(lang_id) = lang_id {
                            workspace
                                .lsp
                                .lock()
                                .unwrap()
                                .save_document(&path, lang_id, &text);
                        }
                        workspace.git_poke();
                        if path == crate::settings::settings_file_path() {
                            workspace.reload_settings(cx);
                        }
                        workspace.status = format!("{done_label} {}", display_name(&path));
                        workspace.persist_workspace_state(cx);
                    }
                    Err(e) => {
                        workspace.status = format!("save failed: {e}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save_as(&mut self, cx: &mut Context<Self>) {
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

        self.status = "Choose save location…".into();
        cx.notify();

        // Native save dialogs pump Windows messages; never call them while App is borrowed.
        cx.spawn(async move |this, cx| {
            let path = rfd::FileDialog::new()
                .set_file_name("untitled.txt")
                .save_file();

            let Some(path) = path else {
                let _ = this.update(cx, |workspace, cx| {
                    workspace.status = "Save cancelled".into();
                    cx.notify();
                });
                return;
            };

            let write_path = path.clone();
            let write_text = text.clone();
            let result = cx
                .background_spawn(async move { std::fs::write(&write_path, write_text.as_bytes()) })
                .await;

            let _ = this.update(cx, move |workspace, cx| {
                match result {
                    Ok(()) => {
                        let lang_id = lang::language_for(&path).unwrap_or("text");
                        if let Some(tab) = workspace.tabs.get_mut(active_idx) {
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
                        if let Some(editor) = workspace
                            .tabs
                            .get(active_idx)
                            .and_then(|t| t.editor.clone())
                        {
                            workspace.attach_language_server(&path, lang_id, &editor, cx);
                        }
                        workspace.selected_path = Some(path.clone());
                        if let Some(parent) = path.parent().map(|p| p.to_path_buf()) {
                            workspace.reload_dir(&parent, cx);
                        }
                        workspace.git_poke();
                        workspace.status = format!("Saved {}", display_name(&path));
                    }
                    Err(e) => workspace.status = format!("save failed: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn active_tab_mut(&mut self) -> Option<&mut OpenTab> {
        self.tabs.get_mut(self.active_tab)
    }

    pub(crate) fn toggle_dir(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.selected_path = Some(path.to_path_buf());
        fn rec(nodes: &mut [TreeNode], path: &Path) -> (bool, Option<PathBuf>) {
            for n in nodes {
                if n.path == path {
                    n.expanded = !n.expanded;

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
        self.persist_workspace_state(cx);
        cx.notify();
        if let Some(dir) = dir_to_load {
            self.load_directory_async(dir, cx);
        }
    }

    pub(crate) fn refresh_explorer(&mut self, cx: &mut Context<Self>) {
        self.workspace_files_cache = None;
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

    fn apply_loaded_dir_inner(&mut self, dir: &Path, entries: Vec<TreeNode>) -> bool {
        let Some(root) = self.root.clone() else {
            return false;
        };
        if dir == root.as_path() {
            let previous = std::mem::take(&mut self.tree);
            self.tree = merge_loaded_dir(dir, previous, entries);
            return true;
        }

        fn apply(nodes: &mut [TreeNode], dir: &Path, entries: &mut Option<Vec<TreeNode>>) -> bool {
            for node in nodes {
                if node.path == dir {
                    if node.is_dir && node.expanded {
                        let previous = std::mem::take(&mut node.children);
                        node.children =
                            merge_loaded_dir(dir, previous, entries.take().unwrap_or_default());
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

    pub(crate) fn reload_dir(&mut self, dir: &Path, cx: &mut Context<Self>) {
        self.load_directory_async(dir.to_path_buf(), cx);
    }

    pub(crate) fn collapse_all_folders(&mut self, cx: &mut Context<Self>) {
        collapse_all(&mut self.tree);
        self.rebuild_explorer_rows();
        self.persist_workspace_state(cx);
        self.status = "Collapsed all folders".into();
        cx.notify();
    }

    pub(crate) fn toggle_explorer_section(&mut self, cx: &mut Context<Self>) {
        self.explorer_section_expanded = !self.explorer_section_expanded;
        self.rebuild_explorer_rows();
        cx.notify();
    }

    // ---------------------------------------------------------------------
    // Selection
    //
    // `selected_path` is the focused row and `explorer_selection` the full
    // multi-selection. The set is only trusted while it still contains the
    // focused row, so code elsewhere that just assigns `selected_path` (open
    // a file, reveal a search hit, finish a rename…) implicitly collapses the
    // selection instead of leaving stale highlights behind.
    // ---------------------------------------------------------------------

    /// Every entry the next explorer command should act on.
    pub(crate) fn explorer_selected_entries(&self) -> Vec<PathBuf> {
        let Some(focused) = self.selected_path.clone() else {
            return Vec::new();
        };
        if self.explorer_selection.contains(&focused) {
            self.explorer_selection.clone()
        } else {
            vec![focused]
        }
    }

    pub(crate) fn set_explorer_selection(&mut self, path: PathBuf) {
        self.explorer_selection = vec![path.clone()];
        self.explorer_selection_anchor = Some(path.clone());
        self.selected_path = Some(path);
    }

    pub(crate) fn set_explorer_selection_many(&mut self, paths: Vec<PathBuf>) {
        if let Some(last) = paths.last().cloned() {
            self.explorer_selection = paths;
            self.explorer_selection_anchor = Some(last.clone());
            self.selected_path = Some(last);
        }
    }

    pub(crate) fn clear_explorer_selection(&mut self, cx: &mut Context<Self>) {
        self.explorer_selection.clear();
        self.explorer_selection_anchor = None;
        self.selected_path = None;
        self.explorer_typeahead.clear();
        cx.notify();
    }

    /// Ctrl/Cmd+click: add or remove one row without disturbing the rest.
    pub(crate) fn toggle_explorer_selection(&mut self, path: PathBuf) {
        let mut selection = self.explorer_selected_entries();
        if let Some(ix) = selection.iter().position(|item| item == &path) {
            selection.remove(ix);
            self.explorer_selection = selection;
            self.selected_path = self.explorer_selection.last().cloned();
        } else {
            selection.push(path.clone());
            self.explorer_selection = selection;
            self.selected_path = Some(path.clone());
            self.explorer_selection_anchor = Some(path);
        }
    }

    /// Shift+click / Shift+arrow: select the inclusive range of visible rows
    /// between the anchor and `path`.
    pub(crate) fn extend_explorer_selection(&mut self, path: PathBuf) {
        let rows = Arc::clone(&self.explorer_rows);
        let target = rows.iter().position(|row| row.path == path);
        // Only trust the stored anchor while the selection it belongs to is
        // still live; otherwise the focused row is the anchor.
        let selection_is_live = self
            .selected_path
            .as_ref()
            .is_some_and(|focused| self.explorer_selection.contains(focused));
        let anchor_path = if selection_is_live {
            self.explorer_selection_anchor.clone()
        } else {
            self.selected_path.clone()
        };
        let anchor = anchor_path
            .as_ref()
            .and_then(|anchor| rows.iter().position(|row| &row.path == anchor));
        match (target, anchor) {
            (Some(target), Some(anchor)) => {
                let (lo, hi) = if anchor <= target {
                    (anchor, target)
                } else {
                    (target, anchor)
                };
                self.explorer_selection = rows[lo..=hi]
                    .iter()
                    .map(|row| row.path.clone())
                    .collect::<Vec<_>>();
                self.selected_path = Some(path);
            }
            _ => self.set_explorer_selection(path),
        }
    }

    pub(crate) fn select_all_explorer_rows(&mut self, cx: &mut Context<Self>) {
        if self.explorer_rows.is_empty() {
            return;
        }
        self.explorer_selection = self
            .explorer_rows
            .iter()
            .map(|row| row.path.clone())
            .collect::<Vec<_>>();
        if self.selected_path.is_none() {
            self.selected_path = self.explorer_selection.first().cloned();
        }
        cx.notify();
    }

    // ---------------------------------------------------------------------
    // Mouse
    // ---------------------------------------------------------------------

    /// A click on a tree row, with VS Code's modifier rules: plain click
    /// selects (and opens files as a preview tab / toggles folders), Ctrl or
    /// Cmd toggles one row, Shift extends the range, Alt on a folder expands
    /// or collapses the whole subtree, and a double click pins the preview.
    pub(crate) fn explorer_row_click(
        &mut self,
        path: PathBuf,
        is_dir: bool,
        modifiers: gpui::Modifiers,
        click_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.explorer_focus_handle);
        self.explorer_typeahead.clear();

        if modifiers.shift {
            self.extend_explorer_selection(path);
            cx.notify();
            return;
        }
        if modifiers.control || modifiers.platform {
            self.toggle_explorer_selection(path);
            cx.notify();
            return;
        }

        self.set_explorer_selection(path.clone());
        if is_dir {
            if modifiers.alt {
                self.toggle_dir_recursive(&path, cx);
            } else {
                self.toggle_dir(&path, cx);
            }
            return;
        }

        self.open_file(path.clone(), window, cx);
        if click_count >= 2 {
            self.pin_preview_tab(&path, cx);
        }
    }

    /// Double clicking a file in VS Code turns the italic preview tab into a
    /// permanent one.
    pub(crate) fn pin_preview_tab(&mut self, path: &Path, cx: &mut Context<Self>) {
        if let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.path.as_deref() == Some(path))
        {
            if tab.preview {
                tab.preview = false;
                cx.notify();
            }
        }
    }

    /// Alt+click on a twistie: expand or collapse every directory below this
    /// one. Directories that were never read are fetched off the UI thread.
    pub(crate) fn toggle_dir_recursive(&mut self, path: &Path, cx: &mut Context<Self>) {
        let mut needs_load = Vec::new();
        let expanded = crate::fs_tree::with_node_mut(&mut self.tree, path, &mut |node| {
            if !node.is_dir {
                return false;
            }
            let expand = !node.expanded;
            node.expanded = expand;
            if expand && !node.children_loaded {
                needs_load.push(node.path.clone());
            }
            crate::fs_tree::set_expanded_recursive(&mut node.children, expand, &mut needs_load);
            expand
        });
        if expanded.is_none() {
            return;
        }
        self.rebuild_explorer_rows();
        self.persist_workspace_state(cx);
        cx.notify();
        for dir in needs_load {
            self.load_directory_async(dir, cx);
        }
    }

    // ---------------------------------------------------------------------
    // Drag and drop
    // ---------------------------------------------------------------------

    /// Hovering a collapsed folder mid-drag expands it after a short delay,
    /// so a file can be dropped deep into the tree in one gesture.
    pub(crate) fn explorer_drag_over(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if self.explorer_drag_target.as_ref() == Some(&dir) {
            return;
        }
        self.explorer_drag_target = Some(dir.clone());
        self.explorer_drag_generation = self.explorer_drag_generation.wrapping_add(1);
        let generation = self.explorer_drag_generation;
        cx.notify();

        let already_expanded = self
            .explorer_rows
            .iter()
            .any(|row| row.path == dir && row.expanded);
        if already_expanded {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let _ = this.update(cx, |workspace, cx| {
                if workspace.explorer_drag_generation == generation
                    && workspace.explorer_drag_target.as_ref() == Some(&dir)
                {
                    let collapsed = workspace
                        .explorer_rows
                        .iter()
                        .any(|row| row.path == dir && row.is_dir && !row.expanded);
                    if collapsed {
                        workspace.toggle_dir(&dir, cx);
                    }
                }
            });
        })
        .detach();
    }

    pub(crate) fn explorer_drag_leave(&mut self, dir: &Path, cx: &mut Context<Self>) {
        if self.explorer_drag_target.as_deref() == Some(dir) {
            self.explorer_drag_target = None;
            cx.notify();
        }
    }

    /// Drop handler: dropping onto a file targets its folder, and dragging a
    /// row that is part of the selection moves the whole selection.
    pub(crate) fn explorer_drop(
        &mut self,
        dragged: &Path,
        destination: &Path,
        cx: &mut Context<Self>,
    ) {
        self.explorer_drag_target = None;
        let destination_dir = if destination.is_dir() {
            destination.to_path_buf()
        } else {
            match destination.parent() {
                Some(parent) => parent.to_path_buf(),
                None => return,
            }
        };

        let selection = self.explorer_selected_entries();
        let sources = if selection.iter().any(|path| path == dragged) {
            selection
        } else {
            vec![dragged.to_path_buf()]
        };

        let mut moved = Vec::new();
        for source in sources {
            let Some(name) = source.file_name() else {
                continue;
            };
            if self.move_entry(&source, &destination_dir, cx) {
                moved.push(destination_dir.join(name));
            }
        }
        if !moved.is_empty() {
            self.ensure_directory_visible(&destination_dir);
            self.rebuild_explorer_rows();
            self.set_explorer_selection_many(moved);
        }
        cx.notify();
    }

    // ---------------------------------------------------------------------
    // Keyboard
    // ---------------------------------------------------------------------

    pub(crate) fn handle_explorer_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;

        if self.inline_creating.is_some() || self.inline_renaming.is_some() {
            return;
        }

        if modifiers.control || modifiers.platform {
            match (key, modifiers.shift) {
                ("c", false) => self.explorer_copy(cx),
                ("x", false) => self.explorer_cut(cx),
                ("v", false) => self.explorer_paste(cx),
                ("a", false) => self.select_all_explorer_rows(cx),
                ("n", true) => {
                    let selected_folder = self.selected_path.clone().filter(|path| path.is_dir());
                    self.start_inline_create(CreatingKind::Folder, selected_folder, window, cx);
                }
                _ => return,
            }
            cx.stop_propagation();
            return;
        }

        let rows = Arc::clone(&self.explorer_rows);
        if rows.is_empty() {
            return;
        }
        let current = self
            .selected_path
            .as_ref()
            .and_then(|selected| rows.iter().position(|row| &row.path == selected));

        if key == "escape" {
            self.explorer_typeahead.clear();
            if self.explorer_selection.len() > 1 {
                if let Some(focused) = self.selected_path.clone() {
                    self.set_explorer_selection(focused);
                }
            }
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if key == "f2" {
            if let Some(path) = self.selected_path.clone() {
                self.start_inline_rename(path, window, cx);
                cx.stop_propagation();
            }
            return;
        }
        if matches!(key, "delete" | "backspace") {
            self.delete_selected_entries(cx);
            cx.stop_propagation();
            return;
        }

        let current_ix = current.unwrap_or(0);
        let page = self.explorer_page_size();

        match key {
            "arrowdown" | "down" => {
                let next = (current_ix + 1).min(rows.len() - 1);
                self.move_explorer_focus(next, modifiers.shift, cx);
            }
            "arrowup" | "up" => {
                self.move_explorer_focus(current_ix.saturating_sub(1), modifiers.shift, cx);
            }
            "pagedown" => {
                let next = (current_ix + page).min(rows.len() - 1);
                self.move_explorer_focus(next, modifiers.shift, cx);
            }
            "pageup" => {
                self.move_explorer_focus(current_ix.saturating_sub(page), modifiers.shift, cx);
            }
            "home" => self.move_explorer_focus(0, modifiers.shift, cx),
            "end" => self.move_explorer_focus(rows.len() - 1, modifiers.shift, cx),
            "arrowright" | "right" => {
                let row = &rows[current_ix];
                if row.is_dir && !row.expanded {
                    let path = row.path.clone();
                    self.toggle_dir(&path, cx);
                } else if row.is_dir {
                    if let Some(next) = rows.get(current_ix + 1) {
                        if next.depth > row.depth {
                            self.move_explorer_focus(current_ix + 1, false, cx);
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
                    if let Some(parent_ix) =
                        (0..current_ix).rev().find(|&ix| rows[ix].depth < row.depth)
                    {
                        self.move_explorer_focus(parent_ix, false, cx);
                    }
                }
            }
            "enter" | "space" => {
                let path = rows[current_ix].path.clone();
                if rows[current_ix].is_dir {
                    self.toggle_dir(&path, cx);
                } else {
                    self.open_file(path.clone(), window, cx);
                    if key == "enter" {
                        self.pin_preview_tab(&path, cx);
                    }
                }
            }
            "*" => {
                // VS Code expands the whole subtree of the focused folder.
                let row = &rows[current_ix];
                if row.is_dir {
                    let path = row.path.clone();
                    self.toggle_dir_recursive(&path, cx);
                }
            }
            _ => {
                if !self.explorer_type_ahead(key, modifiers, current_ix, cx) {
                    return;
                }
            }
        }
        cx.stop_propagation();
    }

    /// Roughly how many rows fit in the panel; used by PageUp/PageDown.
    fn explorer_page_size(&self) -> usize {
        let viewport = self
            .explorer_scroll_handle
            .0
            .borrow()
            .last_item_size
            .map(|size| f32::from(size.item.height))
            .unwrap_or(0.0);
        let row_height = crate::ui::sidebar::explorer::ROW_HEIGHT;
        if viewport > row_height {
            ((viewport / row_height).floor() as usize)
                .saturating_sub(1)
                .max(1)
        } else {
            10
        }
    }

    /// Jump to the next row whose name starts with the typed characters, the
    /// way VS Code's trees respond to typing. Returns whether the key was
    /// consumed.
    fn explorer_type_ahead(
        &mut self,
        key: &str,
        modifiers: gpui::Modifiers,
        current_ix: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        if modifiers.control || modifiers.platform || modifiers.alt || modifiers.function {
            return false;
        }
        let mut chars = key.chars();
        let Some(ch) = chars.next() else {
            return false;
        };
        if chars.next().is_some() || ch.is_control() || ch == ' ' {
            return false;
        }

        self.explorer_typeahead.push(ch);
        // Repeating the same letter cycles through matches, as in VS Code.
        let repeated = self.explorer_typeahead.chars().all(|c| c == ch);
        let query = if repeated && self.explorer_typeahead.chars().count() > 1 {
            ch.to_string()
        } else {
            self.explorer_typeahead.clone()
        };
        let start = if query.chars().count() == 1 {
            current_ix + 1
        } else {
            current_ix
        };
        if let Some(index) = crate::fs_tree::type_ahead_index(&self.explorer_rows, start, &query) {
            let path = self.explorer_rows[index].path.clone();
            self.set_explorer_selection(path);
            self.explorer_reveal_index(index);
            cx.notify();
        }

        // Expire the buffer the way a list search box would.
        self.explorer_typeahead_generation = self.explorer_typeahead_generation.wrapping_add(1);
        let generation = self.explorer_typeahead_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(900))
                .await;
            let _ = this.update(cx, |workspace, _cx| {
                if workspace.explorer_typeahead_generation == generation {
                    workspace.explorer_typeahead.clear();
                }
            });
        })
        .detach();
        true
    }

    fn move_explorer_focus(&mut self, index: usize, extend: bool, cx: &mut Context<Self>) {
        let Some(row) = self.explorer_rows.get(index) else {
            return;
        };
        let path = row.path.clone();
        if extend {
            self.extend_explorer_selection(path);
        } else {
            self.set_explorer_selection(path);
        }
        self.explorer_reveal_index(index);
        cx.notify();
    }

    /// Scroll just enough to bring a row into view, keeping it clear of the
    /// sticky header stack — VS Code never parks the focused row under it.
    pub(crate) fn explorer_reveal_index(&self, index: usize) {
        let sticky = if self.explorer_sticky_scroll {
            self.explorer_sticky_rows
        } else {
            0
        };
        self.explorer_scroll_handle
            .scroll_to_item_with_offset(index, ScrollStrategy::Top, sticky);
    }

    /// Delete everything currently selected (VS Code deletes the whole
    /// selection, not just the focused row).
    pub(crate) fn delete_selected_entries(&mut self, cx: &mut Context<Self>) {
        let entries = self.explorer_selected_entries();
        if entries.is_empty() {
            return;
        }
        // Pick the row that should take focus afterwards, like VS Code does.
        let next_focus = self
            .explorer_rows
            .iter()
            .position(|row| entries.iter().any(|path| path == &row.path))
            .and_then(|first| {
                self.explorer_rows
                    .iter()
                    .skip(first)
                    .find(|row| {
                        !entries
                            .iter()
                            .any(|path| is_same_or_descendant(path, &row.path))
                    })
                    .map(|row| row.path.clone())
            });
        for path in &entries {
            self.delete_entry(path, cx);
        }
        self.explorer_selection.clear();
        if let Some(next) = next_focus.filter(|path| path.exists()) {
            self.set_explorer_selection(next);
        }
        cx.notify();
    }

    fn path_in_workspace(&self, path: &Path) -> bool {
        self.root
            .as_deref()
            .is_some_and(|root| is_same_or_descendant(root, path))
    }

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
        cx.subscribe(&input, |this, _state, event: &InputEvent, cx| match event {
            InputEvent::PressEnter { .. } => this.confirm_inline_create(cx),
            InputEvent::Blur => this.cancel_inline_create(cx),
            _ => {}
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
        let invalid = !name_is_valid || !self.path_in_workspace(&target_path) || target_exists;
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
        cx.subscribe(&input, |this, _state, event: &InputEvent, cx| match event {
            InputEvent::PressEnter { .. } => this.confirm_inline_rename(cx),
            InputEvent::Blur => this.cancel_inline_rename(cx),
            _ => {}
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
                (
                    path_after_move(&path, source, destination).unwrap_or(path),
                    value,
                )
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
        let paths = self.explorer_selected_entries();
        if paths.is_empty() {
            return;
        }
        self.status = if paths.len() == 1 {
            format!("Copied {}", display_name(&paths[0]))
        } else {
            format!("Copied {} items", paths.len())
        };
        self.explorer_clipboard = Some(ExplorerClipboard { paths, cut: false });
        cx.notify();
    }

    pub(crate) fn explorer_cut(&mut self, cx: &mut Context<Self>) {
        let paths = self.explorer_selected_entries();
        if paths.is_empty() {
            return;
        }
        self.status = if paths.len() == 1 {
            format!("Cut {}", display_name(&paths[0]))
        } else {
            format!("Cut {} items", paths.len())
        };
        self.explorer_clipboard = Some(ExplorerClipboard { paths, cut: true });
        cx.notify();
    }

    /// Where a paste lands: the focused folder, otherwise the focused file's
    /// folder, otherwise the workspace root — same rule as VS Code.
    pub(crate) fn explorer_paste_target(&self) -> Option<PathBuf> {
        self.selected_path
            .as_ref()
            .filter(|path| path.is_dir())
            .cloned()
            .or_else(|| {
                self.selected_path
                    .as_ref()
                    .and_then(|path| path.parent().map(Path::to_path_buf))
            })
            .or_else(|| self.root.clone())
    }

    /// `report.md` pasted next to itself becomes `report copy.md`, then
    /// `report copy 2.md`, matching VS Code's naming.
    fn unique_copy_destination(directory: &Path, name: &std::ffi::OsStr) -> Option<PathBuf> {
        let direct = directory.join(name);
        if !direct.exists() {
            return Some(direct);
        }
        let name = name.to_str()?;
        let (stem, extension) = match name.rfind('.') {
            // A leading dot is part of the name (`.gitignore`), not a suffix.
            Some(ix) if ix > 0 => (&name[..ix], &name[ix..]),
            _ => (name, ""),
        };
        for attempt in 1..1000 {
            let candidate = if attempt == 1 {
                format!("{stem} copy{extension}")
            } else {
                format!("{stem} copy {attempt}{extension}")
            };
            let candidate = directory.join(candidate);
            if !candidate.exists() {
                return Some(candidate);
            }
        }
        None
    }

    pub(crate) fn explorer_paste(&mut self, cx: &mut Context<Self>) {
        let Some(clipboard) = self.explorer_clipboard.clone() else {
            return;
        };
        let Some(destination_dir) = self.explorer_paste_target() else {
            return;
        };

        if clipboard.cut {
            let mut moved = 0;
            for path in &clipboard.paths {
                if self.move_entry(path, &destination_dir, cx) {
                    moved += 1;
                }
            }
            if moved == clipboard.paths.len() {
                self.explorer_clipboard = None;
            }
            return;
        }

        let mut pasted: Vec<PathBuf> = Vec::new();
        for source in &clipboard.paths {
            if !self.path_in_workspace(source) || !source.exists() {
                self.status = "Cannot paste: the copied item is no longer available".into();
                continue;
            }
            if is_same_or_descendant(source, &destination_dir) {
                self.status = "Cannot paste a folder into itself".into();
                continue;
            }
            let Some(name) = source.file_name() else {
                continue;
            };
            let Some(destination) = Self::unique_copy_destination(&destination_dir, name) else {
                self.status = "Cannot paste: too many copies with that name".into();
                continue;
            };
            match Self::copy_entry_recursive(source, &destination) {
                Ok(()) => pasted.push(destination),
                Err(error) => {
                    if destination.is_dir() {
                        let _ = std::fs::remove_dir_all(&destination);
                    } else {
                        let _ = std::fs::remove_file(&destination);
                    }
                    self.status = format!("Could not paste: {error}");
                }
            }
        }

        if !pasted.is_empty() {
            self.ensure_directory_visible(&destination_dir);
            self.reload_dir(&destination_dir, cx);
            self.status = if pasted.len() == 1 {
                format!("Pasted {}", display_name(&pasted[0]))
            } else {
                format!("Pasted {} items", pasted.len())
            };
            self.set_explorer_selection_many(pasted);
            self.git_poke();
        }
        cx.notify();
    }

    /// VS Code's "Duplicate": copy next to the original as `name copy.ext`.
    pub(crate) fn explorer_duplicate(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.path_in_workspace(path) || !path.exists() {
            return;
        }
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return;
        };
        let Some(destination) = Self::unique_copy_destination(parent, name) else {
            self.status = "Could not duplicate: too many copies with that name".into();
            cx.notify();
            return;
        };
        match Self::copy_entry_recursive(path, &destination) {
            Ok(()) => {
                self.reload_dir(parent, cx);
                self.status = format!("Duplicated to {}", display_name(&destination));
                self.set_explorer_selection(destination);
                self.git_poke();
            }
            Err(error) => self.status = format!("Could not duplicate: {error}"),
        }
        cx.notify();
    }

    /// VS Code's "Find in Folder…": jump to search, scoped to that folder.
    pub(crate) fn explorer_find_in_folder(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let directory = if path.is_dir() {
            path.to_path_buf()
        } else {
            match path.parent() {
                Some(parent) => parent.to_path_buf(),
                None => return,
            }
        };
        let relative = self
            .root
            .as_ref()
            .and_then(|root| directory.strip_prefix(root).ok())
            .unwrap_or(directory.as_path())
            .to_string_lossy()
            .into_owned();
        let filter = if relative.is_empty() {
            String::new()
        } else {
            format!("{relative}/**")
        };

        self.set_activity_explicit(Activity::Search, window, cx);
        if let Some(input) = self.search_include_input.clone() {
            input.update(cx, |state, cx| state.set_value(filter, window, cx));
        }
        self.focus_search_query(window, cx);
        self.status = format!("Searching in {}", display_name(&directory));
        cx.notify();
    }

    /// VS Code's "Open in Integrated Terminal".
    pub(crate) fn explorer_open_in_terminal(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let directory = if path.is_dir() {
            path.to_path_buf()
        } else {
            match path.parent() {
                Some(parent) => parent.to_path_buf(),
                None => return,
            }
        };
        self.new_terminal_in(Some(directory), window, cx);
    }

    pub(crate) fn toggle_explorer_sticky_scroll(&mut self, cx: &mut Context<Self>) {
        self.explorer_sticky_scroll = !self.explorer_sticky_scroll;
        if !self.explorer_sticky_scroll {
            self.explorer_sticky_rows = 0;
        }
        self.status = if self.explorer_sticky_scroll {
            "Explorer sticky scroll on".into()
        } else {
            "Explorer sticky scroll off".into()
        };
        self.persist_workspace_state(cx);
        cx.notify();
    }

    /// Context-menu delete: deletes the whole selection when the clicked row
    /// is part of it, otherwise just that row — VS Code's rule.
    pub(crate) fn explorer_delete_action(&mut self, path: &Path, cx: &mut Context<Self>) {
        let selection = self.explorer_selected_entries();
        if selection.len() > 1 && selection.iter().any(|item| item == path) {
            self.delete_selected_entries(cx);
        } else {
            self.delete_entry(path, cx);
            cx.notify();
        }
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
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
            path.to_string_lossy().to_string(),
        ));
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
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
            rel.to_string_lossy().to_string(),
        ));
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
                        .is_none_or(|tab_path| !is_same_or_descendant(path, tab_path));
                    let diff_path_is_alive = tab
                        .diff
                        .as_ref()
                        .is_none_or(|diff| !is_same_or_descendant(path, &diff.path));
                    editor_path_is_alive && diff_path_is_alive
                });
                self.diagnostics_by_path
                    .retain(|tab_path, _| !is_same_or_descendant(path, tab_path));
                if self
                    .selected_path
                    .as_ref()
                    .is_some_and(|selected| is_same_or_descendant(path, selected))
                {
                    self.selected_path = path.parent().map(Path::to_path_buf);
                }
                if self
                    .pending_open
                    .as_ref()
                    .is_some_and(|pending| is_same_or_descendant(path, pending))
                {
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

    pub(crate) fn start_server_for_open_buffers(&mut self, server: &str, cx: &mut Context<Self>) {
        let languages: Vec<&'static str> = self
            .lsp
            .lock()
            .unwrap()
            .languages_for_server(server)
            .to_vec();

        // Collect first: attaching borrows `self` mutably.
        let targets: Vec<(PathBuf, String, Entity<InputState>)> = self
            .tabs
            .iter()
            .filter_map(|tab| {
                let path = tab.path.clone()?;
                let editor = tab.editor.clone()?;
                let lang = tab.language()?.to_string();
                languages
                    .iter()
                    .any(|&l| l == lang)
                    .then_some((path, lang, editor))
            })
            .collect();

        for (path, lang, editor) in targets {
            self.attach_language_server(&path, &lang, &editor, cx);
        }
    }

    pub(crate) fn apply_diagnostics(
        &mut self,
        path: &Path,
        diagnostics: Vec<lsp_types::Diagnostic>,
        cx: &mut Context<Self>,
    ) {
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
                            .map(|d| {
                                format!("{}: {}", d.source.as_deref().unwrap_or("error"), d.message)
                            })
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

    pub(crate) fn git_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.git.as_ref().map(|g| g.root.clone()) else {
            self.status = "Not a git repository".into();
            cx.notify();
            return;
        };
        self.status = "Refreshing source control…".into();
        cx.notify();
        let generation = self.git_watch_generation;
        cx.spawn(async move |this, cx| {
            let status = cx.background_spawn(async move { git::status(&root) }).await;
            let _ = this.update(cx, |workspace, cx| {
                if workspace.git_watch_generation != generation {
                    return;
                }
                match status {
                    Some(status) => {
                        workspace.apply_git_status(status, cx);
                        workspace.status = "Source control refreshed".into();
                    }
                    None => workspace.status = "git status failed".into(),
                }
                cx.notify();
            });
        })
        .detach();
    }

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

    /// Ask before discarding: Zed and VS Code both confirm this, because a
    /// discard is the one git-panel action that destroys work irreversibly.
    pub(crate) fn git_request_discard_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some((_, change)) = self.git_change_for(path) else {
            self.status = "Not a changed file".into();
            cx.notify();
            return;
        };
        let name = display_name(path);
        let (title, detail, label) = if change.is_untracked() {
            (
                format!("Delete untracked file \"{name}\"?"),
                "The file is not tracked by git; discarding it deletes it from disk. \
                 This cannot be undone."
                    .to_string(),
                "Delete File".to_string(),
            )
        } else {
            (
                format!("Discard changes in \"{name}\"?"),
                "The file will be restored to its last committed state. \
                 Unsaved and uncommitted edits are lost permanently."
                    .to_string(),
                "Discard Changes".to_string(),
            )
        };
        self.git_confirm = Some(GitConfirm {
            title,
            detail,
            confirm_label: label,
            action: GitConfirmAction::DiscardPath(path.to_path_buf()),
        });
        cx.notify();
    }

    pub(crate) fn git_request_discard_all(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.git.as_ref() else {
            self.status = "Not a git repository".into();
            cx.notify();
            return;
        };
        let count = repo.change_count();
        if count == 0 {
            self.status = "No changes to discard".into();
            cx.notify();
            return;
        }
        self.git_confirm = Some(GitConfirm {
            title: format!("Discard all changes in {count} file(s)?"),
            detail: "Tracked files are restored to their last committed state and \
                     untracked files are deleted from disk. This cannot be undone."
                .to_string(),
            confirm_label: "Discard All".to_string(),
            action: GitConfirmAction::DiscardAll,
        });
        cx.notify();
    }

    pub(crate) fn git_confirm_accept(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.git_confirm.take() else {
            return;
        };
        match confirm.action {
            GitConfirmAction::DiscardPath(path) => self.git_discard_path(&path, cx),
            GitConfirmAction::DiscardAll => self.git_discard_all(cx),
        }
    }

    pub(crate) fn git_confirm_cancel(&mut self, cx: &mut Context<Self>) {
        if self.git_confirm.take().is_some() {
            self.status = "Discard cancelled".into();
            cx.notify();
        }
    }

    fn git_discard_path(&mut self, path: &Path, cx: &mut Context<Self>) {
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

    pub(crate) fn git_stage_all(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.git.as_ref().map(|g| g.root.clone()) else {
            self.status = "Not a git repository".into();
            cx.notify();
            return;
        };
        self.run_git_op(
            root,
            Vec::new(),
            |root, _| git::stage_all(&root),
            "Staged all changes",
            cx,
        );
    }

    pub(crate) fn git_unstage_all(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.git.as_ref().map(|g| g.root.clone()) else {
            self.status = "Not a git repository".into();
            cx.notify();
            return;
        };
        let rels: Vec<String> = self
            .git
            .as_ref()
            .map(|g| {
                g.changes
                    .iter()
                    .filter(|c| c.is_staged())
                    .map(|c| c.rel.clone())
                    .collect()
            })
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

    fn git_discard_all(&mut self, cx: &mut Context<Self>) {
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
                    // Discardable work: tracked files with worktree edits
                    // (including conflicted ones) plus untracked files.
                    // Staged-only entries are left alone — `git restore`
                    // without `--staged` wouldn't touch them anyway.
                    .filter(|c| c.is_untracked() || c.worktree.is_some())
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

    pub(crate) fn toggle_git_section(&mut self, section: GitSection, cx: &mut Context<Self>) {
        let flag = match section {
            GitSection::Repo => &mut self.git_repo_section_expanded,
            GitSection::Conflicts => &mut self.git_conflicts_expanded,
            GitSection::Staged => &mut self.git_staged_expanded,
            GitSection::Changes => &mut self.git_changes_expanded,
            GitSection::Untracked => &mut self.git_untracked_expanded,
        };
        *flag = !*flag;
        cx.notify();
    }

    pub(crate) fn toggle_split_diff(&mut self, cx: &mut Context<Self>) {
        self.split_diff = !self.split_diff;
        cx.notify();
    }

    pub(crate) fn ensure_git_commit_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(input) = &self.git_commit_input {
            return input.clone();
        }
        let branch = self
            .git
            .as_ref()
            .and_then(|g| g.branch.as_deref())
            .unwrap_or("main");
        let placeholder_text = format!("Message (Enter to commit on \"{branch}\")");
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder_text));

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

    /// Commit the staged changes. Zed-style fallback: when nothing is staged
    /// but tracked files are dirty, the commit covers all tracked changes.
    pub(crate) fn git_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (staged, tracked_dirty) = self
            .git
            .as_ref()
            .map(|g| (g.staged_count(), g.tracked_dirty_count()))
            .unwrap_or((0, 0));
        let commit_all = staged == 0 && tracked_dirty > 0;
        self.git_commit_impl(false, commit_all, window, cx);
    }

    pub(crate) fn git_commit_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.git_commit_impl(false, true, window, cx);
    }

    pub(crate) fn git_commit_amend(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.git_commit_impl(true, false, window, cx);
    }

    fn git_commit_impl(
        &mut self,
        amend: bool,
        all: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (root, conflicts, staged, tracked_dirty, change_count) = match self.git.as_ref() {
            Some(repo) => (
                repo.root.clone(),
                repo.conflict_count(),
                repo.staged_count(),
                repo.tracked_dirty_count(),
                repo.change_count(),
            ),
            None => {
                self.status = "Not a git repository — open a folder to commit".into();
                cx.notify();
                return;
            }
        };
        if conflicts > 0 {
            self.status = "Resolve merge conflicts before committing".into();
            cx.notify();
            return;
        }
        if !amend {
            let scope_count = if all {
                tracked_dirty.max(staged)
            } else {
                staged
            };
            if scope_count == 0 {
                self.status = if change_count > 0 {
                    "Nothing staged — use + on a file or 'stage all' first".into()
                } else {
                    "Nothing to commit".into()
                };
                cx.notify();
                return;
            }
        }
        let message = self
            .git_commit_input
            .as_ref()
            .map(|i| i.read(cx).value().trim().to_string())
            .unwrap_or_default();
        // An amend keeps the old message when the box is empty (`--no-edit`);
        // a normal commit needs one.
        if message.is_empty() && !amend {
            self.status = "Commit message is empty".into();
            cx.notify();
            return;
        }

        self.status = if amend {
            "Amending last commit…".into()
        } else {
            "Committing…".into()
        };
        cx.notify();
        let commit_input = self.git_commit_input.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { git::commit(&root, &message, amend, all) })
                .await;
            let is_ok = result.is_ok();
            let status = match result {
                Ok(summary) if amend => format!("Amended: {summary}"),
                Ok(summary) => format!("Committed: {summary}"),
                Err(e) => format!("Commit failed: {e}"),
            };
            let _ = this.update(cx, |workspace, cx| {
                workspace.status = status;
                if is_ok {
                    workspace.git_poke();
                }
                cx.notify();
            });

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

    // -- Remote operations (fetch / pull / push), stash, branches ------------

    /// Run one long git operation on a background thread with progress in
    /// the status bar. `git_op_running` doubles as a lock: a second remote
    /// operation is refused instead of silently interleaving with the first.
    fn git_remote_op(
        &mut self,
        verb: &'static str,
        op: impl FnOnce(PathBuf) -> Result<String, String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.git.as_ref().map(|g| g.root.clone()) else {
            self.status = "Not a git repository".into();
            cx.notify();
            return;
        };
        if let Some(running) = self.git_op_running {
            self.status = format!("Git is busy ({running} in progress)");
            cx.notify();
            return;
        }
        self.git_op_running = Some(verb);
        self.status = format!("{verb}…");
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { op(root) }).await;
            let _ = this.update(cx, |workspace, cx| {
                workspace.git_op_running = None;
                match result {
                    Ok(msg) => {
                        let first_line = msg.lines().next().unwrap_or("").trim();
                        workspace.status = if first_line.is_empty() {
                            format!("{verb} done")
                        } else {
                            format!("{verb}: {first_line}")
                        };
                        workspace.git_poke();
                    }
                    Err(e) => {
                        let first_line = e.lines().next().unwrap_or("git error").trim();
                        workspace.status = format!("{verb} failed: {first_line}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn git_fetch(&mut self, cx: &mut Context<Self>) {
        self.git_remote_op("Fetch", |root| git::fetch(&root), cx);
    }

    pub(crate) fn git_pull(&mut self, cx: &mut Context<Self>) {
        self.git_remote_op("Pull", |root| git::pull(&root), cx);
    }

    pub(crate) fn git_push(&mut self, force: bool, cx: &mut Context<Self>) {
        let branch = self.git.as_ref().and_then(|g| g.branch.clone());
        let has_upstream = self
            .git
            .as_ref()
            .map(|g| g.upstream.is_some())
            .unwrap_or(false);
        let verb = if force { "Force push" } else { "Push" };
        self.git_remote_op(
            verb,
            move |root| git::push(&root, branch.as_deref(), has_upstream, force),
            cx,
        );
    }

    pub(crate) fn git_stash_push(&mut self, cx: &mut Context<Self>) {
        self.git_remote_op("Stash", |root| git::stash_push(&root), cx);
    }

    pub(crate) fn git_stash_pop(&mut self, cx: &mut Context<Self>) {
        self.git_remote_op("Pop stash", |root| git::stash_pop(&root), cx);
    }

    pub(crate) fn git_checkout_branch(&mut self, name: String, cx: &mut Context<Self>) {
        self.git_remote_op("Checkout", move |root| git::checkout(&root, &name), cx);
    }

    pub(crate) fn git_create_branch(&mut self, name: String, cx: &mut Context<Self>) {
        // Branch names cannot contain spaces; normalize the picker query the
        // way `git switch -c` users usually expect.
        let name = name.trim().replace(' ', "-");
        if name.is_empty() {
            return;
        }
        self.git_remote_op(
            "Create branch",
            move |root| git::create_branch(&root, &name),
            cx,
        );
    }

    pub(crate) fn git_delete_branch(&mut self, name: String, cx: &mut Context<Self>) {
        self.git_remote_op(
            "Delete branch",
            move |root| git::delete_branch(&root, &name),
            cx,
        );
    }

    /// `git init` for the "no repository" empty state, then start watching.
    pub(crate) fn git_init(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            self.status = "Open a folder before initializing a repository".into();
            cx.notify();
            return;
        };
        if self.git.is_some() {
            self.status = "Already a git repository".into();
            cx.notify();
            return;
        }
        self.status = "Initializing repository…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let init_root = root.clone();
            let result = cx
                .background_spawn(async move { git::init(&init_root) })
                .await;
            let _ = this.update(cx, |workspace, cx| {
                match result {
                    Ok(_) => {
                        workspace.status = "Initialized empty git repository".into();
                        workspace.start_git_watcher(&root, cx);
                    }
                    Err(e) => {
                        workspace.status = format!("git init failed: {e}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn open_diff(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some((root, change)) = self.git_change_for(path) else {
            self.status = "Not a changed file".into();
            cx.notify();
            return;
        };
        // Staged-only entries diff the index; anything with worktree edits
        // (or an untracked file) diffs the working tree — previously a file
        // that was both staged *and* re-edited showed only its staged half.
        let staged = change.is_staged() && change.worktree.is_none() && !change.is_untracked();

        if let Some(idx) = self.tabs.iter().position(|t| {
            t.diff
                .as_ref()
                .map(|d| d.path == path && d.staged == staged)
                == Some(true)
        }) {
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
            parsed: None,
            error: None,
            commit: None,
        };
        self.tabs.push(OpenTab {
            path: None,
            editor: None,
            dirty: false,
            untitled: false,
            preview: false,
            is_settings: false,
            diff: Some(diff_tab),
            language_override: None,
        });
        self.active_tab = self.tabs.len() - 1;
        self.status = if staged {
            format!("Diff (staged): {}", display_name(path))
        } else {
            format!("Diff: {}", display_name(path))
        };

        self.load_diff_tab(root, rel, path.to_path_buf(), staged, None, cx);
    }

    /// (Re)load one diff tab's contents in the background. The tab is found
    /// again by `(path, staged)` when the result lands — matching on the
    /// path alone used to update the *wrong* tab whenever both the staged
    /// and unstaged diff of the same file were open.
    fn load_diff_tab(
        &mut self,
        root: PathBuf,
        rel: String,
        tab_path: PathBuf,
        staged: bool,
        commit: Option<String>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let tab_path_bg = tab_path.clone();
            let rel_bg = rel.clone();
            let commit_bg = commit.clone();
            let (text, parsed, error) = cx
                .background_spawn(async move {
                    // A commit diff (`git show <sha>`) never falls back to the
                    // working tree — its content is fixed history.
                    if let Some(sha) = commit_bg {
                        return match git::commit_diff(&root, &sha) {
                            Some(raw) if !raw.trim().is_empty() => {
                                let parsed = Arc::new(crate::ui::diff::parse_diff(&raw));
                                (Some(raw), Some(parsed), None)
                            }
                            Some(_) => (
                                Some(String::new()),
                                None,
                                Some("This commit has no textual changes".to_string()),
                            ),
                            None => (
                                None,
                                None,
                                Some("git show failed to run".to_string()),
                            ),
                        };
                    }
                    let raw = git::diff(&root, &rel_bg, staged);
                    match raw {
                        None => (None, None, Some("git diff failed to run".to_string())),
                        Some(raw) if !raw.trim().is_empty() => {
                            let parsed = Arc::new(crate::ui::diff::parse_diff(&raw));
                            (Some(raw), Some(parsed), None)
                        }
                        Some(_) => {
                            // No diff output: brand-new/untracked file (or a
                            // binary file, which produces no text hunks).
                            match std::fs::read_to_string(&tab_path_bg) {
                                Ok(content) => {
                                    let text = git::new_file_diff(&rel_bg, &content);
                                    let parsed = Arc::new(crate::ui::diff::parse_diff(&text));
                                    (Some(text), Some(parsed), None)
                                }
                                Err(_) if !tab_path_bg.exists() => (
                                    Some(String::new()),
                                    None,
                                    Some("File was deleted — nothing left to diff".to_string()),
                                ),
                                Err(_) => (
                                    Some(String::new()),
                                    None,
                                    Some("No text changes (binary or unreadable file)".to_string()),
                                ),
                            }
                        }
                    }
                })
                .await;
            let _ = this.update(cx, |workspace, cx| {
                if let Some(tab) = workspace.tabs.iter_mut().find(|t| {
                    t.diff
                        .as_ref()
                        .map(|d| d.path == tab_path && d.staged == staged)
                        == Some(true)
                }) {
                    if let Some(diff) = &mut tab.diff {
                        diff.text = text;
                        diff.parsed = parsed;
                        diff.error = error;
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Refresh every open diff tab (not just the active one) so a stage /
    /// unstage / external edit updates all of them at once.
    fn refresh_diff_tabs(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.git.as_ref().map(|g| g.root.clone()) else {
            return;
        };
        let jobs: Vec<(String, PathBuf, bool, Option<String>)> = self
            .tabs
            .iter()
            .filter_map(|t| t.diff.as_ref())
            .map(|d| (d.rel.clone(), d.path.clone(), d.staged, d.commit.clone()))
            .collect();
        for (rel, path, staged, commit) in jobs {
            self.load_diff_tab(root.clone(), rel, path, staged, commit, cx);
        }
    }

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
            let Some(lang_id) = tab.language().map(|s| s.to_string()) else {
                self.status = format!("{} has no language mode", display_name(&path));
                cx.notify();
                return;
            };
            (path, editor, lang_id)
        };
        let Some(client) = self.lsp.lock().unwrap().client_for(&lang_id) else {
            self.status = format!("No language server running for {lang_id}");
            cx.notify();
            return;
        };
        let text = editor.read(cx).value().to_string();
        // Honor `editor.tabSize` (see LspClient::format_document).
        let tab_size = self.settings.editor_tab_size as u32;

        self.status = "Formatting…".into();
        cx.notify();
        let editor_weak = editor.downgrade();
        let display = display_name(&path);
        cx.spawn_in(window, async move |this, cx| {
            let edits = cx
                .background_spawn(async move { client.format_document(&path, &text, tab_size) })
                .await;
            match edits {
                Some(edits) if !edits.is_empty() => {
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
                        workspace.status = "Formatting not supported by the language server".into();
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
                language_override: None,
            });
            self.active_tab = self.tabs.len() - 1;
        }
        self.status = "Settings".into();
        cx.notify();
    }

    pub(crate) fn open_settings_json(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = crate::settings::settings_file_path();
        if !path.exists() {
            let _ = self.settings.save();
        }
        self.open_file(path, window, cx);
    }

    pub(crate) fn reload_settings(&mut self, cx: &mut Context<Self>) {
        self.settings = crate::settings::Settings::load();
        let themes = theme::all();
        if let Some(pos) = themes
            .iter()
            .position(|t| t.name == self.settings.workbench_color_theme)
        {
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

    pub(crate) fn save_tab_quiet(&mut self, idx: usize, cx: &mut Context<Self>) -> bool {
        let Some(tab) = self.tabs.get_mut(idx) else {
            return false;
        };
        if tab.is_settings || !tab.dirty || tab.path.is_none() {
            return false;
        }
        // Auto-save deliberately skips format-on-save: formatting mid-typing
        // (the `afterDelay` mode fires every second) would fight the user
        // for the buffer. `save_tab_quiet` is also synchronous by design —
        // unifying it with the async format-then-save path is described in
        // docs/architecture-themes-syntax-formatting.md.
        let path = tab.path.clone().unwrap();
        let Some(editor) = &tab.editor else {
            return false;
        };
        let text = editor.read(cx).value().to_string();
        self.write_file_async(path, text, "Auto-saved", cx);
        true
    }

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

    pub(crate) fn trigger_auto_save_after_delay(
        &mut self,
        _tab_idx: usize,
        cx: &mut Context<Self>,
    ) {
        if self.settings.editor_auto_save != crate::settings::AutoSaveMode::AfterDelay {
            return;
        }
        self.auto_save_generation = self.auto_save_generation.wrapping_add(1);
        let current_gen = self.auto_save_generation;
        let delay = std::time::Duration::from_millis(self.settings.editor_auto_save_delay);

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;

            let _ = this.update(cx, |workspace, cx| {
                if workspace.auto_save_generation == current_gen {
                    workspace.save_all_dirty_quiet(cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn trigger_auto_save_on_focus_change(&mut self, cx: &mut Context<Self>) {
        if self.settings.editor_auto_save != crate::settings::AutoSaveMode::OnFocusChange {
            return;
        }
        self.save_all_dirty_quiet(cx);
    }

    #[allow(dead_code)]
    pub(crate) fn switch_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active_tab = index;
        }
    }

    pub(crate) fn close_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(closed_path) = self.tabs.get(index).and_then(|tab| tab.path.as_ref()) {
            if self
                .markdown_preview
                .as_ref()
                .is_some_and(|preview| &preview.path == closed_path)
            {
                self.markdown_preview = None;
            }
        }
        if let Some(closed_tab) = self.tabs.get(index) {
            if let Some(p) = &closed_tab.path {
                if let Some(lang_id) = closed_tab.language() {
                    self.lsp.lock().unwrap().close_document(p, lang_id);
                }
            }
        }

        if self.tabs.len() == 1 {
            self.tabs.remove(0);
            self.active_tab = 0;

            self.persist_workspace_state(cx);
            window.focus(&self.focus_handle);
            cx.notify();
            return;
        }

        self.tabs.remove(index);

        if index <= self.active_tab && self.active_tab > 0 {
            self.active_tab -= 1;
        }
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len().saturating_sub(1);
        }

        self.persist_workspace_state(cx);
        self.focus_active_editor_or_self(window, cx);
        cx.notify();
    }

    pub(crate) fn switch_tab_to(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            self.trigger_auto_save_on_focus_change(cx);
            self.active_tab = index;

            if let Some(tab) = self.tabs.get_mut(index) {
                tab.preview = false;
            }
            // VS Code's `explorer.autoReveal`: the tree follows the active
            // editor, expanding and scrolling to the file it belongs to.
            if self.activity == Activity::Explorer {
                if let Some(path) = self.tabs.get(index).and_then(|tab| tab.path.clone()) {
                    self.reveal_tree_path(&path);
                }
            }
            self.persist_workspace_state(cx);
            cx.notify();
        }
    }

    pub(crate) fn close_tab_at_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            if let Some(closed_path) = self.tabs.get(index).and_then(|tab| tab.path.as_ref()) {
                if self
                    .markdown_preview
                    .as_ref()
                    .is_some_and(|preview| &preview.path == closed_path)
                {
                    self.markdown_preview = None;
                }
            }
            if let Some(closed_tab) = self.tabs.get(index) {
                if let Some(p) = &closed_tab.path {
                    if let Some(lang_id) = closed_tab.language() {
                        self.lsp.lock().unwrap().close_document(p, lang_id);
                    }
                }
            }

            if self.tabs.len() == 1 {
                self.tabs.remove(0);
                self.active_tab = 0;
            } else {
                self.tabs.remove(index);

                if index <= self.active_tab && self.active_tab > 0 {
                    self.active_tab -= 1;
                }
                if self.active_tab >= self.tabs.len() {
                    self.active_tab = self.tabs.len().saturating_sub(1);
                }
            }
            self.persist_workspace_state(cx);
            cx.notify();
        }
    }

    pub(crate) fn handle_close_tab(
        &mut self,
        _: &crate::actions::CloseTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_tab(self.active_tab, window, cx);
    }

    pub(crate) fn handle_next_tab(
        &mut self,
        _: &crate::actions::NextTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.len() > 1 {
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
            self.persist_workspace_state(cx);
            self.focus_active_editor_or_self(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn handle_prev_tab(
        &mut self,
        _: &crate::actions::PrevTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs.len() > 1 {
            self.active_tab = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
            self.persist_workspace_state(cx);
            self.focus_active_editor_or_self(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn handle_switch_tab(
        &mut self,
        action: &crate::actions::SwitchTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if action.index < self.tabs.len() {
            self.active_tab = action.index;
            self.persist_workspace_state(cx);
            self.focus_active_editor_or_self(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn handle_close_tab_at(
        &mut self,
        action: &crate::actions::CloseTabAt,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        self.save_all_dirty_quiet(cx);
        self.persist_workspace_state(cx);
        cx.quit();
    }

    pub(crate) fn about(&mut self, cx: &mut Context<Self>) {
        self.status = format!("ezicode — {} (Zed theme system)", self.theme().name);
        cx.notify();
    }

    pub(crate) fn toggle_file_finder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = &self.picker {
            if p.kind == crate::ui::picker::PickerKind::FileFinder {
                self.close_modal(window, cx);
                return;
            }
        }

        let root_dir = self.root.as_deref().unwrap_or(Path::new("."));
        let global_state = crate::storage::GlobalState::load();
        let mut recent_files: Vec<PathBuf> =
            self.tabs.iter().filter_map(|t| t.path.clone()).collect();
        for p in global_state.recent_files {
            if !recent_files.contains(&p) {
                if let Some(root) = &self.root {
                    if p.starts_with(root) {
                        recent_files.push(p);
                    }
                } else {
                    recent_files.push(p);
                }
            }
        }

        let items = if let Some((cached_root, cached_items)) = &self.workspace_files_cache {
            if cached_root == root_dir {
                // Instantly reuse cached workspace files with fresh recent files prioritization
                let recent_set: std::collections::HashSet<_> = recent_files.iter().collect();
                let mut ordered = Vec::with_capacity(cached_items.len());
                for item in cached_items {
                    let p = PathBuf::from(&item.id);
                    if recent_set.contains(&p) {
                        let mut it = item.clone();
                        it.is_recent = true;
                        ordered.push(it);
                    }
                }
                for item in cached_items {
                    let p = PathBuf::from(&item.id);
                    if !recent_set.contains(&p) {
                        ordered.push(item.clone());
                    }
                }
                ordered
            } else {
                let scanned = crate::ui::picker::scan_workspace_files(root_dir, &recent_files);
                self.workspace_files_cache = Some((root_dir.to_path_buf(), scanned.clone()));
                scanned
            }
        } else {
            let scanned = crate::ui::picker::scan_workspace_files(root_dir, &recent_files);
            self.workspace_files_cache = Some((root_dir.to_path_buf(), scanned.clone()));
            scanned
        };

        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search files by name (append : to go to line or @ to go to symbol)")
        });

        cx.subscribe(&input, |this, _state, event: &InputEvent, cx| match event {
            InputEvent::Change => {
                this.on_picker_input_changed(cx);
            }
            InputEvent::PressEnter { .. } => {
                this.picker_confirm_pending = true;
                cx.notify();
            }
            _ => {}
        })
        .detach();

        input.update(cx, |this, cx| {
            this.focus(window, cx);
        });

        self.picker = Some(crate::ui::picker::PickerState::new(
            crate::ui::picker::PickerKind::FileFinder,
            input,
            items,
        ));
        cx.notify();
    }

    pub(crate) fn toggle_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = &self.picker {
            if p.kind == crate::ui::picker::PickerKind::CommandPalette {
                self.close_modal(window, cx);
                return;
            }
        }

        let items = crate::ui::picker::command_palette_items();
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Type a command or action..."));

        cx.subscribe(&input, |this, _state, event: &InputEvent, cx| match event {
            InputEvent::Change => {
                this.on_picker_input_changed(cx);
            }
            InputEvent::PressEnter { .. } => {
                this.picker_confirm_pending = true;
                cx.notify();
            }
            _ => {}
        })
        .detach();

        input.update(cx, |this, cx| {
            this.focus(window, cx);
        });

        self.picker = Some(crate::ui::picker::PickerState::new(
            crate::ui::picker::PickerKind::CommandPalette,
            input,
            items,
        ));
        cx.notify();
    }

    pub(crate) fn toggle_goto_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = &self.picker {
            if p.kind == crate::ui::picker::PickerKind::GoToLine {
                self.close_modal(window, cx);
                return;
            }
        }

        let total_lines = self
            .active_editor()
            .map(|ed| {
                let rope = ed.read(cx).text();
                rope.offset_to_position(rope.len()).line + 1
            })
            .unwrap_or(1);

        let placeholder = format!("Go to line:column (1 - {})...", total_lines);
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));

        cx.subscribe(&input, |this, _state, event: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.picker_confirm_pending = true;
                cx.notify();
            }
        })
        .detach();

        input.update(cx, |this, cx| {
            this.focus(window, cx);
        });

        self.picker = Some(crate::ui::picker::PickerState::new(
            crate::ui::picker::PickerKind::GoToLine,
            input,
            Vec::new(),
        ));
        cx.notify();
    }

    pub(crate) fn active_tab_language(&self) -> Option<&str> {
        self.tabs.get(self.active_tab).and_then(|t| t.language())
    }

    pub(crate) fn toggle_language_selector(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = &self.picker {
            if p.kind == crate::ui::picker::PickerKind::LanguageSelector {
                self.close_modal(window, cx);
                return;
            }
        }

        let current_lang = self.active_tab_language();
        let items = crate::ui::picker::language_selector_items(current_lang);
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Select Language Mode (e.g. JavaScript, Python, Rust, Go...)")
        });

        cx.subscribe(&input, |this, _state, event: &InputEvent, cx| match event {
            InputEvent::Change => {
                this.on_picker_input_changed(cx);
            }
            InputEvent::PressEnter { .. } => {
                this.picker_confirm_pending = true;
                cx.notify();
            }
            _ => {}
        })
        .detach();

        input.update(cx, |this, cx| {
            this.focus(window, cx);
        });

        self.picker = Some(crate::ui::picker::PickerState::new(
            crate::ui::picker::PickerKind::LanguageSelector,
            input,
            items,
        ));
        cx.notify();
    }

    /// Zed-style branch switcher: local branches sorted by recency, checkout
    /// on Enter, and "type a new name + Enter with no match" creates it.
    pub(crate) fn toggle_branch_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = &self.picker {
            if p.kind == crate::ui::picker::PickerKind::GitBranch {
                self.close_modal(window, cx);
                return;
            }
        }
        let Some(root) = self.git.as_ref().map(|g| g.root.clone()) else {
            self.status = "Not a git repository".into();
            cx.notify();
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let branches = cx
                .background_spawn(async move { git::branches(&root) })
                .await;
            let _ = this.update_in(cx, |workspace, window, cx| {
                workspace.open_branch_picker(branches, false, window, cx);
            });
        })
        .detach();
    }

    /// Same list, but Enter deletes the selected branch (`git branch -d`).
    pub(crate) fn toggle_branch_delete_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(p) = &self.picker {
            if p.kind == crate::ui::picker::PickerKind::GitBranchDelete {
                self.close_modal(window, cx);
                return;
            }
        }
        let Some(root) = self.git.as_ref().map(|g| g.root.clone()) else {
            self.status = "Not a git repository".into();
            cx.notify();
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let branches = cx
                .background_spawn(async move { git::branches(&root) })
                .await;
            let _ = this.update_in(cx, |workspace, window, cx| {
                workspace.open_branch_picker(branches, true, window, cx);
            });
        })
        .detach();
    }

    fn open_branch_picker(
        &mut self,
        branches: Vec<git::Branch>,
        delete_mode: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items: Vec<crate::ui::picker::PickerItem> = branches
            .iter()
            // The current branch can be neither checked out again nor
            // deleted, but seeing it in checkout mode is useful context.
            .filter(|b| !(delete_mode && b.is_current))
            .map(|b| crate::ui::picker::PickerItem {
                id: b.name.clone(),
                title: b.name.clone(),
                subtitle: Some(if b.is_current {
                    format!("current • {}", b.last_commit)
                } else if let Some(upstream) = &b.upstream {
                    format!("{} • {}", upstream, b.last_commit)
                } else {
                    b.last_commit.clone()
                }),
                icon: Some("ui_icons/git_branch.svg".into()),
                shortcut: None,
                is_recent: b.is_current,
                score: 0,
            })
            .collect();

        let placeholder = if delete_mode {
            "Select a branch to delete…"
        } else {
            "Checkout branch, or type a new name and press Enter to create it…"
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));

        cx.subscribe(&input, |this, _state, event: &InputEvent, cx| match event {
            InputEvent::Change => {
                this.on_picker_input_changed(cx);
            }
            InputEvent::PressEnter { .. } => {
                this.picker_confirm_pending = true;
                cx.notify();
            }
            _ => {}
        })
        .detach();

        input.update(cx, |this, cx| {
            this.focus(window, cx);
        });

        let kind = if delete_mode {
            crate::ui::picker::PickerKind::GitBranchDelete
        } else {
            crate::ui::picker::PickerKind::GitBranch
        };
        self.picker = Some(crate::ui::picker::PickerState::new(kind, input, items));
        cx.notify();
    }

    pub(crate) fn set_active_tab_language(
        &mut self,
        lang_id: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab_idx = self.active_tab;
        let Some(tab) = self.tabs.get_mut(tab_idx) else {
            return;
        };

        let old_lang = tab.language().map(|s| s.to_string());
        tab.language_override = Some(lang_id.to_string());

        if let Some(editor) = tab.editor.clone() {
            // 1. Update syntax highlighting for the current buffer
            let lang_owned = lang_id.to_string();
            editor.update(cx, move |state, cx| {
                state.set_highlighter(lang_owned, cx);
            });

            // 2. Switch LSP language server
            if let Some(path) = tab.path.clone() {
                if let Some(old) = &old_lang {
                    if old != lang_id {
                        let mut lsp = self.lsp.lock().unwrap();
                        lsp.close_document(&path, old);
                    }
                }
                self.attach_language_server(&path, lang_id, &editor, cx);
            }
        }

        let lang_display = crate::lang::language_name(lang_id);
        if let Some(server) = crate::lang::lsp_server_for(lang_id) {
            self.status = format!("Language mode changed to {lang_display} (LSP: {server})");
        } else {
            self.status = format!("Language mode changed to {lang_display}");
        }
        cx.notify();
    }

    pub(crate) fn close_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A pending git confirmation is the topmost modal; Escape cancels it
        // before it would close a picker underneath.
        if self.git_confirm.take().is_some() {
            self.focus_active_editor_or_self(window, cx);
            cx.notify();
            return;
        }
        if self.picker.take().is_some() {
            self.focus_active_editor_or_self(window, cx);
            cx.notify();
        } else {
            // Nothing of ours was open, so let Escape keep travelling. GPUI's
            // action bubble phase stops propagation by default (see
            // `App::propagate`), and this handler sits on the focused
            // terminal's dispatch path — so without this, `on_key_down` never
            // runs, no 0x1b reaches the PTY, and vim/htop/fzf modals can't be
            // dismissed. Deliberately not propagated when a picker *was*
            // closed, so that path keeps its existing behaviour.
            cx.propagate();
        }
    }

    pub(crate) fn picker_next(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.picker {
            p.select_next();
            cx.notify();
        }
    }

    pub(crate) fn picker_prev(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.picker {
            p.select_prev();
            cx.notify();
        }
    }

    pub(crate) fn on_picker_input_changed(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.picker {
            let query = p.input.read(cx).value().to_string();
            p.filter(&query);
            cx.notify();
        }
    }

    pub(crate) fn confirm_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.picker.take() else {
            return;
        };

        match picker.kind {
            crate::ui::picker::PickerKind::FileFinder => {
                let query = picker.input.read(cx).value().to_string();
                if let Some(item) = picker.selected_item() {
                    let path = PathBuf::from(&item.id);
                    self.open_file(path, window, cx);

                    if let Some((_, line_part)) = query.split_once(':') {
                        self.execute_goto_line(line_part, window, cx);
                    }
                }
            }
            crate::ui::picker::PickerKind::CommandPalette => {
                if let Some(item) = picker.selected_item() {
                    let cmd_id = item.id.clone();
                    self.execute_palette_command(&cmd_id, window, cx);
                }
            }
            crate::ui::picker::PickerKind::GoToLine => {
                let val = picker.input.read(cx).value().to_string();
                self.execute_goto_line(&val, window, cx);
            }
            crate::ui::picker::PickerKind::LanguageSelector => {
                if let Some(item) = picker.selected_item() {
                    let lang_id = item.id.clone();
                    self.set_active_tab_language(&lang_id, window, cx);
                }
            }
            crate::ui::picker::PickerKind::GitBranch => {
                let query = picker.input.read(cx).value().trim().to_string();
                match picker.selected_item() {
                    Some(item) => {
                        let name = item.id.clone();
                        let is_current = self
                            .git
                            .as_ref()
                            .and_then(|g| g.branch.as_deref())
                            .map(|b| b == name)
                            .unwrap_or(false);
                        if is_current {
                            self.status = format!("Already on {name}");
                            cx.notify();
                        } else {
                            self.git_checkout_branch(name, cx);
                        }
                    }
                    None if !query.is_empty() => self.git_create_branch(query, cx),
                    None => {}
                }
            }
            crate::ui::picker::PickerKind::GitBranchDelete => {
                if let Some(item) = picker.selected_item() {
                    let name = item.id.clone();
                    self.git_delete_branch(name, cx);
                }
            }
        }
        self.focus_active_editor_or_self(window, cx);
        cx.notify();
    }

    pub(crate) fn select_picker_item(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(p) = &mut self.picker {
            p.selected_index = index;
        }
        self.confirm_picker(window, cx);
    }

    pub(crate) fn execute_palette_command(
        &mut self,
        cmd_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match cmd_id {
            "language.change_mode" => self.toggle_language_selector(window, cx),
            "file.new" => self.new_file(window, cx),
            "file.open" => self.open_file_dialog(window, cx),
            "file.open_folder" => self.open_folder_dialog(window, cx),
            "file.save" => self.save(window, cx),
            "file.quick_open" => self.toggle_file_finder(window, cx),
            "view.goto_line" => self.toggle_goto_line(window, cx),
            "tab.close" => {
                self.close_tab(self.active_tab, window, cx);
            }
            "tab.next" => self.handle_next_tab(&crate::actions::NextTab, window, cx),
            "tab.prev" => self.handle_prev_tab(&crate::actions::PrevTab, window, cx),
            "terminal.toggle" => self.toggle_terminal(window, cx),
            "terminal.new" => self.new_terminal(window, cx),
            "terminal.toggle_right" => self.toggle_terminal_right(window, cx),
            "terminal.close" => self.close_active_terminal(window, cx),
            "terminal.clear" => self.clear_active_terminal(cx),
            "sidebar.toggle" => {
                self.show_sidebar = !self.show_sidebar;
                cx.notify();
            }
            "view.explorer" => self.set_activity_explicit(Activity::Explorer, window, cx),
            "view.search" => self.set_activity_explicit(Activity::Search, window, cx),
            "view.git" => self.set_activity_explicit(Activity::Git, window, cx),
            "view.extensions" => self.set_activity_explicit(Activity::Extensions, window, cx),
            "preferences.settings" => self.open_settings(cx),
            "theme.github_dark" => self.apply_theme_by_name("GitHub Dark", window, cx),
            "theme.github_light" => self.apply_theme_by_name("GitHub Light", window, cx),
            "theme.github_dark_dimmed" => {
                self.apply_theme_by_name("GitHub Dark Dimmed", window, cx)
            }
            "theme.github_dark_high_contrast" => {
                self.apply_theme_by_name("GitHub Dark High Contrast", window, cx)
            }
            "theme.github_light_high_contrast" => {
                self.apply_theme_by_name("GitHub Light High Contrast", window, cx)
            }
            "editor.format" => self.format_document(window, cx),
            "editor.font_increase" => self.increase_font_size(cx),
            "editor.font_decrease" => self.decrease_font_size(cx),
            "editor.font_reset" => self.reset_font_size(cx),
            "editor.copy_diagnostic" => self.copy_active_diagnostic(cx),
            "git.refresh" => self.git_refresh(cx),
            "git.stage_all" => self.git_stage_all(cx),
            "git.unstage_all" => self.git_unstage_all(cx),
            "git.discard_all" => self.git_request_discard_all(cx),
            "git.commit" => self.git_commit(window, cx),
            "git.commit_all" => self.git_commit_all(window, cx),
            "git.commit_amend" => self.git_commit_amend(window, cx),
            "git.fetch" => self.git_fetch(cx),
            "git.pull" => self.git_pull(cx),
            "git.push" => self.git_push(false, cx),
            "git.push_force" => self.git_push(true, cx),
            "git.stash" => self.git_stash_push(cx),
            "git.stash_pop" => self.git_stash_pop(cx),
            "git.branch.checkout" => self.toggle_branch_picker(window, cx),
            "git.branch.delete" => self.toggle_branch_delete_picker(window, cx),
            "git.init" => self.git_init(cx),
            "git.toggle_blame" => self.toggle_git_blame(cx),
            "git.history" => {
                self.set_activity_explicit(Activity::Git, window, cx);
                self.git_show_history(cx);
            }
            "help.about" => self.about(cx),
            "app.quit" => self.quit(cx),
            _ => {}
        }
    }

    pub(crate) fn apply_theme_by_name(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let themes = theme::all();
        if let Some(pos) = themes.iter().position(|t| t.name == name) {
            self.apply_theme(pos, window, cx);
        }
    }

    pub(crate) fn execute_goto_line(
        &mut self,
        val: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let trimmed = val.trim();
        if trimmed.is_empty() {
            return;
        }

        let mut parts = trimmed.split(':');
        let line: u32 = match parts.next().and_then(|s| s.trim().parse().ok()) {
            Some(l) => l,
            None => return,
        };
        let character: u32 = parts
            .next()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(1);

        if let Some(editor) = self.active_editor() {
            let position = lsp_types::Position {
                line: line.saturating_sub(1),
                character: character.saturating_sub(1),
            };
            editor.update(cx, |this, cx| {
                this.set_cursor_position(position, window, cx);
            });
            self.status = format!("Jumped to line {line}:{character}");
            cx.notify();
        }
    }

    pub(crate) fn jump_to_line(
        &mut self,
        line: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.active_editor() {
            let position = lsp_types::Position {
                line: (line as u32).saturating_sub(1),
                character: 0,
            };
            editor.update(cx, |this, cx| {
                this.set_cursor_position(position, window, cx);
            });
            self.status = format!("Jumped to line {line}");
            cx.notify();
        }
    }

    pub(crate) fn restore_tab(
        &mut self,
        tab_info: crate::storage::OpenTabState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let crate::storage::OpenTabState {
            path,
            preview,
            language_override,
            cursor,
        } = tab_info;
        let loaded = match load_buffer_file(&path) {
            Ok(loaded) => loaded,
            Err(_) => return,
        };

        let lang_id: String = if let Some(override_lang) = &language_override {
            override_lang.clone()
        } else {
            loaded.lang_id.to_string()
        };
        let text = loaded.text;

        let editor = cx.new(|cx| {
            let mut state = InputState::new(window, cx)
                .code_editor(lang_id.clone())
                .line_number(true)
                .indent_guides(false)
                .soft_wrap(false)
                .searchable(true)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                });
            state.set_value(text, window, cx);
            if let Some(pos) = cursor {
                state.set_cursor_position(
                    lsp_types::Position {
                        line: pos.line,
                        character: pos.character,
                    },
                    window,
                    cx,
                );
            }
            state
        });

        self.attach_language_server(&path, lang_id.as_str(), &editor, cx);

        let path_clone = path.clone();
        let lang_str = lang_id.clone();
        let editor_ent = editor.clone();

        cx.subscribe(&editor, move |this, _state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let mut ui_changed = false;
                if let Some(tab) = this.tabs.get_mut(this.active_tab) {
                    if !tab.dirty {
                        tab.dirty = true;
                        ui_changed = true;
                    }
                    if tab.preview {
                        tab.preview = false;
                        ui_changed = true;
                    }
                }
                {
                    let current_lang = this
                        .tabs
                        .iter()
                        .find(|t| t.path.as_ref() == Some(&path_clone))
                        .and_then(|t| t.language())
                        .unwrap_or(lang_str.as_str());
                    let mut lsp = this.lsp.lock().unwrap();
                    if lsp.has_client(current_lang) {
                        let text = editor_ent.read(cx).value().to_string();
                        lsp.change_document(&path_clone, current_lang, text);
                    }
                }
                if ui_changed {
                    cx.notify();
                }
                this.markdown_buffer_changed(&path_clone, &editor_ent, cx);
                this.on_editor_blame_change(&path_clone, &editor_ent, cx);
                let tab_idx = this.active_tab;
                this.trigger_auto_save_after_delay(tab_idx, cx);
            }
            match event {
                InputEvent::BlameHover { sha, .. } => {
                    this.on_blame_hover(sha.as_ref(), editor_ent.clone(), cx)
                }
                InputEvent::BlameHoverEnd => this.on_blame_hover_end(editor_ent.clone(), cx),
                InputEvent::BlameOpenCommit { sha } => {
                    this.git_view_commit_diff(sha.to_string(), cx)
                }
                _ => {}
            }
        })
        .detach();

        self.tabs.push(OpenTab {
            path: Some(path),
            editor: Some(editor),
            dirty: false,
            untitled: false,
            preview,
            is_settings: false,
            diff: None,
            language_override,
        });
    }

    pub(crate) fn collect_expanded_folders(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        fn collect(nodes: &[TreeNode], out: &mut Vec<PathBuf>) {
            for n in nodes {
                if n.is_dir && n.expanded {
                    out.push(n.path.clone());
                    collect(&n.children, out);
                }
            }
        }
        collect(&self.tree, &mut out);
        out
    }

    pub(crate) fn persist_workspace_state(&self, cx: &App) {
        let Some(root) = &self.root else {
            return;
        };

        let mut tab_states = Vec::new();
        for tab in &self.tabs {
            if let Some(path) = &tab.path {
                let cursor = tab.editor.as_ref().map(|ed| {
                    let pos = ed.read(cx).cursor_position();
                    crate::storage::CursorPosition {
                        line: pos.line,
                        character: pos.character,
                    }
                });

                tab_states.push(crate::storage::OpenTabState {
                    path: path.clone(),
                    preview: tab.preview,
                    language_override: tab.language_override.clone(),
                    cursor,
                });
            }
        }

        let activity_str = match self.activity {
            Activity::Explorer => "Explorer",
            Activity::Search => "Search",
            Activity::Git => "Git",
            Activity::Extensions => "Extensions",
        }
        .to_string();

        let state = crate::storage::WorkspaceState {
            root: root.clone(),
            tabs: tab_states,
            active_tab: self.active_tab,
            layout: crate::storage::LayoutState {
                sidebar_width: self.sidebar_width,
                terminal_height: self.terminal_height,
                terminal_right_width: self.terminal_right_width,
                show_sidebar: self.show_sidebar,
                show_terminal: self.show_terminal,
                show_terminal_right: self.show_terminal_right,
                terminal_maximized: self.terminal_maximized,
                activity: activity_str,
                explorer_sticky_scroll: self.explorer_sticky_scroll,
            },
            expanded_folders: self.collect_expanded_folders(),
            explorer_selected: self.selected_path.clone(),
        };

        let _ = state.save();
    }
}
