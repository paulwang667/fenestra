//! Offscreen layer targets, pooled across frames by size.

use crate::gpu::Attachments;

/// Frames a pooled target may go unused before it is dropped.
const KEEP_FRAMES: u64 = 60;

/// Where an offscreen layer renders: MSAA attachments resolved into a
/// texture the parent samples.
pub(crate) struct LayerTarget {
    pub msaa: Attachments,
    pub view: wgpu::TextureView,
}

/// Layer targets not in use this frame, with the frame each was last used.
#[derive(Default)]
pub(crate) struct LayerPool {
    free: Vec<(u64, LayerTarget)>,
    frame: u64,
}

impl LayerPool {
    /// Starts a frame.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    /// Pooled targets.
    pub fn len(&self) -> usize {
        self.free.len()
    }

    /// A target of exactly `size`, reused when one is free; `created` counts
    /// new ones.
    pub fn take(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        size: (u32, u32),
        created: &mut u32,
    ) -> LayerTarget {
        if let Some(i) = self.free.iter().position(|(_, t)| t.msaa.size == size) {
            return self.free.swap_remove(i).1;
        }
        *created += 1;
        let resolve = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fenestra-raster layer"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        LayerTarget {
            msaa: Attachments::new(device, format, size),
            view: resolve.create_view(&wgpu::TextureViewDescriptor::default()),
        }
    }

    /// Returns a target after the frame used it.
    pub fn give_back(&mut self, target: LayerTarget) {
        self.free.push((self.frame, target));
    }

    /// Drops targets unused for [`KEEP_FRAMES`] frames.
    pub fn trim(&mut self) {
        let frame = self.frame;
        self.free.retain(|(used, _)| frame - used <= KEEP_FRAMES);
    }
}
