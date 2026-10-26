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
mod paint;

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use fenestra_core::draw::{DrawTarget, Glyph, GlyphRun};
use kurbo::{Affine, Rect, Shape, Stroke};
use lyon_tessellation::{FillTessellator, StrokeTessellator};
use peniko::{BrushRef, Color, Fill, ImageData};
use wgpu::util::DeviceExt;

use geometry::Mesh;
use gpu::{Attachments, GLOBALS_STRIDE, Globals, Pipelines};

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
        }
    }

    /// Starts recording a `width`×`height` device-pixel frame.
    pub fn frame(&mut self, width: u32, height: u32) -> RasterFrame<'_> {
        self.ramps.begin_frame();
        RasterFrame {
            renderer: self,
            size: (width.max(1), height.max(1)),
            stack: Vec::new(),
            mesh: Mesh::new(),
            layers: vec![Layer::default()],
        }
    }

    /// What the renderer currently keeps between frames.
    pub fn cache_sizes(&self) -> CacheSizes {
        CacheSizes {
            ramp_rows: self.ramps.len(),
            ..CacheSizes::default()
        }
    }
}

/// A pipeline a draw uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pipe {
    Paint,
}

/// One draw call: a pipeline, a stencil reference, and an index range.
#[derive(Clone, Debug)]
struct Draw {
    pipe: Pipe,
    stencil: u32,
    indices: Range<u32>,
}

/// A render pass's worth of draws: the root, or (later) an offscreen layer.
#[derive(Debug, Default)]
struct Layer {
    draws: Vec<Draw>,
    /// The current clip depth within this layer.
    clip_level: u32,
}

/// One frame being recorded. Implements [`DrawTarget`]; finish it with
/// [`RasterFrame::render`].
pub struct RasterFrame<'r> {
    renderer: &'r mut RasterRenderer,
    size: (u32, u32),
    stack: Vec<Affine>,
    mesh: Mesh,
    layers: Vec<Layer>,
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

impl RasterFrame<'_> {
    fn current(&self) -> Affine {
        self.stack.last().copied().unwrap_or(Affine::IDENTITY)
    }

    fn layer(&mut self) -> &mut Layer {
        self.layers
            .last_mut()
            .expect("the root layer is never popped")
    }

    /// Records the indices added since `start` as a draw on `pipe`, merging
    /// with the previous draw when it can.
    fn push_draw(&mut self, pipe: Pipe, start: usize) {
        #[expect(clippy::cast_possible_truncation, reason = "index counts fit in u32")]
        let indices = start as u32..self.mesh.indices.len() as u32;
        if indices.is_empty() {
            return;
        }
        let layer = self.layer();
        let stencil = layer.clip_level;
        match layer.draws.last_mut() {
            Some(last)
                if last.pipe == pipe
                    && last.stencil == stencil
                    && last.indices.end == indices.start =>
            {
                last.indices.end = indices.end;
            }
            _ => layer.draws.push(Draw {
                pipe,
                stencil,
                indices,
            }),
        }
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
        debug_assert_eq!(self.layers.len(), 1, "unbalanced push_layer");
        let Self {
            renderer,
            size,
            mesh,
            layers,
            ..
        } = self;
        let mut stats = RenderStats::default();
        if renderer.root.as_ref().is_none_or(|a| a.size != size) {
            renderer.root = Some(Attachments::new(device, renderer.format, size));
        }
        let ramps = renderer.ramps.upload(device, queue);
        let root = renderer.root.as_ref().expect("just built");

        #[expect(clippy::cast_precision_loss, reason = "frame sizes are exact in f32")]
        let globals = Globals {
            origin: [0.0, 0.0],
            size: [size.0 as f32, size.1 as f32],
        };
        let mut uniform = vec![0u8; GLOBALS_STRIDE as usize];
        uniform[..std::mem::size_of::<Globals>()].copy_from_slice(bytemuck::bytes_of(&globals));
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
        // wgpu rejects zero-sized buffers: an empty frame binds none.
        let buffers = (!mesh.indices.is_empty()).then(|| {
            let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("fenestra-raster vertices"),
                contents: bytemuck::cast_slice(&mesh.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("fenestra-raster indices"),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            (vertices, indices)
        });
        #[expect(clippy::cast_possible_truncation, reason = "vertex counts fit in u32")]
        {
            stats.vertices = mesh.vertices.len() as u32;
        }

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("fenestra-raster frame"),
        });
        {
            let [r, g, b, a] = premultiply(base_color);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fenestra-raster root"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &root.color,
                    depth_slice: None,
                    resolve_target: Some(target),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(r),
                            g: f64::from(g),
                            b: f64::from(b),
                            a: f64::from(a),
                        }),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &root.stencil,
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
            if let Some((vertices, indices)) = &buffers {
                pass.set_bind_group(0, &globals_bind, &[0]);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                for draw in &layers[0].draws {
                    match draw.pipe {
                        Pipe::Paint => pass.set_pipeline(&renderer.pipelines.paint),
                    }
                    pass.set_stencil_reference(draw.stencil);
                    pass.draw_indexed(draw.indices.clone(), 0, 0..1);
                    stats.draws += 1;
                }
            }
        }
        queue.submit([encoder.finish()]);
        stats
    }
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
        let start = self.mesh.indices.len();
        let renderer = &mut *self.renderer;
        if let Err(e) = geometry::fill(
            &mut renderer.fill,
            shape,
            style,
            transform,
            params,
            &mut self.mesh,
        ) {
            warn_tessellation(&e);
        }
        self.push_draw(Pipe::Paint, start);
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
        let start = self.mesh.indices.len();
        let renderer = &mut *self.renderer;
        if let Err(e) = geometry::stroke(
            &mut renderer.stroke,
            style,
            shape,
            transform,
            params,
            &mut self.mesh,
        ) {
            warn_tessellation(&e);
        }
        self.push_draw(Pipe::Paint, start);
    }

    fn push_layer(
        &mut self,
        _clip_style: Fill,
        _alpha: f32,
        _transform: Affine,
        _clip: &impl Shape,
    ) {
        todo!("Task 4: opacity layers")
    }

    fn push_clip_layer(&mut self, _clip_style: Fill, _transform: Affine, _clip: &impl Shape) {
        todo!("Task 4: clip layers")
    }

    fn pop_layer(&mut self) {
        todo!("Task 4: layers")
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
