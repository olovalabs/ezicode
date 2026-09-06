//! Mouse event handling for the terminal emulator.
//!
//! This module provides utilities for mouse interaction with the terminal:
//!
//! - [`pixel_to_cell`]: Convert pixel coordinates to grid coordinates
//! - [`mouse_button_report`]: Generate SGR mouse report sequences
//! - [`scroll_report`]: Handle scroll wheel events
//! - [`Selection`]: Text selection data structure
//!
//! # Mouse Reporting (SGR 1006)
//!
//! When mouse tracking is enabled, the terminal sends escape sequences to
//! the application. This module uses the SGR (1006) format:
//!
//! ```text
//! ESC [ < button ; column ; row M   (button press)
//! ESC [ < button ; column ; row m   (button release)
//! ```
//!
//! ## Button Encoding
//!
//! | Button | Code | With Modifiers |
//! |--------|------|----------------|
//! | Left | 0 | + modifier bits |
//! | Middle | 1 | + modifier bits |
//! | Right | 2 | + modifier bits |
//! | Wheel Up | 64 | + modifier bits |
//! | Wheel Down | 65 | + modifier bits |
//!
//! ## Modifier Bits
//!
//! | Modifier | Bit | Value |
//! |----------|-----|-------|
//! | Shift | 2 | 4 |
//! | Alt/Meta | 3 | 8 |
//! | Control | 4 | 16 |
//!
//! # Terminal Modes
//!
//! Mouse reporting depends on terminal mode flags:
//!
//! | Mode | Description |
//! |------|-------------|
//! | `MOUSE_REPORT_CLICK` | Report button press/release |
//! | `MOUSE_MOTION` | Report motion while buttons held |
//! | `MOUSE_DRAG` | Report motion during drag |
//! | `ALT_SCREEN` | Alternate screen (vim, less, etc.) |
//!
//! # Scroll Behavior
//!
//! Scroll handling depends on the terminal mode:
//!
//! 1. **Mouse mode enabled**: Send wheel events (codes 64/65)
//! 2. **Alternate screen, no mouse**: Convert to arrow keys
//! 3. **Normal screen, no mouse**: Return None (handle as scrollback)
//!
//! # Example
//!
//! ```
//! use gpui::{point, px, MouseButton};
//! use alacritty_terminal::term::TermMode;
//! use alacritty_terminal::index::{Point, Line, Column};
//! use gpui_terminal::mouse::{pixel_to_cell, mouse_button_report};
//!
//! // Convert pixel position to cell
//! let position = point(px(100.0), px(50.0));
//! let origin = point(px(10.0), px(10.0));
//! let cell = pixel_to_cell(position, origin, px(10.0), px(20.0));
//!
//! // Generate mouse report for left click
//! let point = Point::new(Line(5), Column(10));
//! let mode = TermMode::MOUSE_REPORT_CLICK;
//! let bytes = mouse_button_report(MouseButton::Left, true, point, 0, mode);
//! ```

use alacritty_terminal::index::{Column, Line, Point as AlacPoint};
use alacritty_terminal::term::TermMode;
use gpui::{MouseButton, Pixels, Point};

/// Type of text selection in the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionType {
    /// Character-by-character selection (single click).
    Simple,
    /// Word-based selection (double click).
    Word,
    /// Line-based selection (triple click).
    Line,
}

/// Represents a text selection in the terminal.
///
/// A selection has a start and end point in the terminal grid.
/// The selection is inclusive of both endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// The starting point of the selection.
    pub start: AlacPoint,
    /// The ending point of the selection.
    pub end: AlacPoint,
    /// The type of selection (character, word, or line).
    pub selection_type: SelectionType,
}

impl Selection {
    /// Create a new selection.
    ///
    /// # Arguments
    ///
    /// * `start` - The starting point of the selection
    /// * `end` - The ending point of the selection
    /// * `selection_type` - The type of selection
    ///
    /// # Returns
    ///
    /// A new `Selection` instance.
    pub fn new(start: AlacPoint, end: AlacPoint, selection_type: SelectionType) -> Self {
        Self {
            start,
            end,
            selection_type,
        }
    }

    /// Check if a point is within the selection.
    ///
    /// # Arguments
    ///
    /// * `point` - The point to check
    ///
    /// # Returns
    ///
    /// `true` if the point is within the selection, `false` otherwise.
    pub fn contains(&self, point: AlacPoint) -> bool {
        let (start, end) = if self.start < self.end {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        };

        point >= start && point <= end
    }
}

/// Convert pixel position to terminal grid coordinates.
///
/// This function transforms a pixel coordinate (e.g., from a mouse event) into
/// the corresponding cell position in the terminal grid.
///
/// # Arguments
///
/// * `position` - The pixel position to convert
/// * `origin` - The top-left origin of the terminal grid in pixels
/// * `cell_width` - The width of a single character cell in pixels
/// * `cell_height` - The height of a single character cell in pixels
///
/// # Returns
///
/// The terminal grid coordinates corresponding to the pixel position.
///
/// # Examples
///
/// ```
/// use gpui::{Point, Pixels, point, px};
/// use gpui_terminal::mouse::pixel_to_cell;
///
/// let position = point(px(100.0), px(50.0));
/// let origin = point(px(10.0), px(10.0));
/// let cell_width = px(10.0);
/// let cell_height = px(20.0);
///
/// let point = pixel_to_cell(position, origin, cell_width, cell_height);
/// // Point will be at column 9, line 2
/// ```
pub fn pixel_to_cell(
    position: Point<Pixels>,
    origin: Point<Pixels>,
    cell_width: Pixels,
    cell_height: Pixels,
) -> AlacPoint {
    // Calculate the column (x-coordinate)
    let col = ((position.x - origin.x) / cell_width).floor();
    let col = col.max(0.0) as usize;

    // Calculate the row (y-coordinate)
    let row = ((position.y - origin.y) / cell_height).floor();
    let row = row.max(0.0) as i32;

    AlacPoint::new(Line(row), Column(col))
}

/// Determine the selection type based on the number of clicks.
///
/// # Arguments
///
/// * `click_count` - The number of consecutive clicks
///
/// # Returns
///
/// The corresponding `SelectionType`:
/// - 1 click: `SelectionType::Simple`
/// - 2 clicks: `SelectionType::Word`
/// - 3 or more clicks: `SelectionType::Line`
///
/// # Examples
///
/// ```
/// use gpui_terminal::mouse::{selection_type_from_clicks, SelectionType};
///
/// assert_eq!(selection_type_from_clicks(1), SelectionType::Simple);
/// assert_eq!(selection_type_from_clicks(2), SelectionType::Word);
/// assert_eq!(selection_type_from_clicks(3), SelectionType::Line);
/// assert_eq!(selection_type_from_clicks(4), SelectionType::Line);
/// ```
pub fn selection_type_from_clicks(click_count: usize) -> SelectionType {
    match click_count {
        1 => SelectionType::Simple,
        2 => SelectionType::Word,
        _ => SelectionType::Line,
    }
}

/// Generate mouse button report escape sequence for SGR mode.
///
/// This function generates the escape sequence that should be sent to the
/// terminal application when mouse reporting is enabled. The sequence follows
/// the SGR (1006) mouse tracking format.
///
/// # Arguments
///
/// * `button` - The mouse button that was pressed/released
/// * `pressed` - `true` if the button was pressed, `false` if released
/// * `point` - The terminal grid coordinates where the event occurred
/// * `modifiers` - Modifier keys held during the event (shift, alt, ctrl)
/// * `mode` - The current terminal mode flags
///
/// # Returns
///
/// An optional vector of bytes representing the mouse report escape sequence.
/// Returns `None` if mouse reporting is not enabled in the terminal mode.
///
/// # Mouse Report Format
///
/// The SGR format is: `ESC [ < button ; col ; row M` (pressed) or `m` (released)
/// where:
/// - button is a number encoding the button and modifiers
/// - col is the column number (1-based)
/// - row is the row number (1-based)
///
/// # Examples
///
/// ```
/// use gpui::MouseButton;
/// use alacritty_terminal::term::TermMode;
/// use alacritty_terminal::index::{Point, Line, Column};
/// use gpui_terminal::mouse::mouse_button_report;
///
/// let point = Point::new(Line(5), Column(10));
/// let mode = TermMode::MOUSE_REPORT_CLICK;
///
/// let bytes = mouse_button_report(MouseButton::Left, true, point, 0, mode);
/// assert!(bytes.is_some());
/// ```
/// Format an SGR 1006 mouse report: `\x1b[<{button};{col};{row}{action}`
fn sgr_mouse_report(point: AlacPoint, button: u8, pressed: bool) -> Vec<u8> {
    let action = if pressed { 'M' } else { 'm' };
    let col = point.column.0 + 1;
    let row = point.line.0 + 1;
    format!("\x1b[<{};{};{}{}", button, col, row, action).into_bytes()
}

/// Format a standard DEC 1000 / X10 mouse report: `\x1b[M{cb}{cx}{cy}`
fn normal_mouse_report(point: AlacPoint, button: u8, utf8: bool) -> Option<Vec<u8>> {
    let max_point = if utf8 { 2015 } else { 223 };

    let col = point.column.0;
    let row = point.line.0;
    if row < 0 || (row as usize) >= max_point || col >= max_point {
        return None;
    }

    let mut msg = vec![b'\x1b', b'[', b'M', 32 + button];

    let mouse_pos_encode = |pos: usize| -> Vec<u8> {
        let pos = 32 + 1 + pos;
        let first = 0xC0 + pos / 64;
        let second = 0x80 + (pos & 63);
        vec![first as u8, second as u8]
    };

    if utf8 && col >= 95 {
        msg.append(&mut mouse_pos_encode(col));
    } else {
        msg.push(32 + 1 + col as u8);
    }

    if utf8 && row >= 95 {
        msg.append(&mut mouse_pos_encode(row as usize));
    } else {
        msg.push(32 + 1 + row as u8);
    }

    Some(msg)
}

/// Encode mouse event according to active terminal mode (SGR 1006, UTF-8 1005, or DEC 1000 X10).
fn format_mouse_report(
    point: AlacPoint,
    button_value: u8,
    pressed: bool,
    mode: TermMode,
) -> Option<Vec<u8>> {
    if point.line.0 < 0 {
        return None;
    }

    if mode.contains(TermMode::SGR_MOUSE) {
        Some(sgr_mouse_report(point, button_value, pressed))
    } else if mode.contains(TermMode::UTF8_MOUSE) {
        if pressed {
            normal_mouse_report(point, button_value, true)
        } else {
            let modifiers = button_value & (4 | 8 | 16);
            normal_mouse_report(point, 3 | modifiers, true)
        }
    } else {
        // Standard DEC 1000 / X10 mode
        if pressed {
            normal_mouse_report(point, button_value, false)
        } else {
            let modifiers = button_value & (4 | 8 | 16);
            normal_mouse_report(point, 3 | modifiers, false)
        }
    }
}

pub fn mouse_button_report(
    button: MouseButton,
    pressed: bool,
    point: AlacPoint,
    modifiers: u8,
    mode: TermMode,
) -> Option<Vec<u8>> {
    // Check if mouse reporting is enabled
    if !mode
        .intersects(TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG)
    {
        return None;
    }

    // Encode button number
    let button_code = match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        _ => return None, // Ignore other buttons
    };

    // Add modifier bits: Bit 2: Shift, Bit 3: Alt, Bit 4: Control
    let button_value = button_code | modifiers;
    format_mouse_report(point, button_value, pressed, mode)
}

/// Generate mouse motion/drag report escape sequence.
///
/// Supports SGR 1006 mode, UTF-8 mode, and standard DEC 1000 X10 mode.
pub fn mouse_motion_report(
    point: AlacPoint,
    button: Option<MouseButton>,
    modifiers: u8,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let motion_enabled = mode.contains(TermMode::MOUSE_MOTION);
    let drag_enabled = mode.contains(TermMode::MOUSE_DRAG) && button.is_some();

    if !motion_enabled && !drag_enabled {
        return None;
    }

    // Base motion code is 32.
    let button_code = match button {
        Some(MouseButton::Left) => 32,
        Some(MouseButton::Middle) => 33,
        Some(MouseButton::Right) => 34,
        _ => 35,
    };

    let button_value = button_code | modifiers;
    format_mouse_report(point, button_value, true, mode)
}

/// Generate scroll wheel report escape sequence.
///
/// This function generates the escape sequence for scroll wheel events:
/// - When mouse reporting is enabled (and Shift is not held): sends repeated mouse wheel events
///   in SGR 1006 or DEC 1000 X10 format matching the application's negotiated mode.
/// - When alternate screen or application cursor mode is active (TUI like vim, nano, less,
///   htop, lazygit, git log) and Shift is not held: sends arrow key sequences (faux scrolling).
/// - Otherwise: returns `None` so the local terminal scrollback buffer can be scrolled.
pub fn scroll_report(
    delta: i32,
    point: AlacPoint,
    modifiers: u8,
    mode: TermMode,
) -> Option<Vec<u8>> {
    if delta == 0 {
        return None;
    }

    // Bit 2 is Shift (value 4). Shift overrides mouse reporting/faux scrolling
    // so the user can scroll terminal history or select text.
    let shift_held = (modifiers & 4) != 0;

    // 1. If mouse reporting is enabled and Shift is not held, send mouse wheel events
    if !shift_held
        && mode.intersects(
            TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG,
        )
    {
        // Button codes for scroll: 64 = wheel up, 65 = wheel down
        let button_code = if delta > 0 { 64 } else { 65 };
        let button_value = button_code | modifiers;

        if let Some(single_report) = format_mouse_report(point, button_value, true, mode) {
            let count = delta.unsigned_abs().min(10) as usize;
            let mut result = Vec::with_capacity(single_report.len() * count);
            for _ in 0..count {
                result.extend_from_slice(&single_report);
            }
            return Some(result);
        }
    }

    // 2. If in alternate screen mode or application cursor mode (TUI like vim, nano, less,
    // htop, lazygit, git log) and Shift is not held, translate scroll wheel to arrow keys.
    if !shift_held
        && (mode.contains(TermMode::ALT_SCREEN)
            || mode.contains(TermMode::APP_CURSOR))
    {
        return Some(scroll_to_arrow_keys(delta, mode));
    }

    // 3. In normal screen mode without mouse reporting, let the terminal handle scrollback
    None
}

/// Convert scroll delta to arrow key sequences.
///
/// This is used when an application is in alternate screen or application cursor mode
/// (like vim, nano, less, git log) but doesn't have mouse reporting enabled. The scroll
/// wheel is translated to arrow key presses to allow smooth TUI navigation.
fn scroll_to_arrow_keys(delta: i32, mode: TermMode) -> Vec<u8> {
    let count = delta.unsigned_abs().min(10) as usize;

    // Determine arrow key sequence based on mode
    let arrow_seq = if delta > 0 {
        // Scroll up = arrow up
        if mode.contains(TermMode::APP_CURSOR) {
            b"\x1bOA"
        } else {
            b"\x1b[A"
        }
    } else {
        // Scroll down = arrow down
        if mode.contains(TermMode::APP_CURSOR) {
            b"\x1bOB"
        } else {
            b"\x1b[B"
        }
    };

    // Repeat the arrow key sequence
    let mut result = Vec::with_capacity(arrow_seq.len() * count);
    for _ in 0..count {
        result.extend_from_slice(arrow_seq);
    }
    result
}

/// Encode modifier keys as a bitmask for mouse reporting.
///
/// # Arguments
///
/// * `shift` - Whether Shift is pressed
/// * `alt` - Whether Alt is pressed
/// * `control` - Whether Control is pressed
///
/// # Returns
///
/// A bitmask encoding the modifiers:
/// - Bit 2 (4): Shift
/// - Bit 3 (8): Alt/Meta
/// - Bit 4 (16): Control
///
/// # Examples
///
/// ```
/// use gpui_terminal::mouse::encode_modifiers;
///
/// assert_eq!(encode_modifiers(false, false, false), 0);
/// assert_eq!(encode_modifiers(true, false, false), 4);
/// assert_eq!(encode_modifiers(false, true, false), 8);
/// assert_eq!(encode_modifiers(false, false, true), 16);
/// assert_eq!(encode_modifiers(true, true, true), 28);
/// ```
pub fn encode_modifiers(shift: bool, alt: bool, control: bool) -> u8 {
    let mut modifiers = 0;
    if shift {
        modifiers |= 4;
    }
    if alt {
        modifiers |= 8;
    }
    if control {
        modifiers |= 16;
    }
    modifiers
}

/// Calculate the number of lines to scroll based on pixel delta.
///
/// This converts a pixel-based scroll delta (from a scroll wheel or trackpad)
/// into a number of terminal lines to scroll.
///
/// # Arguments
///
/// * `pixel_delta` - The scroll delta in pixels (positive = up)
/// * `cell_height` - The height of a character cell in pixels
///
/// # Returns
///
/// The number of lines to scroll (positive = up, negative = down).
/// The result is clamped to a reasonable range.
///
/// # Examples
///
/// ```
/// use gpui::px;
/// use gpui_terminal::mouse::pixels_to_scroll_lines;
///
/// let cell_height = px(20.0);
/// assert_eq!(pixels_to_scroll_lines(px(60.0), cell_height), 3);
/// assert_eq!(pixels_to_scroll_lines(px(-40.0), cell_height), -2);
/// ```
pub fn pixels_to_scroll_lines(pixel_delta: Pixels, cell_height: Pixels) -> i32 {
    let lines = (pixel_delta / cell_height).round();
    // Clamp to reasonable range (-10 to 10 lines per scroll event)
    lines.clamp(-10.0, 10.0) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px};

    #[test]
    fn test_pixel_to_cell() {
        let position = point(px(100.0), px(50.0));
        let origin = point(px(10.0), px(10.0));
        let cell_width = px(10.0);
        let cell_height = px(20.0);

        let point = pixel_to_cell(position, origin, cell_width, cell_height);
        assert_eq!(point.column.0, 9);
        assert_eq!(point.line.0, 2);
    }

    #[test]
    fn test_pixel_to_cell_at_origin() {
        let position = point(px(10.0), px(10.0));
        let origin = point(px(10.0), px(10.0));
        let cell_width = px(10.0);
        let cell_height = px(20.0);

        let point = pixel_to_cell(position, origin, cell_width, cell_height);
        assert_eq!(point.column.0, 0);
        assert_eq!(point.line.0, 0);
    }

    #[test]
    fn test_pixel_to_cell_negative_coordinates() {
        // Coordinates before origin should clamp to 0
        let position = point(px(5.0), px(5.0));
        let origin = point(px(10.0), px(10.0));
        let cell_width = px(10.0);
        let cell_height = px(20.0);

        let point = pixel_to_cell(position, origin, cell_width, cell_height);
        assert_eq!(point.column.0, 0);
        assert_eq!(point.line.0, 0);
    }

    #[test]
    fn test_selection_type_from_clicks() {
        assert_eq!(selection_type_from_clicks(1), SelectionType::Simple);
        assert_eq!(selection_type_from_clicks(2), SelectionType::Word);
        assert_eq!(selection_type_from_clicks(3), SelectionType::Line);
        assert_eq!(selection_type_from_clicks(4), SelectionType::Line);
        assert_eq!(selection_type_from_clicks(10), SelectionType::Line);
    }

    #[test]
    fn test_selection_contains() {
        let selection = Selection::new(
            AlacPoint::new(Line(5), Column(10)),
            AlacPoint::new(Line(7), Column(20)),
            SelectionType::Simple,
        );

        // Point within selection
        assert!(selection.contains(AlacPoint::new(Line(6), Column(15))));

        // Start and end points
        assert!(selection.contains(AlacPoint::new(Line(5), Column(10))));
        assert!(selection.contains(AlacPoint::new(Line(7), Column(20))));

        // Points outside selection
        assert!(!selection.contains(AlacPoint::new(Line(4), Column(15))));
        assert!(!selection.contains(AlacPoint::new(Line(8), Column(15))));
    }

    #[test]
    fn test_selection_contains_reverse() {
        // Test with end < start (reversed selection)
        let selection = Selection::new(
            AlacPoint::new(Line(7), Column(20)),
            AlacPoint::new(Line(5), Column(10)),
            SelectionType::Simple,
        );

        // Should still work correctly
        assert!(selection.contains(AlacPoint::new(Line(6), Column(15))));
        assert!(selection.contains(AlacPoint::new(Line(5), Column(10))));
        assert!(selection.contains(AlacPoint::new(Line(7), Column(20))));
    }

    #[test]
    fn test_mouse_button_report_left_click() {
        let point = AlacPoint::new(Line(5), Column(10));
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;

        let bytes = mouse_button_report(MouseButton::Left, true, point, 0, mode);
        assert!(bytes.is_some());

        let sequence = String::from_utf8(bytes.unwrap()).unwrap();
        // SGR format: ESC[<button;col;row M
        // Line 5 (0-indexed) = row 6 (1-indexed)
        // Column 10 (0-indexed) = col 11 (1-indexed)
        assert_eq!(sequence, "\x1b[<0;11;6M");
    }

    #[test]
    fn test_mouse_button_report_normal_x10() {
        let point = AlacPoint::new(Line(5), Column(10));
        let mode = TermMode::MOUSE_REPORT_CLICK; // Normal X10 mode without SGR_MOUSE

        let bytes = mouse_button_report(MouseButton::Left, true, point, 0, mode);
        assert!(bytes.is_some());
        // ESC [ M (32 + 0) (32 + 1 + 10) (32 + 1 + 5)
        // 32 = ' ', 43 = '+', 38 = '&'
        assert_eq!(bytes.unwrap(), vec![0x1b, b'[', b'M', 32, 43, 38]);
    }

    #[test]
    fn test_mouse_button_report_right_release() {
        let point = AlacPoint::new(Line(0), Column(0));
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;

        let bytes = mouse_button_report(MouseButton::Right, false, point, 0, mode);
        assert!(bytes.is_some());

        let sequence = String::from_utf8(bytes.unwrap()).unwrap();
        // Right button = 2, released = 'm'
        assert_eq!(sequence, "\x1b[<2;1;1m");
    }

    #[test]
    fn test_mouse_button_report_with_modifiers() {
        let point = AlacPoint::new(Line(0), Column(0));
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let modifiers = encode_modifiers(true, true, true); // Shift + Alt + Ctrl = 28

        let bytes = mouse_button_report(MouseButton::Left, true, point, modifiers, mode);
        assert!(bytes.is_some());

        let sequence = String::from_utf8(bytes.unwrap()).unwrap();
        // Button 0 + modifiers 28 = 28
        assert_eq!(sequence, "\x1b[<28;1;1M");
    }

    #[test]
    fn test_mouse_button_report_disabled() {
        let point = AlacPoint::new(Line(0), Column(0));
        let mode = TermMode::empty();

        let bytes = mouse_button_report(MouseButton::Left, true, point, 0, mode);
        assert!(bytes.is_none());
    }

    #[test]
    fn test_scroll_report_mouse_mode() {
        let point = AlacPoint::new(Line(5), Column(10));
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;

        // Scroll up 1 line
        let bytes = scroll_report(1, point, 0, mode);
        assert!(bytes.is_some());
        let sequence = String::from_utf8(bytes.unwrap()).unwrap();
        // Wheel up = button 64
        assert_eq!(sequence, "\x1b[<64;11;6M");

        // Scroll up 3 lines -> repeated 3 times
        let bytes = scroll_report(3, point, 0, mode);
        assert!(bytes.is_some());
        let sequence = String::from_utf8(bytes.unwrap()).unwrap();
        assert_eq!(sequence, "\x1b[<64;11;6M\x1b[<64;11;6M\x1b[<64;11;6M");

        // Scroll down 2 lines -> repeated 2 times
        let bytes = scroll_report(-2, point, 0, mode);
        assert!(bytes.is_some());
        let sequence = String::from_utf8(bytes.unwrap()).unwrap();
        // Wheel down = button 65
        assert_eq!(sequence, "\x1b[<65;11;6M\x1b[<65;11;6M");
    }

    #[test]
    fn test_scroll_report_normal_x10_mouse_mode() {
        let point = AlacPoint::new(Line(5), Column(10));
        let mode = TermMode::MOUSE_REPORT_CLICK; // Normal X10 mode without SGR_MOUSE

        let bytes = scroll_report(1, point, 0, mode);
        assert!(bytes.is_some());
        // ESC [ M (32 + 64) (32 + 1 + 10) (32 + 1 + 5)
        assert_eq!(bytes.unwrap(), vec![0x1b, b'[', b'M', 96, 43, 38]);
    }

    #[test]
    fn test_scroll_report_shift_bypass() {
        let point = AlacPoint::new(Line(0), Column(0));
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let shift_mod = 4; // Shift held

        // Holding Shift must bypass mouse reporting to allow scrolling outer buffer
        let bytes = scroll_report(3, point, shift_mod, mode);
        assert!(bytes.is_none());
    }

    #[test]
    fn test_scroll_report_alternate_screen() {
        let point = AlacPoint::new(Line(0), Column(0));
        let mode = TermMode::ALT_SCREEN;

        // Scroll up in alternate screen = arrow up keys
        let bytes = scroll_report(3, point, 0, mode);
        assert!(bytes.is_some());
        let sequence = bytes.unwrap();
        // Should be 3 arrow up sequences
        assert_eq!(sequence, b"\x1b[A\x1b[A\x1b[A");
    }

    #[test]
    fn test_scroll_report_app_cursor_conpty_tui() {
        let point = AlacPoint::new(Line(0), Column(0));
        // On Windows ConPTY, ALT_SCREEN may be swallowed, but APP_CURSOR is active for TUIs
        let mode = TermMode::APP_CURSOR;

        // Scroll down with app cursor mode -> translates to \x1bOB
        let bytes = scroll_report(-2, point, 0, mode);
        assert!(bytes.is_some());
        let sequence = bytes.unwrap();
        assert_eq!(sequence, b"\x1bOB\x1bOB");

        // Scroll up with app cursor mode -> translates to \x1bOA
        let bytes = scroll_report(2, point, 0, mode);
        assert!(bytes.is_some());
        let sequence = bytes.unwrap();
        assert_eq!(sequence, b"\x1bOA\x1bOA");
    }

    #[test]
    fn test_scroll_report_normal_screen() {
        let point = AlacPoint::new(Line(0), Column(0));
        let mode = TermMode::empty();

        // In normal screen mode, scrolling should be handled locally
        let bytes = scroll_report(3, point, 0, mode);
        assert!(bytes.is_none());
    }

    #[test]
    fn test_encode_modifiers() {
        assert_eq!(encode_modifiers(false, false, false), 0);
        assert_eq!(encode_modifiers(true, false, false), 4);
        assert_eq!(encode_modifiers(false, true, false), 8);
        assert_eq!(encode_modifiers(false, false, true), 16);
        assert_eq!(encode_modifiers(true, true, false), 12);
        assert_eq!(encode_modifiers(true, false, true), 20);
        assert_eq!(encode_modifiers(false, true, true), 24);
        assert_eq!(encode_modifiers(true, true, true), 28);
    }

    #[test]
    fn test_pixels_to_scroll_lines() {
        let cell_height = px(20.0);

        assert_eq!(pixels_to_scroll_lines(px(60.0), cell_height), 3);
        assert_eq!(pixels_to_scroll_lines(px(-40.0), cell_height), -2);
        assert_eq!(pixels_to_scroll_lines(px(10.0), cell_height), 1);
        assert_eq!(pixels_to_scroll_lines(px(-10.0), cell_height), -1);

        // Test clamping
        assert_eq!(pixels_to_scroll_lines(px(300.0), cell_height), 10);
        assert_eq!(pixels_to_scroll_lines(px(-300.0), cell_height), -10);
    }

    #[test]
    fn test_scroll_to_arrow_keys_limit() {
        let mode = TermMode::empty();

        // Large scroll should be clamped to 10 lines
        let bytes = scroll_to_arrow_keys(100, mode);
        let expected = b"\x1b[A\x1b[A\x1b[A\x1b[A\x1b[A\x1b[A\x1b[A\x1b[A\x1b[A\x1b[A";
        assert_eq!(bytes, expected);

        let bytes = scroll_to_arrow_keys(-100, mode);
        let expected = b"\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B";
        assert_eq!(bytes, expected);
    }

    #[test]
    fn test_mouse_motion_report() {
        let point = AlacPoint::new(Line(3), Column(7)); // col 8, row 4

        // 1. In MOUSE_DRAG | SGR_MOUSE mode with Left button held:
        let mode_drag = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let report = mouse_motion_report(point, Some(MouseButton::Left), 0, mode_drag);
        assert!(report.is_some());
        assert_eq!(String::from_utf8(report.unwrap()).unwrap(), "\x1b[<32;8;4M");

        // 2. In MOUSE_DRAG mode with no button held: returns None
        let report = mouse_motion_report(point, None, 0, mode_drag);
        assert!(report.is_none());

        // 3. In MOUSE_MOTION | SGR_MOUSE mode with no button: code 35
        let mode_motion = TermMode::MOUSE_MOTION | TermMode::SGR_MOUSE;
        let report = mouse_motion_report(point, None, 0, mode_motion);
        assert!(report.is_some());
        assert_eq!(String::from_utf8(report.unwrap()).unwrap(), "\x1b[<35;8;4M");

        // 4. In MOUSE_REPORT_CLICK mode (clicks only, no drag/motion): returns None
        let mode_click = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let report = mouse_motion_report(point, Some(MouseButton::Left), 0, mode_click);
        assert!(report.is_none());
    }
}

