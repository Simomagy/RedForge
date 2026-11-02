//! Renders the sky at a few times of day to PNGs, to see the atmosphere and
//! its environment maps without the app:
//!
//! ```sh
//! cargo run --release -p rf-render --example sky -- [out folder]
//! ```
//!
//! Each image has three panels from the same camera: the atmosphere over a
//! ground plane, the prefiltered env cube's sharpest mip (which should show
//! the same sky), and the irradiance cube (over π) with the BRDF table in its
//! corner.

use std::path::PathBuf;

use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use glam::{Vec3, Vec4};
use rf_render::sky::{MOON_COLOUR, STAR_COLOUR};
use rf_render::{FlyCamera, GpuContext, Sky, SkyLights};

const WIDTH: u32 = 960;
const HEIGHT: u32 = 400;
const EXPOSURE: f32 = 0.4;
const GROUND_ALBEDO: Vec3 = Vec3::new(0.2, 0.2, 0.16);

const SHADER: &str = r#"
struct Params {
    view_proj: mat4x4<f32>,
    // Clip space to camera-relative world space.
    inv_view_proj: mat4x4<f32>,
    ground: vec4<f32>,
    // x: exposure, y: panel height.
    values: vec4<f32>,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var t_sky: texture_2d<f32>;
@group(1) @binding(0) var t_irradiance: texture_cube<f32>;
@group(1) @binding(1) var t_prefiltered: texture_cube<f32>;
@group(1) @binding(2) var t_brdf: texture_2d<f32>;
@group(1) @binding(3) var s_env: sampler;

@vertex
fn vs_ground(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    return params.view_proj * vec4<f32>(corners[i] * 20000.0, 0.0, 1.0);
}

@fragment
fn fs_ground() -> @location(0) vec4<f32> {
    return vec4<f32>(params.ground.rgb, 1.0);
}

@vertex
fn vs_screen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_present(@builtin(position) clip: vec4<f32>) -> @location(0) vec4<f32> {
    let h = u32(params.values.y);
    let px = vec2<u32>(clip.xy);
    let panel = px.y / h;
    let y = px.y % h;
    let size = vec2<f32>(textureDimensions(t_sky));
    let ndc = vec2<f32>((f32(px.x) + 0.5) / size.x * 2.0 - 1.0, 1.0 - (f32(y) + 0.5) / size.y * 2.0);
    let near = params.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    let dir = normalize(near.xyz / near.w);
    var c: vec3<f32>;
    if panel == 0u {
        c = textureLoad(t_sky, vec2<u32>(px.x, y), 0).rgb;
    } else if panel == 1u {
        c = textureSampleLevel(t_prefiltered, s_env, dir, 0.0).rgb;
    } else if px.x < 128u && y < 128u {
        let uv = (vec2<f32>(f32(px.x), f32(y)) + 0.5) / 128.0;
        c = vec3<f32>(textureSampleLevel(t_brdf, s_env, uv, 0.0).rg, 0.0) / params.values.x;
    } else {
        // What a white diffuse surface facing that way shows.
        c = textureSampleLevel(t_irradiance, s_env, dir, 0.0).rgb / 3.14159265;
    }
    // Clamped keeping the hue, so the glow round a low sun shows its colour.
    c *= params.values.x;
    return vec4<f32>(c / max(max(c.r, max(c.g, c.b)), 1.0), 1.0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    ground: [f32; 4],
    values: [f32; 4],
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let ctx = pollster::block_on(GpuContext::headless())?;
    let device = &ctx.device;
    let mut sky = Sky::new(&ctx);

    let texture = |label: &str, height: u32, format: wgpu::TextureFormat, usage: wgpu::TextureUsages| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: WIDTH, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
            .create_view(&Default::default())
    };
    let target = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
    let hdr = wgpu::TextureFormat::Rgba16Float;
    let lit = texture("lit", HEIGHT, hdr, target);
    let depth = texture("depth", HEIGHT, wgpu::TextureFormat::Depth32Float, target);
    let with_sky = texture("sky", HEIGHT, hdr, target);
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("output"),
        size: wgpu::Extent3d { width: WIDTH, height: HEIGHT * 3, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let output_view = output.create_view(&Default::default());
    let sky_targets = sky.view_targets(&ctx, &lit, &depth);

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sky example"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let params_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("params"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
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
    let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("params"),
        size: size_of::<Params>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let params_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("params"),
        layout: &params_layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: params_buffer.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&with_sky) },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sky example"),
        bind_group_layouts: &[Some(&params_layout), Some(sky.env_layout())],
        immediate_size: 0,
    });
    let pipeline = |vs: &str, fs: &str, format: wgpu::TextureFormat, depth: bool| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(fs),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some(vs),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: depth.then(|| wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(fs),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    let ground = pipeline("vs_ground", "fs_ground", hdr, true);
    let present = pipeline("vs_screen", "fs_present", wgpu::TextureFormat::Rgba8UnormSrgb, false);

    // The sun's height in degrees, and the night's moon and airglow (the C#'s
    // timecycle at night).
    let cases =
        [("noon", 70.0, Vec3::ZERO), ("low", 5.0, Vec3::ZERO), ("night", -10.0, Vec3::new(0.0004, 0.0005, 0.001))];
    for (name, elevation, glow) in cases {
        let (s, c) = f32::to_radians(elevation).sin_cos();
        let night = glow != Vec3::ZERO;
        let lights = SkyLights {
            sun_dir: Vec3::new(0.3 * c, c, s).normalize(),
            sun_colour: STAR_COLOUR,
            moon_colour: if night { Vec3::splat(0.001) } else { MOON_COLOUR },
            glow_colour: glow,
        };
        let camera = FlyCamera { position: Vec3::new(0.0, 0.0, 2.0), yaw: 0.0, pitch: 0.12, fov_y: 70f32.to_radians() }
            .camera_view(WIDTH as f32 / HEIGHT as f32);
        let sun = Sky::attenuated_light(Vec3::ZERO, STAR_COLOUR, lights.sun_dir);
        let ground_colour = GROUND_ALBEDO * (sun * lights.sun_dir.z.max(0.0) / std::f32::consts::PI + 0.02);
        let mut rel = camera.view;
        rel.w_axis = Vec4::W;
        let params = Params {
            view_proj: (camera.projection * camera.view).to_cols_array_2d(),
            inv_view_proj: (camera.projection * rel).inverse().to_cols_array_2d(),
            ground: ground_colour.extend(1.0).to_array(),
            values: [EXPOSURE, HEIGHT as f32, 0.0, 0.0],
        };
        ctx.queue.write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));

        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ground"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &lit,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_pipeline(&ground);
            pass.set_bind_group(0, &params_group, &[]);
            pass.set_bind_group(1, sky.env_group(), &[]);
            pass.draw(0..6, 0..1);
        }
        sky.update(&ctx, &mut encoder, &lights, camera.eye);
        sky.draw(&ctx, &mut encoder, &sky_targets, &with_sky, &camera, &lights);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&present);
            pass.set_bind_group(0, &params_group, &[]);
            pass.set_bind_group(1, sky.env_group(), &[]);
            pass.draw(0..3, 0..1);
        }
        ctx.queue.submit([encoder.finish()]);

        let pixels = read_rgba8(&ctx, &output, WIDTH, HEIGHT * 3);
        let path = out.join(format!("sky_{name}.png"));
        image::save_buffer(&path, &pixels, WIDTH, HEIGHT * 3, image::ColorType::Rgba8)?;
        println!("{name}: sun light {sun}, saved {}", path.display());
    }
    Ok(())
}

fn read_rgba8(ctx: &GpuContext, texture: &wgpu::Texture, width: u32, height: u32) -> Vec<u8> {
    let row = (width * 4).div_ceil(256) * 256;
    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    ctx.queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
    let mapped = slice.get_mapped_range().expect("mapped readback buffer");
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height as usize {
        out.extend_from_slice(&mapped[y * row as usize..y * row as usize + width as usize * 4]);
    }
    out
}
