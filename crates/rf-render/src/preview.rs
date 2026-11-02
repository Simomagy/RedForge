//! Drawing a texture as it is stored, for the texture viewer.

use bytemuck::{Pod, Zeroable};

use crate::GpuContext;
use crate::targets::OUTPUT_FORMAT;
use crate::texture::GpuTexture;

/// Which part of a texture to show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreviewMode {
    /// Colours over a checkerboard, by alpha.
    #[default]
    Rgba,
    Rgb,
    Alpha,
    Red,
    Green,
    Blue,
    /// A normal map with X and Y in red and green, Z rebuilt.
    Normal,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    values: [f32; 4],
    size: [f32; 4],
}

pub struct TexturePreview {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
}

/// An 8-bit sRGB image of a texture, to show with egui.
pub struct PreviewTarget {
    pub width: u32,
    pub height: u32,
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    /// The image seen as plain UNORM, as egui expects (see `RenderTargets`).
    pub display_view: wgpu::TextureView,
    params: wgpu::Buffer,
}

impl TexturePreview {
    pub fn new(ctx: &GpuContext) -> Self {
        let device = &ctx.device;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("preview"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("preview.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/preview.wgsl").into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("preview"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("preview"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(OUTPUT_FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("preview"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self { layout, pipeline, sampler }
    }

    pub fn create_target(&self, ctx: &GpuContext, width: u32, height: u32) -> PreviewTarget {
        let (width, height) = (width.max(1), height.max(1));
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("preview"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[OUTPUT_FORMAT.remove_srgb_suffix()],
        });
        let view = texture.create_view(&Default::default());
        let display_view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(OUTPUT_FORMAT.remove_srgb_suffix()),
            ..Default::default()
        });
        let params = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("preview"),
            size: size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        PreviewTarget { width, height, texture, view, display_view, params }
    }

    /// Draws one mip and layer of `texture` into `target`.
    pub fn draw(
        &self,
        ctx: &GpuContext,
        texture: &GpuTexture,
        target: &PreviewTarget,
        mip: u32,
        layer: u32,
        mode: PreviewMode,
    ) {
        let p = Params {
            values: [mip.min(texture.mips - 1) as f32, layer.min(texture.layers - 1) as f32, mode as u32 as f32, 0.0],
            size: [target.width as f32, target.height as f32, 0.0, 0.0],
        };
        ctx.queue.write_buffer(&target.params, 0, bytemuck::bytes_of(&p));
        let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("preview"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&texture.view_raw) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: target.params.as_entire_binding() },
            ],
        });
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("preview"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        ctx.queue.submit([encoder.finish()]);
    }
}
