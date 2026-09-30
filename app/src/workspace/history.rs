//! Source Control "History" graph orchestration (Feature 1).
//!
//! Pure git work (paginated `git log`, ref decoding, lane/merge graph) lives in
//! [`crate::git`]. This module drives it from the [`Workspace`]: it loads pages
//! off the UI thread, caches the results, recomputes the graph, and wires the
//! commit context-menu actions (copy SHA / message, checkout, view diff).

use gpui::{AppContext as _, Context};

use crate::git;

use super::{DiffTab, OpenTab, Workspace};

/// Commits fetched per `git log` page. Large enough to fill the viewport and
/// scroll a while, small enough that even huge repos load instantly.
pub(crate) const HISTORY_PAGE: usize = 200;

impl Workspace {
    /// Switch the Source Control panel to the History graph and load page one.
    pub(crate) fn git_show_history(&mut self, cx: &mut Context<Self>) {
        self.git_history_view = true;
        if self.git_history.is_empty() {
            self.reload_git_history(cx);
        }
        cx.notify();
    }

    /// Switch the Source Control panel back to the changes list.
    pub(crate) fn git_show_changes(&mut self, cx: &mut Context<Self>) {
        self.git_history_view = false;
        cx.notify();
    }

    /// Reset the history and reload the first page (repo change / refresh).
    pub(crate) fn reload_git_history(&mut self, cx: &mut Context<Self>) {
        self.git_history.clear();
        self.git_history_graph.clear();
        self.git_history_complete = false;
        self.load_history_page(0, cx);
    }

    /// Load the next page of commits when the user scrolls near the end.
    pub(crate) fn git_history_load_more(&mut self, cx: &mut Context<Self>) {
        if self.git_history_loading || self.git_history_complete {
            return;
        }
        let skip = self.git_history.len();
        self.load_history_page(skip, cx);
    }

    /// Fetch one page of commits off-thread and merge it in.
    fn load_history_page(&mut self, skip: usize, cx: &mut Context<Self>) {
        let Some(root) = self.git.as_ref().map(|g| g.root.clone()) else {
            return;
        };
        self.git_history_loading = true;
        self.git_history_generation = self.git_history_generation.wrapping_add(1);
        let generation = self.git_history_generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let commits = cx
                .background_spawn(async move { git::commit_log(&root, skip, HISTORY_PAGE, false) })
                .await;
            let _ = this.update(cx, |ws, cx| {
                if ws.git_history_generation != generation {
                    // Superseded by a newer load (e.g. a repo refresh).
                    return;
                }
                ws.git_history_loading = false;
                if commits.len() < HISTORY_PAGE {
                    ws.git_history_complete = true;
                }
                if skip == 0 {
                    ws.git_history = commits;
                } else {
                    ws.git_history.extend(commits);
                }
                ws.git_history_graph = git::compute_graph(&ws.git_history);
                cx.notify();
            });
        })
        .detach();
    }

    /// Select a commit in the graph and open its full diff.
    pub(crate) fn git_select_commit(&mut self, sha: String, cx: &mut Context<Self>) {
        self.git_history_selected = Some(sha.clone());
        self.git_view_commit_diff(sha, cx);
        cx.notify();
    }

    /// Open a commit's full diff (`git show <sha>`) in a read-only diff tab.
    pub(crate) fn git_view_commit_diff(&mut self, sha: String, cx: &mut Context<Self>) {
        let Some(root) = self.git.as_ref().map(|g| g.root.clone()) else {
            return;
        };
        let short: String = sha.chars().take(7).collect();
        // Synthetic, unique path so the existing diff-tab lookup keeps working
        // and the tab title reads like the short SHA.
        let tab_path = root.join(&short);

        if let Some(idx) = self.tabs.iter().position(|t| {
            t.diff
                .as_ref()
                .map(|d| d.commit.as_deref() == Some(sha.as_str()))
                == Some(true)
        }) {
            self.active_tab = idx;
            cx.notify();
            return;
        }

        self.tabs.push(OpenTab {
            path: None,
            editor: None,
            dirty: false,
            untitled: false,
            preview: false,
            is_settings: false,
            diff: Some(DiffTab {
                path: tab_path.clone(),
                rel: short.clone(),
                staged: false,
                text: None,
                parsed: None,
                error: None,
                commit: Some(sha.clone()),
            }),
            language_override: None,
        });
        self.active_tab = self.tabs.len() - 1;
        self.status = format!("Commit {short}");
        self.load_diff_tab(root, short, tab_path, false, Some(sha), cx);
        cx.notify();
    }

    /// Copy a commit's full SHA to the clipboard.
    pub(crate) fn git_copy_sha(&mut self, sha: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(sha.to_string()));
        self.status = format!("Copied SHA {}", &sha.chars().take(7).collect::<String>());
        cx.notify();
    }

    /// Copy a commit's subject line to the clipboard.
    pub(crate) fn git_copy_commit_message(&mut self, sha: &str, cx: &mut Context<Self>) {
        let message = self
            .git_history
            .iter()
            .find(|c| c.sha == sha)
            .map(|c| c.subject.clone())
            .unwrap_or_default();
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(message));
        self.status = "Copied commit message".into();
        cx.notify();
    }

    /// Check out a commit (detached HEAD) from the history right-click menu.
    pub(crate) fn git_checkout_commit(&mut self, sha: &str, cx: &mut Context<Self>) {
        let sha = sha.to_string();
        self.git_remote_op("Checkout", move |root| git::checkout(&root, &sha), cx);
    }
}
