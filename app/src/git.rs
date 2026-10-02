use std::collections::HashMap;
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
            // VS Code and Zed both mark conflicted paths with "!", which also
            // keeps the letter distinct from Copied.
            ChangeKind::Conflicted => "!",
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
    /// True when the entry is an unmerged (conflicted) path. Porcelain marks
    /// these with the XY pairs DD, AU, UD, UA, DU, AA and UU — a plain
    /// per-column read would misfile `AA`/`DD` as staged adds/deletes.
    pub conflicted: bool,
}

impl GitChange {
    pub fn is_staged(&self) -> bool {
        self.index.is_some()
    }

    pub fn is_untracked(&self) -> bool {
        self.untracked
    }

    pub fn is_conflicted(&self) -> bool {
        self.conflicted
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

/// Snapshot of one repository: branch + changed files + sync state.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RepoStatus {
    pub root: PathBuf,
    pub branch: Option<String>,
    /// Upstream ref (e.g. `origin/main`) when the branch tracks one.
    pub upstream: Option<String>,
    /// Commits ahead of / behind the upstream.
    pub ahead: u32,
    pub behind: u32,
    /// True on a detached HEAD (branch then holds the short SHA).
    pub detached: bool,
    pub changes: Vec<GitChange>,
}

impl RepoStatus {
    pub fn change_count(&self) -> usize {
        self.changes.len()
    }

    pub fn staged_count(&self) -> usize {
        self.changes
            .iter()
            .filter(|c| c.is_staged() && !c.is_conflicted())
            .count()
    }

    pub fn conflict_count(&self) -> usize {
        self.changes.iter().filter(|c| c.is_conflicted()).count()
    }

    /// Tracked, non-conflicted files with worktree edits ("commit all" scope).
    pub fn tracked_dirty_count(&self) -> usize {
        self.changes
            .iter()
            .filter(|c| !c.is_untracked() && !c.is_conflicted() && c.worktree.is_some())
            .count()
    }
}

/// Render-ready decorations. Building the ancestor map can be expensive in
/// repositories with many untracked files, so callers prepare it off-thread.
pub fn path_kinds(status: &RepoStatus) -> std::collections::HashMap<PathBuf, ChangeKind> {
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
    kinds
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
            // `all` lists every file inside an untracked directory instead of
            // collapsing it to `dir/` — matching Zed's panel, where each new
            // file is individually stageable and diffable.
            "--untracked-files=all",
        ],
    )?;
    if !ok {
        return None;
    }
    let header = parse_branch_header(&raw);
    let branch = header.branch.clone().or_else(|| branch(root));
    Some(RepoStatus {
        root: root.to_path_buf(),
        branch,
        upstream: header.upstream,
        ahead: header.ahead,
        behind: header.behind,
        detached: header.detached,
        changes: parse_porcelain(&raw, root),
    })
}

/// Parsed form of the `## ` header record emitted by
/// `git status --porcelain --branch`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchHeader {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub detached: bool,
}

/// Extract branch + tracking info from the porcelain `## ` header.
///
/// Header shapes (git 2.x): `## main`, `## main...origin/main`,
/// `## main...origin/main [ahead 1]`, `## main...origin/main [ahead 1, behind 2]`,
/// `## main...origin/main [gone]`, `## No commits yet on main`, and
/// `## HEAD (no branch)` on a detached HEAD. `branch` stays `None` when the
/// header is missing or detached, so the caller can fall back to `rev-parse`.
pub fn parse_branch_header(raw: &str) -> BranchHeader {
    let mut out = BranchHeader::default();
    let Some(header) = raw.split('\0').next() else {
        return out;
    };
    let Some(name) = header.strip_prefix("## ") else {
        return out;
    };
    // Fresh repository with no commits yet.
    if let Some(rest) = name.strip_prefix("No commits yet on ") {
        let rest = rest.trim();
        if !rest.is_empty() {
            out.branch = Some(rest.to_string());
        }
        return out;
    }
    if name.starts_with("HEAD (no branch)") {
        out.detached = true;
        return out;
    }
    // `<local>...<upstream> [tracking info]`.
    let (name_part, bracket) = match name.split_once(" [") {
        Some((n, b)) => (n, Some(b.trim_end_matches(']'))),
        None => (name, None),
    };
    let (local, upstream) = match name_part.split_once("...") {
        Some((l, u)) => (l.trim(), Some(u.trim().to_string())),
        None => (name_part.trim(), None),
    };
    if !local.is_empty() && !local.contains(' ') {
        out.branch = Some(local.to_string());
    }
    out.upstream = upstream.filter(|u| !u.is_empty());
    if let Some(bracket) = bracket {
        for part in bracket.split(',') {
            let part = part.trim();
            if let Some(n) = part.strip_prefix("ahead ") {
                out.ahead = n.trim().parse().unwrap_or(0);
            } else if let Some(n) = part.strip_prefix("behind ") {
                out.behind = n.trim().parse().unwrap_or(0);
            } else if part == "gone" {
                // Upstream ref was deleted; keep the name but report no
                // ahead/behind counts (git prints none in this case anyway).
            }
        }
    }
    out
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
    // Set right after pushing a rename/copy entry: the *next* record is
    // always its bare source path. Tracking this explicitly (instead of
    // sniffing whether a record "looks like" a status entry) means a source
    // path whose third byte happens to be a space, like `ab cd.txt`, can no
    // longer be misparsed as a bogus status record.
    let mut expect_rename_source = false;

    for rec in records {
        if expect_rename_source {
            expect_rename_source = false;
            if let Some(last) = out.last_mut() {
                last.old_rel = Some(rec.to_string());
            }
            continue;
        }
        // `--branch` prepends a `## <branch>` header record; it is parsed
        // separately by `parse_branch_header` and skipped here (its third
        // byte is a space too, so without this guard it would be misread as
        // a bogus `##` change entry).
        if rec.starts_with("## ") {
            continue;
        }
        // Malformed / non-status record: ignore rather than guess.
        if rec.len() < 4 || rec.as_bytes()[2] != b' ' {
            continue;
        }

        let xy = &rec[0..2];
        let path = &rec[3..];
        let x = xy.as_bytes()[0] as char;
        let y = xy.as_bytes()[1] as char;

        let untracked = x == '?' || y == '?';
        let conflicted = matches!(xy, "DD" | "AU" | "UD" | "UA" | "DU" | "AA" | "UU");
        out.push(GitChange {
            path: root.join(path),
            rel: path.to_string(),
            old_rel: None,
            index: if conflicted { None } else { kind_of(x) },
            worktree: if untracked {
                None
            } else if conflicted {
                Some(ChangeKind::Conflicted)
            } else {
                kind_of(y)
            },
            untracked,
            conflicted,
        });
        if x == 'R' || x == 'C' || y == 'R' || y == 'C' {
            expect_rename_source = true;
        }
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
    run_git(root, &["add", "-A"])
        .map(|(_, ok)| ok)
        .unwrap_or(false)
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
///
/// `all` adds `--all` (commit every tracked change, staged or not).
/// `amend` rewrites the previous commit; with an empty `message` the old
/// message is kept (`--no-edit`), otherwise it is replaced.
pub fn commit(root: &Path, message: &str, amend: bool, all: bool) -> Result<String, String> {
    let mut args: Vec<&str> = vec!["commit"];
    if all {
        args.push("--all");
    }
    if amend {
        args.push("--amend");
    }
    if amend && message.is_empty() {
        args.push("--no-edit");
    } else {
        args.push("-m");
        args.push(message);
    }
    run_git_result(root, &args)
}

// -- Remote + branch + stash operations --------------------------------------

/// `git fetch --all --prune`.
pub fn fetch(root: &Path) -> Result<String, String> {
    run_git_result(root, &["fetch", "--all", "--prune"])
}

/// `git pull` on the current branch.
pub fn pull(root: &Path) -> Result<String, String> {
    run_git_result(root, &["pull"])
}

/// Push the current branch. Publishes it (`-u origin <branch>`) when it has
/// no upstream yet; `force` uses `--force-with-lease`, which refuses to
/// clobber commits fetched since the last sync.
pub fn push(
    root: &Path,
    branch: Option<&str>,
    has_upstream: bool,
    force: bool,
) -> Result<String, String> {
    let mut args: Vec<&str> = vec!["push"];
    if force {
        args.push("--force-with-lease");
    }
    if !has_upstream {
        if let Some(branch) = branch {
            args.push("--set-upstream");
            args.push("origin");
            args.push(branch);
        }
    }
    run_git_result(root, &args)
}

/// One local branch, as listed by [`branches`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub is_current: bool,
    pub upstream: Option<String>,
    /// Relative age of the last commit, e.g. "3 days ago".
    pub last_commit: String,
}

/// Local branches, most recently committed first.
pub fn branches(root: &Path) -> Vec<Branch> {
    let raw = run_git(
        root,
        &[
            "for-each-ref",
            "refs/heads",
            "--sort=-committerdate",
            "--format=%(HEAD)\t%(refname:short)\t%(upstream:short)\t%(committerdate:relative)",
        ],
    );
    match raw {
        Some((out, true)) => parse_branches(&out),
        _ => Vec::new(),
    }
}

/// Parse `for-each-ref` output: `HEAD-marker \t name \t upstream \t age`.
pub fn parse_branches(raw: &str) -> Vec<Branch> {
    raw.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let head = fields.next()?;
            let name = fields.next()?.trim();
            if name.is_empty() {
                return None;
            }
            let upstream = fields.next().unwrap_or("").trim();
            let last_commit = fields.next().unwrap_or("").trim();
            Some(Branch {
                name: name.to_string(),
                is_current: head == "*",
                upstream: (!upstream.is_empty()).then(|| upstream.to_string()),
                last_commit: last_commit.to_string(),
            })
        })
        .collect()
}

/// `git checkout <name>`.
pub fn checkout(root: &Path, name: &str) -> Result<String, String> {
    run_git_result(root, &["checkout", name])
}

/// Create a branch off HEAD and switch to it (`git checkout -b`).
pub fn create_branch(root: &Path, name: &str) -> Result<String, String> {
    run_git_result(root, &["checkout", "-b", name])
}

/// Delete a fully merged branch (`git branch -d`); git's own error explains
/// when the branch is unmerged, rather than silently forcing `-D`.
pub fn delete_branch(root: &Path, name: &str) -> Result<String, String> {
    run_git_result(root, &["branch", "-d", name])
}

/// Stash the working tree, untracked files included.
pub fn stash_push(root: &Path) -> Result<String, String> {
    run_git_result(root, &["stash", "push", "--include-untracked"])
}

/// Pop the most recent stash entry.
pub fn stash_pop(root: &Path) -> Result<String, String> {
    run_git_result(root, &["stash", "pop"])
}

/// `git init` in `root` (for the "no repository" empty state).
pub fn init(root: &Path) -> Result<String, String> {
    run_git_result(root, &["init"])
}

/// Run git and translate the exit status into a `Result`, so no failure can
/// pass silently: `Ok(stdout)` on success, `Err(stderr-or-stdout)` otherwise.
/// Never blocks the UI thread by itself — callers run it on a background
/// thread.
fn run_git_result(root: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(root)
        .arg("-c")
        .arg("core.quotepath=false");
    cmd.args(args);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let out = cmd
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if !err.is_empty() {
            return Err(err);
        }
        let out_text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !out_text.is_empty() {
            return Err(out_text);
        }
        Err(format!(
            "git {} failed (exit {:?})",
            args.first().unwrap_or(&""),
            out.status.code()
        ))
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
    Some((
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.success(),
    ))
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
            parse_branch_header("## main\0 M a.rs\0").branch,
            Some("main".to_string())
        );

        let tracking = parse_branch_header("## main...origin/main [ahead 1]\0");
        assert_eq!(tracking.branch, Some("main".to_string()));
        assert_eq!(tracking.upstream, Some("origin/main".to_string()));
        assert_eq!(tracking.ahead, 1);
        assert_eq!(tracking.behind, 0);

        assert_eq!(
            parse_branch_header("## No commits yet on main\0").branch,
            Some("main".to_string())
        );

        let detached = parse_branch_header("## HEAD (no branch)\0");
        assert_eq!(detached.branch, None);
        assert!(detached.detached);

        assert_eq!(parse_branch_header(" M a.rs\0"), BranchHeader::default());
    }

    #[test]
    fn extracts_ahead_behind_and_gone_upstreams() {
        let both = parse_branch_header("## feat/x...origin/feat/x [ahead 3, behind 2]\0");
        assert_eq!(both.branch, Some("feat/x".to_string()));
        assert_eq!(both.upstream, Some("origin/feat/x".to_string()));
        assert_eq!(both.ahead, 3);
        assert_eq!(both.behind, 2);

        let behind_only = parse_branch_header("## main...origin/main [behind 4]\0");
        assert_eq!(behind_only.ahead, 0);
        assert_eq!(behind_only.behind, 4);

        let gone = parse_branch_header("## main...origin/main [gone]\0");
        assert_eq!(gone.branch, Some("main".to_string()));
        assert_eq!(gone.upstream, Some("origin/main".to_string()));
        assert_eq!(gone.ahead, 0);
        assert_eq!(gone.behind, 0);

        let no_tracking = parse_branch_header("## main\0");
        assert_eq!(no_tracking.upstream, None);
        assert!(!no_tracking.detached);
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
        assert!(changes[2].is_conflicted());
        assert_eq!(changes[2].index, None);
        assert_eq!(changes[2].worktree, Some(ChangeKind::Conflicted));
        assert!(!changes[2].is_staged());
        assert_eq!(changes[3].rel, "dir/");
        assert!(changes[3].is_untracked());
    }

    #[test]
    fn detects_every_conflict_pair() {
        let raw = "DD a\0AU b\0UD c\0UA d\0DU e\0AA f\0UU g\0M  h\0";
        let changes = parse_porcelain(raw, root());
        assert_eq!(changes.len(), 8);
        for change in &changes[..7] {
            assert!(change.is_conflicted(), "{} not conflicted", change.rel);
            assert!(!change.is_staged());
        }
        // `AA`/`DD` must not leak into the staged bucket as adds/deletes.
        assert!(!changes[5].is_staged());
        assert!(!changes[0].is_staged());
        assert!(!changes[7].is_conflicted());
        assert!(changes[7].is_staged());
    }

    #[test]
    fn rename_source_with_space_at_third_byte_is_not_a_status_record() {
        // Source path `ab cd.txt`: its third byte is a space, so shape
        // sniffing alone would misread it as a status record.
        let raw = "R  new.txt\0ab cd.txt\0 M other.rs\0";
        let changes = parse_porcelain(raw, root());
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].rel, "new.txt");
        assert_eq!(changes[0].old_rel.as_deref(), Some("ab cd.txt"));
        assert_eq!(changes[1].rel, "other.rs");
    }

    #[test]
    fn parses_branch_lists() {
        let raw = "*\tmain\torigin/main\t2 hours ago\n \tfeat/panel\t\t3 days ago\n";
        let branches = parse_branches(raw);
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[0].name, "main");
        assert!(branches[0].is_current);
        assert_eq!(branches[0].upstream.as_deref(), Some("origin/main"));
        assert_eq!(branches[0].last_commit, "2 hours ago");
        assert_eq!(branches[1].name, "feat/panel");
        assert!(!branches[1].is_current);
        assert_eq!(branches[1].upstream, None);
        assert!(parse_branches("").is_empty());
    }

    #[test]
    fn makes_paths_absolute() {
        let changes = parse_porcelain(" M app/src/main.rs\0", Path::new("/home/u/proj"));
        assert_eq!(
            changes[0].path,
            PathBuf::from("/home/u/proj/app/src/main.rs")
        );
    }

    #[test]
    fn letters_match_vs_code() {
        assert_eq!(ChangeKind::Modified.letter(), "M");
        assert_eq!(ChangeKind::Added.letter(), "A");
        assert_eq!(ChangeKind::Deleted.letter(), "D");
        assert_eq!(ChangeKind::Renamed.letter(), "R");
        assert_eq!(ChangeKind::Untracked.letter(), "U");
        assert_eq!(ChangeKind::Conflicted.letter(), "!");
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
