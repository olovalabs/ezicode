//! File-watcher helpers matching Zed's `workspace/didChangeWatchedFiles` path.
//!
//! Language servers (rust-analyzer, typescript-language-server, …) dynamically
//! register glob watchers via `client/registerCapability`. Zed then forwards
//! disk events as LSP `FileEvent`s. Without that, the server thinks the
//! workspace is frozen.

use std::path::Path;

/// LSP / gitignore-style glob. `**` matches any path segment sequence, `*`
/// matches anything except `/`, `?` matches one character except `/`.
pub fn glob_matches(pattern: &str, path: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    let path = path.replace('\\', "/");
    glob_match_bytes(pattern.as_bytes(), path.as_bytes())
}

fn glob_match_bytes(pat: &[u8], text: &[u8]) -> bool {
    let mut pi = 0;
    let mut ti = 0;
    while pi < pat.len() {
        if pi + 1 < pat.len() && pat[pi] == b'*' && pat[pi + 1] == b'*' {
            let rest_index = if pi + 2 < pat.len() && pat[pi + 2] == b'/' {
                pi + 3
            } else {
                pi + 2
            };
            let rest = &pat[rest_index..];
            if rest.is_empty() {
                return true;
            }
            let mut t = ti;
            loop {
                if glob_match_bytes(rest, &text[t..]) {
                    return true;
                }
                if t >= text.len() {
                    return false;
                }
                t += 1;
            }
        } else if pat[pi] == b'*' {
            pi += 1;
            if pi == pat.len() {
                return !text[ti..].contains(&b'/');
            }
            let mut t = ti;
            loop {
                if glob_match_bytes(&pat[pi..], &text[t..]) {
                    return true;
                }
                if t >= text.len() || text[t] == b'/' {
                    return false;
                }
                t += 1;
            }
        } else if pat[pi] == b'?' {
            if ti >= text.len() || text[ti] == b'/' {
                return false;
            }
            pi += 1;
            ti += 1;
        } else {
            if ti >= text.len() || pat[pi] != text[ti] {
                return false;
            }
            pi += 1;
            ti += 1;
        }
    }
    ti == text.len()
}

/// Match `path` against an LSP glob, trying the absolute path, the path
/// relative to `root`, and a `**/` prefix (servers often register `*.rs`).
pub fn path_matches_watch(root: Option<&Path>, glob: &str, path: &Path) -> bool {
    let path_s = path.to_string_lossy().replace('\\', "/");
    if glob_matches(glob, &path_s) {
        return true;
    }
    if let Some(root) = root {
        if let Ok(rel) = path.strip_prefix(root) {
            let rel_s = rel.to_string_lossy().replace('\\', "/");
            if glob_matches(glob, &rel_s) {
                return true;
            }
        }
    }
    if !glob.starts_with("**/") && !glob.starts_with('/') {
        let prefixed = format!("**/{glob}");
        if glob_matches(&prefixed, &path_s) {
            return true;
        }
        if let Some(root) = root {
            if let Ok(rel) = path.strip_prefix(root) {
                let rel_s = rel.to_string_lossy().replace('\\', "/");
                if glob_matches(&prefixed, &rel_s) {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn double_star_matches_nested_rust_files() {
        assert!(glob_matches("**/*.rs", "src/lsp/client.rs"));
        assert!(glob_matches("**/*.rs", "client.rs"));
        assert!(!glob_matches("**/*.rs", "src/lsp/client.toml"));
    }

    #[test]
    fn star_does_not_cross_slashes() {
        assert!(glob_matches("src/*.rs", "src/main.rs"));
        assert!(!glob_matches("src/*.rs", "src/lsp/main.rs"));
    }

    #[test]
    fn relative_globs_match_under_the_workspace_root() {
        let root = Path::new("/home/user/proj");
        assert!(path_matches_watch(
            Some(root),
            "*.toml",
            Path::new("/home/user/proj/Cargo.toml")
        ));
        assert!(path_matches_watch(
            Some(root),
            "**/*.rs",
            Path::new("/home/user/proj/src/main.rs")
        ));
        assert!(!path_matches_watch(
            Some(root),
            "**/*.rs",
            Path::new("/home/user/other/main.rs")
        ));
    }
}
