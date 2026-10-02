mod render;
mod search;
mod session;
mod terminal_tabs;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, ScrollHandle, ScrollStrategy, SharedString,
    Task, UniformListScrollHandle, Window,
};
use gpui_component::input::{InputEvent, InputState, RopeExt as _, TabSize};

use crate::cancellation::Cancellation;
use crate::fs_tree::{
    collapse_all, display_name, entries_match, flatten_visible, is_same_or_descendant,
    merge_loaded_dir, path_after_move, try_load_dir, valid_entry_name, TreeNode, VisibleTreeRow,
};
use crate::git::{self, ChangeKind, GitChange, RepoStatus};
use crate::lang;
use crate::lsp::{LspEvent, LspManager};
use crate::theme;
use session::{DirectoryWatcher, PreparedWorkspace};

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

struct FileLoad {
    token: Cancellation,
    pinned: bool,
    open_generation: Option<u64>,
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
    pub(crate) storage: crate::storage::StateStore,
    /// The active session and the pending transition have independent lifetimes.
    pub(crate) session: Cancellation,
    transition: Cancellation,
    transition_task: Option<Task<()>>,
    pub(crate) loading_root: Option<PathBuf>,
    restore_startup_workspace: bool,
    project_tasks: Vec<(Task<()>, Cancellation)>,
    directory_loads: HashMap<PathBuf, bool>,
    file_loads: HashMap<PathBuf, FileLoad>,
    pending_user_open: Option<PathBuf>,
    file_open_generation: u64,
    pending_reveal: Option<PathBuf>,
    fs_watcher: Option<DirectoryWatcher>,
    fs_task: Option<Task<()>>,
    /// Cached `display_name(root)` so the title bar and explorer header don't
    pub(crate) root_display: String,

    pub(crate) root_display_shared: SharedString,

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

    pub(crate) settings: crate::settings::Settings,

    pub(crate) auto_save_generation: usize,
    auto_save_task: Option<Task<()>>,

    pub(crate) picker: Option<crate::ui::picker::PickerState>,

    pub(crate) picker_confirm_pending: bool,

    pub(crate) workspace_files_cache: Option<(PathBuf, Arc<Vec<crate::ui::picker::PickerItem>>)>,
    file_index_task: Option<Task<()>>,
    file_index_cancel: Cancellation,
    picker_filter_task: Option<Task<()>>,
    picker_filter_cancel: Cancellation,

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
    pub(crate) search_cancel: Cancellation,
    pub(crate) search_task: Option<Task<()>>,
    pub(crate) search_collapsed: HashSet<PathBuf>,
    pub(crate) search_replace_open: bool,
    /// Jump applied once an async `open_file` finishes (search result click
    /// on a file that is not open yet).
    pub(crate) pending_search_jump: Option<(PathBuf, usize, usize)>,
    pub(crate) pending_restore_tabs: Vec<crate::storage::OpenTabState>,
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
fn load_buffer_file(path: &Path, cancellation: &Cancellation) -> Result<LoadedBuffer, String> {
    use std::io::Read as _;
    let _span = crate::perf::span("workspace.read_buffer.background");
    const LIMIT: usize = 8_000_000;
    let metadata = std::fs::metadata(path).map_err(|error| format!("open failed: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", display_name(path)));
    }
    if metadata.len() > LIMIT as u64 {
        return Err(format!("{} is too large (>8MB)", display_name(path)));
    }
    let mut file = std::fs::File::open(path).map_err(|error| format!("open failed: {error}"))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let mut chunk = [0; 64 * 1024];
    loop {
        if cancellation.is_cancelled() {
            return Err("File opening cancelled".into());
        }
        let remaining = (LIMIT + 1 - bytes.len()).min(chunk.len());
        let count = file
            .read(&mut chunk[..remaining])
            .map_err(|error| format!("open failed: {error}"))?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > LIMIT {
            return Err(format!("{} is too large (>8MB)", display_name(path)));
        }
    }
    if bytes.iter().take(8000).any(|&byte| byte == 0) {
        return Err(format!("{} looks binary", display_name(path)));
    }
    let newline_count = bytes.iter().filter(|&&byte| byte == b'\n').count();
    let highlight = bytes.len() <= 400_000 && newline_count <= 8_000;
    let lang_id = if highlight {
        lang::language_for(path).unwrap_or("text")
    } else {
        "text"
    };
    Ok(LoadedBuffer {
        text: String::from_utf8_lossy(&bytes).into_owned(),
        lang_id,
        highlight,
    })
}

impl Workspace {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (storage, storage_ready) = crate::storage::StateStore::new();
        Self::new_with_storage(window, cx, storage, storage_ready)
    }

    fn new_with_storage(
        window: &mut Window,
        cx: &mut Context<Self>,
        storage: crate::storage::StateStore,
        storage_ready: async_channel::Receiver<crate::storage::GlobalState>,
    ) -> Self {
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
                while let Ok(message) = rx.recv().await {
                    let current = this
                        .update(cx, |workspace, _| {
                            message.generation.is_none_or(|generation| {
                                generation == workspace.lsp.lock().unwrap().generation()
                            })
                        })
                        .unwrap_or(false);
                    if !current {
                        continue;
                    }
                    match message.event {
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

        // GPUI waits for quit observers. Do not terminate before outstanding
        // buffer saves and session snapshots have reached disk.
        cx.on_app_quit(|workspace, cx| {
            workspace.save_all_dirty_quiet(cx);
            workspace.persist_workspace_state(cx);
            let flushed = workspace.storage.flush();
            async move {
                let _ = flushed.recv().await;
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

        let workspace = Self {
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
            auto_save_task: None,
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
            storage,
            session: Cancellation::default(),
            transition: Cancellation::default(),
            transition_task: None,
            loading_root: None,
            restore_startup_workspace: true,
            project_tasks: Vec::new(),
            directory_loads: HashMap::new(),
            file_loads: HashMap::new(),
            pending_user_open: None,
            file_open_generation: 0,
            pending_reveal: None,
            fs_watcher: None,
            fs_task: None,
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
            picker: None,
            picker_confirm_pending: false,
            workspace_files_cache: None,
            file_index_task: None,
            file_index_cancel: Cancellation::default(),
            picker_filter_task: None,
            picker_filter_cancel: Cancellation::default(),
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
            search_cancel: Cancellation::default(),
            search_task: None,
            search_collapsed: HashSet::new(),
            search_replace_open: false,
            pending_search_jump: None,
            pending_restore_tabs: Vec::new(),
        };

        cx.spawn(async move |this, cx| {
            if let Ok(global) = storage_ready.recv().await {
                let _ = this.update(cx, |workspace, cx| {
                    if workspace.restore_startup_workspace
                        && workspace.root.is_none()
                        && workspace.loading_root.is_none()
                    {
                        if let Some(root) = global.last_workspace_root {
                            workspace.load_root(root, cx);
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();

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

        let token = self.session.clone();
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
                if token.is_cancelled() {
                    return;
                }
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
        self.restore_startup_workspace = false;
        let _span = crate::perf::span("workspace.switch.request.ui");
        // Opening the active root is a no-op, but still supersedes a pending
        // request for another root. In particular, A -> B -> A is not B.
        self.transition.cancel();
        self.transition_task = None;
        self.loading_root = None;
        if self.root.as_ref() == Some(&path) {
            self.status = format!("Opened folder {}", self.root_display);
            cx.notify();
            return;
        }
        self.transition = Cancellation::default();
        let transition = self.transition.clone();
        let prepare_cancel = transition.clone();
        let store = self.storage.clone();
        self.loading_root = Some(path.clone());
        self.status = format!("Loading folder {}…", display_name(&path));
        cx.notify();
        self.transition_task = Some(cx.spawn(async move |this, cx| {
            let prepared = cx
                .background_spawn(async move {
                    session::prepare_workspace(path, &store, &prepare_cancel)
                })
                .await;
            if transition.is_cancelled() {
                return;
            }
            let mut prepared = match prepared {
                Ok(prepared) => Some(prepared),
                Err(error) => {
                    let _ = this.update(cx, |workspace, cx| {
                        if !transition.is_cancelled() {
                            workspace.loading_root = None;
                            workspace.status = error;
                            cx.notify();
                        }
                    });
                    return;
                }
            };
            // Keep the outgoing editors alive until their named dirty buffers
            // are safely saved. Failed writes leave the old workspace intact.
            // If typing continues during a write, flush its newer snapshot
            // before committing, rather than silently discarding those edits.
            loop {
                let saves = this
                    .update(cx, |workspace, cx| {
                        if transition.is_cancelled() {
                            return None;
                        }
                        if workspace.root.as_ref() == prepared.as_ref().map(|p| &p.root) {
                            workspace.loading_root = None;
                            workspace.status = format!("Opened folder {}", workspace.root_display);
                            cx.notify();
                            return None;
                        }
                        let mut saves = Vec::new();
                        for tab in &workspace.tabs {
                            if !tab.dirty {
                                continue;
                            }
                            if let (Some(path), Some(editor)) = (&tab.path, &tab.editor) {
                                let text: Arc<str> = editor.read(cx).value().to_string().into();
                                let result =
                                    workspace.storage.write_file(path.clone(), text.clone());
                                saves.push((editor.downgrade(), text, result));
                            }
                        }
                        Some(saves)
                    })
                    .ok()
                    .flatten();
                let Some(saves) = saves else {
                    return;
                };
                let mut saved = Vec::new();
                let mut error = None;
                for (editor, text, result) in saves {
                    match result.recv().await {
                        Ok(Ok(())) => saved.push((editor, text)),
                        Ok(Err(message)) => {
                            error = Some(message);
                            break;
                        }
                        Err(_) => {
                            error = Some("Storage worker stopped".into());
                            break;
                        }
                    }
                }
                if transition.is_cancelled() {
                    return;
                }
                let done = this
                    .update(cx, |workspace, cx| {
                        if transition.is_cancelled() {
                            return true;
                        }
                        if let Some(error) = error {
                            workspace.loading_root = None;
                            workspace.status =
                                format!("Folder switch cancelled — could not save {error}");
                            cx.notify();
                            return true;
                        }
                        for (editor, text) in saved {
                            if let Some(editor) = editor.upgrade() {
                                if editor.read(cx).value().as_str() == text.as_ref() {
                                    if let Some(tab) = workspace
                                        .tabs
                                        .iter_mut()
                                        .find(|tab| tab.editor.as_ref() == Some(&editor))
                                    {
                                        tab.dirty = false;
                                    }
                                }
                            }
                        }
                        if workspace
                            .tabs
                            .iter()
                            .any(|tab| tab.dirty && tab.path.is_some() && tab.editor.is_some())
                        {
                            return false;
                        }
                        workspace.commit_workspace(prepared.take().unwrap(), cx);
                        true
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
        }));
    }

    fn commit_workspace(&mut self, prepared: PreparedWorkspace, cx: &mut Context<Self>) {
        let _span = crate::perf::span("workspace.switch.commit.ui");
        self.persist_workspace_state(cx);
        self.session.cancel();
        self.project_tasks.clear();
        self.directory_loads.clear();
        self.file_loads.clear();
        self.pending_user_open = None;
        self.file_open_generation = self.file_open_generation.wrapping_add(1);
        self.fs_task = None;
        self.fs_watcher = None;
        self.file_index_cancel.cancel();
        self.file_index_task = None;
        self.picker_filter_cancel.cancel();
        self.picker_filter_task = None;
        self.close_all_project_tabs(cx);
        self.session = Cancellation::default();
        self.auto_save_generation = self.auto_save_generation.wrapping_add(1);
        self.auto_save_task = None;
        self.explorer_typeahead_generation = self.explorer_typeahead_generation.wrapping_add(1);
        self.explorer_drag_generation = self.explorer_drag_generation.wrapping_add(1);
        self.root_display = display_name(&prepared.root);
        self.root_display_shared = SharedString::from(self.root_display.clone());
        self.root = Some(prepared.root.clone());
        self.lsp
            .lock()
            .unwrap()
            .set_root(Some(prepared.root.clone()));
        self.storage.add_recent_folder(prepared.root.clone());
        self.tree = prepared.tree;
        self.explorer_rows = prepared.rows;
        self.explorer_scroll_handle = UniformListScrollHandle::new();
        self.explorer_section_expanded = true;
        self.explorer_sticky_rows = 0;
        self.selected_path = None;
        self.explorer_selection.clear();
        self.explorer_selection_anchor = None;
        self.explorer_typeahead.clear();
        self.explorer_drag_target = None;
        self.pending_reveal = None;
        self.inline_creating = None;
        self.inline_renaming = None;
        self.explorer_clipboard = None;
        self.markdown_preview = None;
        self.pending_restore_tabs.clear();
        self.picker_confirm_pending = false;
        self.panel_resize = None;
        if let Some(saved) = prepared.saved {
            self.sidebar_width = saved.layout.sidebar_width.clamp(170.0, 800.0);
            self.terminal_height = saved.layout.terminal_height.clamp(80.0, 800.0);
            self.show_sidebar = saved.layout.show_sidebar;
            self.show_terminal = saved.layout.show_terminal;
            self.terminal_maximized = saved.layout.terminal_maximized;
            self.terminal_right_width = saved
                .layout
                .terminal_right_width
                .clamp(TERMINAL_RIGHT_MIN_WIDTH, 800.0);
            self.show_terminal_right =
                saved.layout.show_terminal_right && !self.terminal_right_tabs.is_empty();
            self.explorer_sticky_scroll = saved.layout.explorer_sticky_scroll;
            self.activity = match saved.layout.activity.as_str() {
                "Search" => Activity::Search,
                "Git" => Activity::Git,
                "Extensions" => Activity::Extensions,
                _ => Activity::Explorer,
            };
            self.active_tab = self.tabs.len() + saved.active_tab;
            // Saved tabs are lightweight placeholders. Only the active tab
            // acquires a buffer/highlighter/LSP when it is actually visited.
            for tab in &saved.tabs {
                self.tabs.push(OpenTab {
                    path: Some(tab.path.clone()),
                    editor: None,
                    dirty: false,
                    untitled: false,
                    preview: tab.preview,
                    is_settings: false,
                    diff: None,
                    language_override: tab.language_override.clone(),
                });
            }
            self.pending_restore_tabs = saved.tabs;
            if let Some(selected) = saved.explorer_selected {
                self.set_explorer_selection(selected.clone());
                if let Some(index) = self
                    .explorer_rows
                    .iter()
                    .position(|row| row.path == selected)
                {
                    self.explorer_reveal_index(index);
                }
            }
        }
        self.active_tab = self.active_tab.min(self.tabs.len().saturating_sub(1));
        self.loading_root = None;
        self.status = format!("Opened folder {}", self.root_display);
        self.start_explorer_watcher(cx);
        self.start_git_watcher(&prepared.root, cx);
        crate::perf::mark("workspace.snapshot.published");
        cx.notify();
    }

    fn track_project_task(&mut self, task: Task<()>, cx: &mut Context<Self>) {
        self.project_tasks.retain(|(_, done)| !done.is_cancelled());
        let done = Cancellation::default();
        let completed = done.clone();
        let task = cx.spawn(async move |_, _| {
            task.await;
            completed.cancel();
        });
        self.project_tasks.push((task, done));
    }

    fn start_explorer_watcher(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let token = self.session.clone();
        let (watcher, events) = DirectoryWatcher::new(root.clone(), token.clone());
        for directory in session::loaded_directories(&root, &self.tree) {
            watcher.watch(directory);
        }
        self.fs_watcher = Some(watcher);
        self.fs_task = Some(cx.spawn(async move |this, cx| {
            while events.recv().await.is_ok() {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                let done = this
                    .update(cx, |workspace, cx| {
                        if token.is_cancelled() {
                            return true;
                        }
                        if let Some(watcher) = &workspace.fs_watcher {
                            let (directories, error) = watcher.drain();
                            if !directories.is_empty() {
                                workspace.invalidate_file_index(cx);
                                for directory in directories {
                                    if workspace.directory_is_loaded(&directory) {
                                        workspace.load_directory_async(directory, cx);
                                    }
                                }
                            }
                            if let Some(error) = error {
                                workspace.status = error;
                                cx.notify();
                            }
                        }
                        false
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
        }));
    }

    /// Close every open editor when leaving a project so the next folder
    /// starts clean. Unsaved buffers are flushed first.
    fn close_all_project_tabs(&mut self, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            if let Some(p) = &tab.path {
                if let Some(lang_id) = tab.language() {
                    self.lsp.lock().unwrap().close_document(p, lang_id);
                }
            }
        }

        // Untitled scratch buffers have no save destination; keep them rather
        // than lose unsaved text during a folder switch.
        self.tabs.retain(|tab| tab.untitled);
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
        self.git_op_running = None;
    }

    pub(crate) fn start_git_watcher(&mut self, root: &Path, cx: &mut Context<Self>) {
        // Invalidate whatever watcher may still be running (re-opening the
        // same folder, or a folder that is not a repository) before starting
        // a new one, so stale snapshots can never race the fresh ones.
        self.stop_git_watcher();
        let root = root.to_path_buf();
        let token = self.session.clone();
        let generation = self.git_watch_generation;
        let (poke_tx, poke_rx) = std::sync::mpsc::channel::<()>();
        let (status_tx, status_rx) =
            async_channel::bounded::<(RepoStatus, Arc<HashMap<PathBuf, ChangeKind>>)>(1);
        std::thread::spawn(move || {
            if token.is_cancelled() {
                return;
            }
            let Some(repo_root) = git::find_repo_root(&root) else {
                return;
            };
            let mut last: Option<RepoStatus> = None;
            loop {
                if token.is_cancelled() {
                    break;
                }
                if let Some(status) = git::status(&repo_root) {
                    if token.is_cancelled() {
                        break;
                    }
                    if last.as_ref() != Some(&status) {
                        let kinds = Arc::new(git::path_kinds(&status));
                        if status_tx.send_blocking((status.clone(), kinds)).is_err() {
                            break;
                        }
                        last = Some(status);
                    }
                }
                match poke_rx.recv_timeout(Duration::from_millis(1500)) {
                    Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
                // Multiple saves/stage operations need only one fresh status.
                while poke_rx.try_recv().is_ok() {}
            }
        });

        self.git_poke_tx = Some(poke_tx);

        cx.spawn({
            let rx = status_rx.clone();
            async move |this, cx| {
                while let Ok((status, kinds)) = rx.recv().await {
                    let done = this
                        .update(cx, |workspace, cx| {
                            // A snapshot from a superseded watcher must not
                            // clobber the current repository's state.
                            if workspace.git_watch_generation != generation {
                                return true;
                            }
                            workspace.apply_git_status(status, kinds, cx);
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
    fn apply_git_status(
        &mut self,
        status: RepoStatus,
        kinds: Arc<HashMap<PathBuf, ChangeKind>>,
        cx: &mut Context<Self>,
    ) {
        self.git_path_kinds = kinds;
        self.git = Some(status);
        self.refresh_diff_tabs(cx);
        cx.notify();
    }

    pub(crate) fn git_poke(&self) {
        if let Some(tx) = &self.git_poke_tx {
            let _ = tx.send(());
        }
    }

    pub(crate) fn open_folder_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.restore_startup_workspace = false;
        self.status = "Choose folder…".into();
        cx.notify();
        // Native dialogs pump Windows messages; run them outside the App borrow.
        cx.spawn(async move |this, cx| {
            let path = rfd::AsyncFileDialog::new()
                .pick_folder()
                .await
                .map(|handle| handle.path().to_path_buf());
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
        self.restore_startup_workspace = false;
        self.status = "Choose file…".into();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let path = rfd::AsyncFileDialog::new()
                .pick_file()
                .await
                .map(|handle| handle.path().to_path_buf());
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
        self.restore_startup_workspace = false;
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

        self.watch_editor(&editor, cx);

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

    fn reveal_tree_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self
            .root
            .as_ref()
            .is_some_and(|root| path.starts_with(root))
        {
            return;
        }
        self.pending_reveal = Some(path.to_path_buf());
        self.continue_tree_reveal(cx);
    }

    fn continue_tree_reveal(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.pending_reveal.clone() else {
            return;
        };
        fn expand_to(nodes: &mut [TreeNode], target: &Path, changed: &mut bool) -> Option<PathBuf> {
            for node in nodes {
                if target.starts_with(&node.path) && node.is_dir {
                    *changed |= !node.expanded;
                    node.expanded = true;
                    if !node.children_loaded {
                        return Some(node.path.clone());
                    }
                    return expand_to(&mut node.children, target, changed);
                }
            }
            None
        }
        let mut changed = !self.explorer_section_expanded;
        self.explorer_section_expanded = true;
        let directory = expand_to(&mut self.tree, &path, &mut changed);
        if changed {
            self.rebuild_explorer_rows();
        }
        if let Some(directory) = directory {
            self.request_directory(directory, false, cx);
        } else {
            self.pending_reveal = None;
            if let Some(index) = self.explorer_rows.iter().position(|row| row.path == path) {
                self.explorer_reveal_index(index);
            }
        }
    }

    pub(crate) fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.restore_startup_workspace = false;
        self.file_open_generation = self.file_open_generation.wrapping_add(1);
        if let Some(previous) = self
            .pending_user_open
            .take()
            .filter(|previous| previous != &path)
        {
            if self
                .file_loads
                .get(&previous)
                .is_some_and(|load| !load.pinned)
            {
                self.cancel_file_load(&previous);
            }
        }
        self.set_explorer_selection(path.clone());
        self.reveal_tree_path(&path, cx);
        if let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.path.as_ref() == Some(&path))
        {
            self.active_tab = index;
            self.tabs[index].preview = false;
            self.storage.add_recent_file(path.clone());
            cx.notify();
            return;
        }
        self.pending_user_open = Some(path.clone());
        if let Some(load) = self.file_loads.get_mut(&path) {
            load.open_generation = Some(self.file_open_generation);
            return;
        }
        let token = self.session.child();
        self.file_loads.insert(
            path.clone(),
            FileLoad {
                token: token.clone(),
                pinned: false,
                open_generation: Some(self.file_open_generation),
            },
        );
        self.status = format!("Opening {}…", display_name(&path));
        cx.notify();
        let task = cx.spawn_in(window, async move |this, cx| {
            let load_path = path.clone();
            let load_token = token.clone();
            let loaded = cx
                .background_spawn(async move { load_buffer_file(&load_path, &load_token) })
                .await;
            let _ = this.update_in(cx, move |workspace, window, cx| {
                workspace.complete_file_load(path, &token, loaded, window, cx);
            });
        });
        self.track_project_task(task, cx);
    }

    fn complete_file_load(
        &mut self,
        path: PathBuf,
        token: &Cancellation,
        loaded: Result<LoadedBuffer, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if token.is_cancelled() {
            return;
        }
        let Some(load) = self.file_loads.remove(&path) else {
            return;
        };
        let activate = load.open_generation == Some(self.file_open_generation);
        if self.pending_user_open.as_ref() == Some(&path) {
            self.pending_user_open = None;
        }
        self.finish_open_file(path, loaded, activate, load.pinned, window, cx);
    }

    fn cancel_file_load(&mut self, path: &Path) {
        if let Some(load) = self.file_loads.remove(path) {
            load.token.cancel();
        }
        self.pending_restore_tabs.retain(|tab| tab.path != path);
    }

    /// Render schedules at most the active restored buffer. Inactive tabs
    /// remain cheap metadata, preserving their order, cursor and language.
    pub(crate) fn restore_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        if tab.editor.is_some() {
            return;
        }
        let Some(path) = tab.path.clone() else {
            return;
        };
        let Some(info) = self
            .pending_restore_tabs
            .iter()
            .find(|tab| tab.path == path)
            .cloned()
        else {
            return;
        };
        if self.file_loads.contains_key(&path) {
            return;
        }
        let token = self.session.child();
        self.file_loads.insert(
            path.clone(),
            FileLoad {
                token: token.clone(),
                pinned: !info.preview,
                open_generation: None,
            },
        );
        let task = cx.spawn_in(window, async move |this, cx| {
            let load_path = path.clone();
            let load_token = token.clone();
            let loaded = cx
                .background_spawn(async move { load_buffer_file(&load_path, &load_token) })
                .await;
            let _ = this.update_in(cx, move |workspace, window, cx| {
                if token.is_cancelled() {
                    return;
                }
                workspace.file_loads.remove(&path);
                workspace
                    .pending_restore_tabs
                    .retain(|tab| tab.path != path);
                if let Some(index) = workspace
                    .tabs
                    .iter()
                    .position(|tab| tab.path.as_ref() == Some(&path))
                {
                    match loaded {
                        Ok(loaded) => workspace.restore_tab(index, info, loaded, window, cx),
                        Err(error) => {
                            workspace.close_tab_at_index(index, cx);
                            workspace.status = error;
                        }
                    }
                    cx.notify();
                }
            });
        });
        self.track_project_task(task, cx);
    }

    fn watch_editor(&self, editor: &Entity<InputState>, cx: &mut Context<Self>) {
        // GPUI passes the emitting entity. Capturing a strong editor handle in
        // its own detached subscription creates a cycle in App::event_listeners
        // and keeps the buffer AND every attached language server alive forever.
        cx.subscribe(editor, |workspace, editor, event: &InputEvent, cx| {
            if !matches!(event, InputEvent::Change) {
                return;
            }
            let Some(index) = workspace
                .tabs
                .iter()
                .position(|tab| tab.editor.as_ref() == Some(&editor))
            else {
                return;
            };
            let tab = &mut workspace.tabs[index];
            let changed = !tab.dirty || tab.preview;
            tab.dirty = true;
            tab.preview = false;
            let path = tab.path.clone();
            let language = tab.language().map(str::to_string);
            if let (Some(path), Some(language)) = (path, language) {
                {
                    let mut lsp = workspace.lsp.lock().unwrap();
                    if lsp.has_client(&language) {
                        lsp.change_document(&path, &language, editor.read(cx).value().to_string());
                    }
                }
                workspace.markdown_buffer_changed(&path, &editor, cx);
            }
            workspace.trigger_auto_save_after_delay(index, cx);
            if changed {
                cx.notify();
            }
        })
        .detach();
    }

    fn finish_open_file(
        &mut self,
        path: PathBuf,
        loaded: Result<LoadedBuffer, String>,
        activate: bool,
        pinned: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let loaded = match loaded {
            Ok(loaded) => loaded,
            Err(message) => {
                if !activate {
                    return;
                }
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
            if activate {
                self.active_tab = idx;
            }
            if let Some(tab) = self.tabs.get_mut(idx) {
                tab.preview = false;
            }
            cx.notify();
            return;
        }

        let replace_preview = if let Some(active_idx) = self.tabs.get(self.active_tab) {
            activate && active_idx.preview && !active_idx.dirty
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

        self.watch_editor(&editor, cx);

        if replace_preview {
            if let Some(previous) = self
                .tabs
                .get(self.active_tab)
                .and_then(|tab| tab.path.clone())
            {
                self.cancel_file_load(&previous);
                if let Some(language) = lang::language_for(&previous) {
                    self.lsp.lock().unwrap().close_document(&previous, language);
                }
            }
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                tab.path = Some(path.clone());
                tab.dirty = false;
                tab.untitled = false;
                tab.preview = !pinned;
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
                preview: !pinned,
                is_settings: false,
                diff: None,
                language_override: None,
            });
            if activate {
                self.active_tab = self.tabs.len() - 1;
            }
        }
        if activate {
            self.status = if highlight {
                path.display().to_string()
            } else {
                format!("{} (plain text — large file)", display_name(&path))
            };

            self.storage.add_recent_file(path.clone());
        }

        self.persist_workspace_state(cx);

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
        let token = self.session.clone();
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
            if token.is_cancelled() {
                return;
            }
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
                if token.is_cancelled() {
                    return;
                }
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
        let text: Arc<str> = text.into();
        let editor = self
            .tabs
            .iter()
            .find(|tab| tab.path.as_ref() == Some(&path))
            .and_then(|tab| tab.editor.as_ref())
            .map(Entity::downgrade);
        let token = self.session.clone();
        let saved = self.storage.write_file(path.clone(), text.clone());
        self.status = format!("Saving {}…", display_name(&path));
        cx.spawn(async move |this, cx| {
            let result = saved
                .recv()
                .await
                .unwrap_or_else(|_| Err("Storage worker stopped".into()));
            let _ = this.update(cx, |workspace, cx| {
                if token.is_cancelled() {
                    return;
                }
                let Some(editor) = editor.and_then(|editor| editor.upgrade()) else {
                    return;
                };
                let Some(index) = workspace.tabs.iter().position(|tab| {
                    tab.editor.as_ref() == Some(&editor) && tab.path.as_ref() == Some(&path)
                }) else {
                    return;
                };
                match result {
                    Ok(()) => {
                        if editor.read(cx).value().as_str() == text.as_ref() {
                            workspace.tabs[index].dirty = false;
                        }
                        if let Some(language) = workspace.tabs[index].language() {
                            workspace
                                .lsp
                                .lock()
                                .unwrap()
                                .save_document(&path, language, &text);
                        }
                        workspace.git_poke();
                        if path == crate::settings::settings_file_path() {
                            workspace.reload_settings(cx);
                        }
                        workspace.status = format!("{done_label} {}", display_name(&path));
                        workspace.persist_workspace_state(cx);
                    }
                    Err(error) => workspace.status = format!("save failed: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save_as(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        if tab.is_settings {
            return;
        }
        let Some(editor) = tab.editor.as_ref() else {
            return;
        };
        let text: Arc<str> = editor.read(cx).value().to_string().into();
        let editor = editor.downgrade();
        let token = self.session.clone();
        let store = self.storage.clone();
        self.status = "Choose save location…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let path = rfd::AsyncFileDialog::new()
                .set_file_name("untitled.txt")
                .save_file()
                .await
                .map(|handle| handle.path().to_path_buf());
            if token.is_cancelled() {
                return;
            }
            let Some(path) = path else {
                let _ = this.update(cx, |workspace, cx| {
                    workspace.status = "Save cancelled".into();
                    cx.notify();
                });
                return;
            };
            let result = store
                .write_file(path.clone(), text.clone())
                .recv()
                .await
                .unwrap_or_else(|_| Err("Storage worker stopped".into()));
            let _ = this.update(cx, move |workspace, cx| {
                if token.is_cancelled() {
                    return;
                }
                match result {
                    Ok(()) => {
                        let Some(editor) = editor.upgrade() else {
                            return;
                        };
                        let Some(index) = workspace
                            .tabs
                            .iter()
                            .position(|tab| tab.editor.as_ref() == Some(&editor))
                        else {
                            return;
                        };
                        let language = lang::language_for(&path).unwrap_or("text");
                        let tab = &mut workspace.tabs[index];
                        tab.path = Some(path.clone());
                        tab.untitled = false;
                        tab.dirty = editor.read(cx).value().as_str() != text.as_ref();
                        editor.update(cx, |state, cx| state.set_highlighter(language, cx));
                        workspace.attach_language_server(&path, language, &editor, cx);
                        workspace.selected_path = Some(path.clone());
                        if let Some(parent) = path.parent() {
                            workspace.reload_dir(parent, cx);
                        }
                        workspace.git_poke();
                        workspace.status = format!("Saved {}", display_name(&path));
                        workspace.persist_workspace_state(cx);
                    }
                    Err(error) => workspace.status = format!("save failed: {error}"),
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
            self.request_directory(dir, false, cx);
        }
    }

    pub(crate) fn refresh_explorer(&mut self, cx: &mut Context<Self>) {
        self.workspace_files_cache = None;
        let Some(root) = self.root.clone() else {
            return;
        };
        for directory in session::loaded_directories(&root, &self.tree) {
            self.load_directory_async(directory, cx);
        }
    }

    fn directory_is_loaded(&self, directory: &Path) -> bool {
        fn loaded(nodes: &[TreeNode], directory: &Path) -> bool {
            nodes.iter().any(|node| {
                (node.path == directory && node.children_loaded)
                    || (directory.starts_with(&node.path) && loaded(&node.children, directory))
            })
        }
        self.root.as_deref() == Some(directory) || loaded(&self.tree, directory)
    }

    fn apply_loaded_dir_inner(&mut self, dir: &Path, entries: Vec<TreeNode>) -> bool {
        let Some(root) = self.root.clone() else {
            return false;
        };
        if dir == root.as_path() {
            if entries_match(&self.tree, &entries) {
                return false;
            }
            let previous = std::mem::take(&mut self.tree);
            self.tree = merge_loaded_dir(dir, previous, entries);
            return true;
        }

        fn apply(nodes: &mut [TreeNode], dir: &Path, entries: &mut Option<Vec<TreeNode>>) -> bool {
            for node in nodes {
                if node.path == dir {
                    if node.is_dir && node.expanded {
                        if node.children_loaded
                            && entries_match(&node.children, entries.as_deref().unwrap_or_default())
                        {
                            return false;
                        }
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

    fn load_directory_async(&mut self, directory: PathBuf, cx: &mut Context<Self>) {
        self.request_directory(directory, true, cx);
    }

    fn request_directory(&mut self, directory: PathBuf, refresh: bool, cx: &mut Context<Self>) {
        if !self
            .root
            .as_ref()
            .is_some_and(|root| directory.starts_with(root))
        {
            return;
        }
        if let Some(again) = self.directory_loads.get_mut(&directory) {
            *again |= refresh;
            return;
        }
        self.directory_loads.insert(directory.clone(), false);
        let token = self.session.clone();
        let task = cx.spawn(async move |this, cx| {
            let scan_directory = directory.clone();
            let scan_token = token.clone();
            let entries = cx
                .background_spawn(async move {
                    try_load_dir(&scan_directory, || scan_token.is_cancelled())
                })
                .await;
            let _ = this.update(cx, |workspace, cx| {
                if token.is_cancelled() {
                    return;
                }
                let again = workspace
                    .directory_loads
                    .remove(&directory)
                    .unwrap_or(false);
                match entries {
                    Ok(entries) => {
                        if workspace.apply_loaded_dir_inner(&directory, entries) {
                            workspace.rebuild_explorer_rows();
                            cx.notify();
                        }
                        if let Some(watcher) = &workspace.fs_watcher {
                            watcher.watch(directory.clone());
                        }
                        workspace.continue_tree_reveal(cx);
                    }
                    Err(error) => {
                        workspace.pending_reveal = None;
                        workspace.status =
                            format!("Could not read {}: {error}", directory.display());
                        cx.notify();
                    }
                }
                if again {
                    workspace.load_directory_async(directory, cx);
                }
            });
        });
        self.track_project_task(task, cx);
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
        if let Some(load) = self.file_loads.get_mut(path) {
            load.pinned = true;
        }
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
            self.ensure_directory_visible(&destination_dir, cx);
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

    fn ensure_directory_visible(&mut self, dir: &Path, cx: &mut Context<Self>) {
        self.reveal_tree_path(dir, cx);
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
        self.ensure_directory_visible(&dir, cx);
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
                self.ensure_directory_visible(&creating.parent_dir, cx);
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
        self.reveal_tree_path(&path, cx);

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
            self.ensure_directory_visible(&destination_dir, cx);
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
        let (client, launch) = {
            let mut lsp = self.lsp.lock().unwrap();
            let client = lsp.client_for(lang_id);
            let launch = if client.is_none() {
                lsp.reserve_server(lang_id)
            } else {
                None
            };
            (client, launch)
        };
        if let Some(launch) = launch {
            let token = self.session.clone();
            let manager = self.lsp.clone();
            let task = cx.spawn(async move |this, cx| {
                let (launch, result) = cx
                    .background_spawn(async move {
                        let result = launch.run();
                        (launch, result)
                    })
                    .await;
                if token.is_cancelled() {
                    return;
                }
                let ready = manager.lock().unwrap().finish_start(&launch, result);
                let _ = this.update(cx, |workspace, cx| {
                    if token.is_cancelled() {
                        return;
                    }
                    if ready {
                        workspace.start_server_for_open_buffers(launch.name(), cx);
                    }
                    cx.notify();
                });
            });
            self.track_project_task(task, cx);
        }
        let Some(client) = client else {
            return false;
        };
        let text = editor.read(cx).value().to_string();
        client.did_open(path, lang_id, &text);
        let lsp_path = path.to_path_buf();
        editor.update(cx, move |state, _cx| {
            crate::lsp::attach_lsp_providers(state, client, lsp_path);
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
            let status = cx
                .background_spawn(async move {
                    git::status(&root).map(|status| {
                        let kinds = Arc::new(git::path_kinds(&status));
                        (status, kinds)
                    })
                })
                .await;
            let _ = this.update(cx, |workspace, cx| {
                if workspace.git_watch_generation != generation {
                    return;
                }
                match status {
                    Some((status, kinds)) => {
                        workspace.apply_git_status(status, kinds, cx);
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
        let token = self.session.clone();
        cx.spawn(async move |this, cx| {
            let ok = cx.background_spawn(async move { op(root, rels) }).await;
            let _ = this.update(cx, |workspace, cx| {
                if token.is_cancelled() {
                    return;
                }
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
        let token = self.session.clone();
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
                if token.is_cancelled() {
                    return;
                }
                workspace.status = status;
                if is_ok && !token.is_cancelled() {
                    workspace.git_poke();
                }
                cx.notify();
            });

            if is_ok && !token.is_cancelled() {
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
        let token = self.session.clone();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { op(root) }).await;
            let _ = this.update(cx, |workspace, cx| {
                if token.is_cancelled() {
                    return;
                }
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
        let token = self.session.clone();
        cx.spawn(async move |this, cx| {
            let init_root = root.clone();
            let result = cx
                .background_spawn(async move { git::init(&init_root) })
                .await;
            let _ = this.update(cx, |workspace, cx| {
                if token.is_cancelled() {
                    return;
                }
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

        self.load_diff_tab(root, rel, path.to_path_buf(), staged, cx);
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
        cx: &mut Context<Self>,
    ) {
        let token = self.session.clone();
        cx.spawn(async move |this, cx| {
            let tab_path_bg = tab_path.clone();
            let rel_bg = rel.clone();
            let (text, parsed, error) = cx
                .background_spawn(async move {
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
                if token.is_cancelled() {
                    return;
                }
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
        let jobs: Vec<(String, PathBuf, bool)> = self
            .tabs
            .iter()
            .filter_map(|t| t.diff.as_ref())
            .map(|d| (d.rel.clone(), d.path.clone(), d.staged))
            .collect();
        for (rel, path, staged) in jobs {
            self.load_diff_tab(root.clone(), rel, path, staged, cx);
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
        let token = self.session.clone();
        cx.spawn_in(window, async move |this, cx| {
            let edits = cx
                .background_spawn(async move { client.format_document(&path, &text, tab_size) })
                .await;
            if token.is_cancelled() {
                return;
            }
            match edits {
                Some(edits) if !edits.is_empty() => {
                    let _ = editor_weak.update_in(cx, |state, window, cx| {
                        state.apply_lsp_edits(&edits, window, cx);
                    });
                    let _ = this.update(cx, |workspace, cx| {
                        if token.is_cancelled() {
                            return;
                        }
                        workspace.status = format!("Formatted {display}");
                        cx.notify();
                    });
                }
                Some(_) => {
                    let _ = this.update(cx, |workspace, cx| {
                        if token.is_cancelled() {
                            return;
                        }
                        workspace.status = "Document already formatted".into();
                        cx.notify();
                    });
                }
                None => {
                    let _ = this.update(cx, |workspace, cx| {
                        if token.is_cancelled() {
                            return;
                        }
                        workspace.status = "Formatting not supported by the language server".into();
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    pub(crate) fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.restore_startup_workspace = false;
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
        // for the buffer. Snapshots go through the same ordered background
        // writer as manual saves and workspace transitions.
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

        self.auto_save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;

            let _ = this.update(cx, |workspace, cx| {
                if workspace.auto_save_generation == current_gen {
                    workspace.save_all_dirty_quiet(cx);
                }
            });
        }));
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
        if index >= self.tabs.len() {
            return;
        }
        if let Some(path) = self.tabs[index].path.clone() {
            self.cancel_file_load(&path);
        }
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
                    self.reveal_tree_path(&path, cx);
                }
            }
            self.persist_workspace_state(cx);
            cx.notify();
        }
    }

    pub(crate) fn close_tab_at_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            if let Some(path) = self.tabs[index].path.clone() {
                self.cancel_file_load(&path);
            }
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

        let root_dir = self.root.clone().unwrap_or_else(|| PathBuf::from("."));
        let recent_files = self.file_finder_recent_files();
        let cached = self
            .workspace_files_cache
            .as_ref()
            .filter(|(root, _)| root == &root_dir)
            .map(|(_, items)| items.clone());
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

        let mut picker = crate::ui::picker::PickerState::new(
            crate::ui::picker::PickerKind::FileFinder,
            input,
            Vec::new(),
        );
        picker.indexing = cached.is_none();
        picker.loading = true;
        if let Some(cached) = cached {
            picker.raw_items = cached;
        }
        self.picker = Some(picker);
        if self.picker.as_ref().is_some_and(|picker| !picker.indexing) {
            self.filter_file_finder(cx);
        } else {
            self.start_file_index(root_dir, recent_files, cx);
        }
        cx.notify();
    }

    fn invalidate_file_index(&mut self, cx: &mut Context<Self>) {
        self.workspace_files_cache = None;
        self.file_index_cancel.cancel();
        self.file_index_task = None;
        if self
            .picker
            .as_ref()
            .is_some_and(|picker| picker.kind == crate::ui::picker::PickerKind::FileFinder)
        {
            let root = self.root.clone().unwrap_or_else(|| PathBuf::from("."));
            let recent = self.storage.recent().recent_files;
            if let Some(picker) = &mut self.picker {
                picker.indexing = true;
                picker.loading = true;
            }
            self.start_file_index(root, recent, cx);
        }
    }

    fn start_file_index(&mut self, root: PathBuf, recent: Vec<PathBuf>, cx: &mut Context<Self>) {
        // Closing/reopening Ctrl+P while a scan runs shares that scan. It
        // belongs to the project, not to the lifetime of a particular modal.
        if self.file_index_task.is_some() {
            return;
        }
        self.file_index_cancel = self.session.child();
        let cancelled = self.file_index_cancel.clone();
        let token = self.session.clone();
        let watches = self.fs_watcher.as_ref().map(DirectoryWatcher::sender);
        self.file_index_task = Some(cx.spawn(async move |this, cx| {
            let scan_root = root.clone();
            let scan_cancel = cancelled.clone();
            let scan_token = token.clone();
            let items = cx
                .background_spawn(async move {
                    if scan_token.is_cancelled() {
                        return Vec::new();
                    }
                    crate::ui::picker::scan_workspace_files_cancellable(
                        &scan_root,
                        &recent,
                        &scan_cancel,
                        watches,
                    )
                })
                .await;
            let _ = this.update(cx, |workspace, cx| {
                if token.is_cancelled() || cancelled.is_cancelled() {
                    return;
                }
                workspace.file_index_task = None;
                let items = Arc::new(items);
                workspace.workspace_files_cache = Some((root, items.clone()));
                if let Some(picker) = workspace
                    .picker
                    .as_mut()
                    .filter(|picker| picker.kind == crate::ui::picker::PickerKind::FileFinder)
                {
                    picker.raw_items = items;
                    picker.indexing = false;
                    workspace.filter_file_finder(cx);
                }
                cx.notify();
            });
        }));
    }

    fn file_finder_recent_files(&self) -> Vec<PathBuf> {
        let mut recent = Vec::new();
        for path in self
            .active_path()
            .into_iter()
            .cloned()
            .chain(self.storage.recent().recent_files)
            .chain(self.tabs.iter().rev().filter_map(|tab| tab.path.clone()))
        {
            if !recent.contains(&path)
                && self.root.as_ref().is_none_or(|root| path.starts_with(root))
            {
                recent.push(path);
            }
        }
        recent
    }

    fn filter_file_finder(&mut self, cx: &mut Context<Self>) {
        let recent_files = self.file_finder_recent_files();
        let Some(picker) = self.picker.as_mut() else {
            return;
        };
        let query = picker.input.read(cx).value().to_string();
        let input = picker.input.downgrade();
        let items = picker.raw_items.clone();
        picker.loading = true;
        self.picker_filter_cancel.cancel();
        self.picker_filter_cancel = self.session.child();
        let cancelled = self.picker_filter_cancel.clone();
        let token = self.session.clone();
        self.picker_filter_task = Some(cx.spawn(async move |this, cx| {
            let filter_cancel = cancelled.clone();
            let results = cx
                .background_spawn(async move {
                    crate::ui::picker::filter_items_with_recents(
                        &items,
                        &query,
                        &filter_cancel,
                        &recent_files,
                    )
                })
                .await;
            let _ = this.update(cx, |workspace, cx| {
                if token.is_cancelled() || cancelled.is_cancelled() {
                    return;
                }
                if let Some(picker) = workspace.picker.as_mut() {
                    if input.upgrade().as_ref() != Some(&picker.input) {
                        return;
                    }
                    picker.filtered_items = results;
                    picker.selected_index = 0;
                    picker.loading = picker.indexing;
                    cx.notify();
                }
            });
        }));
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
        let token = self.session.clone();
        cx.spawn_in(window, async move |this, cx| {
            let branches = cx
                .background_spawn(async move { git::branches(&root) })
                .await;
            let _ = this.update_in(cx, |workspace, window, cx| {
                if token.is_cancelled() {
                    return;
                }
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
        let token = self.session.clone();
        cx.spawn_in(window, async move |this, cx| {
            let branches = cx
                .background_spawn(async move { git::branches(&root) })
                .await;
            let _ = this.update_in(cx, |workspace, window, cx| {
                if token.is_cancelled() {
                    return;
                }
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
            self.picker_filter_cancel.cancel();
            self.picker_filter_task = None;
            self.picker_confirm_pending = false;
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
        if self
            .picker
            .as_ref()
            .is_some_and(|picker| picker.kind == crate::ui::picker::PickerKind::FileFinder)
        {
            self.filter_file_finder(cx);
        } else if let Some(picker) = &mut self.picker {
            picker.filter(&picker.input.read(cx).value().to_string());
        }
        cx.notify();
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

    fn restore_tab(
        &mut self,
        index: usize,
        info: crate::storage::OpenTabState,
        loaded: LoadedBuffer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let language = if loaded.highlight {
            info.language_override
                .clone()
                .unwrap_or_else(|| loaded.lang_id.to_string())
        } else {
            "text".to_string()
        };
        let editor = cx.new(|cx| {
            let mut state = InputState::new(window, cx)
                .code_editor(language.clone())
                .line_number(true)
                .indent_guides(false)
                .soft_wrap(false)
                .searchable(true)
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                });
            state.set_value(loaded.text, window, cx);
            if let Some(position) = info.cursor {
                state.set_cursor_position(
                    lsp_types::Position {
                        line: position.line,
                        character: position.character,
                    },
                    window,
                    cx,
                );
            }
            state
        });
        self.watch_editor(&editor, cx);
        self.tabs[index].editor = Some(editor.clone());
        self.attach_language_server(&info.path, &language, &editor, cx);
        if index == self.active_tab {
            editor.update(cx, |state, cx| state.focus(window, cx));
        }
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
        let mut active_tab = 0;
        for (index, tab) in self.tabs.iter().enumerate() {
            if let Some(path) = &tab.path {
                if index == self.active_tab {
                    active_tab = tab_states.len();
                }
                let cursor = tab
                    .editor
                    .as_ref()
                    .map(|ed| {
                        let pos = ed.read(cx).cursor_position();
                        crate::storage::CursorPosition {
                            line: pos.line,
                            character: pos.character,
                        }
                    })
                    .or_else(|| {
                        self.pending_restore_tabs
                            .iter()
                            .find(|tab| &tab.path == path)
                            .and_then(|tab| tab.cursor.clone())
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
            active_tab,
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

        self.storage.save(state);
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        self.transition.cancel();
        self.session.cancel();
    }
}
