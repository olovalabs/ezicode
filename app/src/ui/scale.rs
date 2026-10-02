//! Zed-style UI scaling.
//!
//! Zed sizes its whole interface with one number. `ui_font_size` is the size of
//! UI text *and* the size of `1rem`, and every scalable metric — text, icons,
//! gaps, list rows, tree indents — is expressed in rems. Raising it therefore
//! scales the interface proportionally, the way zooming a browser page does,
//! instead of growing one label while the rows around it stay put.
//!
//! This module is that model on top of the two pieces GPUI already gives us:
//!
//! * `gpui_component::Theme::font_size` is the app's UI font size, and
//!   [`apply_ui_font_size`] is the only thing that writes it. `Root::render`
//!   copies it into `Window::set_rem_size`, so `1rem` *is* `ui_font_size`.
//! * Lengths built with [`rem`] resolve against that size while laying out, so
//!   a change needs no reflow pass of its own: the next frame is simply laid
//!   out with a different rem size.
//!
//! Layout constants stay written as the pixels they occupy at [`UI_FONT_BASE`]
//! — the size the interface was designed at — and pass through [`rem`] on their
//! way into the style. That is the same trick as Zed's `rems_from_px`:
//! `rem(16.0)` is a 16px icon at the design size and a 24px icon when
//! `ui_font_size` is 24, which is why file tree rows, their icons and their
//! indentation stay in step with the text next to them.

use gpui::{px, rems, App, Pixels, Rems, Window};

use crate::settings::{clamp_ui_font_size, DEFAULT_UI_FONT_SIZE};

/// The size the interface was designed at, in pixels.
///
/// Every metric in this crate is a fraction of it, and it is the default
/// `ui_font_size`, so a settings file that never mentions the setting renders
/// exactly the layout it did before the setting existed.
pub(crate) const UI_FONT_BASE: f32 = DEFAULT_UI_FONT_SIZE;

/// A design-time pixel value as a rem length.
///
/// Use this for anything that is *styled* — widths, heights, paddings, gaps,
/// text sizes. GPUI resolves it during layout against the window's rem size, so
/// it scales without the caller knowing anything about the current size.
#[inline]
pub(crate) fn rem(design_px: f32) -> Rems {
    rems(design_px / UI_FONT_BASE)
}

/// The UI font size currently in effect.
#[inline]
pub(crate) fn ui_font_size(cx: &App) -> f32 {
    f32::from(gpui_component::Theme::global(cx).font_size)
}

/// The current size relative to [`UI_FONT_BASE`]: `1.0` when the UI looks
/// exactly as designed.
///
/// For code that *computes* with a size — a scroll delta, how many rows fit in
/// a viewport — rather than styling with it.
#[inline]
pub(crate) fn ui_scale(cx: &App) -> f32 {
    ui_font_size(cx) / UI_FONT_BASE
}

/// A design-time pixel value resolved to pixels right now.
///
/// The [`rem`] equivalent of what layout will compute later, for the places
/// that need a real number instead of a length to hand to a style method.
#[inline]
pub(crate) fn ui_px(design_px: f32, cx: &App) -> Pixels {
    px(design_px * ui_scale(cx))
}

/// Publish `size` (clamped) as the app's UI font size.
///
/// Returns whether anything changed, which lets callers skip the redraw when a
/// step landed on the size already in effect — pressing `+` at the maximum
/// should not repaint the window at all.
pub(crate) fn apply_ui_font_size(size: f32, cx: &mut App) -> bool {
    let size = clamp_ui_font_size(size);
    let theme = gpui_component::Theme::global_mut(cx);
    if theme.font_size == px(size) {
        return false;
    }
    theme.font_size = px(size);
    // Rem sizes are resolved during layout, so a view that was not made dirty
    // would happily reuse the layout it cached at the old size. Refreshing
    // every window re-renders and re-layouts them (Zed's `adjust_ui_font_size`
    // does the same), and gives `Root::render` the chance to pick the new size
    // up into `Window::set_rem_size`. The setting is thus live the moment it is
    // changed; nothing has to be restarted.
    cx.refresh_windows();
    true
}

/// Keep this window's rem size equal to the UI font size.
///
/// `gpui_component::Root::render` already does this for the windows it wraps.
/// Doing it from the workspace's own `render` as well means scaling never
/// depends on a widget-library detail — views drawn without a `Root` (the
/// tests, mostly) behave like the real window — and it costs one comparison per
/// frame.
pub(crate) fn sync_window_rem_size(window: &mut Window, cx: &App) {
    let size = gpui_component::Theme::global(cx).font_size;
    if window.rem_size() != size {
        window.set_rem_size(size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px_of(pixels: Pixels) -> f32 {
        f32::from(pixels)
    }

    /// The base is the identity: at the design size a metric is the number it
    /// was written as, so introducing scaling cannot have moved anything.
    #[test]
    fn design_pixels_are_exact_at_the_base_size() {
        // The explorer's row, icon, text, indent and header heights.
        for design in [2.0, 6.0, 8.0, 12.0, 13.0, 16.0, 22.0, 24.0, 35.0] {
            let resolved = rem(design).to_pixels(px(UI_FONT_BASE));
            assert!(
                (px_of(resolved) - design).abs() < 1e-3,
                "{design}px should stay {design}px at the base size, got {}",
                px_of(resolved)
            );
        }
    }

    /// The whole point of the setting: one factor for text, icons, spacing and
    /// row heights, so the tree grows without losing its alignment.
    #[test]
    fn text_icons_and_rows_scale_by_the_same_factor() {
        let double = px(UI_FONT_BASE * 2.0);
        let row = px_of(rem(22.0).to_pixels(double));
        let icon = px_of(rem(16.0).to_pixels(double));
        let text = px_of(rem(13.0).to_pixels(double));
        let indent = px_of(rem(8.0).to_pixels(double));

        assert!((row - 44.0).abs() < 1e-2, "row: {row}");
        assert!((icon - 32.0).abs() < 1e-2, "icon: {icon}");
        assert!((text - 26.0).abs() < 1e-2, "text: {text}");
        assert!((indent - 16.0).abs() < 1e-2, "indent: {indent}");

        // A nested row is exactly `depth` indents deeper at any size, which is
        // what keeps guide lines and disclosure triangles on the column they
        // belong to when the UI is scaled.
        let depth = 4.0;
        let pad = rem(8.0 + depth * 8.0).to_pixels(double);
        let per_level = rem(8.0).to_pixels(double);
        assert!((px_of(pad) - (8.0 * 2.0 + depth * px_of(per_level))).abs() < 1e-2);
    }

    /// Clamping is the guard rail of the setting: no size, valid or not,
    /// produces a zero or infinite metric.
    #[test]
    fn clamped_sizes_stay_usable() {
        for requested in [0.0, -3.0, f32::NAN, 1e9] {
            let size = clamp_ui_font_size(requested);
            assert!(size > 0.0 && size.is_finite(), "{requested} was not clamped");
            let row = rem(22.0).to_pixels(px(size));
            assert!(px_of(row) > 0.0 && px_of(row).is_finite());
        }
    }
}
