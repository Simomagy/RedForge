//! The renderer: forward shading into an HDR target, then a filmic tonemap.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::GpuContext;
use crate::camera::OrbitCamera;
use crate::mesh::Vertex;
use crate::model::{AlphaMode, GpuModel, MaterialDesc};
use crate::texture::GpuTexture;

pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// What the final image is: 8-bit sRGB, which egui can show directly.
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    sun_dir: [f32; 4],
    sun_colour: [f32; 4],
    sky_colour: [f32; 4],
    ground_colour: [f32; 4],
    options: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MaterialUniform {
    tint: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TonemapUniform {
    values: [f32; 4],
}

/// What to show instead of the lit image.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DebugView {
    #[default]
    Lit,
    Albedo,
    Normals,
    /// Metalness in red, roughness in green.
    Material,
    VertexColour,
}

#[derive(Clone, Debug)]
pub struct ViewSettings {
    pub debug: DebugView,
    pub lod: usize,
    /// Towards the sun.
    pub sun_dir: Vec3,
    pub sun_colour: Vec3,
    pub sky_colour: Vec3,
    pub ground_colour: Vec3,
    pub background: [f64; 3],
    pub exposure: f32,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            debug: DebugView::Lit,
            lod: 0,
            sun_dir: Vec3::new(0.4, -0.6, 0.7).normalize(),
            sun_colour: Vec3::new(3.2, 3.0, 2.7),
            sky_colour: Vec3::new(0.55, 0.65, 0.8),
            ground_colour: Vec3::new(0.25, 0.22, 0.18),
            background: [0.18, 0.2, 0.24],
            exposure: 1.6,
        }
    }
}

/// A GPU material: its constants and textures.
pub struct GpuMaterial {
    pub name: String,
    pub bind_group: wgpu::BindGroup,
    /// For showing what a material uses.
    pub textures: [Option<Arc<GpuTexture>>; 3],
}

/// The textures a view renders into, resized with it.
pub struct RenderTargets {
    pub width: u32,
    pub height: u32,
    hdr: wgpu::TextureView,
    depth: wgpu::TextureView,
    pub output: wgpu::Texture,
    pub output_view: wgpu::TextureView,
    /// The output seen as plain UNORM: the sRGB-encoded bytes as they are, which
    /// is what egui expects of a texture.
    pub display_view: wgpu::TextureView,
    tonemap_group: wgpu::BindGroup,
}

pub struct Renderer {
    material_layout: wgpu::BindGroupLayout,
    tonemap_layout: wgpu::BindGroupLayout,
    frame_buffer: wgpu::Buffer,
    tonemap_buffer: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    pipelines: Pipelines,
    tonemap: wgpu::RenderPipeline,
    white: Arc<GpuTexture>,
    flat_normal: Arc<GpuTexture>,
    default_material: Arc<GpuMaterial>,
}

struct Pipelines {
    opaque: wgpu::RenderPipeline,
    blend: wgpu::RenderPipeline,
    decal: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture_entry(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// The instance transforms, one per draw, as a vertex buffer.
fn instance_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 4] =
        wgpu::vertex_attr_array![7 => Float32x4, 8 => Float32x4, 9 => Float32x4, 10 => Float32x4];
    wgpu::VertexBufferLayout { array_stride: 64, step_mode: wgpu::VertexStepMode::Instance, attributes: &ATTRS }
}

impl Renderer {
    pub fn new(ctx: &GpuContext) -> Self {
        let device = &ctx.device;
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::FRAGMENT),
                texture_entry(1, true),
                texture_entry(2, true),
                texture_entry(3, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let tonemap_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tonemap"),
            entries: &[texture_entry(0, false), uniform_entry(1, wgpu::ShaderStages::FRAGMENT)],
        });
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame"),
            size: size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let tonemap_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tonemap"),
            size: size_of::<TonemapUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: frame_buffer.as_entire_binding() }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("wrap"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 16,
            ..Default::default()
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("model.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/model.wgsl").into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("model"),
            bind_group_layouts: &[Some(&frame_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let pipeline = |label: &str, topology, blend: Option<wgpu::BlendState>, depth_write: bool, bias: i32| {
            let buffers = [Some(Vertex::layout()), Some(instance_layout())];
            let targets =
                [Some(wgpu::ColorTargetState { format: HDR_FORMAT, blend, write_mask: wgpu::ColorWrites::ALL })];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &buffers,
                },
                primitive: wgpu::PrimitiveState { topology, cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth_write),
                    // Reversed Z.
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                    stencil: Default::default(),
                    bias: wgpu::DepthBiasState {
                        constant: bias,
                        slope_scale: if bias != 0 { 1.0 } else { 0.0 },
                        clamp: 0.0,
                    },
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let tri = wgpu::PrimitiveTopology::TriangleList;
        let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);
        let pipelines = Pipelines {
            opaque: pipeline("opaque", tri, None, true, 0),
            blend: pipeline("blend", tri, alpha, false, 0),
            decal: pipeline("decal", tri, alpha, false, 2),
            lines: pipeline("lines", wgpu::PrimitiveTopology::LineList, None, true, 0),
        };

        let tonemap_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tonemap.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/tonemap.wgsl").into()),
        });
        let tonemap_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("tonemap"),
            bind_group_layouts: &[Some(&tonemap_layout)],
            immediate_size: 0,
        });
        let tonemap = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tonemap"),
            layout: Some(&tonemap_pl),
            vertex: wgpu::VertexState {
                module: &tonemap_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &tonemap_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(OUTPUT_FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });

        let white = Arc::new(GpuTexture::solid(ctx, "white", [255, 255, 255, 255]));
        let flat_normal = Arc::new(GpuTexture::solid(ctx, "flat normal", [128, 128, 255, 255]));
        let mut r = Self {
            material_layout,
            tonemap_layout,
            frame_buffer,
            tonemap_buffer,
            frame_group,
            sampler,
            pipelines,
            tonemap,
            white,
            flat_normal,
            default_material: Arc::new(placeholder_material(ctx)),
        };
        let grey = Arc::new(GpuTexture::solid(ctx, "grey", [160, 160, 160, 255]));
        r.default_material = Arc::new(r.create_material(
            ctx,
            &MaterialDesc { bumpiness: 1.0, ..Default::default() },
            Some(grey),
            None,
            None,
        ));
        r
    }

    pub fn default_material(&self) -> Arc<GpuMaterial> {
        self.default_material.clone()
    }

    pub fn create_material(
        &self,
        ctx: &GpuContext,
        desc: &MaterialDesc,
        albedo: Option<Arc<GpuTexture>>,
        normal: Option<Arc<GpuTexture>>,
        params: Option<Arc<GpuTexture>>,
    ) -> GpuMaterial {
        let cutoff = match desc.alpha {
            AlphaMode::Cutout => 0.5,
            AlphaMode::Opaque => 0.0,
            AlphaMode::Blend | AlphaMode::Decal => 0.01,
        };
        let u = MaterialUniform {
            tint: [1.0; 4],
            params: [
                desc.bumpiness,
                cutoff,
                if normal.is_some() { 1.0 } else { 0.0 },
                if params.is_some() { 1.0 } else { 0.0 },
            ],
        };
        let buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&desc.shader_name),
            contents: bytemuck::bytes_of(&u),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let a = albedo.as_deref().unwrap_or(&self.white);
        let n = normal.as_deref().unwrap_or(&self.flat_normal);
        let p = params.as_deref().unwrap_or(&self.white);
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&desc.shader_name),
            layout: &self.material_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&a.view_2d) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&n.view_2d) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&p.view_2d) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        GpuMaterial { name: desc.shader_name.clone(), bind_group, textures: [albedo, normal, params] }
    }

    pub fn create_targets(&self, ctx: &GpuContext, width: u32, height: u32) -> RenderTargets {
        let (width, height) = (width.max(1), height.max(1));
        let texture = |label, format: wgpu::TextureFormat, usage| {
            ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[format.remove_srgb_suffix()],
            })
        };
        let rt = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let hdr =
            texture("hdr", HDR_FORMAT, rt | wgpu::TextureUsages::TEXTURE_BINDING).create_view(&Default::default());
        let depth = texture("depth", DEPTH_FORMAT, rt).create_view(&Default::default());
        let output =
            texture("output", OUTPUT_FORMAT, rt | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC);
        let output_view = output.create_view(&Default::default());
        let display_view = output.create_view(&wgpu::TextureViewDescriptor {
            format: Some(OUTPUT_FORMAT.remove_srgb_suffix()),
            ..Default::default()
        });
        let tonemap_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tonemap"),
            layout: &self.tonemap_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&hdr) },
                wgpu::BindGroupEntry { binding: 1, resource: self.tonemap_buffer.as_entire_binding() },
            ],
        });
        RenderTargets { width, height, hdr, depth, output, output_view, display_view, tonemap_group }
    }

    /// Draws `models` (with their transforms) into `targets.output`.
    pub fn render(
        &self,
        ctx: &GpuContext,
        targets: &RenderTargets,
        camera: &OrbitCamera,
        settings: &ViewSettings,
        models: &[(&GpuModel, Mat4)],
    ) {
        let aspect = targets.width as f32 / targets.height as f32;
        let view_proj = camera.projection(aspect) * camera.view();
        let debug = settings.debug as u32;
        let frame = FrameUniform {
            view_proj: view_proj.to_cols_array_2d(),
            camera_pos: camera.eye().extend(1.0).to_array(),
            sun_dir: settings.sun_dir.normalize().extend(0.0).to_array(),
            sun_colour: settings.sun_colour.extend(0.0).to_array(),
            sky_colour: settings.sky_colour.extend(0.0).to_array(),
            ground_colour: settings.ground_colour.extend(0.0).to_array(),
            options: [debug, 0, 0, 0],
        };
        ctx.queue.write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&frame));
        let passthrough = if settings.debug == DebugView::Lit { 0.0 } else { 1.0 };
        let tm = TonemapUniform { values: [settings.exposure, 11.2, passthrough, 0.0] };
        ctx.queue.write_buffer(&self.tonemap_buffer, 0, bytemuck::bytes_of(&tm));

        // One instance transform per model; draws index into it.
        let instances: Vec<[f32; 16]> = models.iter().map(|(_, m)| m.to_cols_array()).collect();
        let instance_buffer = (!instances.is_empty()).then(|| {
            ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("instances"),
                contents: bytemuck::cast_slice(&instances),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let [r, g, b] = settings.background;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.hdr,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.frame_group, &[]);
            if let Some(ib) = &instance_buffer {
                pass.set_vertex_buffer(1, ib.slice(..));
            }
            // Opaque and cut-out first, then decals, then blended draws.
            for phase in [AlphaMode::Opaque, AlphaMode::Decal, AlphaMode::Blend] {
                for (mi, (model, _)) in models.iter().enumerate() {
                    let lod = settings.lod.min(3);
                    let lod = if model.lods[lod].is_empty() { model.highest_lod() } else { lod };
                    for draw in &model.lods[lod] {
                        let draw_phase = if draw.alpha == AlphaMode::Cutout { AlphaMode::Opaque } else { draw.alpha };
                        if draw_phase != phase {
                            continue;
                        }
                        let pipeline = match (draw.mesh.lines, phase) {
                            (true, _) => &self.pipelines.lines,
                            (_, AlphaMode::Blend) => &self.pipelines.blend,
                            (_, AlphaMode::Decal) => &self.pipelines.decal,
                            _ => &self.pipelines.opaque,
                        };
                        pass.set_pipeline(pipeline);
                        pass.set_bind_group(1, &draw.material.bind_group, &[]);
                        pass.set_vertex_buffer(0, draw.mesh.vertices.slice(..));
                        pass.set_index_buffer(draw.mesh.indices.slice(..), wgpu::IndexFormat::Uint16);
                        let i = mi as u32;
                        pass.draw_indexed(0..draw.mesh.index_count, 0, i..i + 1);
                    }
                }
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tonemap"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.output_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.tonemap);
            pass.set_bind_group(0, &targets.tonemap_group, &[]);
            pass.draw(0..3, 0..1);
        }
        ctx.queue.submit([encoder.finish()]);
    }

    /// Reads the output back as tightly packed RGBA8 (sRGB) rows.
    pub fn read_output(&self, ctx: &GpuContext, targets: &RenderTargets) -> Vec<u8> {
        let (w, h) = (targets.width, targets.height);
        let row = (w * 4).div_ceil(256) * 256;
        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            targets.output.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        ctx.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
        let mapped = slice.get_mapped_range().expect("mapped readback buffer");
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h as usize {
            out.extend_from_slice(&mapped[y * row as usize..y * row as usize + w as usize * 4]);
        }
        out
    }
}

/// Stands in until the renderer's own default material exists.
fn placeholder_material(ctx: &GpuContext) -> GpuMaterial {
    let layout = ctx.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[] });
    let bind_group =
        ctx.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &[] });
    GpuMaterial { name: String::new(), bind_group, textures: [None, None, None] }
}
