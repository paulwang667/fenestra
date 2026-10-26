//! Clip layers and opacity layers, compared with vello.

mod support;

use fenestra_core::draw::DrawTarget;
use kurbo::{Affine, BezPath, Circle, Rect, RoundedRect, Stroke};
use peniko::{Color, Fill};
use support::{EDGES, FILLS, Paint, assert_like_vello_all, render_raster, renderer};

const RED: Color = Color::from_rgb8(220, 40, 50);
const BLUE: Color = Color::from_rgb8(40, 90, 220);
const GREEN: Color = Color::from_rgb8(30, 160, 80);

fn cross<T: DrawTarget>(t: &mut T) {
    t.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        BLUE,
        None,
        &Rect::new(0.0, 0.0, 240.0, 160.0),
    );
    let mut line = BezPath::new();
    line.move_to((0.0, 0.0));
    line.line_to((240.0, 160.0));
    t.stroke(&Stroke::new(10.0), Affine::IDENTITY, RED, None, &line);
}

struct RoundedClip;
impl Paint for RoundedClip {
    fn paint<T: DrawTarget>(&self, t: &mut T) {
        t.push_clip_layer(
            Fill::NonZero,
            Affine::IDENTITY,
            &RoundedRect::new(30.0, 20.0, 210.0, 140.0, 24.0),
        );
        cross(t);
        t.pop_layer();
    }
}

#[test]
fn rounded_clip_matches_vello() {
    assert_like_vello_all("rounded_clip", &RoundedClip, (240, 160), &[EDGES, FILLS]);
}

struct NestedClips;
impl Paint for NestedClips {
    fn paint<T: DrawTarget>(&self, t: &mut T) {
        t.push_clip_layer(
            Fill::NonZero,
            Affine::IDENTITY,
            &Rect::new(40.0, 10.0, 200.0, 150.0),
        );
        t.push_clip_layer(
            Fill::NonZero,
            Affine::IDENTITY,
            &Circle::new((60.0, 80.0), 70.0),
        );
        cross(t);
        t.pop_layer();
        t.pop_layer();
    }
}

#[test]
fn nested_clips_match_vello() {
    assert_like_vello_all("nested_clips", &NestedClips, (240, 160), &[EDGES, FILLS]);
}

struct HalfGroup;
impl Paint for HalfGroup {
    fn paint<T: DrawTarget>(&self, t: &mut T) {
        t.push_layer(
            Fill::NonZero,
            0.5,
            Affine::IDENTITY,
            &Rect::new(0.0, 0.0, 240.0, 160.0),
        );
        t.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            RED,
            None,
            &Rect::new(30.0, 30.0, 150.0, 120.0),
        );
        t.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            BLUE,
            None,
            &Rect::new(90.0, 50.0, 210.0, 140.0),
        );
        t.pop_layer();
    }
}

/// The group fades as one: where its children overlap, only the top one
/// shows through, no darker than either alone.
#[test]
fn opacity_layer_composites_children_once() {
    assert_like_vello_all("half_group", &HalfGroup, (240, 160), &[EDGES, FILLS]);
    let (image, stats) = render_raster(&mut renderer(), &HalfGroup, (240, 160));
    assert_eq!(stats.layers, 1);
    assert_eq!(
        image.pixel(120, 80),
        image.pixel(180, 120),
        "overlap matches blue alone"
    );
}

/// A scroll view's clip, an exit ghost fading (layer) and shrinking
/// (transform), and a rounded clip inside it.
struct Deep;
impl Paint for Deep {
    fn paint<T: DrawTarget>(&self, t: &mut T) {
        t.push_clip_layer(
            Fill::NonZero,
            Affine::IDENTITY,
            &Rect::new(10.0, 10.0, 230.0, 130.0),
        );
        t.push_layer(
            Fill::NonZero,
            0.6,
            Affine::IDENTITY,
            &Rect::new(0.0, 0.0, 240.0, 160.0),
        );
        let about = Affine::translate((120.0, 80.0));
        t.push_transform(about * Affine::scale(0.8) * about.inverse());
        t.push_clip_layer(
            Fill::NonZero,
            Affine::IDENTITY,
            &RoundedRect::new(20.0, 40.0, 220.0, 150.0, 20.0),
        );
        t.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            GREEN,
            None,
            &Rect::new(0.0, 0.0, 240.0, 160.0),
        );
        t.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            RED,
            None,
            &Circle::new((120.0, 140.0), 50.0),
        );
        t.pop_layer();
        t.pop_transform();
        t.pop_layer();
        t.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            BLUE,
            None,
            &Rect::new(0.0, 0.0, 60.0, 40.0),
        );
        t.pop_layer();
    }
}

#[test]
fn clip_in_layer_in_clip_under_a_transform() {
    assert_like_vello_all("deep", &Deep, (240, 160), &[EDGES, FILLS]);
}

struct AfterPop;
impl Paint for AfterPop {
    fn paint<T: DrawTarget>(&self, t: &mut T) {
        t.push_clip_layer(
            Fill::NonZero,
            Affine::IDENTITY,
            &Rect::new(0.0, 0.0, 80.0, 80.0),
        );
        t.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            RED,
            None,
            &Rect::new(0.0, 0.0, 240.0, 160.0),
        );
        t.pop_layer();
        t.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            BLUE,
            None,
            &Rect::new(120.0, 40.0, 220.0, 140.0),
        );
    }
}

#[test]
fn stencil_restores_after_pop() {
    assert_like_vello_all("after_pop", &AfterPop, (240, 160), &[EDGES, FILLS]);
}

#[test]
fn layer_textures_are_pooled() {
    let mut r = renderer();
    let (_, first) = render_raster(&mut r, &Deep, (240, 160));
    let (_, second) = render_raster(&mut r, &Deep, (240, 160));
    assert_eq!(first.layers, 1);
    assert!(first.layer_textures_created > 0);
    assert_eq!(second.layer_textures_created, 0);
    assert!(r.cache_sizes().layer_textures > 0);
}
