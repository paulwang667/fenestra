//! The glass body, composited on the CPU in floating point and quantized once.
//!
//! **Why not let vello paint the tint and the sheen.** vello writes 8-bit
//! targets and quantizes a gradient with no dither, so a sheen that moves a
//! pane a dozen levels across 600px lands as a staircase of flat plateaus —
//! and a 0.7-alpha tint over the frosted backdrop compresses whatever detail
//! the backdrop had to a third of a level, which rounds away to the same
//! staircase. Doing the tint, the sheen and the vibrancy here, in `f32`, and
//! rounding to bytes exactly once with dither is what removes the contours.
//!
//! The helpers at the top are shared with the shell's blur so the backdrop and
//! the body use one transfer function and one dither.

use kurbo::Rect;
use peniko::Color;
use std::sync::OnceLock;

use crate::style::Sheen;

/// 8-bit sRGB code value → linear light, `0.0..=1.0`.
#[must_use]
pub fn srgb_to_linear(v: u8) -> f32 {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = [0.0; 256];
        for (i, out) in t.iter_mut().enumerate() {
            #[expect(clippy::cast_precision_loss, reason = "0..=255")]
            let c = i as f32 / 255.0;
            *out = if c <= 0.040_45 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            };
        }
        t
    })[usize::from(v)]
}

/// Linear light → sRGB, both `0.0..=1.0` (clamped).
#[must_use]
pub fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Rounds an sRGB value in `0.0..=1.0` to a byte with triangular dither of
/// one level peak, keyed by the pixel position so it is deterministic.
///
/// One TPDF level is the textbook dither for an 8-bit target: it decorrelates
/// the rounding error from the signal, so a slope too shallow to step every
/// pixel comes out as fine grain instead of a contour. The same offset goes on
/// all three channels, which keeps the grain neutral rather than coloured.
#[must_use]
pub fn dither_to_u8(v: f32, noise: f32) -> u8 {
    let q = (v * 255.0 + noise).round().clamp(0.0, 255.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=255"
    )]
    let out = q as u8;
    out
}

/// The triangular dither offset for pixel `(x, y)`, in levels, `-1.0..1.0`.
#[must_use]
pub fn dither_noise(x: u32, y: u32) -> f32 {
    let a = hash(x.wrapping_mul(0x9E37_79B9) ^ y.wrapping_mul(0x85EB_CA6B));
    let b = hash(a ^ 0xC2B2_AE35);
    // Two uniforms summed: a triangle on [0, 2), centred.
    unit(a) + unit(b) - 1.0
}

fn hash(mut v: u32) -> u32 {
    v ^= v >> 16;
    v = v.wrapping_mul(0x7FEB_352D);
    v ^= v >> 15;
    v = v.wrapping_mul(0x846C_A68B);
    v ^ (v >> 16)
}

fn unit(v: u32) -> f32 {
    // The top 24 bits are exact in an f32.
    #[expect(clippy::cast_precision_loss, reason = "24-bit value")]
    let u = (v >> 8) as f32;
    u / 16_777_216.0
}

/// Composites a glass pane's body over its frosted `backdrop` — vibrancy, then
/// the solid `tint`, then the `sheen` — in floating point, and returns one
/// dithered RGBA8 image the size of the backdrop.
///
/// `rect` is the pane's logical rect, which the backdrop spans (it is in
/// physical px); it is only needed to lay the sheen's gradient line exactly
/// where vello would have. Vibrancy (`saturation`) works in linear light about
/// Rec. 709 luminance — it is the backdrop's colour the blur washed out, so
/// that is what it restores. Tint and sheen composite source-over in sRGB, the
/// space vello blends in, so a fill alpha tuned against the old path keeps its
/// meaning.
#[must_use]
pub(crate) fn bake_body(
    backdrop: &peniko::ImageData,
    rect: Rect,
    tint: Option<Color>,
    sheen: Option<&Sheen>,
    saturation: f32,
) -> peniko::ImageData {
    let (w, h) = (backdrop.width, backdrop.height);
    let src = backdrop.data.as_ref();
    let mut out = Vec::with_capacity(src.len());
    let sheen = sheen.map(|s| SheenLine::new(s, rect, w, h));
    let tint = tint.map(|c| c.components);
    for y in 0..h {
        for x in 0..w {
            let i = 4 * (y as usize * w as usize + x as usize);
            let px = &src[i..i + 4];
            // Without vibrancy the trip through linear light is the identity,
            // and its three `powf`s per pixel were most of a glass frame's
            // final paint — skip it.
            let mut rgb = if (saturation - 1.0).abs() > f32::EPSILON {
                let mut lin = [
                    srgb_to_linear(px[0]),
                    srgb_to_linear(px[1]),
                    srgb_to_linear(px[2]),
                ];
                let l = 0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2];
                for c in &mut lin {
                    *c = (l + saturation * (*c - l)).max(0.0);
                }
                lin.map(linear_to_srgb)
            } else {
                [px[0], px[1], px[2]].map(|v| f32::from(v) / 255.0)
            };
            if let Some([tr, tg, tb, ta]) = tint {
                let ta = ta.clamp(0.0, 1.0);
                for (c, t) in rgb.iter_mut().zip([tr, tg, tb]) {
                    *c = *c * (1.0 - ta) + t * ta;
                }
            }
            if let Some(line) = &sheen {
                let (white, black) = line.at(x, y);
                // Premultiplied stops: the white and the black halves never
                // meet, so this is a white wash over one end and a black one
                // over the other.
                for c in &mut rgb {
                    *c = *c * (1.0 - white - black) + white;
                }
            }
            let n = dither_noise(x, y);
            out.extend(rgb.map(|c| dither_to_u8(c, n)));
            out.push(px[3]);
        }
    }
    peniko::ImageData {
        data: out.into(),
        format: peniko::ImageFormat::Rgba8,
        alpha_type: backdrop.alpha_type,
        width: w,
        height: h,
    }
}

/// [`bake_body`] without vibrancy (the shell applies it while blurring), as
/// the final pass calls it. (Public, hidden, as the reference the raster
/// backend's GPU glass is tested against.)
#[doc(hidden)]
#[must_use]
pub fn bake_glass_body(
    backdrop: &peniko::ImageData,
    rect: Rect,
    tint: Option<Color>,
    sheen: Option<&Sheen>,
) -> peniko::ImageData {
    bake_body(backdrop, rect, tint, sheen, 1.0)
}

/// The sheen's gradient for a `w`×`h` image of the pane `rect`: `t` at pixel
/// `(x, y)` is `origin + x·step.0 + y·step.1`, white `top` at `t = 0`
/// fading to clear at 0.55, then clear to black `bottom` at 1. (Public,
/// hidden, for the raster backend's GPU glass, which lays the same line.)
#[doc(hidden)]
#[must_use]
pub fn sheen_line(sheen: &Sheen, rect: Rect, w: u32, h: u32) -> (f64, (f64, f64), f32, f32) {
    let l = SheenLine::new(sheen, rect, w, h);
    (l.origin, l.step, l.top, l.bottom)
}

/// The sheen's gradient line in the image's pixel space — the same geometry
/// as the painter's CSS-angle linear gradient — and its three stops.
struct SheenLine {
    origin: f64,
    step: (f64, f64),
    top: f32,
    bottom: f32,
}

impl SheenLine {
    fn new(sheen: &Sheen, rect: Rect, w: u32, h: u32) -> Self {
        let theta = f64::from(sheen.light_deg).to_radians();
        let (sin, cos) = theta.sin_cos();
        let half_len = 0.5 * (rect.width() * sin.abs() + rect.height() * cos.abs());
        let (dx, dy) = (sin, -cos);
        // Logical px per image px along each axis.
        let (sx, sy) = (rect.width() / f64::from(w), rect.height() / f64::from(h));
        let len = (2.0 * half_len).max(1e-6);
        // t at the logical point p is dot(p - start, dir) / len, with p the
        // pixel centre; written so t(x, y) = origin + x*step.0 + y*step.1.
        let start_x = rect.width() * 0.5 - dx * half_len;
        let start_y = rect.height() * 0.5 - dy * half_len;
        let t0 = ((0.5 * sx - start_x) * dx + (0.5 * sy - start_y) * dy) / len;
        Self {
            origin: t0,
            step: (sx * dx / len, sy * dy / len),
            top: sheen.top.clamp(0.0, 1.0),
            bottom: sheen.bottom.clamp(0.0, 1.0),
        }
    }

    /// `(white alpha, black alpha)` at pixel `(x, y)`: white from `top` to
    /// clear at 0.55, clear to `bottom` black at 1.0 — the painter's stops.
    fn at(&self, x: u32, y: u32) -> (f32, f32) {
        let t = self.origin + f64::from(x) * self.step.0 + f64::from(y) * self.step.1;
        #[expect(clippy::cast_possible_truncation, reason = "t is 0..=1")]
        let t = t.clamp(0.0, 1.0) as f32;
        if t < 0.55 {
            (self.top * (1.0 - t / 0.55), 0.0)
        } else {
            (0.0, self.bottom * (t - 0.55) / 0.45)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(v: [u8; 3], w: u32, h: u32) -> peniko::ImageData {
        peniko::ImageData {
            data: [v[0], v[1], v[2], 255].repeat((w * h) as usize).into(),
            format: peniko::ImageFormat::Rgba8,
            alpha_type: peniko::ImageAlphaType::Alpha,
            width: w,
            height: h,
        }
    }

    fn px(img: &peniko::ImageData, x: u32, y: u32) -> [u8; 4] {
        let i = 4 * (y * img.width + x) as usize;
        img.data.as_ref()[i..i + 4].try_into().expect("4 bytes")
    }

    /// **The sheen is a slope, not a staircase.**
    ///
    /// The bug this exists for: a 0.12 white sheen over a dark pane moves it
    /// about eight levels across 600px, and vello rounds that to eight flat
    /// plateaus 70px wide — measured on the sign-in card. With dither the
    /// rounding error is grain, so no run of identical pixels down the pane
    /// gets anywhere near a plateau's length.
    #[test]
    fn a_sheen_over_a_pane_leaves_no_plateaus() {
        let (w, h) = (40, 600);
        let backdrop = flat([20, 40, 30], w, h);
        let tint = crate::theme::oklch(0.35, 0.01, 250.0).with_alpha(0.72);
        let sheen = Sheen {
            light_deg: 180.0,
            top: 0.12,
            bottom: 0.06,
        };
        let out = bake_body(
            &backdrop,
            Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
            Some(tint),
            Some(&sheen),
            1.0,
        );
        let mut longest = 0;
        let mut run = 0;
        let mut prev = None;
        for y in 0..h {
            let p = px(&out, w / 2, y);
            if Some(p) == prev {
                run += 1;
            } else {
                run = 1;
                prev = Some(p);
            }
            longest = longest.max(run);
        }
        assert!(
            longest < 16,
            "a {longest}px plateau of one value: the sheen is banding"
        );
        // And the slope is still there: dither moves grain, not the mean.
        let mean = |y0: u32| {
            (y0..y0 + 40)
                .map(|y| u32::from(px(&out, w / 2, y)[1]))
                .sum::<u32>()
                / 40
        };
        assert!(mean(0) > mean(280) + 4, "the lit end is not lighter");
    }

    /// A tint at full alpha is the tint, whatever is behind it.
    #[test]
    fn an_opaque_tint_covers_the_backdrop() {
        let backdrop = flat([250, 10, 10], 8, 8);
        let tint = crate::theme::oklch(0.5, 0.0, 0.0);
        let out = bake_body(
            &backdrop,
            Rect::new(0.0, 0.0, 8.0, 8.0),
            Some(tint),
            None,
            1.0,
        );
        let want = dither_to_u8(tint.components[0], 0.0);
        for p in out.data.as_ref().as_chunks::<4>().0 {
            assert!(
                p[0].abs_diff(want) <= 1 && p[0].abs_diff(p[2]) <= 1,
                "{p:?}"
            );
        }
    }

    /// **Vibrancy is the backdrop's colour coming back, not the tint's.**
    ///
    /// `Material::saturation` multiplied the chroma of the tint — a near-grey
    /// surface colour with almost none to multiply — so it did nothing at all.
    /// Here it scales the backdrop, which is what the blur washed out.
    #[test]
    fn vibrancy_resaturates_the_backdrop() {
        let backdrop = flat([120, 150, 130], 4, 4);
        let rect = Rect::new(0.0, 0.0, 4.0, 4.0);
        let spread = |img: &peniko::ImageData| {
            let p = px(img, 1, 1);
            i32::from(p[1]) - i32::from(p[0])
        };
        let plain = bake_body(&backdrop, rect, None, None, 1.0);
        let vivid = bake_body(&backdrop, rect, None, None, 1.5);
        assert!(
            spread(&vivid) > spread(&plain) + 4,
            "saturation left the backdrop as it was: {} vs {}",
            spread(&vivid),
            spread(&plain)
        );
    }

    #[test]
    fn baking_is_deterministic() {
        let backdrop = flat([90, 100, 110], 30, 20);
        let rect = Rect::new(5.0, 5.0, 20.0, 15.0);
        let tint = crate::theme::oklch(0.8, 0.02, 90.0).with_alpha(0.5);
        let a = bake_body(&backdrop, rect, Some(tint), Some(&Sheen::glass()), 1.4);
        let b = bake_body(&backdrop, rect, Some(tint), Some(&Sheen::glass()), 1.4);
        assert_eq!(a.data.as_ref(), b.data.as_ref());
    }

    #[test]
    fn transfer_function_round_trips() {
        for v in 0..=255u8 {
            let back = dither_to_u8(linear_to_srgb(srgb_to_linear(v)), 0.0);
            assert_eq!(back, v);
        }
    }

    #[test]
    fn dither_is_triangular_and_centred() {
        let mut sum = 0.0f64;
        let n = 200u32;
        for y in 0..n {
            for x in 0..n {
                let d = dither_noise(x, y);
                assert!((-1.0..1.0).contains(&d));
                sum += f64::from(d);
            }
        }
        let mean = sum / f64::from(n * n);
        assert!(mean.abs() < 0.01, "dither biases the image by {mean}");
    }
}
