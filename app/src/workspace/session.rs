//! Background-only workspace preparation and lifecycle-owned explorer watches.
//! A transition publishes a prepared snapshot; no filesystem calls are needed
//! in the UI commit. Watches cover visited directories, not dependency trees.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};

use notify::Watcher as _;

use crate::cancellation::Cancellation;
use crate::fs_tree::{flatten_visible, try_load_dir, TreeNode, VisibleTreeRow};
use crate::storage::{StateStore, WorkspaceState};

pub(super) struct PreparedWorkspace {
    pub root: PathBuf,
    pub tree: Vec<TreeNode>,
    pub rows: Arc<[VisibleTreeRow]>,
    pub saved: Option<WorkspaceState>,
}

pub(super) fn prepare_workspace(
    path: PathBuf,
    store: &StateStore,
    cancellation: &Cancellation,
) -> Result<PreparedWorkspace, String> {
    let _span = crate::perf::span("workspace.prepare.background");
    if cancellation.is_cancelled() {
        return Err("Folder opening cancelled".into());
    }
    let root = std::fs::canonicalize(&path)
        .map_err(|error| format!("Cannot open {}: {error}", path.display()))?;
    let mut tree = try_load_dir(&root, || cancellation.is_cancelled())
        .map_err(|error| format!("Cannot open {}: {error}", path.display()))?;
    let mut saved = store.load(&root);
    if let Some(state) = &mut saved {
        let old_root = state.root.clone();
        let normalize = |path: &PathBuf| {
            path.strip_prefix(&old_root)
                .map(|relative| root.join(relative))
                .unwrap_or_else(|_| path.clone())
        };
        for tab in &mut state.tabs {
            tab.path = normalize(&tab.path);
        }
        state.expanded_folders = state.expanded_folders.iter().map(normalize).collect();
        state.explorer_selected = state.explorer_selected.as_ref().map(normalize);
        state.root = root.clone();
        // Probe saved paths off-thread. Map the active path rather than its
        // old index: deleted tabs must not change which file gets focused.
        let active_path = state.tabs.get(state.active_tab).map(|tab| tab.path.clone());
        state
            .tabs
            .retain(|tab| !cancellation.is_cancelled() && tab.path.is_file());
        state.active_tab = active_path
            .and_then(|path| state.tabs.iter().position(|tab| tab.path == path))
            .unwrap_or(0);
        state.explorer_selected = state.explorer_selected.take().filter(|path| path.exists());
        let mut expanded: HashSet<PathBuf> = state.expanded_folders.iter().cloned().collect();
        if let Some(selected) = &state.explorer_selected {
            let mut parent = selected.parent();
            while let Some(path) = parent.filter(|path| path.starts_with(&root) && *path != root) {
                expanded.insert(path.to_path_buf());
                parent = path.parent();
            }
        }
        expand_saved(&mut tree, &expanded, cancellation)?;
    }
    if cancellation.is_cancelled() {
        return Err("Folder opening cancelled".into());
    }
    let mut rows = Vec::new();
    flatten_visible(&tree, 0, &mut rows);
    Ok(PreparedWorkspace {
        root,
        tree,
        rows: rows.into(),
        saved,
    })
}

fn expand_saved(
    nodes: &mut [TreeNode],
    expanded: &HashSet<PathBuf>,
    cancellation: &Cancellation,
) -> Result<(), String> {
    for node in nodes {
        if cancellation.is_cancelled() {
            return Err("Folder opening cancelled".into());
        }
        if node.is_dir && expanded.contains(&node.path) {
            // A deleted/inaccessible descendant should not prevent opening
            // the rest of the project. Expansion can be retried from the UI.
            if let Ok(children) = try_load_dir(&node.path, || cancellation.is_cancelled()) {
                node.children = children;
                node.children_loaded = true;
                node.expanded = true;
                expand_saved(&mut node.children, expanded, cancellation)?;
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct WatchState {
    watched: HashSet<PathBuf>,
    dirty: HashSet<PathBuf>,
    error: Option<String>,
}

pub(super) struct DirectoryWatcher {
    commands: mpsc::Sender<PathBuf>,
    cancellation: Cancellation,
    state: Arc<Mutex<WatchState>>,
    #[cfg(test)]
    registered: mpsc::Receiver<PathBuf>,
}

impl DirectoryWatcher {
    pub fn new(root: PathBuf, cancellation: Cancellation) -> (Self, async_channel::Receiver<()>) {
        let cancellation = cancellation.child();
        let worker_cancellation = cancellation.clone();
        let (commands, requests) = mpsc::channel::<PathBuf>();
        #[cfg(test)]
        let (registered_tx, registered) = mpsc::channel();
        // The pending paths are a set, and a single signal represents them.
        // An npm/build event storm cannot grow an unbounded message queue.
        let (signal, events) = async_channel::bounded(1);
        let state = Arc::new(Mutex::new(WatchState::default()));
        let worker_state = state.clone();
        let callback_root = root.clone();
        std::thread::spawn(move || {
            let cancellation = worker_cancellation;
            let callback_state = worker_state.clone();
            let callback_signal = signal.clone();
            let callback_cancel = cancellation.clone();
            let watcher =
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    if callback_cancel.is_cancelled() {
                        return;
                    }
                    let mut state = callback_state.lock().unwrap();
                    match event {
                        Ok(event) if !matches!(event.kind, notify::EventKind::Access(_)) => {
                            if matches!(
                                event.kind,
                                notify::EventKind::Modify(
                                    notify::event::ModifyKind::Data(_)
                                        | notify::event::ModifyKind::Metadata(_)
                                )
                            ) && !event.paths.iter().any(|path| {
                                path.file_name().is_some_and(|name| name == ".gitignore")
                            }) {
                                return;
                            }
                            for path in event.paths {
                                if !path.starts_with(&callback_root) {
                                    continue;
                                }
                                // Only already watched directories need a scan.
                                // No stat/syscall is needed to classify events.
                                if state.watched.contains(&path) {
                                    state.dirty.insert(path.clone());
                                }
                                if let Some(parent) = path.parent() {
                                    if state.watched.contains(parent) {
                                        state.dirty.insert(parent.to_path_buf());
                                    }
                                }
                            }
                        }
                        Err(error) => state.error = Some(format!("Filesystem watcher: {error}")),
                        _ => {}
                    }
                    if !state.dirty.is_empty() || state.error.is_some() {
                        let _ = callback_signal.try_send(());
                    }
                });
            let mut watcher = match watcher {
                Ok(watcher) => watcher,
                Err(error) => {
                    worker_state.lock().unwrap().error =
                        Some(format!("Filesystem watcher: {error}"));
                    let _ = signal.try_send(());
                    return;
                }
            };
            // Native watcher creation, registration AND destruction all run
            // on this thread. Dropping the sole command sender stops it.
            for directory in requests {
                if cancellation.is_cancelled() {
                    break;
                }
                if worker_state.lock().unwrap().watched.contains(&directory) {
                    continue;
                }
                match watcher.watch(&directory, notify::RecursiveMode::NonRecursive) {
                    Ok(()) => {
                        worker_state
                            .lock()
                            .unwrap()
                            .watched
                            .insert(directory.clone());
                        #[cfg(test)]
                        let _ = registered_tx.send(directory);
                    }
                    Err(error) => {
                        worker_state.lock().unwrap().error =
                            Some(format!("Filesystem watcher: {error}"));
                        let _ = signal.try_send(());
                    }
                }
            }
        });
        let watcher = Self {
            commands,
            cancellation,
            state,
            #[cfg(test)]
            registered,
        };
        watcher.watch(root);
        (watcher, events)
    }

    pub fn sender(&self) -> mpsc::Sender<PathBuf> {
        self.commands.clone()
    }

    pub fn watch(&self, directory: PathBuf) {
        let _ = self.commands.send(directory);
    }

    pub fn drain(&self) -> (Vec<PathBuf>, Option<String>) {
        let mut state = self.state.lock().unwrap();
        (state.dirty.drain().collect(), state.error.take())
    }
}

impl Drop for DirectoryWatcher {
    fn drop(&mut self) {
        self.cancellation.cancel();
        // An index scan may still own another sender while blocked in OS I/O.
        // Wake the worker anyway; native teardown must not wait for that scan.
        let _ = self.commands.send(PathBuf::new());
    }
}

/// The watcher follows cached levels, including collapsed ones, so reopening
/// a cached directory never displays files deleted while it was collapsed.
pub(super) fn loaded_directories(root: &Path, tree: &[TreeNode]) -> Vec<PathBuf> {
    fn collect(nodes: &[TreeNode], output: &mut Vec<PathBuf>) {
        for node in nodes {
            if node.is_dir && node.children_loaded {
                output.push(node.path.clone());
                collect(&node.children, output);
            }
        }
    }
    let mut directories = vec![root.to_path_buf()];
    collect(tree, &mut directories);
    directories
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{CursorPosition, LayoutState, OpenTabState};
    use crate::test_support::TempDir;
    use std::time::Duration;

    fn store(fixture: &TempDir) -> StateStore {
        let (store, ready) = StateStore::for_directory(fixture.directory("config"));
        ready.recv_blocking().unwrap();
        store
    }

    fn saved(root: &Path, tabs: Vec<OpenTabState>) -> WorkspaceState {
        WorkspaceState {
            root: root.to_path_buf(),
            tabs,
            active_tab: 0,
            layout: LayoutState::default(),
            expanded_folders: Vec::new(),
            explorer_selected: None,
        }
    }

    fn tab(path: PathBuf) -> OpenTabState {
        OpenTabState {
            path,
            preview: false,
            language_override: Some("text".into()),
            cursor: Some(CursorPosition {
                line: 5,
                character: 3,
            }),
        }
    }

    #[test]
    #[ignore = "31,000-file native watcher benchmark; run explicitly with --ignored --nocapture"]
    fn benchmark_native_workspace_loading() {
        use std::time::Instant;
        let fixture = TempDir::new("native-workspace-benchmark");
        let root = fixture.directory("project");
        for (directory, count, files) in [
            ("src", 100, 80),
            ("node_modules", 2000, 10),
            ("target", 1000, 3),
        ] {
            for index in 0..count {
                for file in 0..files {
                    fixture.file(
                        &format!("project/{directory}/{index}/file-{file}.txt"),
                        "sample\n",
                    );
                }
            }
        }
        let store = store(&fixture);
        let mut recursive_setup = Vec::new();
        let mut recursive_drop = Vec::new();
        let mut sparse_setup = Vec::new();
        let mut sparse_ui_drop = Vec::new();
        let mut prepare = Vec::new();
        for _ in 0..5 {
            let started = Instant::now();
            let mut old =
                notify::recommended_watcher(|_: notify::Result<notify::Event>| {}).unwrap();
            old.watch(&root, notify::RecursiveMode::Recursive).unwrap();
            recursive_setup.push(started.elapsed().as_secs_f64() * 1000.0);
            let started = Instant::now();
            drop(old);
            recursive_drop.push(started.elapsed().as_secs_f64() * 1000.0);
            let started = Instant::now();
            let (watcher, _events) = DirectoryWatcher::new(root.clone(), Cancellation::default());
            watcher
                .registered
                .recv_timeout(Duration::from_secs(30))
                .unwrap();
            sparse_setup.push(started.elapsed().as_secs_f64() * 1000.0);
            let started = Instant::now();
            drop(watcher);
            sparse_ui_drop.push(started.elapsed().as_secs_f64() * 1000.0);
            let started = Instant::now();
            let prepared =
                prepare_workspace(root.clone(), &store, &Cancellation::default()).unwrap();
            assert_eq!(prepared.rows.len(), 1);
            prepare.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        println!(
            "NATIVE_WORKSPACE_BENCHMARK {}",
            serde_json::json!({
                "files": 31000, "directories": 3104, "iterations": 5,
                "recursive_setup_ms": recursive_setup, "recursive_drop_ms": recursive_drop,
                "sparse_setup_ms": sparse_setup, "sparse_ui_drop_ms": sparse_ui_drop,
                "background_prepare_ms": prepare,
            })
        );
    }

    #[test]
    fn large_projects_prepare_only_root_entries_without_reading_saved_buffers() {
        let fixture = TempDir::new("large-session");
        let root = fixture.directory("project");
        for index in 0..3000 {
            fixture.file(&format!("project/src/{index}.txt"), "text");
        }
        fixture.file("project/node_modules/dependency/index.js", "ignored");
        let oversized = root.join("oversized.txt");
        std::fs::File::create(&oversized)
            .unwrap()
            .set_len(1_000_000_000)
            .unwrap();
        let store = store(&fixture);
        store.save(saved(&root, vec![tab(oversized.clone())]));
        let prepared = prepare_workspace(root.clone(), &store, &Cancellation::default()).unwrap();
        assert_eq!(prepared.rows.len(), 2);
        assert!(prepared.tree.iter().all(|node| !node.children_loaded));
        // If preparation read restored buffers, this file would be rejected
        // (or allocate a gigabyte). Its metadata remains a lazy tab instead.
        assert_eq!(prepared.saved.unwrap().tabs[0].path, oversized);
    }

    #[test]
    fn expanded_folders_and_selected_ancestors_are_prepared_off_thread() {
        let fixture = TempDir::new("expanded-session");
        let root = fixture.directory("project");
        let selected = fixture.file("project/src/nested/file.txt", "text");
        let store = store(&fixture);
        let mut state = saved(&root, vec![tab(selected.clone())]);
        state.explorer_selected = Some(selected.clone());
        store.save(state);
        let prepared = prepare_workspace(root.clone(), &store, &Cancellation::default()).unwrap();
        assert!(prepared.rows.iter().any(|row| row.path == selected));
        assert_eq!(loaded_directories(&root, &prepared.tree).len(), 3);
        assert_eq!(
            prepared.saved.unwrap().tabs[0]
                .cursor
                .as_ref()
                .unwrap()
                .line,
            5
        );
    }

    #[test]
    fn deleted_tabs_do_not_shift_the_saved_active_file() {
        let fixture = TempDir::new("deleted-tabs");
        let root = fixture.directory("project");
        let current = fixture.file("project/current.txt", "text");
        let store = store(&fixture);
        let mut state = saved(
            &root,
            vec![tab(root.join("deleted.txt")), tab(current.clone())],
        );
        state.active_tab = 1;
        state.explorer_selected = Some(root.join("deleted"));
        store.save(state);
        let state = prepare_workspace(root, &store, &Cancellation::default())
            .unwrap()
            .saved
            .unwrap();
        assert_eq!(state.tabs.len(), 1);
        assert_eq!(state.tabs[state.active_tab].path, current);
        assert!(state.explorer_selected.is_none());
    }

    #[test]
    fn missing_invalid_and_cancelled_folders_are_rejected() {
        let fixture = TempDir::new("invalid-session");
        let store = store(&fixture);
        let file = fixture.file("file.txt", "not a folder");
        assert!(prepare_workspace(file, &store, &Cancellation::default()).is_err());
        assert!(prepare_workspace(
            fixture.path().join("deleted"),
            &store,
            &Cancellation::default()
        )
        .is_err());
        let cancelled = Cancellation::default();
        cancelled.cancel();
        assert_eq!(
            prepare_workspace(fixture.path().to_path_buf(), &store, &cancelled)
                .err()
                .unwrap(),
            "Folder opening cancelled"
        );
    }

    #[cfg(unix)]
    #[test]
    fn inaccessible_folders_are_rejected_without_publishing_an_empty_tree() {
        use std::os::unix::fs::PermissionsExt as _;
        let fixture = TempDir::new("inaccessible-session");
        let root = fixture.directory("project");
        let store = store(&fixture);
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root can read mode 000. Normal desktop/CI users exercise this branch.
        if std::fs::read_dir(&root).is_err() {
            assert!(prepare_workspace(root.clone(), &store, &Cancellation::default()).is_err());
        }
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn legacy_symlink_paths_restore_into_the_canonical_workspace() {
        let fixture = TempDir::new("alias-session");
        let root = fixture.directory("project");
        let file = fixture.file("project/src/file.txt", "text");
        let alias = fixture.path().join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let store = store(&fixture);
        let mut state = saved(&alias, vec![tab(alias.join("src/file.txt"))]);
        state.explorer_selected = Some(alias.join("src/file.txt"));
        store.save(state);
        store.flush().recv_blocking().unwrap();
        let prepared = prepare_workspace(root.clone(), &store, &Cancellation::default()).unwrap();
        assert_eq!(prepared.root, root);
        assert_eq!(prepared.saved.unwrap().tabs[0].path, file);
        assert!(prepared.rows.iter().any(|row| row.path == file));
    }

    #[test]
    fn watcher_shutdown_does_not_wait_for_an_obsolete_index_sender() {
        let fixture = TempDir::new("watcher-shutdown");
        let (watcher, events) =
            DirectoryWatcher::new(fixture.path().to_path_buf(), Cancellation::default());
        watcher
            .registered
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        let obsolete_scan = watcher.sender();
        drop(watcher);
        let (done, completed) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(events.recv_blocking().is_err());
        });
        assert!(completed.recv_timeout(Duration::from_secs(3)).unwrap());
        drop(obsolete_scan);
    }

    #[test]
    fn watcher_registration_is_lazy_and_deduplicated() {
        let fixture = TempDir::new("lazy-watcher");
        let root = fixture.directory("project");
        let child = fixture.directory("project/src");
        fixture.directory("project/node_modules/dependency/nested");
        let token = Cancellation::default();
        let (watcher, _events) = DirectoryWatcher::new(root.clone(), token.clone());
        assert_eq!(
            watcher
                .registered
                .recv_timeout(Duration::from_secs(3))
                .unwrap(),
            root
        );
        assert_eq!(watcher.state.lock().unwrap().watched.len(), 1);
        watcher.watch(child.clone());
        watcher.watch(child.clone());
        assert_eq!(
            watcher
                .registered
                .recv_timeout(Duration::from_secs(3))
                .unwrap(),
            child
        );
        assert_eq!(watcher.state.lock().unwrap().watched.len(), 2);
        token.cancel();
        drop(watcher);
    }
}
