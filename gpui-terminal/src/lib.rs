//! A GPU-accelerated terminal emulator component for [GPUI].
//!
//! The emulation layer is `alacritty_terminal`; this crate wires it up to GPUI
//! rendering, input, selection, search, hyperlinks and vi mode.
//!
//! [GPUI]: https://github.com/zed-industries/zed

pub mod clipboard;
pub mod colors;
pub mod event;
pub mod hyperlink;
pub mod input;
pub mod mouse;
pub mod render;
pub mod search;
pub mod terminal;
pub mod view;

pub use alacritty_terminal;
pub use clipboard::Clipboard;
pub use colors::{ColorPalette, ColorPaletteBuilder, hsla_to_rgb};
pub use event::{
    ClipboardFormatter, GpuiEventProxy, PtyWriter, TerminalEvent, TerminalNotifier,
};
pub use hyperlink::{HyperlinkKind, HyperlinkMatch, RegexSearches};
pub use render::{PaintContext, TerminalRenderer};
pub use search::SearchState;
pub use terminal::{TermDimensions, TerminalState};
pub use view::{
    BellCallback, ClipboardStoreCallback, ExitCallback, KeyHandler, LinkCallback, ResizeCallback,
    TerminalConfig, TerminalView, TitleCallback,
};
