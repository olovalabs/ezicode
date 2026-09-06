//! The explorer's in-memory snapshot of the opened folder. Directory levels
//! are loaded lazily as the user expands nodes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A node of the explorer tree; children are loaded on first expand.
#[derive(Clone, Debug)]
pub struct TreeNode {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub expanded: bool,
    /// Distinguishes an empty directory from a directory that has not been
    /// opened yet. Without this bit, expanding an empty folder performed a
    /// new directory scan every time it was collapsed and expanded.
    pub children_loaded: bool,
    pub children: Vec<TreeNode>,
}

/// The small, render-ready representation of one visible row.
///
/// Keeping this separate from [`TreeNode`] is important: the UI should not
/// walk a recursive tree or borrow the whole snapshot just to render a single
/// virtualized row. These rows are rebuilt only when the tree structure or
/// expansion state changes, not on every paint.
#[derive(Clone, Debug)]
pub struct VisibleTreeRow {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub expanded: bool,
    pub depth: usize,
}

/// Directories / hidden system files never shown in the explorer by default.
const SKIP: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    ".git",
    ".DS_Store",
    "Thumbs.db",
];

/// One directory entry with every field the sort needs, precomputed once.
///
/// The old implementation called `Path::is_dir()` (a syscall) *inside the
/// sort comparator* — O(n·log n) syscalls — plus allocated two lowercase
/// `String`s per comparison. Materializing the entries once keeps the total
/// probe count at O(n) and makes the comparator allocation-free.
struct Entry {
    name: String,
    /// Case-insensitive sort key: the lowercased name, computed once.
    sort_key: String,
    path: PathBuf,
    is_dir: bool,
}

/// Read one directory level: dirs first, then files, both case-insensitively alphabetically.
pub fn load_dir(dir: &Path) -> Vec<TreeNode> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut entries: Vec<Entry> = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if SKIP.iter().any(|s| *s == name) {
            continue;
        }
        // `DirEntry::file_type` is served by the directory's `d_type` field
        // on most platforms (no syscall); it only falls back to `stat()`
        // where the OS leaves the type unknown. Crucially it runs once per
        // entry, never per sort comparison.
        let is_dir = e.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        entries.push(Entry {
            sort_key: name.to_lowercase(),
            path: e.path(),
            is_dir,
            name,
        });
    }

    // Dirs first, then files; each group case-insensitively alphabetical.
    // The comparator only reads precomputed fields — no filesystem probes,
    // no per-comparison allocations.
    entries.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            return b.is_dir.cmp(&a.is_dir);
        }
        a.sort_key.cmp(&b.sort_key)
    });

    for e in entries {
        out.push(TreeNode {
            name: e.name,
            path: e.path,
            is_dir: e.is_dir,
            expanded: false,
            children_loaded: false,
            children: Vec::new(),
        });
    }
    out
}

/// Reload one directory level while retaining all already-loaded descendants.
///
/// This is deliberately shallow. The previous implementation recursively
/// rescanned every expanded descendant when the root was refreshed, turning a
/// small edit in a large workspace into an O(visible-tree) burst of blocking
/// filesystem work. A watcher only needs the changed directory level. Existing
/// child snapshots remain valid until that directory is explicitly reloaded.
///
/// `previous` is consumed so unchanged child vectors move into the new
/// snapshot instead of being cloned.
#[allow(dead_code)]
pub fn reload_dir_preserving_shallow(dir: &Path, previous: Vec<TreeNode>) -> Vec<TreeNode> {
    merge_loaded_dir(dir, previous, load_dir(dir))
}

/// Merges an already-scanned directory into the previous snapshot.
///
/// Keeping scanning and merging separate lets the filesystem watcher perform
/// the blocking `read_dir` work on its background executor and keeps the UI
/// thread responsible only for a short in-memory state merge.
pub fn merge_loaded_dir(
    _dir: &Path,
    previous: Vec<TreeNode>,
    mut current: Vec<TreeNode>,
) -> Vec<TreeNode> {
    let mut previous_by_path: HashMap<PathBuf, TreeNode> = previous
        .into_iter()
        .map(|node| (node.path.clone(), node))
        .collect();

    for node in &mut current {
        if let Some(prev) = previous_by_path.remove(&node.path) {
            if node.is_dir {
                node.expanded = prev.expanded;
                node.children_loaded = prev.children_loaded;
                node.children = prev.children;
            }
        }
    }
    current
}

/// Reloads a directory from disk while preserving the expanded state of
/// subdirectories.
///
/// Kept as a compatibility helper for callers that need the old eager,
/// recursive behavior. Explorer refreshes and filesystem notifications use
/// [`reload_dir_preserving_shallow`] so they do not rescan unrelated folders.
#[allow(dead_code)]
pub fn reload_dir_preserving(dir: &Path, prev_nodes: &[TreeNode]) -> Vec<TreeNode> {
    let mut previous_by_path: HashMap<&Path, &TreeNode> = HashMap::with_capacity(prev_nodes.len());
    for prev in prev_nodes {
        previous_by_path.insert(prev.path.as_path(), prev);
    }

    let mut current = load_dir(dir);
    for node in &mut current {
        if node.is_dir {
            if let Some(prev) = previous_by_path.get(node.path.as_path()) {
                if prev.expanded {
                    node.expanded = true;
                    node.children_loaded = true;
                    node.children = reload_dir_preserving(&node.path, &prev.children);
                } else {
                    node.children_loaded = prev.children_loaded;
                    node.children = prev.children.clone();
                }
            }
        }
    }
    current
}

/// Appends the currently visible part of a tree to a flat row buffer.
pub fn flatten_visible(nodes: &[TreeNode], depth: usize, out: &mut Vec<VisibleTreeRow>) {
    for node in nodes {
        out.push(VisibleTreeRow {
            name: node.name.clone(),
            path: node.path.clone(),
            is_dir: node.is_dir,
            expanded: node.expanded,
            depth,
        });
        if node.is_dir && node.expanded {
            flatten_visible(&node.children, depth + 1, out);
        }
    }
}

/// Recursively collapses all open folders.
pub fn collapse_all(nodes: &mut [TreeNode]) {
    for node in nodes {
        node.expanded = false;
        collapse_all(&mut node.children);
    }
}

/// Last path component ("src" for C:\...\project\src).
pub fn display_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

/// Validate a name entered in the explorer's create/rename editor.
///
/// The filesystem APIs accept absolute paths and `..` components, but the
/// explorer must never let a filename field escape the workspace or silently
/// address a sibling directory. Separators are intentionally rejected here;
/// creating nested directories is done by choosing the destination folder (or
/// by an explicit move), just like the native VS Code explorer.
pub fn valid_entry_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !Path::new(name).is_absolute()
        && !name.chars().any(|ch| ch == '\0' || ch.is_control())
}

/// Return whether `candidate` is the same path as, or inside, `parent`.
///
/// `Path::starts_with` is component-aware (unlike string prefix checks), so
/// `src2` cannot accidentally be treated as a child of `src`.
pub fn is_same_or_descendant(parent: &Path, candidate: &Path) -> bool {
    candidate == parent || candidate.starts_with(parent)
}

/// Rewrite a path after moving `source` to `destination` while preserving
/// descendants. This is used to keep open tabs and selections valid when a
/// directory is moved as a unit.
pub fn path_after_move(path: &Path, source: &Path, destination: &Path) -> Option<PathBuf> {
    if path == source {
        return Some(destination.to_path_buf());
    }
    path.strip_prefix(source)
        .ok()
        .map(|suffix| destination.join(suffix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shallow_merge_preserves_loaded_descendants() {
        let previous = vec![TreeNode {
            name: "src".into(),
            path: PathBuf::from("/tmp/src"),
            is_dir: true,
            expanded: true,
            children_loaded: true,
            children: vec![TreeNode {
                name: "main.rs".into(),
                path: PathBuf::from("/tmp/src/main.rs"),
                is_dir: false,
                expanded: false,
                children_loaded: false,
                children: Vec::new(),
            }],
        }];
        let current = vec![TreeNode {
            name: "src".into(),
            path: PathBuf::from("/tmp/src"),
            is_dir: true,
            expanded: false,
            children_loaded: false,
            children: Vec::new(),
        }];

        let merged = merge_loaded_dir(Path::new("/tmp"), previous, current);
        assert!(merged[0].expanded);
        assert!(merged[0].children_loaded);
        assert_eq!(merged[0].children[0].name, "main.rs");
    }

    #[test]
    fn entry_names_cannot_escape_the_workspace() {
        assert!(valid_entry_name("main.rs"));
        assert!(valid_entry_name(".env"));
        assert!(!valid_entry_name(""));
        assert!(!valid_entry_name("."));
        assert!(!valid_entry_name(".."));
        assert!(!valid_entry_name("../outside"));
        assert!(!valid_entry_name("nested/file.rs"));
        assert!(!valid_entry_name("nested\\\\file.rs"));
    }

    #[test]
    fn move_rewrites_only_path_components() {
        let source = Path::new("/project/src");
        let destination = Path::new("/project/lib");
        assert_eq!(
            path_after_move(Path::new("/project/src/main.rs"), source, destination),
            Some(PathBuf::from("/project/lib/main.rs"))
        );
        assert_eq!(path_after_move(Path::new("/project/src2/main.rs"), source, destination), None);
    }

    #[test]
    fn flatten_visible_only_includes_expanded_children() {
        let root = TreeNode {
            name: "src".into(),
            path: PathBuf::from("/tmp/src"),
            is_dir: true,
            expanded: false,
            children_loaded: true,
            children: vec![TreeNode {
                name: "main.rs".into(),
                path: PathBuf::from("/tmp/src/main.rs"),
                is_dir: false,
                expanded: false,
                children_loaded: false,
                children: Vec::new(),
            }],
        };
        let mut rows = Vec::new();
        flatten_visible(&[root.clone()], 0, &mut rows);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].depth, 0);

        let mut expanded = root;
        expanded.expanded = true;
        rows.clear();
        flatten_visible(&[expanded], 0, &mut rows);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].depth, 1);
    }
}
