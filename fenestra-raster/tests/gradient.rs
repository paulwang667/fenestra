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

/// A sweep centred at height `.0` on a 1200 px wide fill. On pixel row 20
/// (sampled at y = 20), a centre at 20.05 puts the row just above the seam,
/// where vello samples the ramp's far end however far the row reaches; a
/// centre at 20 puts the row exactly on the seam.
struct FarSeam(f64);
impl Paint for FarSeam {
    fn paint<T: DrawTarget>(&self, t: &mut T) {
        let g = Gradient::new_sweep((10.0, self.0), 0.0, std::f32::consts::TAU).with_stops(stops());
        t.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            &g,
            None,
            &Rect::new(0.0, 0.0, 1200.0, 40.0),
        );
    }
}

#[test]
fn the_sweep_seam_is_exact_far_from_the_centre() {
    let size = (1200, 40);
    for centre in [20.05, 20.0] {
        let scene = FarSeam(centre);
        let (raster, _) = support::render_raster(&mut support::renderer(), &scene, size);
        let vello = support::render_vello(&scene, size);
        let wrong: Vec<u32> = (0..size.0)
            .filter(|&x| {
                let (r, v) = (raster.pixel(x, 20), vello.pixel(x, 20));
                r.iter().zip(v).any(|(a, b)| a.abs_diff(b) > 80)
            })
            .collect();
        assert!(
            wrong.len() <= 2,
            "centre {centre}: {} pixels of the seam row differ, from x = {:?}",
            wrong.len(),
            wrong.first()
        );
    }
}

/// Two fills share one set of stops; a third has its own.
struct SharedStops;
impl Paint for SharedStops {
    fn paint<T: DrawTarget>(&self, t: &mut T) {
        let a = Gradient::new_linear((0.0, 0.0), (100.0, 0.0)).with_stops(stops());
        let b = Gradient::new_radial((150.0, 50.0), 40.0).with_stops(stops());
        let c = Gradient::new_linear((0.0, 0.0), (0.0, 100.0)).with_stops([
            ColorStop::from((0.0, Color::BLACK)),
            ColorStop::from((1.0, Color::WHITE)),
        ]);
        for (g, x) in [(&a, 0.0), (&b, 100.0), (&c, 200.0)] {
            t.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                g,
                None,
                &Rect::new(x, 0.0, x + 100.0, 100.0),
            );
        }
    }
}

#[test]
fn a_ramp_is_built_once_per_set_of_stops() {
    let mut r = support::renderer();
    let (_, first) = support::render_raster(&mut r, &SharedStops, (300, 100));
    let (_, second) = support::render_raster(&mut r, &SharedStops, (300, 100));
    assert_eq!((first.ramps_built, second.ramps_built), (2, 0));
}
