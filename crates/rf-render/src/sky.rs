//! The sky: the C#'s atmosphere (single scattering in an Earth-like planet's
//! air, lit by the sun, the moon and the night sky's glow) drawn over the lit
//! scene, and the image-based lighting made from it.
//!
//! The planet's ground is the world's z = 0, its centre at
//! (0, 0, −[`PLANET_RADIUS`]): the camera's planet-space position is the eye
//! raised by the radius, and directions are the same in both.

use bytemuck::{Pod, Zeroable};
use glam::{DVec3, Mat4, Vec3, Vec4};

use crate::GpuContext;
use crate::camera::CameraView;

/// The planet's radius and the top of its atmosphere, in metres (the C#'s
/// `CreateEarthScene`).
pub const PLANET_RADIUS: f32 = 6_000_000.0;
pub const ATMOSPHERE_RADIUS: f32 = 6_150_000.0;
/// The sun's colour above the atmosphere (`CreateEarthScene`'s star colour).
pub const STAR_COLOUR: Vec3 = Vec3::new(10.0, 10.0, 12.1);
/// The moon's, without a timecycle: the star's × 0.002.
pub const MOON_COLOUR: Vec3 = Vec3::new(0.02, 0.02, 0.0242);
/// The prefiltered cube's mips: roughness `r` is at mip `r × (PREFILTERED_MIPS − 1)`.
pub const PREFILTERED_MIPS: u32 = 5;
/// The albedo of the ground the env cube shows below the horizon.
pub const ENV_GROUND_ALBEDO: Vec3 = Vec3::new(0.2, 0.18, 0.15);

// Atmosphere.CreateEarthLike.
const BRIGHTNESS: f32 = 4.0;
const OPACITY: f32 = 1.0;
const SCATTER_COLOUR: Vec3 = Vec3::new(5.602, 9.478, 19.646);
const K_RAYLEIGH: f32 = 8.0;
const K_MIE: f32 = 7.0;
const MIE_G: f32 = -0.95;
/// AtmosphereShader's march steps.
const STEPS: u32 = 15;

const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Rgba32Float isn't filterable without a feature.
const CUBE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const BRDF_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;
const ENV_SIZE: u32 = 256;
const IRRADIANCE_SIZE: u32 = 32;
const PREFILTERED_SIZE: u32 = 128;
const BRDF_SIZE: u32 = 512;

/// How far the eye moves before the env cube is redrawn: up or down, and
/// across (where the sky hardly changes).
const ENV_REDRAW_HEIGHT: f32 = 15.0;
const ENV_REDRAW_DISTANCE: f32 = 1000.0;
/// The env cube is drawn from at least this high above its ground.
const ENV_MIN_HEIGHT: f32 = 10.0;

const ENV_STAGES: wgpu::ShaderStages = wgpu::ShaderStages::FRAGMENT.union(wgpu::ShaderStages::COMPUTE);

const fn env_texture(binding: u32, view_dimension: wgpu::TextureViewDimension) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: ENV_STAGES,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension,
            multisampled: false,
        },
        count: None,
    }
}

/// The layout of [`Sky::env_group`], for the lighting pass's shader to
/// declare (fragment and compute):
///
/// ```wgsl
/// @group(N) @binding(0) var t_irradiance: texture_cube<f32>;  // Rgba16Float, 32²
/// @group(N) @binding(1) var t_prefiltered: texture_cube<f32>; // Rgba16Float, 128², 5 mips
/// @group(N) @binding(2) var t_brdf: texture_2d<f32>;          // Rg16Float, 512²
/// @group(N) @binding(3) var s_env: sampler;                   // linear, clamped, mipmapped
/// ```
///
/// All in world directions (Z up), as the C#'s `AmbientLight` uses them
/// (learnopengl.com's PBR/IBL), with `roughness = 1 − smoothness`:
/// - diffuse: `textureSample(t_irradiance, s_env, n).rgb × albedo` (π is in
///   the irradiance);
/// - specular: `textureSampleLevel(t_prefiltered, s_env, reflect(-v, n),
///   roughness × 4.0).rgb × (F × brdf.x + brdf.y)`, with
///   `brdf = textureSample(t_brdf, s_env, vec2(max(dot(n, v), 0.0), roughness)).rg`
///   and F Schlick's with roughness.
pub const ENV_BINDINGS: [wgpu::BindGroupLayoutEntry; 4] = [
    env_texture(0, wgpu::TextureViewDimension::Cube),
    env_texture(1, wgpu::TextureViewDimension::Cube),
    env_texture(2, wgpu::TextureViewDimension::D2),
    wgpu::BindGroupLayoutEntry {
        binding: 3,
        visibility: ENV_STAGES,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    },
];

/// The atmosphere's light sources, in the world (towards each light), as the
/// C#'s `Player.UpdateSceneLight` sets them from a timecycle:
/// - `sun_colour`: [`STAR_COLOUR`] × the timecycle's sky sun intensity;
/// - `moon_colour`: its moon intensity × sky moon intensity (the moon is
///   opposite the sun);
/// - `glow_colour`: its sky glow, the night sky's airglow, a light from the
///   zenith.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyLights {
    /// Towards the sun.
    pub sun_dir: Vec3,
    pub sun_colour: Vec3,
    pub moon_colour: Vec3,
    pub glow_colour: Vec3,
}

impl SkyLights {
    /// Daylight for views without a timecycle: the C#'s sun and moon, the sun
    /// as high as the renderer's default.
    pub fn default_day() -> Self {
        Self {
            sun_dir: Vec3::new(0.4, -0.6, 0.7).normalize(),
            sun_colour: STAR_COLOUR,
            moon_colour: MOON_COLOUR,
            glow_colour: Vec3::ZERO,
        }
    }

    /// The shader's lights (the glow only when it shines): directions,
    /// colours and how many.
    fn packed(&self) -> ([[f32; 4]; 4], [[f32; 4]; 4], u32) {
        let sun = self.sun_dir.normalize_or(Vec3::Z);
        let lights = [(sun, self.sun_colour), (-sun, self.moon_colour), (Vec3::Z, self.glow_colour)];
        let (mut dirs, mut colours) = ([[0.0; 4]; 4], [[0.0; 4]; 4]);
        for (i, (dir, colour)) in lights.into_iter().enumerate() {
            dirs[i] = dir.extend(0.0).to_array();
            colours[i] = colour.extend(1.0).to_array();
        }
        (dirs, colours, if self.glow_colour == Vec3::ZERO { 2 } else { 3 })
    }
}

/// `Atmo` in atmosphere.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct AtmoUniform {
    inv_view_proj: [[f32; 4]; 4],
    camera: [f32; 4],
    rad: [f32; 4],
    colour: [f32; 4],
    k: [f32; 4],
    g: [f32; 4],
    ke_star: [f32; 4],
    counts: [u32; 4],
    ground: [f32; 4],
    light_dirs: [[f32; 4]; 4],
    light_colours: [[f32; 4]; 4],
}

impl AtmoUniform {
    fn new(lights: &SkyLights, eye: Vec3, inv_view_proj: Mat4, ground: Option<Vec3>) -> Self {
        let (light_dirs, light_colours, count) = lights.packed();
        Self {
            inv_view_proj: inv_view_proj.to_cols_array_2d(),
            camera: [eye.x, eye.y, eye.z + PLANET_RADIUS, 0.0],
            rad: [PLANET_RADIUS, ATMOSPHERE_RADIUS, BRIGHTNESS, OPACITY],
            colour: SCATTER_COLOUR.extend(1.0).to_array(),
            k: [K_RAYLEIGH, K_MIE, 0.0, 0.0],
            g: [MIE_G; 4],
            ke_star: [2.0 / K_RAYLEIGH, 2.0 / K_MIE, 0.0, 0.0],
            counts: [count, STEPS, 0, 0],
            ground: ground.map_or([0.0; 4], |g| g.extend(1.0).to_array()),
            light_dirs,
            light_colours,
        }
    }
}

/// A view's resources for [`Sky::draw`]: its uniforms, and its lit image and
/// depth bound.
pub struct SkyTargets {
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
}

/// The atmosphere and the environment maps lit by it.
///
/// The C#'s env map renders the whole scene into its cube; this one is the
/// sky alone, with a plain ground below the horizon ([`ENV_GROUND_ALBEDO`]):
/// an editor's lighting shouldn't depend on where the camera last stopped.
pub struct Sky {
    scene_layout: wgpu::BindGroupLayout,
    scene: wgpu::RenderPipeline,
    env_sky: wgpu::RenderPipeline,
    irradiance: wgpu::RenderPipeline,
    prefilter: wgpu::RenderPipeline,
    env_buffer: wgpu::Buffer,
    env_sky_group: wgpu::BindGroup,
    /// The env cube, for the convolutions.
    convolve_group: wgpu::BindGroup,
    env_faces: Vec<wgpu::TextureView>,
    irradiance_faces: Vec<wgpu::TextureView>,
    /// By mip, then face.
    prefiltered_faces: Vec<wgpu::TextureView>,
    env_layout: wgpu::BindGroupLayout,
    env_group: wgpu::BindGroup,
    /// What the env cube shows: the lights and the eye.
    drawn: Option<(SkyLights, Vec3)>,
}

fn layout_entry(binding: u32, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty, count: None }
}

fn texture_type(sample_type: wgpu::TextureSampleType, view_dimension: wgpu::TextureViewDimension) -> wgpu::BindingType {
    wgpu::BindingType::Texture { sample_type, view_dimension, multisampled: false }
}

/// The screen triangle drawn into one target; the instance tells the shader
/// which cube face (and mip) it is.
fn face_pass(
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    group: Option<&wgpu::BindGroup>,
    instance: u32,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("sky"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
        })],
        ..Default::default()
    });
    pass.set_pipeline(pipeline);
    if let Some(group) = group {
        pass.set_bind_group(0, group, &[]);
    }
    pass.draw(0..3, instance..instance + 1);
}

impl Sky {
    /// The pipelines and environment maps; the BRDF table is computed here,
    /// once.
    pub fn new(ctx: &GpuContext) -> Self {
        let device = &ctx.device;
        let atmosphere = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("atmosphere.wgsl"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(include_str!("shaders/cube.wgsl"), include_str!("shaders/atmosphere.wgsl")).into(),
            ),
        });
        let envmap = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("envmap.wgsl"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(include_str!("shaders/cube.wgsl"), include_str!("shaders/envmap.wgsl")).into(),
            ),
        });

        let bind_layout = |label: &str, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries })
        };
        let uniform = layout_entry(
            0,
            wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(size_of::<AtmoUniform>() as u64),
            },
        );
        let d2 = wgpu::TextureViewDimension::D2;
        let scene_layout = bind_layout(
            "sky scene",
            &[
                uniform,
                layout_entry(1, texture_type(wgpu::TextureSampleType::Float { filterable: false }, d2)),
                layout_entry(2, texture_type(wgpu::TextureSampleType::Depth, d2)),
            ],
        );
        let env_sky_layout = bind_layout("sky env cube", &[uniform]);
        let convolve_layout = bind_layout(
            "sky convolve",
            &[
                layout_entry(
                    0,
                    texture_type(wgpu::TextureSampleType::Float { filterable: true }, wgpu::TextureViewDimension::Cube),
                ),
                layout_entry(1, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        );
        let env_layout = bind_layout("sky env maps", &ENV_BINDINGS);

        let pipeline = |label: &str,
                        module: &wgpu::ShaderModule,
                        layout: Option<&wgpu::BindGroupLayout>,
                        entry: &str,
                        format: wgpu::TextureFormat| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[layout],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_face"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let scene = pipeline("sky scene", &atmosphere, Some(&scene_layout), "fs_scene", HDR_FORMAT);
        let env_sky = pipeline("sky env cube", &atmosphere, Some(&env_sky_layout), "fs_env", CUBE_FORMAT);
        let irradiance = pipeline("sky irradiance", &envmap, Some(&convolve_layout), "fs_irradiance", CUBE_FORMAT);
        let prefilter = pipeline("sky prefilter", &envmap, Some(&convolve_layout), "fs_prefilter", CUBE_FORMAT);
        let brdf = pipeline("sky BRDF", &envmap, None, "fs_brdf", BRDF_FORMAT);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sky env"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let texture = |label: &str, size: u32, layers: u32, mips: u32, format: wgpu::TextureFormat| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: layers },
                mip_level_count: mips,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let faces = |t: &wgpu::Texture, mips: u32| -> Vec<wgpu::TextureView> {
            (0..mips)
                .flat_map(|mip| (0..6).map(move |face| (mip, face)))
                .map(|(mip, face)| {
                    t.create_view(&wgpu::TextureViewDescriptor {
                        label: Some("sky cube face"),
                        dimension: Some(d2),
                        base_mip_level: mip,
                        mip_level_count: Some(1),
                        base_array_layer: face,
                        array_layer_count: Some(1),
                        ..Default::default()
                    })
                })
                .collect()
        };
        let cube_view = |t: &wgpu::Texture| {
            t.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::Cube),
                ..Default::default()
            })
        };
        let env_cube = texture("sky env cube", ENV_SIZE, 6, 1, CUBE_FORMAT);
        let irradiance_cube = texture("sky irradiance", IRRADIANCE_SIZE, 6, 1, CUBE_FORMAT);
        let prefiltered_cube = texture("sky prefiltered", PREFILTERED_SIZE, 6, PREFILTERED_MIPS, CUBE_FORMAT);
        let brdf_table = texture("sky BRDF", BRDF_SIZE, 1, 1, BRDF_FORMAT).create_view(&Default::default());

        let env_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky env cube"),
            size: size_of::<AtmoUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let env_sky_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky env cube"),
            layout: &env_sky_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: env_buffer.as_entire_binding() }],
        });
        let convolve_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky convolve"),
            layout: &convolve_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&cube_view(&env_cube)),
                },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let env_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky env maps"),
            layout: &env_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&cube_view(&irradiance_cube)),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&cube_view(&prefiltered_cube)),
                },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&brdf_table) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("sky BRDF") });
        face_pass(&mut encoder, &brdf_table, &brdf, None, 0);
        ctx.queue.submit([encoder.finish()]);

        Self {
            scene_layout,
            scene,
            env_sky,
            irradiance,
            prefilter,
            env_buffer,
            env_sky_group,
            convolve_group,
            env_faces: faces(&env_cube, 1),
            irradiance_faces: faces(&irradiance_cube, 1),
            prefiltered_faces: faces(&prefiltered_cube, PREFILTERED_MIPS),
            env_layout,
            env_group,
            drawn: None,
        }
    }

    /// Redraws the env cube (the atmosphere seen from `eye`) and its
    /// irradiance and prefiltered convolutions, only when `lights` changed or
    /// the eye moved more than 15 m up or down or 1 km across; cheap to call
    /// every frame.
    pub fn update(&mut self, ctx: &GpuContext, encoder: &mut wgpu::CommandEncoder, lights: &SkyLights, eye: Vec3) {
        let stale = self.drawn.is_none_or(|(l, e)| {
            l != *lights
                || (eye.z - e.z).abs() > ENV_REDRAW_HEIGHT
                || eye.truncate().distance(e.truncate()) > ENV_REDRAW_DISTANCE
        });
        if !stale {
            return;
        }
        self.drawn = Some((*lights, eye));
        let eye = Vec3::new(eye.x, eye.y, eye.z.max(ENV_MIN_HEIGHT));
        let u = AtmoUniform::new(lights, eye, Mat4::IDENTITY, Some(ENV_GROUND_ALBEDO));
        ctx.queue.write_buffer(&self.env_buffer, 0, bytemuck::bytes_of(&u));
        for (face, view) in (0..).zip(&self.env_faces) {
            face_pass(encoder, view, &self.env_sky, Some(&self.env_sky_group), face);
        }
        for (face, view) in (0..).zip(&self.irradiance_faces) {
            face_pass(encoder, view, &self.irradiance, Some(&self.convolve_group), face);
        }
        for (index, view) in (0..).zip(&self.prefiltered_faces) {
            face_pass(encoder, view, &self.prefilter, Some(&self.convolve_group), index);
        }
    }

    /// The layout of [`Sky::env_group`]: [`ENV_BINDINGS`].
    pub fn env_layout(&self) -> &wgpu::BindGroupLayout {
        &self.env_layout
    }

    /// For the lighting pass: the irradiance cube, the prefiltered cube, the
    /// BRDF table and their sampler (see [`ENV_BINDINGS`]). The cubes are
    /// black until the first [`Sky::update`].
    pub fn env_group(&self) -> &wgpu::BindGroup {
        &self.env_group
    }

    /// A view's resources for [`Sky::draw`], made again when the view's
    /// textures are: `lit` an Rgba16Float image and `depth` a Depth32Float
    /// one, both single-sampled and with `TEXTURE_BINDING`.
    pub fn view_targets(&self, ctx: &GpuContext, lit: &wgpu::TextureView, depth: &wgpu::TextureView) -> SkyTargets {
        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky"),
            size: size_of::<AtmoUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky"),
            layout: &self.scene_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(lit) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(depth) },
            ],
        });
        SkyTargets { buffer, group }
    }

    /// The atmosphere over the lit scene, as AtmospherePS: writes `target`
    /// (Rgba16Float, the size of the view) with lit × transmittance +
    /// inscattered light, the sky where the depth is 0 and aerial
    /// perspective where there's geometry. `lit` should be black where
    /// nothing is drawn.
    pub fn draw(
        &self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        targets: &SkyTargets,
        target: &wgpu::TextureView,
        camera: &CameraView,
        lights: &SkyLights,
    ) {
        // Camera-relative: planet-space positions are too big for the matrix.
        let mut view = camera.view;
        view.w_axis = Vec4::W;
        let u = AtmoUniform::new(lights, camera.eye, (camera.projection * view).inverse(), None);
        ctx.queue.write_buffer(&targets.buffer, 0, bytemuck::bytes_of(&u));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sky"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.scene);
        pass.set_bind_group(0, &targets.group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// A light's colour after the atmosphere, seen from `eye` (the C#'s
    /// `Atmosphere.GetAttenuatedLight`, which the C# calls with the level's
    /// origin, `Vec3::ZERO`). Black once the light is well below the horizon.
    ///
    /// The C#'s directional light is this for the sun ([`STAR_COLOUR`]), or
    /// the moon (the timecycle's moon intensity) when the sun is down, times
    /// the timecycle's light colour.
    pub fn attenuated_light(eye: Vec3, colour: Vec3, towards_light: Vec3) -> Vec3 {
        let (irad, orad) = (f64::from(PLANET_RADIUS), f64::from(ATMOSPHERE_RADIUS));
        let pos = DVec3::new(eye.x.into(), eye.y.into(), f64::from(eye.z) + irad);
        let dir = towards_light.as_dvec3().normalize_or(DVec3::Z);
        // A little under the horizon, the light still comes through the air.
        if ray_sphere(pos, dir, irad * 0.98).is_some() {
            return Vec3::ZERO;
        }
        let Some(t) = ray_sphere(pos, dir, orad) else { return colour };
        let p = pos + dir * t;
        let len = far_intersection(p, dir, orad);
        let ascale = 2.0 / (orad - irad);
        let depth = |k0: f32| optical_depth(p, dir, len, k0.into(), irad, orad) * ascale * k4pi(k0.into());
        let attenuation = (-(SCATTER_COLOUR.as_dvec3() * depth(K_RAYLEIGH)) - depth(K_MIE)).exp();
        colour * attenuation.min(DVec3::ONE).as_vec3()
    }
}

/// The distance along a ray to a sphere around the origin, 0 from inside it
/// (Ericson's, as the C#'s `Collision.RayIntersectsSphere`).
fn ray_sphere(pos: DVec3, dir: DVec3, r: f64) -> Option<f64> {
    let b = pos.dot(dir);
    let c = pos.length_squared() - r * r;
    if c > 0.0 && b > 0.0 {
        return None;
    }
    let disc = b * b - c;
    (disc >= 0.0).then(|| (-b - disc.sqrt()).max(0.0))
}

fn far_intersection(pos: DVec3, dir: DVec3, r: f64) -> f64 {
    let b = 2.0 * pos.dot(dir);
    let c = pos.length_squared() - r * r;
    0.5 * (-b + (b * b - 4.0 * c).max(0.0).sqrt())
}

fn k4pi(k0: f64) -> f64 {
    (1.0 / k0) * 4.0 * 0.031415
}

/// As the shader's, with the height clamped to the atmosphere.
fn optical_depth(start: DVec3, dir: DVec3, len: f64, k0: f64, r1: f64, r2: f64) -> f64 {
    let density = |p: DVec3| (-k0 * ((p.length() - r1) / (r2 - r1)).clamp(0.0, 1.0)).exp();
    let (y0, y1, y2) = (density(start), density(start + dir * len * 0.5), density(start + dir * len));
    (2.0 * len / 3.0) * (y0 + 4.0 * y1 + y2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sun_dims_and_reddens_near_the_horizon() {
        let at = |elevation: f32| {
            let (s, c) = elevation.to_radians().sin_cos();
            Sky::attenuated_light(Vec3::ZERO, STAR_COLOUR, Vec3::new(0.0, c, s))
        };
        let (high, low) = (at(60.0), at(3.0));
        assert!(high.x > 7.0, "{high}");
        assert!(low.length() < high.length() * 0.6, "{low} vs {high}");
        assert!(low.x / low.z > 2.0 * high.x / high.z, "{low} vs {high}");
        assert_eq!(at(-20.0), Vec3::ZERO);
    }
}
