//! GPU instance, devices and window surfaces.
//!
//! Adapted from vello 0.9's `util` module (Apache-2.0 OR MIT), which the
//! shell used before. The one behavioral difference: a device asks for
//! [`wgpu::Limits::default`] *capped at what the adapter offers*
//! ([`device_limits`]), where vello asks for the defaults outright. The iOS
//! simulator offers 15 inter-stage shader variables against the default 16,
//! so vello's request fails there and the window never gets a device; and
//! vello's `DeviceHandle` keeps its adapter private, so a device cannot be
//! created elsewhere and handed to it.

use wgpu::{
    Adapter, Device, Instance, Queue, Surface, SurfaceConfiguration, SurfaceTarget, Texture,
    TextureFormat, TextureView, util::TextureBlitter,
};

/// The wgpu instance and every device created so far.
pub(crate) struct GpuContext {
    pub instance: Instance,
    pub devices: Vec<DeviceHandle>,
}

/// One device with the adapter it came from.
pub(crate) struct DeviceHandle {
    adapter: Adapter,
    pub device: Device,
    pub queue: Queue,
}

impl DeviceHandle {
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "the web never asks which adapter it got")
    )]
    pub fn adapter(&self) -> &Adapter {
        &self.adapter
    }
}

/// A window surface, its configuration, and the intermediate texture vello
/// renders into (its compute pass cannot bind the surface texture), blitted
/// to the surface on present.
pub(crate) struct GpuSurface<'s> {
    pub surface: Surface<'s>,
    pub config: SurfaceConfiguration,
    pub dev_id: usize,
    pub target_texture: Texture,
    pub target_view: TextureView,
    pub blitter: TextureBlitter,
}

/// The limits a device asks for: the defaults, lowered to the adapter's
/// wherever the adapter offers less.
pub(crate) fn device_limits(adapter: &wgpu::Limits) -> wgpu::Limits {
    wgpu::Limits::default().or_worse_values_from(adapter)
}

impl GpuContext {
    pub fn new() -> Self {
        let instance = Instance::new(wgpu::InstanceDescriptor {
            display: None,
            backends: wgpu::Backends::from_env().unwrap_or_default(),
            flags: wgpu::InstanceFlags::from_build_config().with_env(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: wgpu::BackendOptions::from_env_or_default(),
        });
        Self {
            instance,
            devices: Vec::new(),
        }
    }

    /// Creates a surface for `window` on a compatible device (creating the
    /// device if none fits yet) and configures it.
    pub async fn create_surface<'w>(
        &mut self,
        window: impl Into<SurfaceTarget<'w>>,
        width: u32,
        height: u32,
        present_mode: wgpu::PresentMode,
    ) -> Result<GpuSurface<'w>, vello::Error> {
        let surface = self.instance.create_surface(window.into())?;
        let dev_id = self
            .device(Some(&surface))
            .await
            .ok_or(vello::Error::NoCompatibleDevice)?;
        let handle = &self.devices[dev_id];
        let format = surface
            .get_capabilities(&handle.adapter)
            .formats
            .into_iter()
            .find(|f| matches!(f, TextureFormat::Rgba8Unorm | TextureFormat::Bgra8Unorm))
            .ok_or(vello::Error::UnsupportedSurfaceFormat)?;
        let config = SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };
        let (target_texture, target_view) = create_targets(width, height, &handle.device);
        let surface = GpuSurface {
            surface,
            config,
            dev_id,
            target_texture,
            target_view,
            blitter: TextureBlitter::new(&handle.device, format),
        };
        surface.surface.configure(&handle.device, &surface.config);
        Ok(surface)
    }

    /// Re-applies `surface`'s configuration (after a lost or outdated
    /// surface).
    pub fn configure_surface(&self, surface: &GpuSurface<'_>) {
        surface
            .surface
            .configure(&self.devices[surface.dev_id].device, &surface.config);
    }

    /// Resizes `surface` and its intermediate texture.
    pub fn resize_surface(&self, surface: &mut GpuSurface<'_>, width: u32, height: u32) {
        let device = &self.devices[surface.dev_id].device;
        let (texture, view) = create_targets(width, height, device);
        surface.target_texture = texture;
        surface.target_view = view;
        surface.config.width = width;
        surface.config.height = height;
        surface.surface.configure(device, &surface.config);
    }

    /// A device compatible with `compatible_surface` (any device when
    /// `None`), created if none exists yet.
    pub async fn device(&mut self, compatible_surface: Option<&Surface<'_>>) -> Option<usize> {
        let compatible = match compatible_surface {
            Some(s) => self
                .devices
                .iter()
                .position(|d| d.adapter.is_surface_supported(s)),
            None => (!self.devices.is_empty()).then_some(0),
        };
        match compatible {
            Some(i) => Some(i),
            None => self.new_device(compatible_surface).await,
        }
    }

    async fn new_device(&mut self, compatible_surface: Option<&Surface<'_>>) -> Option<usize> {
        let adapter =
            wgpu::util::initialize_adapter_from_env_or_default(&self.instance, compatible_surface)
                .await
                .ok()?;
        let wanted = wgpu::Features::CLEAR_TEXTURE | wgpu::Features::PIPELINE_CACHE;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: None,
                required_features: adapter.features() & wanted,
                required_limits: device_limits(&adapter.limits()),
                ..Default::default()
            })
            .await
            .ok()?;
        self.devices.push(DeviceHandle {
            adapter,
            device,
            queue,
        });
        Some(self.devices.len() - 1)
    }
}

/// The intermediate texture vello renders into.
fn create_targets(width: u32, height: u32, device: &Device) -> (Texture, TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        format: TextureFormat::Rgba8Unorm,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_drop_to_what_the_adapter_offers() {
        let adapter = wgpu::Limits {
            max_inter_stage_shader_variables: 15,
            ..wgpu::Limits::default()
        };
        let got = device_limits(&adapter);
        assert_eq!(got.max_inter_stage_shader_variables, 15);
        assert_eq!(
            got.max_storage_buffers_per_shader_stage,
            wgpu::Limits::default().max_storage_buffers_per_shader_stage
        );
    }

    #[test]
    fn a_stronger_adapter_still_gets_the_defaults() {
        let adapter = wgpu::Limits {
            max_inter_stage_shader_variables: 31,
            max_texture_dimension_2d: 16384,
            ..wgpu::Limits::default()
        };
        assert_eq!(device_limits(&adapter), wgpu::Limits::default());
    }
}
