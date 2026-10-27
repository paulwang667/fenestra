//! The raster backend's GPU glass against the CPU path it replaces: the same
//! backdrop through both, compared within a tolerance (the GPU runs in f32
//! with its own rounding; the dither is the same hash).

use fenestra_core::WidgetId;
use fenestra_raster::GlassPane;
use fenestra_raster::glass::{blur_reach, box_radius_for_std_dev};
use image::RgbaImage;

use crate::Headless;
use crate::backend::Backend;
use crate::blur::blur_linear_crop;

/// A backdrop with detail at every scale: a red ramp across, a blue ramp
/// down, and an 8 px checker in green.
fn backdrop(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| {
        let checker = ((x / 8) + (y / 8)) % 2 == 0;
        image::Rgba([
            (x * 255 / w) as u8,
            if checker { 220 } else { 40 },
            (y * 255 / h) as u8,
            255,
        ])
    })
}

fn upload(device: &wgpu::Device, queue: &wgpu::Queue, img: &RgbaImage) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("glass test backdrop"),
        size: wgpu::Extent3d {
            width: img.width(),
            height: img.height(),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
    });
    queue.write_texture(
        texture.as_image_copy(),
        img.as_raw(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(img.width() * 4),
            rows_per_image: None,
        },
        texture.size(),
    );
    texture
}

/// A pane with nothing but the blur.
pub(crate) fn pane(rect: (i32, i32, u32, u32), std_dev: f32, saturation: f32) -> GlassPane {
    GlassPane {
        id: WidgetId(1),
        rect,
        std_dev,
        saturation,
        corner_radius: 0.0,
        edge_band: 0.0,
        lensed: false,
        body: None,
        scale: 1.0,
    }
}

/// The CPU path's frosted backdrop for a pane wholly inside `img`.
pub(crate) fn cpu_blur(img: &RgbaImage, p: &GlassPane) -> RgbaImage {
    let (x, y, w, h) = p.rect;
    let (x, y) = (x as u32, y as u32);
    let r = box_radius_for_std_dev(p.std_dev);
    let pad = blur_reach(p.std_dev);
    let (px0, py0) = (x.saturating_sub(pad), y.saturating_sub(pad));
    let px1 = (x + w + pad).min(img.width());
    let py1 = (y + h + pad).min(img.height());
    let wide = image::imageops::crop_imm(img, px0, py0, px1 - px0, py1 - py0).to_image();
    blur_linear_crop(&wide, r, p.saturation, (x - px0, y - py0, w, h))
}

/// The GPU glass for `panes` over `img`; the first pane's result.
pub(crate) fn gpu(h: &mut Headless, img: &RgbaImage, panes: &[GlassPane]) -> Option<RgbaImage> {
    let (device, queue, raster) = h.raster_gpu().expect("raster backend");
    let source = upload(device, queue, img);
    let (out, _) = raster.glass(device, queue, &source, panes);
    let placeholder = out.get(&panes[0].id)?;
    let pixels = raster.read_registered(device, queue, placeholder)?;
    RgbaImage::from_raw(placeholder.width, placeholder.height, pixels)
}

pub(crate) fn headless() -> Headless {
    Headless::with_backend(Backend::Raster).expect("a GPU for the raster backend")
}

/// Within tolerance: at most 1% of pixels off by more than 8/255 in any
/// channel, and a mean absolute difference of at most 1.5/255.
#[track_caller]
pub(crate) fn assert_close(gpu: &RgbaImage, cpu: &RgbaImage, what: &str) {
    assert_eq!(gpu.dimensions(), cpu.dimensions(), "{what}: sizes");
    let (mut over, mut sum, mut n) = (0usize, 0f64, 0usize);
    for (a, b) in gpu.pixels().zip(cpu.pixels()) {
        let d: Vec<u8> = a.0.iter().zip(b.0).map(|(x, y)| x.abs_diff(y)).collect();
        if d.iter().any(|&v| v > 8) {
            over += 1;
        }
        sum += d.iter().map(|&v| f64::from(v)).sum::<f64>();
        n += 4;
    }
    let frac = over as f64 / f64::from(gpu.width() * gpu.height());
    let mean = sum / n as f64;
    assert!(
        frac <= 0.01 && mean <= 1.5,
        "{what}: {:.2}% of pixels off by more than 8/255, mean |Δ| {mean:.2}",
        frac * 100.0
    );
}

#[test]
fn a_small_blur_matches_the_cpu_at_full_resolution() {
    let img = backdrop(240, 160);
    let p = pane((60, 40, 120, 80), 3.0, 1.0);
    let gpu = gpu(&mut headless(), &img, &[p]).expect("a result");
    assert_close(&gpu, &cpu_blur(&img, &p), "σ 3");
}

/// The GPU downsamples a large blur up to 8× (the CPU stops at 4×): at a
/// low radius of 4 or more the two cannot be told apart, and an eighth of
/// the texels is a quarter of the blur's cost on a phone.
#[test]
fn a_very_large_blur_downsamples_by_eight_and_still_matches_the_cpu() {
    use fenestra_raster::glass::{downsample_factor, gpu_downsample_factor};
    let r = box_radius_for_std_dev(40.0);
    assert_eq!(downsample_factor(r), 4, "the CPU keeps its rule");
    assert_eq!(gpu_downsample_factor(r), 8);
    assert_eq!(gpu_downsample_factor(box_radius_for_std_dev(30.0)), 4);
    let img = backdrop(480, 360);
    let mut h = headless();
    for (std_dev, saturation) in [(40.0, 1.0), (64.0, 1.5)] {
        let p = pane((120, 90, 240, 180), std_dev, saturation);
        let gpu = gpu(&mut h, &img, &[p]).expect("a result");
        assert_close(&gpu, &cpu_blur(&img, &p), &format!("σ {std_dev}"));
    }
}

#[test]
fn a_large_blur_matches_the_cpu_downsampled() {
    let img = backdrop(400, 300);
    let p = pane((100, 80, 200, 140), 40.0, 1.0);
    assert_eq!(
        fenestra_raster::glass::downsample_factor(box_radius_for_std_dev(40.0)),
        4
    );
    let gpu = gpu(&mut headless(), &img, &[p]).expect("a result");
    assert_close(&gpu, &cpu_blur(&img, &p), "σ 40");
}

#[test]
fn saturation_matches_the_cpu() {
    let img = backdrop(320, 240);
    let p = pane((80, 60, 160, 120), 20.0, 1.8);
    let gpu = gpu(&mut headless(), &img, &[p]).expect("a result");
    assert_close(&gpu, &cpu_blur(&img, &p), "σ 20, saturation 1.8");
}

#[test]
fn tiny_and_unblurred_panes_do_not_fail() {
    let img = backdrop(120, 80);
    let mut h = headless();
    assert!(gpu(&mut h, &img, &[pane((10, 10, 0, 20), 8.0, 1.0)]).is_none());
    assert!(gpu(&mut h, &img, &[pane((10, 10, 20, 0), 8.0, 1.0)]).is_none());
    let tiny = gpu(&mut h, &img, &[pane((30, 30, 3, 3), 8.0, 1.0)]).expect("a 3×3 result");
    assert_eq!(tiny.dimensions(), (3, 3));
    let p = pane((20, 20, 60, 40), 0.0, 1.0);
    let unblurred = gpu(&mut h, &img, &[p]).expect("a result");
    let crop = image::imageops::crop_imm(&img, 20, 20, 60, 40).to_image();
    assert_close(&unblurred, &crop, "σ 0 is the backdrop");
}

#[test]
fn a_pane_that_resizes_or_goes_away_keeps_no_stale_texture() {
    let img = backdrop(240, 160);
    let mut h = headless();
    let first = pane((40, 40, 40, 30), 6.0, 1.0);
    let second = pane((40, 40, 60, 20), 6.0, 1.0);
    assert_eq!(gpu(&mut h, &img, &[first]).unwrap().dimensions(), (40, 30));
    let resized = gpu(&mut h, &img, &[second]).unwrap();
    assert_eq!(resized.dimensions(), (60, 20));
    assert_close(&resized, &cpu_blur(&img, &second), "resized");
    let (device, queue, raster) = h.raster_gpu().unwrap();
    let source = upload(device, queue, &img);
    assert_eq!(raster.cache_sizes().glass_panes, 1);
    for _ in 0..70 {
        raster.glass(device, queue, &source, &[]);
    }
    assert_eq!(
        raster.cache_sizes().glass_panes,
        0,
        "a pane gone for good is dropped"
    );
}

/// A lensed pane: the CPU's refraction over its blur.
fn lensed(rect: (i32, i32, u32, u32), std_dev: f32) -> GlassPane {
    GlassPane {
        corner_radius: 24.0,
        edge_band: 14.0,
        lensed: true,
        ..pane(rect, std_dev, 1.0)
    }
}

/// The 14 px band just inside the pane's edge (where refraction moves things).
fn band(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    RgbaImage::from_fn(w, h, |x, y| {
        let edge = x.min(y).min(w - 1 - x).min(h - 1 - y);
        if edge < 14 {
            *img.get_pixel(x, y)
        } else {
            image::Rgba([0, 0, 0, 0])
        }
    })
}

#[test]
fn refraction_matches_the_cpu() {
    let img = backdrop(320, 240);
    let p = lensed((60, 60, 200, 120), 1.5);
    let reference = crate::blur::refract_edges(&cpu_blur(&img, &p), 24.0, 14.0);
    let gpu = gpu(&mut headless(), &img, &[p]).expect("a result");
    assert_close(&gpu, &reference, "refracted pane");
    assert_close(&band(&gpu), &band(&reference), "the refracted band");
}

#[test]
fn an_unlensed_pane_is_not_refracted() {
    let img = backdrop(320, 240);
    let p = GlassPane {
        lensed: false,
        ..lensed((60, 60, 200, 120), 1.5)
    };
    let plain = cpu_blur(&img, &p);
    let gpu = gpu(&mut headless(), &img, &[p]).expect("a result");
    assert_close(&gpu, &plain, "unlensed pane");
    let refracted = crate::blur::refract_edges(&plain, 24.0, 14.0);
    assert_ne!(
        band(&plain),
        band(&refracted),
        "the fixture's band does refract"
    );
}

fn image_data(img: &RgbaImage) -> vello::peniko::ImageData {
    vello::peniko::ImageData {
        data: img.as_raw().clone().into(),
        format: vello::peniko::ImageFormat::Rgba8,
        alpha_type: vello::peniko::ImageAlphaType::Alpha,
        width: img.width(),
        height: img.height(),
    }
}

fn rgba(data: &vello::peniko::ImageData) -> RgbaImage {
    RgbaImage::from_raw(data.width, data.height, data.data.as_ref().to_vec()).expect("RGBA8")
}

fn flat(w: u32, h: u32, v: [u8; 3]) -> RgbaImage {
    RgbaImage::from_pixel(w, h, image::Rgba([v[0], v[1], v[2], 255]))
}

/// A pane with a body (logical rect at scale 1 = its device rect).
fn bodied(
    rect: (i32, i32, u32, u32),
    tint: vello::peniko::Color,
    sheen: Option<fenestra_core::Sheen>,
    adaptive: Option<fenestra_core::AdaptiveTint>,
) -> GlassPane {
    let (x, y, w, h) = rect;
    GlassPane {
        body: Some(fenestra_core::GlassBody {
            rect: vello::kurbo::Rect::new(
                f64::from(x),
                f64::from(y),
                f64::from(x) + f64::from(w),
                f64::from(y) + f64::from(h),
            ),
            tint: Some(tint),
            sheen,
            adaptive,
        }),
        ..pane(rect, 8.0, 1.0)
    }
}

/// The CPU final pass for a pane: adapt the tint to the frosted backdrop,
/// then bake the body over it.
fn cpu_body(img: &RgbaImage, p: &GlassPane) -> RgbaImage {
    let frosted = image_data(&cpu_blur(img, p));
    let body = p.body.expect("a body");
    let tint = match (body.tint, body.adaptive) {
        (Some(t), Some(a)) => fenestra_core::adapted_glass_tint(t, a, &frosted),
        (t, _) => t,
    };
    rgba(&fenestra_core::glass::bake_glass_body(
        &frosted,
        body.rect,
        tint,
        body.sheen.as_ref(),
    ))
}

#[test]
fn the_body_matches_the_cpu_bake() {
    let img = backdrop(320, 240);
    let tint = fenestra_core::oklch(0.35, 0.01, 250.0).with_alpha(0.72);
    let sheen = fenestra_core::Sheen {
        light_deg: 180.0,
        top: 0.12,
        bottom: 0.06,
    };
    let p = bodied((60, 40, 200, 160), tint, Some(sheen), None);
    let gpu = gpu(&mut headless(), &img, &[p]).expect("a result");
    assert_close(&gpu, &cpu_body(&img, &p), "tint and sheen");
}

#[test]
fn adaptive_tint_matches_the_cpu_over_dark_and_light() {
    let tint = fenestra_core::oklch(0.6, 0.03, 250.0).with_alpha(0.6);
    let adaptive = Some(fenestra_core::AdaptiveTint::glass());
    let p = bodied((20, 20, 120, 80), tint, None, adaptive);
    let mut h = headless();
    let mut means = Vec::new();
    for (name, v) in [("dark", [10, 10, 12]), ("light", [245, 245, 240])] {
        let img = flat(160, 120, v);
        let gpu = gpu(&mut h, &img, &[p]).expect("a result");
        assert_close(&gpu, &cpu_body(&img, &p), name);
        let unadapted = bodied((20, 20, 120, 80), tint, None, None);
        let plain = cpu_body(&img, &unadapted);
        let luma = |i: &RgbaImage| i.pixels().map(|p| f64::from(p.0[1])).sum::<f64>();
        means.push(luma(&gpu) - luma(&plain));
    }
    assert!(
        means[0] > 0.0 && means[1] < 0.0,
        "over dark the pane lightens, over light it darkens: {means:?}"
    );
}

#[test]
fn a_tint_out_of_gamut_after_the_shift_is_mapped_like_the_cpu() {
    let tint = fenestra_core::oklch(0.6, 0.25, 30.0).with_alpha(0.8);
    let adaptive = Some(fenestra_core::AdaptiveTint::glass());
    let p = bodied((20, 20, 120, 80), tint, None, adaptive);
    let img = flat(160, 120, [8, 8, 8]);
    let gpu = gpu(&mut headless(), &img, &[p]).expect("a result");
    assert_close(&gpu, &cpu_body(&img, &p), "saturated tint over black");
}

/// Each render pass costs a mobile tiler about a millisecond whatever it
/// does, so a pane takes a fixed few: blur across (a small blur reading the
/// backdrop directly; a large one after a downsample, as a tap per block is
/// cheaper than re-averaging each block for every tap), blur down, (the
/// tint,) then everything else at once.
#[test]
fn a_pane_takes_three_to_five_passes() {
    let img = backdrop(320, 240);
    let mut h = headless();
    let (device, queue, raster) = h.raster_gpu().expect("raster backend");
    let source = upload(device, queue, &img);
    let tint = fenestra_core::oklch(0.6, 0.03, 250.0).with_alpha(0.6);
    let rim = |p: GlassPane| GlassPane {
        corner_radius: 16.0,
        edge_band: 7.0,
        lensed: true,
        ..p
    };
    let tinted = |std_dev| GlassPane {
        std_dev,
        ..rim(bodied(
            (60, 40, 200, 160),
            tint,
            Some(fenestra_core::Sheen::glass()),
            Some(fenestra_core::AdaptiveTint::glass()),
        ))
    };
    for (std_dev, plain, with_tint) in [(4.0, 3, 4), (30.0, 4, 5)] {
        let p = rim(pane((60, 40, 200, 160), std_dev, 1.5));
        let (_, stats) = raster.glass(device, queue, &source, &[p]);
        assert_eq!(stats.passes, plain, "σ {std_dev}, no body");
        let (_, stats) = raster.glass(device, queue, &source, &[tinted(std_dev)]);
        assert_eq!(stats.passes, with_tint, "σ {std_dev}, adapted tint");
    }
}

/// Panes share their passes (an atlas per stage): a frame of several takes
/// no more than one pane needs, and each still matches the CPU.
#[test]
fn panes_share_their_passes() {
    let img = backdrop(320, 240);
    let mut h = headless();
    let (device, queue, raster) = h.raster_gpu().expect("raster backend");
    let source = upload(device, queue, &img);
    let tint = fenestra_core::oklch(0.35, 0.01, 250.0).with_alpha(0.72);
    let small = GlassPane {
        id: WidgetId(11),
        ..pane((10, 10, 90, 70), 4.0, 1.0)
    };
    let large = GlassPane {
        id: WidgetId(12),
        ..pane((120, 20, 180, 90), 30.0, 1.5)
    };
    let tinted = GlassPane {
        id: WidgetId(13),
        std_dev: 30.0,
        ..bodied(
            (40, 130, 220, 90),
            tint,
            None,
            Some(fenestra_core::AdaptiveTint::glass()),
        )
    };
    let panes = [small, large, tinted];
    let (out, stats) = raster.glass(device, queue, &source, &panes);
    assert_eq!(stats.panes, 3);
    assert_eq!(stats.passes, 5, "downsample, across, down, tint, compose");
    let read = |p: &GlassPane| {
        let placeholder = &out[&p.id];
        let pixels = raster
            .read_registered(device, queue, placeholder)
            .expect("registered");
        RgbaImage::from_raw(placeholder.width, placeholder.height, pixels).expect("size")
    };
    assert_close(&read(&small), &cpu_blur(&img, &small), "small");
    assert_close(&read(&large), &cpu_blur(&img, &large), "large");
    assert_close(&read(&tinted), &cpu_body(&img, &tinted), "tinted");
}

/// The GPU glass stands on its own: no raster renderer, only a device, a
/// backdrop texture and the panes (the vello backend drives it this way).
#[test]
fn gpu_glass_runs_on_its_own() {
    let img = backdrop(320, 240);
    let mut h = headless();
    let (device, queue, _) = h.raster_gpu().expect("raster backend");
    let source = upload(device, queue, &img);
    let p = pane((60, 40, 200, 160), 12.0, 1.5);
    let mut glass = fenestra_raster::GpuGlass::default();
    let (results, stats) = glass.run(device, queue, &source, &[p]);
    assert_eq!(stats.panes, 1);
    let r = results.iter().find(|r| r.id == p.id).expect("a result");
    assert_eq!((r.placeholder.width, r.placeholder.height), (200, 160));
    let [x, y, w, h] = r.region;
    let got = crate::headless::read_texture(device, queue, &r.texture, (x, y, w, h)).expect("read");
    assert_close(&got, &cpu_blur(&img, &p), "standalone");
}

/// A vello renderer for `device`, as the shells build one.
fn vello_renderer(device: &wgpu::Device) -> vello::Renderer {
    vello::Renderer::new(
        device,
        vello::RendererOptions {
            use_cpu: false,
            antialiasing_support: vello::AaSupport::area_only(),
            num_init_threads: std::num::NonZeroUsize::new(1),
            pipeline_cache: None,
        },
    )
    .expect("a vello renderer")
}

/// A window's surface can come back on another adapter: its vello glass
/// must rebuild there (not bind the old device's pipelines and atlases) and
/// withdraw what it handed the old device's renderer.
#[test]
fn vello_glass_follows_the_window_to_another_device() {
    let img = backdrop(320, 240);
    let p = pane((60, 40, 200, 160), 12.0, 1.5);
    let (mut a, mut b) = (headless(), headless());
    let mut glass = crate::vello_glass::VelloGlass::default();
    let (device_a, queue_a, _) = a.raster_gpu().expect("raster backend");
    let (device_b, queue_b, _) = b.raster_gpu().expect("raster backend");
    let mut renderers = vec![
        Some(vello_renderer(device_a)),
        Some(vello_renderer(device_b)),
    ];
    let source_a = upload(device_a, queue_a, &img);
    let source_b = upload(device_b, queue_b, &img);
    let out = glass.composite(
        (0, device_a, queue_a),
        &mut renderers,
        &source_a,
        (0, 0),
        &[p],
    );
    assert_eq!(out.len(), 1);
    let out = glass.composite(
        (1, device_b, queue_b),
        &mut renderers,
        &source_b,
        (0, 0),
        &[p],
    );
    assert_eq!(out.len(), 1, "composited on the new device");
    assert_eq!(glass.registered(), 1, "and registered only there");
}
