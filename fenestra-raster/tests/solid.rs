//! Solid fills and strokes, compared with vello.

mod support;

use fenestra_core::draw::DrawTarget;
use kurbo::{Affine, BezPath, Cap, Circle, Join, Rect, RoundedRect, Stroke};
use peniko::{Color, Fill};
use support::{Budget, Paint, assert_like_vello, assert_like_vello_with, render_raster, renderer};

const BUDGET: Budget = support::EDGES;
const BLUE: Color = Color::from_rgb8(40, 90, 220);

struct RoundedFill;
impl Paint for RoundedFill {
    fn paint<T: DrawTarget>(&self, t: &mut T) {
        let shape = RoundedRect::new(20.0, 20.0, 180.0, 100.0, 16.0);
        t.fill(Fill::NonZero, Affine::IDENTITY, BLUE, None, &shape);
    }
}

#[test]
fn rounded_rect_fill_matches_vello() {
    assert_like_vello("rounded_fill", &RoundedFill, (200, 120), BUDGET);
}

struct Strokes;
impl Paint for Strokes {
    fn paint<T: DrawTarget>(&self, t: &mut T) {
        let frame = RoundedRect::new(10.5, 10.5, 110.5, 70.5, 12.0);
        t.stroke(&Stroke::new(2.0), Affine::IDENTITY, BLUE, None, &frame);
        let mut dashed = BezPath::new();
        dashed.move_to((10.0, 100.0));
        dashed.line_to((230.0, 100.0));
        t.stroke(
            &Stroke::new(3.0).with_dashes(0.0, [8.0, 8.0]),
            Affine::IDENTITY,
            Color::BLACK,
            None,
            &dashed,
        );
        let mut zigzag = BezPath::new();
        zigzag.move_to((130.0, 70.0));
        zigzag.line_to((160.0, 20.0));
        zigzag.line_to((190.0, 70.0));
        zigzag.line_to((220.0, 20.0));
        let round = Stroke::new(6.0)
            .with_caps(Cap::Round)
            .with_join(Join::Round);
        t.stroke(
            &round,
            Affine::IDENTITY,
            Color::from_rgb8(200, 40, 40),
            None,
            &zigzag,
        );
    }
}

#[test]
fn strokes_match_vello() {
    assert_like_vello("strokes", &Strokes, (240, 120), BUDGET);
}

struct EvenOdd;
impl Paint for EvenOdd {
    fn paint<T: DrawTarget>(&self, t: &mut T) {
        let mut rings = BezPath::new();
        for r in [50.0, 30.0] {
            rings.extend(kurbo::Shape::path_elements(
                &Circle::new((60.0, 60.0), r),
                0.1,
            ));
        }
        t.fill(Fill::EvenOdd, Affine::IDENTITY, BLUE, None, &rings);
    }
}

#[test]
fn even_odd_fill_matches_vello() {
    assert_like_vello("even_odd", &EvenOdd, (120, 120), BUDGET);
}

struct Transformed;
impl Paint for Transformed {
    fn paint<T: DrawTarget>(&self, t: &mut T) {
        let about = Affine::translate((100.0, 75.0));
        t.push_transform(
            about * Affine::rotate(30f64.to_radians()) * Affine::scale(1.5) * about.inverse(),
        );
        t.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            BLUE,
            None,
            &Rect::new(70.0, 55.0, 130.0, 95.0),
        );
        t.pop_transform();
    }
}

#[test]
fn transformed_fill_matches_vello() {
    assert_like_vello("transformed", &Transformed, (200, 150), BUDGET);
}

/// One renderer, two target sizes in a row (a resized window).
#[test]
fn renders_at_a_new_size_after_a_resize() {
    let mut r = renderer();
    assert_like_vello_with(&mut r, "resize_small", &RoundedFill, (100, 80), BUDGET);
    assert_like_vello_with(&mut r, "resize_large", &Strokes, (300, 200), BUDGET);
}

struct Degenerate;
impl Paint for Degenerate {
    fn paint<T: DrawTarget>(&self, t: &mut T) {
        t.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            BLUE,
            None,
            &Rect::new(10.0, 10.0, 10.0, 50.0),
        );
        t.stroke(
            &Stroke::new(0.0),
            Affine::IDENTITY,
            BLUE,
            None,
            &Rect::new(10.0, 10.0, 50.0, 50.0),
        );
        t.fill(Fill::NonZero, Affine::IDENTITY, BLUE, None, &BezPath::new());
    }
}

struct Empty;
impl Paint for Empty {
    fn paint<T: DrawTarget>(&self, _: &mut T) {}
}

#[test]
fn empty_and_degenerate_draws_render_nothing() {
    let mut r = renderer();
    for scene in [&Degenerate as &dyn PaintDyn, &Empty] {
        let (image, stats) = scene.render(&mut r);
        assert_eq!(stats.draws, 0);
        assert!(
            image
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [255; 4])
        );
    }
}

/// `Paint` is generic, so a list of scenes goes through this object-safe shim.
trait PaintDyn {
    fn render(
        &self,
        r: &mut fenestra_raster::RasterRenderer,
    ) -> (support::Image, fenestra_raster::RenderStats);
}
impl<P: Paint> PaintDyn for P {
    fn render(
        &self,
        r: &mut fenestra_raster::RasterRenderer,
    ) -> (support::Image, fenestra_raster::RenderStats) {
        render_raster(r, self, (64, 64))
    }
}

struct Translucent;
impl Paint for Translucent {
    fn paint<T: DrawTarget>(&self, t: &mut T) {
        let half_blue = Color::from_rgba8(40, 90, 220, 128);
        t.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            half_blue,
            None,
            &Rect::new(10.0, 10.0, 110.0, 70.0),
        );
    }
}

/// Over a transparent base, vello writes straight (unpremultiplied) color:
/// a frame meant to be composited (a transparent window, a motion render
/// with alpha) must come out the same.
#[test]
fn translucent_frames_are_written_unpremultiplied_like_vello() {
    let size = (120, 80);
    let vello = support::render_vello_on(&Translucent, size, Color::TRANSPARENT);
    let (raster, _) =
        support::render_raster_on(&mut renderer(), &Translucent, size, Color::TRANSPARENT);
    assert_eq!(vello.pixel(60, 40)[3], 128);
    let frac = support::diff_frac(&vello, &raster, 3);
    assert!(
        frac < 0.02,
        "{:.2}% of pixels differ; vello {:?} raster {:?}",
        frac * 100.0,
        vello.pixel(60, 40),
        raster.pixel(60, 40)
    );
}
