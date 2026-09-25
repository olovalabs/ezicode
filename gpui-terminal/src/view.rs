//! The GPUI element that owns a terminal session.
//!
//! Feature set, modelled on Zed's `terminal` / `terminal_view` crates:
//!
//! * Full alacritty VTE emulation with a real, configurable scrollback.
//! * Terminal queries answered on the PTY (cursor position, device attributes,
//!   text-area size, colour queries) so TUIs behave.
//! * Auto-scroll: output pins the viewport to the bottom, typing jumps back to
//!   the bottom, dragging a selection past the edge keeps scrolling.
//! * Alacritty-backed selection: character, word (semantic), line and block,
//!   scrollback aware, with optional copy-on-select.
//! * Mouse reporting (X10/SGR/UTF-8), motion & drag reporting, alternate
//!   scroll, shift to bypass.
//! * Hyperlinks: `OSC 8`, URLs and `path:line:col` targets, underlined on
//!   hover, opened on modifier-click.
//! * Buffer search with match highlighting and next/previous.
//! * Vi mode: motions, visual selection, yank.
//! * Blinking cursor with all cursor shapes, focus reporting (mode 1004).
//! * Bracketed paste, `OSC 52` clipboard read/write, bell and title events.

use crate::colors::ColorPalette;
use crate::event::{GpuiEventProxy, PtyWriter, TerminalEvent, TerminalNotifier};
use crate::hyperlink::{HyperlinkMatch, RegexSearches};
use crate::input::keystroke_to_bytes;
use crate::render::{PaintContext, TerminalRenderer};
use crate::search::SearchState;
use crate::terminal::TerminalState;
use alacritty_terminal::event::WindowSize as PtyWindowSize;
use alacritty_terminal::grid::{Dimensions as GridDimensions, Scroll as GridScroll};
use alacritty_terminal::index::{
    Column as GridColumn, Direction as GridDirection, Line as GridLine, Point as GridPoint,
    Side as GridSide,
};
use alacritty_terminal::selection::{SelectionType as GridSelectionType, SelectionRange};
use alacritty_terminal::term::search::Match as GridMatch;
use alacritty_terminal::term::{TermMode, viewport_to_point};
use alacritty_terminal::vi_mode::ViMotion;
use gpui::{Edges, *};
use std::io::{Read, Write};
use std::ops::RangeInclusive;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// How long a drag-past-the-edge waits between auto-scroll steps.
const DRAG_SCROLL_INTERVAL: Duration = Duration::from_millis(40);

/// Width of the invisible hit area on the right edge that grabs the scrollbar.
const SCROLLBAR_HIT_WIDTH: Pixels = px(14.0);

#[derive(Clone, Debug)]
pub struct TerminalConfig {
    pub cols: usize,
    pub rows: usize,
    pub font_family: String,
    pub font_size: Pixels,
    /// Number of lines of scrollback to retain. This is now actually honoured —
    /// it is passed to alacritty's `Config::scrolling_history`.
    pub scrollback: usize,
    pub line_height_multiplier: f32,
    pub padding: Edges<Pixels>,
    pub colors: ColorPalette,

    /// Blink the cursor when the application asks for a blinking cursor.
    pub cursor_blink: bool,
    /// Half-period of the cursor blink animation.
    pub blink_interval: Duration,
    /// Copy the selection to the clipboard as soon as the mouse is released.
    pub copy_on_select: bool,
    /// Translate wheel events into arrow keys while the alternate screen is
    /// active and the application is not doing its own mouse reporting.
    pub alternate_scroll: bool,
    /// Multiplier applied to wheel deltas.
    pub scroll_sensitivity: f32,
    /// Characters that terminate a semantic (double-click) selection.
    pub semantic_escape_chars: String,
    /// Paint the overlay scrollbar.
    pub show_scrollbar: bool,
    /// Maximum number of lines a single wheel event may scroll.
    pub max_scroll_lines: i32,
    /// Allow applications to *read* the clipboard through `OSC 52`.
    pub allow_osc52_paste: bool,
    /// Also treat `path:line:column` runs as clickable links, not just URLs.
    pub detect_path_links: bool,
    /// Overlay colour painted over selected cells.
    pub selection_color: Hsla,
    /// Overlay colour painted over search matches.
    pub search_match_color: Hsla,
    /// Overlay colour painted over the focused search match.
    pub active_search_match_color: Hsla,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            font_family: "monospace".into(),
            font_size: px(14.0),
            scrollback: 10_000,
            line_height_multiplier: 1.2,
            padding: Edges::all(px(0.0)),
            colors: ColorPalette::default(),
            cursor_blink: true,
            blink_interval: Duration::from_millis(500),
            copy_on_select: false,
            alternate_scroll: true,
            scroll_sensitivity: 1.0,
            semantic_escape_chars: ",│`|:\"' ()[]{}<>\t".into(),
            show_scrollbar: true,
            max_scroll_lines: 10,
            allow_osc52_paste: false,
            detect_path_links: true,
            selection_color: hsla(215.0 / 360.0, 0.85, 0.45, 0.35),
            search_match_color: hsla(45.0 / 360.0, 0.9, 0.5, 0.30),
            active_search_match_color: hsla(25.0 / 360.0, 0.95, 0.55, 0.55),
        }
    }
}

pub type ResizeCallback = Box<dyn Fn(usize, usize) + Send + Sync>;
pub type KeyHandler = Box<dyn Fn(&KeyDownEvent) -> bool + Send + Sync>;
pub type BellCallback = Box<dyn Fn(&mut Window, &mut Context<TerminalView>)>;
pub type TitleCallback = Box<dyn Fn(&mut Window, &mut Context<TerminalView>, &str)>;
pub type ClipboardStoreCallback = Box<dyn Fn(&mut Window, &mut Context<TerminalView>, &str)>;
pub type ExitCallback = Box<dyn Fn(&mut Window, &mut Context<TerminalView>)>;
/// Invoked when a hyperlink is activated. Return `true` to mark it handled and
/// suppress the built-in `open_url` fallback.
pub type LinkCallback =
    Box<dyn Fn(&mut Window, &mut Context<TerminalView>, &HyperlinkMatch) -> bool>;

pub struct TerminalView {
    state: TerminalState,
    renderer: TerminalRenderer,
    focus_handle: FocusHandle,
    stdin_writer: PtyWriter,
    event_rx: mpsc::Receiver<TerminalEvent>,
    config: TerminalConfig,

    #[allow(dead_code)]
    _reader_task: Task<()>,
    blink_task: Option<Task<()>>,
    drag_scroll_task: Option<Task<()>>,

    /// Shared with the event proxy so `CSI 14 t` / `CSI 18 t` can be answered.
    window_size: Arc<parking_lot::Mutex<PtyWindowSize>>,
    /// Shared with the event proxy so `OSC 4/10/11/12` can be answered.
    palette_handle: Arc<parking_lot::Mutex<ColorPalette>>,
    /// Grid size computed by the last paint pass.
    measured_grid: Arc<parking_lot::Mutex<(usize, usize)>>,
    last_bounds: Arc<parking_lot::Mutex<Bounds<Pixels>>>,

    resize_callback: Option<Arc<ResizeCallback>>,
    key_handler: Option<Arc<KeyHandler>>,
    bell_callback: Option<BellCallback>,
    title_callback: Option<TitleCallback>,
    clipboard_store_callback: Option<ClipboardStoreCallback>,
    exit_callback: Option<ExitCallback>,
    link_callback: Option<LinkCallback>,

    scroll_accumulator: f32,
    scrollbar_dragging: bool,
    scrollbar_hovered: bool,
    mouse_down_button: Option<MouseButton>,
    last_reported_cell: Option<GridPoint>,
    last_mouse_position: Point<Pixels>,
    cell_metrics_valid: bool,

    search: SearchState,
    regex_searches: RegexSearches,
    hovered_link: Option<HyperlinkMatch>,

    cursor_visible: bool,
    was_focused: bool,
    title: Option<String>,
    exit_status: Option<i32>,
    exited: bool,
}

impl TerminalView {
    pub fn new<W, R>(
        stdin_writer: W,
        stdout_reader: R,
        config: TerminalConfig,
        cx: &mut Context<Self>,
    ) -> Self
    where
        W: Write + Send + 'static,
        R: Read + Send + 'static,
    {
        let (event_tx, event_rx) = mpsc::channel();
        let exit_event_tx = event_tx.clone();

        let event_proxy = GpuiEventProxy::new(event_tx);
        let notifier_slot = event_proxy.notifier_slot();

        let alac_config = TerminalState::build_config(
            config.scrollback,
            &config.semantic_escape_chars,
            config.allow_osc52_paste,
        );
        let state =
            TerminalState::with_config(config.cols, config.rows, alac_config, event_proxy);

        let renderer = TerminalRenderer::new(
            config.font_family.clone(),
            config.font_size,
            config.line_height_multiplier,
            config.colors.clone(),
        );

        let focus_handle = cx.focus_handle();

        let stdin_writer: PtyWriter = Arc::new(parking_lot::Mutex::new(
            Box::new(stdin_writer) as Box<dyn Write + Send>
        ));

        let window_size = Arc::new(parking_lot::Mutex::new(PtyWindowSize {
            num_lines: config.rows as u16,
            num_cols: config.cols as u16,
            cell_width: f32::from(renderer.cell_width).max(1.0) as u16,
            cell_height: f32::from(renderer.cell_height).max(1.0) as u16,
        }));
        let palette_handle = Arc::new(parking_lot::Mutex::new(config.colors.clone()));

        // Hand the PTY writer to the event proxy so terminal *queries* can be
        // answered inline instead of being dropped on the floor.
        *notifier_slot.lock() = Some(TerminalNotifier::new(
            Arc::clone(&stdin_writer),
            Arc::clone(&window_size),
            Arc::clone(&palette_handle),
        ));

        let (bytes_tx, bytes_rx) = flume::unbounded::<Vec<u8>>();

        thread::spawn(move || {
            Self::read_stdout_blocking(stdout_reader, bytes_tx);
        });

        let reader_task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                match bytes_rx.recv_async().await {
                    Ok(mut bytes) => {
                        // Coalesce everything that already arrived so a firehose
                        // of output costs one parse + one repaint, not hundreds.
                        while let Ok(more) = bytes_rx.try_recv() {
                            bytes.extend(more);
                        }

                        let result = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                            view.state.process_bytes(&bytes);
                            if view.search.is_active() {
                                view.search.refresh(&view.state);
                            }
                            cx.notify();
                        });
                        if result.is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        let _ = exit_event_tx.send(TerminalEvent::Exit);
                        let _ = this.update(cx, |_view, cx: &mut Context<Self>| {
                            cx.notify();
                        });
                        break;
                    }
                }
            }
        });

        Self {
            state,
            renderer,
            focus_handle,
            stdin_writer,
            event_rx,
            config,
            _reader_task: reader_task,
            blink_task: None,
            drag_scroll_task: None,
            window_size,
            palette_handle,
            measured_grid: Arc::new(parking_lot::Mutex::new((0, 0))),
            last_bounds: Arc::new(parking_lot::Mutex::new(Bounds::default())),
            resize_callback: None,
            key_handler: None,
            bell_callback: None,
            title_callback: None,
            clipboard_store_callback: None,
            exit_callback: None,
            link_callback: None,
            scroll_accumulator: 0.0,
            scrollbar_dragging: false,
            scrollbar_hovered: false,
            mouse_down_button: None,
            last_reported_cell: None,
            last_mouse_position: Point::default(),
            cell_metrics_valid: false,
            search: SearchState::new(),
            regex_searches: RegexSearches::new(),
            hovered_link: None,
            cursor_visible: true,
            was_focused: false,
            title: None,
            exit_status: None,
            exited: false,
        }
    }

    // -----------------------------------------------------------------------
    // Builder-style callbacks
    // -----------------------------------------------------------------------

    pub fn with_resize_callback(
        mut self,
        callback: impl Fn(usize, usize) + Send + Sync + 'static,
    ) -> Self {
        self.resize_callback = Some(Arc::new(Box::new(callback)));
        self
    }

    /// Intercept key events before the terminal sees them. Return `true` to
    /// consume the event.
    pub fn with_key_handler(
        mut self,
        handler: impl Fn(&KeyDownEvent) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.key_handler = Some(Arc::new(Box::new(handler)));
        self
    }

    pub fn with_bell_callback(
        mut self,
        callback: impl Fn(&mut Window, &mut Context<TerminalView>) + 'static,
    ) -> Self {
        self.bell_callback = Some(Box::new(callback));
        self
    }

    pub fn with_title_callback(
        mut self,
        callback: impl Fn(&mut Window, &mut Context<TerminalView>, &str) + 'static,
    ) -> Self {
        self.title_callback = Some(Box::new(callback));
        self
    }

    pub fn with_clipboard_store_callback(
        mut self,
        callback: impl Fn(&mut Window, &mut Context<TerminalView>, &str) + 'static,
    ) -> Self {
        self.clipboard_store_callback = Some(Box::new(callback));
        self
    }

    pub fn with_exit_callback(
        mut self,
        callback: impl Fn(&mut Window, &mut Context<TerminalView>) + 'static,
    ) -> Self {
        self.exit_callback = Some(Box::new(callback));
        self
    }

    /// Called when a hyperlink is activated (modifier-click). Return `true` to
    /// suppress the default `open_url` behaviour — use this to open
    /// `path:line:column` targets in the editor instead of a browser.
    pub fn with_link_callback(
        mut self,
        callback: impl Fn(&mut Window, &mut Context<TerminalView>, &HyperlinkMatch) -> bool + 'static,
    ) -> Self {
        self.link_callback = Some(Box::new(callback));
        self
    }

    // -----------------------------------------------------------------------
    // PTY plumbing
    // -----------------------------------------------------------------------

    fn read_stdout_blocking<R: Read + Send + 'static>(
        mut stdout_reader: R,
        bytes_tx: flume::Sender<Vec<u8>>,
    ) {
        let mut buffer = [0u8; 32768];

        loop {
            match stdout_reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    if bytes_tx.send(buffer[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }

    /// Write bytes to the PTY.
    ///
    /// Never call this while holding the terminal lock.
    fn write_to_pty(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut writer = self.stdin_writer.lock();
        let _ = writer.write_all(bytes);
        let _ = writer.flush();
    }

    /// Send text to the shell as if the user had typed it.
    pub fn send_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.scroll_to_bottom_on_input(cx);
        self.write_to_pty(text.as_bytes());
    }

    pub fn send_bytes(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        self.scroll_to_bottom_on_input(cx);
        self.write_to_pty(bytes);
    }

    // -----------------------------------------------------------------------
    // Geometry helpers
    // -----------------------------------------------------------------------

    fn content_origin(&self) -> Point<Pixels> {
        let bounds = *self.last_bounds.lock();
        Point {
            x: bounds.origin.x + self.config.padding.left,
            y: bounds.origin.y + self.config.padding.top,
        }
    }

    /// Convert a mouse position into a grid point plus the side of the cell the
    /// pointer is on, which is what alacritty's selection needs to decide
    /// whether a cell is included.
    fn grid_point_and_side(&self, position: Point<Pixels>) -> (GridPoint, GridSide) {
        let origin = self.content_origin();
        let cell_w = f32::from(self.renderer.cell_width).max(1.0);
        let cell_h = f32::from(self.renderer.cell_height).max(1.0);

        let rel_x = f32::from(position.x - origin.x);
        let rel_y = f32::from(position.y - origin.y);

        let (cols, rows) = self.dimensions();

        let col_f = (rel_x / cell_w).floor();
        let col = (col_f.max(0.0) as usize).min(cols.saturating_sub(1));
        let row = ((rel_y / cell_h).floor().max(0.0) as usize).min(rows.saturating_sub(1));

        let side = if rel_x - col_f * cell_w > cell_w / 2.0 {
            GridSide::Right
        } else {
            GridSide::Left
        };

        let display_offset = self.state.display_offset();
        let point = viewport_to_point(display_offset, GridPoint::new(row, GridColumn(col)));
        (point, side)
    }

    /// Number of lines the viewport should auto-scroll because the pointer is
    /// being dragged past the top or bottom edge.
    fn drag_scroll_lines(&self, position: Point<Pixels>) -> i32 {
        let bounds = *self.last_bounds.lock();
        let cell_h = f32::from(self.renderer.cell_height).max(1.0);

        let top = f32::from(bounds.origin.y + self.config.padding.top);
        let bottom = f32::from(bounds.origin.y + bounds.size.height - self.config.padding.bottom);
        let y = f32::from(position.y);

        if y < top {
            (((top - y) / cell_h).ceil() as i32).clamp(1, 20)
        } else if y > bottom {
            -(((y - bottom) / cell_h).ceil() as i32).clamp(1, 20)
        } else {
            0
        }
    }

    fn is_over_scrollbar(&self, position: Point<Pixels>) -> bool {
        if !self.config.show_scrollbar || self.state.history_size() == 0 {
            return false;
        }
        let bounds = *self.last_bounds.lock();
        position.x >= bounds.origin.x + bounds.size.width - SCROLLBAR_HIT_WIDTH
    }

    fn mouse_reporting_active(&self, mode: TermMode) -> bool {
        mode.intersects(
            TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG,
        )
    }

    // -----------------------------------------------------------------------
    // Scrolling
    // -----------------------------------------------------------------------

    pub fn scroll_lines(&mut self, lines: i32, cx: &mut Context<Self>) {
        self.state.scroll_lines(lines);
        cx.notify();
    }

    pub fn scroll_to_bottom(&mut self, cx: &mut Context<Self>) {
        self.state.scroll_to_bottom();
        cx.notify();
    }

    pub fn scroll_to_top(&mut self, cx: &mut Context<Self>) {
        self.state.scroll_to_top();
        cx.notify();
    }

    pub fn page_up(&mut self, cx: &mut Context<Self>) {
        self.state.scroll(GridScroll::PageUp);
        cx.notify();
    }

    pub fn page_down(&mut self, cx: &mut Context<Self>) {
        self.state.scroll(GridScroll::PageDown);
        cx.notify();
    }

    /// Jump back to the newest output, which is what every terminal does the
    /// moment the user types something while scrolled up.
    fn scroll_to_bottom_on_input(&mut self, cx: &mut Context<Self>) {
        if !self.state.is_at_bottom() {
            self.state.scroll_to_bottom();
            cx.notify();
        }
    }

    fn scroll_to_mouse_y(&mut self, mouse_y: Pixels, cx: &mut Context<Self>) {
        let bounds = *self.last_bounds.lock();
        if bounds.size.height <= px(0.0) {
            return;
        }

        let y_rel = (mouse_y - bounds.origin.y).clamp(px(0.0), bounds.size.height);
        let fraction = f32::from(y_rel) / f32::from(bounds.size.height).max(1.0);

        let history_size = self.state.history_size();
        if history_size == 0 {
            return;
        }

        let target_offset = ((1.0 - fraction) * history_size as f32).round() as i32;
        let delta = target_offset - self.state.display_offset() as i32;
        if delta != 0 {
            self.state.scroll_lines(delta);
            cx.notify();
        }
    }

    // -----------------------------------------------------------------------
    // Selection & clipboard
    // -----------------------------------------------------------------------

    pub fn selected_text(&self) -> Option<String> {
        self.state.selection_text()
    }

    pub fn has_selection(&self) -> bool {
        self.state.has_selection()
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.state.select_all();
        cx.notify();
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if self.state.has_selection() {
            self.state.clear_selection();
            cx.notify();
        }
    }

    pub fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        match self.state.selection_text() {
            Some(text) if !text.is_empty() => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                cx.notify();
                true
            }
            _ => false,
        }
    }

    /// Wrap `text` in bracketed-paste markers when the application asked for
    /// them, and normalise newlines so pasting a multi-line snippet does not
    /// execute every line.
    fn paste_payload(&self, text: &str) -> Vec<u8> {
        if self.state.mode().contains(TermMode::BRACKETED_PASTE) {
            format!("\x1b[200~{text}\x1b[201~").into_bytes()
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
        }
    }

    pub fn paste_from_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        self.scroll_to_bottom_on_input(cx);
        let payload = self.paste_payload(&text);
        self.write_to_pty(&payload);
    }

    pub fn paste_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.scroll_to_bottom_on_input(cx);
        let payload = self.paste_payload(text);
        self.write_to_pty(&payload);
    }

    // -----------------------------------------------------------------------
    // Search
    // -----------------------------------------------------------------------

    /// Set the search query. Pass an empty query to clear.
    pub fn search(&mut self, query: &str, use_regex: bool, cx: &mut Context<Self>) -> bool {
        let ok = self.search.set_query(query, use_regex, &self.state);
        if ok && !query.is_empty() {
            // Focus the first match and scroll it into view.
            self.search.advance(GridDirection::Right, &self.state);
        }
        cx.notify();
        ok
    }

    pub fn next_match(&mut self, cx: &mut Context<Self>) {
        self.search.advance(GridDirection::Right, &self.state);
        cx.notify();
    }

    pub fn previous_match(&mut self, cx: &mut Context<Self>) {
        self.search.advance(GridDirection::Left, &self.state);
        cx.notify();
    }

    pub fn clear_search(&mut self, cx: &mut Context<Self>) {
        self.search.clear();
        cx.notify();
    }

    /// `(current, total)` match counters for a search UI.
    pub fn search_status(&self) -> (Option<usize>, usize) {
        (self.search.current_index(), self.search.match_count())
    }

    // -----------------------------------------------------------------------
    // Vi mode
    // -----------------------------------------------------------------------

    pub fn is_vi_mode(&self) -> bool {
        self.state.is_vi_mode()
    }

    pub fn toggle_vi_mode(&mut self, cx: &mut Context<Self>) {
        self.state.toggle_vi_mode();
        cx.notify();
    }

    /// Handle a keystroke while vi mode is active. Returns `true` when it was
    /// consumed.
    fn handle_vi_keystroke(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let key = keystroke.key.as_str();
        let shift = keystroke.modifiers.shift;
        let ctrl = keystroke.modifiers.control;

        let motion = match (key, shift) {
            // Screen positioning (`H`, `M`, `L`) before the plain movements so
            // the shifted variants win.
            ("h", true) => Some(ViMotion::High),
            ("m", true) => Some(ViMotion::Middle),
            ("l", true) => Some(ViMotion::Low),
            ("h", false) | ("left", _) => Some(ViMotion::Left),
            ("j", false) | ("down", _) => Some(ViMotion::Down),
            ("k", false) | ("up", _) => Some(ViMotion::Up),
            ("l", false) | ("right", _) => Some(ViMotion::Right),
            ("w", false) => Some(ViMotion::SemanticRight),
            ("w", true) => Some(ViMotion::WordRight),
            ("b", false) => Some(ViMotion::SemanticLeft),
            ("b", true) => Some(ViMotion::WordLeft),
            ("e", false) => Some(ViMotion::SemanticRightEnd),
            ("e", true) => Some(ViMotion::WordRightEnd),
            ("0", _) => Some(ViMotion::First),
            ("$", _) => Some(ViMotion::Last),
            ("^", _) => Some(ViMotion::FirstOccupied),
            ("%", _) => Some(ViMotion::Bracket),
            ("{", _) => Some(ViMotion::ParagraphUp),
            ("}", _) => Some(ViMotion::ParagraphDown),
            _ => None,
        };

        if let Some(motion) = motion {
            self.state.vi_motion(motion);
            cx.notify();
            return true;
        }

        match key {
            "escape" | "q" => {
                self.state.clear_selection();
                self.state.toggle_vi_mode();
                cx.notify();
                true
            }
            "g" => {
                if shift {
                    self.state.scroll_to_bottom();
                } else {
                    self.state.scroll_to_top();
                }
                cx.notify();
                true
            }
            "v" => {
                let point = self.state.vi_cursor_point();
                let ty = if ctrl {
                    GridSelectionType::Block
                } else if shift {
                    GridSelectionType::Lines
                } else {
                    GridSelectionType::Simple
                };
                self.state.start_selection(ty, point, GridSide::Left);
                cx.notify();
                true
            }
            "y" => {
                self.copy_selection(cx);
                self.state.clear_selection();
                cx.notify();
                true
            }
            "n" => {
                if shift {
                    self.previous_match(cx);
                } else {
                    self.next_match(cx);
                }
                true
            }
            "pageup" => {
                self.page_up(cx);
                true
            }
            "pagedown" => {
                self.page_down(cx);
                true
            }
            _ => false,
        }
    }

    // -----------------------------------------------------------------------
    // Hyperlinks
    // -----------------------------------------------------------------------

    fn link_modifier_held(modifiers: &Modifiers) -> bool {
        // Cmd on macOS, Ctrl elsewhere — matching Zed.
        if cfg!(target_os = "macos") {
            modifiers.platform
        } else {
            modifiers.control
        }
    }

    fn update_hovered_link(
        &mut self,
        position: Point<Pixels>,
        modifiers: &Modifiers,
        cx: &mut Context<Self>,
    ) {
        if !Self::link_modifier_held(modifiers) {
            if self.hovered_link.take().is_some() {
                cx.notify();
            }
            return;
        }

        let (point, _) = self.grid_point_and_side(position);
        if let Some(existing) = self.hovered_link.as_ref() {
            if existing.contains(point) {
                return;
            }
        }

        let detect_paths = self.config.detect_path_links;
        let searches = &mut self.regex_searches;
        let found = self
            .state
            .with_term(|term| crate::hyperlink::find_at(term, point, searches, detect_paths));

        if found != self.hovered_link {
            self.hovered_link = found;
            cx.notify();
        }
    }

    fn activate_link(
        &mut self,
        link: HyperlinkMatch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(callback) = self.link_callback.take() {
            let handled = callback(window, cx, &link);
            self.link_callback = Some(callback);
            if handled {
                return;
            }
        }

        if link.is_url() {
            cx.open_url(&link.text);
        }
    }

    // -----------------------------------------------------------------------
    // Keyboard
    // -----------------------------------------------------------------------

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let key = keystroke.key.to_ascii_lowercase();
        let modifiers = keystroke.modifiers;

        // The cursor should be solid while the user is actively typing.
        self.cursor_visible = true;

        // --- clipboard shortcuts -------------------------------------------
        if modifiers.control && modifiers.shift && !modifiers.alt {
            match key.as_str() {
                "c" => {
                    self.copy_selection(cx);
                    return;
                }
                "v" => {
                    self.paste_from_clipboard(cx);
                    return;
                }
                "a" => {
                    self.select_all(cx);
                    return;
                }
                "f" => {
                    // Leave search UI to the embedder but make the intent
                    // observable by clearing stale highlights.
                    self.clear_search(cx);
                    return;
                }
                "space" => {
                    self.toggle_vi_mode(cx);
                    return;
                }
                _ => {}
            }
        }

        if modifiers.control && !modifiers.shift && !modifiers.alt && key == "c" {
            // Ctrl-C copies when there is a selection, otherwise it is SIGINT.
            if self.state.has_selection() {
                self.copy_selection(cx);
                self.state.clear_selection();
                return;
            }
        }

        if modifiers.shift && !modifiers.control && !modifiers.alt && key == "insert" {
            self.paste_from_clipboard(cx);
            return;
        }
        if modifiers.control && !modifiers.shift && !modifiers.alt && key == "insert" {
            self.copy_selection(cx);
            return;
        }

        // --- scrollback navigation -----------------------------------------
        if modifiers.shift && !modifiers.control && !modifiers.alt {
            let scroll = match key.as_str() {
                "pageup" => Some(GridScroll::PageUp),
                "pagedown" => Some(GridScroll::PageDown),
                "home" => Some(GridScroll::Top),
                "end" => Some(GridScroll::Bottom),
                _ => None,
            };

            if let Some(scroll) = scroll {
                self.state.scroll(scroll);
                cx.notify();
                return;
            }
        }

        // --- vi mode --------------------------------------------------------
        if self.state.is_vi_mode() && self.handle_vi_keystroke(keystroke, cx) {
            return;
        }

        // --- embedder hook ---------------------------------------------------
        if let Some(handler) = self.key_handler.clone() {
            if handler(event) {
                self.scroll_to_bottom_on_input(cx);
                return;
            }
        }

        // Typing anywhere dismisses the selection, like every other terminal.
        if self.state.has_selection() {
            self.state.clear_selection();
            cx.notify();
        }

        if let Some(bytes) = keystroke_to_bytes(keystroke, self.state.mode()) {
            self.scroll_to_bottom_on_input(cx);
            self.write_to_pty(&bytes);
            self.restart_blink(cx);
        }
    }

    // -----------------------------------------------------------------------
    // Mouse
    // -----------------------------------------------------------------------

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        self.last_mouse_position = event.position;

        if self.is_over_scrollbar(event.position) {
            self.scrollbar_dragging = true;
            self.scroll_to_mouse_y(event.position.y, cx);
            return;
        }

        self.mouse_down_button = Some(event.button);

        let (point, side) = self.grid_point_and_side(event.position);
        let mode = self.state.mode();

        // Modifier-click opens the hyperlink under the pointer.
        if event.button == MouseButton::Left && Self::link_modifier_held(&event.modifiers) {
            self.update_hovered_link(event.position, &event.modifiers, cx);
            if let Some(link) = self.hovered_link.clone() {
                self.activate_link(link, window, cx);
                return;
            }
        }

        // Shift bypasses application mouse reporting so the user can always
        // select text, even inside a full screen TUI.
        if self.mouse_reporting_active(mode) && !event.modifiers.shift {
            let modifiers = crate::mouse::encode_modifiers(
                event.modifiers.shift,
                event.modifiers.alt,
                event.modifiers.control,
            );
            if let Some(report) = self.viewport_point(event.position).and_then(|vp| {
                crate::mouse::mouse_button_report(event.button, true, vp, modifiers, mode)
            }) {
                self.write_to_pty(&report);
                self.last_reported_cell = self.viewport_point(event.position);
            }
            self.clear_selection(cx);
            return;
        }

        match event.button {
            MouseButton::Left => {
                let ty = match event.click_count {
                    0 | 1 => {
                        if event.modifiers.alt {
                            GridSelectionType::Block
                        } else {
                            GridSelectionType::Simple
                        }
                    }
                    2 => GridSelectionType::Semantic,
                    _ => GridSelectionType::Lines,
                };
                self.state.start_selection(ty, point, side);
                cx.notify();
            }
            MouseButton::Middle => {
                // Middle click pastes, matching X11 convention.
                self.paste_from_clipboard(cx);
            }
            MouseButton::Right => {
                if self.state.has_selection() {
                    self.copy_selection(cx);
                    self.state.clear_selection();
                    cx.notify();
                } else {
                    self.paste_from_clipboard(cx);
                }
            }
            _ => {}
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.drag_scroll_task = None;

        if self.scrollbar_dragging {
            self.scrollbar_dragging = false;
            cx.notify();
            return;
        }

        let mode = self.state.mode();
        if self.mouse_reporting_active(mode) && !event.modifiers.shift {
            let modifiers = crate::mouse::encode_modifiers(
                event.modifiers.shift,
                event.modifiers.alt,
                event.modifiers.control,
            );
            if let Some(report) = self.viewport_point(event.position).and_then(|vp| {
                crate::mouse::mouse_button_report(event.button, false, vp, modifiers, mode)
            }) {
                self.write_to_pty(&report);
            }
        } else if self.config.copy_on_select
            && event.button == MouseButton::Left
            && self.state.has_selection()
        {
            self.copy_selection(cx);
        }

        self.mouse_down_button = None;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.last_mouse_position = event.position;

        let hovered_scrollbar = self.is_over_scrollbar(event.position);
        if hovered_scrollbar != self.scrollbar_hovered {
            self.scrollbar_hovered = hovered_scrollbar;
            cx.notify();
        }

        if self.scrollbar_dragging {
            self.scroll_to_mouse_y(event.position.y, cx);
            return;
        }

        self.update_hovered_link(event.position, &event.modifiers, cx);

        let mode = self.state.mode();
        let motion_reporting = mode.intersects(TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG);

        if motion_reporting && !event.modifiers.shift {
            let viewport_point = self.viewport_point(event.position);
            if viewport_point != self.last_reported_cell {
                let modifiers = crate::mouse::encode_modifiers(
                    event.modifiers.shift,
                    event.modifiers.alt,
                    event.modifiers.control,
                );
                if let Some(report) = viewport_point.and_then(|vp| {
                    crate::mouse::mouse_motion_report(vp, self.mouse_down_button, modifiers, mode)
                }) {
                    self.write_to_pty(&report);
                    self.last_reported_cell = viewport_point;
                }
            }
            return;
        }

        if self.mouse_down_button == Some(MouseButton::Left) {
            self.extend_selection_to(event.position, cx);

            // Dragging above/below the viewport keeps scrolling, which is what
            // makes selecting more than one screen of output possible.
            let lines = self.drag_scroll_lines(event.position);
            if lines != 0 {
                self.start_drag_scroll(lines, cx);
            } else {
                self.drag_scroll_task = None;
            }
        }
    }

    fn extend_selection_to(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let (point, side) = self.grid_point_and_side(position);
        self.state.update_selection(point, side);
        cx.notify();
    }

    fn start_drag_scroll(&mut self, lines: i32, cx: &mut Context<Self>) {
        self.drag_scroll_task = Some(cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(DRAG_SCROLL_INTERVAL).await;

                let keep_going = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                    if view.mouse_down_button != Some(MouseButton::Left) {
                        return false;
                    }
                    view.state.scroll_lines(lines);
                    let position = view.last_mouse_position;
                    view.extend_selection_to(position, cx);
                    true
                });

                match keep_going {
                    Ok(true) => {}
                    _ => break,
                }
            }
        }));
    }

    /// Mouse position as a *viewport* relative point, which is the coordinate
    /// space mouse reports use. `None` when outside the grid.
    fn viewport_point(&self, position: Point<Pixels>) -> Option<GridPoint> {
        let origin = self.content_origin();
        let cell_w = f32::from(self.renderer.cell_width).max(1.0);
        let cell_h = f32::from(self.renderer.cell_height).max(1.0);

        let rel_x = f32::from(position.x - origin.x);
        let rel_y = f32::from(position.y - origin.y);
        if rel_x < 0.0 || rel_y < 0.0 {
            return None;
        }

        let (cols, rows) = self.dimensions();
        let col = ((rel_x / cell_w) as usize).min(cols.saturating_sub(1));
        let row = ((rel_y / cell_h) as usize).min(rows.saturating_sub(1));
        Some(GridPoint::new(GridLine(row as i32), GridColumn(col)))
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let cell_height = self.renderer.cell_height;
        let pixel_y = f32::from(event.delta.pixel_delta(cell_height).y)
            * self.config.scroll_sensitivity.max(0.05);

        if pixel_y == 0.0 {
            return;
        }

        // Reset the accumulator when the direction flips so a quick reversal
        // feels immediate rather than eating the first notch.
        if (self.scroll_accumulator > 0.0 && pixel_y < 0.0)
            || (self.scroll_accumulator < 0.0 && pixel_y > 0.0)
        {
            self.scroll_accumulator = 0.0;
        }
        self.scroll_accumulator += pixel_y;

        let cell_height_f32 = f32::from(cell_height).max(1.0);
        let mut lines = (self.scroll_accumulator / cell_height_f32) as i32;
        if lines == 0 {
            if self.scroll_accumulator >= cell_height_f32 * 0.5 {
                lines = 1;
            } else if self.scroll_accumulator <= -cell_height_f32 * 0.5 {
                lines = -1;
            } else {
                return;
            }
        }

        self.scroll_accumulator -= (lines as f32) * cell_height_f32;
        let max_lines = self.config.max_scroll_lines.max(1);
        lines = lines.clamp(-max_lines, max_lines);

        let mode = self.state.mode();
        let modifiers = crate::mouse::encode_modifiers(
            event.modifiers.shift,
            event.modifiers.alt,
            event.modifiers.control,
        );
        let cell_point = self
            .viewport_point(event.position)
            .unwrap_or_else(|| GridPoint::new(GridLine(0), GridColumn(0)));

        let in_alt_screen = mode.contains(TermMode::ALT_SCREEN);
        let reporting = self.mouse_reporting_active(mode);

        // Browsing history wins whenever there *is* history to browse and the
        // application is not tracking the mouse. In the alternate screen there
        // is no scrollback, so the event always goes to the application.
        if !in_alt_screen && !reporting {
            self.state.scroll_lines(lines);
            cx.notify();
            return;
        }

        if reporting {
            if let Some(report) = crate::mouse::scroll_report(lines, cell_point, modifiers, mode) {
                self.write_to_pty(&report);
                return;
            }
        }

        if in_alt_screen && self.config.alternate_scroll && mode.contains(TermMode::ALTERNATE_SCROLL)
        {
            if let Some(report) = crate::mouse::scroll_report(lines, cell_point, modifiers, mode) {
                self.write_to_pty(&report);
                return;
            }
        }

        self.state.scroll_lines(lines);
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // Cursor blink & focus
    // -----------------------------------------------------------------------

    fn restart_blink(&mut self, cx: &mut Context<Self>) {
        self.cursor_visible = true;

        // Only animate when the terminal is focused *and* the application
        // actually asked for a blinking cursor — otherwise we would repaint
        // twice a second for nothing.
        if !self.config.cursor_blink || !self.was_focused || !self.state.cursor_blinking() {
            self.blink_task = None;
            return;
        }

        let interval = self.config.blink_interval;
        self.blink_task = Some(cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(interval).await;
                let alive = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                    view.cursor_visible = !view.cursor_visible;
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
    }

    /// Emit `CSI I` / `CSI O` when the application enabled focus reporting
    /// (DEC mode 1004). Editors like vim use it to refresh buffers.
    fn report_focus_change(&self, focused: bool) {
        if !self.state.mode().contains(TermMode::FOCUS_IN_OUT) {
            return;
        }
        self.write_to_pty(if focused { b"\x1b[I" } else { b"\x1b[O" });
    }

    // -----------------------------------------------------------------------
    // Events
    // -----------------------------------------------------------------------

    fn process_events(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Drain into a buffer first so the borrow on `self.event_rx` ends before
        // we start calling `&mut self` methods.
        let mut events = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            events.push(event);
        }

        for event in events {
            match event {
                TerminalEvent::Wakeup | TerminalEvent::MouseCursorDirty => {}

                TerminalEvent::Bell => {
                    if let Some(callback) = self.bell_callback.take() {
                        callback(window, cx);
                        self.bell_callback = Some(callback);
                    }
                }

                TerminalEvent::Title(title) => {
                    self.title = Some(title.clone());
                    if let Some(callback) = self.title_callback.take() {
                        callback(window, cx, &title);
                        self.title_callback = Some(callback);
                    }
                }

                TerminalEvent::ResetTitle => {
                    self.title = None;
                    if let Some(callback) = self.title_callback.take() {
                        callback(window, cx, "");
                        self.title_callback = Some(callback);
                    }
                }

                TerminalEvent::ClipboardStore(_, text) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                    if let Some(callback) = self.clipboard_store_callback.take() {
                        callback(window, cx, &text);
                        self.clipboard_store_callback = Some(callback);
                    }
                }

                TerminalEvent::ClipboardLoad(_, formatter) => {
                    if self.config.allow_osc52_paste {
                        let text = cx
                            .read_from_clipboard()
                            .and_then(|item| item.text())
                            .unwrap_or_default();
                        let payload = formatter(&text);
                        self.write_to_pty(payload.as_bytes());
                    }
                }

                TerminalEvent::CursorBlinkingChange => {
                    self.restart_blink(cx);
                }

                TerminalEvent::ChildExit(code) => {
                    self.exit_status = Some(code);
                }

                TerminalEvent::Exit => {
                    if self.exited {
                        continue;
                    }
                    self.exited = true;
                    self.blink_task = None;
                    self.drag_scroll_task = None;
                    if let Some(callback) = self.exit_callback.take() {
                        callback(window, cx);
                        self.exit_callback = Some(callback);
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Accessors / configuration
    // -----------------------------------------------------------------------

    pub fn dimensions(&self) -> (usize, usize) {
        (self.state.cols(), self.state.rows())
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.state.resize(cols, rows);
        self.sync_window_size();
    }

    pub fn config(&self) -> &TerminalConfig {
        &self.config
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// Title reported by the application through `OSC 0 / 2`.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// `true` once the child process is gone.
    pub fn has_exited(&self) -> bool {
        self.exited
    }

    pub fn exit_status(&self) -> Option<i32> {
        self.exit_status
    }

    /// Clear the screen *and* the scrollback.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.state.clear_screen_and_scrollback();
        self.search.clear();
        cx.notify();
    }

    pub fn update_config(&mut self, config: TerminalConfig, cx: &mut Context<Self>) {
        let metrics_changed = self.renderer.font_family != config.font_family
            || self.renderer.font_size != config.font_size
            || self.renderer.line_height_multiplier != config.line_height_multiplier;

        let alac_changed = self.config.scrollback != config.scrollback
            || self.config.semantic_escape_chars != config.semantic_escape_chars
            || self.config.allow_osc52_paste != config.allow_osc52_paste;

        self.renderer.font_family = config.font_family.clone();
        self.renderer.font_size = config.font_size;
        self.renderer.line_height_multiplier = config.line_height_multiplier;
        self.renderer.palette = config.colors.clone();
        *self.palette_handle.lock() = config.colors.clone();

        if alac_changed {
            self.state.set_config(TerminalState::build_config(
                config.scrollback,
                &config.semantic_escape_chars,
                config.allow_osc52_paste,
            ));
        }

        self.config = config;

        if metrics_changed {
            self.cell_metrics_valid = false;
        }

        self.restart_blink(cx);
        cx.notify();
    }

    fn sync_window_size(&self) {
        let (cols, rows) = self.dimensions();
        *self.window_size.lock() = PtyWindowSize {
            num_lines: rows as u16,
            num_cols: cols as u16,
            cell_width: f32::from(self.renderer.cell_width).max(1.0) as u16,
            cell_height: f32::from(self.renderer.cell_height).max(1.0) as u16,
        };
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.process_events(window, cx);

        if !self.cell_metrics_valid {
            self.renderer.measure_cell(window);
            self.cell_metrics_valid = true;
        }

        // Adopt the grid size the previous paint pass computed.
        let (measured_cols, measured_rows) = *self.measured_grid.lock();
        if measured_cols > 0 && (measured_cols, measured_rows) != self.dimensions() {
            self.state.sync_dimensions(measured_cols, measured_rows);
            self.sync_window_size();
            if self.search.is_active() {
                self.search.refresh(&self.state);
            }
            // The paint pass resized the grid behind our back; schedule one more
            // frame so hit-testing and geometry agree with what is on screen.
            // This converges immediately: next frame the sizes already match.
            cx.notify();
        }

        let is_focused = self.focus_handle.is_focused(window);
        if is_focused != self.was_focused {
            self.was_focused = is_focused;
            self.state.set_focused(is_focused);
            self.report_focus_change(is_focused);
            self.restart_blink(cx);
        }

        let state_arc = self.state.term_arc();
        let renderer = self.renderer.clone();
        let resize_callback = self.resize_callback.clone();
        let padding = self.config.padding;
        let last_bounds = self.last_bounds.clone();
        let measured_grid = self.measured_grid.clone();
        let window_size = self.window_size.clone();

        let selection: Option<SelectionRange> = self.state.selection_range();
        let search_matches: Vec<GridMatch> = self.search.matches().to_vec();
        let active_match: Option<GridMatch> = self.search.current_match().cloned();
        let hovered_link: Option<RangeInclusive<GridPoint>> =
            self.hovered_link.as_ref().map(|link| link.range.clone());
        let cursor_visible = self.cursor_visible;
        let show_scrollbar = self.config.show_scrollbar;
        let scrollbar_active = self.scrollbar_dragging || self.scrollbar_hovered;
        let background = self.config.colors.background();

        let selection_color = self.config.selection_color;
        let match_color = self.config.search_match_color;
        let active_match_color = self.config.active_search_match_color;

        let cursor_style = if self.hovered_link.is_some() {
            CursorStyle::PointingHand
        } else {
            CursorStyle::IBeam
        };

        div()
            .size_full()
            .bg(background)
            .cursor(cursor_style)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(
                canvas(
                    move |bounds, _window, _cx| bounds,
                    move |bounds, _, window, cx| {
                        *last_bounds.lock() = bounds;

                        let available_width =
                            f32::from(bounds.size.width - padding.left - padding.right);
                        let available_height =
                            f32::from(bounds.size.height - padding.top - padding.bottom);
                        let cell_width_f32 = f32::from(renderer.cell_width).max(1.0);
                        let cell_height_f32 = f32::from(renderer.cell_height).max(1.0);

                        let cols = ((available_width / cell_width_f32) as usize).max(2);
                        let rows = ((available_height / cell_height_f32) as usize).max(1);

                        let mut term = state_arc.lock();
                        let current_cols = term.columns();
                        let current_rows = term.screen_lines();
                        if cols != current_cols || rows != current_rows {
                            term.resize(crate::terminal::TermDimensions::new(cols, rows));
                            *measured_grid.lock() = (cols, rows);
                            *window_size.lock() = PtyWindowSize {
                                num_lines: rows as u16,
                                num_cols: cols as u16,
                                cell_width: cell_width_f32 as u16,
                                cell_height: cell_height_f32 as u16,
                            };
                            if let Some(ref callback) = resize_callback {
                                callback(cols, rows);
                            }
                        } else {
                            let mut measured = measured_grid.lock();
                            if *measured != (cols, rows) {
                                *measured = (cols, rows);
                            }
                        }

                        let paint_ctx = PaintContext {
                            selection,
                            search_matches: &search_matches,
                            active_match: active_match.as_ref(),
                            hovered_link: hovered_link.as_ref(),
                            is_focused,
                            cursor_visible,
                            show_scrollbar,
                            scrollbar_active,
                            selection_color,
                            match_color,
                            active_match_color,
                        };

                        renderer.paint(bounds, padding, &term, &paint_ctx, window, cx);
                    },
                )
                .size_full(),
            )
    }
}
