//! Deterministic CPU image filters for the two-pass renderer: an integer box
//! blur (three passes ≈ a Gaussian) and the foreground [`ElementFilter`] ops.
//!
//! Determinism is the contract. Goldens are referenced on macOS/Metal and
//! re-run on Linux/lavapipe, so the blur itself must be bit-for-bit identical on
//! any platform given the same input pixels — hence pure integer arithmetic,
//! never a GPU or float-nondeterministic kernel. (The GPU-rendered *input*
//! differs slightly across rasterizers, but blurring only shrinks those
//! differences, and the golden compare is tolerance-based.) The brightness and
//! saturation ops use plain IEEE-754 `f32` per pixel, which is likewise
//! correctly-rounded and platform-stable (Rust never fuses to FMA implicitly).

use fenestra_core::ElementFilter;
use image::RgbaImage;

/// A deterministic Gaussian-approximating blur: three passes of an integer box
/// blur of the given `radius` (the standard 3-box ≈ Gaussian construction).
/// Edges clamp (samples past an edge repeat the edge pixel). A `radius` of `0`
/// (or an empty image) returns the input unchanged.
#[must_use]
pub fn box_blur_rgba8(img: &RgbaImage, radius: u32) -> RgbaImage {
    if radius == 0 || img.width() == 0 || img.height() == 0 {
        return img.clone();
    }
    // A window wider than the image is just a full-image average; cap the radius at
    // the image extent so a hostile (e.g. agent-authored) blur radius can neither
    // overflow `2 * radius + 1` nor unbound the per-row window-fill loop.
    let radius = radius.min(img.width().max(img.height()));
    let mut a = img.clone();
    let mut b = RgbaImage::new(img.width(), img.height());
    // Box blur is separable, so each axis is a 1-D running-sum average —
    // O(pixels) per pass, independent of radius. Three (H then V) passes.
    for _ in 0..3 {
        box_blur_h(&a, &mut b, radius);
        box_blur_v(&b, &mut a, radius);
    }
    a
}

/// Edge refraction (lensing): within a bevel band along the rounded-rect
/// perimeter, resample each pixel from further *inside* along the inward edge
/// normal, so the blurred backdrop appears to bend and compress into the rim —
/// the optical signature that separates real glass from a flat frosted tint
/// (Apple Liquid Glass). `radius_px` is the pane's corner radius in the image's
/// own (physical) pixels; the image is assumed to span the pane's rounded
/// silhouette (the shell crops the backdrop to the pane rect). The interior
/// (beyond the band) is returned byte-identical to the input; only the rim
/// bends.
///
/// Determinism is the contract, as for [`box_blur_rgba8`]: plain IEEE-754 `f32`
/// with edge-clamped bilinear sampling, bit-stable across rasterizers. A
/// degenerate (tiny) image is returned unchanged.
#[must_use]
/// How far a pane settles what is behind it toward one predictable value.
///
/// **The tint is chosen without knowing the picture.** An app writes
/// `Material::new(0.5, ..).tint(dark_chrome)` once, and then that pane has to
/// work over a night sky and over a sunset. Over the sunset the composite came
/// out brown — not the tint and not the picture, but a muddy average of the
/// two that reads as a smudge rather than a control.
///
/// `SETTLE` is how far each pixel moves toward the pane's own mean, and
/// `TOWARD_MID` how far that mean itself moves to the middle. Together they
/// narrow what a fixed tint has to cover: an extreme backdrop stops being
/// extreme, and the composite stops depending on the frame. Modest, because
/// this is frosting a pane and not erasing what is behind it — the point of
/// glass is that you can still see through.
const SETTLE: f32 = 0.35;
const TOWARD_MID: f32 = 0.22;

/// Settles a filtered backdrop toward one predictable value. See [`SETTLE`].
pub(crate) fn settle(img: &RgbaImage) -> RgbaImage {
    let n = f64::from(img.width() * img.height()).max(1.0);
    let mut sum = [0f64; 3];
    for p in img.pixels() {
        for c in 0..3 {
            sum[c] += f64::from(p[c]);
        }
    }
    #[expect(clippy::cast_possible_truncation, reason = "a channel mean is 0..255")]
    let mean: [f32; 3] = std::array::from_fn(|c| (sum[c] / n) as f32);
    // The mean, itself pulled toward the middle: a pane over a sunset settles
    // somewhat darker than the sunset, one over a night somewhat lighter.
    let target: [f32; 3] = std::array::from_fn(|c| mean[c] + (127.5 - mean[c]) * TOWARD_MID);

    let mut out = img.clone();
    for p in out.pixels_mut() {
        for c in 0..3 {
            let v = f32::from(p[c]);
            #[expect(clippy::cast_possible_truncation, reason = "clamped to 0..255")]
            let settled = (v + (target[c] - v) * SETTLE).clamp(0.0, 255.0) as u8;
            p[c] = settled;
        }
    }
    out
}

/// How thick the glass is, in logical px: how far in from its outline a pane
/// bends what is behind it.
///
/// **A thickness, which is what it was not.** The band used to be the corner
/// radius, so a pill — radius half its height — was bevel all the way through
/// and read as a slab, while a large rounded card got a narrow rim off the
/// same code. A pane of glass has one edge measure however it is cut, and
/// tying it to the outline made two surfaces in one design read as two
/// materials.
pub(crate) const EDGE: f32 = 7.0;

/// How much further the blue end bends than the red, as multipliers on the
/// displacement. Glass disperses — shorter wavelengths refract more — and the
/// spread is what puts colour in a rim instead of a grey smear. Small on
/// purpose: at a few px of displacement even this is a fringe of under a
/// pixel, and more reads as a rendering fault rather than as glass.
const DISPERSION: (f32, f32) = (0.94, 1.06);

pub(crate) fn refract_edges(img: &RgbaImage, radius_px: f32, band_px: f32) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    if w < 4 || h < 4 {
        return img.clone();
    }
    let (wf, hf) = (fl(w), fl(h));
    let (hw, hh) = (wf * 0.5, hf * 0.5);
    let r = radius_px.clamp(0.0, hw.min(hh));
    // **The bevel band is a thickness, not a shape.** It was the corner radius
    // — so a pill, whose radius is half its height, was bevel all the way
    // through and every pixel of it was lensed, while a big rounded card got a
    // narrow rim. Backwards: a 2mm pane has a 2mm edge whether it is cut round
    // or square, and the two read as different materials when the edge follows
    // the outline instead of the glass.
    //
    // Clamped to the pane's own half-extent, which is all the room there is.
    let band = band_px.max(1.0).min(hw.min(hh));
    let max_disp = band * 0.55;
    let (ex, ey) = (hw - r, hh - r); // inner box half-extents
    let mut out = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (px, py) = (fl(x) + 0.5, fl(y) + 0.5);
            let (rx, ry) = (px - hw, py - hh); // relative to center
            // Rounded-box signed distance (negative inside the silhouette).
            let (qx, qy) = (rx.abs() - ex, ry.abs() - ey);
            let (ax, ay) = (qx.max(0.0), qy.max(0.0));
            let outside = (ax * ax + ay * ay).sqrt() + qx.max(qy).min(0.0) - r;
            let d = -outside; // inside-distance, positive inside the silhouette
            if d > 0.0 && d < band {
                let (nx, ny) = sdf_normal(rx, ry, ex, ey);
                // Strongest at the very edge, easing (quadratically) to zero at
                // the band's inner boundary; sample `disp` px further inside.
                let t = d / band;
                let disp = max_disp * (1.0 - t) * (1.0 - t);
                // **Three samples, because glass disperses.** A real pane
                // bends short wavelengths further than long ones, which is why
                // the rim of a thick edge fringes. One displacement for all
                // three channels is a lens made of something that does not
                // exist; the split is small — a few percent of a displacement
                // that is itself only a few px — and what it buys is the
                // colour at the rim rather than a grey smear.
                //
                // Paid only inside the band, which is a fraction of the pane.
                let r_px = bilinear(img, px - nx * disp * DISPERSION.0, py - ny * disp * DISPERSION.0);
                let g_px = bilinear(img, px - nx * disp, py - ny * disp);
                let b_px = bilinear(img, px - nx * disp * DISPERSION.1, py - ny * disp * DISPERSION.1);
                out.put_pixel(
                    x,
                    y,
                    image::Rgba([r_px[0], g_px[1], b_px[2], g_px[3]]),
                );
            } else {
                out.put_pixel(x, y, bilinear(img, px, py));
            }
        }
    }
    out
}

/// The outward unit normal of the rounded-box SDF at center-relative `(rx, ry)`
/// for inner half-extents `(ex, ey)`: axis-aligned along the straight edges and
/// radial within a corner quadrant.
fn sdf_normal(rx: f32, ry: f32, ex: f32, ey: f32) -> (f32, f32) {
    let (sx, sy) = (sign(rx), sign(ry));
    let (dx, dy) = (rx.abs() - ex, ry.abs() - ey);
    if dx > 0.0 && dy > 0.0 {
        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
        (sx * dx / len, sy * dy / len)
    } else if dx >= dy {
        (sx, 0.0)
    } else {
        (0.0, sy)
    }
}

/// `-1.0` for negatives, `+1.0` otherwise (a stable axis pick at exactly 0).
fn sign(v: f32) -> f32 {
    if v < 0.0 { -1.0 } else { 1.0 }
}

/// Edge-clamped bilinear sample at fractional pixel-center coords `(sx, sy)`.
fn bilinear(img: &RgbaImage, sx: f32, sy: f32) -> image::Rgba<u8> {
    let (w, h) = (img.width(), img.height());
    let fx = (sx - 0.5).clamp(0.0, fl(w - 1));
    let fy = (sy - 0.5).clamp(0.0, fl(h - 1));
    let (x0f, y0f) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0f, fy - y0f);
    let (x0, y0) = (px_index(x0f), px_index(y0f));
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let p00 = img.get_pixel(x0, y0).0;
    let p10 = img.get_pixel(x1, y0).0;
    let p01 = img.get_pixel(x0, y1).0;
    let p11 = img.get_pixel(x1, y1).0;
    let mut out = [0u8; 4];
    for c in 0..4 {
        let top = f32::from(p00[c]) * (1.0 - tx) + f32::from(p10[c]) * tx;
        let bot = f32::from(p01[c]) * (1.0 - tx) + f32::from(p11[c]) * tx;
        out[c] = f32_to_u8(top * (1.0 - ty) + bot * ty);
    }
    image::Rgba(out)
}

/// `u32` → `f32` for small image dimensions and indices (lossless in range).
#[expect(
    clippy::cast_precision_loss,
    reason = "image dimensions and pixel indices are far below 2^24"
)]
fn fl(v: u32) -> f32 {
    v as f32
}

/// A floored, clamped, non-negative coordinate → pixel index.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "input is floor()ed and clamped to [0, dim-1], so it is a valid index"
)]
fn px_index(v: f32) -> u32 {
    v as u32
}

/// Applies a foreground [`ElementFilter`] to `img`, deterministically. A blur
/// radius is interpreted in the image's own (physical) pixels — the caller
/// scales a logical radius first. Brightness and saturation are per-pixel ops
/// that preserve alpha.
#[must_use]
pub fn apply_element_filter(img: &RgbaImage, filter: ElementFilter) -> RgbaImage {
    match filter {
        ElementFilter::Blur(sigma) => box_blur_rgba8(img, box_radius_for_std_dev(sigma)),
        ElementFilter::Brightness(m) => map_rgb(img, |ch| f32_to_u8(f32::from(ch) * m)),
        ElementFilter::Saturate(m) => saturate(img, m),
    }
}

/// The integer box radius whose three-pass blur best matches a Gaussian of
/// standard deviation `sigma` (physical px): the variance of three box passes of
/// radius `r` is `r(r+1)`, so solve `r(r+1) = sigma²` for the nearest
/// non-negative integer. `sigma <= ~0.4` rounds to `0` (no blur).
#[must_use]
pub(crate) fn box_radius_for_std_dev(sigma: f32) -> u32 {
    if sigma.is_nan() || sigma <= 0.0 {
        return 0;
    }
    let r = (((1.0 + 4.0 * sigma * sigma).sqrt() - 1.0) / 2.0).round();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "box radius is a small, finite, non-negative integer"
    )]
    let radius = r as u32;
    radius
}

/// One horizontal box-blur pass (`src` → `dst`), clamping at the edges.
fn box_blur_h(src: &RgbaImage, dst: &mut RgbaImage, radius: u32) {
    let (w, h) = (src.width(), src.height());
    let count = 2 * radius + 1;
    for y in 0..h {
        for c in 0..4 {
            // Window for x = 0 is [-radius, radius], clamped to [0, w-1].
            let mut sum: u32 = 0;
            for i in 0..count {
                let x = i.saturating_sub(radius).min(w - 1);
                sum += u32::from(src.get_pixel(x, y).0[c]);
            }
            for x in 0..w {
                dst.get_pixel_mut(x, y).0[c] = div_round(sum, count);
                // Slide one px right: drop the leftmost, add the new rightmost.
                let leaving = x.saturating_sub(radius).min(w - 1);
                let entering = (x + radius + 1).min(w - 1);
                sum = sum - u32::from(src.get_pixel(leaving, y).0[c])
                    + u32::from(src.get_pixel(entering, y).0[c]);
            }
        }
    }
}

/// One vertical box-blur pass (`src` → `dst`), clamping at the edges.
fn box_blur_v(src: &RgbaImage, dst: &mut RgbaImage, radius: u32) {
    let (w, h) = (src.width(), src.height());
    let count = 2 * radius + 1;
    for x in 0..w {
        for c in 0..4 {
            let mut sum: u32 = 0;
            for i in 0..count {
                let y = i.saturating_sub(radius).min(h - 1);
                sum += u32::from(src.get_pixel(x, y).0[c]);
            }
            for y in 0..h {
                dst.get_pixel_mut(x, y).0[c] = div_round(sum, count);
                let leaving = y.saturating_sub(radius).min(h - 1);
                let entering = (y + radius + 1).min(h - 1);
                sum = sum - u32::from(src.get_pixel(x, leaving).0[c])
                    + u32::from(src.get_pixel(x, entering).0[c]);
            }
        }
    }
}

/// Rounded integer mean `(sum + count/2) / count` as a byte. The inputs are
/// `u8` channel sums, so the mean is always `<= 255`.
fn div_round(sum: u32, count: u32) -> u8 {
    let mean = (sum + count / 2) / count;
    u8::try_from(mean.min(255)).unwrap_or(255)
}

/// Maps the three color channels of every pixel through `f`, preserving alpha.
fn map_rgb(img: &RgbaImage, f: impl Fn(u8) -> u8) -> RgbaImage {
    let mut out = img.clone();
    for px in out.pixels_mut() {
        px.0[0] = f(px.0[0]);
        px.0[1] = f(px.0[1]);
        px.0[2] = f(px.0[2]);
    }
    out
}

/// Scales saturation about each pixel's luma (Rec. 601 weights). `m == 1.0`
/// leaves the image unchanged; `0.0` is grayscale; `> 1.0` is more vivid.
fn saturate(img: &RgbaImage, m: f32) -> RgbaImage {
    let mut out = img.clone();
    for px in out.pixels_mut() {
        let [r, g, b, _] = px.0;
        let luma = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
        px.0[0] = f32_to_u8(luma + m * (f32::from(r) - luma));
        px.0[1] = f32_to_u8(luma + m * (f32::from(g) - luma));
        px.0[2] = f32_to_u8(luma + m * (f32::from(b) - luma));
    }
    out
}

/// Rounds and clamps an `f32` channel value into a byte.
fn f32_to_u8(v: f32) -> u8 {
    let v = v.round().clamp(0.0, 255.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=255"
    )]
    let out = v as u8;
    out
}

#[cfg(test)]
mod tests {

    /// **Thickness is the glass, not the outline.**
    ///
    /// The band was the corner radius, so a pill — radius half its height —
    /// was bevel all the way through while a big rounded card off the same
    /// code got a narrow rim. Two surfaces in one design read as two
    /// materials. Cut the same sheet two ways and the edge is the same edge:
    /// what this measures is how far in from the outline the picture is still
    /// being bent.
    #[test]
    fn one_thickness_bevels_a_pill_and_a_card_alike() {
        // A grid, so a displacement in any direction lands somewhere different.
        let field = |w: u32, h: u32| {
            RgbaImage::from_fn(w, h, |x, y| {
                if (x / 3 + y / 3) % 2 == 0 {
                    Rgba([0, 0, 0, 255])
                } else {
                    Rgba([255, 255, 255, 255])
                }
            })
        };
        // How far in from the left edge, along the middle row, the output
        // still differs from the input.
        let reach = |img: &RgbaImage, radius: f32| {
            let out = refract_edges(img, radius, EDGE * 2.0);
            let y = img.height() / 2;
            (0..img.width() / 2)
                .filter(|x| out.get_pixel(*x, y) != img.get_pixel(*x, y))
                .max()
                .unwrap_or(0)
        };

        let card = field(240, 120);
        let pill = field(240, 120);
        // A card's radius, and a pill's — half the height, which is what
        // `rounded_full` resolves to.
        let card_reach = reach(&card, 14.0);
        let pill_reach = reach(&pill, 60.0);

        assert!(
            card_reach.abs_diff(pill_reach) <= 4,
            "the same glass bevelled differently by outline: card {card_reach}, pill {pill_reach}"
        );
        // And it is an edge, not the whole pane: a 120 tall pane has 60 to
        // give and the bevel takes a fraction of it.
        assert!(
            pill_reach < 30,
            "the bevel reaches {pill_reach}px into a pane with 60 to spare"
        );
    }

    /// **A pane settles what is behind it, in both directions.**
    ///
    /// The fault this fixes is a fixed tint over an unknown picture: the same
    /// dark chrome that reads as glass over a night sky came out brown over a
    /// sunset. Settling narrows what the tint has to cover, so a bright
    /// backdrop is brought down and a dark one lifted.
    #[test]
    fn a_pane_settles_a_backdrop_toward_the_middle() {
        let mean = |img: &RgbaImage| {
            let n = f64::from(img.width() * img.height());
            img.pixels().map(|p| f64::from(p[0])).sum::<f64>() / n
        };
        let bright = RgbaImage::from_pixel(40, 40, image::Rgba([230, 230, 230, 255]));
        let dark = RgbaImage::from_pixel(40, 40, image::Rgba([16, 16, 16, 255]));

        let settled_bright = mean(&super::settle(&bright));
        let settled_dark = mean(&super::settle(&dark));
        assert!(
            settled_bright < mean(&bright),
            "a bright backdrop was not brought down"
        );
        assert!(settled_dark > mean(&dark), "a dark backdrop was not lifted");
        // Bounded: the point of glass is that you can still see through it,
        // so neither goes anywhere near the middle.
        assert!(
            settled_bright > 190.0 && settled_dark < 60.0,
            "flattened, not settled: {settled_bright:.0} / {settled_dark:.0}"
        );
    }

    /// **And it narrows the range, which is the property.** The distance
    /// between the brightest backdrop and the darkest is what a fixed tint
    /// has to survive.
    #[test]
    fn settling_narrows_the_range_a_tint_has_to_cover() {
        let mean = |img: &RgbaImage| {
            let n = f64::from(img.width() * img.height());
            img.pixels().map(|p| f64::from(p[0])).sum::<f64>() / n
        };
        let bright = RgbaImage::from_pixel(40, 40, image::Rgba([230, 230, 230, 255]));
        let dark = RgbaImage::from_pixel(40, 40, image::Rgba([16, 16, 16, 255]));

        let before = mean(&bright) - mean(&dark);
        let after = mean(&super::settle(&bright)) - mean(&super::settle(&dark));
        assert!(
            after < before * 0.95,
            "settling did not narrow anything: {before:.0} -> {after:.0}"
        );
    }

    /// **The rim splits colour, which is what a lens does.**
    ///
    /// Only where there is contrast behind it: over a flat backdrop every
    /// channel samples the same value and dispersion is correctly invisible.
    /// So the input is a hard black/white split, and the assertion is that
    /// somewhere in the band the red and blue channels disagree — they cannot,
    /// if all three are sampled at one displacement.
    #[test]
    fn the_rim_disperses_where_there_is_contrast_behind_it() {
        let (w, h) = (120u32, 120u32);
        // **A grid, not one split.** Displacement runs along the surface
        // normal — vertical at the top and bottom edges, horizontal at the
        // sides — so a single vertical split leaves the top band sampling
        // down its own column and shows nothing. The first version of this
        // test asserted the code was broken because of that.
        let img = image::RgbaImage::from_fn(w, h, |x, y| {
            if (x / 3 + y / 3) % 2 == 0 {
                image::Rgba([0, 0, 0, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            }
        });
        let out = super::refract_edges(&img, 30.0, super::EDGE * 2.0);

        let split = out
            .enumerate_pixels()
            .any(|(_, _, p)| p[0].abs_diff(p[2]) > 8);
        assert!(
            split,
            "every channel landed on the same sample: the lens is achromatic"
        );
    }

    /// And a flat backdrop stays flat: dispersion of nothing is nothing, and a
    /// coloured fringe over plain colour would be a rendering fault.
    #[test]
    fn a_flat_backdrop_gains_no_fringe() {
        let img = image::RgbaImage::from_pixel(120, 120, image::Rgba([90, 90, 90, 255]));
        let out = super::refract_edges(&img, 30.0, super::EDGE * 2.0);
        assert!(
            out.pixels().all(|p| p[0].abs_diff(p[2]) <= 1),
            "a fringe appeared over a backdrop with nothing to disperse"
        );
    }

    /// What the lensing pass costs, at the sizes real surfaces use.
    ///
    /// **The number that decides where this can be used.** Measured in
    /// release on an M-series laptop: a player control bar at 1426x112 costs
    /// 13.55 ms, and a 60 Hz frame is 16.7. Over a static backdrop that is
    /// paid once and never again; over playing video the backdrop changes
    /// every frame and the whole pane has to be refiltered, so a screenful of
    /// glass over moving content is not affordable on the CPU. That is the
    /// reason the player's centre control is a flat scrim and not glass.
    ///
    /// ```text
    /// cargo test --release -p fenestra-shell --lib -- --ignored --nocapture probe_glass_cost
    /// ```
    #[test]
    #[ignore = "an instrument, not a gate"]
    fn probe_glass_cost() {
        use std::time::Instant;
        for (label, w, h, radius) in [
            ("player control bar", 1426u32, 112u32, 28.0f32),
            ("title pill", 320, 76, 38.0),
            ("play disc", 168, 168, 84.0),
            ("a full window", 2480, 1640, 0.0),
        ] {
            let img = image::RgbaImage::from_fn(w, h, |x, y| {
                image::Rgba([(x % 255) as u8, (y % 255) as u8, 128, 255])
            });
            let n = 10;
            let t = Instant::now();
            for _ in 0..n {
                let b = super::box_blur_rgba8(&img, super::box_radius_for_std_dev(26.0));
                std::hint::black_box(super::refract_edges(&b, radius, super::EDGE * 2.0));
            }
            let per = t.elapsed().as_secs_f64() * 1000.0 / f64::from(n);
            println!("{label} ({w}x{h}): {per:.2} ms");
        }
    }
    use super::*;
    use image::Rgba;

    /// A radius of 0 is a no-op (returns the input unchanged).
    #[test]
    fn radius_zero_is_identity() {
        let mut img = RgbaImage::new(2, 2);
        img.put_pixel(0, 0, Rgba([10, 20, 30, 255]));
        img.put_pixel(1, 1, Rgba([200, 100, 50, 128]));
        assert_eq!(box_blur_rgba8(&img, 0), img);
    }

    /// A flat field stays flat under any radius — clamping never invents an
    /// edge gradient.
    #[test]
    fn uniform_field_is_unchanged() {
        let img = RgbaImage::from_pixel(5, 4, Rgba([77, 88, 99, 255]));
        let out = box_blur_rgba8(&img, 2);
        for px in out.pixels() {
            assert_eq!(px.0, [77, 88, 99, 255]);
        }
    }

    /// Exact, hand-verified output: a 3×1 image whose red channel is
    /// `[0, 0, 90]` (alpha 255) under radius 1. Height 1 makes the vertical
    /// passes identities, so this is three horizontal box passes with rounded
    /// means `(sum + 1) / 3` and edge clamping:
    ///   `[0,0,90] → [0,30,60] → [10,30,50] → [17,30,43]`.
    /// This literal pins the deterministic rounding and clamp rules.
    #[test]
    fn exact_three_pass_small() {
        let mut img = RgbaImage::new(3, 1);
        img.put_pixel(0, 0, Rgba([0, 0, 0, 255]));
        img.put_pixel(1, 0, Rgba([0, 0, 0, 255]));
        img.put_pixel(2, 0, Rgba([90, 0, 0, 255]));
        let out = box_blur_rgba8(&img, 1);
        assert_eq!(out.get_pixel(0, 0).0, [17, 0, 0, 255]);
        assert_eq!(out.get_pixel(1, 0).0, [30, 0, 0, 255]);
        assert_eq!(out.get_pixel(2, 0).0, [43, 0, 0, 255]);
    }

    /// Determinism: the same input blurs to the exact same bytes every time.
    #[test]
    fn is_deterministic() {
        let mut img = RgbaImage::new(8, 6);
        for (i, px) in img.pixels_mut().enumerate() {
            #[expect(clippy::cast_possible_truncation, reason = "test pattern bytes")]
            let v = (i as u32 * 37 % 256) as u8;
            *px = Rgba([v, v.wrapping_mul(3), v.wrapping_add(11), 255]);
        }
        assert_eq!(box_blur_rgba8(&img, 3), box_blur_rgba8(&img, 3));
    }

    /// `box_radius_for_std_dev` solves `r(r+1) = sigma²` and rounds.
    #[test]
    fn radius_from_std_dev() {
        assert_eq!(box_radius_for_std_dev(0.0), 0);
        assert_eq!(box_radius_for_std_dev(-1.0), 0);
        // r(r+1): 1·2=2 → σ=√2≈1.414 maps to 1; 18·19=342 → σ≈18.49 → 18.
        assert_eq!(box_radius_for_std_dev(2.0_f32.sqrt()), 1);
        assert_eq!(box_radius_for_std_dev(18.0), 18);
    }

    /// Brightness scales channels and preserves alpha; saturation at 0 is a
    /// pure luma grayscale (equal R=G=B).
    #[test]
    fn element_filters() {
        let img = RgbaImage::from_pixel(2, 2, Rgba([100, 60, 20, 128]));
        let dim = apply_element_filter(&img, ElementFilter::Brightness(0.5));
        assert_eq!(dim.get_pixel(0, 0).0, [50, 30, 10, 128]);
        let gray = apply_element_filter(&img, ElementFilter::Saturate(0.0));
        let [r, g, b, a] = gray.get_pixel(0, 0).0;
        assert_eq!((r, g, b, a), (r, r, r, 128));
        assert!(g == r && b == r, "grayscale: {r} {g} {b}");
    }

    /// A flat field is unchanged by refraction: bilinear sampling a uniform
    /// image returns the same color wherever it samples from.
    #[test]
    fn refract_uniform_field_is_unchanged() {
        let img = RgbaImage::from_pixel(40, 30, Rgba([60, 120, 200, 210]));
        let out = refract_edges(&img, 12.0, EDGE * 2.0);
        for px in out.pixels() {
            assert_eq!(px.0, [60, 120, 200, 210]);
        }
    }

    /// Determinism: the same input refracts to the exact same bytes every time.
    #[test]
    fn refract_is_deterministic() {
        let mut img = RgbaImage::new(48, 36);
        for (i, px) in img.pixels_mut().enumerate() {
            #[expect(clippy::cast_possible_truncation, reason = "test pattern bytes")]
            let v = (i as u32 * 53 % 256) as u8;
            *px = Rgba([v, v.wrapping_mul(2), v.wrapping_add(7), 255]);
        }
        assert_eq!(refract_edges(&img, 14.0, EDGE * 2.0), refract_edges(&img, 14.0, EDGE * 2.0));
    }

    /// A degenerate (tiny) image is returned unchanged.
    #[test]
    fn refract_tiny_image_is_identity() {
        let img = RgbaImage::from_pixel(3, 3, Rgba([1, 2, 3, 4]));
        assert_eq!(refract_edges(&img, 5.0, EDGE * 2.0), img);
    }

    /// Refraction bends the rim but leaves the center (far from every edge)
    /// untouched.
    #[test]
    fn refract_changes_the_rim_not_the_center() {
        let (w, h) = (60u32, 40u32);
        let mut img = RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                #[expect(clippy::cast_possible_truncation, reason = "test ramp byte")]
                let v = ((x * 255) / (w - 1)) as u8;
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        let out = refract_edges(&img, 14.0, EDGE * 2.0);
        let cy = h / 2;
        // The center column is > band from every edge, so it copies through.
        assert_eq!(
            out.get_pixel(w / 2, cy).0,
            img.get_pixel(w / 2, cy).0,
            "center untouched"
        );
        // A near-edge column is resampled from further inside (the lens bend).
        assert_ne!(out.get_pixel(2, cy).0, img.get_pixel(2, cy).0, "rim bent");
    }

    #[test]
    fn box_blur_huge_radius_is_bounded() {
        // A hostile radius must not overflow `2 * radius + 1` or hang the window
        // fill: it caps at the image extent (a full-image average) and returns.
        let img = RgbaImage::from_pixel(8, 6, Rgba([100, 150, 200, 255]));
        let out = box_blur_rgba8(&img, u32::MAX);
        assert_eq!(out.dimensions(), (8, 6));
        // A uniform image averages to itself at any radius.
        assert_eq!(out.get_pixel(0, 0).0, [100, 150, 200, 255]);
    }
}
