//! Frosted glass on the GPU: the backdrop blur (in linear light, downsampled
//! when the radius allows), vibrancy, the rim's refraction and the body (tint
//! adapted to the backdrop, sheen, dither) — the shell's CPU path
//! (`fenestra-shell` `blur.rs` / `multi_pass.rs`, core's `bake_body` and
//! `adapt_fill`) within the goldens' tolerance, with no readback. A pane takes
//! three to five render passes (a mobile tiler pays about a millisecond for
//! each, whatever it does — see glass.wgsl). Each pane's result is a texture
//! registered under a placeholder image, which the final pass draws like any
//! other.

use std::collections::HashMap;

use fenestra_core::{GlassBody, WidgetId};
use peniko::ImageData;
use wgpu::util::DeviceExt;

/// The smallest box radius a downsampled blur may run at. Below this the
/// three-box kernel is too coarse a stand-in for a Gaussian at the low
/// resolution, and the bilinear upsample starts to show its grid.
pub const MIN_LOW_RADIUS: u32 = 4;

/// The largest downsample factor. At 4 a 2x-scale backdrop is blurred at half
/// its logical resolution, which a σ ≥ 16 blur cannot tell from full.
pub const MAX_DOWNSAMPLE: u32 = 4;

/// The largest downsample factor the GPU glass uses. A blur that leaves a
/// low radius of [`MIN_LOW_RADIUS`] at 8× looks the same at 8× as at 4×
/// (the CPU path's cap), and runs on a quarter of the texels with half the
/// taps — on a phone, about two milliseconds of a full-screen pane.
pub const GPU_MAX_DOWNSAMPLE: u32 = 8;

/// Frames a pane's textures outlive its last use.
const KEEP_FRAMES: u64 = 60;

/// The box radius whose three passes best approximate a Gaussian of standard
/// deviation `sigma` (physical px): the variance of three box passes of
/// radius `r` is `r(r+1)`, so solve `r(r+1) = sigma²` for the nearest
/// non-negative integer. `sigma <= ~0.4` rounds to `0` (no blur).
#[must_use]
pub fn box_radius_for_std_dev(sigma: f32) -> u32 {
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

/// How far past a glass pane its backdrop blur reads, in physical px: the
/// three-box kernel reaches `3r`.
#[must_use]
pub fn blur_reach(std_dev: f32) -> u32 {
    3 * box_radius_for_std_dev(std_dev) + 1
}

/// The power-of-two factor a blur of box `radius` downsamples by: the
/// largest one (up to [`MAX_DOWNSAMPLE`]) that still leaves a low-resolution
/// radius of at least [`MIN_LOW_RADIUS`]. Small blurs get `1` and take the
/// exact full-resolution path.
#[must_use]
pub fn downsample_factor(radius: u32) -> u32 {
    let mut f = 1;
    while f < MAX_DOWNSAMPLE && radius / (f * 2) >= MIN_LOW_RADIUS {
        f *= 2;
    }
    f
}

/// [`downsample_factor`] for the GPU glass: the same rule, up to
/// [`GPU_MAX_DOWNSAMPLE`].
#[must_use]
pub fn gpu_downsample_factor(radius: u32) -> u32 {
    let mut f = 1;
    while f < GPU_MAX_DOWNSAMPLE && radius / (f * 2) >= MIN_LOW_RADIUS {
        f *= 2;
    }
    f
}

/// Radii for three box passes whose combined variance best matches `var`
/// (px²). A box of radius `k` contributes `k(k+1)/3`, so a single integer
/// radius for all three only hits `var` to within `2(k+1)/3` — at the small
/// radii a downsampled blur runs at, a tenth of the spread. Mixing `k` and
/// `k + 1` across the passes cuts that step to a third.
#[must_use]
pub fn three_box_radii(var: f32) -> [u32; 3] {
    let k = box_radius_for_std_dev(var.sqrt());
    // `box_radius_for_std_dev` rounds; step back to the radius at or below.
    let k = if fl(k) * fl(k + 1) > var {
        k.saturating_sub(1)
    } else {
        k
    };
    let step = 2.0 * fl(k + 1) / 3.0;
    let base = fl(k) * fl(k + 1);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "rounded and clamped to 0..=3"
    )]
    let up = ((var - base) / step).round().clamp(0.0, 3.0) as usize;
    std::array::from_fn(|i| if i < up { k + 1 } else { k })
}

#[expect(clippy::cast_precision_loss, reason = "small radii and sizes")]
fn fl(v: u32) -> f32 {
    v as f32
}

/// One glass pane for [`crate::RasterRenderer::glass`], in device pixels.
#[derive(Debug, Clone, Copy)]
pub struct GlassPane {
    pub id: WidgetId,
    /// The pane's rect `(x, y, w, h)` in device px; it may reach past the
    /// source, whose edge pixels then stand in for what is off it.
    pub rect: (i32, i32, u32, u32),
    /// Gaussian standard deviation, device px.
    pub std_dev: f32,
    /// Vibrancy, as [`fenestra_core::Style::backdrop_saturate`].
    pub saturation: f32,
    /// Corner radius, device px.
    pub corner_radius: f32,
    /// The refracting edge band, device px.
    pub edge_band: f32,
    /// Bend the rim: only when the pane lies wholly inside the source.
    pub lensed: bool,
    /// The body to composite; `None` leaves the frosted backdrop as it is.
    pub body: Option<GlassBody>,
    /// Logical → device scale (to lay the sheen's geometry).
    pub scale: f64,
}

/// What a [`crate::RasterRenderer::glass`] call did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GlassStats {
    /// Panes composited.
    pub panes: u32,
    /// Render passes encoded.
    pub passes: u32,
    /// Textures created (not reused from the last frame).
    pub textures_created: u32,
}

/// A float texture the passes read and write.
const FIELD: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The adapted tints, at full float.
const SUMS: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
/// The results, as the image cache keeps images: premultiplied RGBA8.
const RESULT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The longest kernel radius the uniform block holds (`Params::w`).
const MAX_KERNEL: usize = 255;
/// About this many low texels a side feed an adaptive tint's mean (one
/// fragment reads them all, one after another).
const LUMA_SAMPLES: u32 = 12;
/// Atlases grow in steps of this many texels a side.
const ATLAS_STEP: u32 = 256;

struct Pipes {
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    downsample: wgpu::RenderPipeline,
    blur_h: wgpu::RenderPipeline,
    blur_v: wgpu::RenderPipeline,
    tint: wgpu::RenderPipeline,
    compose: wgpu::RenderPipeline,
}

impl Pipes {
    fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fenestra-raster glass"),
            source: wgpu::ShaderSource::Wgsl(include_str!("glass.wgsl").into()),
        });
        let texture_entry = |binding, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fenestra-raster glass"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        // One buffer holds every pane's parameters.
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(PARAMS_SIZE),
                    },
                    count: None,
                },
                texture_entry(1, true),
                texture_entry(2, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fenestra-raster glass"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipe = |entry: &str, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("fenestra-raster glass"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            downsample: pipe("fs_downsample", FIELD),
            blur_h: pipe("fs_blur_h", FIELD),
            blur_v: pipe("fs_blur_v", FIELD),
            tint: pipe("fs_tint", SUMS),
            compose: pipe("fs_compose", RESULT),
            sampler,
            layout,
        }
    }
}

/// One page of atlases: every pane of a frame that fits shares it, so the
/// frame takes one pass per stage, not one per pane per stage.
struct Page {
    /// The low fields blurred across (linear light).
    low_a: wgpu::Texture,
    /// The block means (downsampled blurs), then the fields blurred down
    /// (sRGB, vibrancy applied).
    low_b: wgpu::Texture,
    /// One adapted tint per tinted pane, in a row.
    tint: wgpu::Texture,
    /// The results, each with a one-pixel border of its edge pixels.
    result: wgpu::Texture,
}

/// A pane's placeholder image, kept while its size holds.
struct Placeholder {
    image: ImageData,
    size: (u32, u32),
    last_used: u64,
}

/// The GPU glass: its pipelines (built on first use), the atlas pages, each
/// pane's placeholder and where its result lies. The raster backend keeps
/// one and draws the results through its image cache; the vello backend
/// keeps one per window and hands the results to vello as image overrides.
#[derive(Default)]
pub struct GpuGlass {
    pipes: Option<Pipes>,
    pages: Vec<Page>,
    placeholders: HashMap<WidgetId, Placeholder>,
    /// This frame's results: placeholder blob id → (page, region).
    regions: HashMap<u64, (usize, [u32; 4])>,
    frame: u64,
}

/// The uniform block every pass reads (see `Params` in glass.wgsl).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    a: [i32; 4],
    b: [i32; 4],
    f: [f32; 4],
    g: [f32; 4],
    o: [i32; 4],
    w: [[f32; 4]; 64],
}

const PARAMS_SIZE: u64 = std::mem::size_of::<Params>() as u64;

impl Params {
    fn new(a: [i32; 4], b: [i32; 4], f: [f32; 4], g: [f32; 4], o: [i32; 4]) -> Self {
        Self {
            a,
            b,
            f,
            g,
            o,
            w: [[0.0; 4]; 64],
        }
    }
}

/// The three box passes of half-widths `radii` as one kernel: its radius
/// and its weights for offsets `0..=radius` (symmetric), packed four to a
/// vector. The CPU runs the boxes one after another, each clamping at the
/// field's edge; one kernel clamping once differs only within its reach of
/// the crop's edge, which sits a full reach outside the pane except where
/// the crop meets the frame. A kernel past [`MAX_KERNEL`] is cut there.
fn kernel(radii: [u32; 3]) -> (i32, [[f32; 4]; 64]) {
    let mut taps = vec![1.0_f64];
    for k in radii {
        let width = 2 * k as usize + 1;
        let mut next = vec![0.0; taps.len() + width - 1];
        for (i, t) in taps.iter().enumerate() {
            for n in &mut next[i..i + width] {
                *n += t / width as f64;
            }
        }
        taps = next;
    }
    let full = taps.len() / 2;
    let radius = full.min(MAX_KERNEL);
    let kept = &taps[full - radius..=full + radius];
    let total: f64 = kept.iter().sum();
    let mut w = [[0.0; 4]; 64];
    for (t, v) in kept[radius..].iter().enumerate() {
        #[expect(clippy::cast_possible_truncation, reason = "a weight in 0..=1")]
        let v = (v / total) as f32;
        w[t / 4][t % 4] = v;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "≤ 255"
    )]
    let radius = radius as i32;
    (radius, w)
}

#[expect(
    clippy::cast_possible_wrap,
    reason = "texture sizes and offsets fit in i32"
)]
fn i(v: u32) -> i32 {
    v as i32
}

fn texture(
    device: &wgpu::Device,
    (w, h): (u32, u32),
    format: wgpu::TextureFormat,
    extra: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fenestra-raster glass"),
        size: wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | extra,
        view_formats: &[],
    })
}

/// A shelf packer: rectangles left to right in rows of at most `width`.
struct Shelves {
    width: u32,
    x: u32,
    y: u32,
    row: u32,
    /// The extent used so far.
    used: (u32, u32),
}

impl Shelves {
    fn new(width: u32) -> Self {
        Self {
            width,
            x: 0,
            y: 0,
            row: 0,
            used: (0, 0),
        }
    }

    /// Where a `w`×`h` rectangle goes, and the extent with it — not yet
    /// taken (see [`Self::take`]).
    fn place(&self, (w, h): (u32, u32)) -> ((u32, u32), (u32, u32)) {
        let (x, y) = if self.x + w > self.width {
            (0, self.y + self.row)
        } else {
            (self.x, self.y)
        };
        let used = (self.used.0.max(x + w), self.used.1.max(y + h));
        ((x, y), used)
    }

    fn take(&mut self, (w, h): (u32, u32)) -> (u32, u32) {
        let ((x, y), used) = self.place((w, h));
        if y != self.y {
            self.row = 0;
        }
        self.x = x + w;
        self.y = y;
        self.row = self.row.max(h);
        self.used = used;
        (x, y)
    }
}

/// A composited pane, as [`GpuGlass::run`] returns it.
#[derive(Debug, Clone)]
pub struct GlassResult {
    pub id: WidgetId,
    /// The image to draw where the pane's frosted body goes: its pixels are
    /// never read; it stands for `region` of `texture`.
    pub placeholder: ImageData,
    /// The atlas the result lies in (`Rgba8Unorm`, premultiplied,
    /// `COPY_SRC`).
    pub texture: wgpu::Texture,
    /// Where in `texture`: `[x, y, w, h]`, texels.
    pub region: [u32; 4],
    /// The same, as `[u0, v0, u1, v1]`.
    pub uv: [f32; 4],
}

/// One pane's share of a frame's passes.
struct Job {
    pane: usize,
    page: usize,
    crop: [i32; 4],
    f: u32,
    low: (u32, u32),
    radius: i32,
    weights: [[f32; 4]; 64],
    /// The pane's top-left in its crop.
    origin: [i32; 2],
    /// Its low field's origin in the low atlases.
    low_at: (u32, u32),
    /// Its result region's origin (border included) in the result atlas.
    result_at: (u32, u32),
    /// Its texel in the tint row, if it has a tint.
    tint_at: Option<u32>,
}

/// A page's extents, as packed.
#[derive(Clone, Copy, Default)]
struct Extents {
    low: (u32, u32),
    result: (u32, u32),
    tints: u32,
}

impl GpuGlass {
    /// Composites `panes` from `source` and submits the work: the panes
    /// share one set of passes. Returns each pane's result (its placeholder,
    /// and where in which texture it lies) and the stats. `source` is
    /// `Rgba8Unorm` with `TEXTURE_BINDING`, and must allow its
    /// `Rgba8UnormSrgb` view (`view_formats`). Nothing is read back.
    #[expect(clippy::too_many_lines, reason = "one frame's passes, in order")]
    pub fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &wgpu::Texture,
        panes: &[GlassPane],
    ) -> (Vec<GlassResult>, GlassStats) {
        self.frame += 1;
        let frame = self.frame;
        self.regions.clear();
        let pipes = self.pipes.get_or_insert_with(|| Pipes::new(device));
        let max = device.limits().max_texture_dimension_2d;
        let (sw, sh) = (source.width(), source.height());
        let mut stats = GlassStats::default();

        // Lay the panes out: their low fields in the low atlases, their
        // results in the result atlas, a new page when either would
        // outgrow the device.
        let widest = panes.iter().map(|p| p.rect.2).max().unwrap_or(0).max(sw);
        let atlas_width = max.min((widest + 2).next_multiple_of(ATLAS_STEP));
        let mut jobs: Vec<Job> = Vec::new();
        let mut extents: Vec<Extents> = vec![Extents::default()];
        let mut shelves = (Shelves::new(atlas_width), Shelves::new(atlas_width));
        for (n, pane) in panes.iter().enumerate() {
            let (x, y, w, h) = pane.rect;
            if w == 0 || h == 0 || w + 2 > atlas_width || h + 2 > max {
                continue;
            }
            // The crop the blur reads: the pane padded by the blur's reach,
            // cut to the source (the blur clamps at its edges, as the CPU
            // path clamps at its crop).
            let pad = i(blur_reach(pane.std_dev));
            let cx0 = (x - pad).clamp(0, i(sw));
            let cy0 = (y - pad).clamp(0, i(sh));
            let cx1 = (x + i(w) + pad).clamp(0, i(sw));
            let cy1 = (y + i(h) + pad).clamp(0, i(sh));
            if cx1 <= cx0 || cy1 <= cy0 {
                continue;
            }
            #[expect(clippy::cast_sign_loss, reason = "positive by the check above")]
            let (cw, ch) = ((cx1 - cx0) as u32, (cy1 - cy0) as u32);
            let r = box_radius_for_std_dev(pane.std_dev).min(cw.max(ch));
            let f = gpu_downsample_factor(r);
            let low = (cw.div_ceil(f), ch.div_ceil(f));
            if low.0 > atlas_width || low.1 > max {
                continue;
            }
            let radii = if f == 1 {
                [r; 3]
            } else {
                let sigma_sq = fl(r) * fl(r + 1);
                three_box_radii((sigma_sq / fl(f * f) - 0.25).max(0.0))
                    .map(|k| k.min(low.0.max(low.1)))
            };
            let (radius, weights) = kernel(radii);
            let bordered = (w + 2, h + 2);
            let (_, low_used) = shelves.0.place(low);
            let (_, result_used) = shelves.1.place(bordered);
            if low_used.1 > max || result_used.1 > max {
                extents.push(Extents::default());
                shelves = (Shelves::new(atlas_width), Shelves::new(atlas_width));
            }
            let page = extents.len() - 1;
            let low_at = shelves.0.take(low);
            let result_at = shelves.1.take(bordered);
            let e = &mut extents[page];
            e.low = shelves.0.used;
            e.result = shelves.1.used;
            let tinted = pane.body.as_ref().is_some_and(|b| b.tint.is_some());
            let tint_at = tinted.then(|| {
                e.tints += 1;
                e.tints - 1
            });
            jobs.push(Job {
                pane: n,
                page,
                crop: [cx0, cy0, i(cw), i(ch)],
                f,
                low,
                radius,
                weights,
                origin: [x - cx0, y - cy0],
                low_at,
                result_at,
                tint_at,
            });
        }

        // Grow the pages to fit (never shrink: a frame of one small pane
        // keeps last frame's room), and drop the ones no longer needed.
        let pages_used = if jobs.is_empty() { 0 } else { extents.len() };
        self.pages
            .truncate(pages_used.max(usize::from(!self.placeholders.is_empty())));
        for (n, e) in extents.iter().enumerate().take(pages_used) {
            let grow = |(w, h): (u32, u32)| {
                (
                    w.next_multiple_of(ATLAS_STEP).min(max),
                    h.next_multiple_of(ATLAS_STEP).min(max),
                )
            };
            let fits = |t: &wgpu::Texture, (w, h): (u32, u32)| t.width() >= w && t.height() >= h;
            let page = self.pages.get(n);
            let low_ok = page.is_some_and(|p| fits(&p.low_a, e.low));
            let result_ok = page.is_some_and(|p| fits(&p.result, e.result));
            let tint_ok = page.is_some_and(|p| p.tint.width() >= e.tints);
            if low_ok && result_ok && tint_ok {
                continue;
            }
            let old = page.map(|p| {
                (
                    (p.low_a.width(), p.low_a.height()),
                    (p.result.width(), p.result.height()),
                    p.tint.width(),
                )
            });
            let (low_size, result_size, tint_width) = old.unwrap_or_default();
            let low_size = grow((low_size.0.max(e.low.0), low_size.1.max(e.low.1)));
            let result_size = grow((result_size.0.max(e.result.0), result_size.1.max(e.result.1)));
            let tint_width = tint_width.max(e.tints).next_power_of_two().max(1);
            let new = Page {
                low_a: texture(device, low_size, FIELD, wgpu::TextureUsages::empty()),
                low_b: texture(device, low_size, FIELD, wgpu::TextureUsages::empty()),
                tint: texture(device, (tint_width, 1), SUMS, wgpu::TextureUsages::empty()),
                result: texture(device, result_size, RESULT, wgpu::TextureUsages::COPY_SRC),
            };
            stats.textures_created += 4;
            if n < self.pages.len() {
                self.pages[n] = new;
            } else {
                self.pages.push(new);
            }
        }

        // Every pane's parameters for every stage, in one buffer, each
        // block at an offset the device can bind.
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let stride = PARAMS_SIZE.next_multiple_of(align);
        let mut blocks: Vec<Params> = Vec::new();
        // Per page, per stage: (block, viewport, reads the backdrop).
        type Draw = (usize, [u32; 4], bool);
        let mut draws: Vec<[Vec<Draw>; 5]> = (0..pages_used).map(|_| Default::default()).collect();
        let mut out = Vec::new();
        for job in &jobs {
            let pane = &panes[job.pane];
            let (_, _, w, h) = pane.rect;
            let low_o = [i(job.low_at.0), i(job.low_at.1)];
            let low_rect = [job.low_at.0, job.low_at.1, job.low.0, job.low.1];
            let stage = &mut draws[job.page];
            let mut push = |s: usize, p: Params, rect: [u32; 4], from_source: bool| {
                stage[s].push((blocks.len(), rect, from_source));
                blocks.push(p);
            };
            let with_weights = |mut p: Params| {
                p.w = job.weights;
                p
            };
            let fi = i(job.f);
            if job.f > 1 {
                push(
                    0,
                    Params::new(
                        job.crop,
                        [fi, 0, 0, 0],
                        [0.0; 4],
                        [0.0; 4],
                        [low_o[0], low_o[1], low_o[0], low_o[1]],
                    ),
                    low_rect,
                    true,
                );
            }
            push(
                1,
                with_weights(Params::new(
                    job.crop,
                    [fi, i(job.low.0), i(job.low.1), job.radius],
                    [0.0; 4],
                    [0.0; 4],
                    [low_o[0], low_o[1], low_o[0], low_o[1]],
                )),
                low_rect,
                job.f == 1,
            );
            push(
                2,
                with_weights(Params::new(
                    [0, i(job.low.1), 0, 0],
                    [0, 0, 0, job.radius],
                    [pane.saturation, 0.0, 0.0, 0.0],
                    [0.0; 4],
                    [low_o[0], low_o[1], low_o[0], low_o[1]],
                )),
                low_rect,
                false,
            );
            // The tint the body composites, adapted to the frosted field's
            // mean luma (`adapt_fill`) on the GPU, in this same submission.
            let body = pane.body.as_ref();
            if let (Some(slot), Some(color)) = (job.tint_at, body.and_then(|b| b.tint)) {
                // The low texels under the pane, sampled about
                // `LUMA_SAMPLES` a side.
                let o = job.origin;
                let lo = [o[0] / fi, o[1] / fi];
                let hi = [
                    ((o[0] + i(w) - 1) / fi + 1).min(i(job.low.0)),
                    ((o[1] + i(h) - 1) / fi + 1).min(i(job.low.1)),
                ];
                #[expect(clippy::cast_sign_loss, reason = "hi > lo")]
                let extent = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1) as u32;
                let step = i(extent.div_ceil(LUMA_SAMPLES));
                let adaptive = body.and_then(|b| b.adaptive);
                let (on, pivot, gain) = adaptive.map_or((0, 0.0, 0.0), |a| (1, a.pivot, a.gain));
                push(
                    3,
                    Params::new(
                        [lo[0], lo[1], hi[0], hi[1]],
                        [on, step, 0, 0],
                        color.components,
                        [pivot, gain, 0.0, 0.0],
                        [i(slot), 0, low_o[0], low_o[1]],
                    ),
                    [slot, 0, 1, 1],
                    false,
                );
            }
            // Bend the rim — only for a pane wholly on screen (a clamped one
            // would lens its cut edge) and big enough to have a rim.
            let lensed = pane.lensed && w >= 4 && h >= 4;
            let sheen = body.and_then(|b| {
                b.sheen
                    .as_ref()
                    .map(|s| fenestra_core::glass::sheen_line(s, b.rect, w, h))
            });
            let flags = i32::from(lensed)
                | i32::from(body.is_some()) << 1
                | i32::from(job.tint_at.is_some()) << 2
                | i32::from(sheen.is_some()) << 3;
            let (sheen_origin, step, top, bottom) = sheen.unwrap_or_default();
            #[expect(clippy::cast_possible_truncation, reason = "gradient coordinates")]
            let line = [sheen_origin as f32, step.0 as f32, step.1 as f32];
            #[expect(clippy::cast_precision_loss, reason = "a small texel index")]
            let slot = job.tint_at.unwrap_or(0) as f32;
            let (rx, ry) = job.result_at;
            push(
                4,
                Params::new(
                    [i(w), i(h), job.origin[0], job.origin[1]],
                    [fi, i(job.low.0), i(job.low.1), flags],
                    [pane.corner_radius, pane.edge_band, line[0], line[1]],
                    [line[2], top, bottom, slot],
                    [i(rx) + 1, i(ry) + 1, low_o[0], low_o[1]],
                ),
                [rx, ry, w + 2, h + 2],
                false,
            );

            let held = self
                .placeholders
                .entry(pane.id)
                .and_modify(|p| {
                    if p.size != (w, h) {
                        *p = Placeholder {
                            image: crate::texture_placeholder(w, h),
                            size: (w, h),
                            last_used: frame,
                        };
                    }
                })
                .or_insert_with(|| Placeholder {
                    image: crate::texture_placeholder(w, h),
                    size: (w, h),
                    last_used: frame,
                });
            held.last_used = frame;
            let result = &self.pages[job.page].result;
            let (tw, th) = (fl(result.width()), fl(result.height()));
            let uv = [
                fl(rx + 1) / tw,
                fl(ry + 1) / th,
                fl(rx + 1 + w) / tw,
                fl(ry + 1 + h) / th,
            ];
            self.regions
                .insert(held.image.data.id(), (job.page, [rx + 1, ry + 1, w, h]));
            out.push(GlassResult {
                id: pane.id,
                placeholder: held.image.clone(),
                texture: result.clone(),
                region: [rx + 1, ry + 1, w, h],
                uv,
            });
            stats.panes += 1;
        }
        self.placeholders
            .retain(|_, p| frame - p.last_used <= KEEP_FRAMES);
        if self.placeholders.is_empty() {
            self.pages.clear();
        }
        if jobs.is_empty() {
            return (out, stats);
        }

        let mut bytes = vec![0u8; usize::try_from(stride).unwrap_or(0) * blocks.len()];
        for (n, b) in blocks.iter().enumerate() {
            let at = usize::try_from(stride).unwrap_or(0) * n;
            bytes[at..at + std::mem::size_of::<Params>()].copy_from_slice(bytemuck::bytes_of(b));
        }
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fenestra-raster glass params"),
            contents: &bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        // Read through the sRGB view: loads and bilinear taps then come back
        // in linear light, decoded before they are averaged.
        let source_view = source.create_view(&wgpu::TextureViewDescriptor {
            format: Some(source.format().add_srgb_suffix()),
            ..Default::default()
        });
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = |input: &wgpu::TextureView, second: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fenestra-raster glass"),
                layout: &pipes.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &buffer,
                            offset: 0,
                            size: wgpu::BufferSize::new(PARAMS_SIZE),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(input),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(second),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&pipes.sampler),
                    },
                ],
            })
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("fenestra-raster glass"),
        });
        for (page, stages) in self.pages.iter().zip(&draws) {
            let (low_a, low_b) = (view(&page.low_a), view(&page.low_b));
            let (tint, result) = (view(&page.tint), view(&page.result));
            // Per stage: the pipeline, its output, what its panes read (the
            // backdrop or the stage's input atlas) and its second input.
            let plan: [(
                &wgpu::RenderPipeline,
                &wgpu::TextureView,
                &wgpu::TextureView,
                &wgpu::TextureView,
            ); 5] = [
                (&pipes.downsample, &low_b, &source_view, &tint),
                (&pipes.blur_h, &low_a, &low_b, &tint),
                (&pipes.blur_v, &low_b, &low_a, &tint),
                (&pipes.tint, &tint, &low_b, &low_b),
                (&pipes.compose, &result, &low_b, &tint),
            ];
            for ((pipeline, output, input, second), stage) in plan.iter().zip(stages) {
                if stage.is_empty() {
                    continue;
                }
                let from_atlas = bind(input, second);
                let from_source = bind(&source_view, second);
                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("fenestra-raster glass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: output,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                rp.set_pipeline(pipeline);
                for &(block, [x, y, w, h], reads_source) in stage {
                    #[expect(clippy::cast_possible_truncation, reason = "a uniform offset")]
                    let offset = (stride * block as u64) as u32;
                    let group = if reads_source {
                        &from_source
                    } else {
                        &from_atlas
                    };
                    rp.set_bind_group(0, group, &[offset]);
                    rp.set_viewport(fl(x), fl(y), fl(w), fl(h), 0.0, 1.0);
                    rp.set_scissor_rect(x, y, w, h);
                    rp.draw(0..3, 0..1);
                }
                stats.passes += 1;
            }
        }
        queue.submit([encoder.finish()]);
        (out, stats)
    }

    /// Panes holding a result.
    #[must_use]
    pub fn len(&self) -> usize {
        self.placeholders.len()
    }

    /// No pane holds a result.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.placeholders.is_empty()
    }

    /// The result texture registered under `placeholder` this frame, and the
    /// region `[x, y, w, h]` of it the result is.
    #[must_use]
    pub fn result_of(&self, placeholder: &ImageData) -> Option<(&wgpu::Texture, [u32; 4])> {
        let (page, region) = self.regions.get(&placeholder.data.id())?;
        Some((&self.pages.get(*page)?.result, *region))
    }
}
