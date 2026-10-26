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
mod gpu;
mod layers;
mod paint;

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use fenestra_core::draw::{DrawTarget, Glyph, GlyphRun};
use kurbo::{Affine, Point, Rect, Shape, Stroke};
use lyon_tessellation::{FillTessellator, StrokeTessellator};
use peniko::{BrushRef, Color, Fill, ImageData};
use wgpu::util::DeviceExt;

use geometry::{Mesh, PaintParams, QuadMesh};
use gpu::{Attachments, GLOBALS_STRIDE, Globals, Pipelines};
use layers::{LayerPool, LayerTarget};

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
    /// Images uploaded to the GPU (not found in the cache).
    pub images_uploaded: u32,
    /// Glyphs rasterized into the atlas (not found in it).
    pub glyphs_rasterized: u32,
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
}

impl RasterRenderer {
    /// Builds the pipelines for targets of `format` (fenestra renders into
    /// `Rgba8Unorm`).
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self {
            format,
            pipelines: Pipelines::new(device, format),
            fill: FillTessellator::new(),
            stroke: StrokeTessellator::new(),
            root: None,
            ramps: paint::Ramps::default(),
            pool: LayerPool::default(),
        }
    }

    /// Starts recording a `width`×`height` device-pixel frame.
    pub fn frame(&mut self, width: u32, height: u32) -> RasterFrame<'_> {
        self.ramps.begin_frame();
        self.pool.begin_frame();
        let size = (width.max(1), height.max(1));
        RasterFrame {
            renderer: self,
            size,
            stack: Vec::new(),
            mesh: Mesh::new(),
            quads: QuadMesh::new(),
            layers: vec![Layer::root(size)],
            active: vec![0],
            pushed: Vec::new(),
        }
    }

    /// What the renderer currently keeps between frames.
    pub fn cache_sizes(&self) -> CacheSizes {
        CacheSizes {
            layer_textures: self.pool.len(),
            ramp_rows: self.ramps.len(),
            ..CacheSizes::default()
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
    /// Draws a quad-mesh quad sampling an offscreen layer's texture.
    Composite(usize),
}

/// One draw call: a pipeline, its stencil reference, and an index range into
/// the pipeline's mesh.
#[derive(Clone, Debug)]
struct Draw {
    pipe: Pipe,
    stencil: u32,
    indices: Range<u32>,
}

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
}

impl Layer {
    fn root(size: (u32, u32)) -> Self {
        Self {
            draws: Vec::new(),
            clip_level: 0,
            bounds: Some((0, 0, size.0, size.1)),
            alpha: 1.0,
            clip: 0..0,
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
                    && !matches!(pipe, Pipe::Composite(_))
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
            layers,
            ..
        } = self;
        let mut stats = RenderStats {
            vertices: len32(&mesh.vertices) + len32(&quads.vertices),
            ..RenderStats::default()
        };
        if renderer.root.as_ref().is_none_or(|a| a.size != size) {
            renderer.root = Some(Attachments::new(device, renderer.format, size));
        }
        let ramps = renderer.ramps.upload(device, queue);

        // One globals slot per layer: the device rectangle its pass covers.
        let mut uniform = vec![0u8; layers.len() * GLOBALS_STRIDE as usize];
        for (i, layer) in layers.iter().enumerate() {
            let (x, y, w, h) = layer.bounds.unwrap_or((0, 0, 1, 1));
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
        };

        // Children before parents: every descendant has a higher index.
        let mut targets: Vec<Option<LayerTarget>> = (0..layers.len()).map(|_| None).collect();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("fenestra-raster frame"),
        });
        for (i, layer) in layers.iter().enumerate().rev() {
            let Some((_, _, w, h)) = layer.bounds else {
                continue;
            };
            let target_layer;
            let (attachments, resolve, clear) = if i == 0 {
                let [r, g, b, a] = premultiply(base_color);
                (
                    renderer.root.as_ref().expect("built above"),
                    target,
                    wgpu::Color {
                        r: f64::from(r),
                        g: f64::from(g),
                        b: f64::from(b),
                        a: f64::from(a),
                    },
                )
            } else {
                stats.layers += 1;
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
            // Composited children's textures, bound before the pass begins.
            let child_binds: Vec<(usize, wgpu::BindGroup)> = layer
                .draws
                .iter()
                .filter_map(|d| match d.pipe {
                    Pipe::Composite(child) => targets[child]
                        .as_ref()
                        .map(|t| (child, renderer.pipelines.texture_bind(device, &t.view))),
                    _ => None,
                })
                .collect();
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
                &child_binds,
            );
        }
        queue.submit([encoder.finish()]);
        for target in targets.into_iter().flatten() {
            renderer.pool.give_back(target);
        }
        renderer.pool.trim();
        stats
    }
}

/// A mesh's vertex and index buffers; `None` for an empty mesh (wgpu rejects
/// zero-sized buffers).
type Uploaded = Option<(wgpu::Buffer, wgpu::Buffer)>;

struct Buffers {
    paint: Uploaded,
    quads: Uploaded,
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
}

/// Issues `draws` into `pass`; returns how many it issued.
fn encode_draws(
    pass: &mut wgpu::RenderPass<'_>,
    pipelines: &Pipelines,
    buffers: &Buffers,
    draws: &[Draw],
    child_binds: &[(usize, wgpu::BindGroup)],
) -> u32 {
    let mut bound = Bound::None;
    let mut issued = 0;
    for draw in draws {
        let (pipeline, mesh, want) = match draw.pipe {
            Pipe::Paint => (&pipelines.paint, &buffers.paint, Bound::Paint),
            Pipe::ClipPush => (&pipelines.clip_push, &buffers.paint, Bound::Paint),
            Pipe::ClipPop => (&pipelines.clip_pop, &buffers.paint, Bound::Paint),
            Pipe::Composite(child) => {
                let Some((_, bind)) = child_binds.iter().find(|(c, _)| *c == child) else {
                    continue;
                };
                pass.set_bind_group(1, bind, &[]);
                (&pipelines.image, &buffers.quads, Bound::Quads)
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
            return;
        }
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
        });
        self.active.push(child);
        self.pushed.push(Pushed::Group(child));
    }

    fn push_clip_layer(&mut self, clip_style: Fill, transform: Affine, clip: &impl Shape) {
        let clip = self.clip_triangles(clip_style, transform, clip);
        let layer = self.layer_index();
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
                self.record(parent, Pipe::Composite(child), level, start..end);
                self.clip_pop(parent, clip);
            }
            None => debug_assert!(false, "pop_layer without a push"),
        }
    }

    fn draw_image(&mut self, _image: &ImageData, _transform: Affine) {
        todo!("Task 5: images")
    }

    fn draw_blurred_rounded_rect(
        &mut self,
        _transform: Affine,
        _rect: Rect,
        _color: Color,
        _radius: f64,
        _std_dev: f64,
    ) {
        todo!("Task 5: shadows")
    }

    fn draw_glyphs(&mut self, _run: &GlyphRun<'_>, _glyphs: impl Iterator<Item = Glyph>) {
        todo!("Task 6: glyphs")
    }

    fn push_transform(&mut self, transform: Affine) {
        let t = self.current() * transform;
        self.stack.push(t);
    }

    fn pop_transform(&mut self) {
        self.stack.pop();
    }
}
