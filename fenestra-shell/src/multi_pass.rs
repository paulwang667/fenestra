//! Turns a [`MultiPassSpec`] plan plus a read-back backdrop image into the
//! per-element filtered images the final paint pass composites. This is the CPU
//! middle stage of the two-pass renderer: render with glass skipped → read back
//! → **`process_specs`** → paint the final scene with each filtered image.

use std::collections::HashMap;

use fenestra_core::{ElementFilter, MultiPassSpec, PassKind, WidgetId};
use image::RgbaImage;
use kurbo::Rect;
use vello::peniko;

use crate::blur::{apply_element_filter, blur_linear_crop, box_radius_for_std_dev, refract_edges};
use fenestra_raster::glass::{GlassPane, blur_reach};

/// Filters each spec's region of the read-back `backdrop`, returning the image
/// the final pass draws for that element (keyed by [`WidgetId`]). `scale` maps a
/// spec's logical rect — and a foreground blur's logical radius — onto the
/// physical backdrop. Regions that clamp to nothing (off-screen or zero-size)
/// are skipped, so a missing entry simply means "paint normally".
/// A render function the shell calls for `PassKind::Custom` specs. It receives
/// the render key (so the shell can look up the right function) and the
/// physical pixel dimensions of the element's layout rect. Returns `None` when
/// no render function is registered for the key (the element paints normally).
pub type CustomRender = dyn Fn(u64, u32, u32) -> Option<peniko::ImageData>;

/// Filters each spec's region of the read-back `backdrop`, returning the image
/// the final pass draws for that element (keyed by [`WidgetId`]). `scale` maps a
/// spec's logical rect — and a foreground blur's logical radius — onto the
/// physical backdrop. Regions that clamp to nothing (off-screen or zero-size)
/// are skipped, so a missing entry simply means "paint normally".
///
/// `custom` is called for `PassKind::Custom` specs — the closure receives the
/// render key and physical pixel dimensions, and returns the rendered image.
/// Pass `&|_, _, _| None` when no custom rendering is needed (the element paints
/// normally instead). This keeps `process_specs` wgpu-free: the shell supplies
/// the device/queue-backed closure, core stays pure.
#[must_use]
pub fn process_specs(
    backdrop: &RgbaImage,
    specs: &[MultiPassSpec],
    scale: f64,
    custom: &CustomRender,
    cache: &mut HashMap<(u64, u64), peniko::ImageData>,
) -> HashMap<WidgetId, peniko::ImageData> {
    let frame = (backdrop.width(), backdrop.height());
    let mut glass = GlassCache::default();
    process_specs_in(
        backdrop,
        (0, 0),
        frame,
        specs,
        scale,
        custom,
        cache,
        &mut glass,
    )
}

/// Each glass pane's last backdrop blur, kept so a frame whose backdrop under
/// the pane has not changed reuses it instead of blurring it again.
///
/// **Most glass frames repaint for something that is not behind the glass** —
/// a spinner elsewhere, a hover inside a menu, a caret in a field on the pane.
/// The read-back still has to happen to know, but comparing the crop is a
/// fraction of the blur and the lensing it saves; and handing back the same
/// image keeps its id, so the final pass's bake cache hits too. One per
/// window; panes that leave the frame leave the cache.
#[derive(Default)]
pub struct GlassCache {
    panes: HashMap<WidgetId, GlassMemo>,
}

/// Whether `now` differs from the `then` a pane was blurred from by too little
/// to show through a blur of `std_dev` physical px.
///
/// **Byte equality never held.** The GPU rasterizer is not bit-stable frame to
/// frame: a backdrop that has not moved comes back with one to three pixels a
/// level or two off, so an exact compare missed on most still frames. What
/// matters is whether the blurred image would change, and that has a bound: a
/// blurred pixel is a weighted mean whose largest weight is about
/// `1 / (2πσ²)`, so it can move by at most the summed absolute input change
/// times that. Under half a level, the old image is the new one. The compare
/// is against the input the image was made from, so drift cannot creep past
/// the bound a frame at a time.
fn unchanged_under_blur(then: &[u8], now: &[u8], std_dev: f32) -> bool {
    if then.len() != now.len() {
        return false;
    }
    if then == now {
        return true;
    }
    let peak = 1.0 / (std::f64::consts::TAU * f64::from(std_dev).powi(2).max(1.0));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a small positive budget"
    )]
    let budget = (0.5 / peak) as u64;
    let mut sum = 0u64;
    for (a, b) in then.as_chunks::<4>().0.iter().zip(now.as_chunks::<4>().0) {
        if a != b {
            sum += (0..3).map(|c| u64::from(a[c].abs_diff(b[c]))).sum::<u64>();
            if sum > budget {
                return false;
            }
        }
    }
    true
}

/// What a pane's blur was computed from, and what it produced.
struct GlassMemo {
    key: GlassKey,
    input: Vec<u8>,
    image: peniko::ImageData,
}

/// Every parameter besides the backdrop bytes that the pane's image depends on.
#[derive(PartialEq)]
struct GlassKey {
    wide: (u32, u32),
    crop: (u32, u32, u32, u32),
    radius: u32,
    saturation: u32,
    corner: u32,
    scale: u64,
    lensed: bool,
}

/// The physical-pixel box `(x, y, w, h)` of an `iw`×`ih` frame that `specs`
/// read from the backdrop — each glass pane padded by its blur's reach, each
/// foreground filter's own rect — or `None` when none of them reads it.
///
/// **Only this much of the backdrop needs to exist on the CPU.** The windowed
/// shell rendered and read back the whole frame for a 56 px header: at 2x that
/// is 16 MB copied off the GPU, every frame, to blur a strip. Rendering the
/// scene shifted into a target this size makes the read-back — and the
/// render feeding it — scale with the glass instead of the window.
#[must_use]
pub fn backdrop_region(
    specs: &[MultiPassSpec],
    scale: f64,
    iw: u32,
    ih: u32,
) -> Option<(u32, u32, u32, u32)> {
    let mut region: Option<(u32, u32, u32, u32)> = None;
    for spec in specs {
        let pad = match spec.kind {
            PassKind::BackdropBlur { std_dev, .. } => blur_reach(std_dev),
            PassKind::ElementFilter(_) => 0,
            PassKind::Custom { .. } => continue,
        };
        let Some((x, y, w, h)) = physical_rect(spec.rect, scale, iw, ih) else {
            continue;
        };
        let (x0, y0) = (x.saturating_sub(pad), y.saturating_sub(pad));
        let (x1, y1) = ((x + w + pad).min(iw), (y + h + pad).min(ih));
        region = Some(match region {
            None => (x0, y0, x1, y1),
            Some((a0, b0, a1, b1)) => (a0.min(x0), b0.min(y0), a1.max(x1), b1.max(y1)),
        });
    }
    region.map(|(x0, y0, x1, y1)| (x0, y0, x1 - x0, y1 - y0))
}

/// The part of a `frame`-sized physical backdrop the two-pass `specs` read
/// ([`backdrop_region`]), and `backdrop` (a physical-space scene) shifted so
/// that part's top-left is the origin — ready to render into a target just
/// that size and hand to [`process_specs_in`]. Falls back to a 1×1 corner
/// when nothing reads the backdrop (custom renders only).
#[must_use]
pub fn region_scene(
    backdrop: &vello::Scene,
    specs: &[MultiPassSpec],
    scale: f64,
    (pw, ph): (u32, u32),
) -> ((u32, u32, u32, u32), vello::Scene) {
    let region = backdrop_region(specs, scale, pw, ph).unwrap_or((0, 0, 1.min(pw), 1.min(ph)));
    let mut shifted = vello::Scene::new();
    shifted.append(
        backdrop,
        Some(kurbo::Affine::translate((
            -f64::from(region.0),
            -f64::from(region.1),
        ))),
    );
    (region, shifted)
}

/// [`process_specs`] over a backdrop that covers only part of the frame: the
/// `backdrop` image's top-left sits at `origin` in a `frame`-sized physical
/// frame, and must cover [`backdrop_region`] of `specs`.
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "the frame geometry and both caches"
)]
pub fn process_specs_in(
    backdrop: &RgbaImage,
    origin: (u32, u32),
    frame: (u32, u32),
    specs: &[MultiPassSpec],
    scale: f64,
    custom: &CustomRender,
    cache: &mut HashMap<(u64, u64), peniko::ImageData>,
    glass: &mut GlassCache,
) -> HashMap<WidgetId, peniko::ImageData> {
    glass
        .panes
        .retain(|id, _| specs.iter().any(|s| s.id == *id));
    let mut out = HashMap::with_capacity(specs.len());
    let (iw, ih) = frame;
    let (ox, oy) = origin;
    let (bw, bh) = (backdrop.width(), backdrop.height());
    // Frame coordinates → backdrop-image coordinates, clamped to the image.
    let local = |x: u32, y: u32| (x.saturating_sub(ox).min(bw), y.saturating_sub(oy).min(bh));
    for spec in specs {
        let Some((x, y, w, h)) = physical_rect(spec.rect, scale, iw, ih) else {
            continue;
        };
        let image: peniko::ImageData = match spec.kind {
            PassKind::BackdropBlur {
                std_dev,
                radius,
                saturation,
            } => {
                // **Blur what is around the pane, not just what is under it.**
                // Cropping to the pane first and clamping at the crop made
                // every rim repeat its own edge pixel, so content just outside
                // a pane — which a real lens bleeds in — never reached it. The
                // three-box kernel reaches `3r`, so that is the margin read.
                let r = box_radius_for_std_dev(std_dev);
                let pad = blur_reach(std_dev);
                let (px0, py0) = (x.saturating_sub(pad), y.saturating_sub(pad));
                let px1 = (x + w + pad).min(iw);
                let py1 = (y + h + pad).min(ih);
                let ((lx0, ly0), (lx1, ly1)) = (local(px0, py0), local(px1, py1));
                let wide =
                    image::imageops::crop_imm(backdrop, lx0, ly0, lx1 - lx0, ly1 - ly0).to_image();
                let lensed = fully_inside(spec.rect, scale, iw, ih);
                let key = GlassKey {
                    wide: wide.dimensions(),
                    crop: (x - px0, y - py0, w, h),
                    radius: r,
                    saturation: saturation.to_bits(),
                    corner: radius.to_bits(),
                    scale: scale.to_bits(),
                    lensed,
                };
                if let Some(memo) = glass.panes.get(&spec.id)
                    && memo.key == key
                    && unchanged_under_blur(&memo.input, wide.as_raw(), std_dev)
                {
                    out.insert(spec.id, memo.image.clone());
                    continue;
                }
                let blurred = blur_linear_crop(&wide, r, saturation, (x - px0, y - py0, w, h));
                // Bend the blurred backdrop at the rounded rim (the lensing
                // pass) — but only when the crop spans the whole pane. A
                // canvas-clamped (off-screen) crop is a truncated slice, and
                // refraction would lens its straight cut edge as a fake rim;
                // fall back to the blur there.
                let result = if lensed {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "physical corner radius fits in f32"
                    )]
                    let radius_px = (f64::from(radius) * scale) as f32;
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "physical edge thickness fits in f32"
                    )]
                    let band_px = (f64::from(crate::blur::EDGE) * scale) as f32;
                    refract_edges(&blurred, radius_px, band_px)
                } else {
                    blurred
                };
                let image = to_image_data(&result);
                glass.panes.insert(
                    spec.id,
                    GlassMemo {
                        key,
                        input: wide.into_raw(),
                        image: image.clone(),
                    },
                );
                image
            }
            PassKind::ElementFilter(filter) => {
                let (lx, ly) = local(x, y);
                let sub = image::imageops::crop_imm(backdrop, lx, ly, w, h).to_image();
                let filtered = apply_element_filter(&sub, scale_filter(filter, scale));
                to_image_data(&filtered)
            }
            PassKind::Custom {
                render_key,
                cache_key,
            } => {
                if let Some(cached) = cache.get(&(render_key, cache_key)) {
                    cached.clone()
                } else {
                    match custom(render_key, w, h) {
                        Some(image) => {
                            cache.insert((render_key, cache_key), image.clone());
                            image
                        }
                        None => continue, // no renderer → paint normally
                    }
                }
            }
        };
        out.insert(spec.id, image);
    }
    out
}

/// Splits `specs` for the raster backend's GPU glass: each backdrop blur
/// becomes a [`GlassPane`] over the pane's whole physical rect (not clamped to
/// the `size`d frame — the GPU extends the frame's edge past it rather than
/// stretch the visible slice), lensed only when it lies wholly inside, with
/// its body from `frame`. Every other spec comes back as it was, for the CPU.
pub(crate) fn glass_panes(
    specs: &[MultiPassSpec],
    frame: &fenestra_core::Frame,
    scale: f64,
    (iw, ih): (u32, u32),
) -> (Vec<GlassPane>, Vec<MultiPassSpec>) {
    let mut panes = Vec::new();
    let mut rest = Vec::new();
    for spec in specs {
        let PassKind::BackdropBlur {
            std_dev,
            radius,
            saturation,
        } = spec.kind
        else {
            rest.push(*spec);
            continue;
        };
        let r = spec.rect;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "device coordinates fit in i32"
        )]
        let (x0, y0, x1, y1) = (
            (r.x0 * scale).floor() as i32,
            (r.y0 * scale).floor() as i32,
            (r.x1 * scale).ceil() as i32,
            (r.y1 * scale).ceil() as i32,
        );
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        #[expect(clippy::cast_sign_loss, reason = "positive by the check above")]
        let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "physical radius and band fit in f32"
        )]
        panes.push(GlassPane {
            id: spec.id,
            rect: (x0, y0, w, h),
            std_dev,
            saturation,
            corner_radius: (f64::from(radius) * scale) as f32,
            edge_band: (f64::from(crate::blur::EDGE) * scale) as f32,
            lensed: fully_inside(r, scale, iw, ih),
            body: frame.glass_body(spec.id),
            scale,
        });
    }
    (panes, rest)
}

/// The final pass as a vello scene (logical coordinates), drawing the panes
/// in `baked` from their injected images as they are (the GPU composited
/// their bodies).
pub(crate) fn paint_final_baked(
    frame: &fenestra_core::Frame,
    fonts: &mut fenestra_core::Fonts,
    state: &mut fenestra_core::FrameState,
    injected: &HashMap<WidgetId, peniko::ImageData>,
    baked: &std::collections::HashSet<WidgetId>,
) -> vello::Scene {
    let mut scene = vello::Scene::new();
    let mut target = fenestra_core::draw::SceneTarget::new(&mut scene);
    frame.paint_final_baked_into(&mut target, fonts, state, injected, baked);
    target.finish();
    scene
}

/// A logical rect scaled to an integer pixel rect clamped to the image, or
/// `None` when it has no area inside the image.
fn physical_rect(rect: Rect, scale: f64, iw: u32, ih: u32) -> Option<(u32, u32, u32, u32)> {
    let left = clamp_coord((rect.x0 * scale).floor(), iw);
    let top = clamp_coord((rect.y0 * scale).floor(), ih);
    let right = clamp_coord((rect.x1 * scale).ceil(), iw);
    let bottom = clamp_coord((rect.y1 * scale).ceil(), ih);
    if right <= left || bottom <= top {
        return None;
    }
    Some((left, top, right - left, bottom - top))
}

/// Whether `rect` scaled to physical px sits fully within the `iw`×`ih` backdrop
/// — i.e. the crop is the whole pane, not a canvas-clamped slice. The lensing
/// pass needs the full rounded silhouette, so it is skipped when this is false.
fn fully_inside(rect: Rect, scale: f64, iw: u32, ih: u32) -> bool {
    rect.x0 * scale >= 0.0
        && rect.y0 * scale >= 0.0
        && rect.x1 * scale <= f64::from(iw)
        && rect.y1 * scale <= f64::from(ih)
}

/// Clamps a (possibly out-of-range) coordinate to `[0, max]` and converts it to
/// a pixel index.
fn clamp_coord(v: f64, max: u32) -> u32 {
    let v = v.clamp(0.0, f64::from(max));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to [0, max], finite"
    )]
    let out = v as u32;
    out
}

/// Scales a foreground filter's logical blur radius to physical px; other
/// filters are scale-independent.
fn scale_filter(filter: ElementFilter, scale: f64) -> ElementFilter {
    match filter {
        ElementFilter::Blur(r) => {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "scaled blur radius fits in f32"
            )]
            let scaled = (f64::from(r) * scale) as f32;
            ElementFilter::Blur(scaled)
        }
        other => other,
    }
}

/// Wraps RGBA8 pixels as a straight-alpha peniko image (mirrors
/// `fenestra_core::image_rgba8`).
fn to_image_data(img: &RgbaImage) -> peniko::ImageData {
    peniko::ImageData {
        data: img.as_raw().clone().into(),
        format: peniko::ImageFormat::Rgba8,
        alpha_type: peniko::ImageAlphaType::Alpha,
        width: img.width(),
        height: img.height(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Reading back only the glass region changes nothing.** Two panes, a
    /// foreground filter and a pane hanging off the frame edge, processed
    /// against the whole frame and against just [`backdrop_region`] of it at
    /// its origin, produce the same bytes.
    #[test]
    fn a_region_backdrop_matches_the_whole_frame() {
        let frame = RgbaImage::from_fn(400, 260, |x, y| {
            image::Rgba([
                (x * 5 % 256) as u8,
                (y * 9 % 256) as u8,
                ((x ^ y) % 256) as u8,
                255,
            ])
        });
        let blur = |id, rect| MultiPassSpec {
            id: WidgetId(id),
            rect,
            kind: PassKind::BackdropBlur {
                std_dev: 10.0,
                radius: 8.0,
                saturation: 1.4,
            },
        };
        let specs = [
            blur(1, Rect::new(60.0, 40.0, 180.0, 90.0)),
            blur(2, Rect::new(200.0, 150.0, 330.0, 210.0)),
            blur(3, Rect::new(340.0, 200.0, 460.0, 300.0)),
            MultiPassSpec {
                id: WidgetId(4),
                rect: Rect::new(120.0, 120.0, 170.0, 160.0),
                kind: PassKind::ElementFilter(ElementFilter::Brightness(1.3)),
            },
        ];
        let none = |_: u64, _: u32, _: u32| None;
        let whole = process_specs(&frame, &specs, 1.0, &none, &mut HashMap::new());
        let (x, y, w, h) =
            backdrop_region(&specs, 1.0, 400, 260).expect("glass reads the backdrop");
        assert!(
            w < 400 || h < 260 || (x, y) != (0, 0),
            "the region is the whole frame"
        );
        let part = image::imageops::crop_imm(&frame, x, y, w, h).to_image();
        let region = process_specs_in(
            &part,
            (x, y),
            (400, 260),
            &specs,
            1.0,
            &none,
            &mut HashMap::new(),
            &mut GlassCache::default(),
        );
        assert_eq!(whole.len(), region.len());
        for (id, img) in &whole {
            assert_eq!(
                img.data.as_ref(),
                region[id].data.as_ref(),
                "{id:?} differs"
            );
        }
    }

    fn one_pane() -> [MultiPassSpec; 1] {
        [MultiPassSpec {
            id: WidgetId(7),
            rect: Rect::new(40.0, 30.0, 160.0, 90.0),
            kind: PassKind::BackdropBlur {
                std_dev: 16.0,
                radius: 10.0,
                saturation: 1.0,
            },
        }]
    }

    fn run(frame: &RgbaImage, glass: &mut GlassCache) -> peniko::ImageData {
        let specs = one_pane();
        let mut out = process_specs_in(
            frame,
            (0, 0),
            frame.dimensions(),
            &specs,
            1.0,
            &|_, _, _| None,
            &mut HashMap::new(),
            glass,
        );
        out.remove(&WidgetId(7)).expect("the pane has an image")
    }

    /// **A still backdrop is blurred once.** The GPU's frame-to-frame jitter —
    /// a few pixels a level off — comes back as the very same image, id and
    /// all, so nothing downstream redoes its work either.
    #[test]
    fn a_still_backdrop_reuses_its_blur() {
        let a = RgbaImage::from_fn(200, 120, |x, y| {
            image::Rgba([(x % 256) as u8, (y * 2 % 256) as u8, 90, 255])
        });
        let mut b = a.clone();
        for (x, y) in [(60, 50), (100, 70), (130, 40)] {
            let p = b.get_pixel_mut(x, y);
            p[0] = p[0].wrapping_add(2);
        }
        let mut glass = GlassCache::default();
        let first = run(&a, &mut glass);
        let again = run(&b, &mut glass);
        assert_eq!(
            first.data.id(),
            again.data.id(),
            "jitter re-blurred the pane"
        );
    }

    /// **A backdrop that moved is blurred again** — a bright bar sliding under
    /// the pane is a change the blur shows, and the cache must not hide it.
    #[test]
    fn a_moved_backdrop_is_blurred_again() {
        let bar = |at: u32| {
            RgbaImage::from_fn(200, 120, |x, _| {
                if (at..at + 12).contains(&x) {
                    image::Rgba([250, 250, 250, 255])
                } else {
                    image::Rgba([20, 20, 30, 255])
                }
            })
        };
        let mut glass = GlassCache::default();
        let first = run(&bar(70), &mut glass);
        let moved = run(&bar(90), &mut glass);
        assert_ne!(first.data.id(), moved.data.id());
        assert_ne!(first.data.as_ref(), moved.data.as_ref());
    }

    /// **The rim sees past the pane.** A bright bar just outside the left
    /// edge has to bleed into the pane's left side, as it would through real
    /// frosted glass; a crop clamped at the pane edge repeated the pane's own
    /// dark edge pixel instead and the bar never arrived.
    #[test]
    fn the_blur_reads_content_beyond_the_pane() {
        let backdrop = RgbaImage::from_fn(200, 100, |x, _| {
            if (40..50).contains(&x) {
                image::Rgba([255, 255, 255, 255])
            } else {
                image::Rgba([0, 0, 0, 255])
            }
        });
        let id = WidgetId(1);
        let specs = [MultiPassSpec {
            id,
            rect: Rect::new(50.0, 20.0, 150.0, 80.0),
            kind: PassKind::BackdropBlur {
                std_dev: 8.0,
                radius: 0.0,
                saturation: 1.0,
            },
        }];
        let out = process_specs(&backdrop, &specs, 1.0, &|_, _, _| None, &mut HashMap::new());
        let img = out.get(&id).expect("glass produced an image");
        let px = |x: u32| img.data.as_ref()[4 * (30 * img.width + x) as usize];
        assert!(
            px(10) > px(80) + 20,
            "the bar beside the pane never reached it: rim {} vs middle {}",
            px(10),
            px(80)
        );
    }

    /// The lensing guard: a pane wholly within the backdrop is refracted; one
    /// that runs off any edge is not (its crop would be a truncated slice, and
    /// refraction would lens the straight cut as a fake rim).
    #[test]
    fn fully_inside_detects_canvas_clamping() {
        let inside = Rect::new(10.0, 10.0, 100.0, 80.0);
        assert!(fully_inside(inside, 1.0, 200, 150));
        assert!(fully_inside(inside, 2.0, 400, 300));
        // Off the left / top / right / bottom edge.
        assert!(!fully_inside(
            Rect::new(-1.0, 10.0, 100.0, 80.0),
            1.0,
            200,
            150
        ));
        assert!(!fully_inside(
            Rect::new(10.0, -1.0, 100.0, 80.0),
            1.0,
            200,
            150
        ));
        assert!(!fully_inside(
            Rect::new(10.0, 10.0, 201.0, 80.0),
            1.0,
            200,
            150
        ));
        assert!(!fully_inside(
            Rect::new(10.0, 10.0, 100.0, 151.0),
            1.0,
            200,
            150
        ));
        // An exact fit to the backdrop edge still counts as inside.
        assert!(fully_inside(
            Rect::new(0.0, 0.0, 200.0, 150.0),
            1.0,
            200,
            150
        ));
    }

    /// A `PassKind::Custom` spec with a registered render function produces
    /// an injected image; one with no registered function is skipped (paints
    /// normally). This proves the multi-pass pipeline can carry custom GPU
    /// render specs end-to-end.
    #[test]
    fn custom_pass_produces_image_when_registered() {
        let backdrop = RgbaImage::new(200, 150);
        let id = WidgetId(42);
        let specs = [MultiPassSpec {
            id,
            rect: Rect::new(10.0, 10.0, 110.0, 90.0),
            kind: PassKind::Custom {
                render_key: 42,
                cache_key: 1,
            },
        }];

        // Registered renderer: returns a solid red 100×80 image.
        let result = process_specs(
            &backdrop,
            &specs,
            1.0,
            &|_key, w, h| {
                Some(peniko::ImageData {
                    data: [255u8, 0, 0, 255]
                        .repeat((w as usize) * (h as usize))
                        .into(),
                    format: peniko::ImageFormat::Rgba8,
                    alpha_type: peniko::ImageAlphaType::Alpha,
                    width: w,
                    height: h,
                })
            },
            &mut HashMap::new(),
        );
        let img = result.get(&id).expect("custom render produced an image");
        assert_eq!(img.width, 100);
        assert_eq!(img.height, 80);
        assert_eq!(img.data.as_ref()[0], 255); // red

        let result = process_specs(&backdrop, &specs, 1.0, &|_, _, _| None, &mut HashMap::new());
        assert!(!result.contains_key(&id));
    }

    /// Cache hit: same `(render_key, cache_key)` reuses the cached image
    /// without calling the render function. Cache miss (different `cache_key`)
    /// re-invokes the renderer. This proves the cache key controls re-rendering.
    #[test]
    fn custom_pass_cache_hit_skips_render() {
        use std::cell::Cell;
        use std::rc::Rc;
        let backdrop = RgbaImage::new(200, 150);
        let id = WidgetId(7);
        let rect = Rect::new(10.0, 10.0, 110.0, 90.0);
        let mut cache = HashMap::new();

        // Closure that returns a solid color wrapped in Some, via a `Cell`
        // (no &mut borrow, so it satisfies the `dyn Fn` `+ 'static` bound).
        let calls = Rc::new(Cell::new(0u32));
        let mk_counted = |r: u8, g: u8, b: u8| {
            let calls = Rc::clone(&calls);
            Rc::new(move |_key: u64, w: u32, h: u32| {
                calls.set(calls.get() + 1);
                Some(peniko::ImageData {
                    data: [r, g, b, 255].repeat((w as usize) * (h as usize)).into(),
                    format: peniko::ImageFormat::Rgba8,
                    alpha_type: peniko::ImageAlphaType::Alpha,
                    width: w,
                    height: h,
                })
            }) as Rc<dyn Fn(u64, u32, u32) -> Option<peniko::ImageData>>
        };

        // First call with cache_key=1: renderer returns red.
        let specs = [MultiPassSpec {
            id,
            rect,
            kind: PassKind::Custom {
                render_key: 1,
                cache_key: 1,
            },
        }];
        let renderer = mk_counted(255, 0, 0);
        let result = process_specs(&backdrop, &specs, 1.0, &*renderer, &mut cache);
        let img = result.get(&id).expect("first render produced an image");
        assert_eq!(img.data.as_ref()[0], 255); // red
        assert_eq!(calls.get(), 1, "renderer called once on cache miss");
        assert_eq!(cache.len(), 1, "one image cached");

        // Second call with same cache_key=1: cache hit, renderer NOT called.
        // Use a different color (blue) — if the cache were missed, we'd see blue.
        let renderer = mk_counted(0, 0, 255);
        let result = process_specs(&backdrop, &specs, 1.0, &*renderer, &mut cache);
        let img = result.get(&id).expect("cache hit produced an image");
        assert_eq!(
            img.data.as_ref()[0],
            255,
            "cached red reused, not re-rendered blue"
        );
        assert_eq!(calls.get(), 1, "renderer NOT called on cache hit");
        assert_eq!(cache.len(), 1, "cache unchanged after hit");

        // Third call with cache_key=2: cache miss, renderer called again.
        let specs2 = [MultiPassSpec {
            id,
            rect,
            kind: PassKind::Custom {
                render_key: 1,
                cache_key: 2,
            },
        }];
        let renderer = mk_counted(0, 255, 0);
        let result = process_specs(&backdrop, &specs2, 1.0, &*renderer, &mut cache);
        let img = result.get(&id).expect("cache miss produced an image");
        assert_eq!(img.data.as_ref()[0], 0, "re-rendered green, not cached red");
        assert_eq!(
            calls.get(),
            2,
            "renderer called again on cache miss (different key)"
        );
        assert_eq!(cache.len(), 2, "two images cached");
    }

    /// A non-trivial custom render: a horizontal RGB gradient (red→green→blue
    /// across columns). Proves the custom render closure receives the correct
    /// physical dimensions and the output pixels flow through the pipeline
    /// unchanged — not just a solid color that could be matched by a no-op.
    #[test]
    fn custom_pass_gradient_preserves_pixels() {
        let backdrop = RgbaImage::new(200, 150);
        let id = WidgetId(9);
        let specs = [MultiPassSpec {
            id,
            rect: Rect::new(10.0, 10.0, 110.0, 30.0),
            kind: PassKind::Custom {
                render_key: 1,
                cache_key: 1,
            },
        }];
        // Gradient: column x maps to (r=x, g=255-x, b=128).
        let renderer = |_key: u64, w: u32, h: u32| {
            let mut data = Vec::with_capacity((w * h * 4) as usize);
            for _y in 0..h {
                for x in 0..w {
                    data.push(x as u8); // r
                    data.push(255 - x as u8); // g
                    data.push(128); // b
                    data.push(255); // a
                }
            }
            Some(peniko::ImageData {
                data: data.into(),
                format: peniko::ImageFormat::Rgba8,
                alpha_type: peniko::ImageAlphaType::Alpha,
                width: w,
                height: h,
            })
        };
        let result = process_specs(&backdrop, &specs, 1.0, &renderer, &mut HashMap::new());
        let img = result.get(&id).expect("gradient render produced an image");
        assert_eq!(img.width, 100);
        assert_eq!(img.height, 20);
        let pixels = img.data.as_ref();
        // Left edge: r=0, g=255, b=128.
        assert_eq!(pixels[0], 0, "left edge red");
        assert_eq!(pixels[1], 255, "left edge green");
        assert_eq!(pixels[2], 128, "left edge blue");
        // Right edge: r=99, g=156, b=128.
        let last_col = (img.width - 1) as usize * 4;
        assert_eq!(pixels[last_col], 99, "right edge red");
        assert_eq!(pixels[last_col + 1], 156, "right edge green");
        assert_eq!(pixels[last_col + 2], 128, "right edge blue");
        // Middle row starts after the first row's pixels.
        let mid_row = (img.height / 2) as usize * img.width as usize * 4;
        let mid_col = (img.width / 2) as usize * 4;
        assert_eq!(pixels[mid_row + mid_col], 50, "middle red");
        assert_eq!(pixels[mid_row + mid_col + 1], 205, "middle green");
    }
}
