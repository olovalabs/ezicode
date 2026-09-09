use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflicted,
}

impl ChangeKind {

    pub fn letter(self) -> &'static str {
        match self {
            ChangeKind::Modified => "M",
            ChangeKind::Added => "A",
            ChangeKind::Deleted => "D",
            ChangeKind::Renamed => "R",
            ChangeKind::Copied => "C",
            ChangeKind::TypeChanged => "T",
            ChangeKind::Untracked => "U",
            ChangeKind::Conflicted => "C",
        }
    }
}

/// One changed path with its index (staged) and worktree status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitChange {
    /// Absolute path on disk (the current name for renames).
    pub path: PathBuf,
    /// Repo-relative path with `/` separators (what git expects on its CLI).
    pub rel: String,
    /// The previous name, for renames/copies (repo-relative).
    pub old_rel: Option<String>,
    /// Index (staged) status, if any.
    pub index: Option<ChangeKind>,
    /// Worktree status, if any.
    pub worktree: Option<ChangeKind>,
    /// True when the file is untracked (`??`).
    pub untracked: bool,
}

impl GitChange {
    pub fn is_staged(&self) -> bool {
        self.index.is_some()
    }

    pub fn is_untracked(&self) -> bool {
        self.untracked
    }

    /// Letter shown next to the file in the STAGED CHANGES section.
    pub fn staged_letter(&self) -> &'static str {
        self.index.map(ChangeKind::letter).unwrap_or(" ")
    }

    pub fn worktree_letter(&self) -> &'static str {
        if self.untracked {
            "U"
        } else {
            self.worktree.map(ChangeKind::letter).unwrap_or(" ")
        }
    }
}

/// Snapshot of one repository: branch + changed files.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RepoStatus {
    pub root: PathBuf,
    pub branch: Option<String>,
    pub changes: Vec<GitChange>,
}

impl RepoStatus {
    pub fn change_count(&self) -> usize {
        self.changes.len()
    }

    pub fn staged_count(&self) -> usize {
        self.changes.iter().filter(|c| c.is_staged()).count()
    }
}

/// Find the repository root by walking up from `start` looking for `.git`
/// (a directory for normal repos, a file for worktrees/submodules).
pub fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        dir = dir.parent()?.to_path_buf();
    }
}

/// Current branch name (short SHA on a detached HEAD).
pub fn branch(root: &Path) -> Option<String> {
    let name = run_git(root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|(out, _)| out.trim().to_string())
        .filter(|s| !s.is_empty() && s != "HEAD");
    match name {
        Some(n) => Some(n),
        None => run_git(root, &["rev-parse", "--short", "HEAD"])
            .map(|(out, _)| out.trim().to_string())
            .filter(|s| !s.is_empty()),
    }
}

/// Full `git status` snapshot for `root`.
///
/// `--branch` makes git prepend a `## <branch>` record, so the branch and
/// the change list come from a **single** process spawn. The watcher thread
/// polls this every ~1.5 s; the previous extra `git rev-parse` (itself up to
/// two attempts) doubled the process-spawn cost of every poll. The
/// `rev-parse` fallback now only runs in the rare detached-HEAD case.
pub fn status(root: &Path) -> Option<RepoStatus> {
    let (raw, ok) = run_git(
        root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--branch",
            "--untracked-files=normal",
        ],
    )?;
    if !ok {
        return None;
    }
    let branch = branch_from_porcelain(&raw).or_else(|| branch(root));
    Some(RepoStatus {
        root: root.to_path_buf(),
        branch,
        changes: parse_porcelain(&raw, root),
    })
}

/// Extract the current branch from the `## ` header record emitted by
/// `git status --porcelain --branch`.
///
/// Header shapes (git 2.x): `## main`, `## main...origin/main`,
/// `## main...origin/main [ahead 1]`, `## No commits yet on main`, and
/// `## HEAD (no branch)` on a detached HEAD. Returns `None` when the header
/// is missing or detached, so the caller can fall back to `rev-parse`.
fn branch_from_porcelain(raw: &str) -> Option<String> {
    let header = raw.split('\0').next()?;
    let name = header.strip_prefix("## ")?;
    // Fresh repository with no commits yet.
    if let Some(rest) = name.strip_prefix("No commits yet on ") {
        let name = rest.trim();
        return (!name.is_empty()).then(|| name.to_string());
    }
    // `<local>...<remote> [tracking info]` — keep only the local side.
    let name = name.split("...").next().unwrap_or("").trim();
    if name.is_empty() || name.contains(' ') {
        // `HEAD (no branch)` — detached HEAD; let rev-parse resolve it.
        return None;
    }
    Some(name.to_string())
}

/// Parse `git status --porcelain=v1 -z` output into [`GitChange`]s.
///
/// Record layout (verified against git 2.x):
/// - normal entries: `XY path` (NUL-terminated)
/// - renames/copies: `XY new_path` followed by a bare `old_path` record
/// - untracked dirs keep their trailing `/`
pub fn parse_porcelain(raw: &str, root: &Path) -> Vec<GitChange> {
    let records: Vec<&str> = raw.split('\0').filter(|r| !r.is_empty()).collect();
    let mut out: Vec<GitChange> = Vec::new();

    for rec in records {
        // `--branch` prepends a `## <branch>` header record; it is parsed
        // separately by `branch_from_porcelain` and skipped here (its third
        // byte is a space too, so without this guard it would be misread as
        // a bogus `##` change entry).
        if rec.starts_with("## ") {
            continue;
        }
        let is_status_record = rec.len() >= 3 && rec.as_bytes().get(2) == Some(&b' ');
        if !is_status_record {
            // Rename/copy continuation: the previous (source) name.
            if let Some(last) = out.last_mut() {
                last.old_rel = Some(rec.to_string());
            }
            continue;
        }

        let xy = &rec[0..2];
        let path = &rec[3..];
        let x = xy.as_bytes()[0] as char;
        let y = xy.as_bytes()[1] as char;

        let untracked = x == '?' || y == '?';
        out.push(GitChange {
            path: root.join(path),
            rel: path.to_string(),
            old_rel: None,
            index: kind_of(x),
            worktree: if untracked { None } else { kind_of(y) },
            untracked,
        });
    }
    out
}

fn kind_of(ch: char) -> Option<ChangeKind> {
    match ch {
        'M' => Some(ChangeKind::Modified),
        'A' => Some(ChangeKind::Added),
        'D' => Some(ChangeKind::Deleted),
        'R' => Some(ChangeKind::Renamed),
        'C' => Some(ChangeKind::Copied),
        'T' => Some(ChangeKind::TypeChanged),
        'U' => Some(ChangeKind::Conflicted),
        _ => None,
    }
}

/// Unified diff for one path. `staged` selects the index (`--cached`).
/// Returns an empty string when the path has no diff (e.g. untracked files).
pub fn diff(root: &Path, rel: &str, staged: bool) -> Option<String> {
    let mut args: Vec<&str> = vec!["diff", "--no-ext-diff", "--no-color", "--unified=3"];
    if staged {
        args.push("--cached");
    }
    args.push("--");
    args.push(rel);
    run_git(root, &args).map(|(out, _)| out)
}

/// Build the unified diff git would produce for a brand-new (untracked)
/// file: the whole content as one added hunk. Shown by the diff view until
/// the file is staged.
pub fn new_file_diff(rel: &str, content: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("diff --git a/{rel} b/{rel}\n"));
    out.push_str("new file mode 100644\n");
    out.push_str("--- /dev/null\n");
    out.push_str(&format!("+++ b/{rel}\n"));
    let lines: Vec<&str> = content.lines().collect();
    out.push_str(&format!("@@ -0,0 +1,{} @@\n", lines.len()));
    for line in lines {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    if !content.ends_with('\n') && !content.is_empty() {
        out.push_str("\\ No newline at end of file\n");
    }
    out
}

// -- Diff parsing ------------------------------------------------------------

/// One rendered line of a unified diff.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// Line number in the old file (context/removed lines).
    pub old_no: Option<u32>,
    /// Line number in the new file (context/added lines).
    pub new_no: Option<u32>,
    /// Line text without the leading `+`/`-`/space (or the full header).
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum DiffLineKind {
    /// `diff --git` / `index` / `---` / `+++` / `new file mode` …
    Meta,
    /// `@@ -a,b +c,d @@`
    Hunk,
    Context,
    Add,
    Remove,
    /// `\ No newline at end of file`
    NoNewline,
}

/// Parse a unified diff (as produced by `git diff --unified=3 --no-color`)
/// into numbered lines for rendering.
#[allow(dead_code)]
pub fn parse_diff(raw: &str) -> Vec<DiffLine> {
    let mut out = Vec::new();
    let mut old_no: Option<u32> = None;
    let mut new_no: Option<u32> = None;

    for line in raw.lines() {
        if line.starts_with("--- ")
            || line.starts_with("+++ ")
            || line.starts_with("--- /dev/null")
            || line.starts_with("+++ /dev/null")
            || line.starts_with("--- a/")
            || line.starts_with("+++ b/")
        {
            out.push(DiffLine {
                kind: DiffLineKind::Meta,
                old_no: None,
                new_no: None,
                text: line.to_string(),
            });
            continue;
        }
        if let Some(rest) = line.strip_prefix("@@") {
            let (old, new) = hunk_numbers(rest);
            old_no = old;
            new_no = new;
            out.push(DiffLine {
                kind: DiffLineKind::Hunk,
                old_no: None,
                new_no: None,
                text: line.to_string(),
            });
            continue;
        }
        let Some(first) = line.chars().next() else {
            out.push(DiffLine {
                kind: DiffLineKind::Context,
                old_no,
                new_no,
                text: String::new(),
            });
            bump(&mut old_no, &mut new_no);
            continue;
        };
        match first {
            ' ' => {
                out.push(DiffLine {
                    kind: DiffLineKind::Context,
                    old_no,
                    new_no,
                    text: line[1..].to_string(),
                });
                bump(&mut old_no, &mut new_no);
            }
            '-' => {
                out.push(DiffLine {
                    kind: DiffLineKind::Remove,
                    old_no,
                    new_no: None,
                    text: line[1..].to_string(),
                });
                if let Some(n) = old_no.as_mut() {
                    *n += 1;
                }
            }
            '+' => {
                out.push(DiffLine {
                    kind: DiffLineKind::Add,
                    old_no: None,
                    new_no,
                    text: line[1..].to_string(),
                });
                if let Some(n) = new_no.as_mut() {
                    *n += 1;
                }
            }
            '\\' => {
                out.push(DiffLine {
                    kind: DiffLineKind::NoNewline,
                    old_no: None,
                    new_no: None,
                    text: line.to_string(),
                });
            }
            _ => {
                out.push(DiffLine {
                    kind: DiffLineKind::Meta,
                    old_no: None,
                    new_no: None,
                    text: line.to_string(),
                });
            }
        }
    }
    out
}

#[allow(dead_code)]
fn bump(old: &mut Option<u32>, new: &mut Option<u32>) {
    if let Some(n) = old.as_mut() {
        *n += 1;
    }
    if let Some(n) = new.as_mut() {
        *n += 1;
    }
}

/// Parse the numbers of a `@@ -a,b +c,d @@` header: `(old_start, new_start)`.
#[allow(dead_code)]
fn hunk_numbers(header: &str) -> (Option<u32>, Option<u32>) {
    let mut old = None;
    let mut new = None;
    for part in header.split_whitespace() {
        if let Some(rest) = part.strip_prefix('-') {
            old = rest.split(',').next().and_then(|s| s.parse().ok());
        } else if let Some(rest) = part.strip_prefix('+') {
            new = rest.split(',').next().and_then(|s| s.parse().ok());
        }
    }
    (old, new)
}

/// Stage (add) the given paths.
pub fn stage(root: &Path, rels: &[String]) -> bool {
    let mut args = vec!["add", "--"];
    args.extend(rels.iter().map(String::as_str));
    run_git(root, &args).map(|(_, ok)| ok).unwrap_or(false)
}

/// Stage every change in the repository (`git add -A`).
pub fn stage_all(root: &Path) -> bool {
    run_git(root, &["add", "-A"]).map(|(_, ok)| ok).unwrap_or(false)
}

/// Unstage the given paths (`git restore --staged`).
pub fn unstage(root: &Path, rels: &[String]) -> bool {
    let mut args = vec!["restore", "--staged", "--"];
    args.extend(rels.iter().map(String::as_str));
    run_git(root, &args).map(|(_, ok)| ok).unwrap_or(false)
}

/// Discard worktree changes for tracked paths (`git restore`).
pub fn discard(root: &Path, rels: &[String]) -> bool {
    let mut args = vec!["restore", "--"];
    args.extend(rels.iter().map(String::as_str));
    run_git(root, &args).map(|(_, ok)| ok).unwrap_or(false)
}

/// Delete untracked files/directories (`git clean -f`).
pub fn discard_untracked(root: &Path, rels: &[String]) -> bool {
    let mut args = vec!["clean", "-f", "-d", "--"];
    args.extend(rels.iter().map(String::as_str));
    run_git(root, &args).map(|(_, ok)| ok).unwrap_or(false)
}

/// Commit the staged changes. `Ok(summary)` on success (git prints a
/// "N files changed" summary to stdout), `Err(reason)` when git refuses.
pub fn commit(root: &Path, message: &str) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(root).arg("commit").arg("-m").arg(message);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let out = cmd
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Run git, returning (stdout, success). Never blocks the UI thread by
/// itself — callers run it on a background thread.
fn run_git(root: &Path, args: &[&str]) -> Option<(String, bool)> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(root)
        .arg("-c")
        .arg("core.quotepath=false");
    cmd.args(args);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::null());
    let out = cmd.output().ok()?;
    Some((String::from_utf8_lossy(&out.stdout).into_owned(), out.status.success()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> &'static Path {
        Path::new("/repo")
    }

    #[test]
    fn parses_empty_status() {
        assert!(parse_porcelain("", root()).is_empty());
    }

    #[test]
    fn branch_header_is_skipped_in_change_list() {

        let raw = "## main\0 M a.rs\0";
        let changes = parse_porcelain(raw, root());
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].rel, "a.rs");
    }

    #[test]
    fn extracts_branch_from_header() {
        assert_eq!(
            branch_from_porcelain("## main\0 M a.rs\0"),
            Some("main".to_string())
        );

        assert_eq!(
            branch_from_porcelain("## main...origin/main [ahead 1]\0"),
            Some("main".to_string())
        );

        assert_eq!(
            branch_from_porcelain("## No commits yet on main\0"),
            Some("main".to_string())
        );

        assert_eq!(branch_from_porcelain("## HEAD (no branch)\0"), None);

        assert_eq!(branch_from_porcelain(" M a.rs\0"), None);
    }

    #[test]
    fn parses_worktree_and_staged_letters() {
        let raw = " M a.rs\0M  b.rs\0MM c.rs\0?? new.txt\0";
        let changes = parse_porcelain(raw, root());
        assert_eq!(changes.len(), 4);

        assert_eq!(changes[0].rel, "a.rs");
        assert_eq!(changes[0].index, None);
        assert_eq!(changes[0].worktree, Some(ChangeKind::Modified));
        assert!(!changes[0].is_staged());

        assert_eq!(changes[1].rel, "b.rs");
        assert_eq!(changes[1].index, Some(ChangeKind::Modified));
        assert_eq!(changes[1].worktree, None);
        assert!(changes[1].is_staged());

        assert_eq!(changes[2].index, Some(ChangeKind::Modified));
        assert_eq!(changes[2].worktree, Some(ChangeKind::Modified));

        assert!(changes[3].is_untracked());
        assert_eq!(changes[3].worktree_letter(), "U");
    }

    #[test]
    fn parses_renames_with_continuation_records() {
        let raw = "R  new name.txt\0old name.txt\0RM renamed.rs\0source.rs\0";
        let changes = parse_porcelain(raw, root());
        assert_eq!(changes.len(), 2);

        assert_eq!(changes[0].rel, "new name.txt");
        assert_eq!(changes[0].old_rel.as_deref(), Some("old name.txt"));
        assert_eq!(changes[0].index, Some(ChangeKind::Renamed));

        assert_eq!(changes[1].rel, "renamed.rs");
        assert_eq!(changes[1].old_rel.as_deref(), Some("source.rs"));
        assert_eq!(changes[1].index, Some(ChangeKind::Renamed));
        assert_eq!(changes[1].worktree, Some(ChangeKind::Modified));
    }

    #[test]
    fn parses_deletions_additions_and_conflicts() {
        let raw = "D  gone.rs\0A  fresh.rs\0UU both.rs\0?? dir/\0";
        let changes = parse_porcelain(raw, root());
        assert_eq!(changes[0].index, Some(ChangeKind::Deleted));
        assert_eq!(changes[1].index, Some(ChangeKind::Added));
        assert_eq!(changes[2].index, Some(ChangeKind::Conflicted));
        assert_eq!(changes[2].worktree, Some(ChangeKind::Conflicted));
        assert_eq!(changes[3].rel, "dir/");
        assert!(changes[3].is_untracked());
    }

    #[test]
    fn makes_paths_absolute() {
        let changes = parse_porcelain(" M app/src/main.rs\0", Path::new("/home/u/proj"));
        assert_eq!(changes[0].path, PathBuf::from("/home/u/proj/app/src/main.rs"));
    }

    #[test]
    fn letters_match_vs_code() {
        assert_eq!(ChangeKind::Modified.letter(), "M");
        assert_eq!(ChangeKind::Added.letter(), "A");
        assert_eq!(ChangeKind::Deleted.letter(), "D");
        assert_eq!(ChangeKind::Renamed.letter(), "R");
        assert_eq!(ChangeKind::Untracked.letter(), "U");
    }

    #[test]
    fn synthesizes_new_file_diffs() {
        let diff = new_file_diff("new.txt", "line1\nline2\n");
        assert!(diff.starts_with("diff --git a/new.txt b/new.txt\n"));
        assert!(diff.contains("@@ -0,0 +1,2 @@\n"));
        assert!(diff.contains("+line1\n+line2\n"));
        assert!(!diff.contains("No newline"));

        let no_trailing = new_file_diff("x.sh", "#!/bin/bash\necho hi");
        assert!(no_trailing.contains("+echo hi\n\\ No newline at end of file\n"));
    }

    #[test]
    fn parses_unified_diffs() {
        let raw = "\
diff --git a/a.txt b/a.txt
index 7898192..c1827f0 100644
--- a/a.txt
+++ b/a.txt
@@ -1 +1,2 @@
-a
+aa
+b
";
        let lines = parse_diff(raw);
        let kinds: Vec<DiffLineKind> = lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            vec![
                DiffLineKind::Meta,
                DiffLineKind::Meta,
                DiffLineKind::Meta,
                DiffLineKind::Meta,
                DiffLineKind::Hunk,
                DiffLineKind::Remove,
                DiffLineKind::Add,
                DiffLineKind::Add,
            ]
        );

        assert_eq!(lines[4].text, "@@ -1 +1,2 @@");
        assert_eq!(lines[5].old_no, Some(1));
        assert_eq!(lines[5].new_no, None);
        assert_eq!(lines[5].text, "a");
        assert_eq!(lines[6].old_no, None);
        assert_eq!(lines[6].new_no, Some(1));
        assert_eq!(lines[6].text, "aa");
        assert_eq!(lines[7].new_no, Some(2));
        assert_eq!(lines[7].text, "b");
    }

    #[test]
    fn parses_multi_hunk_diffs_and_no_newline() {
        let raw = "\
@@ -10,2 +12,3 @@ fn foo() {
 ctx
-old
+new
+extra
\\ No newline at end of file
@@ -30 +33 @@ bar
+added
";
        let lines = parse_diff(raw);
        assert_eq!(lines.len(), 8);
        assert_eq!(lines[0].kind, DiffLineKind::Hunk);
        assert_eq!(lines[1].old_no, Some(10));
        assert_eq!(lines[1].new_no, Some(12));
        assert_eq!(lines[2].kind, DiffLineKind::Remove);
        assert_eq!(lines[2].old_no, Some(11));
        assert_eq!(lines[3].kind, DiffLineKind::Add);
        assert_eq!(lines[3].new_no, Some(13));
        assert_eq!(lines[4].kind, DiffLineKind::Add);
        assert_eq!(lines[4].new_no, Some(14));
        assert_eq!(lines[5].kind, DiffLineKind::NoNewline);
        assert_eq!(lines[6].kind, DiffLineKind::Hunk);
        assert_eq!(lines[7].kind, DiffLineKind::Add);
        assert_eq!(lines[7].new_no, Some(33));
    }
}
