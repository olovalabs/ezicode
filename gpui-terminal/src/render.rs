//! Terminal rendering module.
//!
//! This module provides [`TerminalRenderer`], which handles efficient rendering of
//! terminal content using GPUI's text and drawing systems.
//!
//! # Rendering Pipeline
//!
//! The renderer processes the terminal grid in several stages:
//!
//! ```text
//! Terminal Grid → Layout Phase → Paint Phase
//!                      │              │
//!                      ├─ Collect backgrounds
//!                      ├─ Batch text runs
//!                      │              │
//!                      │              ├─ Paint default background
//!                      │              ├─ Paint non-default backgrounds
//!                      │              ├─ Paint text characters
//!                      │              └─ Paint cursor
//! ```
//!
//! # Optimizations
//!
//! The renderer includes several optimizations to minimize draw calls:
//!
//! 1. **Background Merging**: Adjacent cells with the same background color are
//!    merged into single rectangles, reducing the number of quads to paint.
//!
//! 2. **Text Batching**: Adjacent cells with identical styling (color, bold, italic)
//!    are grouped into [`BatchedTextRun`]s for efficient text shaping.
//!
//! 3. **Default Background Skip**: Cells with the default background color don't
//!    generate separate background rectangles.
//!
//! 4. **Cell Measurement**: Font metrics are measured once using the 'M' character
//!    and cached for consistent cell dimensions.
//!
//! # Cell Dimensions
//!
//! Cell size is calculated from actual font metrics:
//!
//! - **Width**: Measured from shaped 'M' character (typically widest in monospace)
//! - **Height**: `(ascent + descent) × line_height_multiplier`
//!
//! The `line_height_multiplier` (default 1.2) adds extra vertical space to
//! accommodate tall glyphs from nerd fonts and other icon fonts.
//!
//! # Example
//!
//! ```ignore
//! use gpui::px;
//! use gpui_terminal::{ColorPalette, TerminalRenderer};
//!
//! let renderer = TerminalRenderer::new(
//!     "JetBrains Mono".to_string(),
//!     px(14.0),
//!     1.2,  // line height multiplier
//!     ColorPalette::default(),
//! );
//! ```

use crate::colors::ColorPalette;
use crate::event::GpuiEventProxy;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::Color;
use gpui::{
    App, Bounds, Edges, Font, FontFeatures, FontStyle, FontWeight, Hsla, Pixels, Point, Size,
    SharedString, StrikethroughStyle, TextRun, UnderlineStyle, Window, px, quad,
    transparent_black,
};

/// A batched run of text with consistent styling.
///
/// This struct groups adjacent terminal cells with identical visual attributes
/// to reduce the number of text rendering calls.
#[derive(Debug, Clone)]
pub struct BatchedTextRun {
    /// The text content to render
    pub text: String,

    /// Number of grid cells this text run spans horizontally
    pub cell_count: usize,

    /// Starting column position
    pub start_col: usize,

    /// Row position
    pub row: usize,

    /// Foreground color
    pub fg_color: Hsla,

    /// Background color
    pub bg_color: Hsla,

    /// Bold flag
    pub bold: bool,

    /// Italic flag
    pub italic: bool,

    /// Underline flag
    pub underline: bool,

    /// Strikethrough flag
    pub strikethrough: bool,
}

/// Background rectangle to paint.
///
/// Represents a rectangular region with a solid color background.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundRect {
    /// Starting column position
    pub start_col: usize,

    /// Ending column position (exclusive)
    pub end_col: usize,

    /// Starting row position
    pub row: usize,

    /// Ending row position (exclusive, supports 2D merged regions)
    pub end_row: usize,

    /// Background color
    pub color: Hsla,
}

impl BackgroundRect {
    /// Create a new single-row background rectangle
    pub fn new(start_col: usize, end_col: usize, row: usize, color: Hsla) -> Self {
        Self {
            start_col,
            end_col,
            row,
            end_row: row + 1,
            color,
        }
    }

    /// Check if this rectangle can be merged horizontally with another on the same row.
    pub fn can_merge_with(&self, other: &Self) -> bool {
        self.row == other.row
            && self.end_row == other.end_row
            && self.color == other.color
            && self.end_col == other.start_col
    }
}

/// A subcell vector rectangle for rendering box drawing and block elements
#[derive(Debug, Clone, PartialEq)]
pub struct BlockElementRect {
    pub bounds: Bounds<Pixels>,
    pub color: Hsla,
}

/// Check if a character is a box drawing or block element character
pub fn is_box_or_block(ch: char) -> bool {
    matches!(ch as u32, 0x2500..=0x257F | 0x2580..=0x259F)
}

/// Check if a character is a decorative or powerline symbol that should preserve exact colors
pub fn is_decorative_symbol(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2500..=0x257F       // Box Drawing
        | 0x2580..=0x259F     // Block Elements
        | 0x25A0..=0x25FF     // Geometric Shapes
        | 0x1FB00..=0x1FB3B   // Sextants
        | 0xE0B0..=0xE0D7     // Powerline symbols (triangles, chevrons, curves)
    )
}

/// Rasterize a box drawing or block element character directly into crisp subcell vector quads.
///
/// This eliminates font line-height gaps and anti-aliasing seams so TUI borders, graphs,
/// and progress bars in tools like lazygit, htop, btop, and opencode render seamlessly.
pub fn rasterize_box_or_block(
    ch: char,
    col: usize,
    row: usize,
    color: Hsla,
    origin: Point<Pixels>,
    cell_width: Pixels,
    cell_height: Pixels,
) -> Option<Vec<BlockElementRect>> {
    let cx = origin.x + cell_width * (col as f32);
    let cy = origin.y + cell_height * (row as f32);
    let w: f32 = cell_width.into();
    let h: f32 = cell_height.into();

    let rect = |x_rel: f32, y_rel: f32, w_rel: f32, h_rel: f32| BlockElementRect {
        bounds: Bounds {
            origin: Point {
                x: cx + px(x_rel),
                y: cy + px(y_rel),
            },
            size: Size {
                width: px(w_rel),
                height: px(h_rel),
            },
        },
        color,
    };

    let code = ch as u32;

    // 1. Block Elements (0x2580..=0x259F)
    match code {
        0x2580 => return Some(vec![rect(0.0, 0.0, w, h * 0.5)]),
        0x2581 => return Some(vec![rect(0.0, h * 0.875, w, h * 0.125)]),
        0x2582 => return Some(vec![rect(0.0, h * 0.75, w, h * 0.25)]),
        0x2583 => return Some(vec![rect(0.0, h * 0.625, w, h * 0.375)]),
        0x2584 => return Some(vec![rect(0.0, h * 0.5, w, h * 0.5)]),
        0x2585 => return Some(vec![rect(0.0, h * 0.375, w, h * 0.625)]),
        0x2586 => return Some(vec![rect(0.0, h * 0.25, w, h * 0.75)]),
        0x2587 => return Some(vec![rect(0.0, h * 0.125, w, h * 0.875)]),
        0x2588 => return Some(vec![rect(0.0, 0.0, w, h)]),
        0x2589 => return Some(vec![rect(0.0, 0.0, w * 0.875, h)]),
        0x258A => return Some(vec![rect(0.0, 0.0, w * 0.75, h)]),
        0x258B => return Some(vec![rect(0.0, 0.0, w * 0.625, h)]),
        0x258C => return Some(vec![rect(0.0, 0.0, w * 0.5, h)]),
        0x258D => return Some(vec![rect(0.0, 0.0, w * 0.375, h)]),
        0x258E => return Some(vec![rect(0.0, 0.0, w * 0.25, h)]),
        0x258F => return Some(vec![rect(0.0, 0.0, w * 0.125, h)]),
        0x2590 => return Some(vec![rect(w * 0.5, 0.0, w * 0.5, h)]),
        0x2591 => return Some(vec![BlockElementRect {
            bounds: Bounds { origin: Point { x: cx, y: cy }, size: Size { width: px(w), height: px(h) } },
            color: Hsla { a: color.a * 0.25, ..color },
        }]),
        0x2592 => return Some(vec![BlockElementRect {
            bounds: Bounds { origin: Point { x: cx, y: cy }, size: Size { width: px(w), height: px(h) } },
            color: Hsla { a: color.a * 0.50, ..color },
        }]),
        0x2593 => return Some(vec![BlockElementRect {
            bounds: Bounds { origin: Point { x: cx, y: cy }, size: Size { width: px(w), height: px(h) } },
            color: Hsla { a: color.a * 0.75, ..color },
        }]),
        0x2594 => return Some(vec![rect(0.0, 0.0, w, h * 0.125)]),
        0x2595 => return Some(vec![rect(w * 0.875, 0.0, w * 0.125, h)]),
        0x2596 => return Some(vec![rect(0.0, h * 0.5, w * 0.5, h * 0.5)]),
        0x2597 => return Some(vec![rect(w * 0.5, h * 0.5, w * 0.5, h * 0.5)]),
        0x2598 => return Some(vec![rect(0.0, 0.0, w * 0.5, h * 0.5)]),
        0x2599 => return Some(vec![rect(0.0, 0.0, w * 0.5, h), rect(w * 0.5, h * 0.5, w * 0.5, h * 0.5)]),
        0x259A => return Some(vec![rect(0.0, 0.0, w * 0.5, h * 0.5), rect(w * 0.5, h * 0.5, w * 0.5, h * 0.5)]),
        0x259B => return Some(vec![rect(0.0, 0.0, w, h * 0.5), rect(0.0, h * 0.5, w * 0.5, h * 0.5)]),
        0x259C => return Some(vec![rect(0.0, 0.0, w, h * 0.5), rect(w * 0.5, h * 0.5, w * 0.5, h * 0.5)]),
        0x259D => return Some(vec![rect(w * 0.5, 0.0, w * 0.5, h * 0.5)]),
        0x259E => return Some(vec![rect(w * 0.5, 0.0, w * 0.5, h * 0.5), rect(0.0, h * 0.5, w * 0.5, h * 0.5)]),
        0x259F => return Some(vec![rect(0.0, h * 0.5, w, h * 0.5), rect(w * 0.5, 0.0, w * 0.5, h * 0.5)]),
        _ => {}
    }

    // 2. Box Drawing Lines (0x2500..=0x257F)
    let t = (h * 0.08).max(1.0).round();
    let ht = (h * 0.16).max(2.0).round();
    let mx = (w - t) * 0.5;
    let my = (h - t) * 0.5;
    let hmx = (w - ht) * 0.5;
    let hmy = (h - ht) * 0.5;

    match code {
        // Light / regular lines
        0x2500 | 0x2504 | 0x2508 => Some(vec![rect(0.0, my, w, t)]), // ─
        0x2502 | 0x2506 | 0x250A => Some(vec![rect(mx, 0.0, t, h)]), // │
        0x250C | 0x256D => Some(vec![rect(mx, my, w - mx, t), rect(mx, my, t, h - my)]), // ┌ / ╭
        0x2510 | 0x256E => Some(vec![rect(0.0, my, mx + t, t), rect(mx, my, t, h - my)]), // ┐ / ╮
        0x2514 | 0x2570 => Some(vec![rect(mx, my, w - mx, t), rect(mx, 0.0, t, my + t)]), // └ / ╰
        0x2518 | 0x256F => Some(vec![rect(0.0, my, mx + t, t), rect(mx, 0.0, t, my + t)]), // ┘ / ╯
        0x251C => Some(vec![rect(mx, 0.0, t, h), rect(mx, my, w - mx, t)]), // ├
        0x2524 => Some(vec![rect(mx, 0.0, t, h), rect(0.0, my, mx + t, t)]), // ┤
        0x252C => Some(vec![rect(0.0, my, w, t), rect(mx, my, t, h - my)]), // ┬
        0x2534 => Some(vec![rect(0.0, my, w, t), rect(mx, 0.0, t, my + t)]), // ┴
        0x253C => Some(vec![rect(0.0, my, w, t), rect(mx, 0.0, t, h)]),     // ┼

        // Heavy lines
        0x2501 | 0x2505 | 0x2509 => Some(vec![rect(0.0, hmy, w, ht)]), // ━
        0x2503 | 0x2507 | 0x250B => Some(vec![rect(hmx, 0.0, ht, h)]), // ┃
        0x250F => Some(vec![rect(hmx, hmy, w - hmx, ht), rect(hmx, hmy, ht, h - hmy)]), // ┏
        0x2513 => Some(vec![rect(0.0, hmy, hmx + ht, ht), rect(hmx, hmy, ht, h - hmy)]), // ┓
        0x2517 => Some(vec![rect(hmx, hmy, w - hmx, ht), rect(hmx, 0.0, ht, hmy + ht)]), // ┗
        0x251B => Some(vec![rect(0.0, hmy, hmx + ht, ht), rect(hmx, 0.0, ht, hmy + ht)]), // ┛
        0x2523 => Some(vec![rect(hmx, 0.0, ht, h), rect(hmx, hmy, w - hmx, ht)]), // ┣
        0x252B => Some(vec![rect(hmx, 0.0, ht, h), rect(0.0, hmy, hmx + ht, ht)]), // ┫
        0x2533 => Some(vec![rect(0.0, hmy, w, ht), rect(hmx, hmy, ht, h - hmy)]), // ┳
        0x253B => Some(vec![rect(0.0, hmy, w, ht), rect(hmx, 0.0, ht, hmy + ht)]), // ┻
        0x254B => Some(vec![rect(0.0, hmy, w, ht), rect(hmx, 0.0, ht, h)]),     // ╋

        // Half lines / single-ended lines
        0x2574 => Some(vec![rect(0.0, my, mx + t, t)]),              // ╴
        0x2575 => Some(vec![rect(mx, 0.0, t, my + t)]),              // ╵
        0x2576 => Some(vec![rect(mx, my, w - mx, t)]),              // ╶
        0x2577 => Some(vec![rect(mx, my, t, h - my)]),              // ╷
        0x2578 => Some(vec![rect(0.0, hmy, hmx + ht, ht)]),         // ╸
        0x2579 => Some(vec![rect(hmx, 0.0, ht, hmy + ht)]),         // ╹
        0x257A => Some(vec![rect(hmx, hmy, w - hmx, ht)]),         // ╺
        0x257B => Some(vec![rect(hmx, hmy, ht, h - hmy)]),         // ╻

        // Double lines
        0x2550 => {
            let offset = (t * 1.3).max(2.0);
            Some(vec![rect(0.0, my - offset, w, t), rect(0.0, my + offset, w, t)])
        } // ═
        0x2551 => {
            let offset = (t * 1.3).max(2.0);
            Some(vec![rect(mx - offset, 0.0, t, h), rect(mx + offset, 0.0, t, h)])
        } // ║
        0x2554 => {
            let offset = (t * 1.3).max(2.0);
            Some(vec![
                rect(mx - offset, my - offset, w - (mx - offset), t),
                rect(mx + offset, my + offset, w - (mx + offset), t),
                rect(mx - offset, my - offset, t, h - (my - offset)),
                rect(mx + offset, my + offset, t, h - (my + offset)),
            ])
        } // ╔
        0x2557 => {
            let offset = (t * 1.3).max(2.0);
            Some(vec![
                rect(0.0, my - offset, mx + offset + t, t),
                rect(0.0, my + offset, mx - offset + t, t),
                rect(mx + offset, my - offset, t, h - (my - offset)),
                rect(mx - offset, my + offset, t, h - (my + offset)),
            ])
        } // ╗
        0x255A => {
            let offset = (t * 1.3).max(2.0);
            Some(vec![
                rect(mx - offset, my + offset, w - (mx - offset), t),
                rect(mx + offset, my - offset, w - (mx + offset), t),
                rect(mx - offset, 0.0, t, my + offset + t),
                rect(mx + offset, 0.0, t, my - offset + t),
            ])
        } // ╚
        0x255D => {
            let offset = (t * 1.3).max(2.0);
            Some(vec![
                rect(0.0, my + offset, mx + offset + t, t),
                rect(0.0, my - offset, mx - offset + t, t),
                rect(mx + offset, 0.0, t, my + offset + t),
                rect(mx - offset, 0.0, t, my - offset + t),
            ])
        } // ╝
        0x2560 => {
            let offset = (t * 1.3).max(2.0);
            Some(vec![
                rect(mx - offset, 0.0, t, h),
                rect(mx + offset, 0.0, t, h),
                rect(mx + offset, my - offset, w - (mx + offset), t),
                rect(mx + offset, my + offset, w - (mx + offset), t),
            ])
        } // ╠
        0x2563 => {
            let offset = (t * 1.3).max(2.0);
            Some(vec![
                rect(mx - offset, 0.0, t, h),
                rect(mx + offset, 0.0, t, h),
                rect(0.0, my - offset, mx - offset + t, t),
                rect(0.0, my + offset, mx - offset + t, t),
            ])
        } // ╣
        0x2566 => {
            let offset = (t * 1.3).max(2.0);
            Some(vec![
                rect(0.0, my - offset, w, t),
                rect(0.0, my + offset, w, t),
                rect(mx - offset, my + offset, t, h - (my + offset)),
                rect(mx + offset, my + offset, t, h - (my + offset)),
            ])
        } // ╦
        0x2569 => {
            let offset = (t * 1.3).max(2.0);
            Some(vec![
                rect(0.0, my - offset, w, t),
                rect(0.0, my + offset, w, t),
                rect(mx - offset, 0.0, t, my - offset + t),
                rect(mx + offset, 0.0, t, my - offset + t),
            ])
        } // ╩
        0x256C => {
            let offset = (t * 1.3).max(2.0);
            Some(vec![
                rect(0.0, my - offset, w, t),
                rect(0.0, my + offset, w, t),
                rect(mx - offset, 0.0, t, h),
                rect(mx + offset, 0.0, t, h),
            ])
        } // ╬

        _ => None,
    }
}

/// Terminal renderer with font settings and cell dimensions.
///
/// This struct manages the rendering of terminal content, including text,
/// backgrounds, and cursor. It maintains font metrics and provides the
/// [`paint`](Self::paint) method for drawing the terminal grid.
///
/// # Font Metrics
///
/// Cell dimensions are calculated from actual font measurements via
/// [`measure_cell`](Self::measure_cell). This ensures accurate character
/// positioning regardless of the font used.
///
/// # Usage
///
/// The renderer is typically used internally by [`TerminalView`](crate::TerminalView),
/// but can also be used directly for custom rendering:
///
/// ```ignore
/// // Measure cell dimensions (call once per font change)
/// renderer.measure_cell(window);
///
/// // Paint the terminal grid
/// renderer.paint(bounds, padding, &term, window, cx);
/// ```
///
/// # Performance
///
/// For optimal performance:
/// - Call `measure_cell` only when font settings change
/// - The `paint` method is designed to be called every frame
/// - Background and text batching minimize GPU draw calls
#[derive(Clone)]
pub struct TerminalRenderer {
    /// Font family name (e.g., "Fira Code", "Menlo")
    pub font_family: String,

    /// Font size in pixels
    pub font_size: Pixels,

    /// Width of a single character cell
    pub cell_width: Pixels,

    /// Height of a single character cell (line height)
    pub cell_height: Pixels,

    /// Multiplier for line height to accommodate tall glyphs
    pub line_height_multiplier: f32,

    /// Color palette for resolving terminal colors
    pub palette: ColorPalette,
}

impl TerminalRenderer {
    /// Creates a new terminal renderer with the given font settings and color palette.
    ///
    /// # Arguments
    ///
    /// * `font_family` - The name of the font family to use
    /// * `font_size` - The font size in pixels
    /// * `line_height_multiplier` - Multiplier for line height (e.g., 1.2 for 20% extra)
    /// * `palette` - The color palette to use for terminal colors
    ///
    /// # Returns
    ///
    /// A new `TerminalRenderer` instance with default cell dimensions.
    ///
    /// # Examples
    ///
    /// ```
    /// use gpui::px;
    /// use gpui_terminal::render::TerminalRenderer;
    /// use gpui_terminal::ColorPalette;
    ///
    /// let renderer = TerminalRenderer::new("Fira Code".to_string(), px(14.0), 1.2, ColorPalette::default());
    /// ```
    pub fn new(
        font_family: String,
        font_size: Pixels,
        line_height_multiplier: f32,
        palette: ColorPalette,
    ) -> Self {
        // Default cell dimensions - will be measured on first paint
        // Using 0.6 as approximate em-width ratio for monospace fonts
        let cell_width = font_size * 0.6;
        let cell_height = font_size * 1.4; // Line height with some spacing

        Self {
            font_family,
            font_size,
            cell_width,
            cell_height,
            line_height_multiplier,
            palette,
        }
    }

    /// Measure cell dimensions based on actual font metrics.
    ///
    /// This method measures the actual width and height of characters
    /// using the GPUI text system.
    ///
    /// # Arguments
    ///
    /// * `window` - The GPUI window for text system access
    pub fn measure_cell(&mut self, window: &mut Window) {
        // Measure using a reference character (M is typically the widest)
        let font = Font {
            family: self.font_family.clone().into(),
            features: FontFeatures::default(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        };

        let text_run = TextRun {
            len: 1,
            font,
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };

        // Shape a single 'M' character to get its metrics
        let shaped = window
            .text_system()
            .shape_line("M".into(), self.font_size, &[text_run], None);

        // Get the width from the shaped line (accessed via Deref to LineLayout)
        if shaped.width > px(0.0) {
            self.cell_width = shaped.width;
        }

        // Calculate height from ascent + descent with multiplier for tall glyphs (nerd fonts, etc.)
        let line_height = shaped.ascent + shaped.descent;
        if line_height > px(0.0) {
            self.cell_height = line_height * self.line_height_multiplier;
        }
    }

    /// Layout cells into batched text runs and background rects for a single row.
    ///
    /// This method processes a row of terminal cells and groups adjacent cells
    /// with identical styling into batched runs. It also collects background
    /// rectangles that need to be painted.
    ///
    /// # Arguments
    ///
    /// * `row` - The row number
    /// * `cells` - Iterator over (column, Cell) pairs
    /// * `colors` - Terminal color configuration
    ///
    /// # Returns
    ///
    /// A tuple of `(backgrounds, text_runs)` where:
    /// - `backgrounds` is a vector of merged background rectangles
    /// - `text_runs` is a vector of batched text runs
    pub fn layout_row(
        &self,
        row: usize,
        cells: impl Iterator<Item = (usize, Cell)>,
        colors: &Colors,
    ) -> (Vec<BackgroundRect>, Vec<BatchedTextRun>) {
        let mut backgrounds = Vec::new();
        let mut text_runs = Vec::new();

        let mut current_run: Option<BatchedTextRun> = None;
        let mut current_bg: Option<BackgroundRect> = None;

        for (col, cell) in cells {
            // Skip wide character spacers
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }

            // Extract cell styling
            let mut fg_color = self.palette.resolve(cell.fg, colors);
            let mut bg_color = self.palette.resolve(cell.bg, colors);

            // Handle INVERSE (reverse video) - critical for TUIs (htop, lazygit, vim, nano selected menus)
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg_color, &mut bg_color);
            }

            // Handle DIM
            if cell.flags.contains(Flags::DIM) {
                fg_color.a *= 0.65;
            }

            // Handle HIDDEN
            if cell.flags.contains(Flags::HIDDEN) {
                fg_color = gpui::transparent_black();
            }

            let bold = cell.flags.contains(Flags::BOLD);
            let italic = cell.flags.contains(Flags::ITALIC);
            let underline = cell.flags.contains(Flags::UNDERLINE);
            let strikethrough = cell.flags.contains(Flags::STRIKEOUT);

            // Get the character (or space if empty)
            let ch = if cell.c == ' ' || cell.c == '\0' {
                ' '
            } else {
                cell.c
            };

            let mut cell_text = ch.to_string();
            if let Some(zerowidth) = cell.zerowidth() {
                for &zc in zerowidth {
                    cell_text.push(zc);
                }
            }
            let cell_width_units = if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };

            // Handle background rectangles
            if let Some(ref mut bg_rect) = current_bg {
                if bg_rect.color == bg_color && bg_rect.end_col == col {
                    // Extend current background
                    bg_rect.end_col = col + 1;
                } else {
                    // Save current background and start new one
                    backgrounds.push(bg_rect.clone());
                    current_bg = Some(BackgroundRect::new(col, col + 1, row, bg_color));
                }
            } else {
                // Start new background
                current_bg = Some(BackgroundRect::new(col, col + 1, row, bg_color));
            }

            // Handle text runs
            if let Some(ref mut run) = current_run {
                if run.fg_color == fg_color
                    && run.bg_color == bg_color
                    && run.bold == bold
                    && run.italic == italic
                    && run.underline == underline
                    && run.strikethrough == strikethrough
                    && run.start_col + run.cell_count == col
                {
                    // Extend current run
                    run.text.push_str(&cell_text);
                    run.cell_count += cell_width_units;
                } else {
                    // Save current run and start new one
                    text_runs.push(run.clone());
                    current_run = Some(BatchedTextRun {
                        text: cell_text,
                        cell_count: cell_width_units,
                        start_col: col,
                        row,
                        fg_color,
                        bg_color,
                        bold,
                        italic,
                        underline,
                        strikethrough,
                    });
                }
            } else {
                // Start new run
                current_run = Some(BatchedTextRun {
                    text: cell_text,
                    cell_count: cell_width_units,
                    start_col: col,
                    row,
                    fg_color,
                    bg_color,
                    bold,
                    italic,
                    underline,
                    strikethrough,
                });
            }
        }

        // Push final run and background
        if let Some(run) = current_run {
            text_runs.push(run);
        }
        if let Some(bg) = current_bg {
            backgrounds.push(bg);
        }

        // Merge adjacent backgrounds with same color
        let merged_backgrounds = self.merge_backgrounds(backgrounds);

        (merged_backgrounds, text_runs)
    }

    /// Merge adjacent background rects with same color horizontally on a single row.
    pub fn merge_backgrounds(&self, mut rects: Vec<BackgroundRect>) -> Vec<BackgroundRect> {
        if rects.is_empty() {
            return rects;
        }

        let mut merged = Vec::new();
        let mut current = rects.remove(0);

        for rect in rects {
            if current.can_merge_with(&rect) {
                current.end_col = rect.end_col;
            } else {
                merged.push(current);
                current = rect;
            }
        }

        merged.push(current);
        merged
    }

    /// Merge background rects in 2D across both columns and rows.
    ///
    /// This optimization reduces GPU quad count significantly by combining
    /// vertically adjacent rows that share the same horizontal span and background color.
    pub fn merge_backgrounds_2d(rects: Vec<BackgroundRect>) -> Vec<BackgroundRect> {
        if rects.is_empty() {
            return rects;
        }
        let mut merged: Vec<BackgroundRect> = Vec::with_capacity(rects.len());
        for rect in rects {
            if let Some(existing) = merged.iter_mut().find(|r| {
                r.color == rect.color
                    && r.start_col == rect.start_col
                    && r.end_col == rect.end_col
                    && r.end_row == rect.row
            }) {
                existing.end_row = rect.end_row;
            } else {
                merged.push(rect);
            }
        }
        merged
    }

    /// Paint terminal content to the window.
    ///
    /// This is the high-performance rendering method that draws:
    /// 1. Full window default background quad.
    /// 2. 2D merged background quads (drastically reducing draw calls).
    /// 3. Pixel-perfect subcell vector quads for Box-Drawing and Block Elements
    ///    (ensuring seamless, zero-gap TUI borders in lazygit, btop, htop, etc.).
    /// 4. Batched text runs shaped in grouped chunks using GPUI's monospace
    ///    column width hint `Some(cell_width)`.
    /// 5. Cursor quad with display offset awareness.
    /// 6. Dynamic scrollbar indicator.
    pub fn paint(
        &self,
        bounds: Bounds<Pixels>,
        padding: Edges<Pixels>,
        term: &Term<GpuiEventProxy>,
        selection: Option<&crate::mouse::Selection>,
        is_focused: bool,
        window: &mut Window,
        _cx: &mut App,
    ) {
        // Get terminal dimensions
        let grid = term.grid();
        let num_lines = grid.screen_lines();
        let num_cols = grid.columns();
        let colors = term.colors();

        // Calculate default background color
        let default_bg = self.palette.resolve(
            Color::Named(alacritty_terminal::vte::ansi::NamedColor::Background),
            colors,
        );

        // Paint default background (covers full bounds including padding)
        window.paint_quad(quad(
            bounds,
            px(0.0),
            default_bg,
            Edges::<Pixels>::default(),
            transparent_black(),
            Default::default(),
        ));

        // Calculate origin offset (content starts after padding)
        let origin = Point {
            x: bounds.origin.x + padding.left,
            y: bounds.origin.y + padding.top,
        };
        // In alternate screen mode (TUIs like vim, lazygit, htop, btop),
        // there is no scrollback history — the display offset must be 0.
        let display_offset = if term.mode().contains(TermMode::ALT_SCREEN) {
            0
        } else {
            grid.display_offset()
        };

        let mut row_backgrounds: Vec<BackgroundRect> = Vec::new();
        let mut batched_text_runs: Vec<BatchedTextRun> = Vec::new();
        let mut block_rects: Vec<BlockElementRect> = Vec::new();

        // Pass 1: Iterate over visible rows
        for line_idx in 0..num_lines {
            let buffer_line = (line_idx as i32) - (display_offset as i32);
            let mut current_bg: Option<BackgroundRect> = None;
            let mut current_run: Option<BatchedTextRun> = None;

            for col_idx in 0..num_cols {
                let cell = &grid[alacritty_terminal::index::Line(buffer_line)]
                    [alacritty_terminal::index::Column(col_idx)];
                let ch = if cell.c == ' ' || cell.c == '\0' {
                    ' '
                } else {
                    cell.c
                };

                let mut fg_color = self.palette.resolve(cell.fg, colors);
                let mut bg_color = self.palette.resolve(cell.bg, colors);

                // Handle INVERSE (reverse video) - critical for TUIs (htop, lazygit, vim, nano selected menus)
                if cell.flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg_color, &mut bg_color);
                }

                // Handle DIM
                if cell.flags.contains(Flags::DIM) {
                    fg_color.a *= 0.65;
                }

                // Handle HIDDEN
                if cell.flags.contains(Flags::HIDDEN) {
                    fg_color = gpui::transparent_black();
                }

                let bold = cell.flags.contains(Flags::BOLD);
                let italic = cell.flags.contains(Flags::ITALIC);
                let underline = cell.flags.contains(Flags::UNDERLINE);
                let strikethrough = cell.flags.contains(Flags::STRIKEOUT);

                // 1. Background batching
                if bg_color != default_bg {
                    if let Some(ref mut bg) = current_bg {
                        if bg.color == bg_color && bg.end_col == col_idx {
                            bg.end_col = col_idx + 1;
                        } else {
                            row_backgrounds.push(bg.clone());
                            current_bg = Some(BackgroundRect::new(
                                col_idx,
                                col_idx + 1,
                                line_idx,
                                bg_color,
                            ));
                        }
                    } else {
                        current_bg = Some(BackgroundRect::new(
                            col_idx,
                            col_idx + 1,
                            line_idx,
                            bg_color,
                        ));
                    }
                } else if let Some(bg) = current_bg.take() {
                    row_backgrounds.push(bg);
                }

                // Skip wide character spacers (their background was already accounted for above)
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }

                // 2. Pixel-perfect procedural rasterization for Box-Drawing & Block Elements
                if let Some(quads) = rasterize_box_or_block(
                    ch,
                    col_idx,
                    line_idx,
                    fg_color,
                    origin,
                    self.cell_width,
                    self.cell_height,
                ) {
                    block_rects.extend(quads);
                    if let Some(run) = current_run.take() {
                        batched_text_runs.push(run);
                    }
                    continue;
                }

                // Skip true blank cells (empty default spaces that have no styling or inverse)
                let is_blank = ch == ' '
                    && bg_color == default_bg
                    && !underline
                    && !strikethrough
                    && !cell.flags.contains(Flags::INVERSE);

                if is_blank {
                    if let Some(run) = current_run.take() {
                        batched_text_runs.push(run);
                    }
                    continue;
                }

                // 3. Regular text batching (combines consecutive same-styled characters)
                let mut cell_text = ch.to_string();
                if let Some(zerowidth) = cell.zerowidth() {
                    for &zc in zerowidth {
                        cell_text.push(zc);
                    }
                }
                let is_wide = cell.flags.contains(Flags::WIDE_CHAR);
                let has_zerowidth = cell.zerowidth().is_some();
                let cell_width_units = if is_wide { 2 } else { 1 };

                // Wide characters (2 cells) and combining characters cannot be batched
                // with single-width ASCII characters because GPUI's monospace `force_width`
                // spaces each glyph at 1 column width, which breaks alignment for all
                // following characters on the line. Isolate them into their own run.
                if is_wide || has_zerowidth {
                    if let Some(run) = current_run.take() {
                        batched_text_runs.push(run);
                    }
                    batched_text_runs.push(BatchedTextRun {
                        text: cell_text,
                        cell_count: cell_width_units,
                        start_col: col_idx,
                        row: line_idx,
                        fg_color,
                        bg_color,
                        bold,
                        italic,
                        underline,
                        strikethrough,
                    });
                    continue;
                }

                if let Some(ref mut run) = current_run {
                    if run.fg_color == fg_color
                        && run.bold == bold
                        && run.italic == italic
                        && run.underline == underline
                        && run.strikethrough == strikethrough
                        && run.start_col + run.cell_count == col_idx
                    {
                        run.text.push_str(&cell_text);
                        run.cell_count += cell_width_units;
                    } else {
                        batched_text_runs.push(run.clone());
                        current_run = Some(BatchedTextRun {
                            text: cell_text,
                            cell_count: cell_width_units,
                            start_col: col_idx,
                            row: line_idx,
                            fg_color,
                            bg_color,
                            bold,
                            italic,
                            underline,
                            strikethrough,
                        });
                    }
                } else {
                    current_run = Some(BatchedTextRun {
                        text: cell_text,
                        cell_count: cell_width_units,
                        start_col: col_idx,
                        row: line_idx,
                        fg_color,
                        bg_color,
                        bold,
                        italic,
                        underline,
                        strikethrough,
                    });
                }
            }

            if let Some(bg) = current_bg {
                row_backgrounds.push(bg);
            }
            if let Some(run) = current_run {
                batched_text_runs.push(run);
            }
        }

        // Pass 2: 2D Background Quad Merging
        let merged_backgrounds = Self::merge_backgrounds_2d(row_backgrounds);

        // 1. Paint 2D merged background rectangles
        for bg in merged_backgrounds {
            let x = origin.x + self.cell_width * (bg.start_col as f32);
            let y = origin.y + self.cell_height * (bg.row as f32);
            let width = self.cell_width * ((bg.end_col - bg.start_col) as f32);
            let height = self.cell_height * ((bg.end_row - bg.row) as f32);

            window.paint_quad(quad(
                Bounds {
                    origin: Point { x, y },
                    size: Size { width, height },
                },
                px(0.0),
                bg.color,
                Edges::<Pixels>::default(),
                transparent_black(),
                Default::default(),
            ));
        }

        // 2. Paint pixel-perfect vector box and block quads (seamless borders!)
        for block in block_rects {
            window.paint_quad(quad(
                block.bounds,
                px(0.0),
                block.color,
                Edges::<Pixels>::default(),
                transparent_black(),
                Default::default(),
            ));
        }

        // 2.5 Paint selection highlight quads if text selection is active
        if let Some(sel) = selection {
            let (sel_start, sel_end) = if sel.start <= sel.end {
                (sel.start, sel.end)
            } else {
                (sel.end, sel.start)
            };

            let sel_color = gpui::hsla(215.0 / 360.0, 0.85, 0.45, 0.35);

            for r in sel_start.line.0..=sel_end.line.0 {
                if r >= 0 && (r as usize) < num_lines {
                    let start_col = if r == sel_start.line.0 {
                        sel_start.column.0
                    } else {
                        0
                    };
                    let end_col = if r == sel_end.line.0 {
                        (sel_end.column.0 + 1).min(num_cols)
                    } else {
                        num_cols
                    };

                    if end_col > start_col {
                        let sel_x = origin.x + self.cell_width * (start_col as f32);
                        let sel_y = origin.y + self.cell_height * (r as f32);
                        let sel_w = self.cell_width * ((end_col - start_col) as f32);

                        window.paint_quad(quad(
                            Bounds {
                                origin: Point { x: sel_x, y: sel_y },
                                size: Size { width: sel_w, height: self.cell_height },
                            },
                            px(0.0),
                            sel_color,
                            Edges::<Pixels>::default(),
                            transparent_black(),
                            Default::default(),
                        ));
                    }
                }
            }
        }

        // 3. Paint batched text runs using monospace cell_width hint (Zed technique).
        // The font family string is converted to a SharedString once and cheaply
        // reference-counted per run, rather than re-cloning + re-interning the
        // family name for every run on every frame.
        let font_family: SharedString = self.font_family.clone().into();
        let font_features = FontFeatures::disable_ligatures();
        for run in batched_text_runs {
            let x = origin.x + self.cell_width * (run.start_col as f32);
            let y = origin.y + self.cell_height * (run.row as f32);

            let font = Font {
                family: font_family.clone(),
                features: font_features.clone(),
                fallbacks: None,
                weight: if run.bold {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                },
                style: if run.italic {
                    FontStyle::Italic
                } else {
                    FontStyle::Normal
                },
            };

            let char_len = run.text.len();
            let text_run = TextRun {
                len: char_len,
                font,
                color: run.fg_color,
                background_color: None,
                underline: if run.underline {
                    Some(UnderlineStyle {
                        thickness: px(1.0),
                        color: Some(run.fg_color),
                        wavy: false,
                    })
                } else {
                    None
                },
                strikethrough: if run.strikethrough {
                    Some(StrikethroughStyle {
                        thickness: px(1.0),
                        color: Some(run.fg_color),
                    })
                } else {
                    None
                },
            };

            // Only use `Some(self.cell_width)` when every character in the run
            // is a single-width glyph matching the column count. Wide characters
            // and combining characters must be allowed to shape at their natural width.
            let force_width = if run.cell_count > 1 || run.text.chars().count() != run.cell_count {
                None
            } else {
                Some(self.cell_width)
            };

            let shaped_line = window.text_system().shape_line(
                run.text.into(),
                self.font_size,
                &[text_run],
                force_width,
            );

            let _ = shaped_line.paint(Point { x, y }, self.cell_height, window, _cx);
        }

        // 4. Paint cursor (only if visible in current viewport scroll position)
        let cursor_screen_line = grid.cursor.point.line.0 + display_offset as i32;
        let show_cursor = term.mode().contains(TermMode::SHOW_CURSOR);
        if show_cursor && cursor_screen_line >= 0 && (cursor_screen_line as usize) < num_lines {
            let cursor_x = origin.x + self.cell_width * (grid.cursor.point.column.0 as f32);
            let cursor_y = origin.y + self.cell_height * (cursor_screen_line as f32);

            let cursor_color = self.palette.resolve(
                Color::Named(alacritty_terminal::vte::ansi::NamedColor::Cursor),
                colors,
            );

            use alacritty_terminal::vte::ansi::CursorShape;
            let cursor_style = term.cursor_style();

            if cursor_style.shape != CursorShape::Hidden {
                if !is_focused || cursor_style.shape == CursorShape::HollowBlock {
                    // Hollow block cursor when unfocused (Zed feature)
                    let cursor_bounds = Bounds {
                        origin: Point { x: cursor_x, y: cursor_y },
                        size: Size { width: self.cell_width, height: self.cell_height },
                    };
                    window.paint_quad(quad(
                        cursor_bounds,
                        px(0.0),
                        transparent_black(),
                        Edges::all(px(1.5)),
                        cursor_color,
                        Default::default(),
                    ));
                } else {
                    let cursor_bounds = match cursor_style.shape {
                        CursorShape::Beam => Bounds {
                            origin: Point { x: cursor_x, y: cursor_y },
                            size: Size { width: px(2.0), height: self.cell_height },
                        },
                        CursorShape::Underline => Bounds {
                            origin: Point {
                                x: cursor_x,
                                y: cursor_y + self.cell_height - px(2.0),
                            },
                            size: Size { width: self.cell_width, height: px(2.0) },
                        },
                        _ => Bounds {
                            origin: Point { x: cursor_x, y: cursor_y },
                            size: Size { width: self.cell_width, height: self.cell_height },
                        },
                    };
                    window.paint_quad(quad(
                        cursor_bounds,
                        px(0.0),
                        cursor_color,
                        Edges::<Pixels>::default(),
                        transparent_black(),
                        Default::default(),
                    ));
                }
            }
        }

        // 5. Paint dynamic scrollbar indicator when there is scrollback history
        let history_size = grid.history_size();
        if history_size > 0 {
            let total_lines = (history_size + num_lines) as f32;
            let available_h: f32 = bounds.size.height.into();
            let thumb_h = (available_h * (num_lines as f32 / total_lines)).max(16.0);
            let scroll_fraction = (history_size - display_offset) as f32 / (history_size as f32);
            let thumb_y = bounds.origin.y + px((available_h - thumb_h) * scroll_fraction);
            let thumb_w = px(4.0);
            let thumb_x = bounds.origin.x + bounds.size.width - thumb_w - px(2.0);

            let thumb_color = if display_offset > 0 {
                gpui::rgba(0xffffff66)
            } else {
                gpui::rgba(0xffffff1a)
            };

            window.paint_quad(quad(
                Bounds {
                    origin: Point { x: thumb_x, y: thumb_y },
                    size: Size { width: thumb_w, height: px(thumb_h) },
                },
                px(2.0),
                thumb_color,
                Edges::<Pixels>::default(),
                transparent_black(),
                Default::default(),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_renderer_creation() {
        let renderer = TerminalRenderer::new(
            "Fira Code".to_string(),
            px(14.0),
            1.2,
            ColorPalette::default(),
        );
        assert_eq!(renderer.font_family, "Fira Code");
        assert_eq!(renderer.font_size, px(14.0));
        assert_eq!(renderer.line_height_multiplier, 1.2);
    }

    #[test]
    fn test_background_rect_merge() {
        let black = Hsla::black();

        let rect1 = BackgroundRect::new(0, 5, 0, black);
        let rect2 = BackgroundRect::new(5, 10, 0, black);

        assert!(rect1.can_merge_with(&rect2));

        let rect3 = BackgroundRect::new(5, 10, 1, black);

        assert!(!rect1.can_merge_with(&rect3));
    }

    #[test]
    fn test_merge_backgrounds() {
        let renderer = TerminalRenderer::new(
            "monospace".to_string(),
            px(14.0),
            1.2,
            ColorPalette::default(),
        );
        let black = Hsla::black();

        let rects = vec![
            BackgroundRect::new(0, 5, 0, black),
            BackgroundRect::new(5, 10, 0, black),
        ];

        let merged = renderer.merge_backgrounds(rects);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].start_col, 0);
        assert_eq!(merged[0].end_col, 10);
    }

    #[test]
    fn test_merge_backgrounds_2d() {
        let black = Hsla::black();
        let rects = vec![
            BackgroundRect::new(0, 10, 0, black),
            BackgroundRect::new(0, 10, 1, black),
            BackgroundRect::new(0, 10, 2, black),
        ];

        let merged = TerminalRenderer::merge_backgrounds_2d(rects);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].start_col, 0);
        assert_eq!(merged[0].end_col, 10);
        assert_eq!(merged[0].row, 0);
        assert_eq!(merged[0].end_row, 3);
    }

    #[test]
    fn test_rasterize_box_and_block() {
        let origin = Point::default();
        let cell_w = px(10.0);
        let cell_h = px(20.0);
        let color = Hsla::black();

        // Full block
        let full = rasterize_box_or_block('█', 0, 0, color, origin, cell_w, cell_h);
        assert!(full.is_some());
        let quads = full.unwrap();
        assert_eq!(quads.len(), 1);
        assert_eq!(quads[0].bounds.size.width, cell_w);
        assert_eq!(quads[0].bounds.size.height, cell_h);

        // Half blocks
        let top_half = rasterize_box_or_block('▀', 0, 0, color, origin, cell_w, cell_h);
        assert!(top_half.is_some());
        let bottom_half = rasterize_box_or_block('▄', 0, 0, color, origin, cell_w, cell_h);
        assert!(bottom_half.is_some());

        // Box drawing
        let h_line = rasterize_box_or_block('─', 0, 0, color, origin, cell_w, cell_h);
        assert!(h_line.is_some());
        let v_line = rasterize_box_or_block('│', 0, 0, color, origin, cell_w, cell_h);
        assert!(v_line.is_some());
        let corner = rasterize_box_or_block('┌', 0, 0, color, origin, cell_w, cell_h);
        assert!(corner.is_some());
    }
}
