//! Editor helpers ([`crate::Scene::helpers`]): what the C# RedForge shows in
//! its selection modes other than "Entity" (collisions, navmeshes, occluders,
//! interiors' rooms and portals...), as coloured meshes. They're drawn into a
//! layer of their own, with its own depth, which goes over the image at 85%,
//! as the C#'s helpers view does; they write their pick ids over the scene's.

use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::GpuContext;
use crate::outline::MASK_FORMAT;
use crate::renderer::Instance;
use crate::targets::{DEPTH_FORMAT, ID_FORMAT, OUTPUT_FORMAT};

const COLOUR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// A helper mesh's vertex: its colour is straight (not premultiplied) RGBA;
/// lines have no normal.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct HelperVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub colour: [u8; 4],
}

impl HelperVertex {
    pub fn new(position: Vec3, normal: Vec3, colour: [u8; 4]) -> Self {
        Self { position: position.to_array(), normal: normal.to_array(), colour }
    }

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 3] =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 5 => Unorm8x4];
        wgpu::VertexBufferLayout {
            array_stride: size_of::<HelperVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRS,
        }
    }
}

/// A helper's geometry on the GPU ([`crate::Renderer::create_helper_mesh`]).
pub struct HelperMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    pub lines: bool,
    /// Some of it is see-through: it's drawn after the rest, without hiding
    /// what's behind it.
    pub translucent: bool,
}

/// A helper mesh drawn with a range of the frame's instances
/// ([`Instance::helper`]).
pub struct HelperBatch<'a> {
    pub mesh: &'a HelperMesh,
    pub instances: Range<u32>,
}

impl Instance {
    /// A helper's instance: its colour (straight RGBA) multiplies its mesh's.
    pub fn helper(transform: Mat4, id: u32, colour: [u8; 4]) -> Self {
        Self { normal: u32::from_le_bytes(colour), ..Instance::new(transform, 0, id) }
    }
}

pub(crate) struct HelperPass {
    opaque: wgpu::RenderPipeline,
    translucent: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    mask: wgpu::RenderPipeline,
    overlay_layout: wgpu::BindGroupLayout,
    overlay: wgpu::RenderPipeline,
}

/// A view's helpers layer.
pub(crate) struct HelperTargets {
    pub colour: wgpu::TextureView,
    pub depth: wgpu::TextureView,
    group: wgpu::BindGroup,
}

impl HelperPass {
    pub fn new(
        ctx: &GpuContext,
        frame_layout: &wgpu::BindGroupLayout,
        instances: wgpu::VertexBufferLayout<'static>,
    ) -> Self {
        let device = &ctx.device;
        let source = [include_str!("shaders/common.wgsl"), include_str!("shaders/helpers.wgsl")].concat();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("helpers.wgsl"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("helpers"),
            bind_group_layouts: &[Some(frame_layout)],
            immediate_size: 0,
        });
        let buffers = [Some(HelperVertex::layout()), Some(instances)];
        let make = |label, fs, targets: &[Option<wgpu::ColorTargetState>], depth, topology| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_helper"),
                    compilation_options: Default::default(),
                    buffers: &buffers,
                },
                primitive: wgpu::PrimitiveState { topology, cull_mode: None, ..Default::default() },
                depth_stencil: depth,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        // Reversed Z, as the views.
        let depth = |write: bool| {
            Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(write),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: Default::default(),
                bias: Default::default(),
            })
        };
        let targets = [
            Some(wgpu::ColorTargetState {
                format: COLOUR_FORMAT,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            }),
            Some(ID_FORMAT.into()),
        ];
        let tri = wgpu::PrimitiveTopology::TriangleList;
        let opaque = make("helpers", "fs_helper", &targets, depth(true), tri);
        let translucent = make("helpers translucent", "fs_helper", &targets, depth(false), tri);
        let lines = make("helper lines", "fs_helper", &targets, depth(true), wgpu::PrimitiveTopology::LineList);
        let mask = make("helper outline mask", "fs_helper_mask", &[Some(MASK_FORMAT.into())], None, tri);

        let overlay_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("helpers overlay"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let overlay_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("helpers overlay"),
            bind_group_layouts: &[None, Some(&overlay_layout)],
            immediate_size: 0,
        });
        let overlay = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("helpers overlay"),
            layout: Some(&overlay_pl),
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
                entry_point: Some("fs_overlay"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: OUTPUT_FORMAT.remove_srgb_suffix(),
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::COLOR,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self { opaque, translucent, lines, mask, overlay_layout, overlay }
    }

    pub fn targets(&self, ctx: &GpuContext, width: u32, height: u32) -> HelperTargets {
        let texture = |label, format| {
            ctx.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let colour = texture("helpers", COLOUR_FORMAT);
        let depth = texture("helpers depth", DEPTH_FORMAT);
        let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("helpers overlay"),
            layout: &self.overlay_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&colour) }],
        });
        HelperTargets { colour, depth, group }
    }

    pub fn create_mesh(&self, ctx: &GpuContext, vertices: &[HelperVertex], indices: &[u32], lines: bool) -> HelperMesh {
        let buffer = |label, contents: &[u8], usage| {
            ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage })
        };
        HelperMesh {
            vertices: buffer("helper vertices", bytemuck::cast_slice(vertices), wgpu::BufferUsages::VERTEX),
            indices: buffer("helper indices", bytemuck::cast_slice(indices), wgpu::BufferUsages::INDEX),
            index_count: indices.len() as u32,
            lines,
            translucent: vertices.iter().any(|v| v.colour[3] < 255),
        }
    }

    /// Draws the helpers (the frame's instances bound): what hides what
    /// first, then what's see-through. Gives the draw calls.
    pub fn draw(&self, rp: &mut wgpu::RenderPass<'_>, batches: &[HelperBatch]) -> usize {
        let mut calls = 0;
        for see_through in [false, true] {
            for b in batches.iter().filter(|b| b.mesh.translucent == see_through && !b.instances.is_empty()) {
                let pipeline = match (b.mesh.lines, see_through) {
                    (true, _) => &self.lines,
                    (false, true) => &self.translucent,
                    (false, false) => &self.opaque,
                };
                rp.set_pipeline(pipeline);
                calls += draw_mesh(rp, b);
            }
        }
        calls
    }

    /// Draws helpers into an outline's mask.
    pub fn draw_mask(&self, rp: &mut wgpu::RenderPass<'_>, batches: &[HelperBatch]) -> usize {
        rp.set_pipeline(&self.mask);
        batches.iter().filter(|b| !b.mesh.lines && !b.instances.is_empty()).map(|b| draw_mesh(rp, b)).sum()
    }

    /// Puts the layer over `output`.
    pub fn overlay(&self, encoder: &mut wgpu::CommandEncoder, targets: &HelperTargets, output: &wgpu::TextureView) {
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("helpers overlay"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        rp.set_pipeline(&self.overlay);
        rp.set_bind_group(1, &targets.group, &[]);
        rp.draw(0..3, 0..1);
    }
}

fn draw_mesh(rp: &mut wgpu::RenderPass<'_>, b: &HelperBatch) -> usize {
    rp.set_vertex_buffer(0, b.mesh.vertices.slice(..));
    rp.set_index_buffer(b.mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
    rp.draw_indexed(0..b.mesh.index_count, 0, b.instances.clone());
    1
}
