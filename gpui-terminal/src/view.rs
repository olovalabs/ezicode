use crate::colors::ColorPalette;
use crate::event::{GpuiEventProxy, TerminalEvent};
use crate::input::keystroke_to_bytes;
use crate::render::TerminalRenderer;
use crate::terminal::TerminalState;
use alacritty_terminal::grid::Dimensions;
use gpui::{Edges, *};
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

#[derive(Clone, Debug)]
pub struct TerminalConfig {

    pub cols: usize,

    pub rows: usize,

    pub font_family: String,

    pub font_size: Pixels,

    pub scrollback: usize,

    pub line_height_multiplier: f32,

    pub padding: Edges<Pixels>,

    pub colors: ColorPalette,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            font_family: "monospace".into(),
            font_size: px(14.0),
            scrollback: 10000,
            line_height_multiplier: 1.2,
            padding: Edges::all(px(0.0)),
            colors: ColorPalette::default(),
        }
    }
}

pub type ResizeCallback = Box<dyn Fn(usize, usize) + Send + Sync>;

pub type KeyHandler = Box<dyn Fn(&KeyDownEvent) -> bool + Send + Sync>;

pub type BellCallback = Box<dyn Fn(&mut Window, &mut Context<TerminalView>)>;

pub type TitleCallback = Box<dyn Fn(&mut Window, &mut Context<TerminalView>, &str)>;

pub type ClipboardStoreCallback = Box<dyn Fn(&mut Window, &mut Context<TerminalView>, &str)>;

pub type ExitCallback = Box<dyn Fn(&mut Window, &mut Context<TerminalView>)>;

pub struct TerminalView {

    state: TerminalState,

    renderer: TerminalRenderer,

    focus_handle: FocusHandle,

    stdin_writer: Arc<parking_lot::Mutex<Box<dyn Write + Send>>>,

    event_rx: mpsc::Receiver<TerminalEvent>,

    config: TerminalConfig,

    #[allow(dead_code)]
    _reader_task: Task<()>,

    resize_callback: Option<Arc<ResizeCallback>>,

    key_handler: Option<Arc<KeyHandler>>,

    bell_callback: Option<BellCallback>,

    title_callback: Option<TitleCallback>,

    clipboard_store_callback: Option<ClipboardStoreCallback>,

    exit_callback: Option<ExitCallback>,

    scroll_accumulator: f32,

    scrollbar_dragging: bool,

    selection: Option<crate::mouse::Selection>,

    mouse_down_button: Option<MouseButton>,

    last_reported_cell: Option<alacritty_terminal::index::Point>,

    last_bounds: Arc<parking_lot::Mutex<Bounds<Pixels>>>,

    cell_metrics_valid: bool,
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

        let state = TerminalState::new(config.cols, config.rows, event_proxy);

        let renderer = TerminalRenderer::new(
            config.font_family.clone(),
            config.font_size,
            config.line_height_multiplier,
            config.colors.clone(),
        );

        let focus_handle = cx.focus_handle();

        let stdin_writer = Arc::new(parking_lot::Mutex::new(
            Box::new(stdin_writer) as Box<dyn Write + Send>
        ));

        let (bytes_tx, bytes_rx) = flume::unbounded::<Vec<u8>>();

        thread::spawn(move || {
            Self::read_stdout_blocking(stdout_reader, bytes_tx);
        });

        let reader_task = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {

                match bytes_rx.recv_async().await {
                    Ok(mut bytes) => {

                        while let Ok(more) = bytes_rx.try_recv() {
                            bytes.extend(more);
                        }

                        let result = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                            view.state.process_bytes(&bytes);
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
            resize_callback: None,
            key_handler: None,
            bell_callback: None,
            title_callback: None,
            clipboard_store_callback: None,
            exit_callback: None,
            scroll_accumulator: 0.0,
            scrollbar_dragging: false,
            selection: None,
            mouse_down_button: None,
            last_reported_cell: None,
            last_bounds: Arc::new(parking_lot::Mutex::new(Bounds::default())),
            cell_metrics_valid: false,
        }
    }

    pub fn with_resize_callback(
        mut self,
        callback: impl Fn(usize, usize) + Send + Sync + 'static,
    ) -> Self {
        self.resize_callback = Some(Arc::new(Box::new(callback)));
        self
    }

    /// Set a callback to intercept key events before terminal processing.
    ///
    /// The callback receives the key event and should return `true` to consume
    /// the event (prevent the terminal from processing it), or `false` to allow
    /// normal terminal processing.
    ///
    /// # Arguments
    ///
    /// * `handler` - A function that receives key events and returns whether to consume them
    ///
    /// # Example
    ///
    /// ```ignore
    /// terminal.with_key_handler(|event| {
    ///     // Handle Ctrl++ to increase font size
    ///     if event.keystroke.modifiers.control && event.keystroke.key == "+" {
    ///         // Handle the event
    ///         return true; // Consume the event
    ///     }
    ///     false // Let terminal handle it
    /// })
    /// ```
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

    /// Set a callback to be invoked when the terminal title changes.
    ///
    /// The callback receives a mutable reference to the window and context,
    /// along with the new title string.
    ///
    /// # Arguments
    ///
    /// * `callback` - A function that will be called with the new title
    ///
    /// # Example
    ///
    /// ```ignore
    /// terminal.with_title_callback(|window, cx, title| {
    ///     // Update window title or tab title
    /// })
    /// ```
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

    /// Set a callback to be invoked when the terminal process exits.
    ///
    /// The callback receives a mutable reference to the window and context,
    /// allowing you to close the terminal view or show an exit message.
    ///
    /// # Arguments
    ///
    /// * `callback` - A function that will be called when the process exits
    ///
    /// # Example
    ///
    /// ```ignore
    /// terminal.with_exit_callback(|window, cx| {
    ///     // Close the terminal tab or show exit message
    /// })
    /// ```
    pub fn with_exit_callback(
        mut self,
        callback: impl Fn(&mut Window, &mut Context<TerminalView>) + 'static,
    ) -> Self {
        self.exit_callback = Some(Box::new(callback));
        self
    }

    fn read_stdout_blocking<R: Read + Send + 'static>(
        mut stdout_reader: R,
        bytes_tx: flume::Sender<Vec<u8>>,
    ) {
        let mut buffer = [0u8; 32768];

        loop {
            match stdout_reader.read(&mut buffer) {
                Ok(0) => {
                    // EOF - channel will be dropped, signaling completion
                    break;
                }
                Ok(n) => {
                    // Send bytes to the async task
                    let bytes = buffer[..n].to_vec();
                    if bytes_tx.send(bytes).is_err() {
                        break; // Channel closed
                    }
                }
                Err(_) => {
                    // Read error
                    break;
                }
            }
        }
    }

    /// Find the start and end column of the word containing `point`.
    fn find_word_boundaries(
        &self,
        point: alacritty_terminal::index::Point,
    ) -> (alacritty_terminal::index::Point, alacritty_terminal::index::Point) {
        use alacritty_terminal::index::{Column, Line};
        let line = point.line;
        let col = point.column.0;

        self.state.with_term(|term| {
            let grid = term.grid();
            let total_cols = grid.columns();
            let display_offset = grid.display_offset();
            let buffer_line = line.0 - display_offset as i32;

            let is_word_char = |c: char| c.is_alphanumeric() || c == '_' || c == '-' || c == '.';

            let mut start_col = col;
            while start_col > 0 {
                let cell_ch = grid[Line(buffer_line)][Column(start_col - 1)].c;
                if is_word_char(cell_ch) {
                    start_col -= 1;
                } else {
                    break;
                }
            }

            let mut end_col = col;
            while end_col + 1 < total_cols {
                let cell_ch = grid[Line(buffer_line)][Column(end_col + 1)].c;
                if is_word_char(cell_ch) {
                    end_col += 1;
                } else {
                    break;
                }
            }

            (
                alacritty_terminal::index::Point::new(line, Column(start_col)),
                alacritty_terminal::index::Point::new(line, Column(end_col)),
            )
        })
    }

    /// Extract text within current selection range
    pub fn get_selected_text(&self) -> Option<String> {
        let sel = self.selection.as_ref()?;
        let (sel_start, sel_end) = if sel.start <= sel.end {
            (sel.start, sel.end)
        } else {
            (sel.end, sel.start)
        };

        self.state.with_term(|term| {
            let grid = term.grid();
            let num_cols = grid.columns();
            let display_offset = grid.display_offset();
            let mut result = String::new();

            for r in sel_start.line.0..=sel_end.line.0 {
                let buffer_line = r - display_offset as i32;
                let start_col = if r == sel_start.line.0 { sel_start.column.0 } else { 0 };
                let end_col = if r == sel_end.line.0 { (sel_end.column.0 + 1).min(num_cols) } else { num_cols };

                let mut line_str = String::new();
                for c in start_col..end_col {
                    let cell = &grid[alacritty_terminal::index::Line(buffer_line)][alacritty_terminal::index::Column(c)];
                    let ch = if cell.c == '\0' { ' ' } else { cell.c };
                    line_str.push(ch);
                }
                let trimmed = line_str.trim_end();
                result.push_str(trimmed);
                if r < sel_end.line.0 {
                    result.push('\n');
                }
            }

            if result.is_empty() {
                None
            } else {
                Some(result)
            }
        })
    }

    /// Copy current selection to system clipboard
    pub fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(text) = self.get_selected_text() {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
            self.selection = None;
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Paste from system clipboard into terminal stdin
    pub fn paste_from_clipboard(&mut self, cx: &mut Context<Self>) {
        if let Some(item) = cx.read_from_clipboard() {
            if let Some(text) = item.text() {
                let mode = self.state.mode();
                let bytes = if mode.contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE) {
                    format!("\x1b[200~{}\x1b[201~", text).into_bytes()
                } else {
                    text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
                };

                let mut writer = self.stdin_writer.lock();
                let _ = writer.write_all(&bytes);
                let _ = writer.flush();
            }
        }
    }

    /// Handle keyboard input events.
    ///
    /// Converts GPUI keystrokes to terminal escape sequences and writes them
    /// to the stdin writer. If a key handler is set and returns true, the event
    /// is consumed and not sent to the terminal.
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;

        // Clipboard shortcuts:
        // 1. Ctrl+Shift+C: copy selection
        if keystroke.modifiers.control && keystroke.modifiers.shift && !keystroke.modifiers.alt {
            let key = keystroke.key.to_ascii_lowercase();
            if key == "c" {
                self.copy_selection(cx);
                return;
            }
            if key == "v" {
                self.paste_from_clipboard(cx);
                return;
            }
        }

        // 2. Ctrl+C: copy if text is selected, else pass through as SIGINT (0x03)
        if keystroke.modifiers.control && !keystroke.modifiers.shift && !keystroke.modifiers.alt {
            let key = keystroke.key.to_ascii_lowercase();
            if key == "c" && self.selection.is_some() {
                self.copy_selection(cx);
                return;
            }
            if key == "v" {
                self.paste_from_clipboard(cx);
                return;
            }
        }

        // 3. Shift+Insert: paste
        if keystroke.modifiers.shift && !keystroke.modifiers.control && !keystroke.modifiers.alt {
            let key = keystroke.key.to_ascii_lowercase();
            if key == "insert" {
                self.paste_from_clipboard(cx);
                return;
            }
        }

        // Any other key typing clears selection
        if self.selection.is_some() {
            self.selection = None;
            cx.notify();
        }

        // Handle terminal scrollback key combinations (Shift + PageUp/PageDown/Home/End)
        // Handled locally by the terminal emulator before key handler interception.
        if keystroke.modifiers.shift && !keystroke.modifiers.control && !keystroke.modifiers.alt {
            let scroll = match keystroke.key.as_str() {
                "pageup" | "PageUp" => Some(alacritty_terminal::grid::Scroll::PageUp),
                "pagedown" | "PageDown" => Some(alacritty_terminal::grid::Scroll::PageDown),
                "home" | "Home" => Some(alacritty_terminal::grid::Scroll::Top),
                "end" | "End" => Some(alacritty_terminal::grid::Scroll::Bottom),
                _ => None,
            };

            if let Some(scroll) = scroll {
                self.state.with_term_mut(|term| {
                    term.scroll_display(scroll);
                });
                cx.notify();
                return;
            }
        }

        // Check if key handler wants to consume this event
        if let Some(ref handler) = self.key_handler
            && handler(event)
        {
            // If user typed input while scrolled up, automatically scroll to bottom
            let scrolled_up = self.state.with_term(|term| term.grid().display_offset() > 0);
            if scrolled_up {
                self.state.with_term_mut(|term| {
                    term.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
                });
                cx.notify();
            }
            return; // Event consumed by handler
        }

        if let Some(bytes) = keystroke_to_bytes(&event.keystroke, self.state.mode()) {
            let scrolled_up = self.state.with_term(|term| term.grid().display_offset() > 0);
            if scrolled_up {
                self.state.with_term_mut(|term| {
                    term.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
                });
                cx.notify();
            }

            let mut writer = self.stdin_writer.lock();
            let _ = writer.write_all(&bytes);
            let _ = writer.flush();
        }
    }

    /// Scroll display offset based on a vertical mouse position (for scrollbar interaction).
    fn scroll_to_mouse_y(&mut self, mouse_y: Pixels, cx: &mut Context<Self>) {
        let bounds = *self.last_bounds.lock();
        let available_h: f32 = bounds.size.height.into();
        if available_h <= 0.0 {
            return;
        }

        let y_rel = (mouse_y - bounds.origin.y).clamp(px(0.0), bounds.size.height);
        let fraction: f32 = (y_rel / bounds.size.height).into();
        // fraction: 0.0 at top of viewport (max history), 1.0 at bottom (0 display offset)
        self.state.with_term_mut(|term| {
            let history_size = term.grid().history_size();
            if history_size > 0 {
                let target_offset = ((1.0 - fraction) * history_size as f32).round() as usize;
                let current_offset = term.grid().display_offset();
                let delta = target_offset as i32 - current_offset as i32;
                if delta != 0 {
                    term.scroll_display(alacritty_terminal::grid::Scroll::Delta(delta));
                }
            }
        });
        cx.notify();
    }

    /// Handle mouse down events.
    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Request focus when clicking the terminal
        window.focus(&self.focus_handle);
        cx.notify();

        let bounds = *self.last_bounds.lock();
        let history_size = self.state.with_term(|t| t.grid().history_size());
        if history_size > 0 && event.position.x >= bounds.origin.x + bounds.size.width - px(14.0) {
            self.scrollbar_dragging = true;
            self.scroll_to_mouse_y(event.position.y, cx);
            return;
        }

        self.mouse_down_button = Some(event.button);

        let origin = Point {
            x: bounds.origin.x + self.config.padding.left,
            y: bounds.origin.y + self.config.padding.top,
        };
        let (curr_cols, curr_rows) = self.dimensions();
        let max_col = curr_cols.saturating_sub(1);
        let max_row = (curr_rows as i32).saturating_sub(1);
        let mut cell_point = crate::mouse::pixel_to_cell(
            event.position,
            origin,
            self.renderer.cell_width,
            self.renderer.cell_height,
        );
        cell_point.column.0 = cell_point.column.0.min(max_col);
        cell_point.line.0 = cell_point.line.0.min(max_row);

        let mode = self.state.mode();
        let mouse_tracking_active = mode.intersects(
            alacritty_terminal::term::TermMode::MOUSE_REPORT_CLICK
                | alacritty_terminal::term::TermMode::MOUSE_MOTION
                | alacritty_terminal::term::TermMode::MOUSE_DRAG,
        );

        // Shift-click bypass: holding Shift bypasses TUI mouse tracking to allow text selection
        if mouse_tracking_active && !event.modifiers.shift {
            let modifiers = crate::mouse::encode_modifiers(
                event.modifiers.shift,
                event.modifiers.alt,
                event.modifiers.control,
            );
            if let Some(report) = crate::mouse::mouse_button_report(
                event.button,
                true,
                cell_point,
                modifiers,
                mode,
            ) {
                let mut writer = self.stdin_writer.lock();
                let _ = writer.write_all(&report);
                let _ = writer.flush();
                self.last_reported_cell = Some(cell_point);
            }
            if self.selection.is_some() {
                self.selection = None;
                cx.notify();
            }
        } else {
            // Local interaction: selection or right-click quick copy/paste
            match event.button {
                MouseButton::Left => {
                    let sel_type = crate::mouse::selection_type_from_clicks(event.click_count);
                    let (start, end) = match sel_type {
                        crate::mouse::SelectionType::Simple => (cell_point, cell_point),
                        crate::mouse::SelectionType::Word => {
                            self.find_word_boundaries(cell_point)
                        }
                        crate::mouse::SelectionType::Line => {
                            let cols = self.state.with_term(|t| t.grid().columns());
                            (
                                alacritty_terminal::index::Point::new(
                                    cell_point.line,
                                    alacritty_terminal::index::Column(0),
                                ),
                                alacritty_terminal::index::Point::new(
                                    cell_point.line,
                                    alacritty_terminal::index::Column(cols.saturating_sub(1)),
                                ),
                            )
                        }
                    };
                    self.selection = Some(crate::mouse::Selection::new(start, end, sel_type));
                    cx.notify();
                }
                MouseButton::Right => {
                    if self.selection.is_some() {
                        self.copy_selection(cx);
                    } else {
                        self.paste_from_clipboard(cx);
                    }
                }
                _ => {}
            }
        }
    }

    /// Handle mouse up events.
    fn on_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.scrollbar_dragging {
            self.scrollbar_dragging = false;
            cx.notify();
            return;
        }

        let mode = self.state.mode();
        let mouse_tracking_active = mode.intersects(
            alacritty_terminal::term::TermMode::MOUSE_REPORT_CLICK
                | alacritty_terminal::term::TermMode::MOUSE_MOTION
                | alacritty_terminal::term::TermMode::MOUSE_DRAG,
        );

        let bounds = *self.last_bounds.lock();
        let origin = Point {
            x: bounds.origin.x + self.config.padding.left,
            y: bounds.origin.y + self.config.padding.top,
        };
        let (curr_cols, curr_rows) = self.dimensions();
        let max_col = curr_cols.saturating_sub(1);
        let max_row = (curr_rows as i32).saturating_sub(1);
        let mut cell_point = crate::mouse::pixel_to_cell(
            event.position,
            origin,
            self.renderer.cell_width,
            self.renderer.cell_height,
        );
        cell_point.column.0 = cell_point.column.0.min(max_col);
        cell_point.line.0 = cell_point.line.0.min(max_row);

        if mouse_tracking_active && !event.modifiers.shift {
            let modifiers = crate::mouse::encode_modifiers(
                event.modifiers.shift,
                event.modifiers.alt,
                event.modifiers.control,
            );
            if let Some(report) = crate::mouse::mouse_button_report(
                event.button,
                false,
                cell_point,
                modifiers,
                mode,
            ) {
                let mut writer = self.stdin_writer.lock();
                let _ = writer.write_all(&report);
                let _ = writer.flush();
            }
        }

        self.mouse_down_button = None;
    }

    /// Handle mouse move events.
    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.scrollbar_dragging {
            self.scroll_to_mouse_y(event.position.y, cx);
            return;
        }

        let bounds = *self.last_bounds.lock();
        let origin = Point {
            x: bounds.origin.x + self.config.padding.left,
            y: bounds.origin.y + self.config.padding.top,
        };
        let (curr_cols, curr_rows) = self.dimensions();
        let max_col = curr_cols.saturating_sub(1);
        let max_row = (curr_rows as i32).saturating_sub(1);
        let mut cell_point = crate::mouse::pixel_to_cell(
            event.position,
            origin,
            self.renderer.cell_width,
            self.renderer.cell_height,
        );
        cell_point.column.0 = cell_point.column.0.min(max_col);
        cell_point.line.0 = cell_point.line.0.min(max_row);

        let mode = self.state.mode();
        let mouse_tracking_active = mode.intersects(
            alacritty_terminal::term::TermMode::MOUSE_MOTION
                | alacritty_terminal::term::TermMode::MOUSE_DRAG,
        );

        if mouse_tracking_active && !event.modifiers.shift {
            if Some(cell_point) != self.last_reported_cell {
                let modifiers = crate::mouse::encode_modifiers(
                    event.modifiers.shift,
                    event.modifiers.alt,
                    event.modifiers.control,
                );
                if let Some(report) = crate::mouse::mouse_motion_report(
                    cell_point,
                    self.mouse_down_button,
                    modifiers,
                    mode,
                ) {
                    let mut writer = self.stdin_writer.lock();
                    let _ = writer.write_all(&report);
                    let _ = writer.flush();
                    self.last_reported_cell = Some(cell_point);
                }
            }
        } else if self.mouse_down_button == Some(MouseButton::Left) {
            // Dragging to extend selection
            if let Some(ref mut sel) = self.selection {
                if sel.end != cell_point {
                    sel.end = cell_point;
                    cx.notify();
                }
            } else {
                self.selection = Some(crate::mouse::Selection::new(
                    cell_point,
                    cell_point,
                    crate::mouse::SelectionType::Simple,
                ));
                cx.notify();
            }
        }
    }

    /// Handle scroll events.
    fn on_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cell_height = self.renderer.cell_height;
        let delta_pixels = event.delta.pixel_delta(cell_height);
        let pixel_y: f32 = delta_pixels.y.into();

        if pixel_y == 0.0 {
            return;
        }

        // Reset accumulator if scrolling direction reversed
        if (self.scroll_accumulator > 0.0 && pixel_y < 0.0)
            || (self.scroll_accumulator < 0.0 && pixel_y > 0.0)
        {
            self.scroll_accumulator = 0.0;
        }
        self.scroll_accumulator += pixel_y;

        let cell_height_f32: f32 = cell_height.into();
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
        lines = lines.clamp(-10, 10);

        let mode = self.state.mode();
        let modifiers = crate::mouse::encode_modifiers(
            event.modifiers.shift,
            event.modifiers.alt,
            event.modifiers.control,
        );
        let bounds = *self.last_bounds.lock();
        let origin = Point {
            x: bounds.origin.x + self.config.padding.left,
            y: bounds.origin.y + self.config.padding.top,
        };
        let (curr_cols, curr_rows) = self.dimensions();
        let max_col = curr_cols.saturating_sub(1);
        let max_row = (curr_rows as i32).saturating_sub(1);
        let mut cell_point = crate::mouse::pixel_to_cell(
            event.position,
            origin,
            self.renderer.cell_width,
            self.renderer.cell_height,
        );
        cell_point.column.0 = cell_point.column.0.min(max_col);
        cell_point.line.0 = cell_point.line.0.min(max_row);

        let display_offset = self.state.with_term(|term| term.grid().display_offset());

        // If viewing history (display_offset > 0) in normal screen mode (not in an alternate
        // screen / TUI) and mouse tracking is not active, browsing history takes priority.
        // In alternate screen mode (TUI), there is no history, so wheel events must always
        // be dispatched to the application (mouse reporting or arrow keys).
        if display_offset > 0
            && !mode.contains(alacritty_terminal::term::TermMode::ALT_SCREEN)
            && !mode.intersects(
                alacritty_terminal::term::TermMode::MOUSE_REPORT_CLICK
                    | alacritty_terminal::term::TermMode::MOUSE_MOTION
                    | alacritty_terminal::term::TermMode::MOUSE_DRAG,
            )
        {
            self.state.with_term_mut(|term| {
                term.scroll_display(alacritty_terminal::grid::Scroll::Delta(lines));
            });
            cx.notify();
            return;
        }

        if let Some(report) = crate::mouse::scroll_report(lines, cell_point, modifiers, mode) {
            let mut writer = self.stdin_writer.lock();
            let _ = writer.write_all(&report);
            let _ = writer.flush();
        } else {
            self.state.with_term_mut(|term| {
                term.scroll_display(alacritty_terminal::grid::Scroll::Delta(lines));
            });
            cx.notify();
        }
    }

    /// Process pending terminal events.
    ///
    /// This method drains all available events from the event receiver
    /// and handles them appropriately. Note: bytes are processed in the
    /// async reader task, not here.
    fn process_events(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Process terminal events (from alacritty event proxy)
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                TerminalEvent::Wakeup => {
                    // Terminal has new content - already handled by async task
                }
                TerminalEvent::Bell => {
                    if let Some(ref callback) = self.bell_callback {
                        callback(window, cx);
                    }
                }
                TerminalEvent::Title(title) => {
                    if let Some(ref callback) = self.title_callback {
                        callback(window, cx, &title);
                    }
                }
                TerminalEvent::ClipboardStore(text) => {
                    if let Some(ref callback) = self.clipboard_store_callback {
                        callback(window, cx, &text);
                    }
                }
                TerminalEvent::ClipboardLoad => {
                    // Terminal wants to load data from clipboard
                    // TODO: Implement clipboard integration
                }
                TerminalEvent::Exit => {
                    if let Some(ref callback) = self.exit_callback {
                        callback(window, cx);
                    }
                }
            }
        }
    }

    /// Get the current terminal dimensions.
    ///
    /// # Returns
    ///
    /// A tuple of (columns, rows).
    pub fn dimensions(&self) -> (usize, usize) {
        (self.state.cols(), self.state.rows())
    }

    /// Resize the terminal to new dimensions.
    ///
    /// This method should be called when the terminal view size changes.
    /// It updates the internal grid and notifies the terminal process of the new size.
    ///
    /// # Arguments
    ///
    /// * `cols` - New number of columns
    /// * `rows` - New number of rows
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.state.resize(cols, rows);
    }

    /// Get the current terminal configuration.
    ///
    /// # Returns
    ///
    /// A reference to the current configuration.
    pub fn config(&self) -> &TerminalConfig {
        &self.config
    }

    /// Get the focus handle for this terminal view.
    ///
    /// # Returns
    ///
    /// A reference to the focus handle.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// Update the terminal configuration.
    ///
    /// This method updates the terminal's configuration, including font settings,

    pub fn update_config(&mut self, config: TerminalConfig, cx: &mut Context<Self>) {

        let metrics_changed = self.renderer.font_family != config.font_family
            || self.renderer.font_size != config.font_size
            || self.renderer.line_height_multiplier != config.line_height_multiplier;

        self.renderer.font_family = config.font_family.clone();
        self.renderer.font_size = config.font_size;
        self.renderer.line_height_multiplier = config.line_height_multiplier;
        self.renderer.palette = config.colors.clone();

        self.config = config;

        if metrics_changed {

            self.cell_metrics_valid = false;
        }

        cx.notify();
    }

    #[allow(dead_code)]
    fn calculate_dimensions(&self, bounds: Bounds<Pixels>) -> (usize, usize) {
        let width_f32: f32 = bounds.size.width.into();
        let height_f32: f32 = bounds.size.height.into();
        let cell_width_f32: f32 = self.renderer.cell_width.into();
        let cell_height_f32: f32 = self.renderer.cell_height.into();

        let cols = ((width_f32 / cell_width_f32) as usize).max(1);
        let rows = ((height_f32 / cell_height_f32) as usize).max(1);
        (cols, rows)
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {

        self.process_events(window, cx);

        if !self.cell_metrics_valid {
            self.renderer.measure_cell(window);
            self.cell_metrics_valid = true;
        }

        let state_arc = self.state.term_arc();
        let renderer = self.renderer.clone();
        let resize_callback = self.resize_callback.clone();
        let padding = self.config.padding;
        let last_bounds = self.last_bounds.clone();
        let is_focused = self.focus_handle.is_focused(window);
        let selection = self.selection.clone();

        div()
            .size_full()
            .bg(rgb(0x1e1e1e))
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
                        use alacritty_terminal::grid::Dimensions;

                        let available_width: f32 =
                            (bounds.size.width - padding.left - padding.right).into();
                        let available_height: f32 =
                            (bounds.size.height - padding.top - padding.bottom).into();
                        let cell_width_f32: f32 = renderer.cell_width.into();
                        let cell_height_f32: f32 = renderer.cell_height.into();

                        let cols = ((available_width / cell_width_f32) as usize).max(1);
                        let rows = ((available_height / cell_height_f32) as usize).max(1);

                        struct TermSize {
                            cols: usize,
                            rows: usize,
                        }
                        impl Dimensions for TermSize {
                            fn total_lines(&self) -> usize {
                                self.rows
                            }
                            fn screen_lines(&self) -> usize {
                                self.rows
                            }
                            fn columns(&self) -> usize {
                                self.cols
                            }
                            fn last_column(&self) -> alacritty_terminal::index::Column {
                                alacritty_terminal::index::Column(self.cols.saturating_sub(1))
                            }
                            fn bottommost_line(&self) -> alacritty_terminal::index::Line {
                                alacritty_terminal::index::Line(self.rows as i32 - 1)
                            }
                            fn topmost_line(&self) -> alacritty_terminal::index::Line {
                                alacritty_terminal::index::Line(0)
                            }
                        }

                        let mut term = state_arc.lock();
                        let current_cols = term.columns();
                        let current_rows = term.screen_lines();
                        if cols != current_cols || rows != current_rows {

                            if let Some(ref callback) = resize_callback {
                                callback(cols, rows);
                            }
                            term.resize(TermSize { cols, rows });
                        }

                        renderer.paint(
                            bounds,
                            padding,
                            &term,
                            selection.as_ref(),
                            is_focused,
                            window,
                            cx,
                        );
                    },
                )
                .size_full(),
            )
    }
}
