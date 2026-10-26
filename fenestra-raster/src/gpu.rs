//! Pipelines and render attachments.

use crate::geometry::PaintV;

/// Samples per pixel of every color and stencil attachment.
pub(crate) const MSAA: u32 = 4;
/// The clip stencil.
pub(crate) const STENCIL: wgpu::TextureFormat = wgpu::TextureFormat::Stencil8;
/// Bytes between two passes' [`Globals`] in the uniform buffer (wgpu's
/// minimum dynamic-offset alignment).
pub(crate) const GLOBALS_STRIDE: u64 = 256;

/// The device rectangle a pass renders (`shaders.wgsl` `Globals`).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Globals {
    pub origin: [f32; 2],
    pub size: [f32; 2],
}

/// Premultiplied source-over, in the target's (sRGB-encoded) values.
const OVER: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
};

/// Draws where the stencil equals the draw's clip level, leaving it as is.
fn stencil_state() -> wgpu::DepthStencilState {
    let face = wgpu::StencilFaceState {
        compare: wgpu::CompareFunction::Equal,
        fail_op: wgpu::StencilOperation::Keep,
        depth_fail_op: wgpu::StencilOperation::Keep,
        pass_op: wgpu::StencilOperation::Keep,
    };
    wgpu::DepthStencilState {
        format: STENCIL,
        depth_write_enabled: Some(false),
        depth_compare: Some(wgpu::CompareFunction::Always),
        stencil: wgpu::StencilState {
            front: face,
            back: face,
            read_mask: 0xff,
            write_mask: 0,
        },
        bias: wgpu::DepthBiasState::default(),
    }
}

/// Every pipeline the renderer draws with, for one target format.
pub(crate) struct Pipelines {
    pub globals_layout: wgpu::BindGroupLayout,
    pub paint: wgpu::RenderPipeline,
}

impl Pipelines {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fenestra-raster"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders.wgsl").into()),
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fenestra-raster globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Globals>() as u64),
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fenestra-raster paint"),
            bind_group_layouts: &[Some(&globals_layout)],
            immediate_size: 0,
        });
        let paint_buffers = [wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<PaintV>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4
            ],
        }];
        let pipeline = |label: &str, fs: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_paint"),
                    compilation_options: Default::default(),
                    buffers: &paint_buffers,
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(stencil_state()),
                multisample: wgpu::MultisampleState {
                    count: MSAA,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(OVER),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            paint: pipeline("fenestra-raster paint", "fs_paint"),
            globals_layout,
        }
    }
}

/// The multisampled color and stencil a pass of `size` renders into before
/// resolving.
pub(crate) struct Attachments {
    pub size: (u32, u32),
    pub color: wgpu::TextureView,
    pub stencil: wgpu::TextureView,
}

impl Attachments {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, size: (u32, u32)) -> Self {
        let texture = |label: &str, format: wgpu::TextureFormat| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: MSAA,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        Self {
            size,
            color: texture("fenestra-raster msaa color", format),
            stencil: texture("fenestra-raster msaa stencil", STENCIL),
        }
    }
}
