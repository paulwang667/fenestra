//! Turns a [`MultiPassSpec`] plan plus a read-back backdrop image into the
//! per-element filtered images the final paint pass composites. This is the CPU
//! middle stage of the two-pass renderer: render with glass skipped → read back
//! → **`process_specs`** → paint the final scene with each filtered image.

use std::collections::HashMap;

use fenestra_core::{ElementFilter, MultiPassSpec, PassKind, WidgetId};
use image::RgbaImage;
use kurbo::Rect;
use vello::peniko;

use crate::blur::{
    apply_element_filter, box_blur_rgba8, box_radius_for_std_dev, refract_edges, settle,
};

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
    let mut out = HashMap::with_capacity(specs.len());
    let (iw, ih) = (backdrop.width(), backdrop.height());
    for spec in specs {
        let Some((x, y, w, h)) = physical_rect(spec.rect, scale, iw, ih) else {
            continue;
        };
        let image: peniko::ImageData = match spec.kind {
            PassKind::BackdropBlur { std_dev, radius } => {
                let sub = image::imageops::crop_imm(backdrop, x, y, w, h).to_image();
                let blurred = box_blur_rgba8(&sub, box_radius_for_std_dev(std_dev));
                // Bend the blurred backdrop at the rounded rim (the lensing
                // pass) — but only when the crop spans the whole pane. A
                // canvas-clamped (off-screen) crop is a truncated slice, and
                // refraction would lens its straight cut edge as a fake rim;
                // fall back to the blur there.
                // Settle it toward one predictable value before the pane's
                // own tint goes over the top; see `blur::SETTLE`.
                let blurred = settle(&blurred);
                let result = if fully_inside(spec.rect, scale, iw, ih) {
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
                to_image_data(&result)
            }
            PassKind::ElementFilter(filter) => {
                let sub = image::imageops::crop_imm(backdrop, x, y, w, h).to_image();
                let filtered = apply_element_filter(&sub, scale_filter(filter, scale));
                to_image_data(&filtered)
            }
            PassKind::Custom { render_key, cache_key } => {
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
        let result = process_specs(&backdrop, &specs, 1.0, &|_key, w, h| {
            Some(peniko::ImageData {
                data: vec![255u8, 0, 0, 255]
                    .repeat((w as usize) * (h as usize))
                .into(),
                format: peniko::ImageFormat::Rgba8,
                alpha_type: peniko::ImageAlphaType::Alpha,
                width: w,
                height: h,
            })
        }, &mut HashMap::new());
        let img = result.get(&id).expect("custom render produced an image");
        assert_eq!(img.width, 100);
        assert_eq!(img.height, 80);
        assert_eq!(img.data.as_ref()[0], 255); // red

        let result = process_specs(&backdrop, &specs, 1.0, &|_, _, _| None, &mut HashMap::new());
        assert!(result.get(&id).is_none());
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
                    data: vec![r, g, b, 255].repeat((w as usize) * (h as usize)).into(),
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
            kind: PassKind::Custom { render_key: 1, cache_key: 1 },
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
        assert_eq!(img.data.as_ref()[0], 255, "cached red reused, not re-rendered blue");
        assert_eq!(calls.get(), 1, "renderer NOT called on cache hit");
        assert_eq!(cache.len(), 1, "cache unchanged after hit");

        // Third call with cache_key=2: cache miss, renderer called again.
        let specs2 = [MultiPassSpec {
            id,
            rect,
            kind: PassKind::Custom { render_key: 1, cache_key: 2 },
        }];
        let renderer = mk_counted(0, 255, 0);
        let result = process_specs(&backdrop, &specs2, 1.0, &*renderer, &mut cache);
        let img = result.get(&id).expect("cache miss produced an image");
        assert_eq!(img.data.as_ref()[0], 0, "re-rendered green, not cached red");
        assert_eq!(calls.get(), 2, "renderer called again on cache miss (different key)");
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
                    data.push(x as u8);        // r
                    data.push(255 - x as u8); // g
                    data.push(128);            // b
                    data.push(255);            // a
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
