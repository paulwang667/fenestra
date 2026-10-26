//! Linear, radial, and sweep gradients, compared with vello.

mod support;

use fenestra_core::draw::DrawTarget;
use kurbo::{Affine, Point, Rect, RoundedRect};
use peniko::{Color, ColorStop, Fill, Gradient};
use support::{EDGES, FILLS, Paint, assert_like_vello_all};

fn stops() -> [ColorStop; 3] {
    [
        ColorStop::from((0.0, Color::from_rgb8(230, 40, 60))),
        ColorStop::from((0.5, Color::from_rgb8(250, 200, 40))),
        ColorStop::from((1.0, Color::from_rgb8(40, 90, 220))),
    ]
}

struct Filled(Gradient, Affine);
impl Paint for Filled {
    fn paint<T: DrawTarget>(&self, t: &mut T) {
        t.push_transform(self.1);
        let shape = RoundedRect::new(20.0, 20.0, 220.0, 140.0, 18.0);
        t.fill(Fill::NonZero, Affine::IDENTITY, &self.0, None, &shape);
        t.pop_transform();
    }
}

#[test]
fn linear_gradient_matches_vello() {
    let g = Gradient::new_linear((20.0, 20.0), (220.0, 140.0)).with_stops(stops());
    assert_like_vello_all(
        "linear",
        &Filled(g, Affine::IDENTITY),
        (240, 160),
        &[EDGES, FILLS],
    );
}

#[test]
fn radial_gradient_matches_vello() {
    let g = Gradient::new_radial((120.0, 80.0), 90.0).with_stops(stops());
    assert_like_vello_all(
        "radial",
        &Filled(g, Affine::IDENTITY),
        (240, 160),
        &[EDGES, FILLS],
    );
}

#[test]
fn sweep_gradient_matches_vello() {
    let g = Gradient::new_sweep((120.0, 80.0), 0.0, std::f32::consts::TAU).with_stops(stops());
    assert_like_vello_all(
        "sweep",
        &Filled(g, Affine::IDENTITY),
        (240, 160),
        &[EDGES, FILLS],
    );
}

#[test]
fn gradient_under_rotation_matches_vello() {
    let g = Gradient::new_linear((20.0, 80.0), (220.0, 80.0)).with_stops(stops());
    let about = Affine::translate((120.0, 80.0));
    let t = about * Affine::rotate(30f64.to_radians()) * about.inverse();
    assert_like_vello_all("linear_rotated", &Filled(g, t), (240, 160), &[EDGES, FILLS]);
}

struct Translucent;
impl Paint for Translucent {
    fn paint<T: DrawTarget>(&self, t: &mut T) {
        // Over a dark band, so the alpha ramp (interpolated premultiplied,
        // as vello does) shows.
        t.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from_rgb8(20, 20, 20),
            None,
            &Rect::new(0.0, 60.0, 240.0, 100.0),
        );
        let g = Gradient::new_linear(Point::new(10.0, 0.0), Point::new(230.0, 0.0)).with_stops([
            ColorStop::from((0.0, Color::from_rgba8(255, 0, 0, 255))),
            ColorStop::from((1.0, Color::from_rgba8(0, 0, 255, 0))),
        ]);
        t.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            &g,
            None,
            &Rect::new(10.0, 10.0, 230.0, 150.0),
        );
    }
}

#[test]
fn translucent_stops_match_vello() {
    assert_like_vello_all("translucent", &Translucent, (240, 160), &[EDGES, FILLS]);
}

/// Steep gradients show where a gradient is sampled within the pixel:
/// vello evaluates at the pixel's integer coordinate.
#[test]
fn steep_gradients_sample_where_vello_does() {
    for (name, g) in [
        (
            "steep_radial",
            Gradient::new_radial((60.0, 40.0), 20.0).with_stops(stops()),
        ),
        (
            "steep_linear",
            Gradient::new_linear((50.0, 0.0), (70.0, 0.0)).with_stops(stops()),
        ),
        (
            "steep_sweep",
            Gradient::new_sweep((60.0, 40.0), 0.0, std::f32::consts::TAU).with_stops(stops()),
        ),
    ] {
        assert_like_vello_all(
            name,
            &Filled(g, Affine::IDENTITY),
            (120, 80),
            &[EDGES, FILLS],
        );
    }
}
