use std::io::Write as _;
use std::ops::Range;
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

/// Like [`run_git`] but pipes `input` into git's stdin. Used by
/// `git blame --contents -` so a dirty (unsaved) buffer can still be blamed,
/// exactly as Zed feeds buffer contents to blame. Returns `(stdout, success)`.
fn run_git_stdin(root: &Path, args: &[&str], input: &[u8]) -> Option<(String, bool)> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(root)
        .arg("-c")
        .arg("core.quotepath=false");
    cmd.args(args);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::null());
    let mut child = cmd.spawn().ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        // Ignore write errors: git may exit early (e.g. path not tracked) and
        // close the pipe, which would otherwise surface as a broken-pipe error.
        let _ = stdin.write_all(input);
    }
    let out = child.wait_with_output().ok()?;
    Some((
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.success(),
    ))
}

// ============================================================================
// Blame (Feature 2: Zed-style inline git blame)
//
// Mirrors Zed's `crates/git/src/blame.rs`: we drive `git blame --incremental`
// (optionally with `--contents -` for dirty buffers) and parse its streamed
// records into [`BlameEntry`] runs. The per-buffer cache, debounce, offset
// shifting and lazy commit-detail fetch live in `crate::git_blame`, keeping
// this module a pure, UI-free service layer.
// ============================================================================

/// One contiguous run of lines attributed to a single commit, as produced by
/// `git blame --incremental`. Named and shaped after Zed's `BlameEntry`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameEntry {
    /// Full 40-char commit SHA (all-zero for not-yet-committed lines).
    pub sha: String,
    /// 0-based, half-open range of *final* (working-copy) line numbers.
    pub range: Range<u32>,
    /// 1-based line number this run had in the original commit.
    pub original_line_number: u32,
    pub author: Option<String>,
    pub author_mail: Option<String>,
    pub author_time: Option<i64>,
    pub author_tz: Option<String>,
    pub committer_time: Option<i64>,
    pub summary: Option<String>,
    /// `<sha> <filename>` of the previous revision, when git reports one.
    pub previous: Option<String>,
    pub filename: String,
    /// True when this commit is a history boundary (has no parent to blame).
    pub boundary: bool,
}

impl BlameEntry {
    /// A line git could not attribute to any commit yet — local,
    /// uncommitted changes. Zed renders no annotation at all for
    /// these (its parser drops zero-SHA entries), so the UI skips
    /// them rather than labelling them.
    pub fn is_uncommitted(&self) -> bool {
        self.sha.is_empty() || self.sha.bytes().all(|b| b == b'0')
    }

    /// The abbreviated SHA shown in the UI (git's default is 7–8 chars;
    /// Zed uses 7).
    pub fn short_sha(&self) -> String {
        self.sha.chars().take(7).collect()
    }
}

/// The blame for one file: the commit runs plus a per-row index into them,
/// so a render pass can look up a line in O(1). Owns the offset-shifting used
/// to keep annotations aligned between re-blames (see [`FileBlame::shift`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileBlame {
    pub entries: Vec<BlameEntry>,
    /// Index into `entries` for each final line; `None` for lines with no
    /// blame yet (freshly typed lines between debounced re-blames).
    pub rows: Vec<Option<usize>>,
}

impl FileBlame {
    /// Build the per-row index from a set of entries (their `range`s are the
    /// authoritative source of truth).
    pub fn from_entries(entries: Vec<BlameEntry>) -> Self {
        let max_row = entries.iter().map(|e| e.range.end).max().unwrap_or(0) as usize;
        let mut rows = vec![None; max_row];
        for (ix, entry) in entries.iter().enumerate() {
            for row in entry.range.start..entry.range.end {
                if let Some(slot) = rows.get_mut(row as usize) {
                    *slot = Some(ix);
                }
            }
        }
        Self { entries, rows }
    }

    /// The blame entry attributed to `row` (0-based), if any.
    #[cfg(test)]
    pub fn line(&self, row: usize) -> Option<&BlameEntry> {
        let ix = (*self.rows.get(row)?)?;
        self.entries.get(ix)
    }

    /// Shift the per-row index to absorb an edit that replaced `removed` rows
    /// starting at `start_row` with `added` rows. The newly inserted rows get
    /// `None` (no annotation until the next re-blame), while rows below the
    /// edit keep pointing at their original commit — exactly how Zed keeps
    /// blame aligned between debounced re-blames.
    pub fn shift(&mut self, start_row: usize, removed: usize, added: usize) {
        let len = self.rows.len();
        let start = start_row.min(len);
        let end = (start_row + removed).min(len);
        let tail = self.rows.split_off(end);
        self.rows.truncate(start);
        self.rows.resize(start + added, None);
        self.rows.extend(tail);
    }
}

const GIT_BLAME_NO_COMMIT_ERROR: &str = "no such ref: HEAD";

/// Blame a single file. `contents` (when `Some`) is fed to git via
/// `--contents -`, so an unsaved buffer is blamed exactly as it currently
/// reads on screen (this is what Zed does); pass `None` to blame the file on
/// disk. Returns `None` on any git failure so callers stay silent for
/// untracked / binary / out-of-repo paths.
pub fn blame_file(root: &Path, rel: &str, contents: Option<&str>) -> Option<FileBlame> {
    let (raw, ok) = match contents {
        Some(text) => run_git_stdin(
            root,
            &["blame", "--incremental", "--contents", "-", "--", rel],
            text.as_bytes(),
        )?,
        None => run_git(root, &["blame", "--incremental", "--", rel])?,
    };
    if !ok {
        // A fresh repo with no HEAD yet, or an untracked path: no blame, but
        // not an error the user needs to see.
        if raw.contains(GIT_BLAME_NO_COMMIT_ERROR) {
            return Some(FileBlame::default());
        }
        return None;
    }
    Some(FileBlame::from_entries(parse_blame_incremental(&raw)))
}

/// Parse the output of `git blame --incremental` into commit runs.
///
/// Each run starts with `<sha> <orig-line> <final-line> <num-lines>` and ends
/// with a `filename …` record. Signature fields (author/summary/…) are only
/// emitted the first time a SHA appears, so we copy them forward from the
/// first entry that carried them, just like Zed's `GitBlameParser`.
pub fn parse_blame_incremental(raw: &str) -> Vec<BlameEntry> {
    let mut entries: Vec<BlameEntry> = Vec::new();
    // First index in `entries` that carried full signature info for a SHA.
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut current: Option<BlameEntry> = None;

    for line in raw.lines() {
        let line = line.trim_end_matches(['\r', '\n']);

        // Start of a new commit run.
        if current.is_none() {
            if let Some(mut entry) = parse_blame_header(line) {
                if let Some(&slot) = seen.get(&entry.sha) {
                    if let Some(prev) = entries.get(slot) {
                        entry.author.clone_from(&prev.author);
                        entry.author_mail.clone_from(&prev.author_mail);
                        entry.author_time = prev.author_time;
                        entry.author_tz.clone_from(&prev.author_tz);
                        entry.committer_time = prev.committer_time;
                        entry.summary.clone_from(&prev.summary);
                        entry.boundary = prev.boundary;
                    }
                }
                current = Some(entry);
            }
            continue;
        }

        // Signature line for the run in progress. Borrow `current` only for the
        // field updates; the `filename` line (which closes the run) sets a flag
        // so `current.take()` runs *after* the borrow ends — otherwise the
        // outstanding `&mut` and the `take()` would overlap.
        let entry = current.as_mut().expect("run in progress");
        if line == "boundary" {
            entry.boundary = true;
            continue;
        }
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        let mut finished = false;
        match key {
            "author" => entry.author = Some(value.to_string()),
            "author-mail" => entry.author_mail = Some(value.to_string()),
            "author-time" => entry.author_time = value.parse().ok(),
            "author-tz" => entry.author_tz = Some(value.to_string()),
            "committer-time" => entry.committer_time = value.parse().ok(),
            "summary" => entry.summary = Some(value.to_string()),
            "previous" => entry.previous = Some(value.to_string()),
            "filename" => {
                entry.filename = value.to_string();
                finished = true;
            }
            _ => {}
        }
        if finished {
            let done = current.take().expect("run in progress");
            seen.entry(done.sha.clone()).or_insert(entries.len());
            entries.push(done);
        }
    }
    entries
}

/// Parse a blame header line: `<40-hex-sha> <orig-line> <final-line> <count>`.
/// Returns `None` for anything that is not a header (defensive parsing).
fn parse_blame_header(line: &str) -> Option<BlameEntry> {
    let mut parts = line.split_whitespace();
    let sha = parts.next()?;
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let original_line_number: u32 = parts.next()?.parse().ok()?;
    let final_line_number: u32 = parts.next()?.parse().ok()?;
    let count: u32 = parts.next()?.parse().ok()?;
    let start = final_line_number.saturating_sub(1);
    Some(BlameEntry {
        sha: sha.to_string(),
        range: start..start + count,
        original_line_number,
        author: None,
        author_mail: None,
        author_time: None,
        author_tz: None,
        committer_time: None,
        summary: None,
        previous: None,
        filename: String::new(),
        boundary: false,
    })
}

// ============================================================================
// History graph (Feature 1: Source Control "History" view)
//
// Mirrors Zed's `crates/git_ui` commit list: paginated `git log`, decoded ref
// badges, and a lane/merge graph computed from each commit's parents.
// ============================================================================

/// The kind of ref decorating a commit, driving its badge color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    Head,
    LocalBranch,
    RemoteBranch,
    Tag,
}

/// A branch/tag badge shown next to a commit in the history graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitRef {
    pub name: String,
    pub kind: RefKind,
}

/// One row of `git log`, with parents (for the graph) and ref badges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub short_sha: String,
    pub parents: Vec<String>,
    pub author: String,
    pub author_email: String,
    /// Unix timestamp of the author date.
    pub author_time: i64,
    /// Pre-formatted relative age (git's `%ar`, e.g. "3 days ago").
    pub relative_time: String,
    pub subject: String,
    pub refs: Vec<CommitRef>,
}

/// Field and record separators used by our `git log` format string. Both are
/// control characters that cannot appear in commit metadata, so parsing is
/// unambiguous even for messages containing newlines or tabs.
const LOG_FIELD_SEP: char = '\x1f';
const LOG_RECORD_SEP: char = '\x1e';

/// Page through `git log`. `skip`/`limit` map to `--skip`/`--max-count`, so a
/// virtualized list can lazily load a repo of any size (Zed paginates the
/// same way). When `all` is true, `--all` includes every branch.
pub fn commit_log(root: &Path, skip: usize, limit: usize, all: bool) -> Vec<Commit> {
    let format = format!(
        "--pretty=format:%H{f}%P{f}%an{f}%ae{f}%at{f}%ar{f}%s{f}%D{r}",
        f = LOG_FIELD_SEP,
        r = LOG_RECORD_SEP,
    );
    let skip_arg = format!("--skip={skip}");
    let max_arg = format!("--max-count={limit}");
    let mut args: Vec<&str> = vec!["log", &format, &skip_arg, &max_arg];
    if all {
        args.push("--all");
    }
    args.push("--date-order");
    match run_git(root, &args) {
        Some((out, true)) => parse_log(&out),
        _ => Vec::new(),
    }
}

/// Parse the record-separated `git log` output produced by [`commit_log`].
pub fn parse_log(raw: &str) -> Vec<Commit> {
    raw.split(LOG_RECORD_SEP)
        .map(|record| record.trim_start_matches(['\n', '\r']))
        .filter(|record| !record.is_empty())
        .filter_map(|record| {
            let mut fields = record.split(LOG_FIELD_SEP);
            let sha = fields.next()?.trim();
            if sha.is_empty() {
                return None;
            }
            let parents_raw = fields.next().unwrap_or("");
            let author = fields.next().unwrap_or("").to_string();
            let author_email = fields.next().unwrap_or("").to_string();
            let author_time = fields.next().unwrap_or("").trim().parse().unwrap_or(0);
            let relative_time = fields.next().unwrap_or("").to_string();
            let subject = fields.next().unwrap_or("").to_string();
            let decoration = fields.next().unwrap_or("");
            let parents = parents_raw
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>();
            Some(Commit {
                sha: sha.to_string(),
                short_sha: sha.chars().take(7).collect(),
                parents,
                author,
                author_email,
                author_time,
                relative_time,
                subject,
                refs: parse_refs(decoration),
            })
        })
        .collect()
}

/// Parse the `%D` ref decoration, e.g.
/// `HEAD -> main, origin/main, tag: v1.0, origin/HEAD`.
fn parse_refs(decoration: &str) -> Vec<CommitRef> {
    let mut refs = Vec::new();
    for raw in decoration.split(',') {
        let name = raw.trim();
        if name.is_empty() {
            continue;
        }
        if let Some(tag) = name.strip_prefix("tag: ") {
            refs.push(CommitRef {
                name: tag.trim().to_string(),
                kind: RefKind::Tag,
            });
            continue;
        }
        if let Some((_head, branch)) = name.split_once(" -> ") {
            // `HEAD -> main`: record HEAD and the branch it points at.
            refs.push(CommitRef {
                name: branch.trim().to_string(),
                kind: RefKind::Head,
            });
            continue;
        }
        if name == "HEAD" {
            refs.push(CommitRef {
                name: "HEAD".to_string(),
                kind: RefKind::Head,
            });
            continue;
        }
        // Remote-tracking refs are `origin/…`; local branches have no slash.
        let kind = if name.contains('/') {
            RefKind::RemoteBranch
        } else {
            RefKind::LocalBranch
        };
        refs.push(CommitRef {
            name: name.to_string(),
            kind,
        });
    }
    refs
}

/// A single row of the rendered commit graph: which lane the commit's node
/// sits in, and the lanes passing through above (incoming) and below
/// (outgoing) it, so the UI can draw vertical connectors and merge diagonals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphRow {
    /// Lane (column) index of this commit's node.
    pub column: usize,
    /// A stable color bucket for the node, derived from its lane.
    pub color: usize,
    /// SHA each lane is waiting for, just above this row.
    pub lanes_in: Vec<Option<String>>,
    /// SHA each lane is waiting for, just below this row.
    pub lanes_out: Vec<Option<String>>,
}

impl GraphRow {
    /// Number of lanes to reserve horizontal space for on this row.
    pub fn width(&self) -> usize {
        self.lanes_in.len().max(self.lanes_out.len()).max(1)
    }
}

/// Assign lanes/colors to an ordered (newest-first) commit list, producing one
/// [`GraphRow`] per commit. A classic single-pass lane allocator: each lane
/// remembers the SHA it is next expecting; a commit takes the first lane
/// waiting for it (or a fresh one), then hands its first parent back to that
/// lane and opens new lanes for any additional (merge) parents.
pub fn compute_graph(commits: &[Commit]) -> Vec<GraphRow> {
    // `lanes[i] == Some(sha)` means lane i is waiting to place commit `sha`.
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());

    for commit in commits {
        let lanes_in = lanes.clone();

        // The lane already waiting for this commit, if any.
        let column = match lanes.iter().position(|l| l.as_deref() == Some(&commit.sha)) {
            Some(ix) => ix,
            None => {
                // No lane expected us (a branch tip): use the first free lane.
                match lanes.iter().position(Option::is_none) {
                    Some(ix) => {
                        lanes[ix] = Some(commit.sha.clone());
                        ix
                    }
                    None => {
                        lanes.push(Some(commit.sha.clone()));
                        lanes.len() - 1
                    }
                }
            }
        };

        // Any *other* lanes also waiting for this same commit collapse into
        // `column` (two branches merging back to a shared ancestor).
        for lane in lanes.iter_mut() {
            if lane.as_deref() == Some(&commit.sha) {
                *lane = None;
            }
        }

        // The commit's first parent continues in this commit's lane.
        let mut parents = commit.parents.iter();
        lanes[column] = parents.next().cloned();

        // Extra parents (merge commits) each need a lane. Reuse a lane already
        // waiting for that parent, else the first free lane, else a new one.
        for parent in parents {
            if lanes.iter().any(|l| l.as_deref() == Some(parent.as_str())) {
                continue;
            }
            match lanes.iter().position(Option::is_none) {
                Some(ix) => lanes[ix] = Some(parent.clone()),
                None => lanes.push(Some(parent.clone())),
            }
        }

        // Trim trailing empty lanes so the graph stays as narrow as possible.
        while matches!(lanes.last(), Some(None)) {
            lanes.pop();
        }

        rows.push(GraphRow {
            column,
            color: column,
            lanes_in,
            lanes_out: lanes.clone(),
        });
    }

    rows
}

/// A file touched by a commit, with its change letter (from `--name-status`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitFile {
    pub path: String,
    pub old_path: Option<String>,
    pub kind: ChangeKind,
}

/// Full details for one commit, loaded lazily when a commit is opened/hovered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitDetails {
    pub sha: String,
    pub short_sha: String,
    pub author: String,
    pub author_email: String,
    pub author_time: i64,
    /// Committer timestamp (unix seconds). Zed's blame popover dates
    /// the commit with the committer time, not the author time.
    pub committer_time: i64,
    /// Pre-formatted absolute date (git's `%ad`).
    pub date: String,
    pub subject: String,
    pub body: String,
    pub files: Vec<CommitFile>,
    /// Remote avatar URL for the author (Zed's blame popover),
    /// built from the repo's GitHub remote and the author email.
    pub avatar_url: Option<String>,
}

/// Load the message, author, date and touched files for one commit. Runs on a
/// background thread; results are cached by SHA in `crate::git_blame`.
pub fn commit_details(root: &Path, sha: &str) -> Option<CommitDetails> {
    let format = format!(
        "--pretty=format:%H{f}%an{f}%ae{f}%at{f}%ct{f}%ad{f}%s{f}%b{r}",
        f = LOG_FIELD_SEP,
        r = LOG_RECORD_SEP,
    );
    let (raw, ok) = run_git(
        root,
        &[
            "show",
            "--no-color",
            "--name-status",
            "--date=format:%Y-%m-%d %H:%M",
            &format,
            sha,
        ],
    )?;
    if !ok {
        return None;
    }
    let mut details = parse_commit_details(&raw)?;
    // Zed's blame popover shows the commit author's hosting-provider
    // avatar. The fast path is GitHub's avatar CDN, keyed on the
    // author email, so it needs no API call.
    details.avatar_url =
        remote_url(root).and_then(|remote| avatar_url(&remote, &details.author_email));
    Some(details)
}

/// The URL of the repo's `origin` remote (falling back to its first
/// remote), cached per repo root for the process lifetime. Runs `git
/// remote get-url` on the calling thread, so callers should prefer a
/// background thread; the cache makes repeat calls free.
pub fn remote_url(root: &Path) -> Option<String> {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};
    static CACHE: LazyLock<Mutex<HashMap<PathBuf, Option<String>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    let Ok(mut cache) = CACHE.lock() else {
        return None;
    };
    if let Some(url) = cache.get(root) {
        return url.clone();
    }
    let url = run_git(root, &["remote", "get-url", "origin"])
        .filter(|(_, ok)| *ok)
        .map(|(out, _)| out.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            // No `origin`: use the first remote, if any.
            let (out, ok) = run_git(root, &["remote"])?;
            if !ok {
                return None;
            }
            let name = out.lines().next()?.trim();
            if name.is_empty() {
                return None;
            }
            let (url, ok) = run_git(root, &["remote", "get-url", name])?;
            if !ok {
                return None;
            }
            let url = url.trim().to_string();
            (!url.is_empty()).then_some(url)
        });
    cache.insert(root.to_path_buf(), url.clone());
    url
}

/// Build the commit author's avatar URL for a hosting-provider remote,
/// exactly as Zed's GitHub provider does: GitHub's avatar CDN resolves
/// an avatar from the author's email with no API round-trip. Returns
/// `None` for non-GitHub remotes, missing emails and GitHub's
/// `[bot]@users.noreply.github.com` addresses (Zed skips those too).
pub fn avatar_url(remote_url: &str, author_email: &str) -> Option<String> {
    // Handle every remote spelling: `https://github.com/o/r`,
    // `ssh://git@github.com/o/r` and the scp-like `git@github.com:o/r`.
    let authority = remote_url.split("://").nth(1).unwrap_or(remote_url);
    let host = authority
        .rsplit('@')
        .next()
        .unwrap_or(authority)
        .split(|c| c == '/' || c == ':')
        .next()
        .unwrap_or("");
    if host != "github.com" {
        return None;
    }
    let email = author_email
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim();
    if email.is_empty() || email.ends_with("[bot]@users.noreply.github.com") {
        return None;
    }
    Some(format!(
        "https://avatars.githubusercontent.com/u/e?email={}&s=128",
        url::form_urlencoded::byte_serialize(email.as_bytes()).collect::<String>()
    ))
}

/// Parse the output of the `git show --name-status` invocation in
/// [`commit_details`]: a header record, the record separator, then one
/// name-status line per changed file.
pub fn parse_commit_details(raw: &str) -> Option<CommitDetails> {
    let (header, rest) = raw.split_once(LOG_RECORD_SEP)?;
    let mut fields = header.split(LOG_FIELD_SEP);
    let sha = fields.next()?.trim().to_string();
    let author = fields.next().unwrap_or("").to_string();
    let author_email = fields.next().unwrap_or("").to_string();
    let author_time = fields.next().unwrap_or("").trim().parse().unwrap_or(0);
    let committer_time = fields.next().unwrap_or("").trim().parse().unwrap_or(0);
    let date = fields.next().unwrap_or("").to_string();
    let subject = fields.next().unwrap_or("").to_string();
    let body = fields.next().unwrap_or("").trim().to_string();

    let files = rest
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter_map(parse_name_status)
        .collect();

    Some(CommitDetails {
        short_sha: sha.chars().take(7).collect(),
        sha,
        author,
        author_email,
        author_time,
        committer_time,
        date,
        subject,
        body,
        files,
        avatar_url: None,
    })
}

/// Parse one `git ... --name-status` line, e.g. `M\tsrc/a.rs` or
/// `R100\told.rs\tnew.rs`.
fn parse_name_status(line: &str) -> Option<CommitFile> {
    let mut fields = line.split('\t');
    let status = fields.next()?;
    let letter = status.chars().next()?;
    let kind = match letter {
        'M' => ChangeKind::Modified,
        'A' => ChangeKind::Added,
        'D' => ChangeKind::Deleted,
        'R' => ChangeKind::Renamed,
        'C' => ChangeKind::Copied,
        'T' => ChangeKind::TypeChanged,
        _ => return None,
    };
    if matches!(kind, ChangeKind::Renamed | ChangeKind::Copied) {
        let old_path = fields.next()?.to_string();
        let path = fields.next()?.to_string();
        Some(CommitFile {
            path,
            old_path: Some(old_path),
            kind,
        })
    } else {
        let path = fields.next()?.to_string();
        Some(CommitFile {
            path,
            old_path: None,
            kind,
        })
    }
}

/// Unified diff for one commit (`git show <sha>`), reusing the existing diff
/// parser for the commit-details view.
pub fn commit_diff(root: &Path, sha: &str) -> Option<String> {
    run_git(
        root,
        &[
            "show",
            "--no-ext-diff",
            "--no-color",
            "--unified=3",
            "--format=",
            sha,
        ],
    )
    .map(|(out, _)| out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> &'static Path {
        Path::new("/repo")
    }

    #[test]
    fn builds_github_avatar_urls() {
        let expected = "https://avatars.githubusercontent.com/u/e?email=joe%40example.com&s=128";

        // https, ssh:// and scp-like remotes all resolve to the same avatar.
        for remote in [
            "https://github.com/zed-industries/zed.git",
            "ssh://git@github.com/zed-industries/zed.git",
            "git@github.com:zed-industries/zed.git",
        ] {
            assert_eq!(avatar_url(remote, "joe@example.com").as_deref(), Some(expected));
        }

        // Non-GitHub remotes have no avatar (Zed falls back to initials).
        assert!(avatar_url("https://gitlab.com/a/b.git", "joe@example.com").is_none());

        // GitHub's noreply bot addresses and empty emails are skipped.
        assert!(avatar_url("https://github.com/a/b.git", "dependabot[bot]@users.noreply.github.com").is_none());
        assert!(avatar_url("https://github.com/a/b.git", "").is_none());
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

    // -- Blame -------------------------------------------------------------

    const SHA_A: &str = "6ad46b5257ba16d12c5ca9f0d4900320959df7f4";
    const SHA_B: &str = "486c2409237a2c627230589e567024a96751d475";

    #[test]
    fn parses_incremental_blame_entry() {
        let raw = format!(
            "{SHA_A} 2 2 1\n\
             author Joe Schmoe\n\
             author-mail <joe@example.com>\n\
             author-time 1709741400\n\
             author-tz +0100\n\
             committer Joe Schmoe\n\
             committer-time 1709741400\n\
             summary Joe's cool commit\n\
             previous {SHA_B} index.js\n\
             filename index.js\n"
        );
        let entries = parse_blame_incremental(&raw);
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.sha, SHA_A);
        // final line 2, 1 line -> row range 1..2 (0-based)
        assert_eq!(e.range, 1..2);
        assert_eq!(e.original_line_number, 2);
        assert_eq!(e.author.as_deref(), Some("Joe Schmoe"));
        assert_eq!(e.author_time, Some(1709741400));
        assert_eq!(e.summary.as_deref(), Some("Joe's cool commit"));
        assert_eq!(e.filename, "index.js");
        assert!(!e.is_uncommitted());
        assert_eq!(e.short_sha(), "6ad46b5");
    }

    #[test]
    fn copies_signature_forward_for_repeated_sha() {
        // Second entry for the same SHA omits author/summary; the parser must
        // carry them forward, exactly like Zed's GitBlameParser.
        let raw = format!(
            "{SHA_A} 1 1 1\n\
             author Ada\n\
             author-time 100\n\
             summary first\n\
             filename a.rs\n\
             {SHA_A} 3 4 2\n\
             previous {SHA_B} a.rs\n\
             filename a.rs\n"
        );
        let entries = parse_blame_incremental(&raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].sha, SHA_A);
        assert_eq!(entries[1].range, 3..5);
        assert_eq!(entries[1].author.as_deref(), Some("Ada"));
        assert_eq!(entries[1].summary.as_deref(), Some("first"));
    }

    #[test]
    fn marks_uncommitted_lines() {
        let zero = "0".repeat(40);
        let raw = format!(
            "{zero} 1 1 1\n\
             author Not Committed Yet\n\
             author-time 0\n\
             summary Version of ... not committed\n\
             filename a.rs\n"
        );
        let entries = parse_blame_incremental(&raw);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_uncommitted());
        assert_eq!(entries[0].author.as_deref(), Some("Not Committed Yet"));
    }

    #[test]
    fn skips_non_header_garbage() {
        // A line that is not a valid header must not start an entry.
        let raw = "not a header\nrandom line\n";
        assert!(parse_blame_incremental(raw).is_empty());
    }

    #[test]
    fn builds_per_row_index_from_entries() {
        let raw = format!(
            "{SHA_A} 1 1 2\nauthor A\nauthor-time 1\nsummary s\nfilename a.rs\n\
             {SHA_B} 1 3 1\nauthor B\nauthor-time 2\nsummary t\nfilename a.rs\n"
        );
        let blame = FileBlame::from_entries(parse_blame_incremental(&raw));
        assert_eq!(blame.rows.len(), 3);
        assert_eq!(blame.line(0).unwrap().sha, SHA_A);
        assert_eq!(blame.line(1).unwrap().sha, SHA_A);
        assert_eq!(blame.line(2).unwrap().sha, SHA_B);
        assert!(blame.line(3).is_none());
    }

    #[test]
    fn shifts_rows_on_insert() {
        // rows: [A, A, B] — insert 2 new lines at row 1 (typed, no blame yet).
        let mut blame = FileBlame {
            entries: vec![],
            rows: vec![Some(0), Some(0), Some(1)],
        };
        blame.shift(1, 0, 2);
        assert_eq!(
            blame.rows,
            vec![Some(0), None, None, Some(0), Some(1)],
            "inserted rows are blank, rows below keep their commit"
        );
    }

    #[test]
    fn shifts_rows_on_delete() {
        let mut blame = FileBlame {
            entries: vec![],
            rows: vec![Some(0), Some(1), Some(2), Some(3)],
        };
        // Delete 2 rows starting at row 1.
        blame.shift(1, 2, 0);
        assert_eq!(blame.rows, vec![Some(0), Some(3)]);
    }

    #[test]
    fn shifts_rows_on_replace() {
        let mut blame = FileBlame {
            entries: vec![],
            rows: vec![Some(0), Some(1), Some(2)],
        };
        // Replace row 1 (1 removed, 1 added).
        blame.shift(1, 1, 1);
        assert_eq!(blame.rows, vec![Some(0), None, Some(2)]);
    }

    // -- History log + graph ----------------------------------------------

    #[test]
    fn parses_log_records() {
        let raw = format!(
            "{a}\x1f{b} {c}\x1fAda\x1fada@x.io\x1f1700000000\x1f2 days ago\x1fInit commit\x1fHEAD -> main, tag: v1.0, origin/main\x1e\n\
             {b}\x1f\x1fLin\x1flin@x.io\x1f1600000000\x1f3 days ago\x1fRoot\x1f\x1e",
            a = "a".repeat(40),
            b = "b".repeat(40),
            c = "c".repeat(40),
        );
        let commits = parse_log(&raw);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].sha, "a".repeat(40));
        assert_eq!(commits[0].short_sha, "aaaaaaa");
        assert_eq!(commits[0].parents, vec!["b".repeat(40), "c".repeat(40)]);
        assert_eq!(commits[0].author, "Ada");
        assert_eq!(commits[0].author_time, 1700000000);
        assert_eq!(commits[0].relative_time, "2 days ago");
        assert_eq!(commits[0].subject, "Init commit");
        assert_eq!(commits[0].refs.len(), 3);
        assert_eq!(commits[0].refs[0].kind, RefKind::Head);
        assert_eq!(commits[0].refs[0].name, "main");
        assert_eq!(commits[0].refs[1].kind, RefKind::Tag);
        assert_eq!(commits[0].refs[1].name, "v1.0");
        assert_eq!(commits[0].refs[2].kind, RefKind::RemoteBranch);
        // Root commit: no parents, no refs.
        assert!(commits[1].parents.is_empty());
        assert!(commits[1].refs.is_empty());
    }

    fn commit(sha: &str, parents: &[&str]) -> Commit {
        Commit {
            sha: sha.to_string(),
            short_sha: sha.chars().take(7).collect(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            author: String::new(),
            author_email: String::new(),
            author_time: 0,
            relative_time: String::new(),
            subject: String::new(),
            refs: Vec::new(),
        }
    }

    #[test]
    fn linear_history_is_single_lane() {
        let commits = vec![
            commit("c", &["b"]),
            commit("b", &["a"]),
            commit("a", &[]),
        ];
        let rows = compute_graph(&commits);
        assert_eq!(rows.len(), 3);
        for row in &rows {
            assert_eq!(row.column, 0, "linear history stays in lane 0");
        }
        // The tip's outgoing lane waits for its parent.
        assert_eq!(rows[0].lanes_out, vec![Some("b".to_string())]);
        // The root has no outgoing lanes.
        assert!(rows[2].lanes_out.is_empty());
    }

    #[test]
    fn merge_commit_opens_a_second_lane() {
        // m merges b (feature) into a (main); both descend from r.
        let commits = vec![
            commit("m", &["a", "b"]),
            commit("a", &["r"]),
            commit("b", &["r"]),
            commit("r", &[]),
        ];
        let rows = compute_graph(&commits);
        assert_eq!(rows[0].column, 0);
        // After the merge, two lanes are active (waiting for a and b).
        assert_eq!(rows[0].lanes_out.len(), 2);
        assert_eq!(rows[0].lanes_out[0], Some("a".to_string()));
        assert_eq!(rows[0].lanes_out[1], Some("b".to_string()));
        // b sits in lane 1.
        assert_eq!(rows[2].column, 1);
        // Once both branches reach r, the graph collapses back to one lane.
        assert_eq!(rows[3].column, 0);
        assert!(rows[3].lanes_out.is_empty());
    }

    #[test]
    fn parses_commit_details_with_files() {
        let raw = format!(
            "{a}\x1fAda\x1fada@x.io\x1f1700000000\x1f1700000123\x1f2024-03-01 10:00\x1fFix bug\x1fLonger body\x1e\n\
             M\tsrc/a.rs\n\
             A\tsrc/b.rs\n\
             R100\told.rs\tnew.rs\n",
            a = "a".repeat(40),
        );
        let details = parse_commit_details(&raw).unwrap();
        assert_eq!(details.short_sha, "aaaaaaa");
        assert_eq!(details.author, "Ada");
        assert_eq!(details.author_time, 1700000000);
        assert_eq!(details.committer_time, 1700000123);
        assert_eq!(details.date, "2024-03-01 10:00");
        assert_eq!(details.subject, "Fix bug");
        assert_eq!(details.body, "Longer body");
        assert_eq!(details.files.len(), 3);
        assert_eq!(details.files[0].kind, ChangeKind::Modified);
        assert_eq!(details.files[0].path, "src/a.rs");
        assert_eq!(details.files[2].kind, ChangeKind::Renamed);
        assert_eq!(details.files[2].old_path.as_deref(), Some("old.rs"));
        assert_eq!(details.files[2].path, "new.rs");
    }
}

/// Integration tests that drive a real, throwaway git repository through the
/// history + blame service. Skipped automatically when the `git` binary is not
/// available (e.g. a minimal CI image) so the suite still passes.
#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Guard that removes the temp repo on drop, even if an assertion panics.
    struct TempRepo {
        path: PathBuf,
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Run a git command in `dir`, with a hermetic identity and no global/system
    /// config bleeding in, and assert it succeeded.
    fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Ada Lovelace")
            .env("GIT_AUTHOR_EMAIL", "ada@example.com")
            .env("GIT_COMMITTER_NAME", "Ada Lovelace")
            .env("GIT_COMMITTER_EMAIL", "ada@example.com")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_DATE", "2025-01-01T00:00:00")
            .env("GIT_COMMITTER_DATE", "2025-01-01T00:00:00")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("spawn git");
        assert!(status.success(), "git {args:?} failed");
    }

    fn write(dir: &Path, rel: &str, contents: &str) {
        std::fs::write(dir.join(rel), contents).expect("write file");
    }

    /// Build a repo with a merge, a tag and several commits so the graph, refs
    /// and pagination all have something to chew on.
    fn make_repo() -> TempRepo {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("ezicode-git-it-{}-{nanos}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create temp dir");
        let repo = TempRepo { path };
        let dir = &repo.path;

        run(dir, &["-c", "init.defaultBranch=main", "init", "-q"]);
        // Ensure we are on `main` even on git versions predating -b/defaultBranch.
        run(dir, &["symbolic-ref", "HEAD", "refs/heads/main"]);

        write(dir, "a.txt", "line one\nline two\nline three\n");
        run(dir, &["add", "a.txt"]);
        run(dir, &["commit", "-q", "-m", "first commit"]);

        write(dir, "a.txt", "line one\nline two changed\nline three\n");
        run(dir, &["commit", "-qam", "second commit"]);

        // Diverge onto a feature branch touching a *different* file so the merge
        // stays clean.
        run(dir, &["checkout", "-q", "-b", "feature"]);
        write(dir, "b.txt", "feature file\n");
        run(dir, &["add", "b.txt"]);
        run(dir, &["commit", "-q", "-m", "feature commit"]);

        run(dir, &["checkout", "-q", "main"]);
        write(dir, "c.txt", "main file\n");
        run(dir, &["add", "c.txt"]);
        run(dir, &["commit", "-q", "-m", "third commit"]);

        run(dir, &["merge", "--no-ff", "-q", "-m", "merge feature", "feature"]);
        run(dir, &["tag", "v1.0"]);

        repo
    }

    #[test]
    fn history_log_graph_blame_and_details() {
        if !git_available() {
            eprintln!("skipping: git binary not available");
            return;
        }
        let repo = make_repo();
        let root = repo.path.clone();

        // --- commit_log: newest first, well-formed fields ---
        let commits = commit_log(&root, 0, 100, false);
        assert!(
            commits.len() >= 5,
            "expected >=5 commits, got {}",
            commits.len()
        );
        // Newest commit is the merge.
        assert_eq!(commits[0].subject, "merge feature");
        assert_eq!(commits[0].parents.len(), 2, "merge has two parents");
        for c in &commits {
            assert_eq!(c.short_sha.len(), 7, "short sha is 7 chars");
            assert!(c.sha.starts_with(c.short_sha.as_str()));
            assert_eq!(c.author, "Ada Lovelace");
            assert!(!c.relative_time.is_empty());
        }

        // HEAD/branch/tag refs are decoded onto the tip commits.
        assert!(
            commits.iter().any(|c| !c.refs.is_empty()),
            "at least one commit carries a ref badge"
        );
        assert!(
            commits
                .iter()
                .any(|c| c.refs.iter().any(|r| r.kind == RefKind::Tag)),
            "the v1.0 tag is decoded"
        );

        // --- pagination: page size and skip both honoured ---
        let page = commit_log(&root, 0, 2, false);
        assert_eq!(page.len(), 2);
        let next = commit_log(&root, 2, 2, false);
        assert_eq!(next.len(), 2);
        assert_ne!(page[0].sha, next[0].sha, "skip advances the window");
        assert_eq!(page[0].sha, commits[0].sha);
        assert_eq!(next[0].sha, commits[2].sha);

        // --- compute_graph: one row per commit, merge widens the lanes ---
        let graph = compute_graph(&commits);
        assert_eq!(graph.len(), commits.len());
        assert!(
            graph.iter().any(|row| row.width() >= 2),
            "the merge produces at least two lanes"
        );

        // --- commit_details: message + touched files ---
        let details = commit_details(&root, &commits[0].sha).expect("merge details");
        assert_eq!(details.subject, "merge feature");
        assert_eq!(details.author, "Ada Lovelace");
        // The second (non-merge) commit changed a.txt.
        let second = commits
            .iter()
            .find(|c| c.subject == "second commit")
            .expect("second commit present");
        let second_details = commit_details(&root, &second.sha).expect("second details");
        assert!(
            second_details.files.iter().any(|f| f.path == "a.txt"),
            "second commit touched a.txt"
        );

        // --- commit_diff: a real unified diff ---
        let diff = commit_diff(&root, &second.sha).expect("commit diff");
        assert!(diff.contains("diff --git"), "diff has a git header");
        assert!(diff.contains("a.txt"), "diff mentions the changed file");

        // --- blame: one entry per line, attributed to the author ---
        let blame = blame_file(&root, "a.txt", None).expect("blame a.txt");
        assert_eq!(blame.rows.len(), 3, "a.txt has three lines");
        for slot in &blame.rows {
            let entry = slot.and_then(|ix| blame.entries.get(ix)).expect("row blamed");
            assert_eq!(entry.author.as_deref().unwrap_or_default(), "Ada Lovelace");
            assert!(!entry.is_uncommitted());
        }
    }

    #[test]
    fn blame_shift_tracks_edits() {
        if !git_available() {
            return;
        }
        let repo = make_repo();
        let mut blame = blame_file(&repo.path, "a.txt", None).expect("blame");
        let original = blame.rows.len();
        // Insert two lines at row 1: the rows below shift down and the inserted
        // rows have no blame yet.
        blame.shift(1, 0, 2);
        assert_eq!(blame.rows.len(), original + 2);
        assert!(blame.rows[1].is_none());
        assert!(blame.rows[2].is_none());
        assert!(blame.rows[0].is_some());
    }
}
