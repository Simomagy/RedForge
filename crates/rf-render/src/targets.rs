//! The textures a view renders into, resized with it: the G-buffer (as the C#
//! RedForge's `DeferredScene`), the ambient occlusion, the lit and finished
//! HDR images, the picking ids and the 8-bit output.

use std::sync::Mutex;
use std::time::Instant;

use crate::GpuContext;
use crate::post::{Post, PostTargets};
use crate::sky::{Sky, SkyTargets};

pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// What the final image is: 8-bit sRGB, which egui can show directly.
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Where the scene passes write each pixel's instance id and depth bits, for
/// picking (depth textures can't be read a pixel at a time).
pub(crate) const ID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Uint;
/// Albedo, normal (signed), and smoothness/metalness/emission; each alpha is
/// how much a decal covered. With the ids that's wgpu's 32 bytes a pixel.
pub(crate) const GBUFFER_FORMATS: [wgpu::TextureFormat; 3] =
    [wgpu::TextureFormat::Rgba8UnormSrgb, wgpu::TextureFormat::Rgba16Float, wgpu::TextureFormat::Rgba8Unorm];
pub(crate) const SSAO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

/// The layouts of the bind groups each view makes over its own textures.
pub(crate) struct ViewLayouts {
    /// The lighting pass: depth, the G-buffer, the occlusion.
    pub lighting: wgpu::BindGroupLayout,
    /// The background pass: depth and the lit image.
    pub background: wgpu::BindGroupLayout,
    /// The occlusion pass: depth, normals and its constants; then its blur.
    pub ssao: wgpu::BindGroupLayout,
    pub blur: wgpu::BindGroupLayout,
}

fn texture_entry(binding: u32, sample_type: wgpu::TextureSampleType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

impl ViewLayouts {
    pub fn new(device: &wgpu::Device) -> Self {
        let float = wgpu::TextureSampleType::Float { filterable: false };
        let depth = texture_entry(0, wgpu::TextureSampleType::Depth);
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries })
        };
        let uniform = wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        Self {
            lighting: layout(
                "lighting",
                &[
                    depth,
                    texture_entry(1, float),
                    texture_entry(2, float),
                    texture_entry(3, float),
                    texture_entry(5, float),
                ],
            ),
            background: layout("background", &[depth, texture_entry(6, float)]),
            ssao: layout("ssao", &[depth, texture_entry(1, float), uniform]),
            blur: layout("ssao blur", &[texture_entry(3, float)]),
        }
    }
}

/// The textures a view renders into, resized with it.
pub struct RenderTargets {
    pub width: u32,
    pub height: u32,
    pub(crate) gbuffer: [wgpu::TextureView; 3],
    pub(crate) depth_view: wgpu::TextureView,
    pub(crate) ids: wgpu::Texture,
    pub(crate) ids_view: wgpu::TextureView,
    pub(crate) ssao_raw: wgpu::TextureView,
    pub(crate) ssao: wgpu::TextureView,
    /// The G-buffer lit, before the background (or sky) and blended draws.
    pub(crate) lit: wgpu::TextureView,
    /// The finished HDR image.
    pub(crate) hdr: wgpu::TextureView,
    pub output: wgpu::Texture,
    pub output_view: wgpu::TextureView,
    /// The output seen as plain UNORM: the bytes as they are, which is what
    /// egui expects of a texture, and what the tonemap writes (as the C#'s
    /// UNORM backbuffer took its values).
    pub display_view: wgpu::TextureView,
    pub(crate) lighting_group: wgpu::BindGroup,
    pub(crate) background_group: wgpu::BindGroup,
    pub(crate) ssao_group: wgpu::BindGroup,
    pub(crate) blur_group: wgpu::BindGroup,
    /// The exposure's adaptation and bloom.
    pub(crate) post: PostTargets,
    /// The atmosphere's pass over the lit image.
    pub(crate) sky: SkyTargets,
    /// When this view was last drawn, for the adaptation.
    pub(crate) last_frame: Mutex<Option<Instant>>,
}

impl RenderTargets {
    pub(crate) fn new(
        ctx: &GpuContext,
        layouts: &ViewLayouts,
        ssao_constants: &wgpu::Buffer,
        post: &Post,
        sky: &Sky,
        width: u32,
        height: u32,
    ) -> Self {
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
        let sampled = rt | wgpu::TextureUsages::TEXTURE_BINDING;
        let view = |t: wgpu::Texture| t.create_view(&Default::default());
        let gbuffer: [wgpu::TextureView; 3] = ["albedo", "normal", "params"]
            .iter()
            .zip(GBUFFER_FORMATS)
            .map(|(label, format)| view(texture(label, format, sampled)))
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        let depth_view = view(texture("depth", DEPTH_FORMAT, sampled));
        let ids = texture("ids", ID_FORMAT, rt | wgpu::TextureUsages::COPY_SRC);
        let ids_view = ids.create_view(&Default::default());
        let ssao_raw = view(texture("ssao raw", SSAO_FORMAT, sampled));
        let ssao = view(texture("ssao", SSAO_FORMAT, sampled));
        let lit = view(texture("lit", HDR_FORMAT, sampled));
        let hdr = view(texture("hdr", HDR_FORMAT, sampled));
        let output = texture("output", OUTPUT_FORMAT, sampled | wgpu::TextureUsages::COPY_SRC);
        let output_view = output.create_view(&Default::default());
        let display_view = output.create_view(&wgpu::TextureViewDescriptor {
            format: Some(OUTPUT_FORMAT.remove_srgb_suffix()),
            ..Default::default()
        });

        let group = |label, layout, entries: &[(u32, wgpu::BindingResource)]| {
            let entries: Vec<_> = entries
                .iter()
                .map(|(binding, resource)| wgpu::BindGroupEntry { binding: *binding, resource: resource.clone() })
                .collect();
            ctx.device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some(label), layout, entries: &entries })
        };
        let tv = wgpu::BindingResource::TextureView;
        let lighting_group = group(
            "lighting",
            &layouts.lighting,
            &[(0, tv(&depth_view)), (1, tv(&gbuffer[0])), (2, tv(&gbuffer[1])), (3, tv(&gbuffer[2])), (5, tv(&ssao))],
        );
        let background_group = group("background", &layouts.background, &[(0, tv(&depth_view)), (6, tv(&lit))]);
        let ssao_group = group(
            "ssao",
            &layouts.ssao,
            &[(0, tv(&depth_view)), (1, tv(&gbuffer[1])), (2, ssao_constants.as_entire_binding())],
        );
        let blur_group = group("ssao blur", &layouts.blur, &[(3, tv(&ssao_raw))]);
        let post = post.targets(ctx, &hdr, width, height);
        let sky = sky.view_targets(ctx, &lit, &depth_view);
        Self {
            width,
            height,
            gbuffer,
            depth_view,
            ids,
            ids_view,
            ssao_raw,
            ssao,
            lit,
            hdr,
            output,
            output_view,
            display_view,
            lighting_group,
            background_group,
            ssao_group,
            blur_group,
            post,
            sky,
            last_frame: Mutex::new(None),
        }
    }
}
