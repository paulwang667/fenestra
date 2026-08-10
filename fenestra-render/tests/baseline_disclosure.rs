//! The screenshot comparison must never hand its caller the contents of the
//! baseline it was pointed at.
//!
//! `match_screenshot` and the scenario runner's `expect.screenshot` both take
//! a path to a baseline PNG and return a diff image when the comparison
//! fails. Those two facts are safe apart and dangerous together: the MCP
//! server takes the path from an agent, so a diff image that draws the
//! baseline underneath its red markers turns "compare my render to a file"
//! into "show me any PNG on this disk". The underlay is the *rendered* image
//! for that reason — the caller authored it and already has it.
//!
//! These tests exercise `diff_images` directly rather than through the render
//! path: the leak is in the pixel arithmetic, and pinning it here keeps the
//! regression test off the GPU.

use fenestra_render::diff_images;
use image::{Rgba, RgbaImage};

/// A baseline that is a *picture*, not a flat fill — a leak that preserved
/// only an average would still have to fail these tests.
fn secret(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the modulo keeps every channel inside u8"
        )]
        Rgba([(x * 7 % 256) as u8, (y * 11 % 256) as u8, 200, 255])
    })
}

/// What the diff draws for a pixel that did not differ: the *rendered*
/// pixel at a third brightness.
fn dimmed(p: Rgba<u8>) -> Rgba<u8> {
    Rgba([p.0[0] / 3, p.0[1] / 3, p.0[2] / 3, 255])
}

/// The one-shot dump: a per-channel tolerance nothing can exceed means no
/// pixel is marked as differing, and a negative budget still declares the
/// comparison failed — which is what makes the diff image come back. Before
/// the fix this returned the whole baseline at a third brightness.
#[test]
fn a_tolerance_nothing_exceeds_cannot_dump_the_baseline() {
    let baseline = secret(64, 48);
    let flat = Rgba([99, 150, 201, 255]);
    let actual = RgbaImage::from_pixel(64, 48, flat);

    let diff = diff_images(&baseline, &actual, 255, -1.0, &[]);
    assert!(!diff.ok, "a negative budget fails every comparison");
    assert_eq!(diff.differing, 0, "nothing can exceed a tolerance of 255");
    let img = diff
        .diff_png
        .expect("a failed comparison returns a diff image");

    // Every pixel matched, so every pixel is the rendered colour dimmed —
    // and nothing anywhere in the image is the baseline's.
    let want = dimmed(flat);
    for (x, y, p) in img.enumerate_pixels() {
        assert_eq!(*p, want, "pixel {x},{y} is not the dimmed render");
        assert_ne!(
            *p,
            dimmed(*baseline.get_pixel(x, y)),
            "pixel {x},{y} carries the baseline's colour"
        );
    }
}

/// The ordinary failing comparison still says *where* things differ — that is
/// the whole point of the image — but what it draws underneath is the render.
///
/// The tolerance here is chosen so that a substantial minority of pixels pass
/// and the rest do not: a diff in which *everything* is marked red would be
/// satisfied by either underlay and would not be testing anything.
#[test]
fn a_failing_diff_marks_differences_over_the_render_not_the_baseline() {
    let baseline = secret(64, 64);
    let flat = Rgba([99, 150, 201, 255]);
    let actual = RgbaImage::from_pixel(64, 64, flat);

    let diff = diff_images(&baseline, &actual, 60, 0.0, &[]);
    assert!(!diff.ok, "some pixels are past a tolerance of 60");
    assert!(diff.differing > 0, "and some are not");
    let img = diff
        .diff_png
        .expect("a failed comparison returns a diff image");

    let red = Rgba([255, 0, 0, 255]);
    let mut passed = 0u32;
    let mut discriminating = 0u32;
    for (x, y, p) in img.enumerate_pixels() {
        if *p == red {
            continue;
        }
        passed += 1;
        assert_eq!(*p, dimmed(flat), "pixel {x},{y} is not the dimmed render");
        // Where the baseline's dimmed colour differs from the render's, this
        // pixel is one that would have exposed the old underlay.
        if dimmed(*baseline.get_pixel(x, y)) != dimmed(flat) {
            discriminating += 1;
        }
    }
    assert!(passed > 0, "the comparison must leave some pixels unmarked");
    assert!(
        discriminating > 0,
        "the fixture must contain pixels where the two underlays disagree, \
         or this test would pass against either"
    );
}

/// A masked region is drawn as flat grey — it must not fall through to the
/// baseline either.
#[test]
fn masked_regions_show_neither_image() {
    use fenestra_describe::dto::Bounds;

    let baseline = secret(16, 16);
    let actual = RgbaImage::from_pixel(16, 16, Rgba([99, 150, 201, 255]));
    let masks = vec![Bounds {
        x: 0.0,
        y: 0.0,
        w: 8.0,
        h: 8.0,
    }];

    let diff = diff_images(&baseline, &actual, 0, 0.0, &masks);
    let img = diff.diff_png.expect("the unmasked half still differs");
    for y in 0..8 {
        for x in 0..8 {
            assert_eq!(
                *img.get_pixel(x, y),
                Rgba([40, 40, 40, 255]),
                "masked pixel {x},{y}"
            );
        }
    }
}

/// A passing comparison returns no image at all, so there is nothing to leak.
#[test]
fn a_passing_comparison_returns_no_image() {
    let baseline = secret(16, 16);
    let diff = diff_images(&baseline, &baseline.clone(), 0, 0.0, &[]);
    assert!(diff.ok);
    assert!(diff.diff_png.is_none());
}
