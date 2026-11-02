//! Game textures on the GPU.

use rf_formats::formats::dds::{TextureFormat as Tf, compute_pitch};
use rf_formats::formats::ytd::{TextureBase, dimension};

use crate::{GpuContext, RenderError, Result};

/// How a texture's layers are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureKind {
    D2,
    /// `layers` 2D textures, e.g. a volume texture's slices.
    Array,
    Cube,
}

pub struct GpuTexture {
    pub name: String,
    pub texture: wgpu::Texture,
    /// The whole texture, as a 2D, 2D array or cube view.
    pub view: wgpu::TextureView,
    /// Layer 0 as a plain 2D view, for materials.
    pub view_2d: wgpu::TextureView,
    /// Every layer as a 2D array, with sRGB formats seen as plain UNORM so the
    /// stored values come back unchanged (for previews).
    pub view_raw: wgpu::TextureView,
    pub kind: TextureKind,
    pub width: u32,
    pub height: u32,
    pub layers: u32,
    pub mips: u32,
    /// The format as stored in the game file.
    pub source_format: Tf,
    pub gpu_format: wgpu::TextureFormat,
}

/// What the pixels go to the GPU as, and how to convert them if needed.
enum Upload {
    Direct(wgpu::TextureFormat),
    /// Converted to RGBA8 by the function, per mip.
    ToRgba8(fn(&[u8]) -> Vec<u8>),
    /// Converted to RGBA16F / R16F.
    ToHalf(wgpu::TextureFormat, fn(&[u8]) -> Vec<u8>),
}

fn f32_to_f16_bits(v: f32) -> u16 {
    rf_formats::formats::ydr::f32_to_half(v)
}

fn a8_to_rgba8(src: &[u8]) -> Vec<u8> {
    src.iter().flat_map(|&a| [255, 255, 255, a]).collect()
}

fn bgr5a1_to_rgba8(src: &[u8]) -> Vec<u8> {
    src.as_chunks::<2>()
        .0
        .iter()
        .flat_map(|c| {
            let v = u16::from_le_bytes([c[0], c[1]]);
            let ch = |s: u16| (((v >> s) & 31) as u32 * 255 / 31) as u8;
            [ch(10), ch(5), ch(0), if v & 0x8000 != 0 { 255 } else { 0 }]
        })
        .collect()
}

fn rgba16u_to_rgba8(src: &[u8]) -> Vec<u8> {
    src.as_chunks::<2>().0.iter().map(|c| c[1]).collect()
}

fn r16u_to_rgba8(src: &[u8]) -> Vec<u8> {
    src.as_chunks::<2>().0.iter().flat_map(|c| [c[1], c[1], c[1], 255]).collect()
}

fn f32s_to_f16s(src: &[u8]) -> Vec<u8> {
    src.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|c| f32_to_f16_bits(f32::from_le_bytes([c[0], c[1], c[2], c[3]])).to_le_bytes())
        .collect()
}

fn r32ui_to_rgba8(src: &[u8]) -> Vec<u8> {
    src.as_chunks::<4>().0.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect()
}

fn upload_for(f: Tf, ctx: &GpuContext) -> Option<Upload> {
    use wgpu::TextureFormat as W;
    let bc = ctx.has_bc();
    Some(match f {
        Tf::BC1 if bc => Upload::Direct(W::Bc1RgbaUnorm),
        Tf::BC1SRGB if bc => Upload::Direct(W::Bc1RgbaUnormSrgb),
        Tf::BC2 if bc => Upload::Direct(W::Bc2RgbaUnorm),
        Tf::BC2SRGB if bc => Upload::Direct(W::Bc2RgbaUnormSrgb),
        Tf::BC3 if bc => Upload::Direct(W::Bc3RgbaUnorm),
        Tf::BC3SRGB if bc => Upload::Direct(W::Bc3RgbaUnormSrgb),
        Tf::BC4 if bc => Upload::Direct(W::Bc4RUnorm),
        Tf::BC5 if bc => Upload::Direct(W::Bc5RgUnorm),
        Tf::BC6H if bc => Upload::Direct(W::Bc6hRgbUfloat),
        Tf::BC7 if bc => Upload::Direct(W::Bc7RgbaUnorm),
        Tf::BC7SRGB if bc => Upload::Direct(W::Bc7RgbaUnormSrgb),
        Tf::A8B8G8R8 => Upload::Direct(W::Rgba8Unorm),
        Tf::A8B8G8R8SRGB => Upload::Direct(W::Rgba8UnormSrgb),
        Tf::A8R8G8B8 => Upload::Direct(W::Bgra8Unorm),
        Tf::A8R8G8B8SRGB => Upload::Direct(W::Bgra8UnormSrgb),
        Tf::L8 => Upload::Direct(W::R8Unorm),
        Tf::R8G8U => Upload::Direct(W::Rg8Unorm),
        Tf::A8 => Upload::ToRgba8(a8_to_rgba8),
        Tf::A1R5G5B5 => Upload::ToRgba8(bgr5a1_to_rgba8),
        Tf::R11G11B10F => Upload::Direct(W::Rg11b10Ufloat),
        Tf::R10G10B10A2 => Upload::Direct(W::Rgb10a2Unorm),
        Tf::R16G16B16A16F => Upload::Direct(W::Rgba16Float),
        Tf::R16G16B16A16U if ctx.has_norm16() => Upload::Direct(W::Rgba16Unorm),
        Tf::R16G16B16A16U => Upload::ToRgba8(rgba16u_to_rgba8),
        Tf::R16F => Upload::Direct(W::R16Float),
        Tf::R16U if ctx.has_norm16() => Upload::Direct(W::R16Unorm),
        Tf::R16U => Upload::ToRgba8(r16u_to_rgba8),
        // 32-bit floats can't be filtered without an extra feature.
        Tf::R32G32B32A32F => Upload::ToHalf(W::Rgba16Float, f32s_to_f16s),
        Tf::R32F => Upload::ToHalf(W::R16Float, f32s_to_f16s),
        Tf::R32UI => Upload::ToRgba8(r32ui_to_rgba8),
        _ => return None,
    })
}

impl GpuTexture {
    /// Uploads a game texture with all its mips and layers.
    pub fn upload(ctx: &GpuContext, t: &TextureBase) -> Result<Self> {
        let name = t.name.clone().unwrap_or_default();
        let Some(data) = t.data.as_deref() else {
            return Err(RenderError::Texture(format!("{name}: no pixel data")));
        };
        let Some(upload) = upload_for(t.format, ctx) else {
            return Err(RenderError::Texture(format!("{name}: format {:?} isn't supported", t.format)));
        };
        let (width, height) = (t.width.max(1) as u32, t.height.max(1) as u32);
        let layers =
            (t.depth.max(1) as u32) * if t.dimension == dimension::TEXTURE_CUBE && t.depth == 1 { 6 } else { 1 };
        let kind = match (t.dimension, layers) {
            (dimension::TEXTURE_CUBE, 6) => TextureKind::Cube,
            (_, 1) => TextureKind::D2,
            _ => TextureKind::Array,
        };
        let gpu_format = match &upload {
            Upload::Direct(f) | Upload::ToHalf(f, _) => *f,
            Upload::ToRgba8(_) => wgpu::TextureFormat::Rgba8Unorm,
        };
        let (bw, bh) = gpu_format.block_dimensions();
        // Compressed textures must be a whole number of blocks.
        let (pw, ph) = (width.div_ceil(bw) * bw, height.div_ceil(bh) * bh);
        let max_mips = 32 - pw.max(ph).leading_zeros();
        let mut mips = (t.mip_levels.max(1) as u32).min(max_mips);
        // The C# sizes mips as `Width / div`; stop before an uncompressed one is empty.
        if bw == 1 {
            mips = mips.min(32 - width.min(height).leading_zeros());
        }

        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&name),
            size: wgpu::Extent3d { width: pw, height: ph, depth_or_array_layers: layers },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: gpu_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[gpu_format.remove_srgb_suffix()],
        });

        // The data is each layer, each mip; sizes as the game computes them.
        let dxgi = t.format.dxgi();
        let mut offset = 0usize;
        for layer in 0..layers {
            for mip in 0..(t.mip_levels.max(1) as u32) {
                let (mw, mh) = ((width >> mip).max(1), (height >> mip).max(1));
                let (row, slice) = compute_pitch(dxgi, mw as i32, mh as i32)
                    .map_err(|e| RenderError::Texture(format!("{name}: {e}")))?;
                let (row, slice) = (row as usize, slice as usize);
                let end = offset + slice;
                if mip < mips {
                    let src = data.get(offset..end.min(data.len())).unwrap_or(&[]);
                    let mut src = std::borrow::Cow::Borrowed(src);
                    if src.len() < slice {
                        let mut padded = src.into_owned();
                        padded.resize(slice, 0);
                        src = padded.into();
                    }
                    let (bytes, bytes_per_row) = match &upload {
                        Upload::Direct(_) => (src, row),
                        Upload::ToRgba8(conv) => (conv(&src).into(), mw as usize * 4),
                        Upload::ToHalf(f, conv) => {
                            let bpp = if *f == wgpu::TextureFormat::Rgba16Float { 8 } else { 2 };
                            (conv(&src).into(), mw as usize * bpp)
                        }
                    };
                    let rows = (mh.div_ceil(bh)) as usize;
                    let (cw, ch) = ((mw.div_ceil(bw) * bw), (mh.div_ceil(bh) * bh));
                    // The physical size of the mip in the padded texture: whole
                    // blocks (a padded size halved needn't be).
                    let (gw, gh) = ((pw >> mip).max(1), (ph >> mip).max(1));
                    let size = wgpu::Extent3d {
                        width: cw.min(gw.div_ceil(bw) * bw),
                        height: ch.min(gh.div_ceil(bh) * bh),
                        depth_or_array_layers: 1,
                    };
                    if bytes.len() >= bytes_per_row * rows {
                        ctx.queue.write_texture(
                            wgpu::TexelCopyTextureInfo {
                                texture: &texture,
                                mip_level: mip,
                                origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
                                aspect: wgpu::TextureAspect::All,
                            },
                            &bytes[..bytes_per_row * rows],
                            wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(bytes_per_row as u32),
                                rows_per_image: Some(rows as u32),
                            },
                            size,
                        );
                    }
                }
                offset = end;
            }
        }

        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(match kind {
                TextureKind::D2 => wgpu::TextureViewDimension::D2,
                TextureKind::Array => wgpu::TextureViewDimension::D2Array,
                TextureKind::Cube => wgpu::TextureViewDimension::Cube,
            }),
            ..Default::default()
        });
        let view_2d = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2),
            array_layer_count: Some(1),
            ..Default::default()
        });
        let view_raw = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(gpu_format.remove_srgb_suffix()),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        Ok(Self {
            name,
            texture,
            view,
            view_2d,
            view_raw,
            kind,
            width,
            height,
            layers,
            mips,
            source_format: t.format,
            gpu_format,
        })
    }

    /// A 1x1 texture of one colour (RGBA8, linear).
    pub fn solid(ctx: &GpuContext, name: &str, rgba: [u8; 4]) -> Self {
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(name),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        ctx.queue.write_texture(
            texture.as_image_copy(),
            &rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
        let view = texture.create_view(&Default::default());
        let view_2d = texture.create_view(&Default::default());
        let view_raw = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        Self {
            name: name.to_owned(),
            texture,
            view,
            view_2d,
            view_raw,
            kind: TextureKind::D2,
            width: 1,
            height: 1,
            layers: 1,
            mips: 1,
            source_format: Tf::A8B8G8R8,
            gpu_format: wgpu::TextureFormat::Rgba8Unorm,
        }
    }
}
