//! VS Code-style project search state on [`Workspace`](super::Workspace).
//!
//! The heavy lifting (parallel walk + ripgrep matching) lives in
//! [`crate::search`]; this module owns the UI state around it: the three
//! sidebar inputs, the toggle flags, debounced background runs, result
//! navigation and Replace All.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{AppContext, Context, Window};
use gpui_component::input::{InputEvent, InputState};

use super::Workspace;
use crate::search::{self, SearchOptions, MAX_FILES, MAX_MATCHES};

/// How long to wait after the last keystroke before searching, so fast
/// typing issues one background run instead of one per character.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(220);

impl Workspace {
    /// Lazily create the query / replace / include inputs. Idempotent —
    /// safe to call from render and from every Search entry point.
    pub(crate) fn ensure_search_inputs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.search_query_input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx).placeholder("Search")
            });
            cx.subscribe(&input, |this, _state, event: &InputEvent, cx| match event {
                InputEvent::Change => this.schedule_search(cx),
                InputEvent::PressEnter { .. } => this.run_search_now(cx),
                _ => {}
            })
            .detach();
            self.search_query_input = Some(input);
        }
        if self.search_replace_input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx).placeholder("Replace")
            });
            cx.subscribe(&input, |_this, _state, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            })
            .detach();
            self.search_replace_input = Some(input);
        }
        if self.search_include_input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx).placeholder("files to include")
            });
            cx.subscribe(&input, |this, _state, event: &InputEvent, cx| match event {
                InputEvent::Change => this.schedule_search(cx),
                InputEvent::PressEnter { .. } => this.run_search_now(cx),
                _ => {}
            })
            .detach();
            self.search_include_input = Some(input);
        }
    }

    pub(crate) fn focus_search_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_search_inputs(window, cx);
        if let Some(input) = self.search_query_input.clone() {
            input.update(cx, |state, cx| state.focus(window, cx));
        }
    }

    fn search_snapshot(&self, cx: &Context<Self>) -> SearchOptions {
        let query = self
            .search_query_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default();
        let include_filter = self
            .search_include_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default();
        SearchOptions {
            query,
            case_sensitive: self.search_case_sensitive,
            whole_word: self.search_whole_word,
            use_regex: self.search_use_regex,
            include_filter,
        }
    }

    /// Debounced entry point for typing: waits for a pause, then searches.
    /// Stale generations are dropped so only the latest query paints.
    pub(crate) fn schedule_search(&mut self, cx: &mut Context<Self>) {
        self.search_generation = self.search_generation.wrapping_add(1);
        let my_gen = self.search_generation;
        let query = self
            .search_query_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default();
        if query.trim().is_empty() {
            self.search_results.clear();
            self.search_total_matches = 0;
            self.search_truncated = false;
            self.search_in_progress = false;
            self.search_error = None;
            cx.notify();
            return;
        }
        self.search_in_progress = true;
        self.search_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let _ = this.update(cx, |workspace, cx| {
                if workspace.search_generation == my_gen {
                    workspace.run_search_now(cx);
                }
            });
        })
        .detach();
    }

    /// Immediate search (Enter key, flag toggles, refresh button).
    pub(crate) fn run_search_now(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            self.search_error = Some("Open a folder to search".to_string());
            self.search_results.clear();
            self.search_total_matches = 0;
            self.search_in_progress = false;
            cx.notify();
            return;
        };
        let opts = self.search_snapshot(cx);
        if opts.query_trimmed().is_empty() {
            self.search_results.clear();
            self.search_total_matches = 0;
            self.search_truncated = false;
            self.search_in_progress = false;
            self.search_error = None;
            cx.notify();
            return;
        }
        self.search_generation = self.search_generation.wrapping_add(1);
        let my_gen = self.search_generation;
        self.search_in_progress = true;
        self.search_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let output =
                cx.background_spawn(async move { search::run_search(&root, &opts, MAX_MATCHES, MAX_FILES) })
                    .await;
            let _ = this.update(cx, |workspace, cx| {
                if workspace.search_generation != my_gen {
                    return; // a newer query already superseded this run
                }
                workspace.search_in_progress = false;
                match output {
                    Ok(out) => {
                        workspace.search_results = out.files;
                        workspace.search_total_matches = out.total_matches;
                        workspace.search_truncated = out.truncated;
                        workspace.search_elapsed_ms = out.elapsed_ms;
                        workspace.search_error = None;
                        if workspace.search_total_matches == 0 {
                            workspace.status =
                                format!("No results in {} files", out.searched_files);
                        } else {
                            workspace.status = format!(
                                "{} {} in {} {}",
                                out.total_matches,
                                if out.total_matches == 1 { "result" } else { "results" },
                                workspace.search_results.len(),
                                if workspace.search_results.len() == 1 {
                                    "file"
                                } else {
                                    "files"
                                }
                            );
                        }
                    }
                    Err(message) => {
                        workspace.search_results.clear();
                        workspace.search_total_matches = 0;
                        workspace.search_error = Some(message);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn toggle_search_case(&mut self, cx: &mut Context<Self>) {
        self.search_case_sensitive = !self.search_case_sensitive;
        self.run_search_now(cx);
    }

    pub(crate) fn toggle_search_whole_word(&mut self, cx: &mut Context<Self>) {
        self.search_whole_word = !self.search_whole_word;
        self.run_search_now(cx);
    }

    pub(crate) fn toggle_search_regex(&mut self, cx: &mut Context<Self>) {
        self.search_use_regex = !self.search_use_regex;
        self.run_search_now(cx);
    }

    pub(crate) fn toggle_search_replace_open(&mut self, cx: &mut Context<Self>) {
        self.search_replace_open = !self.search_replace_open;
        cx.notify();
    }

    pub(crate) fn toggle_search_file_collapsed(&mut self, path: &PathBuf, cx: &mut Context<Self>) {
        if !self.search_collapsed.remove(path) {
            self.search_collapsed.insert(path.clone());
        }
        cx.notify();
    }

    pub(crate) fn collapse_all_search(&mut self, cx: &mut Context<Self>) {
        self.search_collapsed = self.search_results.iter().map(|f| f.path.clone()).collect();
        cx.notify();
    }

    pub(crate) fn expand_all_search(&mut self, cx: &mut Context<Self>) {
        self.search_collapsed.clear();
        cx.notify();
    }

    pub(crate) fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_results.clear();
        self.search_total_matches = 0;
        self.search_truncated = false;
        self.search_in_progress = false;
        self.search_error = None;
        self.search_collapsed.clear();
        if let Some(input) = self.search_query_input.clone() {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.focus_search_query(window, cx);
        cx.notify();
    }

    /// Open a search hit in the editor and place the cursor on it. Files
    /// that are not open yet load asynchronously — the cursor jump is then
    /// applied from [`super::Workspace::finish_open_file`] via
    /// `pending_search_jump`.
    pub(crate) fn open_search_result(
        &mut self,
        path: PathBuf,
        line: usize,
        col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(idx) = self.tabs.iter().position(|t| t.path.as_ref() == Some(&path)) {
            self.pending_search_jump = None;
            self.active_tab = idx;
            if let Some(tab) = self.tabs.get_mut(idx) {
                tab.preview = false;
            }
            self.jump_to_position(line, col, window, cx);
            cx.notify();
            return;
        }
        self.pending_search_jump = Some((path.clone(), line, col));
        self.open_file(path, window, cx);
    }

    pub(crate) fn jump_to_position(
        &mut self,
        line: usize,
        col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |state, cx| {
                state.set_cursor_position(
                    lsp_types::Position {
                        line: (line as u32).saturating_sub(1),
                        character: col as u32,
                    },
                    window,
                    cx,
                );
            });
            self.status = format!("Line {line}:{col}");
        }
    }

    fn replace_text(&self, cx: &Context<Self>) -> String {
        self.search_replace_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default()
    }

    /// Replace all hits across every file in the results list.
    pub(crate) fn replace_all_in_search(&mut self, cx: &mut Context<Self>) {
        if self.search_results.is_empty() {
            return;
        }
        let opts = self.search_snapshot(cx);
        let replace = self.replace_text(cx);
        let files = self.search_results.clone();
        self.status = format!(
            "Replacing {} {}…",
            self.search_total_matches,
            if self.search_total_matches == 1 {
                "occurrence"
            } else {
                "occurrences"
            }
        );
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (changed, made) = cx
                .background_spawn(async move { search::apply_replace(&files, &opts, &replace) })
                .await;
            let _ = this.update(cx, |workspace, cx| {
                workspace.status = if made == 0 {
                    "Nothing replaced".into()
                } else {
                    format!(
                        "Replaced {made} {} in {changed} {}",
                        if made == 1 { "occurrence" } else { "occurrences" },
                        if changed == 1 { "file" } else { "files" }
                    )
                };
                workspace.git_poke();
                // Re-run so the list reflects the new contents.
                workspace.run_search_now(cx);
            });
        })
        .detach();
    }

    /// Replace every hit inside one file (the per-file action next to the
    /// file header), then refresh the list.
    pub(crate) fn replace_in_search_file(&mut self, path: &PathBuf, cx: &mut Context<Self>) {
        let Some(file) = self.search_results.iter().find(|f| &f.path == path).cloned() else {
            return;
        };
        let opts = self.search_snapshot(cx);
        let replace = self.replace_text(cx);
        let label = file.rel.clone();
        cx.spawn(async move |this, cx| {
            let (changed, made) = cx
                .background_spawn(async move { search::apply_replace(std::slice::from_ref(&file), &opts, &replace) })
                .await;
            let _ = this.update(cx, |workspace, cx| {
                workspace.status = if made == 0 {
                    format!("Nothing replaced in {label}")
                } else {
                    format!(
                        "Replaced {made} {} in {label}",
                        if made == 1 { "occurrence" } else { "occurrences" }
                    )
                };
                let _ = changed;
                workspace.git_poke();
                workspace.run_search_now(cx);
            });
        })
        .detach();
    }

    /// Drop results when the project changes so stale paths never paint.
    pub(crate) fn clear_search_results(&mut self) {
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_results.clear();
        self.search_total_matches = 0;
        self.search_truncated = false;
        self.search_in_progress = false;
        self.search_error = None;
        self.search_collapsed.clear();
    }
}
