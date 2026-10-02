use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use crate::settings::config_dir;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CursorPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenTabState {
    pub path: PathBuf,
    #[serde(default)]
    pub preview: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_override: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<CursorPosition>,
}

fn default_sidebar_width() -> f32 {
    300.0
}

fn default_terminal_height() -> f32 {
    320.0
}

fn default_terminal_right_width() -> f32 {
    420.0
}

fn default_true() -> bool {
    true
}

fn default_activity() -> String {
    "Explorer".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LayoutState {
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: f32,
    #[serde(default = "default_terminal_height")]
    pub terminal_height: f32,
    #[serde(default = "default_terminal_right_width")]
    pub terminal_right_width: f32,
    #[serde(default = "default_true")]
    pub show_sidebar: bool,
    #[serde(default)]
    pub show_terminal: bool,
    #[serde(default)]
    pub show_terminal_right: bool,
    #[serde(default)]
    pub terminal_maximized: bool,
    #[serde(default = "default_activity")]
    pub activity: String,
    /// VS Code's `workbench.tree.enableStickyScroll`, per workspace.
    #[serde(default = "default_true")]
    pub explorer_sticky_scroll: bool,
}

impl Default for LayoutState {
    fn default() -> Self {
        Self {
            sidebar_width: default_sidebar_width(),
            terminal_height: default_terminal_height(),
            terminal_right_width: default_terminal_right_width(),
            show_sidebar: default_true(),
            show_terminal: false,
            show_terminal_right: false,
            terminal_maximized: false,
            activity: default_activity(),
            explorer_sticky_scroll: default_true(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct WorkspaceState {
    pub root: PathBuf,
    #[serde(default)]
    pub tabs: Vec<OpenTabState>,
    #[serde(default)]
    pub active_tab: usize,
    #[serde(default)]
    pub layout: LayoutState,
    #[serde(default)]
    pub expanded_folders: Vec<PathBuf>,
    /// Focused explorer row, restored on the next session like VS Code does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explorer_selected: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct GlobalState {
    #[serde(default)]
    pub recent_folders: Vec<PathBuf>,
    #[serde(default)]
    pub recent_files: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_workspace_root: Option<PathBuf>,
}

/// Returns the path to the `globalStorage` directory under ezicode's config directory.
pub fn global_storage_dir() -> PathBuf {
    config_dir().join("globalStorage")
}

/// Returns the path to the `workspaceStorage` directory under ezicode's config directory.
pub fn workspace_storage_base_dir() -> PathBuf {
    config_dir().join("workspaceStorage")
}

/// Returns the full path to `globalStorage/storage.json`.
pub fn global_storage_file() -> PathBuf {
    global_storage_dir().join("storage.json")
}

/// Computes a deterministic, filesystem-safe ID for a workspace root path.
/// Format: `<folder_name>-<hash>` (e.g. `my-project-7a8f3b2c1d0e4f5a`).
pub fn workspace_id(root: &Path) -> String {
    let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let folder_name = canonical
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("workspace");

    let clean_name: String = folder_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();

    let mut hasher = DefaultHasher::new();
    canonical.to_string_lossy().hash(&mut hasher);
    let hash = hasher.finish();

    format!("{clean_name}-{hash:016x}")
}

/// Returns the path to the workspace storage directory for a specific workspace root.
pub fn workspace_storage_dir(root: &Path) -> PathBuf {
    workspace_storage_base_dir().join(workspace_id(root))
}

/// Returns the path to the `state.json` file for a specific workspace root.
pub fn workspace_state_file(root: &Path) -> PathBuf {
    workspace_storage_dir(root).join("state.json")
}

fn write_json_safe<T: Serialize>(path: &Path, value: &T) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, json.as_bytes())?;
    std::fs::rename(temporary, path)
}

impl GlobalState {
    #[allow(dead_code)]
    pub fn load() -> Self {
        read_json(&global_storage_file()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), std::io::Error> {
        write_json_safe(&global_storage_file(), self)
    }

    fn record_folder(&mut self, folder: PathBuf) {
        let canonical = std::fs::canonicalize(&folder).unwrap_or(folder);
        self.recent_folders.retain(|path| path != &canonical);
        self.recent_folders.insert(0, canonical.clone());
        self.recent_folders.truncate(30);
        self.last_workspace_root = Some(canonical);
    }

    #[allow(dead_code)]
    pub fn add_recent_folder(&mut self, folder: PathBuf) {
        self.record_folder(folder);
        let _ = self.save();
    }

    #[allow(dead_code)]
    pub fn remove_recent_folder(&mut self, folder: &Path) {
        let canonical = std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
        self.recent_folders.retain(|path| path != &canonical);
        if self.last_workspace_root.as_ref() == Some(&canonical) {
            self.last_workspace_root = self.recent_folders.first().cloned();
        }
        let _ = self.save();
    }

    fn record_file(&mut self, file: PathBuf) {
        let canonical = std::fs::canonicalize(&file).unwrap_or(file);
        self.recent_files.retain(|path| path != &canonical);
        self.recent_files.insert(0, canonical);
        self.recent_files.truncate(50);
    }

    #[allow(dead_code)]
    pub fn add_recent_file(&mut self, file: PathBuf) {
        self.record_file(file);
        let _ = self.save();
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

impl WorkspaceState {
    /// Loads workspace state for the given root folder.
    #[allow(dead_code)]
    pub fn load(root: &Path) -> Option<Self> {
        let path = workspace_state_file(root);
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(state) = serde_json::from_str::<WorkspaceState>(&content) {
                return Some(state);
            }
        }
        None
    }

    /// Saves workspace state to its workspaceStorage folder.
    #[allow(dead_code)]
    pub fn save(&self) -> Result<(), std::io::Error> {
        let path = workspace_state_file(&self.root);
        write_json_safe(&path, self)
    }

    /// Deletes stored state for a workspace root.
    #[allow(dead_code)]
    pub fn delete(root: &Path) -> Result<(), std::io::Error> {
        let dir = workspace_storage_dir(root);
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
        Ok(())
    }
}

/// One ordered writer for session metadata and buffer saves. Disk I/O never
/// runs while the UI owns an App borrow. The small metadata cache also makes
/// A -> B -> A restoration independent of the speed of the storage device.
#[derive(Clone)]
pub struct StateStore {
    tx: std::sync::mpsc::Sender<StorageRequest>,
    cache: std::sync::Arc<std::sync::Mutex<StateCache>>,
    global: std::sync::Arc<std::sync::Mutex<GlobalState>>,
    directory: std::sync::Arc<PathBuf>,
}

const SESSION_CACHE_LIMIT: usize = 8;

#[derive(Default)]
struct StateCache {
    recent: std::collections::VecDeque<WorkspaceState>,
    pending: std::collections::HashMap<PathBuf, WorkspaceState>,
}

impl StateCache {
    fn remember(&mut self, state: WorkspaceState) -> bool {
        self.recent.retain(|old| old.root != state.root);
        self.recent.push_front(state.clone());
        self.recent.truncate(SESSION_CACHE_LIMIT);
        // Queue only one wakeup per root; a burst of cursor/tree changes is
        // persisted as its latest snapshot, not hundreds of JSON rewrites.
        self.pending.insert(state.root.clone(), state).is_none()
    }

    fn get(&self, root: &Path) -> Option<WorkspaceState> {
        self.pending
            .get(root)
            .cloned()
            .or_else(|| self.recent.iter().find(|state| state.root == root).cloned())
    }
}

enum StorageRequest {
    Workspace(PathBuf),
    RecentFolder(PathBuf),
    RecentFile(PathBuf),
    WriteFile {
        path: PathBuf,
        text: std::sync::Arc<str>,
        reply: async_channel::Sender<Result<(), String>>,
    },
    Flush(async_channel::Sender<()>),
}

impl StateStore {
    pub fn new() -> (Self, async_channel::Receiver<GlobalState>) {
        Self::for_directory(config_dir())
    }

    pub(crate) fn for_directory(
        directory: PathBuf,
    ) -> (Self, async_channel::Receiver<GlobalState>) {
        let directory = std::sync::Arc::new(directory);
        let worker_directory = directory.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let (ready_tx, ready_rx) = async_channel::bounded(1);
        let cache = std::sync::Arc::new(std::sync::Mutex::new(StateCache::default()));
        let global = std::sync::Arc::new(std::sync::Mutex::new(GlobalState::default()));
        let worker_cache = cache.clone();
        let worker_global = global.clone();
        std::thread::spawn(move || {
            let global_file = worker_directory.join("globalStorage/storage.json");
            let mut global_state: GlobalState = read_json(&global_file).unwrap_or_default();
            *worker_global.lock().unwrap() = global_state.clone();
            let _ = ready_tx.try_send(global_state.clone());
            drop(ready_tx);
            while let Ok(request) = rx.recv() {
                match request {
                    StorageRequest::Workspace(root) => {
                        let state = worker_cache.lock().unwrap().pending.remove(&root);
                        if let Some(state) = state {
                            let file = worker_directory
                                .join("workspaceStorage")
                                .join(workspace_id(&state.root))
                                .join("state.json");
                            if let Err(error) = write_json_safe(&file, &state) {
                                eprintln!(
                                    "[workspace] could not persist {}: {error}",
                                    root.display()
                                );
                            }
                        }
                    }
                    StorageRequest::RecentFolder(path) => {
                        global_state.record_folder(path);
                        *worker_global.lock().unwrap() = global_state.clone();
                        if let Err(error) = write_json_safe(&global_file, &global_state) {
                            eprintln!("[workspace] could not save recent folders: {error}");
                        }
                    }
                    StorageRequest::RecentFile(path) => {
                        global_state.record_file(path);
                        *worker_global.lock().unwrap() = global_state.clone();
                        if let Err(error) = write_json_safe(&global_file, &global_state) {
                            eprintln!("[workspace] could not save recent files: {error}");
                        }
                    }
                    StorageRequest::WriteFile { path, text, reply } => {
                        let result = std::fs::write(&path, text.as_bytes())
                            .map_err(|error| format!("{}: {error}", path.display()));
                        let _ = reply.try_send(result);
                    }
                    StorageRequest::Flush(reply) => {
                        let _ = reply.try_send(());
                    }
                }
            }
        });
        (
            Self {
                tx,
                cache,
                global,
                directory,
            },
            ready_rx,
        )
    }

    /// Must be called on the background executor: a cache miss reads disk.
    pub fn load(&self, root: &Path) -> Option<WorkspaceState> {
        if let Some(state) = self.cache.lock().unwrap().get(root) {
            return Some(state);
        }
        let path = self
            .directory
            .join("workspaceStorage")
            .join(workspace_id(root))
            .join("state.json");
        let from_disk = read_json(&path);
        // A newer UI snapshot may have arrived while the disk read ran.
        self.cache.lock().unwrap().get(root).or(from_disk)
    }

    pub fn save(&self, state: WorkspaceState) {
        let root = state.root.clone();
        if self.cache.lock().unwrap().remember(state) {
            let _ = self.tx.send(StorageRequest::Workspace(root));
        }
    }

    pub fn recent(&self) -> GlobalState {
        self.global.lock().unwrap().clone()
    }

    pub fn add_recent_folder(&self, path: PathBuf) {
        let _ = self.tx.send(StorageRequest::RecentFolder(path));
    }

    pub fn add_recent_file(&self, path: PathBuf) {
        let _ = self.tx.send(StorageRequest::RecentFile(path));
    }

    /// Saves are ordered even across workspace transitions. A late auto-save
    /// must never overwrite a newer explicit save of the same buffer.
    pub fn write_file(
        &self,
        path: PathBuf,
        text: std::sync::Arc<str>,
    ) -> async_channel::Receiver<Result<(), String>> {
        let (reply, rx) = async_channel::bounded(1);
        let _ = self
            .tx
            .send(StorageRequest::WriteFile { path, text, reply });
        rx
    }

    pub fn flush(&self) -> async_channel::Receiver<()> {
        let (reply, rx) = async_channel::bounded(1);
        let _ = self.tx.send(StorageRequest::Flush(reply));
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(root: PathBuf, active_tab: usize) -> WorkspaceState {
        WorkspaceState {
            root,
            tabs: Vec::new(),
            active_tab,
            layout: LayoutState::default(),
            expanded_folders: Vec::new(),
            explorer_selected: None,
        }
    }

    #[test]
    fn session_cache_is_bounded_and_coalesces_snapshots() {
        let mut cache = StateCache::default();
        let root = PathBuf::from("/project/a");
        assert!(cache.remember(snapshot(root.clone(), 1)));
        assert!(!cache.remember(snapshot(root.clone(), 2)));
        assert_eq!(cache.get(&root).unwrap().active_tab, 2);
        for index in 0..20 {
            cache.remember(snapshot(PathBuf::from(format!("/project/{index}")), index));
        }
        assert_eq!(cache.recent.len(), SESSION_CACHE_LIMIT);
        // An evicted but not yet persisted snapshot is still authoritative.
        assert_eq!(cache.get(&root).unwrap().active_tab, 2);
        cache.pending.clear();
        assert!(cache.get(&root).is_none());
    }

    #[test]
    fn storage_serializes_saves_and_flushes_before_shutdown() {
        let fixture = crate::test_support::TempDir::new("ordered-storage");
        let root = fixture.directory("project");
        let (store, ready) = StateStore::for_directory(fixture.directory("config"));
        ready.recv_blocking().unwrap();
        let file = root.join("buffer.txt");
        let first = store.write_file(file.clone(), std::sync::Arc::from("first"));
        let second = store.write_file(file.clone(), std::sync::Arc::from("second"));
        store.save(snapshot(root.clone(), 1));
        store.save(snapshot(root.clone(), 2));
        assert_eq!(store.load(&root).unwrap().active_tab, 2);
        store.flush().recv_blocking().unwrap();
        assert!(first.recv_blocking().unwrap().is_ok());
        assert!(second.recv_blocking().unwrap().is_ok());
        assert_eq!(std::fs::read_to_string(file).unwrap(), "second");
        let path = fixture
            .path()
            .join("config/workspaceStorage")
            .join(workspace_id(&root))
            .join("state.json");
        assert_eq!(read_json::<WorkspaceState>(&path).unwrap().active_tab, 2);
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn buffer_write_failures_are_reported_without_discarding_session_metadata() {
        let fixture = crate::test_support::TempDir::new("failed-storage");
        let (store, ready) = StateStore::for_directory(fixture.directory("config"));
        ready.recv_blocking().unwrap();
        let root = fixture.directory("project");
        store.save(snapshot(root.clone(), 7));
        let result = store.write_file(
            root.join("missing/file.txt"),
            std::sync::Arc::from("unsaved"),
        );
        assert!(result.recv_blocking().unwrap().is_err());
        assert_eq!(store.load(&root).unwrap().active_tab, 7);
    }

    #[test]
    fn test_workspace_id_deterministic() {
        let path1 = PathBuf::from("/path/to/project");
        let path2 = PathBuf::from("/path/to/project");
        let path3 = PathBuf::from("/path/to/other");

        assert_eq!(workspace_id(&path1), workspace_id(&path2));
        assert_ne!(workspace_id(&path1), workspace_id(&path3));
        assert!(workspace_id(&path1).starts_with("project-"));
    }

    #[test]
    fn test_workspace_state_serde_roundtrip() {
        let state = WorkspaceState {
            root: PathBuf::from("/home/user/project"),
            tabs: vec![
                OpenTabState {
                    path: PathBuf::from("/home/user/project/src/main.rs"),
                    preview: false,
                    language_override: Some("rust".to_string()),
                    cursor: Some(CursorPosition {
                        line: 42,
                        character: 10,
                    }),
                },
                OpenTabState {
                    path: PathBuf::from("/home/user/project/README.md"),
                    preview: true,
                    language_override: None,
                    cursor: None,
                },
            ],
            active_tab: 0,
            layout: LayoutState {
                sidebar_width: 280.0,
                terminal_height: 250.0,
                terminal_right_width: 420.0,
                show_sidebar: true,
                show_terminal: true,
                show_terminal_right: true,
                terminal_maximized: false,
                activity: "Search".to_string(),
                explorer_sticky_scroll: true,
            },
            expanded_folders: vec![PathBuf::from("/home/user/project/src")],
            explorer_selected: Some(PathBuf::from("/home/user/project/src/main.rs")),
        };

        let json = serde_json::to_string_pretty(&state).unwrap();
        let deserialized: WorkspaceState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, deserialized);
    }

    #[test]
    fn test_global_state_serde_roundtrip() {
        let mut state = GlobalState::default();
        let folder1 = PathBuf::from("/tmp/folder1");
        let folder2 = PathBuf::from("/tmp/folder2");
        let file1 = PathBuf::from("/tmp/file1.rs");

        state.recent_folders.push(folder1.clone());
        state.recent_folders.push(folder2.clone());
        state.recent_files.push(file1.clone());
        state.last_workspace_root = Some(folder1);

        let json = serde_json::to_string(&state).unwrap();
        let deserialized: GlobalState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, deserialized);
    }

    #[test]
    fn test_partial_json_defaults() {
        let partial_json = r#"{
            "root": "/tmp/test"
        }"#;

        let state: WorkspaceState = serde_json::from_str(partial_json).unwrap();
        assert_eq!(state.root, PathBuf::from("/tmp/test"));
        assert!(state.tabs.is_empty());
        assert_eq!(state.active_tab, 0);
        assert_eq!(state.layout.sidebar_width, 300.0);
        assert_eq!(state.layout.terminal_height, 320.0);
        assert_eq!(state.layout.terminal_right_width, 420.0);
        assert!(state.layout.show_sidebar);
        assert!(!state.layout.show_terminal);
        assert!(!state.layout.show_terminal_right);
        assert_eq!(state.layout.activity, "Explorer");
        assert!(state.expanded_folders.is_empty());
        assert!(state.layout.explorer_sticky_scroll);
        assert!(state.explorer_selected.is_none());
    }

    /// Workspace files written before the right terminal dock existed have no
    /// `terminal_right_width` / `show_terminal_right` keys. They must keep
    /// loading, with the dock's serde defaults filling the gap — a stale
    /// layout file can never brick a workspace.
    #[test]
    fn test_layout_without_right_dock_fields_still_loads() {
        let old_json = r#"{
            "root": "/tmp/legacy",
            "layout": {
                "sidebar_width": 280.0,
                "terminal_height": 250.0,
                "show_sidebar": true,
                "show_terminal": true,
                "terminal_maximized": false,
                "activity": "Explorer"
            }
        }"#;

        let state: WorkspaceState = serde_json::from_str(old_json).unwrap();
        assert_eq!(state.layout.sidebar_width, 280.0);
        assert_eq!(state.layout.terminal_height, 250.0);
        assert!(state.layout.show_terminal);
        // Right-dock defaults, not a parse error.
        assert_eq!(state.layout.terminal_right_width, 420.0);
        assert!(!state.layout.show_terminal_right);
        // Sticky scroll defaults on, exactly like VS Code.
        assert!(state.layout.explorer_sticky_scroll);
    }
}
