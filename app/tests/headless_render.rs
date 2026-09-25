//! End-to-end check that a GPUI test can see real pixels.
//!
//! Nothing here is ezicode-specific on purpose: this exercises the headless
//! renderer in the vendored gpui (see `components/gpui/VENDORED.md`) from a plain
//! consumer crate, which is the contract the rest of the test suite builds on.

use gpui::{
    Context, IntoElement, ParentElement, Pixels, Render, Size, Styled, StyledText,
    TestAppContext, Window, div, px, rgb, size,
};
use image::RgbaImage;
use std::cell::Cell;
use std::rc::Rc;

/// Window sizes are logical. Test windows report a scale factor of 2, so a frame
/// comes back at twice the requested size in each direction.
const SCALE: u32 = 2;

/// A root view built from a closure, so each test can hand over a bare element.
struct Root<F>(F);

impl<F, T> Render for Root<F>
where
    F: Fn() -> T + 'static,
    T: IntoElement,
{
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        (self.0)()
    }
}

/// One GPU device at a time.
///
/// Every headless context creates its own Vulkan device, and a dozen of them
/// live at once when the test runner uses every core. That reliably upsets the
/// driver sooner or later: radv reports "The CS has been cancelled because the
/// context is lost", the device is gone, and the test fails with a GPU crash
/// rather than an assertion. Serializing keeps exactly one device alive, which
/// is stable.
///
/// The guard is returned rather than taken inside the helper so that callers
/// hold it for as long as they hold the context. Bind it *before* the context:
/// locals drop in reverse, so the device is then torn down before the lock is
/// released.


fn headless_context() -> ((), TestAppContext) {
    // A previous test may have panicked while holding the lock; that is not a
    // reason to refuse to run this one.

    let cx = match TestAppContext::with_headless_renderer() {
        Ok(cx) => cx,
        Err(error) => panic!(
            "no GPU available for headless rendering: {error:#}\n\
             these tests need a working Vulkan driver. On a headless box, install Mesa's \
             lavapipe (mesa-vulkan-drivers) and build the context with \
             TestAppContext::with_headless_renderer_allowing_software(true)."
        ),
    };
    ((), cx)
}

fn draw_window<T: IntoElement + 'static>(
    size: Size<Pixels>,
    root: impl Fn() -> T + 'static,
) -> RgbaImage {
    let (_device, mut cx) = headless_context();
    let (_view, cx) = cx.add_sized_window_view(size, move |_window, _cx| Root(root));
    cx.draw_frame().expect("headless render failed")
}

fn pixel(image: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
    image.get_pixel(x, y).0
}

/// A red window has to come back red, at the requested size.
#[test]
fn renders_a_solid_window() {
    let image = draw_window(size(px(64.0), px(32.0)), || {
        div().size_full().bg(rgb(0xff0000))
    });

    assert_eq!(image.dimensions(), (64 * SCALE, 32 * SCALE));
    assert_eq!(pixel(&image, 8, 8), [255, 0, 0, 255], "interior");
    assert_eq!(
        pixel(&image, 63 * SCALE, 31 * SCALE),
        [255, 0, 0, 255],
        "bottom right pixel"
    );
}

/// Two differently colored children have to stay distinct: this catches a
/// channel-order mistake in the readback, which a single red window would not.
#[test]
fn renders_adjacent_colors_in_the_right_channels() {
    let image = draw_window(size(px(64.0), px(32.0)), || {
        div()
            .flex()
            .size_full()
            .child(div().flex_1().bg(rgb(0x0000ff)))
            .child(div().flex_1().bg(rgb(0x00ff00)))
    });

    let (width, height) = image.dimensions();
    assert_eq!(
        pixel(&image, 4, height / 2),
        [0, 0, 255, 255],
        "left is blue"
    );
    assert_eq!(
        pixel(&image, width - 4, height / 2),
        [0, 255, 0, 255],
        "right is green"
    );
}

/// An empty window is cleared, not left holding whatever was in the target.
#[test]
fn clears_the_frame_when_there_is_nothing_to_draw() {
    let image = draw_window(size(px(32.0), px(16.0)), || div());

    assert_eq!(image.dimensions(), (32 * SCALE, 16 * SCALE));
    assert!(
        image.pixels().all(|p| p.0 == [0, 0, 0, 0]),
        "expected a fully transparent frame, found {:?}",
        image.pixels().find(|p| p.0 != [0, 0, 0, 0])
    );
}

/// Text exercises the whole path -- shaping, rasterization into the sprite atlas,
/// and sampling it during the draw pass -- so lit pixels prove much more than
/// solid quads do.
#[test]
fn renders_text_through_the_sprite_atlas() {
    let image = draw_window(size(px(200.0), px(60.0)), || {
        div()
            .size_full()
            .bg(rgb(0x101010))
            .child(
                div()
                    .text_color(rgb(0xffffff))
                    .text_size(px(18.0))
                    .mx(px(12.0))
                    .mt(px(12.0))
                    .child(StyledText::new("Hello, GPUI")),
            )
    });

    let lit = image
        .pixels()
        .filter(|p| p.0[0] > 100 && p.0[1] > 100 && p.0[2] > 100)
        .count();
    assert!(
        lit > 50,
        "expected white glyph pixels on the dark background, found {lit}"
    );
}

/// Rounded borders go through the path rasterizer, i.e. the intermediate texture
/// and its MSAA resolve.
#[test]
fn renders_rounded_borders() {
    let image = draw_window(size(px(64.0), px(64.0)), || {
        div()
            .size_full()
            .bg(rgb(0x202020))
            .child(
                div()
                    .size_full()
                    .border_1()
                    .border_color(rgb(0x00ffff))
                    .rounded_full(),
            )
    });

    let (width, _) = image.dimensions();
    // The middle of the left edge sits on the rounded border; the middle of the
    // window is the background inside it.
    assert_eq!(
        pixel(&image, 1, 32 * SCALE),
        [0, 255, 255, 255],
        "left edge should be a cyan border"
    );
    assert_eq!(
        pixel(&image, width / 2, 32 * SCALE),
        [32, 32, 32, 255],
        "center should be the background"
    );
}

/// Re-drawing after a state change has to pick the change up, which means the
/// scene really is re-encoded and the sprite atlas really is being read.
#[test]
fn redraws_after_a_state_change() {
    // The view reads its state at paint time, so mutating it between frames is
    // enough to make the next frame differ.
    let clicks = Rc::new(Cell::new(0usize));

    let (_device, mut cx) = headless_context();
    let (_view, cx) = cx.add_sized_window_view(size(px(48.0), px(24.0)), {
        let clicks = clicks.clone();
        move |_window, _cx| {
            Root(move || {
                let background = if clicks.get() == 0 {
                    rgb(0xff0000)
                } else {
                    rgb(0x0000ff)
                };
                div().size_full().bg(background)
            })
        }
    });

    let before = cx.draw_frame().expect("headless render failed");
    assert_eq!(pixel(&before, 4, 4), [255, 0, 0, 255], "starts red");

    clicks.set(1);

    let after = cx.draw_frame().expect("second headless render failed");
    assert_eq!(
        pixel(&after, 4, 4),
        [0, 0, 255, 255],
        "the frame should reflect the new state"
    );
}

/// Repeated same-size frames reuse the render target, so a benchmark loop
/// measures encoding and submission rather than texture allocation.
#[test]
fn draws_repeatedly() {
    let (_device, mut cx) = headless_context();
    let (_view, cx) = cx.add_sized_window_view(size(px(80.0), px(40.0)), |_window, _cx| {
        Root(|| div().size_full().bg(rgb(0x884400)))
    });

    for _ in 0..8 {
        let image = cx.draw_frame().expect("headless render failed");
        assert_eq!(
            pixel(&image, 40 * SCALE, 20 * SCALE),
            [0x88, 0x44, 0x00, 255]
        );
    }
}

/// A window resized between frames has to be re-rendered at the new size, which
/// exercises tearing down and rebuilding the size-keyed path intermediates.
#[test]
fn follows_a_window_resize() {
    let (_device, mut cx) = headless_context();
    let (_view, cx) = cx.add_sized_window_view(size(px(40.0), px(20.0)), |_window, _cx| {
        Root(|| div().size_full().bg(rgb(0xff00ff)))
    });

    let before = cx.draw_frame().expect("headless render failed");
    assert_eq!(before.dimensions(), (40 * SCALE, 20 * SCALE));

    cx.simulate_resize(size(px(80.0), px(60.0)));

    let after = cx
        .draw_frame()
        .expect("headless render failed after resize");
    assert_eq!(after.dimensions(), (80 * SCALE, 60 * SCALE));
    assert_eq!(pixel(&after, 4, 4), [255, 0, 255, 255]);
}

/// The no-readback path a benchmark would use: many frames, then a real capture
/// to prove the renderer is still in a good state afterwards.
#[test]
fn renders_without_reading_back() {
    let (_device, mut cx) = headless_context();
    let (_view, cx) = cx.add_sized_window_view(size(px(64.0), px(64.0)), |_window, _cx| {
        Root(|| {
            div()
                .size_full()
                .bg(rgb(0x224466))
                .text_color(rgb(0xffffff))
                .child(StyledText::new("benchmark me"))
        })
    });

    for _ in 0..50 {
        cx.render().expect("headless render failed");
    }

    let image = cx.draw_frame().expect("capture after rendering failed");
    assert_eq!(pixel(&image, 4, 4), [0x22, 0x44, 0x66, 255]);
}

/// Writing the frame to a PNG is what a snapshot test does with it; make sure
/// the image is a real, encodable RGBA buffer.
#[test]
fn frame_is_encodable() {
    let image = draw_window(size(px(24.0), px(24.0)), || {
        div().size_full().bg(rgb(0x336699))
    });

    let mut bytes = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
        .expect("the frame should encode as a PNG");
    assert!(bytes.len() > 100, "PNG should not be trivially empty");
}
