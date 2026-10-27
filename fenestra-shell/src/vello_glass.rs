//! The vello backend's GPU glass: the raster backend's [`GpuGlass`] run on
//! the backdrop texture vello rendered, its results handed to vello as image
//! overrides — vello copies each into its image atlas on the GPU, so nothing
//! is read back and nothing is uploaded.

use std::collections::HashMap;

use fenestra_core::WidgetId;
use fenestra_raster::{GlassPane, GpuGlass};
use vello::peniko::ImageData;
use vello::wgpu;

/// One window's (or one headless renderer's) GPU glass for vello.
#[derive(Default)]
pub(crate) struct VelloGlass {
    glass: GpuGlass,
    /// The device (index into the renderers) the glass and the overrides
    /// live on: a window's surface can come back on another adapter.
    device: Option<usize>,
    /// The placeholders handed to that device's renderer as overrides, by
    /// blob id.
    registered: HashMap<u64, ImageData>,
}

impl VelloGlass {
    /// Composites `panes` (frame coordinates) on device `dev_id` (whose
    /// vello renderer is `renderers[dev_id]`) from `source`, the backdrop
    /// region vello rendered with its top-left at `origin` in the frame, and
    /// makes each result's placeholder draw it in that renderer.
    /// Placeholders registered last time and not this time are withdrawn;
    /// on a new device everything is withdrawn from the old one and the
    /// glass rebuilt. Returns each pane's placeholder, for the final pass to
    /// draw as baked.
    pub(crate) fn composite(
        &mut self,
        (dev_id, device, queue): (usize, &wgpu::Device, &wgpu::Queue),
        renderers: &mut [Option<vello::Renderer>],
        source: &wgpu::Texture,
        (ox, oy): (u32, u32),
        panes: &[GlassPane],
    ) -> HashMap<WidgetId, ImageData> {
        if self.device != Some(dev_id) {
            self.release(renderers);
            self.glass = GpuGlass::default();
            self.device = Some(dev_id);
        }
        let Some(renderer) = renderers.get_mut(dev_id).and_then(Option::as_mut) else {
            return HashMap::new();
        };
        let (ox, oy) = (i(ox), i(oy));
        let shifted: Vec<GlassPane> = panes
            .iter()
            .map(|p| GlassPane {
                rect: (p.rect.0 - ox, p.rect.1 - oy, p.rect.2, p.rect.3),
                ..*p
            })
            .collect();
        let (results, _) = self.glass.run(device, queue, source, &shifted);
        let mut out = HashMap::with_capacity(results.len());
        let mut now = HashMap::with_capacity(results.len());
        for r in results {
            renderer.override_image(
                &r.placeholder,
                Some(wgpu::TexelCopyTextureInfoBase {
                    texture: r.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: r.region[0],
                        y: r.region[1],
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                }),
            );
            now.insert(r.placeholder.data.id(), r.placeholder.clone());
            out.insert(r.id, r.placeholder);
        }
        for (id, image) in self.registered.drain() {
            if !now.contains_key(&id) {
                renderer.override_image(&image, None);
            }
        }
        self.registered = now;
        out
    }

    /// Withdraws every placeholder from the renderer they were handed to:
    /// for a frame that draws no GPU glass (no panes, or the CPU path), or
    /// before moving to another device.
    pub(crate) fn release(&mut self, renderers: &mut [Option<vello::Renderer>]) {
        if let Some(renderer) = self
            .device
            .and_then(|d| renderers.get_mut(d))
            .and_then(Option::as_mut)
        {
            for image in self.registered.values() {
                renderer.override_image(image, None);
            }
        }
        self.registered.clear();
    }

    /// Placeholders vello currently draws from the GPU glass.
    pub(crate) fn registered(&self) -> usize {
        self.registered.len()
    }
}

/// A texture for vello to render a backdrop region into that the GPU glass
/// can then read in linear light: storage (vello writes it), sampled
/// through its sRGB view, and copyable (the CPU filters read it back).
pub(crate) fn render_target(device: &wgpu::Device, (w, h): (u32, u32)) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fenestra backdrop region"),
        size: wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
    })
}

#[expect(clippy::cast_possible_wrap, reason = "frame coordinates fit in i32")]
fn i(v: u32) -> i32 {
    v as i32
}
