//! The renderer, deferred as the C# RedForge's: each frame draws the sun's
//! shadow maps, the scene into a G-buffer, its ambient occlusion, lights it
//! (the sun with its shadows, the sky, the fill light, emission), puts it over
//! the background, draws what's blended lit as it goes, and tonemaps the result.

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
    WaterDesc, terrain_slot, water_slot,
};
use crate::post::{Post, PostSettings, Tonemap};
use crate::shadows::{CASCADES, Cascades, SHADOW_SIZE};
use crate::sky::{Sky, SkyLights};
use crate::targets::{DEPTH_FORMAT, GBUFFER_FORMATS, HDR_FORMAT, ID_FORMAT, SSAO_FORMAT, ViewLayouts};
use crate::texture::GpuTexture;

pub use crate::targets::RenderTargets;

/// How much of the next cascade shows near a cascade's edge, in UV (the C#'s
/// `Render.CSM.BlurBetweenCascades`).
const CASCADE_BLEND: f32 = 0.05;
/// The C#'s SSAO settings: radius (m), strength, distance bias, samples.
const SSAO_RADIUS: f32 = 0.4;
const SSAO_STRENGTH: f32 = 1.0;
const SSAO_DISTANCE_BIAS: f32 = 1.0;
const SSAO_SAMPLES: usize = 32;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ShadowsUniform {
    cascades: [[[f32; 4]; 4]; CASCADES],
    texels: [f32; 4],
    depths: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    proj: [[f32; 4]; 4],
    inv_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    sun_dir: [f32; 4],
    sun_colour: [f32; 4],
    fill: [f32; 4],
    background: [f32; 4],
    options: [u32; 4],
    viewport: [f32; 4],
    time: [f32; 4],
    shadows: ShadowsUniform,
}

/// The frame uniform's size, which the shadow passes' copies are apart (a
/// multiple of the dynamic offset alignment).
const FRAME_STRIDE: u64 = size_of::<FrameUniform>().next_multiple_of(256) as u64;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SsaoUniform {
    samples: [[f32; 4]; SSAO_SAMPLES],
    noise: [[f32; 4]; 9],
    params: [f32; 4],
}

impl SsaoUniform {
    /// The C#'s sample kernel: points towards the normal, closer in for the
    /// first ones; and the directions that turn it from pixel to pixel.
    fn new() -> Self {
        let mut seed = 0x1234567u32;
        let mut rnd = |lo: f32, hi: f32| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            lo + (hi - lo) * (seed >> 8) as f32 / (1 << 24) as f32
        };
        let mut samples = [[0.0; 4]; SSAO_SAMPLES];
        for (i, s) in samples.iter_mut().enumerate() {
            let v = Vec3::new(rnd(-1.0, 1.0), rnd(-1.0, 1.0), rnd(0.8, 1.0)).normalize();
            let scale = 0.1 + 0.9 * i as f32 / SSAO_SAMPLES as f32;
            *s = (v * scale).extend(0.0).to_array();
        }
        let mut noise = [[0.0; 4]; 9];
        for n in &mut noise {
            *n = Vec3::new(rnd(-1.0, 1.0), rnd(-1.0, 1.0), 0.0).normalize_or(Vec3::X).extend(0.0).to_array();
        }
        Self { samples, noise, params: [SSAO_RADIUS, SSAO_STRENGTH, SSAO_DISTANCE_BIAS, SSAO_SAMPLES as f32] }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MaterialUniform {
    tint: [f32; 4],
    params: [f32; 4],
    decal_masks: [f32; 4],
    modes: [u32; 4],
    config: [u32; 4],
    extra: [f32; 4],
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
    /// A world-space normal its whole model is lit with instead of its own
    /// (grass, lit as the ground), as signed bytes with 127 in w; 0 for none.
    pub normal: u32,
    pub _pad: u32,
}

impl Instance {
    pub fn new(transform: Mat4, tint: u32, id: u32) -> Self {
        Self { transform: transform.to_cols_array_2d(), tint: tint as f32, id, normal: 0, _pad: 0 }
    }

    /// Lit as if facing `normal` everywhere.
    pub fn with_normal(self, normal: Vec3) -> Self {
        let n = normal.normalize_or(Vec3::Z);
        let b = |v: f32| ((v.clamp(-1.0, 1.0) * 127.0).round() as i8) as u8 as u32;
        Self { normal: b(n.x) | b(n.y) << 8 | b(n.z) << 16 | 127 << 24, ..self }
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
    /// The ambient occlusion (dark where occluded).
    Occlusion,
}

#[derive(Clone, Debug)]
pub struct ViewSettings {
    pub debug: DebugView,
    pub lod: usize,
    /// Towards the sun (or the moon), and its light's colour.
    pub sun_dir: Vec3,
    pub sun_colour: Vec3,
    /// The ambient light's fill colour, added to the sky's.
    pub fill: Vec3,
    /// The time of day (0-24), for the glows that only show at some hours;
    /// `None` shows them all (the viewers).
    pub hour: Option<f32>,
    /// The sky's lights: the atmosphere drawn where nothing is and over
    /// everything far, and lighting the scene. Without, the background colour
    /// shows and a day's sky lights it (the viewers).
    pub sky: Option<SkyLights>,
    pub background: [f64; 3],
    /// The user's exposure plus the scene's, in EV (the image is exposed to
    /// its average first).
    pub exposure_ev: f32,
    /// Night vision: the tint colours fade to in the dark, and how much.
    pub scotopic: [f32; 4],
    pub tonemap: Tonemap,
    pub bloom: bool,
    pub shadows: bool,
    pub ssao: bool,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            debug: DebugView::Lit,
            lod: 0,
            sun_dir: Vec3::new(0.4, -0.6, 0.7).normalize(),
            sun_colour: Vec3::new(3.2, 3.0, 2.7),
            fill: Vec3::ZERO,
            hour: None,
            sky: None,
            background: [0.18, 0.2, 0.24],
            exposure_ev: 0.0,
            scotopic: [1.0, 1.0, 1.0, 0.0],
            tonemap: Tonemap::Rdr2,
            bloom: true,
            shadows: true,
            ssao: true,
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
    /// Rivers and lakes, drawn over the lit scene before the sky.
    Water,
}

/// A GPU material: its constants and textures.
pub struct GpuMaterial {
    pub name: String,
    pub kind: MaterialKind,
    /// Added over what's behind (fire), not blended.
    pub additive: bool,
    pub bind_group: wgpu::BindGroup,
    /// For showing what a material uses: each texture with its use.
    pub textures: Vec<(&'static str, Arc<GpuTexture>)>,
}

/// How a [`Light`] shines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LightKind {
    #[default]
    Point,
    /// In a cone around its direction.
    Spot,
    /// From a segment along its direction.
    Capsule,
}

/// A light of the map, in the world.
#[derive(Clone, Copy, Debug, Default)]
pub struct Light {
    pub kind: LightKind,
    pub position: Vec3,
    pub direction: Vec3,
    pub colour: Vec3,
    /// How far it reaches, and how sharply it fades there.
    pub range: f32,
    pub falloff_exponent: f32,
    /// A spot light's cone: half angles, in radians.
    pub cone_inner: f32,
    pub cone_outer: f32,
    /// Half a capsule's length.
    pub half_length: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LightUniform {
    position: [f32; 4],
    colour: [f32; 4],
    direction: [f32; 4],
    shape: [f32; 4],
}

impl From<&Light> for LightUniform {
    fn from(l: &Light) -> Self {
        let kind = match l.kind {
            LightKind::Point => 0.0,
            LightKind::Spot => 1.0,
            LightKind::Capsule => 2.0,
        };
        let half_length = if l.kind == LightKind::Capsule { l.half_length } else { 0.0 };
        Self {
            position: l.position.extend(l.range).to_array(),
            colour: l.colour.extend(l.falloff_exponent).to_array(),
            direction: l.direction.normalize_or(Vec3::NEG_Z).extend(kind).to_array(),
            shape: [l.cone_inner, l.cone_outer, half_length, 0.0],
        }
    }
}

/// The lights' storage buffer, kept between frames and grown as needed.
struct LightBuffer {
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
}

/// One model's LOD, drawn with a range of the frame's instance transforms, or
/// of a buffer of its own ([`Renderer::create_instance_buffer`]).
pub struct DrawBatch<'a> {
    pub model: &'a GpuModel,
    pub lod: usize,
    pub instances: std::ops::Range<u32>,
    pub buffer: Option<&'a wgpu::Buffer>,
}

/// What [`Renderer::render_scene`] draws: every instance, the batches the
/// camera sees, and what casts shadows into each of the sun's cascades
/// (batches of the same instances).
pub struct Scene<'a> {
    pub instances: &'a [Instance],
    pub batches: &'a [DrawBatch<'a>],
    /// The cascades the casters were picked for ([`Cascades::casts`]), and
    /// each one's casters.
    pub shadows: Option<(&'a Cascades, &'a [Vec<DrawBatch<'a>>; CASCADES])>,
    /// The lights in view.
    pub lights: &'a [Light],
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

/// A pass of the frame that draws models.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pass {
    /// Into a cascade of the sun's shadow map.
    Shadow(usize),
    /// Into the G-buffer: opaque and cut-out draws, then decals over them.
    Opaque,
    Decal,
    /// Over the lit G-buffer, before the sky.
    Water,
    /// Over the finished image, lit as they're drawn.
    Blend,
}

pub struct Renderer {
    instance_buffer: std::sync::Mutex<Option<wgpu::Buffer>>,
    bone_buffer: std::sync::Mutex<Option<BoneBuffer>>,
    light_buffer: std::sync::Mutex<Option<LightBuffer>>,
    lights_layout: wgpu::BindGroupLayout,
    bones_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    terrain_layout: wgpu::BindGroupLayout,
    water_layout: wgpu::BindGroupLayout,
    water_view_layout: wgpu::BindGroupLayout,
    /// The ripples' normal maps ("waterbump2"), once the app has them.
    water_bump: std::sync::Mutex<Arc<GpuTexture>>,
    clamp_sampler: wgpu::Sampler,
    /// When the renderer started, for the frame's time.
    start: std::time::Instant,
    view_layouts: ViewLayouts,
    frame_buffer: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    /// The frame's uniform once per cascade, with that cascade's projection.
    shadow_frames: wgpu::Buffer,
    shadow_frame_group: wgpu::BindGroup,
    /// One layer per cascade.
    shadow_layers: Vec<wgpu::TextureView>,
    /// The world's sky (the map's), and the day sky lighting the viewers:
    /// each keeps one environment map.
    world_sky: std::sync::Mutex<Sky>,
    studio_sky: std::sync::Mutex<Sky>,
    ssao_buffer: wgpu::Buffer,
    post: Post,
    sampler: wgpu::Sampler,
    point_sampler: wgpu::Sampler,
    pipelines: Pipelines,
    white: Arc<GpuTexture>,
    flat_normal: Arc<GpuTexture>,
    black: Arc<GpuTexture>,
    default_material: Arc<GpuMaterial>,
}

struct Pipelines {
    terrain: wgpu::RenderPipeline,
    opaque: wgpu::RenderPipeline,
    decal: wgpu::RenderPipeline,
    lines: wgpu::RenderPipeline,
    blend: wgpu::RenderPipeline,
    /// Fire: added over the image.
    additive: wgpu::RenderPipeline,
    shadow: wgpu::RenderPipeline,
    /// Shadows of cut-out and blended draws, which test their alpha.
    shadow_alpha: wgpu::RenderPipeline,
    /// Skinned meshes drawn posed: with the bone weights stream and matrices.
    skinned_opaque: wgpu::RenderPipeline,
    skinned_decal: wgpu::RenderPipeline,
    skinned_blend: wgpu::RenderPipeline,
    skinned_shadow: wgpu::RenderPipeline,
    skinned_shadow_alpha: wgpu::RenderPipeline,
    ssao: wgpu::RenderPipeline,
    ssao_blur: wgpu::RenderPipeline,
    lighting: wgpu::RenderPipeline,
    background: wgpu::RenderPipeline,
    lights: wgpu::RenderPipeline,
    water: wgpu::RenderPipeline,
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
    texture_entry_of(binding, wgpu::TextureSampleType::Float { filterable }, wgpu::TextureViewDimension::D2)
}

fn texture_entry_of(
    binding: u32,
    sample_type: wgpu::TextureSampleType,
    view_dimension: wgpu::TextureViewDimension,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture { sample_type, view_dimension, multisampled: false },
        count: None,
    }
}

fn sampler_entry(binding: u32, ty: wgpu::SamplerBindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(ty),
        count: None,
    }
}

/// The [`Instance`]s, one per draw, as a vertex buffer.
fn instance_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
        7 => Float32x4, 8 => Float32x4, 9 => Float32x4, 10 => Float32x4, 11 => Float32, 14 => Uint32, 15 => Uint32
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

/// Which pass a draw goes in, and for shadows whether it tests its alpha
/// (decals and lines cast none).
fn in_pass(draw: &GpuDraw, pass: Pass) -> Option<bool> {
    let phase = draw_phase(draw);
    let water = draw.material.kind == MaterialKind::Water;
    match pass {
        Pass::Water => water.then_some(false),
        _ if water => None,
        Pass::Shadow(_) => (!draw.mesh.lines && phase != AlphaMode::Decal)
            .then_some(draw.alpha != AlphaMode::Opaque && draw.material.kind != MaterialKind::Terrain),
        Pass::Opaque => (phase == AlphaMode::Opaque).then_some(false),
        Pass::Decal => (phase == AlphaMode::Decal).then_some(false),
        Pass::Blend => (phase == AlphaMode::Blend).then_some(false),
    }
}

impl Renderer {
    pub fn new(ctx: &GpuContext) -> Self {
        let device = &ctx.device;
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                texture_entry_of(1, wgpu::TextureSampleType::Depth, wgpu::TextureViewDimension::D2Array),
                sampler_entry(2, wgpu::SamplerBindingType::Comparison),
            ],
        });
        let shadow_frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow frame"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(size_of::<FrameUniform>() as u64),
                },
                count: None,
            }],
        });
        let world_sky = Sky::new(ctx);
        let studio_sky = Sky::new(ctx);
        let env_layout = world_sky.env_layout();
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
        let mut water_entries = vec![uniform_entry(0, wgpu::ShaderStages::FRAGMENT)];
        water_entries.extend((0..water_slot::COUNT as u32).map(|i| texture_entry(1 + i, true)));
        water_entries.push(sampler_entry(1 + water_slot::COUNT as u32, wgpu::SamplerBindingType::Filtering));
        let water_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water"),
            entries: &water_entries,
        });
        let water_view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water view"),
            entries: &[
                texture_entry(0, true),
                texture_entry_of(1, wgpu::TextureSampleType::Depth, wgpu::TextureViewDimension::D2),
                texture_entry(2, true),
                sampler_entry(3, wgpu::SamplerBindingType::Filtering),
            ],
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
        let view_layouts = ViewLayouts::new(device);
        let lights_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lights"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(size_of::<LightUniform>() as u64),
                },
                count: None,
            }],
        });

        let uniform = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let frame_buffer = uniform("frame", size_of::<FrameUniform>() as u64);
        let shadow_frames = uniform("shadow frames", FRAME_STRIDE * CASCADES as u64);
        let ssao_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ssao"),
            contents: bytemuck::bytes_of(&SsaoUniform::new()),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let shadow_map = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow map"),
            size: wgpu::Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: CASCADES as u32 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_array = shadow_map.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let shadow_layers = (0..CASCADES as u32)
            .map(|i| {
                shadow_map.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &frame_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: frame_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&shadow_array) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&shadow_sampler) },
            ],
        });
        let shadow_frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow frame"),
            layout: &shadow_frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &shadow_frames,
                    offset: 0,
                    size: wgpu::BufferSize::new(size_of::<FrameUniform>() as u64),
                }),
            }],
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
        let clamp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let point_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("point"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let module = |label, source: String| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        };
        let common = include_str!("shaders/common.wgsl");
        let lighting = include_str!("shaders/lighting.wgsl");
        let model_shader = module("model.wgsl", [common, lighting, include_str!("shaders/model.wgsl")].concat());
        let terrain_shader = module("terrain.wgsl", [common, include_str!("shaders/terrain.wgsl")].concat());
        let deferred_shader =
            module("deferred.wgsl", [common, lighting, include_str!("shaders/deferred.wgsl")].concat());
        let ssao_shader = module("ssao.wgsl", [common, include_str!("shaders/ssao.wgsl")].concat());
        let water_shader = module("water.wgsl", [common, lighting, include_str!("shaders/water.wgsl")].concat());
        let lights_shader = module(
            "lights.wgsl",
            [common, lighting, include_str!("shaders/deferred.wgsl"), include_str!("shaders/lights.wgsl")].concat(),
        );

        let pipeline_layout = |label, groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: groups,
                immediate_size: 0,
            })
        };
        let gbuffer_pl = pipeline_layout("g-buffer", &[Some(&frame_layout), Some(&material_layout)]);
        let skinned_gbuffer_pl =
            pipeline_layout("skinned g-buffer", &[Some(&frame_layout), Some(&material_layout), Some(&bones_layout)]);
        let forward_pl =
            pipeline_layout("forward", &[Some(&frame_layout), Some(&material_layout), None, Some(env_layout)]);
        let skinned_forward_pl = pipeline_layout(
            "skinned forward",
            &[Some(&frame_layout), Some(&material_layout), Some(&bones_layout), Some(env_layout)],
        );
        let terrain_pl = pipeline_layout("terrain", &[Some(&frame_layout), Some(&terrain_layout)]);
        let shadow_pl = pipeline_layout("shadow", &[Some(&shadow_frame_layout)]);
        let shadow_alpha_pl = pipeline_layout("shadow alpha", &[Some(&shadow_frame_layout), Some(&material_layout)]);
        let skinned_shadow_pl =
            pipeline_layout("skinned shadow", &[Some(&shadow_frame_layout), None, Some(&bones_layout)]);
        let skinned_shadow_alpha_pl = pipeline_layout(
            "skinned shadow alpha",
            &[Some(&shadow_frame_layout), Some(&material_layout), Some(&bones_layout)],
        );

        // Reversed Z for the views; the shadow maps' depth grows away from the light.
        let depth = |write: bool, bias: i32| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
            stencil: Default::default(),
            bias: wgpu::DepthBiasState { constant: bias, slope_scale: if bias != 0 { 1.0 } else { 0.0 }, clamp: 0.0 },
        };
        let shadow_depth = wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: wgpu::DepthBiasState { constant: 0, slope_scale: 2.0, clamp: 0.0 },
        };
        let target = |format, blend| Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL });
        // Decals and blended draws don't count for picking: they leave the ids alone.
        let ids = |write: bool| {
            let write_mask = if write { wgpu::ColorWrites::ALL } else { wgpu::ColorWrites::empty() };
            Some(wgpu::ColorTargetState { format: ID_FORMAT, blend: None, write_mask })
        };
        let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);
        // Decals leave the params' alpha alone: it's the surface's translucency.
        let gbuffer = |blend: Option<wgpu::BlendState>| {
            let mut t: Vec<_> = GBUFFER_FORMATS.iter().map(|f| target(*f, blend)).collect();
            if blend.is_some() {
                t[2].as_mut().unwrap().write_mask = wgpu::ColorWrites::COLOR;
            }
            t.push(ids(blend.is_none()));
            t
        };
        // Blended draws are premultiplied: their reflections don't fade with
        // their opacity.
        let forward = [target(HDR_FORMAT, Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING)), ids(false)];
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let keep = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let additive = [target(HDR_FORMAT, Some(wgpu::BlendState { color: add, alpha: keep })), ids(false)];

        struct Desc<'a> {
            label: &'a str,
            layout: &'a wgpu::PipelineLayout,
            module: &'a wgpu::ShaderModule,
            vs: &'a str,
            fs: Option<&'a str>,
            buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
            targets: &'a [Option<wgpu::ColorTargetState>],
            depth: Option<wgpu::DepthStencilState>,
            topology: wgpu::PrimitiveTopology,
        }
        let make = |d: Desc| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(d.label),
                layout: Some(d.layout),
                vertex: wgpu::VertexState {
                    module: d.module,
                    entry_point: Some(d.vs),
                    compilation_options: Default::default(),
                    buffers: d.buffers,
                },
                primitive: wgpu::PrimitiveState { topology: d.topology, cull_mode: None, ..Default::default() },
                depth_stencil: d.depth,
                multisample: Default::default(),
                fragment: d.fs.map(|fs| wgpu::FragmentState {
                    module: d.module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: d.targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let tri = wgpu::PrimitiveTopology::TriangleList;
        let plain = [Some(Vertex::layout()), Some(instance_layout())];
        let skinned = [Some(Vertex::layout()), Some(instance_layout()), Some(VertexSkin::layout())];
        let terrain_buffers = [Some(Vertex::layout()), Some(instance_layout()), Some(VertexExtra::layout())];
        let (opaque_targets, decal_targets) = (gbuffer(None), gbuffer(alpha));
        let model = |label, layout, skin: bool, fs, targets: &[Option<wgpu::ColorTargetState>], depth| {
            make(Desc {
                label,
                layout,
                module: &model_shader,
                vs: if skin { "vs_skinned" } else { "vs_main" },
                fs,
                buffers: if skin { &skinned } else { &plain },
                targets,
                depth: Some(depth),
                topology: tri,
            })
        };
        let fullscreen = |label, layout, module, fs, format| {
            make(Desc {
                label,
                layout,
                module,
                vs: "vs_fullscreen",
                fs: Some(fs),
                buffers: &[],
                targets: &[target(format, None)],
                depth: None,
                topology: tri,
            })
        };
        let lighting_pl =
            pipeline_layout("lighting", &[Some(&frame_layout), Some(&view_layouts.lighting), None, Some(env_layout)]);
        let background_pl = pipeline_layout("background", &[Some(&frame_layout), Some(&view_layouts.background)]);
        let lights_pl =
            pipeline_layout("lights", &[Some(&frame_layout), Some(&view_layouts.lighting), Some(&lights_layout)]);
        let water_pl = pipeline_layout(
            "water",
            &[Some(&frame_layout), Some(&water_layout), Some(&water_view_layout), Some(env_layout)],
        );
        // The inside of each light's box: it covers the light's pixels with the
        // eye in it too.
        let lights = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("lights"),
            layout: Some(&lights_pl),
            vertex: wgpu::VertexState {
                module: &lights_shader,
                entry_point: Some("vs_light"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Front), ..Default::default() },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &lights_shader,
                entry_point: Some("fs_light"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: Some(wgpu::BlendState { color: add, alpha: add }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let ssao_pl = pipeline_layout("ssao", &[Some(&frame_layout), Some(&view_layouts.ssao)]);
        let blur_pl = pipeline_layout("ssao blur", &[None, Some(&view_layouts.blur)]);
        let pipelines = Pipelines {
            terrain: make(Desc {
                label: "terrain",
                layout: &terrain_pl,
                module: &terrain_shader,
                vs: "vs_main",
                fs: Some("fs_main"),
                buffers: &terrain_buffers,
                targets: &opaque_targets,
                depth: Some(depth(true, 0)),
                topology: tri,
            }),
            opaque: model("opaque", &gbuffer_pl, false, Some("fs_gbuffer"), &opaque_targets, depth(true, 0)),
            decal: model("decal", &gbuffer_pl, false, Some("fs_gbuffer"), &decal_targets, depth(false, 2)),
            lines: make(Desc {
                label: "lines",
                layout: &gbuffer_pl,
                module: &model_shader,
                vs: "vs_main",
                fs: Some("fs_gbuffer"),
                buffers: &plain,
                targets: &opaque_targets,
                depth: Some(depth(true, 0)),
                topology: wgpu::PrimitiveTopology::LineList,
            }),
            blend: model("blend", &forward_pl, false, Some("fs_forward"), &forward, depth(false, 0)),
            additive: model("additive", &forward_pl, false, Some("fs_forward"), &additive, depth(false, 0)),
            shadow: model("shadow", &shadow_pl, false, None, &[], shadow_depth.clone()),
            shadow_alpha: model("shadow alpha", &shadow_alpha_pl, false, Some("fs_shadow"), &[], shadow_depth.clone()),
            skinned_opaque: model(
                "skinned opaque",
                &skinned_gbuffer_pl,
                true,
                Some("fs_gbuffer"),
                &opaque_targets,
                depth(true, 0),
            ),
            skinned_decal: model(
                "skinned decal",
                &skinned_gbuffer_pl,
                true,
                Some("fs_gbuffer"),
                &decal_targets,
                depth(false, 2),
            ),
            skinned_blend: model(
                "skinned blend",
                &skinned_forward_pl,
                true,
                Some("fs_forward"),
                &forward,
                depth(false, 0),
            ),
            skinned_shadow: model("skinned shadow", &skinned_shadow_pl, true, None, &[], shadow_depth.clone()),
            skinned_shadow_alpha: model(
                "skinned shadow alpha",
                &skinned_shadow_alpha_pl,
                true,
                Some("fs_shadow"),
                &[],
                shadow_depth,
            ),
            ssao: fullscreen("ssao", &ssao_pl, &ssao_shader, "fs_ssao", SSAO_FORMAT),
            ssao_blur: fullscreen("ssao blur", &blur_pl, &ssao_shader, "fs_blur", SSAO_FORMAT),
            lighting: fullscreen("lighting", &lighting_pl, &deferred_shader, "fs_lighting", HDR_FORMAT),
            background: fullscreen("background", &background_pl, &deferred_shader, "fs_background", HDR_FORMAT),
            lights,
            water: make(Desc {
                label: "water",
                layout: &water_pl,
                module: &water_shader,
                vs: "vs_water",
                fs: Some("fs_water"),
                buffers: &plain,
                targets: &[target(HDR_FORMAT, None)],
                depth: Some(depth(true, 0)),
                topology: tri,
            }),
        };

        let white = Arc::new(GpuTexture::solid(ctx, "white", [255, 255, 255, 255]));
        let flat_normal = Arc::new(GpuTexture::solid(ctx, "flat normal", [128, 128, 255, 255]));
        let black = Arc::new(GpuTexture::solid(ctx, "black", [0, 0, 0, 0]));
        let mut r = Self {
            instance_buffer: Default::default(),
            bone_buffer: Default::default(),
            light_buffer: Default::default(),
            lights_layout,
            bones_layout,
            material_layout,
            terrain_layout,
            water_layout,
            water_view_layout,
            water_bump: std::sync::Mutex::new(Arc::new(GpuTexture::solid(ctx, "water bump", [128, 128, 128, 128]))),
            clamp_sampler,
            start: std::time::Instant::now(),
            view_layouts,
            frame_buffer,
            frame_group,
            shadow_frames,
            shadow_frame_group,
            shadow_layers,
            world_sky: std::sync::Mutex::new(world_sky),
            studio_sky: std::sync::Mutex::new(studio_sky),
            ssao_buffer,
            post: Post::new(ctx),
            sampler,
            point_sampler,
            pipelines,
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
            tint: [1.0; 4],
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
                    MaterialMap::Skin => 3,
                },
                desc.emissive_hours.is_some() as u32 | (desc.additive as u32) << 1,
                0,
            ],
            extra: {
                let [on, off] = desc.emissive_hours.unwrap_or_default();
                [desc.emissive, desc.translucency, on, off]
            },
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
        GpuMaterial {
            name: desc.shader_name.clone(),
            kind: MaterialKind::Standard,
            additive: desc.additive,
            bind_group,
            textures,
        }
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
        GpuMaterial {
            name: desc.shader_name.clone(),
            kind: MaterialKind::Terrain,
            additive: false,
            bind_group,
            textures,
        }
    }

    /// A material for the water shaders; `textures` by [`water_slot`].
    pub fn create_water_material(
        &self,
        ctx: &GpuContext,
        desc: &WaterDesc,
        textures: [Option<Arc<GpuTexture>>; water_slot::COUNT],
    ) -> GpuMaterial {
        let buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&desc.shader_name),
            contents: bytemuck::bytes_of(&desc.params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        // Without its maps: still water of a murky colour, no foam or pollen.
        let fallback = |slot: usize| match slot {
            water_slot::FLOW => GpuTexture::solid(ctx, "still water", [128, 128, 128, 0]),
            water_slot::FOG => GpuTexture::solid(ctx, "water colour", [40, 52, 46, 255]),
            _ => GpuTexture::solid(ctx, "no water layer", [0, 0, 0, 0]),
        };
        let owned: Vec<Arc<GpuTexture>> =
            textures.iter().enumerate().map(|(i, t)| t.clone().unwrap_or_else(|| Arc::new(fallback(i)))).collect();
        let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }];
        for (i, t) in owned.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: 1 + i as u32,
                resource: wgpu::BindingResource::TextureView(&t.view_2d),
            });
        }
        entries.push(wgpu::BindGroupEntry {
            binding: 1 + water_slot::COUNT as u32,
            resource: wgpu::BindingResource::Sampler(&self.sampler),
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&desc.shader_name),
            layout: &self.water_layout,
            entries: &entries,
        });
        const LABELS: [&str; water_slot::COUNT] = ["Flow", "Fog", "Foam", "Pollen", "Pollen placement"];
        let textures = LABELS.iter().zip(textures).filter_map(|(l, t)| Some((*l, t?))).collect();
        GpuMaterial { name: desc.shader_name.clone(), kind: MaterialKind::Water, additive: false, bind_group, textures }
    }

    /// The ripples' normal maps the water shaders use (the game's
    /// "waterbump2", in `graphics.ytd`).
    pub fn set_water_bump(&self, texture: Arc<GpuTexture>) {
        *self.water_bump.lock().unwrap() = texture;
    }

    pub fn create_targets(&self, ctx: &GpuContext, width: u32, height: u32) -> RenderTargets {
        let sky = self.world_sky.lock().unwrap();
        RenderTargets::new(ctx, &self.view_layouts, &self.ssao_buffer, &self.post, &sky, width, height)
    }

    /// The world sky's moon and stars ([`Sky::set_textures`]).
    pub fn set_sky_textures(&self, ctx: &GpuContext, moon: Option<&GpuTexture>, stars: Option<&GpuTexture>) {
        self.world_sky.lock().unwrap().set_textures(ctx, moon, stars);
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
        self.render(
            ctx,
            targets,
            &view,
            settings,
            &[instance],
            &[DrawBatch { model, lod, instances: 0..1, buffer: None }],
        );
    }

    /// Draws batches of model instances into `targets.output`: `instances`
    /// holds every instance, and each batch draws one model's LOD with a
    /// range of them. Everything casts shadows.
    pub fn render(
        &self,
        ctx: &GpuContext,
        targets: &RenderTargets,
        camera: &CameraView,
        settings: &ViewSettings,
        instances: &[Instance],
        batches: &[DrawBatch],
    ) -> RenderStats {
        let cascades = Cascades::new(camera, settings.sun_dir);
        let casters = std::array::from_fn(|_| Vec::new());
        let shadows = settings.shadows.then_some((&cascades, &casters));
        self.draw_scene(ctx, targets, camera, settings, &Scene { instances, batches, shadows, lights: &[] }, true)
    }

    /// Draws a scene: its batches as [`Renderer::render`] does, and its own
    /// shadow casters.
    pub fn render_scene(
        &self,
        ctx: &GpuContext,
        targets: &RenderTargets,
        camera: &CameraView,
        settings: &ViewSettings,
        scene: &Scene,
    ) -> RenderStats {
        self.draw_scene(ctx, targets, camera, settings, scene, false)
    }

    /// `all_cast`: the batches cast the shadows, not the scene's casters.
    fn draw_scene(
        &self,
        ctx: &GpuContext,
        targets: &RenderTargets,
        camera: &CameraView,
        settings: &ViewSettings,
        scene: &Scene,
        all_cast: bool,
    ) -> RenderStats {
        let mut stats = RenderStats { instances: scene.instances.len(), ..Default::default() };
        let cascades = scene.shadows.filter(|_| settings.shadows).map(|(c, _)| c);
        self.frame(ctx, targets, camera, settings, scene.instances, cascades, scene.lights, &mut |rp, pass, frame| {
            let batches = match (pass, scene.shadows) {
                (Pass::Shadow(i), Some((_, casters))) if !all_cast => &casters[i][..],
                _ => scene.batches,
            };
            // The instance buffer bound: the frame's, or a batch's own.
            let mut bound = frame.map(|b| b as *const wgpu::Buffer);
            for batch in batches {
                if batch.instances.is_empty() {
                    continue;
                }
                if let Some(b) = batch.buffer.or(frame)
                    && bound != Some(b as *const _)
                {
                    rp.set_vertex_buffer(1, b.slice(..));
                    bound = Some(b as *const _);
                }
                for draw in &batch.model.lods[batch.lod.min(3)] {
                    let Some(pipeline) = self.pipeline(rp, draw, pass) else { continue };
                    rp.set_pipeline(pipeline);
                    rp.set_bind_group(1, &draw.material.bind_group, &[]);
                    rp.set_vertex_buffer(0, draw.mesh.vertices.slice(..));
                    rp.set_index_buffer(draw.mesh.indices.slice(..), wgpu::IndexFormat::Uint16);
                    rp.draw_indexed(0..draw.mesh.index_count, 0, batch.instances.clone());
                    stats.draw_calls += 1;
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
        let cascades = settings.shadows.then(|| Cascades::new(camera, settings.sun_dir));
        self.frame(ctx, targets, camera, settings, &instances, cascades.as_ref(), &[], &mut |rp, pass, _| {
            for ((m, insts), &first) in models.iter().zip(&draw_instances).zip(&rig_slots) {
                for (draw, &inst) in m.model.lods[m.lod.min(3)].iter().zip(insts) {
                    let skinned = draw.rig.zip(draw.mesh.skin.as_ref()).zip(bone_group.as_ref());
                    let pipeline = match skinned {
                        Some(((rig, skin), group)) if !draw.mesh.lines => {
                            let Some(alpha) = in_pass(draw, pass) else { continue };
                            rp.set_vertex_buffer(2, skin.slice(..));
                            let offset = (first + rig as u64) * slot;
                            rp.set_bind_group(2, group, &[offset as u32]);
                            let p = &self.pipelines;
                            match pass {
                                Pass::Shadow(_) if alpha => &p.skinned_shadow_alpha,
                                Pass::Shadow(_) => &p.skinned_shadow,
                                Pass::Blend | Pass::Water => &p.skinned_blend,
                                Pass::Decal => &p.skinned_decal,
                                Pass::Opaque => &p.skinned_opaque,
                            }
                        }
                        _ => {
                            let Some(p) = self.pipeline(rp, draw, pass) else { continue };
                            p
                        }
                    };
                    rp.set_pipeline(pipeline);
                    rp.set_bind_group(1, &draw.material.bind_group, &[]);
                    rp.set_vertex_buffer(0, draw.mesh.vertices.slice(..));
                    rp.set_index_buffer(draw.mesh.indices.slice(..), wgpu::IndexFormat::Uint16);
                    rp.draw_indexed(0..draw.mesh.index_count, 0, inst..inst + 1);
                    stats.draw_calls += 1;
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

    /// The pipeline of an unskinned draw in a pass (binding the terrain's
    /// extra stream); `None` when it isn't drawn in that pass.
    fn pipeline<'a>(
        &'a self,
        rp: &mut wgpu::RenderPass<'_>,
        draw: &'a GpuDraw,
        pass: Pass,
    ) -> Option<&'a wgpu::RenderPipeline> {
        let alpha = in_pass(draw, pass)?;
        let p = &self.pipelines;
        Some(match (pass, draw.material.kind, draw.mesh.lines) {
            (Pass::Shadow(_), ..) if alpha => &p.shadow_alpha,
            (Pass::Shadow(_), ..) => &p.shadow,
            (Pass::Water, ..) => &p.water,
            (_, MaterialKind::Terrain, _) => {
                rp.set_vertex_buffer(2, draw.mesh.extra.as_ref()?.slice(..));
                &p.terrain
            }
            (_, _, true) => &p.lines,
            (Pass::Blend, ..) if draw.material.additive => &p.additive,
            (Pass::Blend, ..) => &p.blend,
            (Pass::Decal, ..) => &p.decal,
            _ => &p.opaque,
        })
    }

    /// A buffer of instances for [`DrawBatch::buffer`], for instances drawn
    /// again and again (the map's instanced props and grass).
    pub fn create_instance_buffer(&self, ctx: &GpuContext, instances: &[Instance]) -> wgpu::Buffer {
        ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("instance batch"),
            contents: bytemuck::cast_slice(instances),
            usage: wgpu::BufferUsages::VERTEX,
        })
    }

    /// Writes the lights into their buffer (grown as needed) and gives its
    /// bind group.
    fn upload_lights(&self, ctx: &GpuContext, lights: &[Light]) -> wgpu::BindGroup {
        let data: Vec<LightUniform> = lights.iter().map(LightUniform::from).collect();
        let bytes: &[u8] = bytemuck::cast_slice(&data);
        let mut slot = self.light_buffer.lock().unwrap();
        if slot.as_ref().is_none_or(|b| b.buffer.size() < bytes.len() as u64) {
            let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("lights"),
                size: (bytes.len() as u64).next_power_of_two().max(16 * 1024),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("lights"),
                layout: &self.lights_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
            });
            *slot = Some(LightBuffer { buffer, group });
        }
        let b = slot.as_ref().unwrap();
        ctx.queue.write_buffer(&b.buffer, 0, bytes);
        b.group.clone()
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

    /// The frame's constants for a view.
    fn frame_uniform(
        &self,
        targets: &RenderTargets,
        camera: &CameraView,
        settings: &ViewSettings,
        cascades: Option<&Cascades>,
    ) -> FrameUniform {
        let view_proj = camera.projection * camera.view;
        let (w, h) = (targets.width as f32, targets.height as f32);
        let mut shadows = ShadowsUniform::zeroed();
        if let Some(c) = cascades {
            for (i, cascade) in c.cascades.iter().enumerate() {
                shadows.cascades[i] = cascade.view_proj.to_cols_array_2d();
                shadows.texels[i] = cascade.texel;
                shadows.depths[i] = cascade.max.z - cascade.min.z;
            }
            shadows.params = [CASCADES as f32, CASCADE_BLEND, 1.0 / SHADOW_SIZE as f32, 0.0];
        }
        let [r, g, b] = settings.background.map(|c| c as f32);
        FrameUniform {
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            view: camera.view.to_cols_array_2d(),
            proj: camera.projection.to_cols_array_2d(),
            inv_proj: camera.projection.inverse().to_cols_array_2d(),
            camera_pos: camera.eye.extend(1.0).to_array(),
            sun_dir: settings.sun_dir.normalize_or(Vec3::Z).extend(0.0).to_array(),
            sun_colour: settings.sun_colour.extend(0.0).to_array(),
            fill: settings.fill.extend(0.0).to_array(),
            background: [r, g, b, 1.0],
            options: [settings.debug as u32, settings.ssao as u32, 0, 0],
            viewport: [w, h, 1.0 / w, 1.0 / h],
            time: [self.start.elapsed().as_secs_f32() % 4096.0, settings.hour.unwrap_or(-1.0), 0.0, 0.0],
            shadows,
        }
    }

    /// One frame: the constants and instances, then the passes (`draw` draws
    /// a pass's models, with the frame bound at 0 and the instances at vertex
    /// buffer 1), ending in `targets.output`.
    #[allow(clippy::too_many_arguments)]
    fn frame(
        &self,
        ctx: &GpuContext,
        targets: &RenderTargets,
        camera: &CameraView,
        settings: &ViewSettings,
        instances: &[Instance],
        cascades: Option<&Cascades>,
        lights: &[Light],
        draw: &mut dyn FnMut(&mut wgpu::RenderPass<'_>, Pass, Option<&wgpu::Buffer>),
    ) {
        let frame = self.frame_uniform(targets, camera, settings, cascades);
        ctx.queue.write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&frame));
        if let Some(c) = cascades {
            let mut bytes = vec![0u8; (FRAME_STRIDE * CASCADES as u64) as usize];
            for (i, cascade) in c.cascades.iter().enumerate() {
                let f = FrameUniform { view_proj: cascade.view_proj.to_cols_array_2d(), ..frame };
                let at = i * FRAME_STRIDE as usize;
                bytes[at..at + size_of::<FrameUniform>()].copy_from_slice(bytemuck::bytes_of(&f));
            }
            ctx.queue.write_buffer(&self.shadow_frames, 0, &bytes);
        }

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
        let models_pass = |rp: &mut wgpu::RenderPass<'_>,
                           draw: &mut dyn FnMut(&mut wgpu::RenderPass<'_>, Pass, Option<&wgpu::Buffer>),
                           pass| {
            if let Some(ib) = &instance_buffer {
                rp.set_vertex_buffer(1, ib.slice(..));
            }
            draw(rp, pass, instance_buffer.as_ref());
        };
        let colour = |view, load: wgpu::LoadOp<wgpu::Color>| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })
        };
        let clear = wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT);

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        // The sky's environment maps, redrawn when its lights change.
        let mut sky = match &settings.sky {
            Some(_) => self.world_sky.lock().unwrap(),
            None => self.studio_sky.lock().unwrap(),
        };
        match &settings.sky {
            Some(lights) => sky.update(ctx, &mut encoder, lights, camera.eye),
            None => sky.update(ctx, &mut encoder, &SkyLights::default_day(), Vec3::ZERO),
        }
        // The sun's shadow maps, a cascade at a time.
        if cascades.is_some() {
            for (i, layer) in self.shadow_layers.iter().enumerate() {
                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("shadow"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: layer,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
                rp.set_bind_group(0, &self.shadow_frame_group, &[(i as u64 * FRAME_STRIDE) as u32]);
                models_pass(&mut rp, draw, Pass::Shadow(i));
            }
        }
        // The G-buffer: opaque, then decals over it.
        {
            let mut attachments: Vec<_> = targets.gbuffer.iter().map(|v| colour(v, clear)).collect();
            attachments.push(colour(&targets.ids_view, clear));
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("g-buffer"),
                color_attachments: &attachments,
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            rp.set_bind_group(0, &self.frame_group, &[]);
            models_pass(&mut rp, draw, Pass::Opaque);
            models_pass(&mut rp, draw, Pass::Decal);
        }
        fn fullscreen(
            encoder: &mut wgpu::CommandEncoder,
            label: &str,
            view: &wgpu::TextureView,
            pipeline: &wgpu::RenderPipeline,
            groups: &[(u32, &wgpu::BindGroup)],
        ) {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            rp.set_pipeline(pipeline);
            for (i, g) in groups {
                rp.set_bind_group(*i, *g, &[]);
            }
            rp.draw(0..3, 0..1);
        }
        let p = &self.pipelines;
        if settings.ssao {
            fullscreen(
                &mut encoder,
                "ssao",
                &targets.ssao_raw,
                &p.ssao,
                &[(0, &self.frame_group), (1, &targets.ssao_group)],
            );
            fullscreen(&mut encoder, "ssao blur", &targets.ssao, &p.ssao_blur, &[(1, &targets.blur_group)]);
        }
        fullscreen(
            &mut encoder,
            "lighting",
            &targets.lit,
            &p.lighting,
            &[(0, &self.frame_group), (1, &targets.lighting_group), (3, sky.env_group())],
        );
        let light_group =
            (settings.debug == DebugView::Lit && !lights.is_empty()).then(|| self.upload_lights(ctx, lights));
        if let Some(group) = &light_group {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("lights"),
                color_attachments: &[colour(&targets.lit, wgpu::LoadOp::Load)],
                ..Default::default()
            });
            rp.set_pipeline(&p.lights);
            rp.set_bind_group(0, &self.frame_group, &[]);
            rp.set_bind_group(1, &targets.lighting_group, &[]);
            rp.set_bind_group(2, group, &[]);
            rp.draw(0..36, 0..lights.len() as u32);
        }
        // Water over the lit scene, which shows through it: copies of the
        // image and depth under it, then the water's own colour and depth.
        {
            let copy = |from: &wgpu::Texture, to: &wgpu::Texture, encoder: &mut wgpu::CommandEncoder| {
                encoder.copy_texture_to_texture(from.as_image_copy(), to.as_image_copy(), from.size());
            };
            copy(&targets.lit_texture, &targets.under_texture, &mut encoder);
            copy(&targets.depth_texture, &targets.under_depth_texture, &mut encoder);
            let bump = self.water_bump.lock().unwrap().clone();
            let water_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("water view"),
                layout: &self.water_view_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&targets.under) },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&targets.under_depth),
                    },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&bump.view_2d) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.clamp_sampler) },
                ],
            });
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("water"),
                color_attachments: &[colour(&targets.lit, wgpu::LoadOp::Load)],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            rp.set_bind_group(0, &self.frame_group, &[]);
            rp.set_bind_group(2, &water_group, &[]);
            rp.set_bind_group(3, sky.env_group(), &[]);
            models_pass(&mut rp, draw, Pass::Water);
        }
        match &settings.sky {
            Some(lights) => sky.draw(ctx, &mut encoder, &targets.sky, &targets.hdr, camera, lights),
            None => fullscreen(
                &mut encoder,
                "background",
                &targets.hdr,
                &p.background,
                &[(0, &self.frame_group), (1, &targets.background_group)],
            ),
        }
        // Blended draws over it, lit as they go.
        {
            let load = wgpu::LoadOp::Load;
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("blended"),
                color_attachments: &[colour(&targets.hdr, load), colour(&targets.ids_view, load)],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            rp.set_bind_group(0, &self.frame_group, &[]);
            rp.set_bind_group(3, sky.env_group(), &[]);
            models_pass(&mut rp, draw, Pass::Blend);
        }
        let now = std::time::Instant::now();
        let last = targets.last_frame.lock().unwrap().replace(now);
        let post = PostSettings {
            exposure_ev: settings.exposure_ev,
            scotopic: settings.scotopic,
            bloom: settings.bloom,
            tonemap: settings.tonemap,
            passthrough: settings.debug != DebugView::Lit,
            elapsed: last.map_or(f32::INFINITY, |t| (now - t).as_secs_f32()),
            ..Default::default()
        };
        self.post.run(ctx, &mut encoder, &targets.post, &targets.display_view, &post);
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
    GpuMaterial { name: String::new(), kind: MaterialKind::Standard, additive: false, bind_group, textures: Vec::new() }
}
