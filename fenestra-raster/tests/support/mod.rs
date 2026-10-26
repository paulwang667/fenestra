//! Shared test support: a GPU device, the same drawing rendered by vello and
//! by the raster backend, and a pixel comparison between the two.

#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::sync::OnceLock;

use fenestra_core::draw::{DrawTarget, SceneTarget};
use fenestra_raster::{RasterRenderer, RenderStats};
use peniko::Color;

/// The format both renderers draw into.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// One device for every test in the binary.
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

pub fn gpu() -> &'static Gpu {
    static GPU: OnceLock<Gpu> = OnceLock::new();
    GPU.get_or_init(|| {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .expect("a GPU adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .expect("a GPU device");
        Gpu { device, queue }
    })
}

/// A drawing both renderers can paint.
pub trait Paint {
    fn paint<T: DrawTarget>(&self, t: &mut T);
}

/// How far the raster image may stray from vello's: the fraction of pixels
/// whose largest channel difference exceeds `channel` must stay at or below
/// `max_frac`.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub channel: u8,
    pub max_frac: f64,
}

/// The budget for shapes: 4x MSAA quantizes edge coverage where vello
/// computes it exactly, so edge pixels may differ by up to ~1/4 coverage
/// (≈67/255 seen); a misplaced edge — half a pixel or more — differs by
/// ≥128/255 along its whole length and fails.
pub const EDGES: Budget = Budget {
    channel: 80,
    max_frac: 0.005,
};

/// The budget for color inside shapes: nearly exact (3/255), on all but
/// the edge pixels (≤5% of these test scenes). Paired with [`EDGES`] where a
/// test is about color (gradients, images), since `EDGES` alone would let a
/// wrong ramp through.
pub const FILLS: Budget = Budget {
    channel: 3,
    max_frac: 0.05,
};

/// A `width`×`height` RGBA8 image, row-major.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Image {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        self.pixels[i..i + 4].try_into().unwrap()
    }

    pub fn save(&self, path: &std::path::Path) {
        image::save_buffer(
            path,
            &self.pixels,
            self.width,
            self.height,
            image::ExtendedColorType::Rgba8,
        )
        .expect("write png");
    }
}

fn target(size: (u32, u32), usage: wgpu::TextureUsages) -> wgpu::Texture {
    gpu().device.create_texture(&wgpu::TextureDescriptor {
        label: Some("test target"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: usage | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// Copies `texture` back to the CPU.
pub fn read(texture: &wgpu::Texture) -> Image {
    let (device, queue) = (&gpu().device, &gpu().queue);
    let (width, height) = (texture.width(), texture.height());
    let row = (width * 4).next_multiple_of(256);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test readback"),
        size: u64::from(row) * u64::from(height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        let start = (y * row) as usize;
        pixels.extend_from_slice(&data[start..start + (width * 4) as usize]);
    }
    Image {
        width,
        height,
        pixels,
    }
}

/// `scene` rendered by vello over white.
pub fn render_vello(scene: &impl Paint, size: (u32, u32)) -> Image {
    let mut s = vello::Scene::new();
    let mut t = SceneTarget::new(&mut s);
    scene.paint(&mut t);
    t.finish();
    let texture = target(size, wgpu::TextureUsages::STORAGE_BINDING);
    let view = texture.create_view(&Default::default());
    // A renderer per render: a shared one, used from parallel test threads,
    // drew images white on a later render of the same scene (its image atlas
    // carried over between renders); a fresh one is cheap and always right.
    let mut vello = vello::Renderer::new(
        &gpu().device,
        vello::RendererOptions {
            use_cpu: false,
            antialiasing_support: vello::AaSupport::area_only(),
            num_init_threads: std::num::NonZeroUsize::new(1),
            pipeline_cache: None,
        },
    )
    .expect("vello renderer");
    vello
        .render_to_texture(
            &gpu().device,
            &gpu().queue,
            &s,
            &view,
            &vello::RenderParams {
                base_color: Color::WHITE,
                width: size.0,
                height: size.1,
                antialiasing_method: vello::AaConfig::Area,
            },
        )
        .expect("vello render");
    read(&texture)
}

/// `scene` rendered by the raster backend over white.
pub fn render_raster(
    renderer: &mut RasterRenderer,
    scene: &impl Paint,
    size: (u32, u32),
) -> (Image, RenderStats) {
    let texture = target(
        size,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
    );
    let view = texture.create_view(&Default::default());
    let mut frame = renderer.frame(size.0, size.1);
    scene.paint(&mut frame);
    let stats = frame.render(&gpu().device, &gpu().queue, &view, Color::WHITE);
    (read(&texture), stats)
}

/// The fraction of pixels whose largest channel difference exceeds `channel`.
pub fn diff_frac(a: &Image, b: &Image, channel: u8) -> f64 {
    let over = a
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.pixels.as_chunks::<4>().0.iter())
        .filter(|(p, q)| {
            p.iter()
                .zip(q.iter())
                .any(|(x, y)| x.abs_diff(*y) > channel)
        })
        .count();
    over as f64 / f64::from(a.width * a.height)
}

/// A fresh renderer for the shared device.
pub fn renderer() -> RasterRenderer {
    RasterRenderer::new(&gpu().device, FORMAT)
}

/// Renders `scene` both ways and fails (writing both images and a diff
/// next to the test binary's temp dir) when they differ beyond `budget`.
pub fn assert_like_vello(
    name: &str,
    scene: &impl Paint,
    size: (u32, u32),
    budget: Budget,
) -> RenderStats {
    assert_like_vello_with(&mut renderer(), name, scene, size, budget)
}

/// [`assert_like_vello`] against several budgets at once.
pub fn assert_like_vello_all(name: &str, scene: &impl Paint, size: (u32, u32), budgets: &[Budget]) {
    let mut r = renderer();
    for (i, budget) in budgets.iter().enumerate() {
        assert_like_vello_with(&mut r, &format!("{name}.{i}"), scene, size, *budget);
    }
}

/// [`assert_like_vello`] through a given renderer.
pub fn assert_like_vello_with(
    renderer: &mut RasterRenderer,
    name: &str,
    scene: &impl Paint,
    size: (u32, u32),
    budget: Budget,
) -> RenderStats {
    let reference = render_vello(scene, size);
    let (raster, stats) = render_raster(renderer, scene, size);
    let frac = diff_frac(&reference, &raster, budget.channel);
    if frac > budget.max_frac {
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
        reference.save(&dir.join(format!("{name}.vello.png")));
        raster.save(&dir.join(format!("{name}.raster.png")));
        let diff = Image {
            width: raster.width,
            height: raster.height,
            pixels: reference
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .zip(raster.pixels.as_chunks::<4>().0.iter())
                .flat_map(|(p, q)| {
                    let d = p
                        .iter()
                        .zip(q.iter())
                        .map(|(x, y)| x.abs_diff(*y))
                        .max()
                        .unwrap();
                    if d > budget.channel {
                        [255, 0, 0, 255]
                    } else {
                        [q[0] / 3 + 170, q[1] / 3 + 170, q[2] / 3 + 170, 255]
                    }
                })
                .collect(),
        };
        diff.save(&dir.join(format!("{name}.diff.png")));
        panic!(
            "{name}: {:.2}% of pixels differ by more than {}/255 (budget {:.2}%); images in {}",
            frac * 100.0,
            budget.channel,
            budget.max_frac * 100.0,
            dir.display()
        );
    }
    stats
}
