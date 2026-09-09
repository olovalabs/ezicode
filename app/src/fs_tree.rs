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

        let is_dir = e.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        entries.push(Entry {
            sort_key: name.to_lowercase(),
            path: e.path(),
            is_dir,
            name,
        });
    }

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

pub fn collapse_all(nodes: &mut [TreeNode]) {
    for node in nodes {
        node.expanded = false;
        collapse_all(&mut node.children);
    }
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
