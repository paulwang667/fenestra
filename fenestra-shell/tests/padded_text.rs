//! Padded static text paints inside its content box: the glyphs are inset
//! by the element's padding instead of sticking to the padding-box edge
//! (which left the padding visible only on the far side).

use fenestra_core::{Color, Element, Fonts, FrameState, Theme, div, stack, text};
use fenestra_shell::render_element_over;

const PAD: f32 = 20.0;

/// A padded, blue-backed text label centered on a black canvas.
fn scene() -> Element<()> {
    stack().w(300.0).h(120.0).children([div()
        .w_full()
        .h_full()
        .items_center()
        .justify_center()
        .child(
            text("Hello world")
                .px(PAD)
                .py(PAD)
                .bg(Color::new([0.31, 0.47, 1.0, 1.0])),
        )])
}

fn render() -> image::RgbaImage {
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    state.reduced_motion = true;
    render_element_over(
        scene(),
        &Theme::dark(),
        (300, 120),
        1.0,
        Color::new([0.0, 0.0, 0.0, 1.0]),
        &mut fonts,
        &mut state,
    )
    .expect("headless render")
}

fn is_box(p: [u8; 4]) -> bool {
    // The blue label background (normalized 0.31/0.47/1.0).
    p[2] > 200 && p[0] < 130
}

/// The label box's horizontal span on one row.
fn box_span(img: &image::RgbaImage, y: u32) -> (u32, u32) {
    let mut x0 = None;
    let mut x1 = None;
    for x in 0..img.width() {
        if is_box(img.get_pixel(x, y).0) {
            x0.get_or_insert(x);
            x1 = Some(x);
        }
    }
    (x0.expect("label box found"), x1.expect("label box found"))
}

/// Bright (glyph) pixels on one row within the box.
fn glyph_span(img: &image::RgbaImage, y: u32, x0: u32, x1: u32) -> Option<(u32, u32)> {
    let mut first = None;
    let mut last = None;
    for x in x0..=x1 {
        let p = img.get_pixel(x, y).0;
        if is_box(p) {
            continue;
        }
        let lum = 0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]);
        if lum > 90.0 {
            first.get_or_insert(x);
            last = Some(x);
        }
    }
    match (first, last) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    }
}

#[test]
fn padded_text_paints_inside_the_content_box() {
    let img = render();
    let (bx0, bx1) = box_span(&img, 60);
    let (gx0, gx1) = glyph_span(&img, 60, bx0, bx1).expect("glyphs on the center row");
    // The glyphs sit PAD inside the box on each side (a couple of px of
    // glyph bearing tolerance). Before the fix they sat at the box edge:
    // the left inset was ~2 and the right inset was 2*PAD+2.
    let left = gx0 - bx0;
    let right = bx1 - gx1;
    assert!(
        left >= PAD as u32 - 2,
        "left inset {left}px must reach the padding (box {bx0}..{bx1}, glyphs {gx0}..{gx1})"
    );
    assert!(
        right >= PAD as u32 - 2,
        "right inset {right}px must reach the padding (box {bx0}..{bx1}, glyphs {gx0}..{gx1})"
    );
    // And the insetting is symmetric, not all on one side.
    assert!(
        left.abs_diff(right) <= 4,
        "padding is symmetric: left {left} vs right {right}"
    );
}

#[test]
fn padded_text_paints_below_the_top_padding() {
    let img = render();
    let (bx0, bx1) = box_span(&img, 60);
    let cx = (bx0 + bx1) / 2;
    let mut by0 = None;
    let mut by1 = None;
    for y in 0..img.height() {
        if is_box(img.get_pixel(cx, y).0) {
            by0.get_or_insert(y);
            by1 = Some(y);
        }
    }
    let (by0, by1) = (by0.expect("box top"), by1.expect("box bottom"));
    // Glyph ink must start at least PAD below the box top.
    let ink_top = (by0..=by1)
        .find(|&y| glyph_span(&img, y, bx0, bx1).is_some())
        .expect("glyphs inside the box");
    assert!(
        ink_top - by0 >= PAD as u32 - 2,
        "glyph ink starts {top}px below the box top; top padding must inset it",
        top = ink_top - by0
    );
}
