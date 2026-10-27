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
        view_formats: &[],
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
