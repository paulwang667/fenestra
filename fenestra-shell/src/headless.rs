//! Offscreen rendering: a wgpu device plus vello renderer with no window or
//! display server. This is the backbone of fenestra's snapshot testing.

use std::num::NonZeroUsize;

use fenestra_core::draw::DrawTarget;
use fenestra_core::{Fonts, Frame, FrameState};
use image::RgbaImage;
use vello::peniko::Color;
use vello::util::RenderContext;
use vello::wgpu::{
    self, BufferDescriptor, BufferUsages, CommandEncoderDescriptor, Extent3d, TexelCopyBufferInfo,
    TexelCopyBufferLayout, TextureDescriptor, TextureFormat, TextureUsages,
};
use vello::{AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene};

use crate::ShellError;
use crate::backend::{Backend, backend_from_env, effective_backend, warn_raster_fallback};

/// Env var selecting vello's CPU compute pipeline (any value but `0` or
/// empty): the renderer's shader stages run as native Rust instead of GPU
/// compute, leaving the adapter only upload/copy work. This is the switch
/// that makes software adapters viable where their compute rasterization
/// crashes or diverges — Windows' built-in WARP hits
/// `STATUS_ACCESS_VIOLATION` under vello's GPU compute but is fine as a
/// copy engine. Applies to headless rendering and the live window alike;
/// adapter *selection* itself is steered by wgpu's own `WGPU_BACKEND` /
/// `WGPU_ADAPTER_NAME`, which fenestra honors end to end.
pub const CPU_ENV: &str = "FENESTRA_CPU";

/// Reads [`CPU_ENV`] from the process environment.
pub(crate) fn cpu_compute_requested() -> bool {
    cpu_flag(std::env::var_os(CPU_ENV).as_deref())
}

/// `FENESTRA_CPU` semantics: set-and-not-`0` enables (empty disables, so
/// `FENESTRA_CPU= cargo test` behaves like unset).
fn cpu_flag(v: Option<&std::ffi::OsStr>) -> bool {
    v.is_some_and(|v| !v.is_empty() && v != "0")
}

/// A reusable offscreen renderer. Creating one compiles its shaders, so
/// tests should create it once and render many scenes through it.
pub struct Headless {
    context: RenderContext,
    dev_id: usize,
    /// vello, built on first use: frames go through it with
    /// [`Backend::Vello`], and [`render`](Self::render) always does.
    renderer: Option<Renderer>,
    /// The raster backend, with [`Backend::Raster`].
    raster: Option<fenestra_raster::RasterRenderer>,
    backend: Backend,
    max_dim: u32,
    /// Raster glass composites on the GPU (the default) rather than read
    /// back and blurred on the CPU.
    gpu_glass: bool,
    /// Backdrop regions read back for the CPU filters so far.
    backdrop_reads: u32,
}

impl Headless {
    /// Acquires an adapter (no surface required) and renders through the
    /// [`Backend`] `FENESTRA_RENDERER` chooses. Set [`CPU_ENV`] to run
    /// vello's compute stages on the CPU (software-adapter environments).
    pub fn new() -> Result<Self, ShellError> {
        Self::with_backend(backend_from_env())
    }

    /// [`Headless::new`] rendering frames through `backend` — or vello, when
    /// the adapter cannot run the raster backend.
    pub fn with_backend(backend: Backend) -> Result<Self, ShellError> {
        let mut context = RenderContext::new();
        let dev_id = pollster::block_on(context.device(None)).ok_or(ShellError::NoDevice)?;
        let handle = &context.devices[dev_id];
        let max_dim = handle.device.limits().max_texture_dimension_2d;
        let adapter = handle.adapter().get_info().backend;
        let effective = effective_backend(backend, adapter);
        if effective != backend {
            warn_raster_fallback(adapter);
        }
        let backend = effective;
        let raster = (backend == Backend::Raster).then(|| {
            fenestra_raster::RasterRenderer::new(&handle.device, TextureFormat::Rgba8Unorm)
        });
        let mut headless = Self {
            context,
            dev_id,
            renderer: None,
            raster,
            backend,
            max_dim,
            gpu_glass: true,
            backdrop_reads: 0,
        };
        if backend == Backend::Vello {
            headless.vello()?;
        }
        Ok(headless)
    }

    /// The device, queue and raster renderer, with [`Backend::Raster`]
    /// (crate tests drive the GPU glass through them).
    #[cfg(test)]
    pub(crate) fn raster_gpu(
        &mut self,
    ) -> Option<(
        &wgpu::Device,
        &wgpu::Queue,
        &mut fenestra_raster::RasterRenderer,
    )> {
        let handle = &self.context.devices[self.dev_id];
        Some((&handle.device, &handle.queue, self.raster.as_mut()?))
    }

    /// Composite raster glass on the GPU (`true`, the default) or through the
    /// old read-back-and-CPU path — the reference the GPU path is tested
    /// against.
    #[doc(hidden)]
    pub fn set_gpu_glass(&mut self, on: bool) {
        self.gpu_glass = on;
    }

    /// How many backdrop regions this renderer has read back for the CPU
    /// filters (not counting the frame read that returns the image).
    #[doc(hidden)]
    pub fn backdrop_reads(&self) -> u32 {
        self.backdrop_reads
    }

    /// The backend frames render through.
    pub fn backend(&self) -> Backend {
        self.backend
    }

    /// vello's renderer, built on first use.
    fn vello(&mut self) -> Result<&mut Renderer, ShellError> {
        if self.renderer.is_none() {
            let device = &self.context.devices[self.dev_id].device;
            self.renderer = Some(
                Renderer::new(
                    device,
                    RendererOptions {
                        use_cpu: cpu_compute_requested(),
                        antialiasing_support: AaSupport::area_only(),
                        num_init_threads: NonZeroUsize::new(1),
                        pipeline_cache: None,
                    },
                )
                .map_err(ShellError::Vello)?,
            );
        }
        Ok(self.renderer.as_mut().expect("just built"))
    }

    /// The largest texture dimension the device supports; render sizes are
    /// clamped to it.
    pub fn max_dimension(&self) -> u32 {
        self.max_dim
    }

    /// Clamps a requested render size on both axes to the supported
    /// `1..=max_dimension()` range.
    pub fn clamp_size(&self, width: u32, height: u32) -> (u32, u32) {
        (width.clamp(1, self.max_dim), height.clamp(1, self.max_dim))
    }

    /// Renders `scene` at the given pixel size over `base_color` and reads the
    /// result back into an RGBA image. The size is clamped to
    /// `1..=max_dimension()` on both axes, so hostile dimensions cannot
    /// trigger wgpu's fatal validation handler.
    pub fn render(
        &mut self,
        scene: &Scene,
        width: u32,
        height: u32,
        base_color: Color,
    ) -> Result<RgbaImage, ShellError> {
        let (width, height) = self.clamp_size(width, height);
        let target = self.target(width, height, TextureUsages::STORAGE_BINDING);
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        self.vello()?;
        let handle = &self.context.devices[self.dev_id];
        self.renderer
            .as_mut()
            .expect("built above")
            .render_to_texture(
                &handle.device,
                &handle.queue,
                scene,
                &view,
                &RenderParams {
                    base_color,
                    width,
                    height,
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(ShellError::Vello)?;
        self.read(&target, (0, 0, width, height))
    }

    /// An RGBA8 texture to render into and read back.
    fn target(&self, width: u32, height: u32, usage: TextureUsages) -> wgpu::Texture {
        self.context.devices[self.dev_id]
            .device
            .create_texture(&TextureDescriptor {
                label: Some("fenestra headless target"),
                size: Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TextureFormat::Rgba8Unorm,
                usage: usage | TextureUsages::COPY_SRC,
                // The raster backend's GPU glass reads it in linear light.
                view_formats: if usage.contains(TextureUsages::TEXTURE_BINDING) {
                    &[TextureFormat::Rgba8UnormSrgb]
                } else {
                    &[]
                },
            })
    }

    /// Reads the `(x, y, width, height)` part of `texture` back.
    fn read(
        &self,
        texture: &wgpu::Texture,
        region: (u32, u32, u32, u32),
    ) -> Result<RgbaImage, ShellError> {
        let handle = &self.context.devices[self.dev_id];
        read_texture(&handle.device, &handle.queue, texture, region)
    }

    /// Renders `frame` with real frosted-glass backdrop blur and foreground
    /// [`ElementFilter`](fenestra_core::ElementFilter)s via the two-pass
    /// pipeline, falling back to a single pass when the frame has neither.
    ///
    /// The fast path is byte-for-byte identical to [`render`](Self::render): a
    /// frame with no filtered node produces an empty plan, so the backdrop scene
    /// *is* the final image and nothing is read back or reprocessed. Otherwise
    /// the backdrop scene (every glass subtree skipped) is rendered and read
    /// back — only the part the glass reads
    /// ([`backdrop_region`](crate::multi_pass::backdrop_region)) — each region
    /// is blurred/filtered on the CPU
    /// ([`process_specs_in`](crate::multi_pass::process_specs_in)), and the final scene
    /// composites those images. `width`/`height` are physical pixels.
    pub fn render_plan(
        &mut self,
        frame: &Frame,
        fonts: &mut Fonts,
        state: &mut FrameState,
        width: u32,
        height: u32,
        base_color: Color,
    ) -> Result<RgbaImage, ShellError> {
        if self.raster.is_some() {
            return self.render_plan_raster(frame, fonts, state, width, height, base_color);
        }
        // Paint produces logical-space scenes; a hi-DPI frame (scale != 1)
        // scales them onto the physical texture here. `process_specs`
        // already maps spec rects logical→physical via the same factor, and
        // the injected images are physical-resolution cuts drawn back into
        // logical rects — so the two passes stay consistent at any scale.
        let scale = frame.scale();
        let (backdrop_scene, specs) = frame.paint_backdrop(fonts, state);
        let backdrop_scene = Self::at_scale(backdrop_scene, scale);
        if specs.is_empty() {
            // Fast path: one pass, identical to `render`.
            return self.render(&backdrop_scene, width, height, base_color);
        }
        // Only the part of the backdrop the glass reads is rendered and read
        // back — the same path the windowed shell takes, so the goldens cover it.
        let (width, height) = self.clamp_size(width, height);
        let (region, shifted) =
            crate::multi_pass::region_scene(&backdrop_scene, &specs, scale, (width, height));
        let backdrop = self.render(&shifted, region.2, region.3, base_color)?;
        let injected = crate::multi_pass::process_specs_in(
            &backdrop,
            (region.0, region.1),
            (width, height),
            &specs,
            scale,
            &|_, _, _| None, // no custom renderers registered in the headless path
            &mut std::collections::HashMap::new(),
            &mut crate::multi_pass::GlassCache::default(),
        );
        let final_scene = Self::at_scale(frame.paint_final(fonts, state, &injected), scale);
        self.render(&final_scene, width, height, base_color)
    }

    /// [`render_plan`](Self::render_plan) through the raster backend. The
    /// backdrop pass renders the whole frame and only the part the glass
    /// reads is copied back (the raster backend paints straight into its
    /// frame, so there is no scene to shift into a smaller target).
    fn render_plan_raster(
        &mut self,
        frame: &Frame,
        fonts: &mut Fonts,
        state: &mut FrameState,
        width: u32,
        height: u32,
        base_color: Color,
    ) -> Result<RgbaImage, ShellError> {
        let (width, height) = self.clamp_size(width, height);
        let scale = vello::kurbo::Affine::scale(frame.scale());
        let usage = TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING;
        let target = self.target(width, height, usage);
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let handle = &self.context.devices[self.dev_id];
        let raster = self.raster.as_mut().expect("raster backend");
        let mut f = raster.frame(width, height);
        f.push_transform(scale);
        let specs = frame.paint_backdrop_into(&mut f, fonts, state);
        f.pop_transform();
        f.render(&handle.device, &handle.queue, &view, base_color);
        if specs.is_empty() {
            return self.read(&target, (0, 0, width, height));
        }
        // Glass composites on the GPU straight from the backdrop texture;
        // only the other filters (if any) still read their region back.
        let (mut injected, specs) = if self.gpu_glass {
            let (panes, rest) =
                crate::multi_pass::glass_panes(&specs, frame, frame.scale(), (width, height));
            let (glass, _) = raster.glass(&handle.device, &handle.queue, &target, &panes);
            (glass, rest)
        } else {
            (std::collections::HashMap::new(), specs)
        };
        let baked: std::collections::HashSet<_> = injected.keys().copied().collect();
        if !specs.is_empty() {
            let region = crate::multi_pass::backdrop_region(&specs, frame.scale(), width, height)
                .unwrap_or((0, 0, 1.min(width), 1.min(height)));
            let backdrop = self.read(&target, region)?;
            self.backdrop_reads += 1;
            injected.extend(crate::multi_pass::process_specs_in(
                &backdrop,
                (region.0, region.1),
                (width, height),
                &specs,
                frame.scale(),
                &|_, _, _| None, // no custom renderers registered in the headless path
                &mut std::collections::HashMap::new(),
                &mut crate::multi_pass::GlassCache::default(),
            ));
        }
        let handle = &self.context.devices[self.dev_id];
        let raster = self.raster.as_mut().expect("raster backend");
        let mut f = raster.frame(width, height);
        f.push_transform(scale);
        frame.paint_final_baked_into(&mut f, fonts, state, &injected, &baked);
        f.pop_transform();
        f.render(&handle.device, &handle.queue, &view, base_color);
        self.read(&target, (0, 0, width, height))
    }

    /// Wraps a logical-space scene for a physical target. Returns the scene
    /// untouched at scale 1.0 so the reference goldens stay byte-identical.
    fn at_scale(scene: Scene, scale: f64) -> Scene {
        if (scale - 1.0).abs() < f64::EPSILON {
            return scene;
        }
        let mut scaled = Scene::new();
        scaled.append(&scene, Some(vello::kurbo::Affine::scale(scale)));
        scaled
    }
}

/// Reads the `(x, y, width, height)` part of an RGBA8 `texture` (with
/// `COPY_SRC`) back to the CPU.
pub(crate) fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    (x, y, width, height): (u32, u32, u32, u32),
) -> Result<RgbaImage, ShellError> {
    // wgpu requires copy rows padded to 256 bytes.
    let padded_byte_width = (width * 4).next_multiple_of(256);
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("fenestra headless readback"),
        size: u64::from(padded_byte_width) * u64::from(height),
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("fenestra headless copy"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        TexelCopyBufferInfo {
            buffer: &buffer,
            layout: TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_byte_width),
                rows_per_image: None,
            },
        },
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|_| ShellError::Readback)?;
    rx.recv()
        .map_err(|_| ShellError::Readback)?
        .map_err(|_| ShellError::Readback)?;

    let data = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let start = (row * padded_byte_width) as usize;
        pixels.extend_from_slice(&data[start..start + (width * 4) as usize]);
    }
    drop(data);
    buffer.unmap();

    Ok(RgbaImage::from_raw(width, height, pixels)
        .expect("readback buffer matches image dimensions"))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::cpu_flag;

    #[test]
    fn cpu_flag_semantics() {
        assert!(!cpu_flag(None));
        assert!(!cpu_flag(Some(OsStr::new(""))));
        assert!(!cpu_flag(Some(OsStr::new("0"))));
        assert!(cpu_flag(Some(OsStr::new("1"))));
        assert!(cpu_flag(Some(OsStr::new("true"))));
    }
}
