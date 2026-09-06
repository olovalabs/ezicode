//! Application entry point.
//!
//! Boots GPUI, registers keybindings/themes/fonts, then opens the single
//! window hosting [`workspace::Workspace`]. All state and behavior live in
//! the other modules — keep this file thin.

mod actions;
mod assets;
mod file_icons;
mod fs_tree;
mod git;
mod lang;
mod lsp;
mod settings;
mod terminal;
mod theme;
mod ui;
mod workspace;

use std::sync::Arc;

use gpui::{
    px, size, App, AppContext, Application, Bounds, KeyBinding, WindowBounds, WindowDecorations,
    WindowOptions,
};
use gpui_component::{Root, TitleBar};

use actions::*;
use assets::{load_embedded_fonts, sync_component_fonts, CombinedAssets};
use workspace::Workspace;

/// Log every panic to a file (and stderr) so an "app just closed itself"
/// report comes with the actual panic message and backtrace instead of a
/// vanishing window. Panics on the main thread terminate the process, so
/// without this a UI-thread panic looks like a silent exit.
fn install_panic_logger() {
    let log_path = std::env::temp_dir().join("olova-editor-panic.log");
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic payload".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "<unknown>".to_string());
        let entry = format!(
            "\n=== {} ===\npanic in thread '{name}' at {location}\n{msg}\nbacktrace:\n{}\n",
            chrono_like_timestamp(),
            std::backtrace::Backtrace::force_capture()
        );
        eprintln!("\n[PANIC] {msg} (at {location}) — see {}", log_path.display());
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log_path) {
            let _ = f.write_all(entry.as_bytes());
        }
    }));
}

/// Wall-clock timestamp without pulling a date crate into the binary.
fn chrono_like_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

fn main() {
    install_panic_logger();
    Application::new()
        .with_assets(CombinedAssets)
        .run(|cx: &mut App| {
            gpui_component::init(cx);
            // Windows/Linux keymap style (like Zed's default-linux.json):
            // `ctrl` modifiers only. On Windows GPUI maps `cmd` to the
            // Windows key, which produced wrong "Win+X" menu labels.
            cx.bind_keys([
                KeyBinding::new("ctrl-s", Save, None),
                KeyBinding::new("ctrl-n", NewFile, None),
                KeyBinding::new("ctrl-o", OpenFile, None),
                KeyBinding::new("ctrl-,", OpenSettings, None),
                KeyBinding::new("ctrl-`", ToggleTerminal, None),
                KeyBinding::new("ctrl-j", ToggleTerminal, None),
                KeyBinding::new("ctrl-shift-`", NewTerminal, None),
                // Terminal navigation (Zed-style: Alt+arrows, Alt+1..5)
                KeyBinding::new("alt-right", NextTerminal, None),
                KeyBinding::new("alt-left", PrevTerminal, None),
                KeyBinding::new("ctrl-shift-w", CloseTerminal, None),
                // Quick terminal tab switching (Zed-style: Alt+1..5)
                KeyBinding::new("alt-1", TerminalTab1, None),
                KeyBinding::new("alt-2", TerminalTab2, None),
                KeyBinding::new("alt-3", TerminalTab3, None),
                KeyBinding::new("alt-4", TerminalTab4, None),
                KeyBinding::new("alt-5", TerminalTab5, None),
                // Clear terminal screen (sends ANSI clear + scrollback erase)
                KeyBinding::new("ctrl-shift-k", ClearTerminal, None),
                KeyBinding::new("ctrl-b", ToggleSidebar, None),
                KeyBinding::new("ctrl-shift-e", ShowExplorer, None),
                KeyBinding::new("ctrl-shift-f", ShowSearch, None),
                KeyBinding::new("ctrl-shift-g", ShowGit, None),
                KeyBinding::new("ctrl-shift-x", ShowExtensions, None),
                // Tab keybindings
                KeyBinding::new("ctrl-w", CloseTab, None),
                KeyBinding::new("ctrl-tab", NextTab, None),
                KeyBinding::new("ctrl-shift-tab", PrevTab, None),
                // Font zoom keybindings (Zed-compatible)
                KeyBinding::new("ctrl-=", IncreaseFontSize, None),
                KeyBinding::new("ctrl-+", IncreaseFontSize, None),
                KeyBinding::new("ctrl-shift-+", IncreaseFontSize, None),
                KeyBinding::new("ctrl-shift-=", IncreaseFontSize, None),
                KeyBinding::new("ctrl--", DecreaseFontSize, None),
                KeyBinding::new("ctrl-_", DecreaseFontSize, None),
                KeyBinding::new("ctrl-0", ResetFontSize, None),
                KeyBinding::new("ctrl-alt-c", CopyDiagnostic, None),
                // Format the current document with its language server
                // (VS Code / Zed compatible).
                KeyBinding::new("shift-alt-f", FormatDocument, Some("Workspace")),
            ]);
            // Go to definition (F12, like VS Code). The editor library
            // resolves definitions with a modifier-hover first and jumps on
            // this action.
            cx.bind_keys([KeyBinding::new(
                "f12",
                gpui_component::input::GoToDefinition,
                Some("Input"),
            )]);

            // Start on GitHub Dark and paint tree-sitter captures with the
            // theme's exact syntax palette from day one.
            gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, cx);
            let default_theme = &theme::all()[theme::default_index()];
            gpui_component::Theme::global_mut(cx).highlight_theme =
                Arc::new(default_theme.highlight_theme());

            // Initialize Tree-Sitter language definitions with rich TSX/JSX queries.
            lang::init_languages();

            // Zed's fonts: IBM Plex Sans (UI) + Lilex (code).
            load_embedded_fonts(cx);
            sync_component_fonts(cx);

            let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitleBar::title_bar_options()),
                    window_decorations: Some(WindowDecorations::Client),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| Workspace::new(window, cx));
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .unwrap();
            cx.activate(true);
        });
}
