//! Pipelines and render attachments.

use crate::geometry::{ImageV, PaintV};

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

/// What a draw does to the clip stencil. Every draw only touches samples
/// where the stencil equals its reference (the clip level it was recorded at).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StencilOp {
    /// Draws color, leaving the stencil.
    Test,
    /// Raises the stencil by one inside the shape (pushing a clip).
    Increment,
    /// Lowers it by one inside the shape (popping a clip).
    Decrement,
}

fn stencil_state(op: StencilOp) -> wgpu::DepthStencilState {
    let pass_op = match op {
        StencilOp::Test => wgpu::StencilOperation::Keep,
        StencilOp::Increment => wgpu::StencilOperation::IncrementClamp,
        StencilOp::Decrement => wgpu::StencilOperation::DecrementClamp,
    };
    let face = wgpu::StencilFaceState {
        compare: wgpu::CompareFunction::Equal,
        fail_op: wgpu::StencilOperation::Keep,
        depth_fail_op: wgpu::StencilOperation::Keep,
        pass_op,
    };
    wgpu::DepthStencilState {
        format: STENCIL,
        depth_write_enabled: Some(false),
        depth_compare: Some(wgpu::CompareFunction::Always),
        stencil: wgpu::StencilState {
            front: face,
            back: face,
            read_mask: 0xff,
            write_mask: if op == StencilOp::Test { 0 } else { 0xff },
        },
        bias: wgpu::DepthBiasState::default(),
    }
}

/// Every pipeline the renderer draws with, for one target format.
pub(crate) struct Pipelines {
    /// Group 0: the pass's globals and the gradient ramps.
    pub globals_layout: wgpu::BindGroupLayout,
    /// Group 1 of textured quads: a texture and its sampler.
    pub texture_layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
    pub paint: wgpu::RenderPipeline,
    pub clip_push: wgpu::RenderPipeline,
    pub clip_pop: wgpu::RenderPipeline,
    pub image: wgpu::RenderPipeline,
}

impl Pipelines {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fenestra-raster"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders.wgsl").into()),
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fenestra-raster globals"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<Globals>() as u64
                        ),
                    },
                    count: None,
                },
                // Gradient ramps, read with textureLoad (nearest sample, as vello does).
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fenestra-raster texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let paint_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fenestra-raster paint"),
            bind_group_layouts: &[Some(&globals_layout)],
            immediate_size: 0,
        });
        let textured_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fenestra-raster textured"),
            bind_group_layouts: &[Some(&globals_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let paint_buffers = [wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<PaintV>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4
            ],
        }];
        let image_buffers = [wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ImageV>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4],
        }];
        let pipeline = |label: &str,
                        layout: &wgpu::PipelineLayout,
                        buffers: &[wgpu::VertexBufferLayout<'_>],
                        (vs, fs): (&str, &str),
                        op: StencilOp| {
            let write_mask = if op == StencilOp::Test {
                wgpu::ColorWrites::ALL
            } else {
                wgpu::ColorWrites::empty()
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(stencil_state(op)),
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
                        write_mask,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let paint = ("vs_paint", "fs_paint");
        let stencil = ("vs_paint", "fs_stencil");
        Self {
            paint: pipeline(
                "fenestra-raster paint",
                &paint_layout,
                &paint_buffers,
                paint,
                StencilOp::Test,
            ),
            clip_push: pipeline(
                "fenestra-raster clip push",
                &paint_layout,
                &paint_buffers,
                stencil,
                StencilOp::Increment,
            ),
            clip_pop: pipeline(
                "fenestra-raster clip pop",
                &paint_layout,
                &paint_buffers,
                stencil,
                StencilOp::Decrement,
            ),
            image: pipeline(
                "fenestra-raster image",
                &textured_layout,
                &image_buffers,
                ("vs_image", "fs_image"),
                StencilOp::Test,
            ),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("fenestra-raster bilinear"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            globals_layout,
            texture_layout,
        }
    }

    /// Binds `view` for a textured-quad draw.
    pub fn texture_bind(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fenestra-raster texture"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
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
