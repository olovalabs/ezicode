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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalState {

    Running,

    Exited(i32),

    Error,
}

pub struct Terminal {
    pub view: Entity<TerminalView>,

    pub name: String,

    pub state: TerminalState,

    pub working_dir: Option<PathBuf>,

    #[allow(dead_code)]
    pub shell_path: String,

    pub pid: Option<u32>,

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

    fn create_dummy_pty() -> (Box<dyn std::io::Read + Send>, Box<dyn std::io::Write + Send>) {

        (
            Box::new(std::io::empty()),
            Box::new(std::io::sink()),
        )
    }

    pub fn check_process_exit(&mut self) -> bool {
        if self.state != TerminalState::Running {
            return true;
        }

        if let Some(pid) = self.pid {
            if !Self::process_is_alive(pid) {
                self.state = TerminalState::Exited(0);
                return true;
            }
        }
        false
    }

    #[cfg(unix)]
    fn process_is_alive(pid: u32) -> bool {

        unsafe {
            libc::kill(pid as i32, 0) == 0
        }
    }

    #[cfg(windows)]
    fn process_is_alive(pid: u32) -> bool {

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

    #[allow(dead_code)]
    pub fn rename(&mut self, new_name: String, cx: &mut Context<Self>) {
        self.name = new_name;
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

pub fn render_terminal_panel(
    tabs: &[Entity<Terminal>],
    active: usize,
    maximized: bool,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let active_view = tabs
        .get(active)
        .map(|term| term.read(cx).view.clone())
        .or_else(|| tabs.first().map(|term| term.read(cx).view.clone()));

    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(rgba(TAB_BAR_BG))
        .child(render_terminal_tab_bar(tabs, active, maximized, t, cx))
        .child(
            div()
                .flex_1()
                .w_full()
                .min_h(px(0.0))
                .overflow_hidden()
                .bg(rgba(TAB_BAR_BG))
                .when_some(active_view, |d, view| d.child(view)),
        )
}

fn render_terminal_tab_bar(
    tabs: &[Entity<Terminal>],
    active: usize,
    maximized: bool,
    t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let tabs_root = div()
        .id("terminal-tab-bar")
        .h(px(28.0))
        .w_full()
        .flex()
        .flex_row()
        .bg(rgba(TAB_BAR_BG))
        .border_t_1()
        .border_color(rgba(TAB_BAR_BORDER_TOP));

    let with_tabs = tabs_root.children(tabs.iter().enumerate().map(|(idx, _)| {
        let is_active = idx == active;
        let term = tabs[idx].read(cx);
        let name = term.name.clone();
        let state = term.state;
        render_terminal_tab(name, state, idx, is_active, t, cx)
    }));

    with_tabs
        .child(render_new_terminal_button(t, cx))
        .child(
            div()
                .flex_1()
                .h_full()
                .border_b_1()
                .border_color(rgba(TAB_BAR_BORDER_BOTTOM)),
        )
        .child(
            div()
                .h_full()
                .flex()
                .items_center()
                .border_b_1()
                .border_color(rgba(TAB_BAR_BORDER_BOTTOM))
                .child(div().w(px(1.0)).h(px(14.0)).bg(rgba(0x383e47ff))),
        )
        .child(render_maximize_terminal_button(maximized, t, cx))
        .child(render_hide_panel_button(t, cx))
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

fn render_terminal_tab(
    name: String,
    state: TerminalState,
    index: usize,
    is_active: bool,
    _t: &Colors,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let tab_bg = if is_active { TAB_ACTIVE_BG } else { TAB_BAR_BG };
    let text_color = if is_active { TAB_ACTIVE_TEXT } else { TAB_INACTIVE_TEXT };

    let icon_path = if name.contains("bash") {
        "file_icons/file_type_shell.svg"
    } else {
        "file_icons/file_type_powershell.svg"
    };

    let display_name = match state {
        TerminalState::Running => name,
        TerminalState::Exited(code) => format!("{name} (exit {code})"),
        TerminalState::Error => format!("{name} (error)"),
    };

    let mut tab = div()
        .id(("terminal-tab", index))
        .w(px(180.0))
        .h_full()
        .pl(px(12.0))
        .pr(px(8.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .bg(rgba(tab_bg))
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, window, cx| {
            this.activate_terminal(index, window, cx);
        }));

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

    tab = tab
        .child(crate::ui::common::icon_img(icon_path, 15.0))
        .child(
            div()
                .child(display_name)
                .text_size(px(12.0))
                .text_color(rgba(text_color))
                .overflow_x_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .flex_1(),
        );

    if is_active {
        tab = tab.child(
            div()
                .id(("terminal-tab-close", index))
                .w(px(16.0))
                .h(px(16.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(2.0))
                .cursor_pointer()
                .hover(|h| h.bg(rgba(TAB_CLOSE_HOVER)))
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgba(TAB_ACTIVE_TEXT))
                        .line_height(px(11.0))
                        .child("✕"),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.close_terminal(index, window, cx);
                })),
        );
    }

    tab
}

fn render_new_terminal_button(_t: &Colors, cx: &mut Context<Workspace>) -> impl IntoElement {
    div()
        .id("term-new-btn")
        .w(px(28.0))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .border_b_1()
        .border_color(rgba(TAB_BAR_BORDER_BOTTOM))
        .hover(|s| s.bg(rgba(TAB_INACTIVE_HOVER)))
        .child(
            div()
                .text_size(px(14.0))
                .text_color(rgba(0x8b949eff))
                .line_height(px(14.0))
                .child("+"),
        )
        .on_click(cx.listener(|this, _, window, cx| {
            this.new_terminal(window, cx);
        }))
}

fn render_hide_panel_button(_t: &Colors, cx: &mut Context<Workspace>) -> impl IntoElement {
    div()
        .id("term-hide-btn")
        .w(px(28.0))
        .h_full()
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
        assert_eq!(terminal_keystroke_to_bytes(&backspace), Some(b"\x7f".to_vec()));

        let tab = Keystroke::parse("tab").unwrap();
        assert_eq!(terminal_keystroke_to_bytes(&tab), Some(b"\t".to_vec()));

        let shift_tab = Keystroke::parse("shift-tab").unwrap();
        assert_eq!(terminal_keystroke_to_bytes(&shift_tab), Some(b"\x1b[Z".to_vec()));

        let up = Keystroke::parse("up").unwrap();
        assert_eq!(terminal_keystroke_to_bytes(&up), Some(b"\x1b[A".to_vec()));
    }
}
