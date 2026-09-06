//! Keyboard input handling for the terminal emulator.
//!
//! This module provides [`keystroke_to_bytes`], which converts GPUI keyboard
//! events into terminal escape sequences that can be written to the PTY.
//!
//! # Key Mappings
//!
//! ## Special Keys
//!
//! | Key | Sequence | Notes |
//! |-----|----------|-------|
//! | Enter | `\r` (0x0D) | Carriage return |
//! | Escape | `\x1b` (0x1B) | ESC |
//! | Backspace | `\x7f` (0x7F) | DEL |
//! | Tab | `\t` (0x09) | Horizontal tab |
//! | Shift+Tab | `\x1b[Z` | Backtab |
//! | Space | ` ` (0x20) | Space |
//! | Ctrl+Space | `\x00` | NUL |
//!
//! ## Arrow Keys
//!
//! Arrow key sequences depend on application cursor mode:
//!
//! | Key | Normal Mode | App Cursor Mode |
//! |-----|-------------|-----------------|
//! | Up | `\x1b[A` | `\x1bOA` |
//! | Down | `\x1b[B` | `\x1bOB` |
//! | Right | `\x1b[C` | `\x1bOC` |
//! | Left | `\x1b[D` | `\x1bOD` |
//!
//! ## Navigation Keys
//!
//! | Key | Sequence |
//! |-----|----------|
//! | Home | `\x1b[H` |
//! | End | `\x1b[F` |
//! | PageUp | `\x1b[5~` |
//! | PageDown | `\x1b[6~` |
//! | Insert | `\x1b[2~` |
//! | Delete | `\x1b[3~` |
//!
//! ## Function Keys
//!
//! | Key | Sequence |
//! |-----|----------|
//! | F1-F4 | `\x1bOP` - `\x1bOS` |
//! | F5-F12 | `\x1b[15~` - `\x1b[24~` |
//!
//! ## Control Combinations
//!
//! Ctrl+A through Ctrl+Z map to ASCII control characters 0x01-0x1A:
//!
//! | Combination | Byte |
//! |-------------|------|
//! | Ctrl+A | 0x01 |
//! | Ctrl+C | 0x03 (interrupt) |
//! | Ctrl+D | 0x04 (EOF) |
//! | Ctrl+Z | 0x1A (suspend) |
//!
//! ## Alt Combinations
//!
//! Alt+key sends ESC followed by the key: `\x1b` + key
//!
//! # Terminal Mode Effects
//!
//! The [`TermMode`] flags affect key sequences:
//!
//! - **APP_CURSOR**: Changes arrow key sequences from CSI to SS3 format
//!
//! # Example
//!
//! ```
//! use gpui::Keystroke;
//! use alacritty_terminal::term::TermMode;
//! use gpui_terminal::input::keystroke_to_bytes;
//!
//! // Enter key
//! let keystroke = Keystroke::parse("enter").unwrap();
//! assert_eq!(keystroke_to_bytes(&keystroke, TermMode::empty()), Some(b"\r".to_vec()));
//!
//! // Ctrl+C (interrupt)
//! let keystroke = Keystroke::parse("ctrl-c").unwrap();
//! assert_eq!(keystroke_to_bytes(&keystroke, TermMode::empty()), Some(vec![0x03]));
//! ```

use alacritty_terminal::term::TermMode;
use gpui::Keystroke;

/// Convert a GPUI keystroke to terminal escape sequence bytes.
///
/// This function translates GPUI keyboard events into the appropriate byte sequences
/// expected by terminal applications. It handles special keys, control characters,
/// and application cursor mode.
///
/// # Arguments
///
/// * `keystroke` - The GPUI keystroke to convert
/// * `mode` - The current terminal mode (affects arrow key sequences)
///
/// # Returns
///
/// An optional vector of bytes representing the terminal escape sequence.
/// Returns `None` if the keystroke should not produce any output.
///
/// # Examples
///
/// ```
/// use gpui::Keystroke;
/// use alacritty_terminal::term::TermMode;
/// use gpui_terminal::input::keystroke_to_bytes;
///
/// let keystroke = Keystroke::parse("enter").unwrap();
/// let bytes = keystroke_to_bytes(&keystroke, TermMode::empty());
/// assert_eq!(bytes, Some(b"\r".to_vec()));
/// ```
pub fn keystroke_to_bytes(keystroke: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let key_lower = keystroke.key.to_ascii_lowercase();

    // Standard xterm modifier calculation: 1 + shift*1 + alt*2 + ctrl*4
    let mod_code = 1
        + (if keystroke.modifiers.shift { 1 } else { 0 })
        + (if keystroke.modifiers.alt { 2 } else { 0 })
        + (if keystroke.modifiers.control { 4 } else { 0 });

    // Handle special navigation and function keys
    match key_lower.as_str() {
        "space" => {
            if keystroke.modifiers.control {
                return Some(vec![0x00]); // Ctrl+Space = NUL
            }
            return Some(b" ".to_vec());
        }
        "enter" | "return" => return Some(b"\r".to_vec()),
        "escape" => return Some(b"\x1b".to_vec()),
        "tab" => {
            if keystroke.modifiers.shift {
                return Some(b"\x1b[Z".to_vec()); // Backtab
            }
            return Some(b"\t".to_vec());
        }
        "backspace" => {
            if keystroke.modifiers.alt {
                return Some(b"\x1b\x7f".to_vec()); // Alt+Backspace (word delete backwards)
            }
            if keystroke.modifiers.control {
                return Some(vec![0x08]); // Ctrl+Backspace (BS)
            }
            return Some(b"\x7f".to_vec());
        }

        // Arrow keys
        "up" => {
            if mod_code > 1 {
                return Some(format!("\x1b[1;{mod_code}A").into_bytes());
            }
            if mode.contains(TermMode::APP_CURSOR) {
                return Some(b"\x1bOA".to_vec());
            }
            return Some(b"\x1b[A".to_vec());
        }
        "down" => {
            if mod_code > 1 {
                return Some(format!("\x1b[1;{mod_code}B").into_bytes());
            }
            if mode.contains(TermMode::APP_CURSOR) {
                return Some(b"\x1bOB".to_vec());
            }
            return Some(b"\x1b[B".to_vec());
        }
        "right" => {
            if mod_code > 1 {
                return Some(format!("\x1b[1;{mod_code}C").into_bytes());
            }
            if mode.contains(TermMode::APP_CURSOR) {
                return Some(b"\x1bOC".to_vec());
            }
            return Some(b"\x1b[C".to_vec());
        }
        "left" => {
            if mod_code > 1 {
                return Some(format!("\x1b[1;{mod_code}D").into_bytes());
            }
            if mode.contains(TermMode::APP_CURSOR) {
                return Some(b"\x1bOD".to_vec());
            }
            return Some(b"\x1b[D".to_vec());
        }

        // Navigation keys
        "home" => {
            if mod_code > 1 {
                return Some(format!("\x1b[1;{mod_code}H").into_bytes());
            }
            if mode.contains(TermMode::APP_CURSOR) {
                return Some(b"\x1bOH".to_vec());
            }
            return Some(b"\x1b[H".to_vec());
        }
        "end" => {
            if mod_code > 1 {
                return Some(format!("\x1b[1;{mod_code}F").into_bytes());
            }
            if mode.contains(TermMode::APP_CURSOR) {
                return Some(b"\x1bOF".to_vec());
            }
            return Some(b"\x1b[F".to_vec());
        }
        "pageup" => {
            if mod_code > 1 {
                return Some(format!("\x1b[5;{mod_code}~").into_bytes());
            }
            return Some(b"\x1b[5~".to_vec());
        }
        "pagedown" => {
            if mod_code > 1 {
                return Some(format!("\x1b[6;{mod_code}~").into_bytes());
            }
            return Some(b"\x1b[6~".to_vec());
        }
        "insert" => {
            if mod_code > 1 {
                return Some(format!("\x1b[2;{mod_code}~").into_bytes());
            }
            return Some(b"\x1b[2~".to_vec());
        }
        "delete" => {
            if mod_code > 1 {
                return Some(format!("\x1b[3;{mod_code}~").into_bytes());
            }
            return Some(b"\x1b[3~".to_vec());
        }

        // Function keys F1-F4
        "f1" => {
            if mod_code > 1 {
                return Some(format!("\x1b[1;{mod_code}P").into_bytes());
            }
            return Some(b"\x1bOP".to_vec());
        }
        "f2" => {
            if mod_code > 1 {
                return Some(format!("\x1b[1;{mod_code}Q").into_bytes());
            }
            return Some(b"\x1bOQ".to_vec());
        }
        "f3" => {
            if mod_code > 1 {
                return Some(format!("\x1b[1;{mod_code}R").into_bytes());
            }
            return Some(b"\x1bOR".to_vec());
        }
        "f4" => {
            if mod_code > 1 {
                return Some(format!("\x1b[1;{mod_code}S").into_bytes());
            }
            return Some(b"\x1bOS".to_vec());
        }

        // Function keys F5-F12
        "f5" => {
            if mod_code > 1 {
                return Some(format!("\x1b[15;{mod_code}~").into_bytes());
            }
            return Some(b"\x1b[15~".to_vec());
        }
        "f6" => {
            if mod_code > 1 {
                return Some(format!("\x1b[17;{mod_code}~").into_bytes());
            }
            return Some(b"\x1b[17~".to_vec());
        }
        "f7" => {
            if mod_code > 1 {
                return Some(format!("\x1b[18;{mod_code}~").into_bytes());
            }
            return Some(b"\x1b[18~".to_vec());
        }
        "f8" => {
            if mod_code > 1 {
                return Some(format!("\x1b[19;{mod_code}~").into_bytes());
            }
            return Some(b"\x1b[19~".to_vec());
        }
        "f9" => {
            if mod_code > 1 {
                return Some(format!("\x1b[20;{mod_code}~").into_bytes());
            }
            return Some(b"\x1b[20~".to_vec());
        }
        "f10" => {
            if mod_code > 1 {
                return Some(format!("\x1b[21;{mod_code}~").into_bytes());
            }
            return Some(b"\x1b[21~".to_vec());
        }
        "f11" => {
            if mod_code > 1 {
                return Some(format!("\x1b[23;{mod_code}~").into_bytes());
            }
            return Some(b"\x1b[23~".to_vec());
        }
        "f12" => {
            if mod_code > 1 {
                return Some(format!("\x1b[24;{mod_code}~").into_bytes());
            }
            return Some(b"\x1b[24~".to_vec());
        }

        _ => {}
    }

    // Handle Ctrl+key combinations
    if keystroke.modifiers.control && !keystroke.modifiers.alt {
        let key = keystroke.key.as_str();
        if key.len() == 1 {
            let ch = key.chars().next().unwrap();
            if ch.is_ascii_alphabetic() {
                let upper = ch.to_ascii_uppercase();
                let ctrl_char = (upper as u8) - b'@';
                return Some(vec![ctrl_char]);
            }

            match ch {
                '[' => return Some(b"\x1b".to_vec()),  // Ctrl+[
                '\\' => return Some(b"\x1c".to_vec()), // Ctrl+\
                ']' => return Some(b"\x1d".to_vec()),  // Ctrl+]
                '^' => return Some(b"\x1e".to_vec()),  // Ctrl+^
                '_' => return Some(b"\x1f".to_vec()),  // Ctrl+_
                '?' => return Some(b"\x7f".to_vec()),  // Ctrl+?
                _ => {}
            }
        }
    }

    // Handle Alt+key combinations
    if keystroke.modifiers.alt && !keystroke.modifiers.control {
        let key = keystroke.key.as_str();
        if key.len() == 1 {
            let ch = key.chars().next().unwrap();
            if ch.is_ascii() {
                return Some(vec![b'\x1b', ch as u8]);
            }
        }
    }

    // Handle regular printable characters:
    // If key_char is available (from IME or platform event)
    if !keystroke.modifiers.control && !keystroke.modifiers.alt {
        if let Some(key_char) = &keystroke.key_char {
            if !key_char.is_empty() {
                return Some(key_char.as_bytes().to_vec());
            }
        }
    }

    // Fallback for shifted US keyboard layout symbols when key_char is None (common on Windows)
    let key = keystroke.key.as_str();
    if key.len() == 1 {
        let ch = key.chars().next().unwrap();
        if keystroke.modifiers.shift && !keystroke.modifiers.control && !keystroke.modifiers.alt {
            let shifted = match ch {
                '1' => '!',
                '2' => '@',
                '3' => '#',
                '4' => '$',
                '5' => '%',
                '6' => '^',
                '7' => '&',
                '8' => '*',
                '9' => '(',
                '0' => ')',
                '-' => '_',
                '=' => '+',
                '[' => '{',
                ']' => '}',
                '\\' => '|',
                ';' => ':',
                '\'' => '"',
                ',' => '<',
                '.' => '>',
                '/' => '?',
                '`' => '~',
                _ if ch.is_ascii_alphabetic() => ch.to_ascii_uppercase(),
                _ => ch,
            };
            return Some(vec![shifted as u8]);
        }

        if ch.is_ascii() && !keystroke.modifiers.control && !keystroke.modifiers.alt {
            let out_char = if ch.is_ascii_alphabetic() {
                ch.to_ascii_lowercase()
            } else {
                ch
            };
            return Some(vec![out_char as u8]);
        }

        // For non-ASCII characters, encode as UTF-8
        if !keystroke.modifiers.control && !keystroke.modifiers.alt {
            return Some(key.as_bytes().to_vec());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enter_key() {
        let keystroke = Keystroke::parse("enter").unwrap();
        let bytes = keystroke_to_bytes(&keystroke, TermMode::empty());
        assert_eq!(bytes, Some(b"\r".to_vec()));
    }

    #[test]
    fn test_escape_key() {
        let keystroke = Keystroke::parse("escape").unwrap();
        let bytes = keystroke_to_bytes(&keystroke, TermMode::empty());
        assert_eq!(bytes, Some(b"\x1b".to_vec()));
    }

    #[test]
    fn test_backspace_key() {
        let keystroke = Keystroke::parse("backspace").unwrap();
        let bytes = keystroke_to_bytes(&keystroke, TermMode::empty());
        assert_eq!(bytes, Some(b"\x7f".to_vec()));
    }

    #[test]
    fn test_tab_key() {
        let keystroke = Keystroke::parse("tab").unwrap();
        let bytes = keystroke_to_bytes(&keystroke, TermMode::empty());
        assert_eq!(bytes, Some(b"\t".to_vec()));
    }

    #[test]
    fn test_shift_tab() {
        let keystroke = Keystroke::parse("shift-tab").unwrap();
        let bytes = keystroke_to_bytes(&keystroke, TermMode::empty());
        assert_eq!(bytes, Some(b"\x1b[Z".to_vec()));
    }

    #[test]
    fn test_arrow_keys_normal_mode() {
        let mode = TermMode::empty();

        let up = Keystroke::parse("up").unwrap();
        assert_eq!(keystroke_to_bytes(&up, mode), Some(b"\x1b[A".to_vec()));

        let down = Keystroke::parse("down").unwrap();
        assert_eq!(keystroke_to_bytes(&down, mode), Some(b"\x1b[B".to_vec()));

        let right = Keystroke::parse("right").unwrap();
        assert_eq!(keystroke_to_bytes(&right, mode), Some(b"\x1b[C".to_vec()));

        let left = Keystroke::parse("left").unwrap();
        assert_eq!(keystroke_to_bytes(&left, mode), Some(b"\x1b[D".to_vec()));
    }

    #[test]
    fn test_arrow_keys_app_cursor_mode() {
        let mode = TermMode::APP_CURSOR;

        let up = Keystroke::parse("up").unwrap();
        assert_eq!(keystroke_to_bytes(&up, mode), Some(b"\x1bOA".to_vec()));

        let down = Keystroke::parse("down").unwrap();
        assert_eq!(keystroke_to_bytes(&down, mode), Some(b"\x1bOB".to_vec()));

        let right = Keystroke::parse("right").unwrap();
        assert_eq!(keystroke_to_bytes(&right, mode), Some(b"\x1bOC".to_vec()));

        let left = Keystroke::parse("left").unwrap();
        assert_eq!(keystroke_to_bytes(&left, mode), Some(b"\x1bOD".to_vec()));
    }

    #[test]
    fn test_navigation_keys() {
        let mode = TermMode::empty();

        let home = Keystroke::parse("home").unwrap();
        assert_eq!(keystroke_to_bytes(&home, mode), Some(b"\x1b[H".to_vec()));

        let end = Keystroke::parse("end").unwrap();
        assert_eq!(keystroke_to_bytes(&end, mode), Some(b"\x1b[F".to_vec()));

        let pageup = Keystroke::parse("pageup").unwrap();
        assert_eq!(keystroke_to_bytes(&pageup, mode), Some(b"\x1b[5~".to_vec()));

        let pagedown = Keystroke::parse("pagedown").unwrap();
        assert_eq!(
            keystroke_to_bytes(&pagedown, mode),
            Some(b"\x1b[6~".to_vec())
        );

        let insert = Keystroke::parse("insert").unwrap();
        assert_eq!(keystroke_to_bytes(&insert, mode), Some(b"\x1b[2~".to_vec()));

        let delete = Keystroke::parse("delete").unwrap();
        assert_eq!(keystroke_to_bytes(&delete, mode), Some(b"\x1b[3~".to_vec()));
    }

    #[test]
    fn test_function_keys() {
        let mode = TermMode::empty();

        let f1 = Keystroke::parse("f1").unwrap();
        assert_eq!(keystroke_to_bytes(&f1, mode), Some(b"\x1bOP".to_vec()));

        let f2 = Keystroke::parse("f2").unwrap();
        assert_eq!(keystroke_to_bytes(&f2, mode), Some(b"\x1bOQ".to_vec()));

        let f5 = Keystroke::parse("f5").unwrap();
        assert_eq!(keystroke_to_bytes(&f5, mode), Some(b"\x1b[15~".to_vec()));

        let f12 = Keystroke::parse("f12").unwrap();
        assert_eq!(keystroke_to_bytes(&f12, mode), Some(b"\x1b[24~".to_vec()));
    }

    #[test]
    fn test_ctrl_combinations() {
        let mode = TermMode::empty();

        // Ctrl+A = 0x01
        let ctrl_a = Keystroke::parse("ctrl-a").unwrap();
        assert_eq!(keystroke_to_bytes(&ctrl_a, mode), Some(vec![0x01]));

        // Ctrl+C = 0x03
        let ctrl_c = Keystroke::parse("ctrl-c").unwrap();
        assert_eq!(keystroke_to_bytes(&ctrl_c, mode), Some(vec![0x03]));

        // Ctrl+Z = 0x1a
        let ctrl_z = Keystroke::parse("ctrl-z").unwrap();
        assert_eq!(keystroke_to_bytes(&ctrl_z, mode), Some(vec![0x1a]));

        // Ctrl+Space = 0x00
        let ctrl_space = Keystroke::parse("ctrl-space").unwrap();
        assert_eq!(keystroke_to_bytes(&ctrl_space, mode), Some(vec![0x00]));
    }

    #[test]
    fn test_alt_combinations() {
        let mode = TermMode::empty();

        // Alt+a sends ESC followed by 'a'
        let alt_a = Keystroke::parse("alt-a").unwrap();
        assert_eq!(keystroke_to_bytes(&alt_a, mode), Some(b"\x1ba".to_vec()));

        // Alt+x sends ESC followed by 'x'
        let alt_x = Keystroke::parse("alt-x").unwrap();
        assert_eq!(keystroke_to_bytes(&alt_x, mode), Some(b"\x1bx".to_vec()));
    }

    #[test]
    fn test_regular_characters() {
        let mode = TermMode::empty();

        let a = Keystroke::parse("a").unwrap();
        assert_eq!(keystroke_to_bytes(&a, mode), Some(b"a".to_vec()));

        let z = Keystroke::parse("z").unwrap();
        assert_eq!(keystroke_to_bytes(&z, mode), Some(b"z".to_vec()));

        let zero = Keystroke::parse("0").unwrap();
        assert_eq!(keystroke_to_bytes(&zero, mode), Some(b"0".to_vec()));
    }

    #[test]
    fn test_space_key() {
        let mode = TermMode::empty();

        let space = Keystroke::parse("space").unwrap();
        assert_eq!(keystroke_to_bytes(&space, mode), Some(b" ".to_vec()));
    }

    #[test]
    fn test_modified_arrow_keys() {
        let mode = TermMode::empty();

        // Shift: mod_code = 1 + 1 = 2
        let shift_up = Keystroke::parse("shift-up").unwrap();
        assert_eq!(keystroke_to_bytes(&shift_up, mode), Some(b"\x1b[1;2A".to_vec()));

        // Alt: mod_code = 1 + 2 = 3
        let alt_down = Keystroke::parse("alt-down").unwrap();
        assert_eq!(keystroke_to_bytes(&alt_down, mode), Some(b"\x1b[1;3B".to_vec()));

        // Ctrl: mod_code = 1 + 4 = 5
        let ctrl_left = Keystroke::parse("ctrl-left").unwrap();
        assert_eq!(keystroke_to_bytes(&ctrl_left, mode), Some(b"\x1b[1;5D".to_vec()));

        let ctrl_right = Keystroke::parse("ctrl-right").unwrap();
        assert_eq!(keystroke_to_bytes(&ctrl_right, mode), Some(b"\x1b[1;5C".to_vec()));

        // Ctrl+Shift: mod_code = 1 + 1 + 4 = 6
        let ctrl_shift_right = Keystroke::parse("ctrl-shift-right").unwrap();
        assert_eq!(keystroke_to_bytes(&ctrl_shift_right, mode), Some(b"\x1b[1;6C".to_vec()));
    }

    #[test]
    fn test_modified_navigation_and_fn_keys() {
        let mode = TermMode::empty();

        // Ctrl-Home
        let ctrl_home = Keystroke::parse("ctrl-home").unwrap();
        assert_eq!(keystroke_to_bytes(&ctrl_home, mode), Some(b"\x1b[1;5H".to_vec()));

        // Shift-F1
        let shift_f1 = Keystroke::parse("shift-f1").unwrap();
        assert_eq!(keystroke_to_bytes(&shift_f1, mode), Some(b"\x1b[1;2P".to_vec()));

        // Ctrl-F5
        let ctrl_f5 = Keystroke::parse("ctrl-f5").unwrap();
        assert_eq!(keystroke_to_bytes(&ctrl_f5, mode), Some(b"\x1b[15;5~".to_vec()));
    }

    #[test]
    fn test_shifted_symbols_fallback() {
        let mode = TermMode::empty();

        let shift_1 = Keystroke::parse("shift-1").unwrap();
        assert_eq!(keystroke_to_bytes(&shift_1, mode), Some(b"!".to_vec()));

        let shift_dash = Keystroke::parse("shift--").unwrap();
        assert_eq!(keystroke_to_bytes(&shift_dash, mode), Some(b"_".to_vec()));

        let shift_slash = Keystroke::parse("shift-/").unwrap();
        assert_eq!(keystroke_to_bytes(&shift_slash, mode), Some(b"?".to_vec()));
    }
}

