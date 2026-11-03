//! Outlines of what's hovered and selected ([`crate::Scene::outlines`]), as
//! the C#'s `OutlineShader`: each layer's models are drawn into a mask, which
//! is widened across, then down, into a band of the layer's colour over the
//! output.

use crate::GpuContext;
use crate::targets::OUTPUT_FORMAT;

pub(crate) const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
/// The most outline layers a frame draws.
pub const MAX_OUTLINES: usize = 8;
/// Each layer's colour in the uniform buffer, at a dynamic offset.
const STRIDE: u64 = 256;

pub(crate) struct OutlinePass {
    textures_layout: wgpu::BindGroupLayout,
    across: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    colours: wgpu::Buffer,
    colours_group: wgpu::BindGroup,
}

/// A view's mask and its widened copy.
pub(crate) struct OutlineTargets {
    pub mask: wgpu::TextureView,
    wide: wgpu::TextureView,
    /// The mask (twice, as the widened copy is drawn into), then the mask
    /// and the widened copy.
    across: wgpu::BindGroup,
    down: wgpu::BindGroup,
}

impl OutlinePass {
    pub fn new(ctx: &GpuContext) -> Self {
        let device = &ctx.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("outline.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/outline.wgsl").into()),
        });
        let colours_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("outline colour"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(16),
                },
                count: None,
            }],
        });
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let textures_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("outline textures"),
            entries: &[texture(0), texture(1)],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("outline"),
            bind_group_layouts: &[Some(&colours_layout), Some(&textures_layout)],
            immediate_size: 0,
        });
        let fullscreen = |label, entry_point, target: wgpu::ColorTargetState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_fullscreen"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry_point),
                    compilation_options: Default::default(),
                    targets: &[Some(target)],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let across = fullscreen("outline across", "fs_across", MASK_FORMAT.into());
        let down = fullscreen(
            "outline down",
            "fs_down",
            wgpu::ColorTargetState {
                format: OUTPUT_FORMAT.remove_srgb_suffix(),
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::COLOR,
            },
        );
        let colours = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("outline colours"),
            size: STRIDE * MAX_OUTLINES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let colours_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("outline colour"),
            layout: &colours_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &colours,
                    offset: 0,
                    size: wgpu::BufferSize::new(16),
                }),
            }],
        });
        Self { textures_layout, across, down, colours, colours_group }
    }

    pub fn targets(&self, ctx: &GpuContext, width: u32, height: u32) -> OutlineTargets {
        let texture = |label| {
            ctx.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: MASK_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let (mask, wide) = (texture("outline mask"), texture("outline wide"));
        let group = |second| {
            ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("outline textures"),
                layout: &self.textures_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&mask) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(second) },
                ],
            })
        };
        let (across, down) = (group(&mask), group(&wide));
        OutlineTargets { mask, wide, across, down }
    }

    /// Writes the layers' colours, before the frame is recorded.
    pub fn set_colours(&self, ctx: &GpuContext, colours: &[[f32; 4]]) {
        let mut bytes = vec![0u8; (STRIDE as usize) * colours.len().min(MAX_OUTLINES)];
        for (i, c) in colours.iter().take(MAX_OUTLINES).enumerate() {
            let at = i * STRIDE as usize;
            bytes[at..at + 16].copy_from_slice(bytemuck::cast_slice(c));
        }
        if !bytes.is_empty() {
            ctx.queue.write_buffer(&self.colours, 0, &bytes);
        }
    }

    /// Widens layer `layer`'s mask (just drawn into `targets.mask`) into its
    /// band over `output`.
    pub fn run(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        targets: &OutlineTargets,
        output: &wgpu::TextureView,
        layer: usize,
    ) {
        let offset = (layer as u64 * STRIDE) as u32;
        let pass = |encoder: &mut wgpu::CommandEncoder, label, view, load, pipeline, group| {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            rp.set_pipeline(pipeline);
            rp.set_bind_group(0, &self.colours_group, &[offset]);
            rp.set_bind_group(1, group, &[]);
            rp.draw(0..3, 0..1);
        };
        let clear = wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT);
        pass(encoder, "outline across", &targets.wide, clear, &self.across, &targets.across);
        pass(encoder, "outline down", output, wgpu::LoadOp::Load, &self.down, &targets.down);
    }
}
