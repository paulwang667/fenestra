//! A conventional GPU rasterizer for fenestra — the kind mobile GPUs and
//! drivers are built for. Shapes are tessellated to triangles (lyon) and
//! drawn with 4x MSAA; clips are a stencil; opacity groups render offscreen;
//! shadows are evaluated analytically; text comes from a glyph atlas.
//!
//! [`RasterRenderer`] owns everything that outlives a frame (pipelines,
//! attachments, caches). Each frame is recorded through a [`RasterFrame`],
//! which implements [`DrawTarget`], and submitted with
//! [`RasterFrame::render`].

mod geometry;
pub mod glass;
mod glyphs;
mod gpu;
mod images;
mod layers;
mod paint;

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use fenestra_core::draw::{DrawTarget, Glyph, GlyphRun};
use kurbo::{Affine, Point, Rect, Shape, Stroke};
use lyon_tessellation::{FillTessellator, StrokeTessellator};
use peniko::{BrushRef, Color, Fill, ImageData};
use wgpu::util::DeviceExt;

pub use glass::{GlassPane, GlassStats};

use geometry::{Mesh, PaintParams, QuadMesh, ShadowMesh};
use gpu::{Attachments, GLOBALS_STRIDE, Globals, Pipelines};
use layers::{LayerPool, LayerTarget};

/// A placeholder image to register a GPU texture under
/// ([`RasterRenderer::register_texture`]): a unique one-byte blob that only
/// carries the size.
pub fn texture_placeholder(width: u32, height: u32) -> ImageData {
    ImageData {
        data: peniko::Blob::from(vec![0u8]),
        format: peniko::ImageFormat::Rgba8,
        alpha_type: peniko::ImageAlphaType::AlphaPremultiplied,
        width,
        height,
    }
}

/// What one frame cost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    /// Draw calls issued.
    pub draws: u32,
    /// Vertices uploaded.
    pub vertices: u32,
    /// Offscreen layers rendered.
    pub layers: u32,
    /// Layer textures created (not reused from the pool).
    pub layer_textures_created: u32,
    /// Pixels of the offscreen layers rendered (their textures' area).
    pub layer_pixels: u64,
    /// Images uploaded to the GPU (not found in the cache).
    pub images_uploaded: u32,
    /// Glyphs rasterized into the atlas (not found in it).
    pub glyphs_rasterized: u32,
    /// Gradient ramps built (not found among the rows held).
    pub ramps_built: u32,
}

/// What the renderer keeps between frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheSizes {
    /// Pooled offscreen layer textures.
    pub layer_textures: usize,
    /// Cached image textures.
    pub images: usize,
    /// Gradient ramp rows in use.
    pub ramp_rows: usize,
    /// Glyph atlas pages.
    pub atlas_pages: usize,
    /// Glass panes holding GPU textures.
    pub glass_panes: usize,
}

/// Draws fenestra frames into `wgpu` textures of one format.
pub struct RasterRenderer {
    format: wgpu::TextureFormat,
    pipelines: Pipelines,
    fill: FillTessellator,
    stroke: StrokeTessellator,
    /// The root pass's MSAA attachments, rebuilt when the frame size changes.
    root: Option<Attachments>,
    ramps: paint::Ramps,
    pool: LayerPool,
    images: images::ImageCache,
    atlas: glyphs::Atlas,
    glass: glass::Glass,
    /// The device's largest texture side; bigger images are downscaled.
    max_texture_dimension: u32,
}

impl RasterRenderer {
    /// Builds the pipelines for targets of `format` (fenestra renders into
    /// `Rgba8Unorm`).
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self::with_max_texture_dimension(device, format, device.limits().max_texture_dimension_2d)
    }

    /// [`RasterRenderer::new`] with a smaller texture size cap than the
    /// device's, so tests can exercise image downscaling.
    #[doc(hidden)]
    pub fn with_max_texture_dimension(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        max_texture_dimension: u32,
    ) -> Self {
        Self {
            format,
            pipelines: Pipelines::new(device, format),
            fill: FillTessellator::new(),
            stroke: StrokeTessellator::new(),
            root: None,
            ramps: paint::Ramps::default(),
            pool: LayerPool::default(),
            images: images::ImageCache::default(),
            atlas: glyphs::Atlas::default(),
            glass: glass::Glass::default(),
            max_texture_dimension,
        }
    }

    /// Starts recording a `width`×`height` device-pixel frame.
    pub fn frame(&mut self, width: u32, height: u32) -> RasterFrame<'_> {
        self.ramps.begin_frame();
        self.pool.begin_frame();
        self.images.begin_frame();
        self.atlas.begin_frame();
        let size = (width.max(1), height.max(1));
        RasterFrame {
            renderer: self,
            size,
            stack: Vec::new(),
            mesh: Mesh::new(),
            quads: QuadMesh::new(),
            shadows: ShadowMesh::new(),
            layers: vec![Layer::root(size)],
            active: vec![0],
            pushed: Vec::new(),
        }
    }

    /// Makes drawing `placeholder` draw `view` instead: a premultiplied
    /// `Rgba8Unorm` texture of the placeholder's width × height (see
    /// [`texture_placeholder`]). The placeholder's pixels are never read or
    /// uploaded; the entry is dropped like a cached image once it goes undrawn.
    pub fn register_texture(&mut self, placeholder: &ImageData, view: wgpu::TextureView) {
        self.images
            .register(placeholder, view, [0.0, 0.0, 1.0, 1.0]);
    }

    /// Composites every glass pane from `source` — the backdrop frame, an
    /// `Rgba8Unorm` texture with `TEXTURE_BINDING` that allows its
    /// `Rgba8UnormSrgb` view (`view_formats`) — on the GPU and submits the
    /// work. The panes share one set of passes. Returns each pane's
    /// placeholder image, already registered (draw it in the next frame
    /// where the pane's frosted body goes), and what it cost. Nothing is
    /// read back.
    pub fn glass(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &wgpu::Texture,
        panes: &[GlassPane],
    ) -> (
        std::collections::HashMap<fenestra_core::WidgetId, ImageData>,
        GlassStats,
    ) {
        let (results, stats) = self.glass.run(device, queue, source, panes);
        let mut out = std::collections::HashMap::new();
        for (id, placeholder, view, uv) in results {
            self.images.register(&placeholder, view, uv);
            out.insert(id, placeholder);
        }
        (out, stats)
    }

    /// Test support: reads the texture a glass placeholder stands for back
    /// (premultiplied RGBA8, rows packed). Waits for the GPU.
    #[doc(hidden)]
    pub fn read_registered(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        placeholder: &ImageData,
    ) -> Option<Vec<u8>> {
        let (texture, [x, y, w, h]) = self.glass.result_of(placeholder)?;
        let row = (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fenestra-raster glass readback"),
            size: u64::from(row * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let data = slice.get_mapped_range();
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            let start = (y * row) as usize;
            out.extend_from_slice(&data[start..start + (w * 4) as usize]);
        }
        Some(out)
    }

    /// What the renderer currently keeps between frames.
    pub fn cache_sizes(&self) -> CacheSizes {
        CacheSizes {
            layer_textures: self.pool.len(),
            images: self.images.len(),
            atlas_pages: self.atlas.pages(),
            ramp_rows: self.ramps.len(),
            glass_panes: self.glass.len(),
        }
    }
}

/// What a draw call does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pipe {
    /// Fills paint-mesh triangles.
    Paint,
    /// Raises the stencil inside paint-mesh triangles.
    ClipPush,
    /// Lowers it again.
    ClipPop,
    /// Draws a quad-mesh quad sampling a texture.
    Textured(Tex),
    /// Draws shadow-mesh quads (blurred rounded rects).
    Shadow,
    /// Draws quad-mesh glyphs from a coverage atlas page, tinted.
    Mask(usize),
}

/// The texture a textured draw samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tex {
    /// An offscreen layer's (by layer index).
    Layer(usize),
    /// A cached image's.
    Image(images::Key),
    /// A glyph atlas page's.
    Page(usize),
}

/// One draw call: a pipeline, its stencil reference, and an index range into
/// the pipeline's mesh.
#[derive(Clone, Debug)]
struct Draw {
    pipe: Pipe,
    stencil: u32,
    indices: Range<u32>,
}

/// Clip levels one layer's stencil holds: the 255th is kept for compositing
/// a child layer, and a deeper clip opens a layer with a fresh stencil.
const STENCIL_CLIPS: u32 = 254;

/// A device-pixel rectangle: x, y, width, height.
type Bounds = (u32, u32, u32, u32);

/// A render pass's worth of draws: the root, or an offscreen layer that its
/// parent composites at `alpha`, clipped by the `clip` triangles.
#[derive(Debug)]
struct Layer {
    draws: Vec<Draw>,
    /// The current clip depth within this layer.
    clip_level: u32,
    /// Where it renders; `None` when nothing of it can show.
    bounds: Option<Bounds>,
    alpha: f32,
    /// The layer's clip shape in the paint mesh (drawn into the parent).
    clip: Range<u32>,
    /// The first quad-mesh vertex of its composite in the parent.
    composite: Option<usize>,
    /// The texture it renders into: `bounds`' size rounded up to a pool
    /// bucket (set by [`shrink_layers`]).
    texture: (u32, u32),
}

impl Layer {
    fn root(size: (u32, u32)) -> Self {
        Self {
            draws: Vec::new(),
            clip_level: 0,
            bounds: Some((0, 0, size.0, size.1)),
            alpha: 1.0,
            clip: 0..0,
            composite: None,
            texture: size,
        }
    }
}

/// What a push left to undo at its pop.
enum Pushed {
    /// A stencil clip in the current layer, with its triangles.
    Clip(Range<u32>),
    /// A new offscreen layer.
    Group(usize),
}

/// One frame being recorded. Implements [`DrawTarget`]; finish it with
/// [`RasterFrame::render`].
pub struct RasterFrame<'r> {
    renderer: &'r mut RasterRenderer,
    size: (u32, u32),
    stack: Vec<Affine>,
    mesh: Mesh,
    quads: QuadMesh,
    shadows: ShadowMesh,
    /// Every layer recorded this frame; 0 is the root. A layer's index is
    /// always above its parent's.
    layers: Vec<Layer>,
    /// The layers open now, innermost last.
    active: Vec<usize>,
    pushed: Vec<Pushed>,
}

/// Prints a tessellation failure once per process; the shape is skipped.
fn warn_tessellation(err: &lyon_tessellation::TessellationError) {
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        eprintln!("fenestra-raster: skipped a shape lyon could not tessellate ({err:?})");
    }
}

fn premultiply(c: Color) -> [f32; 4] {
    let [r, g, b, a] = c.components;
    [r * a, g * a, b * a, a]
}

#[expect(clippy::cast_possible_truncation, reason = "mesh sizes fit in u32")]
fn len32<T>(v: &[T]) -> u32 {
    v.len() as u32
}

impl RasterFrame<'_> {
    fn current(&self) -> Affine {
        self.stack.last().copied().unwrap_or(Affine::IDENTITY)
    }

    fn layer_index(&self) -> usize {
        *self.active.last().expect("the root layer is never popped")
    }

    /// Records `indices` as a draw on `pipe` in layer `layer`, at the
    /// layer's clip level, merging with the previous draw when it can.
    fn record(&mut self, layer: usize, pipe: Pipe, stencil: u32, indices: Range<u32>) {
        if indices.is_empty() {
            return;
        }
        let draws = &mut self.layers[layer].draws;
        match draws.last_mut() {
            Some(last)
                if last.pipe == pipe
                    && last.stencil == stencil
                    && last.indices.end == indices.start =>
            {
                last.indices.end = indices.end;
            }
            _ => draws.push(Draw {
                pipe,
                stencil,
                indices,
            }),
        }
    }

    /// Records the paint indices added since `start` in the current layer.
    fn record_paint(&mut self, start: u32) {
        let layer = self.layer_index();
        let stencil = self.layers[layer].clip_level;
        let end = len32(&self.mesh.indices);
        self.record(layer, Pipe::Paint, stencil, start..end);
    }

    /// Tessellates a clip shape into the paint mesh; empty when nothing of
    /// it can show (or it could not be tessellated).
    fn clip_triangles(&mut self, style: Fill, transform: Affine, clip: &impl Shape) -> Range<u32> {
        let start = len32(&self.mesh.indices);
        let transform = self.current() * transform;
        if transform.is_finite()
            && let Err(e) = geometry::fill(
                &mut self.renderer.fill,
                clip,
                style,
                transform,
                PaintParams::default(),
                &mut self.mesh,
            )
        {
            warn_tessellation(&e);
        }
        start..len32(&self.mesh.indices)
    }

    /// Raises layer `layer`'s stencil inside `clip`.
    fn clip_push(&mut self, layer: usize, clip: Range<u32>) {
        let level = self.layers[layer].clip_level;
        self.record(layer, Pipe::ClipPush, level, clip);
        self.layers[layer].clip_level = level + 1;
    }

    /// Lowers it again.
    fn clip_pop(&mut self, layer: usize, clip: Range<u32>) {
        let level = self.layers[layer].clip_level;
        self.record(layer, Pipe::ClipPop, level, clip);
        self.layers[layer].clip_level = level.saturating_sub(1);
    }

    /// Opens an offscreen layer that its parent composites at `alpha`,
    /// clipped by `clip`.
    fn push_group(&mut self, clip_style: Fill, alpha: f32, transform: Affine, clip: &impl Shape) {
        let parent = self.layer_index();
        let bounds = self.layers[parent]
            .bounds
            .and_then(|b| self.bounds(b, transform, clip));
        let clip = self.clip_triangles(clip_style, transform, clip);
        let child = self.layers.len();
        self.layers.push(Layer {
            draws: Vec::new(),
            clip_level: 0,
            bounds,
            alpha: alpha.max(0.0),
            clip,
            composite: None,
            texture: (0, 0),
        });
        self.active.push(child);
        self.pushed.push(Pushed::Group(child));
    }

    /// The device pixels `clip` (under `transform`) covers within `parent`.
    fn bounds(&self, parent: Bounds, transform: Affine, clip: &impl Shape) -> Option<Bounds> {
        let transform = self.current() * transform;
        if !transform.is_finite() {
            return None;
        }
        let r = transform.transform_rect_bbox(clip.bounding_box());
        let (px, py, pw, ph) = parent;
        let x0 = r.x0.floor().max(f64::from(px));
        let y0 = r.y0.floor().max(f64::from(py));
        let x1 = r.x1.ceil().min(f64::from(px + pw));
        let y1 = r.y1.ceil().min(f64::from(py + ph));
        if !(x1 > x0 && y1 > y0) {
            return None;
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped to the parent's non-negative pixel bounds"
        )]
        Some((x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
    }

    /// Encodes the frame, submits it, and resolves it into `target` (the
    /// frame's size, the renderer's format, `RENDER_ATTACHMENT`), cleared to
    /// `base_color` first.
    pub fn render(
        self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        base_color: Color,
    ) -> RenderStats {
        debug_assert!(self.stack.is_empty(), "unbalanced push_transform");
        debug_assert!(self.pushed.is_empty(), "unbalanced push_layer");
        let Self {
            renderer,
            size,
            mesh,
            quads,
            shadows,
            layers,
            ..
        } = self;
        let mut stats = RenderStats {
            vertices: len32(&mesh.vertices) + len32(&quads.vertices) + len32(&shadows.vertices),
            ..RenderStats::default()
        };
        if renderer.root.as_ref().is_none_or(|a| a.size != size) {
            renderer.root = Some(Attachments::new(device, renderer.format, size));
        }
        stats.ramps_built = renderer.ramps.built;
        let ramps = renderer.ramps.upload(device, queue);
        renderer.atlas.upload(device, queue);
        stats.glyphs_rasterized = renderer.atlas.rasterized;
        stats.images_uploaded =
            renderer
                .images
                .upload(device, queue, renderer.max_texture_dimension);

        let mut layers = layers;
        let mut quads = quads;
        shrink_layers(&mut layers, &mesh, &mut quads, &shadows);

        // One globals slot per layer: the device rectangle its pass covers
        // (an offscreen layer's bounds map onto its texture's top-left).
        let mut uniform = vec![0u8; layers.len() * GLOBALS_STRIDE as usize];
        for (i, layer) in layers.iter().enumerate() {
            let (x, y, _, _) = layer.bounds.unwrap_or((0, 0, 1, 1));
            let (w, h) = layer.texture;
            #[expect(
                clippy::cast_precision_loss,
                reason = "pixel coordinates are exact in f32"
            )]
            let globals = Globals {
                origin: [x as f32, y as f32],
                size: [w as f32, h as f32],
            };
            let at = i * GLOBALS_STRIDE as usize;
            uniform[at..at + std::mem::size_of::<Globals>()]
                .copy_from_slice(bytemuck::bytes_of(&globals));
        }
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fenestra-raster globals"),
            contents: &uniform,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fenestra-raster globals"),
            layout: &renderer.pipelines.globals_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &uniform,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<Globals>() as u64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(ramps),
                },
            ],
        });
        let buffers = Buffers {
            paint: upload(device, "paint", &mesh.vertices, &mesh.indices),
            quads: upload(device, "quads", &quads.vertices, &quads.indices),
            shadows: upload(device, "shadows", &shadows.vertices, &shadows.indices),
        };

        // A frame that may come out translucent renders the root offscreen
        // and is written straight (unpremultiplied) at the end, as vello
        // writes its output; an opaque one resolves into `target` directly.
        let straight = (base_color.components[3] < 1.0).then(|| {
            renderer.pool.take(
                device,
                renderer.format,
                size,
                &mut stats.layer_textures_created,
            )
        });

        // Children before parents: every descendant has a higher index.
        let mut targets: Vec<Option<LayerTarget>> = (0..layers.len()).map(|_| None).collect();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("fenestra-raster frame"),
        });
        for (i, layer) in layers.iter().enumerate().rev() {
            let Some(_) = layer.bounds else {
                continue;
            };
            let (w, h) = layer.texture;
            let target_layer;
            let (attachments, resolve, clear) = if i == 0 {
                let [r, g, b, a] = premultiply(base_color);
                let (attachments, resolve) = match &straight {
                    Some(t) => (&t.msaa, &t.view),
                    None => (renderer.root.as_ref().expect("built above"), target),
                };
                (
                    attachments,
                    resolve,
                    wgpu::Color {
                        r: f64::from(r),
                        g: f64::from(g),
                        b: f64::from(b),
                        a: f64::from(a),
                    },
                )
            } else {
                stats.layers += 1;
                stats.layer_pixels += u64::from(w) * u64::from(h);
                target_layer = renderer.pool.take(
                    device,
                    renderer.format,
                    (w, h),
                    &mut stats.layer_textures_created,
                );
                targets[i] = Some(target_layer);
                let t = targets[i].as_ref().expect("just stored");
                (&t.msaa, &t.view, wgpu::Color::TRANSPARENT)
            };
            // The textures this pass samples, bound before it begins.
            let mut binds: Vec<(Tex, wgpu::BindGroup)> = Vec::new();
            for draw in &layer.draws {
                let tex = match draw.pipe {
                    Pipe::Textured(tex) => tex,
                    Pipe::Mask(page) => Tex::Page(page),
                    _ => continue,
                };
                if binds.iter().any(|(t, _)| *t == tex) {
                    continue;
                }
                let view = match tex {
                    Tex::Layer(child) => targets[child].as_ref().map(|t| &t.view),
                    Tex::Image(k) => renderer.images.view(&k),
                    Tex::Page(page) => renderer.atlas.view(page),
                };
                if let Some(view) = view {
                    binds.push((tex, renderer.pipelines.texture_bind(device, view)));
                }
            }
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fenestra-raster layer"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &attachments.color,
                    depth_slice: None,
                    resolve_target: Some(resolve),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &attachments.stencil,
                    depth_ops: None,
                    stencil_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0),
                        store: wgpu::StoreOp::Discard,
                    }),
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            #[expect(clippy::cast_possible_truncation, reason = "layer counts fit in u32")]
            let offset = (i as u64 * GLOBALS_STRIDE) as u32;
            pass.set_bind_group(0, &globals_bind, &[offset]);
            stats.draws += encode_draws(
                &mut pass,
                &renderer.pipelines,
                &buffers,
                &layer.draws,
                &binds,
            );
        }
        if let Some(t) = &straight {
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fenestra-raster unpremultiply"),
                layout: &renderer.pipelines.unpremultiply_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&t.view),
                }],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fenestra-raster straight alpha"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
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
            pass.set_pipeline(&renderer.pipelines.unpremultiply);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);
        for target in targets.into_iter().flatten().chain(straight) {
            renderer.pool.give_back(target);
        }
        renderer.pool.trim();
        renderer.atlas.trim();
        stats
    }
}

/// A mesh's vertex and index buffers; `None` for an empty mesh (wgpu rejects
/// zero-sized buffers).
type Uploaded = Option<(wgpu::Buffer, wgpu::Buffer)>;

struct Buffers {
    paint: Uploaded,
    quads: Uploaded,
    shadows: Uploaded,
}

fn upload<V: bytemuck::Pod>(
    device: &wgpu::Device,
    label: &str,
    vertices: &[V],
    indices: &[u32],
) -> Uploaded {
    if indices.is_empty() {
        return None;
    }
    let make = |contents: &[u8], usage| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents,
            usage,
        })
    };
    Some((
        make(bytemuck::cast_slice(vertices), wgpu::BufferUsages::VERTEX),
        make(bytemuck::cast_slice(indices), wgpu::BufferUsages::INDEX),
    ))
}

/// Which mesh a pass has bound.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bound {
    None,
    Paint,
    Quads,
    Shadows,
}

/// Issues `draws` into `pass`; returns how many it issued.
fn encode_draws(
    pass: &mut wgpu::RenderPass<'_>,
    pipelines: &Pipelines,
    buffers: &Buffers,
    draws: &[Draw],
    binds: &[(Tex, wgpu::BindGroup)],
) -> u32 {
    let mut bound = Bound::None;
    let mut issued = 0;
    for draw in draws {
        let (pipeline, mesh, want) = match draw.pipe {
            Pipe::Paint => (&pipelines.paint, &buffers.paint, Bound::Paint),
            Pipe::ClipPush => (&pipelines.clip_push, &buffers.paint, Bound::Paint),
            Pipe::ClipPop => (&pipelines.clip_pop, &buffers.paint, Bound::Paint),
            Pipe::Shadow => (&pipelines.shadow, &buffers.shadows, Bound::Shadows),
            Pipe::Textured(tex) => {
                let Some((_, bind)) = binds.iter().find(|(t, _)| *t == tex) else {
                    continue;
                };
                pass.set_bind_group(1, bind, &[]);
                (&pipelines.image, &buffers.quads, Bound::Quads)
            }
            Pipe::Mask(page) => {
                let Some((_, bind)) = binds.iter().find(|(t, _)| *t == Tex::Page(page)) else {
                    continue;
                };
                pass.set_bind_group(1, bind, &[]);
                (&pipelines.mask, &buffers.quads, Bound::Quads)
            }
        };
        let Some((vertices, indices)) = mesh else {
            continue;
        };
        if bound != want {
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            bound = want;
        }
        pass.set_pipeline(pipeline);
        pass.set_stencil_reference(draw.stencil);
        pass.draw_indexed(draw.indices.clone(), 0, 0..1);
        issued += 1;
    }
    issued
}

impl DrawTarget for RasterFrame<'_> {
    fn fill<'b>(
        &mut self,
        style: Fill,
        transform: Affine,
        brush: impl Into<BrushRef<'b>>,
        _brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        let transform = self.current() * transform;
        if !transform.is_finite() {
            return;
        }
        let Some(params) = self.renderer.ramps.params(brush.into()) else {
            return;
        };
        let start = len32(&self.mesh.indices);
        if let Err(e) = geometry::fill(
            &mut self.renderer.fill,
            shape,
            style,
            transform,
            params,
            &mut self.mesh,
        ) {
            warn_tessellation(&e);
        }
        self.record_paint(start);
    }

    fn stroke<'b>(
        &mut self,
        style: &Stroke,
        transform: Affine,
        brush: impl Into<BrushRef<'b>>,
        _brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        let transform = self.current() * transform;
        if !transform.is_finite() {
            return;
        }
        let Some(params) = self.renderer.ramps.params(brush.into()) else {
            return;
        };
        let start = len32(&self.mesh.indices);
        if let Err(e) = geometry::stroke(
            &mut self.renderer.stroke,
            style,
            shape,
            transform,
            params,
            &mut self.mesh,
        ) {
            warn_tessellation(&e);
        }
        self.record_paint(start);
    }

    fn push_layer(&mut self, clip_style: Fill, alpha: f32, transform: Affine, clip: &impl Shape) {
        // A layer that does not fade only clips.
        if alpha >= 1.0 {
            self.push_clip_layer(clip_style, transform, clip);
        } else {
            self.push_group(clip_style, alpha, transform, clip);
        }
    }

    fn push_clip_layer(&mut self, clip_style: Fill, transform: Affine, clip: &impl Shape) {
        let layer = self.layer_index();
        // A full stencil: the clip opens a layer with a fresh one.
        if self.layers[layer].clip_level >= STENCIL_CLIPS {
            self.push_group(clip_style, 1.0, transform, clip);
            return;
        }
        let clip = self.clip_triangles(clip_style, transform, clip);
        self.clip_push(layer, clip.clone());
        self.pushed.push(Pushed::Clip(clip));
    }

    fn pop_layer(&mut self) {
        match self.pushed.pop() {
            Some(Pushed::Clip(clip)) => {
                let layer = self.layer_index();
                self.clip_pop(layer, clip);
            }
            Some(Pushed::Group(child)) => {
                self.active.pop();
                let parent = self.layer_index();
                let Layer {
                    bounds,
                    alpha,
                    clip,
                    ..
                } = &self.layers[child];
                let (Some((x, y, w, h)), alpha, clip) = (*bounds, *alpha, clip.clone()) else {
                    return;
                };
                self.clip_push(parent, clip.clone());
                self.layers[child].composite = Some(self.quads.vertices.len());
                let start = len32(&self.quads.indices);
                let (x0, y0) = (f64::from(x), f64::from(y));
                let (x1, y1) = (f64::from(x + w), f64::from(y + h));
                geometry::quad(
                    &mut self.quads,
                    [
                        Point::new(x0, y0),
                        Point::new(x1, y0),
                        Point::new(x1, y1),
                        Point::new(x0, y1),
                    ],
                    [alpha; 4],
                );
                let level = self.layers[parent].clip_level;
                let end = len32(&self.quads.indices);
                self.record(parent, Pipe::Textured(Tex::Layer(child)), level, start..end);
                self.clip_pop(parent, clip);
            }
            None => debug_assert!(false, "pop_layer without a push"),
        }
    }

    fn draw_image(&mut self, image: &ImageData, transform: Affine) {
        let t = self.current() * transform;
        if !t.is_finite() || image.width == 0 || image.height == 0 {
            return;
        }
        let key = self.renderer.images.use_image(image);
        let (w, h) = (f64::from(image.width), f64::from(image.height));
        let start = len32(&self.quads.indices);
        geometry::quad_uv(
            &mut self.quads,
            [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].map(|p| t * Point::from(p)),
            self.renderer.images.uv(&key),
            [1.0; 4],
        );
        let layer = self.layer_index();
        let stencil = self.layers[layer].clip_level;
        let end = len32(&self.quads.indices);
        self.record(layer, Pipe::Textured(Tex::Image(key)), stencil, start..end);
    }

    fn draw_blurred_rounded_rect(
        &mut self,
        transform: Affine,
        rect: Rect,
        color: Color,
        radius: f64,
        std_dev: f64,
    ) {
        let t = self.current() * transform;
        if !t.is_finite() || rect.is_zero_area() {
            return;
        }
        // Also takes a NaN deviation.
        if std_dev.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            // No blur: the rounded rect itself.
            let shape = kurbo::RoundedRect::from_rect(rect, radius);
            self.fill(Fill::NonZero, transform, color, None, &shape);
            return;
        }
        let start = len32(&self.shadows.indices);
        geometry::shadow(
            &mut self.shadows,
            t,
            rect,
            radius,
            std_dev,
            premultiply(color),
        );
        let layer = self.layer_index();
        let stencil = self.layers[layer].clip_level;
        let end = len32(&self.shadows.indices);
        self.record(layer, Pipe::Shadow, stencil, start..end);
    }

    fn draw_glyphs(&mut self, run: &GlyphRun<'_>, glyphs: impl Iterator<Item = Glyph>) {
        let transform = self.current() * run.transform;
        if !transform.is_finite() {
            return;
        }
        let color = premultiply(run.color);
        let layer = self.layer_index();
        let stencil = self.layers[layer].clip_level;
        for glyph in glyphs {
            let Some(placed) = self.renderer.atlas.place(run, transform, glyph) else {
                continue;
            };
            let (pipe, tint) = match placed.kind {
                glyphs::Kind::Mask => (Pipe::Mask(placed.page), color),
                glyphs::Kind::Color => (Pipe::Textured(Tex::Page(placed.page)), [color[3]; 4]),
            };
            let start = len32(&self.quads.indices);
            geometry::quad_uv(&mut self.quads, placed.corners, placed.uv, tint);
            let end = len32(&self.quads.indices);
            self.record(layer, pipe, stencil, start..end);
        }
    }

    fn push_transform(&mut self, transform: Affine) {
        let t = self.current() * transform;
        self.stack.push(t);
    }

    fn pop_transform(&mut self) {
        self.stack.pop();
    }
}

/// Rounds a layer's size up to its pool bucket, so layers whose content
/// size changes a little every frame (an animated fade) reuse targets.
fn bucket((w, h): (u32, u32)) -> (u32, u32) {
    (w.next_multiple_of(64), h.next_multiple_of(64))
}

/// Shrinks each offscreen layer to the device pixels its draws touch
/// (fenestra pushes an opacity layer with the whole canvas as its clip) and
/// moves its composite quad to match. Children first, so a parent measures
/// its children's composites after they shrank.
fn shrink_layers(
    layers: &mut [Layer],
    mesh: &Mesh,
    quads: &mut QuadMesh,
    shadows: &geometry::ShadowMesh,
) {
    for i in (1..layers.len()).rev() {
        let Some((bx, by, bw, bh)) = layers[i].bounds else {
            continue;
        };
        let mut content = Rect::new(
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        let mut add = |p: [f32; 2]| {
            content = content.union_pt(Point::new(f64::from(p[0]), f64::from(p[1])));
        };
        for draw in &layers[i].draws {
            let range = draw.indices.start as usize..draw.indices.end as usize;
            match draw.pipe {
                Pipe::Paint => mesh.indices[range]
                    .iter()
                    .for_each(|&v| add(mesh.vertices[v as usize].pos)),
                Pipe::Shadow => shadows.indices[range]
                    .iter()
                    .for_each(|&v| add(shadows.vertices[v as usize].pos)),
                Pipe::Textured(Tex::Layer(child)) if layers[child].bounds.is_none() => {}
                Pipe::Textured(_) | Pipe::Mask(_) => {
                    quads.indices[range]
                        .iter()
                        .for_each(|&v| add(quads.vertices[v as usize].pos));
                }
                Pipe::ClipPush | Pipe::ClipPop => {}
            }
        }
        // A pixel of margin for antialiased edges.
        let content = content.inflate(1.0, 1.0);
        let bounds = Rect::new(
            f64::from(bx),
            f64::from(by),
            f64::from(bx + bw),
            f64::from(by + bh),
        );
        let r = content.intersect(bounds);
        let shrunk = (r.width() > 0.0 && r.height() > 0.0).then(|| {
            let (x0, y0) = (r.x0.floor().max(bounds.x0), r.y0.floor().max(bounds.y0));
            let (x1, y1) = (r.x1.ceil().min(bounds.x1), r.y1.ceil().min(bounds.y1));
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "within the layer's non-negative pixel bounds"
            )]
            let b = (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32);
            b
        });
        let layer = &mut layers[i];
        layer.bounds = shrunk;
        let Some((x, y, w, h)) = shrunk else {
            continue;
        };
        layer.texture = bucket((w, h));
        if let Some(v) = layer.composite {
            let (tw, th) = layer.texture;
            #[expect(
                clippy::cast_precision_loss,
                reason = "pixel coordinates are exact in f32"
            )]
            let (x0, y0, x1, y1, u, t) = (
                x as f32,
                y as f32,
                (x + w) as f32,
                (y + h) as f32,
                w as f32 / tw as f32,
                h as f32 / th as f32,
            );
            for (vertex, (pos, uv)) in quads.vertices[v..v + 4].iter_mut().zip([
                ([x0, y0], [0.0, 0.0]),
                ([x1, y0], [u, 0.0]),
                ([x1, y1], [u, t]),
                ([x0, y1], [0.0, t]),
            ]) {
                vertex.pos = pos;
                vertex.uv = uv;
            }
        }
    }
}
