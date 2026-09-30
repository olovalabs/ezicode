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

use gpui::{AppContext as _, Context, Entity};
use gpui_component::input::{BlameDetail, BlameLine, InputState, RopeExt as _};

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
                ed.text().lines_len(),
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
        // Zed waits `REGENERATE_ON_EDIT_DEBOUNCE_INTERVAL` (2s) before
        // re-running blame after an edit.
        self.schedule_blame(path.to_path_buf(), editor.clone(), 2000, cx);
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
        let root = self.git.as_ref().map(|g| g.root.as_path());
        let lines = blame_annotations(blame, root, inline.show_commit_summary, now);
        let enabled = inline.enabled;
        let min_column = inline.min_column;
        let padding = inline.padding;
        let gutter = self.git_blame_gutter;
        editor.update(cx, |state, cx| {
            state.set_inline_blame(lines, enabled, min_column, padding, BLAME_ICON, cx);
            state.set_blame_gutter(gutter, cx);
        });
    }
}

/// Build one annotation per row from a [`git::FileBlame`], formatted like
/// Zed's inline blame: `"Author, 3 days ago"` (+ optional `" - summary"`).
/// Uncommitted lines (zero SHA) produce no annotation at all — Zed's
/// blame parser drops zero-SHA entries, so lines git cannot attribute to
/// a commit yet stay clean while you edit.
fn blame_annotations(
    blame: &git::FileBlame,
    root: Option<&Path>,
    show_summary: bool,
    now: i64,
) -> Vec<Option<BlameLine>> {
    // Resolve the repo's remote once per call; `remote_url` caches it per
    // root, so this is a single `git remote get-url` (at most) per render.
    let remote = root.and_then(git::remote_url);
    blame
        .rows
        .iter()
        .map(|slot| {
            let entry = slot.and_then(|ix| blame.entries.get(ix))?;
            if entry.is_uncommitted() {
                return None;
            }
            let author = entry.author.as_deref().unwrap_or_default();
            let when = relative_time(entry.author_time.unwrap_or(0), now);
            let mut text = format!("{author}, {when}");
            if show_summary {
                if let Some(summary) = &entry.summary {
                    if !summary.is_empty() {
                        text.push_str(" - ");
                        text.push_str(summary);
                    }
                }
            }
            // Zed's `git.blame.show_avatar`: the author's hosting-provider
            // avatar, resolved from their email via the GitHub CDN.
            let avatar_url = entry
                .author_mail
                .as_deref()
                .and_then(|mail| remote.as_deref().and_then(|r| git::avatar_url(r, mail)))
                .unwrap_or_default();
            Some(BlameLine {
                text: text.into(),
                sha: entry.short_sha().into(),
                avatar_url: avatar_url.into(),
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
        // Zed's blame popover shows the author's hosting-provider avatar.
        avatar_url: d.avatar_url.clone().unwrap_or_default().into(),
        // Zed dates the popover with the committer time, not the
        // author time, in a medium absolute format.
        date: absolute_timestamp(d.committer_time).into(),
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

/// Relative timestamp for blame annotations, worded exactly like
/// Zed's `time_format::format_relative_time` / `format_relative_date`:
/// "Just now", "N minutes ago", "N hours ago", then calendar buckets
/// ("Today", "Yesterday", "N days ago", "N weeks ago", "N months ago",
/// "1 year, 2 months ago", "Z years ago"), evaluated in the user's
/// local timezone.
pub(crate) fn relative_time(then: i64, now: i64) -> String {
    let secs = (now - then).max(0);
    let minutes = secs / 60;
    match minutes {
        0 => "Just now".to_string(),
        1 => "1 minute ago".to_string(),
        2..=59 => format!("{minutes} minutes ago"),
        _ => {
            let hours = secs / 3600;
            match hours {
                1 => "1 hour ago".to_string(),
                2..=23 => format!("{hours} hours ago"),
                _ => relative_date(then, now),
            }
        }
    }
}

fn relative_date(then: i64, now: i64) -> String {
    let (ty, tm, td) = local_date(then);
    let (ny, nm, nd) = local_date(now);
    let days = days_from_civil(ny, nm, nd) - days_from_civil(ty, tm, td);
    match days {
        0 => "Today".to_string(),
        1 => "Yesterday".to_string(),
        2..=6 => format!("{days} days ago"),
        _ => {
            let weeks = days / 7;
            match weeks {
                1 => "1 week ago".to_string(),
                2..=4 => format!("{weeks} weeks ago"),
                _ => {
                    let months = month_difference(ty, tm, ny, nm);
                    match months {
                        0..=1 => "1 month ago".to_string(),
                        2..=11 => format!("{months} months ago"),
                        12..=59 => format_compound_year_month(months),
                        m => format!("{} years ago", (m + 6) / 12),
                    }
                }
            }
        }
    }
}

/// Zed's `calculate_month_difference`: a calendar-aware month gap, so
/// 31 Jan → 1 Mar counts as one month, not one month and one day.
fn month_difference(then_y: i32, then_m: u32, now_y: i32, now_m: u32) -> usize {
    let month_diff = if now_m >= then_m {
        now_m - then_m
    } else {
        12 - then_m + now_m
    };
    let year_diff = (now_y - then_y) as usize;
    if year_diff == 0 {
        (now_m - then_m) as usize
    } else if month_diff == 0 {
        year_diff * 12
    } else if then_m > now_m {
        (year_diff - 1) * 12 + month_diff as usize
    } else {
        year_diff * 12 + month_diff as usize
    }
}

fn format_compound_year_month(months: usize) -> String {
    let years = months / 12;
    let months = months % 12;
    let year_unit = if years == 1 { "year" } else { "years" };
    if months == 0 {
        format!("{years} {year_unit} ago")
    } else {
        let month_unit = if months == 1 { "month" } else { "months" };
        format!("{years} {year_unit}, {months} {month_unit} ago")
    }
}

/// Days since the Unix epoch for a calendar date (Howard Hinnant's
/// algorithm), so relative dates count calendar days rather than
/// 24-hour periods.
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = (y - era * 400) as u32;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era as i64 * 146097 + doe as i64 - 719468
}

/// Local calendar date (year, month, day) of a unix timestamp.
/// Zed formats blame dates in the user's zone, so "Today" and
/// "Yesterday" follow the platform timezone, not UTC.
#[cfg(unix)]
fn local_date(unix: i64) -> (i32, u32, u32) {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let secs = unix as libc::time_t;
    let local = unsafe { libc::localtime_r(&secs, &mut tm) };
    if local.is_null() {
        return (1970, 1, 1);
    }
    (tm.tm_year + 1900, (tm.tm_mon + 1) as u32, tm.tm_mday as u32)
}

#[cfg(not(unix))]
fn local_date(unix: i64) -> (i32, u32, u32) {
    // No localtime on this platform: fall back to UTC.
    civil_from_days(unix.div_euclid(86_400))
}

#[cfg(not(unix))]
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i32 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Zed's `TimestampFormat::MediumAbsolute` for the blame popover
/// date, rendered in the user's zone and 12/24-hour locale
/// convention: "Today at 3:30 PM", "Yesterday at 11:00 AM", or
/// "02/24/2024 3:00 PM".
fn absolute_timestamp(unix: i64) -> String {
    let time = format_time(unix);
    let (ty, tm, td) = local_date(unix);
    let (ny, nm, nd) = local_date(unix_now());
    let days = days_from_civil(ny, nm, nd) - days_from_civil(ty, tm, td);
    match days {
        0 => format!("Today at {time}"),
        1 => format!("Yesterday at {time}"),
        _ => format!("{} {time}", format_date(unix)),
    }
}

fn format_time(unix: i64) -> String {
    let (hour, minute) = local_hm(unix);
    if is_12_hour_locale() {
        let meridiem = if hour < 12 { "AM" } else { "PM" };
        let hour = match hour % 12 {
            0 => 12,
            h => h,
        };
        format!("{hour}:{minute:02} {meridiem}")
    } else {
        format!("{hour:02}:{minute:02}")
    }
}

fn format_date(unix: i64) -> String {
    let (year, month, day) = local_date(unix);
    if is_12_hour_locale() {
        format!("{month:02}/{day:02}/{year}")
    } else {
        format!("{day:02}/{month:02}/{year}")
    }
}

/// Local (hour, minute) of a unix timestamp, in the user's zone.
#[cfg(unix)]
fn local_hm(unix: i64) -> (u32, u32) {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let secs = unix as libc::time_t;
    let local = unsafe { libc::localtime_r(&secs, &mut tm) };
    if local.is_null() {
        return (0, 0);
    }
    (tm.tm_hour as u32, tm.tm_min as u32)
}

#[cfg(not(unix))]
fn local_hm(unix: i64) -> (u32, u32) {
    let secs = unix.max(0);
    let rem = secs.rem_euclid(86_400);
    ((rem / 3600) as u32, ((rem % 3600) / 60) as u32)
}

/// Zed's `is_12_hour_time_by_locale`: the locales that prefer
/// "3:30 PM" over "15:30".
fn is_12_hour_locale() -> bool {
    let locale = std::env::var("LC_TIME")
        .or_else(|_| std::env::var("LC_ALL"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_else(|_| String::from("en_US"));
    let locale = locale
        .split('.')
        .next()
        .unwrap_or_default()
        .replace('_', "-");
    matches!(
        locale.as_str(),
        "es-MX" | "es-CO" | "es-SV" | "es-NI" | "es-HN" | "en-US" | "en-CA" | "en-AU"
            | "en-NZ" | "ar-SA" | "ar-EG" | "ar-JO" | "en-IN" | "hi-IN" | "en-PK" | "ur-PK"
            | "en-PH" | "fil-PH" | "bn-BD" | "ccp-BD" | "en-IE" | "ga-IE" | "en-MY" | "ms-MY"
    )
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
            entries: vec![entry(&"a".repeat(40), "Ada Lovelace", now - 3600, "Fix bug")],
            rows: vec![Some(0)],
        };
        let lines = blame_annotations(&blame, None, false, now);
        let line = lines[0].as_ref().unwrap();
        assert_eq!(line.text.as_ref(), "Ada Lovelace, 1 hour ago");
        assert_eq!(line.sha.as_ref(), "aaaaaaa");
    }

    #[test]
    fn appends_summary_when_requested() {
        let now = 1_000_000;
        let blame = FileBlame {
            entries: vec![entry(&"a".repeat(40), "Ada", now - 3600, "Fix bug")],
            rows: vec![Some(0)],
        };
        let lines = blame_annotations(&blame, None, true, now);
        assert_eq!(
            lines[0].as_ref().unwrap().text.as_ref(),
            "Ada, 1 hour ago - Fix bug"
        );
    }

    #[test]
    fn formats_uncommitted_like_zed() {
        // Zed's parser drops zero-SHA entries, so lines git cannot
        // attribute to a commit yet carry no annotation at all.
        let now = 1_000_000;
        let mut e = entry(&"0".repeat(40), "Not Committed Yet", 0, "");
        e.summary = None;
        let blame = FileBlame {
            entries: vec![e],
            rows: vec![Some(0)],
        };
        let lines = blame_annotations(&blame, None, false, now);
        assert!(lines[0].is_none());
    }

    #[test]
    fn blank_rows_have_no_annotation() {
        let blame = FileBlame {
            entries: vec![entry(&"a".repeat(40), "Ada", 0, "s")],
            rows: vec![Some(0), None],
        };
        let lines = blame_annotations(&blame, None, false, 1000);
        assert!(lines[0].is_some());
        assert!(lines[1].is_none());
    }

    #[test]
    fn relative_time_buckets() {
        // Time buckets are timezone-independent; the calendar
        // buckets below are covered by the pure helpers.
        let now = 10_000_000;
        assert_eq!(relative_time(now, now), "Just now");
        assert_eq!(relative_time(now - 45, now), "Just now");
        assert_eq!(relative_time(now - 61, now), "1 minute ago");
        assert_eq!(relative_time(now - 2 * 60, now), "2 minutes ago");
        assert_eq!(relative_time(now - 3599, now), "59 minutes ago");
        assert_eq!(relative_time(now - 3600, now), "1 hour ago");
        assert_eq!(relative_time(now - 2 * 3600, now), "2 hours ago");
        assert_eq!(relative_time(now - 23 * 3600, now), "23 hours ago");
    }

    #[test]
    fn relative_date_calendar_helpers() {
        // days_from_civil: 1970-01-01 is epoch day 0.
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2024, 1, 1), 19723);
        assert_eq!(days_from_civil(2024, 3, 1), 19783);
        // month_difference is calendar-aware.
        assert_eq!(month_difference(2024, 1, 2024, 3), 2);
        assert_eq!(month_difference(2023, 12, 2024, 11), 11);
        assert_eq!(month_difference(2023, 11, 2024, 11), 12);
        // Compound year+month wording.
        assert_eq!(format_compound_year_month(12), "1 year ago");
        assert_eq!(format_compound_year_month(13), "1 year, 1 month ago");
        assert_eq!(format_compound_year_month(59), "4 years, 11 months ago");
    }
}
