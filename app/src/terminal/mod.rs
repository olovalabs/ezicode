//! Integrated Terminal powered by `gpui-terminal` (Alacritty + PTY engine).
//!
//! Uses `gpui-terminal` and `portable-pty` for production-grade terminal
//! emulation with full TUI support (opencode, vim, htop, lazygit), 24-bit
//! RGB truecolor, smooth scrolling, selection, and automatic PTY resizing.
//!
//! Architecture inspired by Zed's terminal implementation:
//! - Process lifecycle management with exit detection
//! - Working directory tracking per terminal
//! - Multiple terminal sessions with tabs
//! - Proper focus management and resize handling
//! - Terminal state indicators (running, exited, error)

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gpui::{
    div, prelude::*, px, rgba, svg, Context, Edges, Entity, FocusHandle, IntoElement, Window,
};
use gpui_terminal::{TerminalConfig, TerminalView};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

use crate::assets::MONO_FONT;
use crate::theme::Colors;
use crate::workspace::Workspace;

/// Terminal process state for lifecycle management (like Zed's TaskStatus)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalState {
    /// Terminal is running and accepting input
    Running,
    /// Terminal process has exited (shell closed, command finished)
    Exited(i32),
    /// Terminal encountered an error during initialization or runtime
    Error,
}

pub struct Terminal {
    pub view: Entity<TerminalView>,
    /// Shell display name shown on the tab (e.g. "PowerShell 1", "bash 2")
    pub name: String,
    /// Current process state (running, exited, error)
    pub state: TerminalState,
    /// Working directory for this terminal session
    pub working_dir: Option<PathBuf>,
    /// Shell executable path (e.g. "/bin/bash", "powershell.exe")
    #[allow(dead_code)]
    pub shell_path: String,
    /// Process ID of the shell (for monitoring)
    pub pid: Option<u32>,
    /// Shared handle to the PTY writer, used for programmatic input
    /// (clearing screen, pasting, sending escape sequences).
    pty_writer: Arc<Mutex<Box<dyn std::io::Write + Send>>>,
}



struct SharedWriter(Arc<Mutex<Box<dyn std::io::Write + Send>>>);

impl std::io::Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut guard = self
            .0
            .lock()
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "lock poisoned"))?;
        guard.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let mut guard = self
            .0
            .lock()
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "lock poisoned"))?;
        guard.flush()
    }
}

/// Get the user's home directory as a fallback working directory.
/// Zed uses the project root or home directory for new terminals.
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



/// Convert a GPUI keystroke to terminal escape sequence bytes with complete support
/// for Shift and Caps Lock capitalization, shifted symbols, and control keys.
#[allow(dead_code)]
pub fn terminal_keystroke_to_bytes(keystroke: &gpui::Keystroke) -> Option<Vec<u8>> {
    gpui_terminal::input::keystroke_to_bytes(keystroke, alacritty_terminal::term::TermMode::empty())
}

impl Terminal {
    /// Create a new terminal session. Spawns a shell process in the given
    /// working directory (or the user's home directory if `root_dir` is `None`),
    /// wires up the PTY reader/writer to a `TerminalView`, and installs a
    /// resize callback so the PTY dimensions always match the view.
    ///
    /// Inspired by Zed's terminal creation:
    /// - Tracks the working directory
    /// - Monitors the child process for exit
    /// - Sets TERM/COLORTERM for proper color support
    /// - Handles errors gracefully (marks state as Error instead of panicking)
    pub fn new(
        root_dir: Option<&Path>,
        name: String,
        palette: gpui_terminal::ColorPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Determine shell and working directory (Zed-style: project root → home)
        let (shell_cmd, shell_path) = Self::detect_shell();
        let working_dir = root_dir
            .map(|p| p.to_path_buf())
            .or_else(|| dirs_home().map(|h| h.to_path_buf()));

        // Open the PTY pair
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
                // Create a view that shows the error instead of crashing
                let config = TerminalConfig {
                    font_family: MONO_FONT.into(),
                    font_size: px(13.5),
                    cols: 80,
                    rows: 24,
                    scrollback: 10000,
                    line_height_multiplier: 1.2,
                    padding: Edges::all(px(6.0)),
                    colors: palette,
                };
                let (dummy_reader, dummy_writer) = Self::create_dummy_pty();
                let pty_writer = Arc::new(Mutex::new(dummy_writer));
                let view = cx.new(|cx| {
                    TerminalView::new(SharedWriter(pty_writer.clone()), dummy_reader, config, cx)
                });
                return Self {
                    view,
                    name,
                    state: TerminalState::Error,
                    working_dir,
                    shell_path: shell_path.clone(),
                    pid: None,
                    pty_writer,
                };
            }
        };

        // Build the shell command with proper environment
        let mut cmd = CommandBuilder::new(&shell_cmd);
        if let Some(dir) = &working_dir {
            cmd.cwd(dir);
        }
        // Zed-compatible environment variables for proper terminal behavior
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "zed");
        cmd.env("TERM_PROGRAM_VERSION", "0.1.0");
        cmd.env("LANG", "en_US.UTF-8");
        cmd.env("LC_ALL", "en_US.UTF-8");

        // Spawn the shell process. We only need the child handle briefly to
        // extract the PID; the process stays alive because the PTY master
        // remains open (stored below). Dropping the Child handle does NOT
        // kill the process in portable-pty.
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
                };
                let (dummy_reader, dummy_writer) = Self::create_dummy_pty();
                let pty_writer = Arc::new(Mutex::new(dummy_writer));
                let view = cx.new(|cx| {
                    TerminalView::new(SharedWriter(pty_writer.clone()), dummy_reader, config, cx)
                });
                return Self {
                    view,
                    name,
                    state: TerminalState::Error,
                    working_dir,
                    shell_path,
                    pid: None,
                    pty_writer,
                };
            }
        };

        // Extract the PID for process monitoring (Zed-style lifecycle tracking).
        // portable-pty's Child::process_id() returns Option<u32>.
        let pid = child.process_id();

        let writer = pair.master.take_writer().expect("take writer failed");
        let reader = pair.master.try_clone_reader().expect("clone reader failed");
        let pty_master = Arc::new(Mutex::new(pair.master));

        let config = TerminalConfig {
            font_family: MONO_FONT.into(),
            font_size: px(13.5),
            cols: 80,
            rows: 24,
            scrollback: 10_000,
            line_height_multiplier: 1.2,
            padding: Edges::all(px(6.0)),
            colors: palette,
        };

        // Resize callback: when the TerminalView changes size, propagate to PTY
        // so the shell reflows its output (Zed-style automatic resize).
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

        let view = cx.new(|cx| {
            TerminalView::new(SharedWriter(pty_writer.clone()), reader, config, cx)
                .with_resize_callback(resize_callback)
        });

        view.read(cx).focus_handle().focus(window);

        // Note: `child` (the Child handle) is intentionally dropped here.
        // In portable-pty, dropping the Child handle does NOT kill the process;
        // the shell stays alive as long as the PTY master is open (stored in
        // `pty_master` above). This matches Zed's approach.
        drop(child);

        Self {
            view,
            name,
            state: TerminalState::Running,
            working_dir,
            shell_path,
            pid,
            pty_writer,
        }
    }

    /// Detect the system's default shell. Returns (command_path, display_name).
    pub fn detect_shell() -> (String, String) {
        if cfg!(windows) {
            // On Windows, prefer Git Bash if installed because it provides a complete Unix
            // TUI environment with native vim, nano, less, tig, git, etc.
            for path in &[
                r"C:\Program Files\Git\bin\bash.exe",
                r"C:\Program Files (x86)\Git\bin\bash.exe",
            ] {
                if std::path::Path::new(path).exists() {
                    return (path.to_string(), "bash".to_string());
                }
            }
            // Fall back to standard Windows PowerShell
            ("powershell.exe".to_string(), "PowerShell".to_string())
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

    /// Helper to get just the detected shell display name (e.g. "bash", "PowerShell", "zsh")
    pub fn detect_shell_name() -> String {
        Self::detect_shell().1
    }

    /// Create a dummy PTY pair for error recovery. This lets us create a
    /// TerminalView that can display an error message without panicking.
    fn create_dummy_pty() -> (Box<dyn std::io::Read + Send>, Box<dyn std::io::Write + Send>) {
        // Use an empty reader and a sink writer — the terminal will render
        // nothing and any input is discarded, but the view stays valid.
        (
            Box::new(std::io::empty()),
            Box::new(std::io::sink()),
        )
    }

    /// Check if the shell process has exited. Called periodically from the
    /// workspace to update the terminal state (Zed-style process monitoring).
    pub fn check_process_exit(&mut self) -> bool {
        if self.state != TerminalState::Running {
            return true; // Already exited
        }
        // If we have a PID, check if the process is still alive
        if let Some(pid) = self.pid {
            if !Self::process_is_alive(pid) {
                self.state = TerminalState::Exited(0);
                return true;
            }
        }
        false
    }

    /// Platform-specific process liveness check.
    #[cfg(unix)]
    fn process_is_alive(pid: u32) -> bool {
        // kill(pid, 0) returns 0 if the process exists, -1 if it doesn't
        unsafe {
            libc::kill(pid as i32, 0) == 0
        }
    }

    #[cfg(windows)]
    fn process_is_alive(pid: u32) -> bool {
        // On Windows, check if the process handle is still valid
        extern "system" {
            fn OpenProcess(dwDesiredAccess: u32, bInheritHandle: i32, dwProcessId: u32) -> *mut std::ffi::c_void;
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
        true // Unknown platform, assume alive
    }

    /// Send raw bytes to the terminal's PTY stdin. Used for programmatic
    /// input like clearing the screen, pasting, or injecting escape sequences.
    /// Returns `true` if the bytes were successfully written.
    pub fn send_bytes(&self, bytes: &[u8]) -> bool {
        if let Ok(mut writer) = self.pty_writer.lock() {
            use std::io::Write;
            writer.write_all(bytes).is_ok() && writer.flush().is_ok()
        } else {
            false
        }
    }

    /// Rename the terminal tab. Zed supports renaming terminals via
    /// right-click → "Rename" or the `terminal: rename` command.
    #[allow(dead_code)]
    pub fn rename(&mut self, new_name: String, cx: &mut Context<Self>) {
        self.name = new_name;
        cx.notify();
    }

    /// Dynamically update the terminal color palette when the active theme changes.
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

/// Render the terminal panel (Zed-style: tab bar + active terminal view).
///
/// Layout:
/// - Top: terminal tab bar with session tabs, [+] button, and panel controls
/// - Bottom: active terminal view (fills remaining space)
///
/// The panel hides itself when no terminal tabs exist (all closed).
pub fn render_terminal_panel(
    tabs: &[Entity<Terminal>],
    active: usize,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    // Guard: only render the body when at least one tab exists. The caller
    // hides the panel when the last tab is closed, so this is defensive.
    let active_view = tabs
        .get(active)
        .map(|term| term.read(cx).view.clone())
        .or_else(|| tabs.first().map(|term| term.read(cx).view.clone()));

    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(rgba(t.terminal_bg))
        .border_t_1()
        .border_color(rgba(t.border_variant))
        // Terminal tab strip (Zed-style: one tab per shell, [+] to add,
        // chevron to collapse/hide the panel)
        .child(render_terminal_tab_bar(tabs, active, t, cx))
        // Embedded Terminal View from gpui-terminal (active tab only).
        // The view fills all remaining vertical space and handles its own
        // scrolling, cursor rendering, and PTY I/O.
        .child(
            div()
                .flex_1()
                .w_full()
                .min_h(px(0.0))
                .overflow_hidden()
                .bg(rgba(t.terminal_bg))
                .when_some(active_view, |d, view| d.child(view)),
        )
}

/// Terminal tab strip at the top of the panel (Zed-style).
///
/// Layout: [terminal icon] [tab1] [tab2] ... [+] [split] [collapse]
///
/// Each tab shows the shell name and a status indicator. The [+] button
/// opens a new terminal session. Close buttons appear on hover (VS Code/Zed).
fn render_terminal_tab_bar(
    tabs: &[Entity<Terminal>],
    active: usize,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let tabs_root = div()
        .id("terminal-tab-bar")
        .h(px(28.0))
        .w_full()
        .px(px(6.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(2.0))
        .bg(rgba(t.toolbar))
        .border_b_1()
        .border_color(rgba(t.border_variant))
        .child(
            // Terminal icon anchoring the strip (like Zed's panel icon)
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .mr(px(4.0))
                .child(
                    svg()
                        .path("ui_icons/terminal_tint.svg")
                        .w(px(13.0))
                        .h(px(13.0))
                        .text_color(rgba(t.icon)),
                ),
        );

    let with_tabs = tabs_root.children(tabs.iter().enumerate().map(|(idx, _)| {
        let is_active = idx == active;
        let term = tabs[idx].read(cx);
        let name = term.name.clone();
        let state = term.state;
        render_terminal_tab(name, state, idx, is_active, t, cx)
    }));

    with_tabs
        .child(div().flex_1()) // Spacer pushes buttons to the right
        .child(render_new_terminal_button(t, cx))
        .child(render_hide_panel_button(t, cx))
}

/// A single terminal tab with status indicator and close button (Zed-style).
///
/// Shows:
/// - Shell name (e.g. "bash 1", "PowerShell 2")
/// - Status dot (green=running, red=exited, yellow=error)
/// - Close button (×) visible on hover
///
/// Clicking the tab activates it. Clicking × kills that shell session.
fn render_terminal_tab(
    name: String,
    state: TerminalState,
    index: usize,
    is_active: bool,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let bg = if is_active { t.tab_active_bg } else { t.tab_inactive_bg };
    let fg = if is_active { t.tab_active_fg } else { t.tab_inactive_fg };

    // Status indicator color based on terminal state (Zed-style)
    let status_color = match state {
        TerminalState::Running => 0xFF_4CAF50, // Green dot
        TerminalState::Exited(_) => 0xFF_F44336, // Red dot
        TerminalState::Error => 0xFF_FF9800,     // Orange dot
    };

    // Display name with exit code if exited
    let display_name = match state {
        TerminalState::Running => name,
        TerminalState::Exited(code) => format!("{name} (exit {code})"),
        TerminalState::Error => format!("{name} (error)"),
    };

    div()
        .id(("terminal-tab", index))
        .h(px(22.0))
        .pl(px(8.0))
        .pr(px(4.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .rounded(px(4.0))
        .bg(rgba(bg))
        .cursor_pointer()
        .text_size(px(11.5))
        .text_color(rgba(fg))
        .on_click(cx.listener(move |this, _, window, cx| {
            this.activate_terminal(index, window, cx);
        }))
        .hover(|h| if is_active { h } else { h.bg(rgba(t.element_hover)) })
        // Status indicator dot (Zed-style process state)
        .child(
            div()
                .w(px(6.0))
                .h(px(6.0))
                .rounded_full()
                .bg(rgba(status_color))
                .flex_shrink_0(),
        )
        // Tab label
        .child(
            div()
                .child(display_name)
                .overflow_x_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .max_w(px(120.0)),
        )
        // Close button (visible on hover, like VS Code/Zed)
        .child(
            div()
                .id(("terminal-tab-close", index))
                .w(px(16.0))
                .h(px(16.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.0))
                .cursor_pointer()
                .opacity(0.5)
                .hover(|h| h.opacity(1.0).bg(rgba(t.element_hover)))
                .child(div().text_size(px(11.0)).text_color(rgba(fg)).child("×"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.close_terminal(index, window, cx);
                })),
        )
}

/// "+" button that spawns a new terminal tab.
fn render_new_terminal_button(t: &Colors, cx: &mut Context<Workspace>) -> impl IntoElement {
    div()
        .id("term-new-btn")
        .w(px(22.0))
        .h(px(20.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(t.element_hover)))
        .text_size(px(15.0))
        .text_color(rgba(t.text_muted))
        .child("+")
        .on_click(cx.listener(|this, _, window, cx| {
            this.new_terminal(window, cx);
        }))
}

/// Chevron button that hides the panel while keeping all shell sessions alive
/// (equivalent to the Ctrl+` toggle, but mouse driven).
fn render_hide_panel_button(t: &Colors, cx: &mut Context<Workspace>) -> impl IntoElement {
    div()
        .id("term-hide-btn")
        .w(px(22.0))
        .h(px(20.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(t.element_hover)))
        .child(
            svg()
                .path("ui_icons/chevron-down_tint.svg")
                .w(px(14.0))
                .h(px(14.0))
                .text_color(rgba(t.text_muted)),
        )
        .on_click(cx.listener(|this, _, window, cx| {
            this.hide_terminal(window, cx);
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Keystroke;

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
        assert_eq!(terminal_keystroke_to_bytes(&backspace), Some(b"\x7f".to_vec()));

        let tab = Keystroke::parse("tab").unwrap();
        assert_eq!(terminal_keystroke_to_bytes(&tab), Some(b"\t".to_vec()));

        let shift_tab = Keystroke::parse("shift-tab").unwrap();
        assert_eq!(terminal_keystroke_to_bytes(&shift_tab), Some(b"\x1b[Z".to_vec()));

        let up = Keystroke::parse("up").unwrap();
        assert_eq!(terminal_keystroke_to_bytes(&up), Some(b"\x1b[A".to_vec()));
    }
}

