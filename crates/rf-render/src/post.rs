//! The HDR post-processing, ported from the C#'s `HDRScene`: eye adaptation,
//! bloom, lens flares and the tonemap curve, from a view's HDR image to its
//! 8-bit output.
//!
//! [`Post::run`] records, per frame:
//! 1. `luminance` (compute, one workgroup): the sum of the log luminances on an
//!    81×81 grid of the image, blended into the view's adapted value.
//! 2. `bloom h`, `bloom v`: the bright pass on every 8th pixel, blurred by a
//!    15-tap Gaussian at 1/8 resolution.
//! 3. `final`: the bloom and flares added, the exposure, the scotopic shift and
//!    the curve.
//!
//! The curve's result is written as linear for the sRGB output to encode; the
//! C# wrote it to a UNORM backbuffer as it was, so its images are darker.

use std::sync::atomic::{AtomicBool, Ordering};

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::GpuContext;
use crate::renderer::OUTPUT_FORMAT;

const BLOOM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The adapted value before the first frame: the sum of the log luminances, so
/// an average of 1 (the C#'s `StartupLuminance`).
const STARTUP_LUMINANCE: f32 = 0.0;
/// The luminance is measured on a grid of this many samples a side.
const LUMINANCE_GRID: u32 = 81;
/// The adapted luminance's limits (night is kept darker by the scene's
/// exposure instead).
const MIN_LUMINANCE: f32 = 0.00001;
const MAX_LUMINANCE: f32 = 1000.0;
const BLOOM_AMOUNT: f32 = 0.2;
const BRIGHT_THRESHOLD: f32 = 0.8;
const FLARE_THRESHOLD: f32 = 5.0;
const FLARE_AMOUNT: f32 = 0.25;
const FLARE_COUNT: u32 = 7;
/// The exposed luminance where the scotopic shift is gone (about where
/// lamp-lit surfaces are).
const SCOTOPIC_LUMINANCE: f32 = 0.5;
/// The flares' hues and saturations (at full brightness), and how far from the
/// centre they mirror the bloom.
const FLARES: [(f32, f32, f32); 8] = [
    (0.0, 0.9, -0.3),
    (40.0, 0.85, -0.6),
    (100.0, 0.8, -1.5),
    (200.0, 0.75, -3.5),
    (220.0, 0.5, 5.5),
    (160.0, 0.4, 2.6),
    (70.0, 0.3, 1.5),
    (0.0, 0.2, 1.2),
];

/// The tonemap curves, in the C#'s order (the shader's index).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tonemap {
    /// The original RedForge one, from the old DirectX samples.
    ReinhardExt,
    Reinhard,
    /// Per-channel Reinhard blended with luminance Reinhard, which keeps hues.
    ReinhardJodie,
    /// Narkowicz's fit.
    Aces,
    /// Stephen Hill's fit of the RRT and ODT.
    AcesFitted,
    /// Hable's filmic curve.
    Uncharted2,
    /// Sobotka's, with Wrensch's polynomial fit.
    AgX,
    /// Timothy Lottes' filmic curve.
    Lottes,
    Linear,
    /// Hable's curve with the constants of RDR2's timecycles.
    #[default]
    Rdr2,
}

impl Tonemap {
    pub const ALL: [Tonemap; 10] = [
        Tonemap::ReinhardExt,
        Tonemap::Reinhard,
        Tonemap::ReinhardJodie,
        Tonemap::Aces,
        Tonemap::AcesFitted,
        Tonemap::Uncharted2,
        Tonemap::AgX,
        Tonemap::Lottes,
        Tonemap::Linear,
        Tonemap::Rdr2,
    ];

    /// The name in `RedForge.Settings.ini` (`Render.HDR.TonemapMode`).
    pub fn name(self) -> &'static str {
        match self {
            Tonemap::ReinhardExt => "Reinhard Extended",
            Tonemap::Reinhard => "Reinhard",
            Tonemap::ReinhardJodie => "Reinhard-Jodie",
            Tonemap::Aces => "ACES",
            Tonemap::AcesFitted => "ACES Fitted",
            Tonemap::Uncharted2 => "Uncharted 2",
            Tonemap::AgX => "AgX",
            Tonemap::Lottes => "Lottes",
            Tonemap::Linear => "Linear",
            Tonemap::Rdr2 => "RDR2",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.name().eq_ignore_ascii_case(name.trim()))
    }

    /// Where the curve wants the average luminance. Reinhard-style curves
    /// saturate slowly, so they are tuned high; filmic ones expect scene-linear
    /// values.
    fn middle_grey(self) -> f32 {
        match self {
            Tonemap::ReinhardExt | Tonemap::Reinhard => 0.72,
            Tonemap::ReinhardJodie => 0.5,
            Tonemap::Aces | Tonemap::Linear => 0.25,
            Tonemap::AcesFitted => 0.23,
            Tonemap::Uncharted2 => 0.42,
            Tonemap::AgX => 0.13,
            Tonemap::Lottes => 0.18,
            // (lands at about 0.24)
            Tonemap::Rdr2 => 0.45,
        }
    }
}

/// What [`Post::run`] does in a frame. The defaults are the C#'s.
#[derive(Clone, Copy, Debug)]
pub struct PostSettings {
    /// The user's exposure plus the scene's (the timecycle's), in EV.
    pub exposure_ev: f32,
    /// Night vision: rgb the tint (luminance 1), a how much (0 by day).
    pub scotopic: [f32; 4],
    pub bloom: bool,
    /// Lens flares, made from the bloom (so only with it).
    pub flares: bool,
    pub tonemap: Tonemap,
    /// For debug views: the values copied as they are, without exposure or
    /// curve.
    pub passthrough: bool,
    /// Seconds since this view's last frame, for the adaptation;
    /// `f32::INFINITY` snaps to the image's luminance.
    pub elapsed: f32,
    /// How fast the adaptation follows, per second
    /// (`Render.HDR.LumBlendSpeed`).
    pub adaptation_speed: f32,
}

impl Default for PostSettings {
    fn default() -> Self {
        Self {
            exposure_ev: 0.0,
            scotopic: [1.0, 1.0, 1.0, 0.0],
            bloom: true,
            flares: true,
            tonemap: Tonemap::Rdr2,
            passthrough: false,
            elapsed: f32::INFINITY,
            adaptation_speed: 2.0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    luminance: [f32; 4],
    exposure: [f32; 4],
    scotopic: [f32; 4],
    bloom: [f32; 4],
    modes: [u32; 4],
    flares: [[f32; 4]; 8],
}

/// The post-processing's pipelines, shared by every view.
pub struct Post {
    luminance_layout: wgpu::BindGroupLayout,
    bloom_layout: wgpu::BindGroupLayout,
    final_layout: wgpu::BindGroupLayout,
    luminance: wgpu::ComputePipeline,
    bloom_h: wgpu::RenderPipeline,
    bloom_v: wgpu::RenderPipeline,
    finish: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    /// rgb: colour, a: texcoord scale.
    flares: [[f32; 4]; 8],
}

/// A view's post-processing state: its bloom textures, its adapted luminance
/// and the bind groups to its HDR image.
pub struct PostTargets {
    pub width: u32,
    pub height: u32,
    params: wgpu::Buffer,
    /// The adapted sum of the log luminances, kept on resizes.
    adapted: wgpu::Buffer,
    /// The next run snaps: set until the first one, and by pass-through
    /// frames.
    snap: AtomicBool,
    bloom_h: wgpu::TextureView,
    bloom_v: wgpu::TextureView,
    luminance_group: wgpu::BindGroup,
    bloom_h_group: wgpu::BindGroup,
    bloom_v_group: wgpu::BindGroup,
    final_group: wgpu::BindGroup,
}

fn entry(binding: u32, visibility: wgpu::ShaderStages, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None }
}

const UNIFORM: wgpu::BindingType = wgpu::BindingType::Buffer {
    ty: wgpu::BufferBindingType::Uniform,
    has_dynamic_offset: false,
    min_binding_size: None,
};

fn texture(filterable: bool) -> wgpu::BindingType {
    wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Float { filterable },
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    }
}

fn storage(read_only: bool) -> wgpu::BindingType {
    wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Storage { read_only },
        has_dynamic_offset: false,
        min_binding_size: wgpu::BufferSize::new(4),
    }
}

/// The C#'s `ColourF.FromHSB`.
fn hsb(hue: f32, saturation: f32, brightness: f32) -> [f32; 3] {
    let (h, s, b) = (hue.clamp(0.0, 360.0), saturation.clamp(0.0, 1.0), brightness.clamp(0.0, 1.0));
    if s < 1e-6 {
        return [b; 3];
    }
    let sector = h / 60.0;
    let frac = sector - sector.floor();
    let p = b * (1.0 - s);
    let q = b * (1.0 - s * frac);
    let t = b * (1.0 - s * (1.0 - frac));
    match sector.floor() as i32 {
        1 => [q, b, p],
        2 => [p, b, t],
        3 => [p, q, b],
        4 => [t, p, b],
        5 => [b, p, q],
        _ => [b, t, p],
    }
}

impl Post {
    pub fn new(ctx: &GpuContext) -> Self {
        let device = &ctx.device;
        let compute = wgpu::ShaderStages::COMPUTE;
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let luminance_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post luminance"),
            entries: &[
                entry(0, compute, UNIFORM),
                entry(1, compute, texture(false)),
                entry(2, compute, storage(false)),
            ],
        });
        let bloom_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post bloom"),
            entries: &[entry(0, fragment, UNIFORM), entry(1, fragment, texture(false))],
        });
        let final_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post final"),
            entries: &[
                entry(0, fragment, UNIFORM),
                entry(1, fragment, texture(false)),
                entry(3, fragment, storage(true)),
                entry(4, fragment, texture(true)),
                entry(5, fragment, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/post.wgsl").into()),
        });
        let pipeline_layout = |label, layout| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            })
        };
        let luminance = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("post luminance"),
            layout: Some(&pipeline_layout("post luminance", &luminance_layout)),
            module: &shader,
            entry_point: Some("cs_luminance"),
            compilation_options: Default::default(),
            cache: None,
        });
        let fullscreen = |label, layout: &wgpu::PipelineLayout, entry_point, format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
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
                    entry_point: Some(entry_point),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let bloom_pl = pipeline_layout("post bloom", &bloom_layout);
        let bloom_h = fullscreen("post bloom h", &bloom_pl, "fs_bloom_h", BLOOM_FORMAT);
        let bloom_v = fullscreen("post bloom v", &bloom_pl, "fs_bloom_v", BLOOM_FORMAT);
        let finish = fullscreen("post final", &pipeline_layout("post final", &final_layout), "fs_final", OUTPUT_FORMAT);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let flares = FLARES.map(|(h, s, scale)| {
            let [r, g, b] = hsb(h, s, 1.0);
            [r, g, b, scale]
        });
        Self { luminance_layout, bloom_layout, final_layout, luminance, bloom_h, bloom_v, finish, sampler, flares }
    }

    /// A view's targets for its HDR image (`width` × `height`, the size of the
    /// output too), adapted to the startup luminance until the first
    /// [`run`](Self::run) snaps.
    pub fn targets(&self, ctx: &GpuContext, hdr: &wgpu::TextureView, width: u32, height: u32) -> PostTargets {
        let adapted = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("post adapted luminance"),
            contents: bytemuck::bytes_of(&STARTUP_LUMINANCE),
            usage: wgpu::BufferUsages::STORAGE,
        });
        self.build(ctx, hdr, width, height, adapted, true)
    }

    /// `targets` for a resized HDR image, keeping the adapted luminance.
    pub fn resize(
        &self,
        ctx: &GpuContext,
        targets: &mut PostTargets,
        hdr: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) {
        let snap = targets.snap.load(Ordering::Relaxed);
        *targets = self.build(ctx, hdr, width, height, targets.adapted.clone(), snap);
    }

    fn build(
        &self,
        ctx: &GpuContext,
        hdr: &wgpu::TextureView,
        width: u32,
        height: u32,
        adapted: wgpu::Buffer,
        snap: bool,
    ) -> PostTargets {
        let device = &ctx.device;
        let (width, height) = (width.max(1), height.max(1));
        let bloom = |label| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: (width / 8).max(1),
                        height: (height / 8).max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: BLOOM_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let bloom_h = bloom("post bloom h");
        let bloom_v = bloom("post bloom v");
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post"),
            size: size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = |label, layout, entries: &[(u32, wgpu::BindingResource)]| {
            let entries: Vec<_> = entries
                .iter()
                .map(|(binding, resource)| wgpu::BindGroupEntry { binding: *binding, resource: resource.clone() })
                .collect();
            device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some(label), layout, entries: &entries })
        };
        let uniform = params.as_entire_binding();
        let view = wgpu::BindingResource::TextureView;
        let luminance_group = group(
            "post luminance",
            &self.luminance_layout,
            &[(0, uniform.clone()), (1, view(hdr)), (2, adapted.as_entire_binding())],
        );
        let bloom_h_group = group("post bloom h", &self.bloom_layout, &[(0, uniform.clone()), (1, view(hdr))]);
        let bloom_v_group = group("post bloom v", &self.bloom_layout, &[(0, uniform.clone()), (1, view(&bloom_h))]);
        let final_group = group(
            "post final",
            &self.final_layout,
            &[
                (0, uniform),
                (1, view(hdr)),
                (3, adapted.as_entire_binding()),
                (4, view(&bloom_v)),
                (5, wgpu::BindingResource::Sampler(&self.sampler)),
            ],
        );
        PostTargets {
            width,
            height,
            params,
            adapted,
            snap: AtomicBool::new(snap),
            bloom_h,
            bloom_v,
            luminance_group,
            bloom_h_group,
            bloom_v_group,
            final_group,
        }
    }

    /// Records the post-processing of `targets`' HDR image into `output` (an
    /// [`OUTPUT_FORMAT`] view of the same size).
    pub fn run(
        &self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        targets: &PostTargets,
        output: &wgpu::TextureView,
        settings: &PostSettings,
    ) {
        // Pass-through frames don't measure, so the next lit one snaps.
        let snap = if settings.passthrough {
            targets.snap.store(true, Ordering::Relaxed);
            false
        } else {
            targets.snap.swap(false, Ordering::Relaxed)
        };
        let rate = settings.elapsed * settings.adaptation_speed;
        let blend = if snap || rate.is_nan() { 1.0 } else { rate.clamp(0.0, 1.0) };
        let bloom = settings.bloom && !settings.passthrough;
        let params = Params {
            luminance: [blend, 1.0 / (LUMINANCE_GRID * LUMINANCE_GRID) as f32, MIN_LUMINANCE, MAX_LUMINANCE],
            exposure: [settings.exposure_ev.exp2(), settings.tonemap.middle_grey(), SCOTOPIC_LUMINANCE, 0.0],
            scotopic: settings.scotopic,
            bloom: [BRIGHT_THRESHOLD, if bloom { BLOOM_AMOUNT } else { 0.0 }, FLARE_THRESHOLD, FLARE_AMOUNT],
            modes: [
                settings.tonemap as u32,
                if bloom && settings.flares { FLARE_COUNT } else { 0 },
                settings.passthrough as u32,
                0,
            ],
            flares: self.flares,
        };
        ctx.queue.write_buffer(&targets.params, 0, bytemuck::bytes_of(&params));

        if !settings.passthrough {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("post luminance"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.luminance);
            pass.set_bind_group(0, &targets.luminance_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        if bloom {
            fullscreen_pass(encoder, "post bloom h", &targets.bloom_h, &self.bloom_h, &targets.bloom_h_group);
            fullscreen_pass(encoder, "post bloom v", &targets.bloom_v, &self.bloom_v, &targets.bloom_v_group);
        }
        fullscreen_pass(encoder, "post final", output, &self.finish, &targets.final_group);
    }
}

fn fullscreen_pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    group: &wgpu::BindGroup,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
        })],
        ..Default::default()
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[]);
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tonemap_names_round_trip() {
        for t in Tonemap::ALL {
            assert_eq!(Tonemap::from_name(t.name()), Some(t));
        }
        assert_eq!(Tonemap::from_name("rdr2"), Some(Tonemap::Rdr2));
        assert_eq!(Tonemap::from_name("Filmic"), None);
    }

    #[test]
    fn hsb_is_the_csharps() {
        let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-6);
        assert!(near(hsb(0.0, 0.9, 1.0), [1.0, 0.1, 0.1]));
        assert!(near(hsb(200.0, 0.75, 1.0), [0.25, 0.75, 1.0]));
        assert!(near(hsb(100.0, 0.8, 1.0), [0.46666667, 1.0, 0.2]));
        assert!(near(hsb(70.0, 0.0, 0.5), [0.5; 3]));
    }
}
