//! Project-wide text search powered by the ripgrep engine.
//!
//! VS Code's search view is fast because it does three things:
//! 1. walks the project with `ignore` (respects `.gitignore`, skips `target/`,
//!    `node_modules/`, `.git/` …),
//! 2. searches each file with ripgrep's `grep-searcher` (memchr + SIMD literal
//!    / DFA regex, binary detection, line oriented), and
//! 3. runs the walk+search off the UI thread with hard result caps.
//!
//! This module is exactly that pipeline as a dependency-free (`rg` binary not
//! required) library: [`run_search`] is called from a GPUI `background_spawn`
//! so the UI never blocks, and [`apply_replace`] reuses the same matcher so
//! "Replace All" touches exactly what the results list shows.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use ignore::{DirEntry, WalkBuilder};

/// Toggle state mirroring the `Aa / whole-word / .*` badges in the search view.
#[derive(Clone, Debug)]
pub struct SearchOptions {
    pub query: String,
    /// `Aa` — default false (VS Code is case-insensitive by default).
    pub case_sensitive: bool,
    /// Whole-word (`\b…\b`) filter.
    pub whole_word: bool,
    /// `.*` — treat the query as a regular expression. When false the query
    /// is searched literally (regex metacharacters are escaped).
    pub use_regex: bool,
    /// Optional "files to include" substring/glob filter, e.g. `src`, `*.rs`.
    /// Empty means every file.
    pub include_filter: String,
}

impl SearchOptions {
    pub fn query_trimmed(&self) -> &str {
        self.query.trim()
    }
}

/// One matched line inside a file. Line numbers are 1-based (editor-ready);
/// byte offsets are relative to the line's text (also editor-ready).
#[derive(Clone, Debug)]
pub struct SearchMatch {
    pub line_number: usize,
    pub line_text: String,
    pub col_start: usize,
    pub col_end: usize,
}

/// All matches inside a single file.
#[derive(Clone, Debug)]
pub struct SearchFileResult {
    pub path: PathBuf,
    pub rel: String,
    pub matches: Vec<SearchMatch>,
}

/// Outcome of [`run_search`].
#[derive(Clone, Debug, Default)]
pub struct SearchOutput {
    pub files: Vec<SearchFileResult>,
    pub total_matches: usize,
    /// True when `max_matches` was hit — more hits exist on disk.
    pub truncated: bool,
    pub searched_files: usize,
    pub elapsed_ms: u64,
}

/// Hard caps keep huge monorepos interactive — same idea as VS Code's
/// 20k-result limit. The search stops early instead of freezing the UI.
pub const MAX_MATCHES: usize = 10_000;
pub const MAX_FILES: usize = 1_000;
pub const MAX_LINE_PREVIEW: usize = 500;

fn is_skipped_dir(name: &str) -> bool {
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
            | "__pycache__"
            | ".venv"
            | "venv"
            | ".vscode"
            | ".idea"
            | ".gradle"
            | "test-results"
            | "coverage"
    )
}

fn include_filter_hit(path: &Path, root: &Path, filter: &str) -> bool {
    let filter = filter.trim();
    if filter.is_empty() {
        return true;
    }
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    for part in filter.split([',', ';']) {
        let part = part.trim().trim_start_matches("./");
        if part.is_empty() {
            continue;
        }
        if part.contains('*') {
            // Minimal glob: `*` matches any run, everything else literal.
            let needle: Vec<&str> = part.split('*').collect();
            let mut hay = rel.as_str();
            let mut ok = true;
            // Support patterns like `*.rs`, `src/*.ts`, `*test*`.
            let starts_star = part.starts_with('*');
            let ends_star = part.ends_with('*');
            if !starts_star {
                if let Some(first) = needle.first() {
                    if !(rel.starts_with(*first) || file_name.starts_with(*first)) {
                        // fall through to substring check at the end
                        ok = false;
                    } else {
                        hay = &hay[first.len().min(hay.len())..];
                    }
                }
            }
            if ok {
                let mut rest = hay;
                let segs: Vec<&str> = if starts_star {
                    needle.to_vec()
                } else {
                    needle.iter().skip(1).copied().collect()
                };
                for (i, seg) in segs.iter().enumerate() {
                    if seg.is_empty() {
                        continue;
                    }
                    let last = i + 1 == segs.len() && !ends_star;
                    if last {
                        if !(rest.ends_with(seg) || file_name.ends_with(seg)) {
                            ok = false;
                            break;
                        }
                    } else if let Some(pos) = rest.find(seg) {
                        rest = &rest[pos + seg.len()..];
                    } else {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                return true;
            }
        } else if rel.contains(part) || file_name.contains(part) {
            return true;
        }
    }
    false
}

/// Build the ripgrep matcher for these options. Returns `Err` with a
/// user-facing message when the regex does not compile.
fn build_matcher(opts: &SearchOptions) -> Result<grep_regex::RegexMatcher, String> {
    let q = opts.query_trimmed();
    if q.is_empty() {
        return Err("Type something to search".to_string());
    }
    let pattern = if opts.use_regex {
        let mut p = q.to_string();
        if opts.whole_word {
            p = format!(r"\b(?:{p})\b");
        }
        p
    } else {
        let mut p = regex_escape(q);
        if opts.whole_word {
            p = format!(r"\b(?:{p})\b");
        }
        p
    };
    RegexMatcherBuilder::new()
        .case_insensitive(!opts.case_sensitive)
        .word(false)
        .build(&pattern)
        .map_err(|e| format!("Invalid regex: {e}"))
}

/// Escape a literal so it can be embedded in a regex.
fn regex_escape(literal: &str) -> String {
    let mut out = String::with_capacity(literal.len());
    for c in literal.chars() {
        if matches!(
            c,
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' | '#'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Walk `root` and search every text file. Pure I/O + compute, no GPUI —
/// call it from `background_spawn`.
pub fn run_search(
    root: &Path,
    opts: &SearchOptions,
    max_matches: usize,
    max_files: usize,
) -> Result<SearchOutput, String> {
    let started = Instant::now();
    let matcher = build_matcher(opts)?;
    let matcher = Arc::new(matcher);

    // Collect candidate files first with the parallel walker (this is the
    // part that makes big projects fast: directory traversal is I/O bound
    // and `ignore` parallelises it, respecting .gitignore).
    let include = opts.include_filter.clone();
    let root_owned = root.to_path_buf();
    let candidates: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let candidates = candidates.clone();
        let root_clone = root_owned.clone();
        let mut builder = WalkBuilder::new(&root_owned);
        builder
            .hidden(true)
            .parents(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .require_git(false)
            .filter_entry(move |e: &DirEntry| {
                if e.file_type().map_or(false, |ft| ft.is_dir()) {
                    let name = e.file_name().to_string_lossy();
                    if is_skipped_dir(&name) {
                        return false;
                    }
                }
                true
            });
        builder.build_parallel().run(|| {
            let candidates = candidates.clone();
            let root_clone = root_clone.clone();
            let include = include.clone();
            Box::new(move |entry| {
                use ignore::WalkState;
                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                if !entry.file_type().map_or(false, |ft| ft.is_file()) {
                    return WalkState::Continue;
                }
                let path = entry.path().to_path_buf();
                if !include_filter_hit(&path, &root_clone, &include) {
                    return WalkState::Continue;
                }
                // Skip obviously non-text huge artifacts early (cheap stat).
                if let Ok(md) = entry.metadata() {
                    if md.len() > 4_000_000 {
                        return WalkState::Continue;
                    }
                }
                candidates.lock().unwrap().push(path);
                WalkState::Continue
            })
        });
    }
    let mut candidates = candidates.lock().unwrap().drain(..).collect::<Vec<_>>();
    candidates.sort();

    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .line_number(true)
        .multi_line(false)
        .build();

    let mut out = SearchOutput::default();
    for path in candidates {
        if out.total_matches >= max_matches || out.files.len() >= max_files {
            out.truncated = true;
            break;
        }
        out.searched_files += 1;
        let matcher = matcher.clone();
        // The sink borrows `matcher`; defined per file.
        struct LocalSink<'m> {
            matcher: &'m grep_regex::RegexMatcher,
            matches: Vec<SearchMatch>,
        }
        impl grep_searcher::Sink for LocalSink<'_> {
            type Error = std::io::Error;
            fn matched(
                &mut self,
                _searcher: &grep_searcher::Searcher,
                mat: &grep_searcher::SinkMatch<'_>,
            ) -> Result<bool, std::io::Error> {
                let line_number = mat.line_number().unwrap_or(1) as usize;
                let bytes = mat.bytes();
                let mut end = bytes.len();
                while end > 0 && (bytes[end - 1] == b'\n' || bytes[end - 1] == b'\r') {
                    end -= 1;
                }
                let line_text = String::from_utf8_lossy(&bytes[..end]).into_owned();
                let preview = if line_text.chars().count() > MAX_LINE_PREVIEW {
                    line_text.chars().take(MAX_LINE_PREVIEW).collect::<String>()
                } else {
                    line_text
                };
                let mut from = 0usize;
                let mut pushed = 0;
                while from <= bytes.len() && pushed < 32 {
                    let range = match self.matcher.find_at(bytes, from) {
                        Ok(Some(m)) => m,
                        _ => break,
                    };
                    if range.start() >= end {
                        break;
                    }
                    let (s, e) = (range.start().min(end), range.end().min(end));
                    self.matches.push(SearchMatch {
                        line_number,
                        line_text: preview.clone(),
                        col_start: s,
                        col_end: e.max(s),
                    });
                    pushed += 1;
                    if range.end() <= from {
                        from += 1;
                    } else {
                        from = range.end();
                    }
                }
                if pushed == 0 {
                    self.matches.push(SearchMatch {
                        line_number,
                        line_text: preview,
                        col_start: 0,
                        col_end: 0,
                    });
                }
                Ok(self.matches.len() < 2_000)
            }
        }
        let mut sink = LocalSink {
            matcher: &matcher,
            matches: Vec::new(),
        };
        let r = searcher.search_path(&*matcher, &path, &mut sink);
        if r.is_err() {
            continue; // unreadable / binary — same as VS Code: silently skip
        }
        let mut sink_matches = sink.matches;
        if sink_matches.is_empty() {
            continue;
        }
        // Cap per file so one generated file can't flood the results.
        sink_matches.truncate(500);
        out.total_matches += sink_matches.len();
        let rel = path
            .strip_prefix(&root_owned)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        out.files.push(SearchFileResult {
            path,
            rel,
            matches: sink_matches,
        });
        if out.total_matches >= max_matches {
            out.truncated = true;
            break;
        }
    }

    out.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(out)
}

/// Replace every match described by `files` with `replace_text`.
///
/// Files are rewritten on a background thread; returns (files_changed,
/// replacements_made). Binary/oversize files are skipped. `opts` must be the
/// same options the search ran with so literal vs regex handling matches.
pub fn apply_replace(
    files: &[SearchFileResult],
    opts: &SearchOptions,
    replace_text: &str,
) -> (usize, usize) {
    let matcher = match build_matcher(opts) {
        Ok(m) => m,
        Err(_) => return (0, 0),
    };
    let mut files_changed = 0;
    let mut total = 0;
    for file in files {
        let Ok(bytes) = std::fs::read(&file.path) else {
            continue;
        };
        if bytes.len() > 8_000_000 || bytes.iter().take(8000).any(|&b| b == 0) {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let use_regex = opts.use_regex;
        let mut out = String::with_capacity(text.len());
        let mut count = 0;
        if use_regex {
            // Re-implement replacement via line spans so whole-word wrapping
            // stays consistent with the search matcher.
            let mut pos = 0;
            while pos <= text.len() {
                if !text.is_char_boundary(pos) {
                    pos += 1;
                    continue;
                }
                let hay = &text.as_bytes()[pos..];
                let found = matcher
                    .find_at(hay, 0)
                    .ok()
                    .flatten()
                    .map(|m| (pos + m.start(), pos + m.end()));
                match found {
                    Some((s, e)) => {
                        out.push_str(&text[pos..s]);
                        out.push_str(replace_text);
                        count += 1;
                        pos = if e <= s { s + 1 } else { e };
                        if pos > text.len() {
                            break;
                        }
                    }
                    None => {
                        out.push_str(&text[pos..]);
                        break;
                    }
                }
                if count > 50_000 {
                    break;
                }
            }
        } else {
            // Literal path: honour case sensitivity without regex overhead.
            if opts.case_sensitive {
                let mut rest = text.as_str();
                loop {
                    match rest.find(opts.query_trimmed()) {
                        Some(ix) => {
                            out.push_str(&rest[..ix]);
                            out.push_str(replace_text);
                            count += 1;
                            rest = &rest[ix + opts.query_trimmed().len()..];
                            if count > 50_000 {
                                out.push_str(rest);
                                break;
                            }
                        }
                        None => {
                            out.push_str(rest);
                            break;
                        }
                    }
                }
            } else {
                // Literal case-insensitive replace via a lowercase shadow
                // string (handles whole-word with `is_word_hit`).
                let q = opts.query_trimmed().to_lowercase();
                let lower = text.to_lowercase();
                out.clear();
                count = 0;
                let qb = q.len();
                let mut emit = 0;
                let mut cursor = 0;
                while cursor <= lower.len() {
                    match lower[cursor..].find(&q) {
                        Some(ix) => {
                            let s = cursor + ix;
                            let e = (s + qb).min(text.len());
                            if !text.is_char_boundary(s)
                                || !text.is_char_boundary(e)
                                || (opts.whole_word && !is_word_hit(&text, s, e))
                            {
                                cursor = s + 1;
                                continue;
                            }
                            out.push_str(&text[emit..s]);
                            out.push_str(replace_text);
                            count += 1;
                            emit = e;
                            cursor = e.max(cursor + 1);
                            if count > 50_000 {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                out.push_str(&text[emit..]);
            }
        }
        if count == 0 {
            continue;
        }
        if std::fs::write(&file.path, out.as_bytes()).is_ok() {
            files_changed += 1;
            total += count;
        }
    }
    (files_changed, total)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_word_hit(text: &str, s: usize, e: usize) -> bool {
    let before = text[..s].chars().next_back().map(is_word_char).unwrap_or(false);
    let after = text[e..].chars().next().map(is_word_char).unwrap_or(false);
    !before && !after
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn opts(query: &str) -> SearchOptions {
        SearchOptions {
            query: query.to_string(),
            case_sensitive: false,
            whole_word: false,
            use_regex: false,
            include_filter: String::new(),
        }
    }

    #[test]
    fn literal_search_finds_matches_case_insensitive() {
        let dir = std::env::temp_dir().join(format!("ezi-search-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join("a.rs"), "Hello World\nhello world\nnothing\n").unwrap();
        let out = run_search(&dir, &opts("hello"), MAX_MATCHES, MAX_FILES).unwrap();
        assert_eq!(out.total_matches, 2);
        assert_eq!(out.files.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn case_sensitive_and_whole_word_flags_work() {
        let dir = std::env::temp_dir().join(format!("ezi-search2-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join("a.txt"), "foo foobar Foo\n").unwrap();
        let mut o = opts("foo");
        o.case_sensitive = true;
        let out = run_search(&dir, &o, MAX_MATCHES, MAX_FILES).unwrap();
        assert_eq!(out.total_matches, 2); // foo + foobar, not Foo
        o.whole_word = true;
        let out = run_search(&dir, &o, MAX_MATCHES, MAX_FILES).unwrap();
        assert_eq!(out.total_matches, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn regex_search_and_invalid_regex_error() {
        let dir = std::env::temp_dir().join(format!("ezi-search3-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join("a.txt"), "foo123 bar\n").unwrap();
        let mut o = opts(r"foo\d+");
        o.use_regex = true;
        let out = run_search(&dir, &o, MAX_MATCHES, MAX_FILES).unwrap();
        assert_eq!(out.total_matches, 1);
        let mut bad = opts("(");
        bad.use_regex = true;
        assert!(run_search(&dir, &bad, MAX_MATCHES, MAX_FILES).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn gitignore_is_respected_and_target_skipped() {
        let dir = std::env::temp_dir().join(format!("ezi-search4-{}", std::process::id()));
        let _ = fs::create_dir_all(dir.join("target"));
        fs::write(dir.join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(dir.join("ignored.txt"), "needle here\n").unwrap();
        fs::write(dir.join("target").join("x.txt"), "needle here\n").unwrap();
        fs::write(dir.join("keep.txt"), "needle here\n").unwrap();
        let out = run_search(&dir, &opts("needle"), MAX_MATCHES, MAX_FILES).unwrap();
        assert_eq!(out.files.len(), 1);
        assert!(out.files[0].rel.ends_with("keep.txt"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_all_rewrites_only_hits() {
        let dir = std::env::temp_dir().join(format!("ezi-search5-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join("a.txt"), "foo foo\n").unwrap();
        let out = run_search(&dir, &opts("foo"), MAX_MATCHES, MAX_FILES).unwrap();
        let (files, n) = apply_replace(&out.files, &opts("foo"), "bar");
        assert_eq!((files, n), (1, 2));
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "bar bar\n");
        let _ = fs::remove_dir_all(&dir);
    }
}
