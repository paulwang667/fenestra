//! Painting never drops content that ends up inside the clip: a node whose own
//! layout rect lies outside its ancestors' clip can still paint into view
//! through its transform or through an overflowing descendant.

use fenestra_core::{
    Color, DrawTarget, Element, Fonts, FrameState, Glyph, GlyphRun, Theme, build_frame, col, div,
};
use kurbo::{Affine, Rect, Shape, Stroke};
use peniko::{Brush, BrushRef, Fill, ImageData};

const MARK: Color = Color::from_rgb8(255, 0, 0);

/// Counts solid fills in `MARK`.
#[derive(Default)]
struct MarkCounter(usize);

impl DrawTarget for MarkCounter {
    fn fill<'b>(
        &mut self,
        _: Fill,
        _: Affine,
        brush: impl Into<BrushRef<'b>>,
        _: Option<Affine>,
        _: &impl Shape,
    ) {
        if let Brush::Solid(c) = brush.into().to_owned()
            && c == MARK
        {
            self.0 += 1;
        }
    }
    fn stroke<'b>(
        &mut self,
        _: &Stroke,
        _: Affine,
        _: impl Into<BrushRef<'b>>,
        _: Option<Affine>,
        _: &impl Shape,
    ) {
    }
    fn push_layer(&mut self, _: Fill, _: f32, _: Affine, _: &impl Shape) {}
    fn push_clip_layer(&mut self, _: Fill, _: Affine, _: &impl Shape) {}
    fn pop_layer(&mut self) {}
    fn draw_image(&mut self, _: &ImageData, _: Affine) {}
    fn draw_blurred_rounded_rect(&mut self, _: Affine, _: Rect, _: Color, _: f64, _: f64) {}
    fn draw_glyphs(&mut self, _: &GlyphRun<'_>, _: impl Iterator<Item = Glyph>) {}
    fn push_transform(&mut self, _: Affine) {}
    fn pop_transform(&mut self) {}
}

fn marks(root: &Element<()>) -> usize {
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    let frame = build_frame(
        root,
        &Theme::light(),
        &mut fonts,
        &mut state,
        (400.0, 400.0),
        1.0,
    );
    let mut counter = MarkCounter::default();
    frame.paint_into(&mut counter, &mut fonts, &mut state);
    counter.0
}

#[test]
fn a_node_translated_into_the_clip_paints() {
    // Laid out at y = 400..420, far below the 200px clip; painted at 100..120.
    let root = col::<()>().w(200.0).h(200.0).overflow_hidden().children([
        div::<()>().w(100.0).h(400.0).shrink0(),
        div::<()>()
            .w(100.0)
            .h(20.0)
            .shrink0()
            .bg(MARK)
            .translate(0.0, -300.0),
    ]);
    assert_eq!(marks(&root), 1);
}

#[test]
fn an_absolute_child_overflowing_into_the_clip_paints() {
    // The zero-height wrapper sits at y = 400; its child is placed at y = 150.
    let root = col::<()>().w(200.0).h(200.0).overflow_hidden().children([
        div::<()>().w(100.0).h(400.0).shrink0(),
        div::<()>().w(100.0).h(0.0).shrink0().children([div::<()>()
            .absolute()
            .top(-250.0)
            .w(100.0)
            .h(20.0)
            .bg(MARK)]),
    ]);
    assert_eq!(marks(&root), 1);
}
