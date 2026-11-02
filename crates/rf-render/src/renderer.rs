//! The renderer: forward shading into an HDR target, then a filmic tonemap.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::GpuContext;
use crate::anim::Pose;
use crate::camera::{CameraView, OrbitCamera};
use crate::mesh::{Vertex, VertexExtra, VertexSkin};
use crate::model::{
    AlphaMode, DecalMode, GpuDraw, GpuModel, MaterialDesc, MaterialMap, NormalMap, Slot, TerrainDesc, TerrainParams,
    terrain_slot,
};
use crate::texture::GpuTexture;

pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// What the final image is: 8-bit sRGB, which egui can show directly.
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Where the scene pass writes each pixel's instance id and depth bits, for
/// picking (depth textures can't be read a pixel at a time).
const ID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Uint;
/// The colour water is drawn with, in linear RGB.
const WATER_COLOUR: [f32; 4] = [0.03, 0.07, 0.07, 1.0];

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
    decal_masks: [f32; 4],
    modes: [u32; 4],
    config: [u32; 4],
    palette: [f32; 4],
}

/// One drawn instance of a model: its transform and the entity's values the
/// shaders use.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Instance {
    pub transform: [[f32; 4]; 4],
    /// The row of the tint palette.
    pub tint: f32,
    /// What [`Renderer::pick`] finds where this instance is drawn; 0 for
    /// nothing.
    pub id: u32,
    pub _pad: [u32; 2],
}

impl Instance {
    pub fn new(transform: Mat4, tint: u32, id: u32) -> Self {
        Self { transform: transform.to_cols_array_2d(), tint: tint as f32, id, _pad: [0; 2] }
    }
}

/// A pick on its way back from the GPU.
pub struct PickRequest {
    buffer: wgpu::Buffer,
    ready: Arc<std::sync::atomic::AtomicBool>,
}

impl PickRequest {
    /// The pick, once the GPU has it (polls without waiting).
    pub fn try_get(&self, ctx: &GpuContext) -> Option<Pick> {
        let _ = ctx.device.poll(wgpu::PollType::Poll);
        if !self.ready.load(std::sync::atomic::Ordering::Acquire) {
            return None;
        }
        let mapped = self.buffer.slice(..).get_mapped_range().ok()?;
        let id = u32::from_le_bytes(mapped[0..4].try_into().unwrap());
        let depth = f32::from_le_bytes(mapped[4..8].try_into().unwrap());
        Some(Pick { id, depth })
    }
}

/// What's drawn at a pixel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pick {
    /// The [`Instance::id`] drawn there, 0 for none.
    pub id: u32,
    /// Its reversed depth (0 for nothing, at infinity).
    pub depth: f32,
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

/// Which pipeline draws a material.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MaterialKind {
    #[default]
    Standard,
    /// The terrain shader: four layers blended by masks and heights.
    Terrain,
}

/// A GPU material: its constants and textures.
pub struct GpuMaterial {
    pub name: String,
    pub kind: MaterialKind,
    pub bind_group: wgpu::BindGroup,
    /// For showing what a material uses: each texture with its use.
    pub textures: Vec<(&'static str, Arc<GpuTexture>)>,
}

/// The textures a view renders into, resized with it.
pub struct RenderTargets {
    pub width: u32,
    pub height: u32,
    hdr: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    ids: wgpu::Texture,
    ids_view: wgpu::TextureView,
    pub output: wgpu::Texture,
    pub output_view: wgpu::TextureView,
    /// The output seen as plain UNORM: the sRGB-encoded bytes as they are, which
    /// is what egui expects of a texture.
    pub display_view: wgpu::TextureView,
    tonemap_group: wgpu::BindGroup,
}

/// One model's LOD, drawn with a range of the frame's instance transforms.
pub struct DrawBatch<'a> {
    pub model: &'a GpuModel,
    pub lod: usize,
    pub instances: std::ops::Range<u32>,
}

/// What a frame drew.
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderStats {
    pub instances: usize,
    pub draw_calls: usize,
}

/// One model drawn by [`Renderer::render_posed`], at one of its LODs.
pub struct PosedModel<'a> {
    pub model: &'a GpuModel,
    pub lod: usize,
}

/// The bones a skinned draw can use (the C#'s `BoneMatrices[256]`), and the
/// bytes a rig's matrices take in the bone buffer (3 rows of 4 floats each).
const RIG_BONES: u64 = 256;
const RIG_BYTES: u64 = RIG_BONES * 48;

/// The bone buffer of posed draws, kept between frames and grown as needed,
/// with its bind group (one rig's matrices, picked by a dynamic offset).
struct BoneBuffer {
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
}

pub struct Renderer {
    instance_buffer: std::sync::Mutex<Option<wgpu::Buffer>>,
    bone_buffer: std::sync::Mutex<Option<BoneBuffer>>,
    bones_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    terrain_layout: wgpu::BindGroupLayout,
    tonemap_layout: wgpu::BindGroupLayout,
    frame_buffer: wgpu::Buffer,
    tonemap_buffer: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    point_sampler: wgpu::Sampler,
    pipelines: Pipelines,
    tonemap: wgpu::RenderPipeline,
    white: Arc<GpuTexture>,
    flat_normal: Arc<GpuTexture>,
    black: Arc<GpuTexture>,
    default_material: Arc<GpuMaterial>,
}

struct Pipelines {
    terrain: wgpu::RenderPipeline,
    opaque: wgpu::RenderPipeline,
    blend: wgpu::RenderPipeline,
    decal: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    /// Skinned meshes drawn posed: with the bone weights stream and matrices.
    skinned_opaque: wgpu::RenderPipeline,
    skinned_blend: wgpu::RenderPipeline,
    skinned_decal: wgpu::RenderPipeline,
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

/// The [`Instance`]s, one per draw, as a vertex buffer.
fn instance_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
        7 => Float32x4, 8 => Float32x4, 9 => Float32x4, 10 => Float32x4, 11 => Float32, 14 => Uint32
    ];
    wgpu::VertexBufferLayout {
        array_stride: size_of::<Instance>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &ATTRS,
    }
}

/// The phase a draw goes in: cut-out draws go with the opaque ones.
fn draw_phase(draw: &GpuDraw) -> AlphaMode {
    if draw.alpha == AlphaMode::Cutout { AlphaMode::Opaque } else { draw.alpha }
}

fn sampler_entry(binding: u32, ty: wgpu::SamplerBindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(ty),
        count: None,
    }
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
                sampler_entry(4, wgpu::SamplerBindingType::Filtering),
                texture_entry(5, true),
                texture_entry(6, true),
                texture_entry(7, true),
                texture_entry(8, true),
                sampler_entry(9, wgpu::SamplerBindingType::Filtering),
            ],
        });
        let mut terrain_entries = vec![uniform_entry(0, wgpu::ShaderStages::FRAGMENT)];
        terrain_entries.extend((0..terrain_slot::COUNT as u32).map(|i| texture_entry(1 + i, true)));
        terrain_entries.push(sampler_entry(1 + terrain_slot::COUNT as u32, wgpu::SamplerBindingType::Filtering));
        let terrain_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain"),
            entries: &terrain_entries,
        });
        let tonemap_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tonemap"),
            entries: &[texture_entry(0, false), uniform_entry(1, wgpu::ShaderStages::FRAGMENT)],
        });
        let bones_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bones"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(RIG_BYTES),
                },
                count: None,
            }],
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
        let point_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("point"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("model.wgsl"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(include_str!("shaders/common.wgsl"), include_str!("shaders/model.wgsl")).into(),
            ),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("model"),
            bind_group_layouts: &[Some(&frame_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let skinned_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("skinned model"),
            bind_group_layouts: &[Some(&frame_layout), Some(&material_layout), Some(&bones_layout)],
            immediate_size: 0,
        });
        let terrain_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain.wgsl"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(include_str!("shaders/common.wgsl"), include_str!("shaders/terrain.wgsl")).into(),
            ),
        });
        let terrain_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain"),
            bind_group_layouts: &[Some(&frame_layout), Some(&terrain_layout)],
            immediate_size: 0,
        });
        let depth = |write: bool, bias: i32| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            // Reversed Z.
            depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
            stencil: Default::default(),
            bias: wgpu::DepthBiasState { constant: bias, slope_scale: if bias != 0 { 1.0 } else { 0.0 }, clamp: 0.0 },
        };
        // Blended draws don't count for picking: they leave the ids alone.
        let scene_targets = |blend: Option<wgpu::BlendState>| {
            let ids = if blend.is_some() { wgpu::ColorWrites::empty() } else { wgpu::ColorWrites::ALL };
            [
                Some(wgpu::ColorTargetState { format: HDR_FORMAT, blend, write_mask: wgpu::ColorWrites::ALL }),
                Some(wgpu::ColorTargetState { format: ID_FORMAT, blend: None, write_mask: ids }),
            ]
        };
        // Skinned pipelines add the bone weights stream and the bone matrices.
        let pipeline =
            |label: &str, topology, blend: Option<wgpu::BlendState>, depth_write: bool, bias: i32, skinned: bool| {
                let buffers = [Some(Vertex::layout()), Some(instance_layout()), skinned.then(VertexSkin::layout)];
                let buffers = if skinned { &buffers[..] } else { &buffers[..2] };
                let targets = scene_targets(blend);
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(if skinned { &skinned_layout } else { &layout }),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some(if skinned { "vs_skinned" } else { "vs_main" }),
                        compilation_options: Default::default(),
                        buffers,
                    },
                    primitive: wgpu::PrimitiveState { topology, cull_mode: None, ..Default::default() },
                    depth_stencil: Some(depth(depth_write, bias)),
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
        let terrain = {
            let buffers = [Some(Vertex::layout()), Some(instance_layout()), Some(VertexExtra::layout())];
            let targets = scene_targets(None);
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("terrain"),
                layout: Some(&terrain_pl),
                vertex: wgpu::VertexState {
                    module: &terrain_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &buffers,
                },
                primitive: wgpu::PrimitiveState { topology: tri, cull_mode: None, ..Default::default() },
                depth_stencil: Some(depth(true, 0)),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &terrain_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipelines = Pipelines {
            terrain,
            opaque: pipeline("opaque", tri, None, true, 0, false),
            blend: pipeline("blend", tri, alpha, false, 0, false),
            decal: pipeline("decal", tri, alpha, false, 2, false),
            lines: pipeline("lines", wgpu::PrimitiveTopology::LineList, None, true, 0, false),
            skinned_opaque: pipeline("skinned opaque", tri, None, true, 0, true),
            skinned_blend: pipeline("skinned blend", tri, alpha, false, 0, true),
            skinned_decal: pipeline("skinned decal", tri, alpha, false, 2, true),
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
        let black = Arc::new(GpuTexture::solid(ctx, "black", [0, 0, 0, 0]));
        let mut r = Self {
            instance_buffer: Default::default(),
            bone_buffer: Default::default(),
            bones_layout,
            material_layout,
            terrain_layout,
            tonemap_layout,
            frame_buffer,
            tonemap_buffer,
            frame_group,
            sampler,
            point_sampler,
            pipelines,
            tonemap,
            white,
            flat_normal,
            black,
            default_material: Arc::new(placeholder_material(ctx)),
        };
        let grey = Arc::new(GpuTexture::solid(ctx, "grey", [160, 160, 160, 255]));
        let mut textures: [Option<Arc<GpuTexture>>; 7] = Default::default();
        textures[Slot::Diffuse as usize] = Some(grey);
        r.default_material =
            Arc::new(r.create_material(ctx, &MaterialDesc { bumpiness: 1.0, ..Default::default() }, textures));
        r
    }

    pub fn default_material(&self) -> Arc<GpuMaterial> {
        self.default_material.clone()
    }

    pub fn create_material(
        &self,
        ctx: &GpuContext,
        desc: &MaterialDesc,
        textures: [Option<Arc<GpuTexture>>; 7],
    ) -> GpuMaterial {
        let cutoff = match desc.alpha {
            AlphaMode::Cutout => 0.5,
            AlphaMode::Opaque => 0.0,
            AlphaMode::Blend | AlphaMode::Decal => 0.01,
        };
        let has = |slot: Slot| textures[slot as usize].is_some();
        let u = MaterialUniform {
            tint: if desc.water { WATER_COLOUR } else { [1.0; 4] },
            params: [
                desc.bumpiness,
                cutoff,
                if has(Slot::Normal) { 1.0 } else { 0.0 },
                if has(Slot::Material) { 1.0 } else { 0.0 },
            ],
            decal_masks: desc.decal_masks,
            modes: [
                match desc.decal {
                    DecalMode::None => 0,
                    DecalMode::Decal => 1,
                    DecalMode::Dirt => 2,
                },
                if desc.palette.is_some() { 2 } else { has(Slot::TintPalette) as u32 },
                desc.two_layer.is_some() as u32,
                desc.two_layer.unwrap_or(0) as u32,
            ],
            config: [
                match desc.normal_map {
                    NormalMap::TangentXY => 0,
                    NormalMap::Direct => 1,
                },
                match desc.material_map {
                    MaterialMap::MetalRoughness => 0,
                    MaterialMap::Smoothness => 1,
                    MaterialMap::Roughness => 2,
                },
                desc.water as u32,
                0,
            ],
            palette: desc.palette.unwrap_or_default(),
        };
        let buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&desc.shader_name),
            contents: bytemuck::bytes_of(&u),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let get = |slot: Slot| textures[slot as usize].as_deref();
        let a = get(Slot::Diffuse).unwrap_or(&self.white);
        let n = get(Slot::Normal).unwrap_or(&self.flat_normal);
        let p = get(Slot::Material).unwrap_or(&self.white);
        // A missing second layer shows the first.
        let a2 = get(Slot::Diffuse2).unwrap_or(a);
        let n2 = get(Slot::Normal2).unwrap_or(n);
        let p2 = get(Slot::Material2).unwrap_or(p);
        let tint = get(Slot::TintPalette).unwrap_or(&self.white);
        fn view(t: &GpuTexture) -> wgpu::BindingResource<'_> {
            wgpu::BindingResource::TextureView(&t.view_2d)
        }
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&desc.shader_name),
            layout: &self.material_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: view(a) },
                wgpu::BindGroupEntry { binding: 2, resource: view(n) },
                wgpu::BindGroupEntry { binding: 3, resource: view(p) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 5, resource: view(a2) },
                wgpu::BindGroupEntry { binding: 6, resource: view(n2) },
                wgpu::BindGroupEntry { binding: 7, resource: view(p2) },
                wgpu::BindGroupEntry { binding: 8, resource: view(tint) },
                wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::Sampler(&self.point_sampler) },
            ],
        });
        let textures = Slot::ALL.iter().zip(textures).filter_map(|(s, t)| Some((s.label(), t?))).collect();
        GpuMaterial { name: desc.shader_name.clone(), kind: MaterialKind::Standard, bind_group, textures }
    }

    /// A material for the terrain shader; `textures` by [`terrain_slot`].
    pub fn create_terrain_material(
        &self,
        ctx: &GpuContext,
        desc: &TerrainDesc,
        textures: [Option<Arc<GpuTexture>>; terrain_slot::COUNT],
    ) -> GpuMaterial {
        let mut params: TerrainParams = desc.params;
        if textures[terrain_slot::TINT].is_none() {
            params.tint_blends = [0.0; 4];
        }
        let buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&desc.shader_name),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let fallback = |slot: usize| -> &GpuTexture {
            match slot {
                4..8 | terrain_slot::TINT_NORMAL => &self.flat_normal,
                terrain_slot::LOOKUP => &self.black,
                _ => &self.white,
            }
        };
        let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }];
        for (i, t) in textures.iter().enumerate() {
            let t = t.as_deref().unwrap_or_else(|| fallback(i));
            entries.push(wgpu::BindGroupEntry {
                binding: 1 + i as u32,
                resource: wgpu::BindingResource::TextureView(&t.view_2d),
            });
        }
        entries.push(wgpu::BindGroupEntry {
            binding: 1 + terrain_slot::COUNT as u32,
            resource: wgpu::BindingResource::Sampler(&self.sampler),
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&desc.shader_name),
            layout: &self.terrain_layout,
            entries: &entries,
        });
        const LABELS: [&str; terrain_slot::COUNT] = [
            "Diffuse 1",
            "Diffuse 2",
            "Diffuse 3",
            "Diffuse 4",
            "Normal 1",
            "Normal 2",
            "Normal 3",
            "Normal 4",
            "Material 1",
            "Material 2",
            "Material 3",
            "Material 4",
            "Lookup",
            "Tint",
            "Tint normal",
        ];
        let textures = LABELS.iter().zip(textures).filter_map(|(l, t)| Some((*l, t?))).collect();
        GpuMaterial { name: desc.shader_name.clone(), kind: MaterialKind::Terrain, bind_group, textures }
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
        let copy = wgpu::TextureUsages::COPY_SRC;
        let depth = texture("depth", DEPTH_FORMAT, rt);
        let depth_view = depth.create_view(&Default::default());
        let ids = texture("ids", ID_FORMAT, rt | copy);
        let ids_view = ids.create_view(&Default::default());
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
        RenderTargets {
            width,
            height,
            hdr,
            depth_view,
            ids,
            ids_view,
            output,
            output_view,
            display_view,
            tonemap_group,
        }
    }

    /// Draws one model with an orbiting camera (the model viewer).
    pub fn render_model(
        &self,
        ctx: &GpuContext,
        targets: &RenderTargets,
        camera: &OrbitCamera,
        settings: &ViewSettings,
        model: &GpuModel,
    ) {
        let view = camera.camera_view(targets.width as f32 / targets.height as f32);
        let lod = settings.lod.min(3);
        let lod = if model.lods[lod].is_empty() { model.highest_lod() } else { lod };
        let instance = Instance::new(Mat4::IDENTITY, 0, 0);
        self.render(ctx, targets, &view, settings, &[instance], &[DrawBatch { model, lod, instances: 0..1 }]);
    }

    /// Draws batches of model instances into `targets.output`: `instances`
    /// holds every instance, and each batch draws one model's LOD with a
    /// range of them.
    pub fn render(
        &self,
        ctx: &GpuContext,
        targets: &RenderTargets,
        camera: &CameraView,
        settings: &ViewSettings,
        instances: &[Instance],
        batches: &[DrawBatch],
    ) -> RenderStats {
        let mut stats = RenderStats { instances: instances.len(), ..Default::default() };
        self.frame(ctx, targets, camera, settings, instances, |pass| {
            // Opaque and cut-out first, then decals, then blended draws.
            for phase in [AlphaMode::Opaque, AlphaMode::Decal, AlphaMode::Blend] {
                for batch in batches {
                    if batch.instances.is_empty() {
                        continue;
                    }
                    for draw in &batch.model.lods[batch.lod.min(3)] {
                        if draw_phase(draw) != phase {
                            continue;
                        }
                        let Some(pipeline) = self.pipeline(pass, draw, phase) else { continue };
                        pass.set_pipeline(pipeline);
                        pass.set_bind_group(1, &draw.material.bind_group, &[]);
                        pass.set_vertex_buffer(0, draw.mesh.vertices.slice(..));
                        pass.set_index_buffer(draw.mesh.indices.slice(..), wgpu::IndexFormat::Uint16);
                        pass.draw_indexed(0..draw.mesh.index_count, 0, batch.instances.clone());
                        stats.draw_calls += 1;
                    }
                }
            }
        });
        stats
    }

    /// Draws models posed by a skeleton, all with one transform (a ped and its
    /// parts): skinned meshes blend their rig's bone matrices, unskinned ones
    /// that follow a bone move with it. The models must have been uploaded with
    /// [`ModelOptions::skin`](crate::model::ModelOptions::skin).
    #[allow(clippy::too_many_arguments)]
    pub fn render_posed(
        &self,
        ctx: &GpuContext,
        targets: &RenderTargets,
        camera: &CameraView,
        settings: &ViewSettings,
        transform: Mat4,
        pose: &Pose,
        models: &[PosedModel],
    ) -> RenderStats {
        // Instance 0 is the transform; unskinned draws that follow a bone get
        // one of their own. Each rig's matrices take a slot of the bone buffer.
        let mut instances = vec![Instance::new(transform, 0, 0)];
        let mut draw_instances: Vec<Vec<u32>> = Vec::with_capacity(models.len());
        let mut rig_slots: Vec<u64> = Vec::with_capacity(models.len());
        let mut rig_count = 0u64;
        for m in models {
            rig_slots.push(rig_count);
            rig_count += m.model.rigs.len() as u64;
            let draws = &m.model.lods[m.lod.min(3)];
            draw_instances.push(
                draws
                    .iter()
                    .map(|d| match d.bone.filter(|_| d.rig.is_none()).and_then(|t| pose.bone_transform(t)) {
                        Some(bone) => {
                            instances.push(Instance::new(transform * bone, 0, 0));
                            instances.len() as u32 - 1
                        }
                        None => 0,
                    })
                    .collect(),
            );
        }
        let align = ctx.device.limits().min_storage_buffer_offset_alignment as u64;
        let slot = RIG_BYTES.div_ceil(align) * align;
        let mut bones = vec![0u8; (slot * rig_count) as usize];
        for (m, &first) in models.iter().zip(&rig_slots) {
            for (r, tags) in m.model.rigs.iter().enumerate() {
                let at = ((first + r as u64) * slot) as usize;
                let rows: Vec<[[f32; 4]; 3]> = pose.skin_rows(&tags[..tags.len().min(RIG_BONES as usize)]).collect();
                let bytes: &[u8] = bytemuck::cast_slice(&rows);
                bones[at..at + bytes.len()].copy_from_slice(bytes);
            }
        }
        let bone_group = (rig_count > 0).then(|| self.upload_bones(ctx, &bones));

        let mut stats = RenderStats { instances: instances.len(), ..Default::default() };
        self.frame(ctx, targets, camera, settings, &instances, |pass| {
            for phase in [AlphaMode::Opaque, AlphaMode::Decal, AlphaMode::Blend] {
                for ((m, insts), &first) in models.iter().zip(&draw_instances).zip(&rig_slots) {
                    for (draw, &inst) in m.model.lods[m.lod.min(3)].iter().zip(insts) {
                        if draw_phase(draw) != phase {
                            continue;
                        }
                        let skinned = draw.rig.zip(draw.mesh.skin.as_ref()).zip(bone_group.as_ref());
                        let pipeline = match skinned {
                            Some(((rig, skin), group)) if !draw.mesh.lines => {
                                pass.set_vertex_buffer(2, skin.slice(..));
                                let offset = (first + rig as u64) * slot;
                                pass.set_bind_group(2, group, &[offset as u32]);
                                match phase {
                                    AlphaMode::Blend => &self.pipelines.skinned_blend,
                                    AlphaMode::Decal => &self.pipelines.skinned_decal,
                                    _ => &self.pipelines.skinned_opaque,
                                }
                            }
                            _ => {
                                let Some(p) = self.pipeline(pass, draw, phase) else { continue };
                                p
                            }
                        };
                        pass.set_pipeline(pipeline);
                        pass.set_bind_group(1, &draw.material.bind_group, &[]);
                        pass.set_vertex_buffer(0, draw.mesh.vertices.slice(..));
                        pass.set_index_buffer(draw.mesh.indices.slice(..), wgpu::IndexFormat::Uint16);
                        pass.draw_indexed(0..draw.mesh.index_count, 0, inst..inst + 1);
                        stats.draw_calls += 1;
                    }
                }
            }
        });
        stats
    }

    /// [`Renderer::render_posed`] with an orbiting camera (the animation viewer).
    pub fn render_posed_orbit(
        &self,
        ctx: &GpuContext,
        targets: &RenderTargets,
        camera: &OrbitCamera,
        settings: &ViewSettings,
        pose: &Pose,
        models: &[PosedModel],
    ) -> RenderStats {
        let view = camera.camera_view(targets.width as f32 / targets.height as f32);
        self.render_posed(ctx, targets, &view, settings, Mat4::IDENTITY, pose, models)
    }

    /// The pipeline of an unskinned draw in a phase (binding the terrain's
    /// extra stream); `None` for a terrain draw without it.
    fn pipeline<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'_>,
        draw: &'a GpuDraw,
        phase: AlphaMode,
    ) -> Option<&'a wgpu::RenderPipeline> {
        Some(match (draw.material.kind, draw.mesh.lines, phase) {
            (MaterialKind::Terrain, ..) => {
                pass.set_vertex_buffer(2, draw.mesh.extra.as_ref()?.slice(..));
                &self.pipelines.terrain
            }
            (_, true, _) => &self.pipelines.lines,
            (_, _, AlphaMode::Blend) => &self.pipelines.blend,
            (_, _, AlphaMode::Decal) => &self.pipelines.decal,
            _ => &self.pipelines.opaque,
        })
    }

    /// Writes the bone matrices into the bone buffer (grown as needed) and
    /// gives its bind group.
    fn upload_bones(&self, ctx: &GpuContext, bytes: &[u8]) -> wgpu::BindGroup {
        let mut slot = self.bone_buffer.lock().unwrap();
        if slot.as_ref().is_none_or(|b| b.buffer.size() < bytes.len() as u64) {
            let size = (bytes.len() as u64).next_power_of_two().max(64 * 1024);
            let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bones"),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bones"),
                layout: &self.bones_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(RIG_BYTES),
                    }),
                }],
            });
            *slot = Some(BoneBuffer { buffer, group });
        }
        let b = slot.as_ref().unwrap();
        ctx.queue.write_buffer(&b.buffer, 0, bytes);
        b.group.clone()
    }

    /// One frame: the frame's constants and instances, the scene pass (`draw`
    /// fills it, with the frame bound at 0 and the instances at vertex buffer
    /// 1), then the tonemap pass into `targets.output`.
    fn frame(
        &self,
        ctx: &GpuContext,
        targets: &RenderTargets,
        camera: &CameraView,
        settings: &ViewSettings,
        instances: &[Instance],
        draw: impl FnOnce(&mut wgpu::RenderPass<'_>),
    ) {
        let view_proj = camera.projection * camera.view;
        let debug = settings.debug as u32;
        let frame = FrameUniform {
            view_proj: view_proj.to_cols_array_2d(),
            camera_pos: camera.eye.extend(1.0).to_array(),
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

        // The instances go in one buffer, kept between frames and grown as needed.
        let instance_buffer = (!instances.is_empty()).then(|| {
            let mut slot = self.instance_buffer.lock().unwrap();
            let bytes: &[u8] = bytemuck::cast_slice(instances);
            if slot.as_ref().is_none_or(|b| b.size() < bytes.len() as u64) {
                let size = (bytes.len() as u64).next_power_of_two().max(64 * 1024);
                *slot = Some(ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("instances"),
                    size,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
            let buffer = slot.clone().unwrap();
            ctx.queue.write_buffer(&buffer, 0, bytes);
            buffer
        });

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let [r, g, b] = settings.background;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &targets.hdr,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &targets.ids_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.frame_group, &[]);
            if let Some(ib) = &instance_buffer {
                pass.set_vertex_buffer(1, ib.slice(..));
            }
            draw(&mut pass);
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

    /// What the last frame drew at a pixel of `targets` (waits for the GPU).
    pub fn pick(&self, ctx: &GpuContext, targets: &RenderTargets, x: u32, y: u32) -> Option<Pick> {
        let request = self.request_pick(ctx, targets, x, y)?;
        let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
        request.try_get(ctx)
    }

    /// Starts reading what the last frame drew at a pixel; [`PickRequest::try_get`]
    /// has it once the GPU is done, without waiting.
    pub fn request_pick(&self, ctx: &GpuContext, targets: &RenderTargets, x: u32, y: u32) -> Option<PickRequest> {
        if x >= targets.width || y >= targets.height {
            return None;
        }
        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pick"),
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &targets.ids,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: Some(1) },
            },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
        ctx.queue.submit([encoder.finish()]);
        let ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = ready.clone();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            if r.is_ok() {
                flag.store(true, std::sync::atomic::Ordering::Release);
            }
        });
        Some(PickRequest { buffer, ready })
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
    GpuMaterial { name: String::new(), kind: MaterialKind::Standard, bind_group, textures: Vec::new() }
}
