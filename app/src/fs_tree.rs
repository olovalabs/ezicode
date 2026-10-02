use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct TreeNode {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub expanded: bool,

    pub children_loaded: bool,
    pub children: Vec<TreeNode>,
}

#[derive(Clone, Debug)]
pub struct VisibleTreeRow {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub expanded: bool,
    pub depth: usize,
}

const SKIP: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    ".git",
    ".DS_Store",
    "Thumbs.db",
];

struct Entry {
    name: String,

    sort_key: String,
    path: PathBuf,
    is_dir: bool,
}

pub fn load_dir(dir: &Path) -> Vec<TreeNode> {
    try_load_dir(dir, || false).unwrap_or_default()
}

/// Read a single level. Cancellation is checked between entries so obsolete
/// scans of very wide directories release their memory promptly.
pub fn try_load_dir(dir: &Path, cancelled: impl Fn() -> bool) -> std::io::Result<Vec<TreeNode>> {
    let _span = crate::perf::span("workspace.read_directory.background");
    if cancelled() {
        return Err(std::io::ErrorKind::Interrupted.into());
    }
    let rd = std::fs::read_dir(dir)?;
    let mut entries: Vec<Entry> = Vec::new();
    for entry in rd {
        if cancelled() {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if SKIP.iter().any(|skip| *skip == name) {
            continue;
        }
        crate::perf::note_stat();
        let is_dir = entry.file_type()?.is_dir();
        entries.push(Entry {
            sort_key: name.to_lowercase(),
            path: entry.path(),
            is_dir,
            name,
        });
    }
    if cancelled() {
        return Err(std::io::ErrorKind::Interrupted.into());
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.sort_key.cmp(&b.sort_key))
    });
    Ok(entries
        .into_iter()
        .map(|entry| TreeNode {
            name: entry.name,
            path: entry.path,
            is_dir: entry.is_dir,
            expanded: false,
            children_loaded: false,
            children: Vec::new(),
        })
        .collect())
}

pub fn entries_match(previous: &[TreeNode], current: &[TreeNode]) -> bool {
    previous.len() == current.len()
        && previous.iter().zip(current).all(|(old, new)| {
            old.path == new.path && old.name == new.name && old.is_dir == new.is_dir
        })
}

#[allow(dead_code)]
pub fn reload_dir_preserving_shallow(dir: &Path, previous: Vec<TreeNode>) -> Vec<TreeNode> {
    merge_loaded_dir(dir, previous, load_dir(dir))
}

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

/// Last row index belonging to the subtree rooted at `index` (the row itself
/// when it is a file or a collapsed/empty folder). Used by sticky scroll to
/// know where a folder's section ends, exactly like VS Code's
/// `getLastDescendant`.
pub fn subtree_end(rows: &[VisibleTreeRow], index: usize) -> usize {
    let Some(start) = rows.get(index) else {
        return index;
    };
    let depth = start.depth;
    let mut end = index;
    for (ix, row) in rows.iter().enumerate().skip(index + 1) {
        if row.depth <= depth {
            break;
        }
        end = ix;
    }
    end
}

/// Indices of every rendered ancestor of `index`, ordered root-first.
pub fn ancestor_chain(rows: &[VisibleTreeRow], index: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let Some(row) = rows.get(index) else {
        return out;
    };
    let mut depth = row.depth;
    let mut ix = index;
    while depth > 0 && ix > 0 {
        ix -= 1;
        if rows[ix].depth < depth {
            depth = rows[ix].depth;
            out.push(ix);
        }
    }
    out.reverse();
    out
}

/// The sticky header stack painted over the top of the tree: the ancestor
/// folders of the first row under the widget, plus the distance the innermost
/// one has already drifted out of view.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StickyLayout {
    /// Row indices to pin, root-first.
    pub rows: Vec<usize>,
    /// Pixels the *last* pinned row is nudged upwards, so it slides behind the
    /// rows above it as its section scrolls past instead of popping out of
    /// existence. Only the innermost row moves — Zed's project panel does the
    /// same, and keeping the outer rows still is what makes the widget look
    /// calm while scrolling.
    pub shift: f32,
}

/// Compute the sticky header stack for a scroll position.
///
/// `scroll_top` is the distance in pixels between the top of the content and
/// the top of the viewport. A folder is pinned only while its own row is
/// scrolled behind the widget, and the stack is capped at `max_rows` lines
/// (VS Code's `workbench.tree.stickyScrollMaxItemCount`, default 7).
pub fn sticky_layout(
    rows: &[VisibleTreeRow],
    scroll_top: f32,
    row_height: f32,
    max_rows: usize,
) -> StickyLayout {
    let mut layout = StickyLayout::default();
    if rows.is_empty() || row_height <= 0.0 || max_rows == 0 || scroll_top <= 0.0 {
        return layout;
    }

    // Pinned rows are the ancestors of the row sitting at the top of the
    // viewport. A taller widget can uncover deeper rows, so the stack is then
    // refined downwards — but only ever by *growing* it, which keeps the
    // result stable (no flicker) while scrolling.
    let candidates = |probe: usize, limit: usize| -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        for ix in ancestor_chain(rows, probe) {
            if out.len() >= limit {
                break;
            }
            // Only pin ancestors whose real row is already hidden behind the
            // widget; a parent still visible on its own needs no sticky copy.
            let row_top = ix as f32 * row_height;
            let slot_top = scroll_top + out.len() as f32 * row_height;
            if row_top < slot_top - 0.01 {
                out.push(ix);
            } else {
                break;
            }
        }
        out
    };

    let top_row = ((scroll_top / row_height).floor() as usize).min(rows.len() - 1);
    let mut pinned = candidates(top_row, max_rows);
    for _ in 0..max_rows {
        if pinned.len() >= max_rows {
            break;
        }
        let probe_y = scroll_top + pinned.len() as f32 * row_height;
        let probe = ((probe_y / row_height).floor() as usize).min(rows.len() - 1);
        let next = candidates(probe, max_rows);
        if next.len() > pinned.len() && next.starts_with(&pinned) {
            pinned = next;
        } else {
            break;
        }
    }

    if pinned.is_empty() {
        return layout;
    }

    // Drift the innermost row up while its section runs out, so the outgoing
    // folder is pushed away by the next one instead of blinking.
    let last_slot = pinned.len() - 1;
    let last = pinned[last_slot];
    let section_bottom = (subtree_end(rows, last) + 1) as f32 * row_height - scroll_top;
    let slot_bottom = (last_slot + 1) as f32 * row_height;
    if section_bottom < slot_bottom {
        layout.shift = (slot_bottom - section_bottom).clamp(0.0, row_height);
    }
    layout.rows = pinned;
    layout
}

/// Next row whose name starts with `query`, searching forward from `start` and
/// wrapping around — VS Code's list type-ahead.
pub fn type_ahead_index(rows: &[VisibleTreeRow], start: usize, query: &str) -> Option<usize> {
    if rows.is_empty() || query.is_empty() {
        return None;
    }
    let needle = query.to_lowercase();
    let len = rows.len();
    for offset in 0..len {
        let ix = (start + offset) % len;
        if rows[ix].name.to_lowercase().starts_with(&needle) {
            return Some(ix);
        }
    }
    None
}

pub fn collapse_all(nodes: &mut [TreeNode]) {
    for node in nodes {
        node.expanded = false;
        collapse_all(&mut node.children);
    }
}

/// Expand or collapse every directory in `nodes` (Alt+click on a twistie in
/// VS Code). Directories whose children were never read are reported through
/// `needs_load` so the caller can fetch them off the UI thread.
pub fn set_expanded_recursive(
    nodes: &mut [TreeNode],
    expanded: bool,
    needs_load: &mut Vec<PathBuf>,
) {
    for node in nodes {
        if !node.is_dir {
            continue;
        }
        node.expanded = expanded;
        if expanded && !node.children_loaded {
            needs_load.push(node.path.clone());
        }
        set_expanded_recursive(&mut node.children, expanded, needs_load);
    }
}

/// Run `f` on the node at `path`, walking only the branch that can contain it.
pub fn with_node_mut<R>(
    nodes: &mut [TreeNode],
    path: &Path,
    f: &mut dyn FnMut(&mut TreeNode) -> R,
) -> Option<R> {
    for node in nodes {
        if node.path == path {
            return Some(f(node));
        }
        if node.is_dir && path.starts_with(&node.path) {
            if let Some(result) = with_node_mut(&mut node.children, path, &mut *f) {
                return Some(result);
            }
        }
    }
    None
}

pub fn display_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

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

pub fn is_same_or_descendant(parent: &Path, candidate: &Path) -> bool {
    candidate == parent || candidate.starts_with(parent)
}

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
    fn directory_reads_are_shallow_sorted_and_skip_dependency_trees() {
        let fixture = crate::test_support::TempDir::new("lazy-tree");
        fixture.file("node_modules/dependency/deep.js", "ignored");
        fixture.file("target/debug/deep.o", "ignored");
        fixture.file("src/nested/main.rs", "fn main() {}");
        fixture.file("z.txt", "z");
        fixture.file("A.txt", "a");
        let nodes = try_load_dir(fixture.path(), || false).unwrap();
        assert_eq!(
            nodes
                .iter()
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>(),
            ["src", "A.txt", "z.txt"]
        );
        assert!(nodes
            .iter()
            .all(|node| !node.children_loaded && node.children.is_empty()));
    }

    #[test]
    fn invalid_directories_are_errors_not_empty_successful_workspaces() {
        let fixture = crate::test_support::TempDir::new("invalid-tree");
        let file = fixture.file("not-a-directory", "text");
        assert!(try_load_dir(&file, || false).is_err());
        assert!(try_load_dir(&fixture.path().join("missing"), || false).is_err());
    }

    #[test]
    fn cancelled_wide_scans_stop_between_entries() {
        let fixture = crate::test_support::TempDir::new("wide-tree");
        for index in 0..2000 {
            fixture.file(&format!("{index}.txt"), "text");
        }
        let probes = std::cell::Cell::new(0);
        let error = try_load_dir(fixture.path(), || {
            let count = probes.get() + 1;
            probes.set(count);
            count > 10
        })
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(probes.get(), 11);
        assert_eq!(
            try_load_dir(&fixture.path().join("missing"), || true)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::Interrupted
        );
    }

    #[test]
    fn unchanged_snapshots_reuse_expanded_tree_state() {
        let mut previous = vec![TreeNode {
            name: "src".into(),
            path: PathBuf::from("src"),
            is_dir: true,
            expanded: true,
            children_loaded: true,
            children: Vec::new(),
        }];
        previous[0].children_loaded = true;
        let mut next = previous.clone();
        next[0].expanded = false;
        next[0].children.clear();
        assert!(entries_match(&previous, &next));
        next[0].is_dir = false;
        assert!(!entries_match(&previous, &next));
    }

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
        assert_eq!(
            path_after_move(Path::new("/project/src2/main.rs"), source, destination),
            None
        );
    }

    fn row(name: &str, depth: usize, is_dir: bool) -> VisibleTreeRow {
        VisibleTreeRow {
            name: name.to_string(),
            path: PathBuf::from(format!("/p/{name}")),
            is_dir,
            expanded: is_dir,
            depth,
        }
    }

    /// src/            0
    ///   ui/           1
    ///     a.rs        2
    ///     b.rs        2
    ///   main.rs       1
    /// README.md       0
    fn sample_rows() -> Vec<VisibleTreeRow> {
        vec![
            row("src", 0, true),
            row("ui", 1, true),
            row("a.rs", 2, false),
            row("b.rs", 2, false),
            row("main.rs", 1, false),
            row("README.md", 0, false),
        ]
    }

    #[test]
    fn subtree_end_covers_all_descendants() {
        let rows = sample_rows();
        assert_eq!(subtree_end(&rows, 0), 4);
        assert_eq!(subtree_end(&rows, 1), 3);
        assert_eq!(subtree_end(&rows, 2), 2);
        assert_eq!(subtree_end(&rows, 5), 5);
    }

    #[test]
    fn ancestors_are_reported_root_first() {
        let rows = sample_rows();
        assert_eq!(ancestor_chain(&rows, 3), vec![0, 1]);
        assert_eq!(ancestor_chain(&rows, 4), vec![0]);
        assert!(ancestor_chain(&rows, 0).is_empty());
    }

    #[test]
    fn nothing_sticks_at_the_top_of_the_list() {
        let rows = sample_rows();
        assert!(sticky_layout(&rows, 0.0, 22.0, 7).rows.is_empty());
    }

    #[test]
    fn scrolled_rows_pin_their_parent_chain() {
        let rows = sample_rows();
        // Scrolled so that `a.rs` (index 2) is the first visible row: both
        // `src` and `ui` are hidden above, so both pin.
        let layout = sticky_layout(&rows, 44.0, 22.0, 7);
        assert_eq!(layout.rows, vec![0, 1]);
        assert_eq!(layout.shift, 0.0);
    }

    #[test]
    fn the_stack_grows_with_the_widget_like_vs_code() {
        let rows = sample_rows();
        // `src` scrolled out, so it pins; the widget then covers `ui`, whose
        // child is the first row below it, so `ui` pins too — exactly how
        // VS Code appends sticky rows one at a time.
        let layout = sticky_layout(&rows, 22.0, 22.0, 7);
        assert_eq!(layout.rows, vec![0, 1]);

        // A file at the root level has no parents to pin.
        let flat = vec![row("a.rs", 0, false), row("b.rs", 0, false)];
        assert!(sticky_layout(&flat, 22.0, 22.0, 7).rows.is_empty());
    }

    #[test]
    fn the_stack_is_capped_and_slides_out_with_its_section() {
        let rows = sample_rows();
        let capped = sticky_layout(&rows, 44.0, 22.0, 1);
        assert_eq!(capped.rows, vec![0]);

        // Half a row past the end of the `ui` section: the `ui` header is
        // being pushed up out of the widget rather than vanishing.
        let sliding = sticky_layout(&rows, 44.0 + 11.0, 22.0, 7);
        assert_eq!(sliding.rows, vec![0, 1]);
        assert!(sliding.shift > 0.0 && sliding.shift <= 22.0);
    }

    #[test]
    fn type_ahead_wraps_around_and_ignores_case() {
        let rows = sample_rows();
        assert_eq!(type_ahead_index(&rows, 0, "re"), Some(5));
        assert_eq!(type_ahead_index(&rows, 3, "SRC"), Some(0));
        assert_eq!(type_ahead_index(&rows, 0, "zz"), None);
    }

    #[test]
    fn recursive_expand_reports_unloaded_directories() {
        let mut nodes = vec![TreeNode {
            name: "src".into(),
            path: PathBuf::from("/tmp/src"),
            is_dir: true,
            expanded: false,
            children_loaded: true,
            children: vec![TreeNode {
                name: "ui".into(),
                path: PathBuf::from("/tmp/src/ui"),
                is_dir: true,
                expanded: false,
                children_loaded: false,
                children: Vec::new(),
            }],
        }];
        let mut needs_load = Vec::new();
        set_expanded_recursive(&mut nodes, true, &mut needs_load);
        assert!(nodes[0].expanded);
        assert!(nodes[0].children[0].expanded);
        assert_eq!(needs_load, vec![PathBuf::from("/tmp/src/ui")]);

        needs_load.clear();
        set_expanded_recursive(&mut nodes, false, &mut needs_load);
        assert!(!nodes[0].expanded);
        assert!(!nodes[0].children[0].expanded);
        assert!(needs_load.is_empty());
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
        flatten_visible(std::slice::from_ref(&root), 0, &mut rows);
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
