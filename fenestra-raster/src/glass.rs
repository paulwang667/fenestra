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
/// The adapted tint, at full float.
const SUMS: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
/// The result, as the image cache keeps images: premultiplied RGBA8.
const RESULT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The longest kernel radius the uniform block holds (`Params::w`).
const MAX_KERNEL: usize = 255;
/// About this many low texels a side feed the adaptive tint's mean.
const LUMA_SAMPLES: u32 = 48;

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
                        has_dynamic_offset: false,
                        min_binding_size: None,
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

/// One pane's textures, kept while its sizes hold.
struct PaneTextures {
    size: (u32, u32),
    low: (u32, u32),
    /// The low field blurred across (linear light).
    low_a: wgpu::Texture,
    /// The block means (a downsampled blur), then the field blurred down
    /// (sRGB, vibrancy applied).
    low_b: wgpu::Texture,
    /// The 1×1 tint the body composites, adapted or not.
    tint: wgpu::Texture,
    result: wgpu::Texture,
    placeholder: ImageData,
    last_used: u64,
}

/// The GPU glass state the renderer keeps: pipelines (built on first use)
/// and each pane's textures.
#[derive(Default)]
pub(crate) struct Glass {
    pipes: Option<Pipes>,
    panes: HashMap<WidgetId, PaneTextures>,
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
    w: [[f32; 4]; 64],
}

impl Params {
    fn new(a: [i32; 4], b: [i32; 4], f: [f32; 4], g: [f32; 4]) -> Self {
        Self {
            a,
            b,
            f,
            g,
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

impl Glass {
    /// Composites `panes` from `source` and submits the work. Returns each
    /// pane's placeholder and its texture view (to register), and the stats.
    /// `source` is RGBA8, and must allow its sRGB view (`view_formats`).
    pub(crate) fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &wgpu::Texture,
        panes: &[GlassPane],
    ) -> (Vec<(WidgetId, ImageData, wgpu::TextureView)>, GlassStats) {
        self.frame += 1;
        let frame = self.frame;
        let pipes = self.pipes.get_or_insert_with(|| Pipes::new(device));
        // Read through the sRGB view: loads and bilinear taps then come back
        // in linear light, decoded before they are averaged.
        let source_view = source.create_view(&wgpu::TextureViewDescriptor {
            format: Some(source.format().add_srgb_suffix()),
            ..Default::default()
        });
        let (sw, sh) = (source.width(), source.height());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("fenestra-raster glass"),
        });
        let mut stats = GlassStats::default();
        let mut out = Vec::new();
        for pane in panes {
            let (x, y, w, h) = pane.rect;
            if w == 0 || h == 0 {
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
            let f = downsample_factor(r);
            let low = (cw.div_ceil(f), ch.div_ceil(f));
            let radii = if f == 1 {
                [r; 3]
            } else {
                let sigma_sq = fl(r) * fl(r + 1);
                three_box_radii((sigma_sq / fl(f * f) - 0.25).max(0.0))
                    .map(|k| k.min(low.0.max(low.1)))
            };
            let (radius, weights) = kernel(radii);

            let stale = self
                .panes
                .get(&pane.id)
                .is_none_or(|t| t.size != (w, h) || t.low != low);
            if stale {
                stats.textures_created += 4;
                self.panes.insert(
                    pane.id,
                    PaneTextures {
                        size: (w, h),
                        low,
                        low_a: texture(device, low, FIELD, wgpu::TextureUsages::empty()),
                        low_b: texture(device, low, FIELD, wgpu::TextureUsages::empty()),
                        tint: texture(device, (1, 1), SUMS, wgpu::TextureUsages::empty()),
                        result: texture(device, (w, h), RESULT, wgpu::TextureUsages::COPY_SRC),
                        placeholder: crate::texture_placeholder(w, h),
                        last_used: frame,
                    },
                );
            }
            let tex = self.panes.get_mut(&pane.id).expect("inserted above");
            tex.last_used = frame;
            let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
            let (low_a, low_b) = (view(&tex.low_a), view(&tex.low_b));
            let tint = view(&tex.tint);
            let result = view(&tex.result);
            let origin = [x - cx0, y - cy0];
            let mut pass = |pipeline: &wgpu::RenderPipeline,
                            params: Params,
                            input: &wgpu::TextureView,
                            second: Option<&wgpu::TextureView>,
                            output: &wgpu::TextureView| {
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("fenestra-raster glass params"),
                    contents: bytemuck::bytes_of(&params),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("fenestra-raster glass"),
                    layout: &pipes.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(input),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            // Only the last pass reads it; the rest take any
                            // non-filtered view.
                            resource: wgpu::BindingResource::TextureView(second.unwrap_or(&tint)),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::Sampler(&pipes.sampler),
                        },
                    ],
                });
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
                rp.set_bind_group(0, &bind, &[]);
                rp.draw(0..3, 0..1);
                stats.passes += 1;
            };
            let with_weights = |mut p: Params| {
                p.w = weights;
                p
            };
            let crop = [cx0, cy0, i(cw), i(ch)];
            if f > 1 {
                pass(
                    &pipes.downsample,
                    Params::new(crop, [i(f), 0, 0, 0], [0.0; 4], [0.0; 4]),
                    &source_view,
                    None,
                    &low_b,
                );
            }
            pass(
                &pipes.blur_h,
                with_weights(Params::new(
                    crop,
                    [i(f), i(low.0), i(low.1), radius],
                    [0.0; 4],
                    [0.0; 4],
                )),
                if f > 1 { &low_b } else { &source_view },
                None,
                &low_a,
            );
            pass(
                &pipes.blur_v,
                with_weights(Params::new(
                    [radius, i(low.1), 0, 0],
                    [0; 4],
                    [pane.saturation, 0.0, 0.0, 0.0],
                    [0.0; 4],
                )),
                &low_a,
                None,
                &low_b,
            );
            // The tint the body composites, adapted to the frosted field's
            // mean luma (`adapt_fill`) on the GPU, in this same submission.
            let body = pane.body.as_ref();
            let tinted = body.and_then(|b| b.tint.map(|c| (c, b.adaptive)));
            if let Some((color, adaptive)) = tinted {
                // The low texels under the pane, sampled about
                // `LUMA_SAMPLES` a side.
                let fi = i(f);
                let lo = [origin[0] / fi, origin[1] / fi];
                let hi = [
                    ((origin[0] + i(w) - 1) / fi + 1).min(i(low.0)),
                    ((origin[1] + i(h) - 1) / fi + 1).min(i(low.1)),
                ];
                #[expect(clippy::cast_sign_loss, reason = "hi > lo")]
                let extent = ((hi[0] - lo[0]).max(hi[1] - lo[1]).max(1)) as u32;
                let step = i(extent.div_ceil(LUMA_SAMPLES));
                let (on, pivot, gain) = adaptive.map_or((0, 0.0, 0.0), |a| (1, a.pivot, a.gain));
                pass(
                    &pipes.tint,
                    Params::new(
                        [lo[0], lo[1], hi[0], hi[1]],
                        [on, step, 0, 0],
                        color.components,
                        [pivot, gain, 0.0, 0.0],
                    ),
                    &low_b,
                    Some(&low_b),
                    &tint,
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
                | i32::from(tinted.is_some()) << 2
                | i32::from(sheen.is_some()) << 3;
            let (sheen_origin, step, top, bottom) = sheen.unwrap_or_default();
            #[expect(clippy::cast_possible_truncation, reason = "gradient coordinates")]
            let line = [sheen_origin as f32, step.0 as f32, step.1 as f32];
            pass(
                &pipes.compose,
                Params::new(
                    [i(w), i(h), origin[0], origin[1]],
                    [i(f), i(low.0), i(low.1), flags],
                    [pane.corner_radius, pane.edge_band, line[0], line[1]],
                    [line[2], top, bottom, 0.0],
                ),
                &low_b,
                Some(&tint),
                &result,
            );
            stats.panes += 1;
            out.push((pane.id, tex.placeholder.clone(), result));
        }
        queue.submit([encoder.finish()]);
        self.panes.retain(|_, t| frame - t.last_used <= KEEP_FRAMES);
        (out, stats)
    }

    /// Panes holding textures.
    pub(crate) fn len(&self) -> usize {
        self.panes.len()
    }

    /// The result texture registered under `placeholder`, if a pane holds it.
    pub(crate) fn result_of(&self, placeholder: &ImageData) -> Option<&wgpu::Texture> {
        self.panes
            .values()
            .find(|t| t.placeholder.data.id() == placeholder.data.id())
            .map(|t| &t.result)
    }
}
