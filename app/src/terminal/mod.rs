use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{
    div, prelude::*, px, rgba, svg, Context, DragMoveEvent, Edges, Entity, FocusHandle,
    IntoElement, MouseButton, ScrollHandle, SharedString, Window,
};
use gpui_component::input::{Input, InputState};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::Sizable as _;
use gpui_terminal::{TerminalConfig, TerminalView};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

use crate::actions::{
    ClearTerminal, CloseAllTerminals, CloseCleanTerminals, CloseOtherTerminals, CloseTerminal,
    CloseTerminalsLeft, CloseTerminalsRight, NewTerminal, TerminalCopy, TerminalPaste,
    TerminalPasteText, TerminalSelectAll, ToggleTerminalPin,
};
use crate::assets::MONO_FONT;
use crate::theme::Colors;
use crate::workspace::Workspace;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalState {
    Running,

    Exited(i32),

    Error,
}

/// Decide what a terminal's state becomes after probing its child process.
/// `None` means "no state to move to", i.e. the probe was skipped.
fn next_state_after_probe(state: TerminalState, pid: Option<u32>) -> Option<TerminalState> {
    // Only live processes are worth probing; once a state is recorded the pid
    // is gone (or was never there), so there is nothing left to ask about.
    if state != TerminalState::Running {
        return None;
    }
    let pid = pid?;
    (!Terminal::process_is_alive(pid)).then_some(TerminalState::Exited(0))
}

pub struct Terminal {
    pub view: Entity<TerminalView>,

    /// Fallback tab label ("folder – bash"), used until the child process
    /// reports a title of its own.
    pub name: SharedString,

    /// Last title reported through `OSC 0 / 2`, mirrored here so the tab strip
    /// never has to read the view. See [`Terminal::set_osc_title`].
    pub osc_title: Option<SharedString>,

    /// Title set by the user through "Rename" on the tab. When present it
    /// wins over both the OSC title and the shell label, exactly like a
    /// renamed terminal tab in Zed.
    pub custom_title: Option<SharedString>,

    pub state: TerminalState,

    /// When set, everything the user types is swallowed before it reaches the
    /// PTY: the process keeps running and printing, but can no longer be
    /// driven from the keyboard ("Make Tab Read-Only" in the tab menu).
    ///
    /// Shared with [`SharedWriter`] so the check happens on the write path
    /// itself — paste, IME and programmatic sends are all covered.
    read_only: Arc<AtomicBool>,

    pub working_dir: Option<PathBuf>,

    #[allow(dead_code)]
    pub shell_path: String,

    pub pid: Option<u32>,

    pty_writer: Arc<Mutex<Box<dyn std::io::Write + Send>>>,
}

struct SharedWriter {
    writer: Arc<Mutex<Box<dyn std::io::Write + Send>>>,
    /// Mirrors [`Terminal::read_only`]: while set, writes are silently
    /// swallowed so a read-only tab never feeds bytes to its process.
    read_only: Arc<AtomicBool>,
}

impl std::io::Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.read_only.load(Ordering::Relaxed) {
            // Pretend the write succeeded: the terminal view keeps working
            // (no error spam), the process just never sees the input.
            return Ok(buf.len());
        }
        let mut guard = self
            .writer
            .lock()
            .map_err(|_| std::io::Error::other("lock poisoned"))?;
        guard.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.read_only.load(Ordering::Relaxed) {
            return Ok(());
        }
        let mut guard = self
            .writer
            .lock()
            .map_err(|_| std::io::Error::other("lock poisoned"))?;
        guard.flush()
    }
}

fn dirs_home() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE").ok().map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
}

#[allow(dead_code)]
pub fn terminal_keystroke_to_bytes(keystroke: &gpui::Keystroke) -> Option<Vec<u8>> {
    gpui_terminal::input::keystroke_to_bytes(keystroke, alacritty_terminal::term::TermMode::empty())
}

impl Terminal {
    pub fn new(
        root_dir: Option<&Path>,
        name: String,
        palette: gpui_terminal::ColorPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = SharedString::from(name);
        let read_only = Arc::new(AtomicBool::new(false));

        let (shell_cmd, shell_path) = Self::detect_shell();
        let working_dir = root_dir
            .map(|p| p.to_path_buf())
            .or_else(|| dirs_home().map(|h| h.to_path_buf()));

        let pty_system = native_pty_system();
        let pair = match pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("[terminal] Failed to open PTY: {e}");

                let config = TerminalConfig {
                    font_family: MONO_FONT.into(),
                    font_size: px(13.5),
                    cols: 80,
                    rows: 24,
                    scrollback: 10000,
                    line_height_multiplier: 1.2,
                    padding: Edges::all(px(6.0)),
                    colors: palette,
                    right_click_paste: false,
                    ..TerminalConfig::default()
                };
                let (dummy_reader, dummy_writer) = Self::create_dummy_pty();
                let pty_writer = Arc::new(Mutex::new(dummy_writer));
                let shared_writer = SharedWriter {
                    writer: pty_writer.clone(),
                    read_only: read_only.clone(),
                };
                let view = cx.new(|cx| TerminalView::new(shared_writer, dummy_reader, config, cx));
                return Self {
                    view,
                    name,
                    osc_title: None,
                    custom_title: None,
                    state: TerminalState::Error,
                    working_dir,
                    shell_path: shell_path.clone(),
                    pid: None,
                    pty_writer,
                    read_only,
                };
            }
        };

        let mut cmd = CommandBuilder::new(&shell_cmd);
        if let Some(dir) = &working_dir {
            cmd.cwd(dir);
        }

        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "zed");
        cmd.env("TERM_PROGRAM_VERSION", "0.1.0");
        cmd.env("LANG", "en_US.UTF-8");
        cmd.env("LC_ALL", "en_US.UTF-8");

        let child = match pair.slave.spawn_command(cmd) {
            Ok(child) => child,
            Err(e) => {
                eprintln!("[terminal] Failed to spawn shell: {e}");
                let config = TerminalConfig {
                    font_family: MONO_FONT.into(),
                    font_size: px(13.5),
                    cols: 80,
                    rows: 24,
                    scrollback: 10000,
                    line_height_multiplier: 1.2,
                    padding: Edges::all(px(6.0)),
                    colors: palette,
                    right_click_paste: false,
                    ..TerminalConfig::default()
                };
                let (dummy_reader, dummy_writer) = Self::create_dummy_pty();
                let pty_writer = Arc::new(Mutex::new(dummy_writer));
                let shared_writer = SharedWriter {
                    writer: pty_writer.clone(),
                    read_only: read_only.clone(),
                };
                let view = cx.new(|cx| TerminalView::new(shared_writer, dummy_reader, config, cx));
                return Self {
                    view,
                    name,
                    osc_title: None,
                    custom_title: None,
                    state: TerminalState::Error,
                    working_dir,
                    shell_path,
                    pid: None,
                    pty_writer,
                    read_only,
                };
            }
        };

        let pid = child.process_id();

        let writer = pair.master.take_writer().expect("take writer failed");
        let reader = pair.master.try_clone_reader().expect("clone reader failed");
        let pty_master = Arc::new(Mutex::new(pair.master));

        let config = TerminalConfig {
            font_family: MONO_FONT.into(),
            font_size: px(13.5),
            cols: 80,
            rows: 24,
            // 10k lines of scrollback — 100k was consuming ~96 MB per terminal tab.
            scrollback: 10_000,
            line_height_multiplier: 1.2,
            padding: Edges::all(px(6.0)),
            colors: palette,
            cursor_blink: true,
            copy_on_select: false,
            right_click_paste: false,
            alternate_scroll: true,
            detect_path_links: true,
            show_scrollbar: true,
            ..TerminalConfig::default()
        };

        let pty_for_resize = pty_master.clone();
        let resize_callback = move |cols: usize, rows: usize| {
            if let Ok(master) = pty_for_resize.lock() {
                let _ = master.resize(PtySize {
                    cols: cols as u16,
                    rows: rows as u16,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }
        };

        let pty_writer: Arc<Mutex<Box<dyn std::io::Write + Send>>> = Arc::new(Mutex::new(writer));

        let shared_writer = SharedWriter {
            writer: pty_writer.clone(),
            read_only: read_only.clone(),
        };
        let view = cx.new(|cx| {
            TerminalView::new(shared_writer, reader, config, cx)
                .with_resize_callback(resize_callback)
        });

        view.read(cx).focus_handle().focus(window);

        drop(child);

        Self {
            view,
            name,
            osc_title: None,
            custom_title: None,
            state: TerminalState::Running,
            working_dir,
            shell_path,
            pid,
            pty_writer,
            read_only,
        }
    }

    pub fn detect_shell() -> (String, String) {
        if cfg!(windows) {
            ("powershell.exe".to_string(), "powershell.exe".to_string())
        } else {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
            let name = if shell.ends_with("zsh") {
                "zsh"
            } else if shell.ends_with("fish") {
                "fish"
            } else if shell.ends_with("nu") {
                "nu"
            } else if shell.ends_with("sh") {
                "sh"
            } else {
                "bash"
            };
            (shell, name.to_string())
        }
    }

    pub fn detect_shell_name() -> String {
        Self::detect_shell().1
    }

    fn create_dummy_pty() -> (
        Box<dyn std::io::Read + Send>,
        Box<dyn std::io::Write + Send>,
    ) {
        (Box::new(std::io::empty()), Box::new(std::io::sink()))
    }

    /// Returns `true` only when the state *just* changed.
    ///
    /// Callers use the return value to decide whether to re-render, so an
    /// already-exited terminal has to report "no change" — otherwise every
    /// render would schedule another one, forever, and with several terminals
    /// open the `kill(2)` probe below would run once per terminal per frame.
    pub fn check_process_exit(&mut self) -> bool {
        let Some(next) = next_state_after_probe(self.state, self.pid) else {
            return false;
        };
        if self.state == next {
            return false;
        }
        self.state = next;
        true
    }

    #[cfg(unix)]
    fn process_is_alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    #[cfg(windows)]
    fn process_is_alive(pid: u32) -> bool {
        extern "system" {
            fn OpenProcess(
                dwDesiredAccess: u32,
                bInheritHandle: i32,
                dwProcessId: u32,
            ) -> *mut std::ffi::c_void;
            fn GetExitCodeProcess(hProcess: *mut std::ffi::c_void, lpExitCode: *mut u32) -> i32;
            fn CloseHandle(hObject: *mut std::ffi::c_void) -> i32;
        }
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        const STILL_ACTIVE: u32 = 259;
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut exit_code: u32 = 0;
            let result = GetExitCodeProcess(handle, &mut exit_code);
            CloseHandle(handle);
            result != 0 && exit_code == STILL_ACTIVE
        }
    }

    #[cfg(not(any(unix, windows)))]
    fn process_is_alive(_pid: u32) -> bool {
        true
    }

    pub fn send_bytes(&self, bytes: &[u8]) -> bool {
        if let Ok(mut writer) = self.pty_writer.lock() {
            use std::io::Write;
            writer.write_all(bytes).is_ok() && writer.flush().is_ok()
        } else {
            false
        }
    }

    /// Label for this terminal's tab. Precedence mirrors Zed: a user-assigned
    /// name ("Rename" on the tab) wins, then the title reported by the child
    /// process (`OSC 0 / 2`), then the shell label.
    ///
    /// Returns a `SharedString` so re-rendering the tab strip costs zero
    /// allocations no matter how many terminals are open.
    pub fn tab_label(&self) -> SharedString {
        if let Some(title) = &self.custom_title {
            return title.clone();
        }
        match &self.osc_title {
            Some(title) => title.clone(),
            None => self.name.clone(),
        }
    }

    /// Set (or clear, with `None` / an empty string) the user-assigned tab
    /// title. A custom title survives later OSC title reports, like Zed.
    pub fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        let next = title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| SharedString::new(t.to_string()));
        if self.custom_title == next {
            return;
        }
        self.custom_title = next;
        cx.notify();
    }

    /// Whether user input is currently being swallowed before the PTY.
    pub fn is_read_only(&self) -> bool {
        self.read_only.load(Ordering::Relaxed)
    }

    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        if self.read_only.load(Ordering::Relaxed) == read_only {
            return;
        }
        self.read_only.store(read_only, Ordering::Relaxed);
        cx.notify();
    }

    /// Cache the title the child process reported through `OSC 0 / 2` (an
    /// empty title means "no title", so the shell label shows again).
    pub fn set_osc_title(&mut self, title: &str, cx: &mut Context<Self>) {
        let title = title.trim();
        let next = if title.is_empty() {
            None
        } else {
            Some(SharedString::new(title.to_string()))
        };
        if self.osc_title == next {
            return;
        }
        self.osc_title = next;
        cx.notify();
    }

    pub fn set_theme(&mut self, palette: &gpui_terminal::ColorPalette, cx: &mut Context<Self>) {
        self.view.update(cx, |view, cx| {
            let mut config = view.config().clone();
            config.colors = palette.clone();
            view.update_config(config, cx);
        });
    }

    pub fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        self.view.read(cx).focus_handle().clone()
    }
}

const TAB_BAR_BG: u32 = 0x010409ff;
const TAB_BAR_BORDER_TOP: u32 = 0x394049ff;
const TAB_BAR_BORDER_BOTTOM: u32 = 0x383e47ff;
const TAB_ACTIVE_BG: u32 = 0x0d1117ff;
const TAB_ACTIVE_BORDER_L: u32 = 0x31373fff;
const TAB_ACTIVE_BORDER_R: u32 = 0x2c3139ff;
const TAB_INACTIVE_BORDER_R: u32 = 0x252a31ff;
const TAB_ACTIVE_TEXT: u32 = 0xf0f6fcff;
const TAB_INACTIVE_TEXT: u32 = 0xc9d1d9ff;
const TAB_INACTIVE_HOVER: u32 = 0x161b22ff;
const TAB_CLOSE_HOVER: u32 = 0x30363dff;

/// Tabs grow to fill the strip while there is room, shrink to this width when
/// there isn't, and only start scrolling once even the minimum doesn't fit.
const TAB_MIN_WIDTH: f32 = 84.0;
const TAB_MAX_WIDTH: f32 = 180.0;

/// Which dock a terminal panel is rendered in.
///
/// The two docks are completely independent: separate tab lists, separate
/// child processes, separate PTYs. Every button in the panel is routed by
/// this enum so the right dock never touches the bottom dock's sessions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalDock {
    /// The classic bottom panel (VS Code style).
    Bottom,
    /// The Zed-style panel docked to the right edge of the workspace.
    Right,
}

impl TerminalDock {
    /// Element ids in this module are namespaced by dock. GPUI keys element
    /// state (scroll offsets, ...) by the *path* of element ids, and the two
    /// panels' paths are otherwise identical (their ancestors carry no ids),
    /// which would make the bottom and right tab strips share scroll state.
    fn namespaced(self, id: &'static str) -> SharedString {
        match self {
            TerminalDock::Bottom => SharedString::new_static(id),
            TerminalDock::Right => SharedString::from(format!("right-{id}")),
        }
    }

    /// Indexed variant of [`TerminalDock::namespaced`] for per-tab ids.
    fn namespaced_index(self, id: &'static str, index: usize) -> gpui::ElementId {
        gpui::ElementId::named_usize(self.namespaced(id), index)
    }
}

/// Payload carried by GPUI while a terminal tab is being dragged.
///
/// The terminal is identified by entity rather than index, so the drop stays
/// correct even if tabs are opened or closed mid-drag, and the same payload
/// works across docks (a bottom tab can be dropped on the right dock's strip).
#[derive(Clone)]
pub struct TerminalTabDrag {
    pub terminal: Entity<Terminal>,
    /// Snapshot of the tab label, rendered on the drag chip.
    pub label: SharedString,
    /// Snapshot of the tab icon, rendered on the drag chip.
    pub icon: &'static str,
}

impl Render for TerminalTabDrag {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // Same chip styling as the explorer's drag payload: a compact,
        // slightly translucent pill that follows the pointer.
        div()
            .flex()
            .flex_row()
            .items_center()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(4.0))
            .bg(rgba(0x161b22f0))
            .border_1()
            .border_color(rgba(0x394049ff))
            .text_size(px(12.0))
            .text_color(rgba(TAB_ACTIVE_TEXT))
            .child(crate::ui::common::icon_img(self.icon, 15.0))
            .child(div().w(px(6.0)).flex_none())
            .child(self.label.clone())
    }
}

/// Everything `render_terminal_panel` needs to know about one dock's tabs.
pub struct TerminalPanelParams<'a> {
    pub dock: TerminalDock,
    pub tabs: &'a [Entity<Terminal>],
    pub active: usize,
    /// Number of pinned tabs; pinned tabs are always a prefix of `tabs`.
    pub pinned_count: usize,
    pub maximized: bool,
    pub tab_scroll: &'a ScrollHandle,
    /// Insertion index (`0..=tabs.len()`) of an in-flight tab drag targeting
    /// this dock, used to paint the drop indicator.
    pub drag_target: Option<usize>,
    /// The tab currently being renamed inline in this dock, if any.
    pub renaming: Option<(Entity<Terminal>, Entity<InputState>)>,
}

pub fn render_terminal_panel(
    params: TerminalPanelParams,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let tabs = params.tabs;
    let active = params.active;
    let dock = params.dock;
    let active_terminal = tabs.get(active).or_else(|| tabs.first()).cloned();
    let active_view = active_terminal.as_ref().map(|term| {
        let term = term.read(cx);
        (term.view.clone(), term.state, term.is_read_only())
    });

    div()
        .size_full()
        .flex()
        .flex_col()
        // Scopes the Zed-style tab shortcuts (Ctrl/Cmd+W, Ctrl/Cmd+K
        // sequences, ...) to whichever terminal owns focus; see the
        // `TerminalPanel` bindings in main.rs.
        .key_context("TerminalPanel")
        .bg(rgba(TAB_BAR_BG))
        .child(render_terminal_tab_bar(&params, t, cx))
        .child(
            div()
                .flex_1()
                .w_full()
                .min_h(px(0.0))
                .overflow_hidden()
                .bg(rgba(TAB_BAR_BG))
                .when_some(active_view, |d, (view, state, read_only)| {
                    let menu_view = view.clone();
                    let action_context = view.read(cx).focus_handle().clone();
                    d.child(
                        div()
                            .id(dock.namespaced("terminal-context-surface"))
                            .size_full()
                            .child(view)
                            .context_menu(move |menu, _window, cx| {
                                let has_selection = menu_view.read(cx).has_selection();
                                let has_clipboard_text = cx
                                    .read_from_clipboard()
                                    .and_then(|item| item.text())
                                    .is_some();
                                let writable = state == TerminalState::Running && !read_only;

                                menu.action_context(action_context.clone())
                                    .menu("New Terminal", Box::new(NewTerminal))
                                    .separator()
                                    .menu_with_enable("Copy", Box::new(TerminalCopy), has_selection)
                                    .menu_with_enable(
                                        "Paste",
                                        Box::new(TerminalPaste),
                                        writable && has_clipboard_text,
                                    )
                                    .menu_with_enable(
                                        "Paste Text",
                                        Box::new(TerminalPasteText),
                                        writable && has_clipboard_text,
                                    )
                                    .menu("Select All", Box::new(TerminalSelectAll))
                                    .menu("Clear", Box::new(ClearTerminal))
                                    .separator()
                                    .menu("Close Terminal Tab", Box::new(CloseTerminal))
                            }),
                    )
                }),
        )
}

fn render_terminal_tab_bar(
    params: &TerminalPanelParams,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let dock = params.dock;
    let tabs = params.tabs;
    let active = params.active;
    let maximized = params.maximized;
    let tab_scroll = params.tab_scroll;
    // Only meaningful while a drag is actually in flight; guards against a
    // stale indicator if a drag ended somewhere we couldn't observe.
    let drag_target = if cx.has_active_drag() {
        params.drag_target
    } else {
        None
    };
    let tab_count = tabs.len();

    let tabs_root = div()
        .id(dock.namespaced("terminal-tab-bar"))
        .h(px(28.0))
        .w_full()
        .flex()
        .flex_row()
        .bg(rgba(TAB_BAR_BG))
        .border_t_1()
        .border_color(rgba(TAB_BAR_BORDER_TOP));

    // The tabs get their own scroll container. They used to be laid out inline
    // with a fixed width, so opening enough of them pushed the `+` and the
    // window buttons off the right edge where they couldn't be reached.
    //
    // Scrolling horizontally (wheel / trackpad) is handled by GPUI; the
    // scrollbar itself is zero-width, and the active tab is scrolled into view
    // whenever the selection changes.
    let tab_strip = div()
        .id(dock.namespaced("terminal-tab-strip"))
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .flex()
        .flex_row()
        .items_center()
        .overflow_x_scroll()
        .scrollbar_width(px(0.0))
        .track_scroll(tab_scroll)
        .border_b_1()
        .border_color(rgba(TAB_BAR_BORDER_BOTTOM))
        .children(tabs.iter().enumerate().map(|(idx, terminal)| {
            let is_active = idx == active;
            let term = terminal.read(cx);
            // Cached label (custom title, else the title reported via
            // `OSC 0 / 2`, else the shell label) — reading the view here
            // would subscribe the whole strip to every terminal,
            // re-rendering all of them on every PTY write.
            let label = term.tab_label();
            let state = term.state;
            let is_read_only = term.is_read_only();
            let rename_input = params
                .renaming
                .as_ref()
                .filter(|(renaming_terminal, _)| renaming_terminal == terminal)
                .map(|(_, input)| input.clone());
            render_terminal_tab(
                TerminalTabState {
                    dock,
                    index: idx,
                    is_active,
                    is_pinned: idx < params.pinned_count,
                    is_read_only,
                    label,
                    state,
                    terminal: terminal.clone(),
                    rename_input,
                    // Drop indicator: a caret on the left edge of the tab at
                    // the insertion index, or on the right edge of the last
                    // tab when the drop appends.
                    drop_before: drag_target == Some(idx),
                    drop_after: idx + 1 == tab_count && drag_target == Some(tab_count),
                },
                t,
                cx,
            )
        }))
        // Tail drop zone: the empty space after the last tab accepts drops
        // and appends the dragged tab at the end, like Zed's tab bar.
        .child(render_terminal_tab_dropzone(dock, tab_count, cx));

    // The separator + maximize button only make sense for the bottom panel;
    // the right dock has no "maximized" state (it is resized by dragging).
    tabs_root
        .child(tab_strip)
        .child(render_new_terminal_button(dock, t, cx))
        .when(dock == TerminalDock::Bottom, |bar| {
            bar.child(
                div()
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(rgba(TAB_BAR_BORDER_BOTTOM))
                    .child(div().w(px(1.0)).h(px(14.0)).bg(rgba(0x383e47ff))),
            )
            .child(render_maximize_terminal_button(maximized, t, cx))
        })
        .child(render_hide_panel_button(dock, t, cx))
}

fn render_maximize_terminal_button(
    maximized: bool,
    _t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let icon_path = if maximized {
        "ui_icons/screen_normal.svg"
    } else {
        "ui_icons/screen_full.svg"
    };

    div()
        .id("term-max-btn")
        .w(px(28.0))
        .h_full()
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .border_b_1()
        .border_color(rgba(TAB_BAR_BORDER_BOTTOM))
        .hover(|s| s.bg(rgba(TAB_INACTIVE_HOVER)))
        .child(
            svg()
                .path(icon_path)
                .w(px(14.0))
                .h(px(14.0))
                .text_color(rgba(0x8b949eff)),
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.toggle_terminal_maximized(cx);
        }))
}

/// Per-tab data gathered by the tab bar before rendering one tab.
struct TerminalTabState {
    dock: TerminalDock,
    index: usize,
    is_active: bool,
    is_pinned: bool,
    is_read_only: bool,
    label: SharedString,
    state: TerminalState,
    terminal: Entity<Terminal>,
    /// Present while this tab's title is being renamed inline.
    rename_input: Option<Entity<InputState>>,
    /// Paint the drop indicator on the left edge of this tab.
    drop_before: bool,
    /// Paint the drop indicator on the right edge (last tab only).
    drop_after: bool,
}

fn render_terminal_tab(
    tab: TerminalTabState,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let TerminalTabState {
        dock,
        index,
        is_active,
        is_pinned,
        is_read_only,
        label,
        state,
        terminal,
        rename_input,
        drop_before,
        drop_after,
    } = tab;

    let tab_bg = if is_active { TAB_ACTIVE_BG } else { TAB_BAR_BG };
    let text_color = if is_active {
        TAB_ACTIVE_TEXT
    } else {
        TAB_INACTIVE_TEXT
    };

    let icon_path = if label.contains("bash") {
        "file_icons/file_type_shell.svg"
    } else {
        "file_icons/file_type_powershell.svg"
    };

    let display_name = match state {
        TerminalState::Running => label.clone(),
        TerminalState::Exited(code) => SharedString::from(format!("{label} (exit {code})")),
        TerminalState::Error => SharedString::from(format!("{label} (error)")),
    };

    let mut tab = div()
        .id(dock.namespaced_index("terminal-tab", index))
        .relative()
        .flex_1()
        .min_w(px(TAB_MIN_WIDTH))
        .max_w(px(TAB_MAX_WIDTH))
        .h_full()
        .pl(px(12.0))
        .pr(px(8.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .overflow_hidden()
        .bg(rgba(tab_bg))
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, window, cx| match dock {
            TerminalDock::Bottom => this.activate_terminal(index, window, cx),
            TerminalDock::Right => this.activate_terminal_right(index, window, cx),
        }))
        // Middle click closes the tab (pinned tabs are spared), like Zed.
        .on_mouse_down(
            MouseButton::Middle,
            cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.middle_click_close_terminal(dock, index, window, cx);
            }),
        )
        // ---- Drag & drop reordering -------------------------------------
        // GPUI only starts a drag once the pointer travels past its drag
        // threshold with the button held, so plain clicks never reorder.
        .on_drag(
            TerminalTabDrag {
                terminal: terminal.clone(),
                label: display_name.clone(),
                icon: icon_path,
            },
            |drag, _offset, _window, cx| {
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            },
        )
        .on_drag_move(cx.listener(
            move |this, event: &DragMoveEvent<TerminalTabDrag>, _window, cx| {
                if event.bounds.contains(&event.event.position) {
                    // Left half inserts before this tab, right half after —
                    // the same split Zed uses for its tab drops.
                    let insert = if event.event.position.x < event.bounds.center().x {
                        index
                    } else {
                        index + 1
                    };
                    this.set_terminal_drag_target(dock, insert, index, cx);
                } else {
                    this.clear_terminal_drag_target_from(dock, index, cx);
                }
            },
        ))
        .drag_over::<TerminalTabDrag>(|style, _, _, _| style.bg(rgba(TAB_INACTIVE_HOVER)))
        .on_drop(
            cx.listener(move |this, drag: &TerminalTabDrag, window, cx| {
                cx.stop_propagation();
                this.drop_terminal_tab(drag, dock, index, window, cx);
            }),
        );

    if is_active {
        tab = tab
            .border_l_1()
            .border_color(rgba(TAB_ACTIVE_BORDER_L))
            .border_r_1()
            .border_color(rgba(TAB_ACTIVE_BORDER_R))
            .border_b_1()
            .border_color(rgba(TAB_ACTIVE_BG));
    } else {
        tab = tab
            .border_r_1()
            .border_color(rgba(TAB_INACTIVE_BORDER_R))
            .border_b_1()
            .border_color(rgba(TAB_BAR_BORDER_BOTTOM))
            .hover(|h| h.bg(rgba(TAB_INACTIVE_HOVER)));
    }

    tab = tab.child(
        div()
            .flex_shrink_0()
            .child(crate::ui::common::icon_img(icon_path, 15.0)),
    );

    if is_read_only {
        tab = tab.child(
            div().flex_shrink_0().child(
                svg()
                    .path("ui_icons/lock.svg")
                    .w(px(11.0))
                    .h(px(11.0))
                    .text_color(rgba(text_color)),
            ),
        );
    }

    if let Some(input) = rename_input {
        // Inline rename: the label swaps for a focused input, Enter commits,
        // Escape/blur cancels (wired up in Workspace::start_terminal_rename).
        tab = tab.child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .h(px(20.0))
                .flex()
                .items_center()
                .bg(rgba(TAB_ACTIVE_BG))
                .border_1()
                .border_color(rgba(t.border_focused))
                .rounded(px(3.0))
                .px(px(2.0))
                .child(
                    Input::new(&input)
                        .xsmall()
                        .text_size(px(12.0))
                        .appearance(false)
                        .bordered(false),
                ),
        );
    } else {
        tab = tab.child(
            div()
                .child(display_name)
                .text_size(px(12.0))
                .text_color(rgba(text_color))
                .overflow_x_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .min_w(px(0.0))
                .flex_1(),
        );
    }

    if is_pinned {
        // Pinned tabs trade the close button for a pin glyph; clicking it
        // unpins, exactly like the end slot of a pinned tab in Zed.
        tab = tab.child(
            div()
                .id(dock.namespaced_index("terminal-tab-pin", index))
                .w(px(16.0))
                .h(px(16.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(2.0))
                .cursor_pointer()
                .hover(|h| h.bg(rgba(TAB_CLOSE_HOVER)))
                .child(
                    svg()
                        .path("ui_icons/pin.svg")
                        .w(px(11.0))
                        .h(px(11.0))
                        .text_color(rgba(text_color)),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.toggle_terminal_pin_at(dock, index, window, cx);
                })),
        );
    } else if is_active {
        tab = tab.child(
            div()
                .id(dock.namespaced_index("terminal-tab-close", index))
                .w(px(16.0))
                .h(px(16.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(2.0))
                .cursor_pointer()
                .hover(|h| h.bg(rgba(TAB_CLOSE_HOVER)))
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(rgba(TAB_ACTIVE_TEXT))
                        .line_height(px(13.0))
                        .child("✕"),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    // Removing the tab shifts every index after it, so the
                    // parent's activate-on-click must not run afterwards.
                    cx.stop_propagation();
                    match dock {
                        TerminalDock::Bottom => this.close_terminal(index, window, cx),
                        TerminalDock::Right => this.close_terminal_right(index, window, cx),
                    }
                })),
        );
    }

    // Drop indicator: a 2px accent caret at the insertion point. Painted as
    // an absolutely-positioned overlay so it never shifts the tab layout.
    if drop_before {
        tab = tab.child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(2.0))
                .bg(rgba(t.text_accent)),
        );
    }
    if drop_after {
        tab = tab.child(
            div()
                .absolute()
                .right_0()
                .top_0()
                .bottom_0()
                .w(px(2.0))
                .bg(rgba(t.text_accent)),
        );
    }

    // Right-click context menu, targeting *this* tab (not the active one).
    // Applied last: `context_menu` wraps the element.
    let workspace = cx.entity();
    tab.context_menu(move |menu, window, cx| {
        build_terminal_tab_menu(menu, &workspace, dock, &terminal, window, cx)
    })
}

/// The stretch of tab bar after the last tab. Dropping a dragged tab here
/// appends it at the end of the strip.
fn render_terminal_tab_dropzone(
    dock: TerminalDock,
    tab_count: usize,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    /// `owner_ix` used by the tail zone when claiming the drag target, so it
    /// never fights with a real tab over who cleared the indicator.
    const TAIL_OWNER: usize = usize::MAX;

    div()
        .id(dock.namespaced("terminal-tab-dropzone"))
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .on_drag_move(cx.listener(
            move |this, event: &DragMoveEvent<TerminalTabDrag>, _window, cx| {
                if event.bounds.contains(&event.event.position) {
                    this.set_terminal_drag_target(dock, tab_count, TAIL_OWNER, cx);
                } else {
                    this.clear_terminal_drag_target_from(dock, TAIL_OWNER, cx);
                }
            },
        ))
        .on_drop(
            cx.listener(move |this, drag: &TerminalTabDrag, window, cx| {
                cx.stop_propagation();
                this.drop_terminal_tab(drag, dock, tab_count, window, cx);
            }),
        )
}

/// Build the Zed-style right-click menu for one terminal tab.
///
/// Every entry resolves the clicked terminal *by entity* at click time, so
/// the actions stay correct even if tabs were opened, closed or reordered
/// while the menu was open. The `action` on each entry is only used to
/// display the keyboard shortcut; the click handlers do the actual work.
fn build_terminal_tab_menu(
    menu: PopupMenu,
    workspace: &Entity<Workspace>,
    dock: TerminalDock,
    terminal: &Entity<Terminal>,
    _window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let ws = workspace.read(cx);
    let (tabs, pinned_count) = match dock {
        TerminalDock::Bottom => (&ws.terminal_tabs, ws.terminal_pinned_count),
        TerminalDock::Right => (&ws.terminal_right_tabs, ws.terminal_right_pinned_count),
    };
    let Some(ix) = tabs.iter().position(|t| t == terminal) else {
        // The tab vanished between right-click and menu build; show nothing.
        return menu;
    };
    let total = tabs.len();
    let has_left = ix > 0;
    let has_right = ix + 1 < total;
    // "Clean" terminals are ones whose process already exited. Pinned tabs
    // are spared by the bulk close actions, so they don't count here.
    let has_clean = tabs
        .iter()
        .enumerate()
        .any(|(i, term)| i >= pinned_count && term.read(cx).state != TerminalState::Running);
    let is_pinned = ix < pinned_count;
    let is_read_only = terminal.read(cx).is_read_only();
    let action_context = terminal.read(cx).focus_handle(cx);

    let ws = workspace.downgrade();

    // One tiny macro beats nine hand-rolled closures: each expansion clones
    // the weak workspace + terminal entity and calls a Workspace method.
    macro_rules! handler {
        ($method:ident) => {{
            let ws = ws.clone();
            let term = terminal.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut gpui::App| {
                _ = ws.update(cx, |this, cx| this.$method(dock, &term, window, cx));
            }
        }};
    }

    let rename_handler = {
        let ws = ws.clone();
        let term = terminal.clone();
        move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut gpui::App| {
            let ws = ws.clone();
            let term = term.clone();
            // Deferred: dismissing the menu re-focuses the terminal, which
            // would immediately blur (and cancel) the rename input if the
            // rename started before the dismissal finished.
            window.defer(cx, move |window, cx| {
                _ = ws.update(cx, |this, cx| {
                    this.start_terminal_rename_for(dock, &term, window, cx)
                });
            });
        }
    };

    let read_only_handler = {
        let ws = ws.clone();
        let term = terminal.clone();
        move |_: &gpui::ClickEvent, _window: &mut Window, cx: &mut gpui::App| {
            _ = ws.update(cx, |this, cx| {
                this.toggle_terminal_read_only_for(dock, &term, cx)
            });
        }
    };

    menu.action_context(action_context)
        .item(
            PopupMenuItem::new("Close")
                .action(Box::new(CloseTerminal))
                .on_click(handler!(close_terminal_tab_for)),
        )
        .item(
            PopupMenuItem::new("Close Others")
                .action(Box::new(CloseOtherTerminals))
                .disabled(total == 1)
                .on_click(handler!(close_other_terminal_tabs_for)),
        )
        .separator()
        .item(
            PopupMenuItem::new("Close Left")
                .action(Box::new(CloseTerminalsLeft))
                .disabled(!has_left)
                .on_click(handler!(close_terminal_tabs_left_for)),
        )
        .item(
            PopupMenuItem::new("Close Right")
                .action(Box::new(CloseTerminalsRight))
                .disabled(!has_right)
                .on_click(handler!(close_terminal_tabs_right_for)),
        )
        .separator()
        .item(
            PopupMenuItem::new("Close Clean")
                .action(Box::new(CloseCleanTerminals))
                .disabled(!has_clean)
                .on_click(handler!(close_clean_terminal_tabs_for)),
        )
        .item(
            PopupMenuItem::new("Close All")
                .action(Box::new(CloseAllTerminals))
                .on_click(handler!(close_all_terminal_tabs_for)),
        )
        .separator()
        .item(
            PopupMenuItem::new(if is_read_only {
                "Make Tab Editable"
            } else {
                "Make Tab Read-Only"
            })
            .on_click(read_only_handler),
        )
        .separator()
        .item(
            PopupMenuItem::new(if is_pinned { "Unpin Tab" } else { "Pin Tab" })
                .action(Box::new(ToggleTerminalPin))
                .on_click(handler!(toggle_terminal_pin_for)),
        )
        .separator()
        .item(PopupMenuItem::new("Rename").on_click(rename_handler))
}

fn render_new_terminal_button(
    dock: TerminalDock,
    _t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    div()
        .id(dock.namespaced("term-new-btn"))
        .w(px(28.0))
        .h_full()
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .border_b_1()
        .border_color(rgba(TAB_BAR_BORDER_BOTTOM))
        .hover(|s| s.bg(rgba(TAB_INACTIVE_HOVER)))
        .child(
            div()
                .text_size(px(16.0))
                .text_color(rgba(0x8b949eff))
                .line_height(px(16.0))
                .child("+"),
        )
        .on_click(cx.listener(move |this, _, window, cx| match dock {
            TerminalDock::Bottom => this.new_terminal(window, cx),
            TerminalDock::Right => this.new_terminal_right(window, cx),
        }))
}

fn render_hide_panel_button(
    dock: TerminalDock,
    _t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    div()
        .id(dock.namespaced("term-hide-btn"))
        .w(px(28.0))
        .h_full()
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .border_b_1()
        .border_color(rgba(TAB_BAR_BORDER_BOTTOM))
        .hover(|s| s.bg(rgba(TAB_INACTIVE_HOVER)))
        .child(
            svg()
                .path("ui_icons/panel_close.svg")
                .w(px(14.0))
                .h(px(14.0))
                .text_color(rgba(0x8b949eff)),
        )
        .on_click(cx.listener(move |this, _, window, cx| match dock {
            TerminalDock::Bottom => this.hide_terminal(window, cx),
            TerminalDock::Right => this.hide_terminal_right(window, cx),
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Keystroke;

    /// An already-recorded state must never be probed again: `poll_terminal_
    /// processes` runs on every render, so returning a state here would make a
    /// finished terminal schedule a re-render on every single frame.
    #[test]
    fn finished_terminal_is_not_reprobed() {
        assert_eq!(
            next_state_after_probe(TerminalState::Exited(0), Some(1)),
            None
        );
        assert_eq!(next_state_after_probe(TerminalState::Error, Some(1)), None);
    }

    /// A running terminal with no pid (the PTY failed to spawn) has nothing to
    /// probe either, so it must not spin either.
    #[test]
    fn running_terminal_without_pid_is_not_probed() {
        assert_eq!(next_state_after_probe(TerminalState::Running, None), None);
    }

    /// The shell running this test is definitely alive, so probing it must not
    /// report a state change.
    #[test]
    fn live_process_stays_running() {
        let pid = std::process::id();
        assert_eq!(
            next_state_after_probe(TerminalState::Running, Some(pid)),
            None
        );
    }

    #[test]
    fn test_capital_letters_with_shift() {
        let keystroke = Keystroke::parse("shift-a").unwrap();
        let bytes = terminal_keystroke_to_bytes(&keystroke);
        assert_eq!(bytes, Some(b"A".to_vec()));

        let keystroke_z = Keystroke::parse("shift-z").unwrap();
        let bytes_z = terminal_keystroke_to_bytes(&keystroke_z);
        assert_eq!(bytes_z, Some(b"Z".to_vec()));
    }

    #[test]
    fn test_caps_lock_capitalization() {
        let a = Keystroke::parse("a").unwrap();
        let bytes = gpui_terminal::input::keystroke_to_bytes_with_caps(
            &a,
            alacritty_terminal::term::TermMode::empty(),
            true,
        );
        assert_eq!(bytes, Some(b"A".to_vec()));

        let shift_a = Keystroke::parse("shift-a").unwrap();
        let bytes_inverted = gpui_terminal::input::keystroke_to_bytes_with_caps(
            &shift_a,
            alacritty_terminal::term::TermMode::empty(),
            true,
        );
        assert_eq!(bytes_inverted, Some(b"a".to_vec()));
    }

    #[test]
    fn test_lowercase_letters() {
        let keystroke = Keystroke::parse("a").unwrap();
        let bytes = terminal_keystroke_to_bytes(&keystroke);
        assert_eq!(bytes, Some(b"a".to_vec()));

        let keystroke_z = Keystroke::parse("z").unwrap();
        let bytes_z = terminal_keystroke_to_bytes(&keystroke_z);
        assert_eq!(bytes_z, Some(b"z".to_vec()));
    }

    #[test]
    fn test_shifted_symbols() {
        let k1 = Keystroke::parse("shift-1").unwrap();
        assert_eq!(terminal_keystroke_to_bytes(&k1), Some(b"!".to_vec()));

        let k_dash = Keystroke::parse("shift--").unwrap();
        assert_eq!(terminal_keystroke_to_bytes(&k_dash), Some(b"_".to_vec()));
    }

    #[test]
    fn test_ctrl_combinations() {
        let ctrl_c = Keystroke::parse("ctrl-c").unwrap();
        assert_eq!(terminal_keystroke_to_bytes(&ctrl_c), Some(vec![0x03]));

        let ctrl_a = Keystroke::parse("ctrl-a").unwrap();
        assert_eq!(terminal_keystroke_to_bytes(&ctrl_a), Some(vec![0x01]));

        let ctrl_z = Keystroke::parse("ctrl-z").unwrap();
        assert_eq!(terminal_keystroke_to_bytes(&ctrl_z), Some(vec![0x1a]));
    }

    #[test]
    fn test_special_keys() {
        let enter = Keystroke::parse("enter").unwrap();
        assert_eq!(terminal_keystroke_to_bytes(&enter), Some(b"\r".to_vec()));

        let backspace = Keystroke::parse("backspace").unwrap();
        assert_eq!(
            terminal_keystroke_to_bytes(&backspace),
            Some(b"\x7f".to_vec())
        );

        let tab = Keystroke::parse("tab").unwrap();
        assert_eq!(terminal_keystroke_to_bytes(&tab), Some(b"\t".to_vec()));

        let shift_tab = Keystroke::parse("shift-tab").unwrap();
        assert_eq!(
            terminal_keystroke_to_bytes(&shift_tab),
            Some(b"\x1b[Z".to_vec())
        );

        let up = Keystroke::parse("up").unwrap();
        assert_eq!(terminal_keystroke_to_bytes(&up), Some(b"\x1b[A".to_vec()));
    }
}
