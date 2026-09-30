//! Zed-style inline git blame orchestration (Feature 2).
//!
//! The pure git work lives in [`crate::git`] (`blame --incremental` parsing,
//! offset shifting). This module is the *state + cache* layer that drives it
//! from the [`Workspace`]: it computes blame off the UI thread, caches it per
//! file, shifts it on edits, invalidates it on save / branch switch / commit /
//! external change, and pushes pre-formatted annotations into each editor's
//! [`InputState`]. All rendering happens in the vendored input widget.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{Context, Entity};
use gpui_component::input::{BlameDetail, BlameLine, InputState};

use crate::git;

use super::Workspace;

/// Files larger than this are never blamed (matches the spirit of Zed skipping
/// blame for very large buffers so typing/scrolling never stalls).
const MAX_BLAME_BYTES: usize = 2_000_000;

/// Asset path of the small git icon drawn before each inline annotation.
const BLAME_ICON: &str = "ui_icons/git_branch.svg";

impl Workspace {
    /// Whether blame should be shown at all right now (inline setting on, or the
    /// gutter toggle active).
    fn blame_wanted(&self) -> bool {
        self.settings.git.inline_blame.enabled || self.git_blame_gutter
    }

    /// Toggle the "Toggle Git Blame" gutter view for every open editor (Zed's
    /// editor-wide `git::Blame`). Ensures blame is computed for the active file.
    pub(crate) fn toggle_git_blame(&mut self, cx: &mut Context<Self>) {
        self.git_blame_gutter = !self.git_blame_gutter;
        let on = self.git_blame_gutter;
        let editors: Vec<Entity<InputState>> =
            self.tabs.iter().filter_map(|t| t.editor.clone()).collect();
        for ed in editors {
            ed.update(cx, |state, cx| state.set_blame_gutter(on, cx));
        }
        self.status = if on {
            "Git blame: on".into()
        } else {
            "Git blame: off".into()
        };
        self.recompute_active_blame(cx);
        cx.notify();
    }

    /// Recompute blame for the currently active editor (no debounce). Called on
    /// file open, save, git status change and the blame toggle.
    pub(crate) fn recompute_active_blame(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let (Some(path), Some(editor)) = (tab.path.clone(), tab.editor.clone()) else {
            return;
        };
        self.schedule_blame(path, editor, 0, cx);
    }

    /// React to an edit in `editor`: shift the cached blame so annotations stay
    /// aligned, then debounce a fresh blame (mirrors Zed's shift-then-reblame).
    pub(crate) fn on_editor_blame_change(
        &mut self,
        path: &Path,
        editor: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) {
        if !self.blame_wanted() {
            return;
        }
        // Coarse offset shift at the cursor line, keyed on the line-count delta,
        // so existing annotations don't drift while the debounce is pending.
        let (cursor_row, new_lines) = {
            let ed = editor.read(cx);
            (
                ed.cursor_position().line as usize,
                ed.text().len_lines(),
            )
        };
        if let Some(blame) = self.git_blame_cache.get_mut(path) {
            let old_lines = blame.rows.len();
            if new_lines > old_lines {
                let added = new_lines - old_lines;
                blame.shift(cursor_row, 0, added);
                editor.update(cx, |state, _| {
                    state.shift_inline_blame(cursor_row, 0, added)
                });
            } else if old_lines > new_lines {
                let removed = old_lines - new_lines;
                blame.shift(cursor_row, removed, 0);
                editor.update(cx, |state, _| {
                    state.shift_inline_blame(cursor_row, removed, 0)
                });
            }
        }
        self.schedule_blame(path.to_path_buf(), editor.clone(), 250, cx);
    }

    /// Invalidate every cached blame (branch switch / commit / pull / external
    /// change) and recompute for the active editor.
    pub(crate) fn invalidate_blame(&mut self, cx: &mut Context<Self>) {
        self.git_blame_cache.clear();
        // Bump the generation so any in-flight request is dropped on return.
        self.git_blame_generation = self.git_blame_generation.wrapping_add(1);
        self.recompute_active_blame(cx);
    }

    /// Compute blame for `path` after `delay_ms`, off the UI thread, and apply
    /// it to `editor`. Cancels stale requests via the blame generation counter.
    fn schedule_blame(
        &mut self,
        path: PathBuf,
        editor: Entity<InputState>,
        delay_ms: u64,
        cx: &mut Context<Self>,
    ) {
        if !self.blame_wanted() {
            editor.update(cx, |state, cx| state.clear_inline_blame(cx));
            return;
        }
        let Some(root) = git::find_repo_root(&path) else {
            editor.update(cx, |state, cx| state.clear_inline_blame(cx));
            return;
        };
        let Ok(rel) = path.strip_prefix(&root) else {
            return;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");

        let text = editor.read(cx).text().to_string();
        if text.len() > MAX_BLAME_BYTES || text.as_bytes().iter().take(8000).any(|&b| b == 0) {
            // Too large, or binary: never annotate (and never block).
            editor.update(cx, |state, cx| state.clear_inline_blame(cx));
            return;
        }

        self.git_blame_generation = self.git_blame_generation.wrapping_add(1);
        let generation = self.git_blame_generation;

        cx.spawn(async move |this, cx| {
            if delay_ms > 0 {
                cx.background_executor()
                    .timer(Duration::from_millis(delay_ms))
                    .await;
            }
            let blame = cx
                .background_spawn(async move { git::blame_file(&root, &rel, Some(&text)) })
                .await;
            let _ = this.update(cx, |ws, cx| {
                if ws.git_blame_generation != generation {
                    // Superseded by a newer request (file changed / closed).
                    return;
                }
                match blame {
                    Some(blame) => {
                        ws.git_blame_cache.insert(path.clone(), blame);
                        ws.apply_cached_blame(&path, &editor, cx);
                    }
                    None => {
                        // Untracked / out-of-repo / git error: stay silent.
                        editor.update(cx, |state, cx| state.clear_inline_blame(cx));
                    }
                }
            });
        })
        .detach();
    }

    /// The mouse hovered a blame annotation for `sha`: resolve the commit
    /// detail (cached by SHA, fetched off-thread on the first hover) and push it
    /// into the editor's popover. Uncommitted (empty-sha) lines have no popover.
    pub(crate) fn on_blame_hover(
        &mut self,
        sha: &str,
        editor: Entity<InputState>,
        cx: &mut Context<Self>,
    ) {
        if sha.is_empty() {
            return;
        }
        let sha = sha.to_string();

        // Fast path: already cached.
        if let Some(details) = self
            .commit_detail_cache
            .lock()
            .ok()
            .and_then(|c| c.get(&sha).cloned())
        {
            let detail = to_blame_detail(&details);
            editor.update(cx, |state, cx| state.set_blame_detail(Some(detail), cx));
            return;
        }

        let Some(root) = self.git.as_ref().map(|g| g.root.clone()) else {
            return;
        };
        let cache = self.commit_detail_cache.clone();
        let sha_bg = sha.clone();

        cx.spawn(async move |_this, cx| {
            let details = cx
                .background_spawn(async move { git::commit_details(&root, &sha_bg) })
                .await;
            if let Some(details) = details {
                if let Ok(mut cache) = cache.lock() {
                    cache.insert(sha.clone(), details.clone());
                }
                let detail = to_blame_detail(&details);
                let _ = editor.update(cx, |state, cx| state.set_blame_detail(Some(detail), cx));
            }
        })
        .detach();
    }

    /// The mouse left the blame annotation: hide the popover.
    pub(crate) fn on_blame_hover_end(
        &mut self,
        editor: Entity<InputState>,
        cx: &mut Context<Self>,
    ) {
        editor.update(cx, |state, cx| state.set_blame_detail(None, cx));
    }

    /// Push the cached blame for `path` into `editor` as pre-formatted lines.
    fn apply_cached_blame(
        &self,
        path: &Path,
        editor: &Entity<InputState>,
        cx: &mut Context<Self>,
    ) {
        let Some(blame) = self.git_blame_cache.get(path) else {
            return;
        };
        let inline = self.settings.git.inline_blame;
        let now = unix_now();
        let lines = blame_annotations(blame, inline.show_commit_summary, now);
        let enabled = inline.enabled;
        let min_column = inline.min_column;
        let gutter = self.git_blame_gutter;
        editor.update(cx, |state, cx| {
            state.set_inline_blame(lines, enabled, min_column, BLAME_ICON, cx);
            state.set_blame_gutter(gutter, cx);
        });
    }
}

/// Build one annotation per row from a [`git::FileBlame`], formatted like Zed's
/// inline blame: `"Author, 3 days ago"` (+ optional `" • summary"`), and
/// `"You, uncommitted changes"` for local edits.
fn blame_annotations(
    blame: &git::FileBlame,
    show_summary: bool,
    now: i64,
) -> Vec<Option<BlameLine>> {
    blame
        .rows
        .iter()
        .map(|slot| {
            let entry = slot.and_then(|ix| blame.entries.get(ix))?;
            let author = entry.author_display();
            let text = if entry.is_uncommitted() {
                format!("{author}, uncommitted changes")
            } else {
                let when = relative_time(entry.author_time.unwrap_or(0), now);
                let mut text = format!("{author}, {when}");
                if show_summary {
                    if let Some(summary) = &entry.summary {
                        if !summary.is_empty() {
                            text.push_str(" • ");
                            text.push_str(summary);
                        }
                    }
                }
                text
            };
            let sha = if entry.is_uncommitted() {
                String::new()
            } else {
                entry.short_sha()
            };
            Some(BlameLine {
                text: text.into(),
                sha: sha.into(),
            })
        })
        .collect()
}

/// Convert git [`CommitDetails`](git::CommitDetails) into the widget's
/// git-agnostic [`BlameDetail`] shown in the hover popover.
fn to_blame_detail(d: &git::CommitDetails) -> BlameDetail {
    let message = if d.body.is_empty() {
        d.subject.clone()
    } else {
        format!("{}\n\n{}", d.subject, d.body)
    };
    let author_display = if d.author.is_empty() {
        "Unknown".to_string()
    } else {
        d.author.clone()
    };
    BlameDetail {
        sha: d.sha.clone().into(),
        short_sha: d.short_sha.clone().into(),
        author: author_display.into(),
        author_email: d.author_email.clone().into(),
        date: d.date.clone().into(),
        message: message.into(),
    }
}

/// Seconds since the Unix epoch, or 0 if the clock is before it.
pub(crate) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A compact "N units ago" string, matching git's relative dates closely enough
/// for the inline annotation.
pub(crate) fn relative_time(then: i64, now: i64) -> String {
    let secs = (now - then).max(0);
    if secs < 45 {
        return "just now".to_string();
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{} minute{} ago", mins.max(1), plural(mins.max(1)));
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{hours} hour{} ago", plural(hours));
    }
    let days = hours / 24;
    if days < 7 {
        return format!("{days} day{} ago", plural(days));
    }
    let weeks = days / 7;
    if weeks < 5 {
        return format!("{weeks} week{} ago", plural(weeks));
    }
    let months = days / 30;
    if months < 12 {
        return format!("{} month{} ago", months.max(1), plural(months.max(1)));
    }
    let years = days / 365;
    format!("{} year{} ago", years.max(1), plural(years.max(1)))
}

fn plural(n: i64) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{BlameEntry, FileBlame};

    fn entry(sha: &str, author: &str, time: i64, summary: &str) -> BlameEntry {
        BlameEntry {
            sha: sha.to_string(),
            range: 0..1,
            original_line_number: 1,
            author: Some(author.to_string()),
            author_mail: None,
            author_time: Some(time),
            author_tz: None,
            committer_time: None,
            summary: Some(summary.to_string()),
            previous: None,
            filename: "a.rs".to_string(),
            boundary: false,
        }
    }

    #[test]
    fn formats_committed_line() {
        let now = 1_000_000;
        let blame = FileBlame {
            entries: vec![entry(&"a".repeat(40), "Ada Lovelace", now - 3 * 86400, "Fix bug")],
            rows: vec![Some(0)],
        };
        let lines = blame_annotations(&blame, false, now);
        let line = lines[0].as_ref().unwrap();
        assert_eq!(line.text.as_ref(), "Ada Lovelace, 3 days ago");
        assert_eq!(line.sha.as_ref(), "aaaaaaaa");
    }

    #[test]
    fn appends_summary_when_requested() {
        let now = 1_000_000;
        let blame = FileBlame {
            entries: vec![entry(&"a".repeat(40), "Ada", now - 3600, "Fix bug")],
            rows: vec![Some(0)],
        };
        let lines = blame_annotations(&blame, true, now);
        assert_eq!(lines[0].as_ref().unwrap().text.as_ref(), "Ada, 1 hour ago • Fix bug");
    }

    #[test]
    fn formats_uncommitted_like_zed() {
        let now = 1_000_000;
        let mut e = entry(&"0".repeat(40), "Not Committed Yet", 0, "");
        e.summary = None;
        let blame = FileBlame {
            entries: vec![e],
            rows: vec![Some(0)],
        };
        let lines = blame_annotations(&blame, false, now);
        let line = lines[0].as_ref().unwrap();
        assert_eq!(line.text.as_ref(), "You, uncommitted changes");
        assert_eq!(line.sha.as_ref(), "");
    }

    #[test]
    fn blank_rows_have_no_annotation() {
        let blame = FileBlame {
            entries: vec![entry(&"a".repeat(40), "Ada", 0, "s")],
            rows: vec![Some(0), None],
        };
        let lines = blame_annotations(&blame, false, 1000);
        assert!(lines[0].is_some());
        assert!(lines[1].is_none());
    }

    #[test]
    fn relative_time_buckets() {
        let now = 10_000_000;
        assert_eq!(relative_time(now, now), "just now");
        assert_eq!(relative_time(now - 3600, now), "1 hour ago");
        assert_eq!(relative_time(now - 2 * 86400, now), "2 days ago");
        assert_eq!(relative_time(now - 14 * 86400, now), "2 weeks ago");
    }
}
