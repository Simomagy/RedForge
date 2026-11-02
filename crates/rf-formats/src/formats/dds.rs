//! DDS files, as RedForge's C# `DDSIO.cs` (a port of DirectXTex's DDS code)
//! reads and writes them for the textures it puts next to the XML.
//!
//! - [`write_dds`] is `GetDDSFile`: always a 2D texture, with the depth (array
//!   size) and mips as stored. Cube maps and volumes are written as plain 2D
//!   arrays, with the `DX10` header whenever the depth is more than 1.
//! - [`read_dds`] is `GetTexture`: the format, size, depth and mip count from the
//!   header, and everything after it as the pixel data.
//!
//! The DirectXTex helpers ([`compute_pitch`], [`bits_per_pixel`], ...) take a
//! DXGI format number (see [`dxgi`]).

use crate::{Error, Result};

/// `DXGI_FORMAT` values, as numbers (the C# enum, with the Xbox and Windows 10
/// extensions DirectXTex knows).
pub mod dxgi {
    pub const UNKNOWN: u32 = 0;
    pub const R32G32B32A32_TYPELESS: u32 = 1;
    pub const R32G32B32A32_FLOAT: u32 = 2;
    pub const R32G32B32A32_UINT: u32 = 3;
    pub const R32G32B32A32_SINT: u32 = 4;
    pub const R32G32B32_TYPELESS: u32 = 5;
    pub const R32G32B32_SINT: u32 = 8;
    pub const R16G16B16A16_TYPELESS: u32 = 9;
    pub const R16G16B16A16_FLOAT: u32 = 10;
    pub const R16G16B16A16_UNORM: u32 = 11;
    pub const R16G16B16A16_SNORM: u32 = 13;
    pub const R32G32_FLOAT: u32 = 16;
    pub const X32_TYPELESS_G8X24_UINT: u32 = 22;
    pub const R10G10B10A2_TYPELESS: u32 = 23;
    pub const R10G10B10A2_UNORM: u32 = 24;
    pub const R11G11B10_FLOAT: u32 = 26;
    pub const R8G8B8A8_UNORM: u32 = 28;
    pub const R8G8B8A8_UNORM_SRGB: u32 = 29;
    pub const R16G16_FLOAT: u32 = 34;
    pub const R16G16_UNORM: u32 = 35;
    pub const R32_FLOAT: u32 = 41;
    pub const R32_UINT: u32 = 42;
    pub const X24_TYPELESS_G8_UINT: u32 = 47;
    pub const R8G8_TYPELESS: u32 = 48;
    pub const R8G8_UNORM: u32 = 49;
    pub const R16_FLOAT: u32 = 54;
    pub const R16_UNORM: u32 = 56;
    pub const R16_SINT: u32 = 59;
    pub const R8_TYPELESS: u32 = 60;
    pub const R8_UNORM: u32 = 61;
    pub const A8_UNORM: u32 = 65;
    pub const R1_UNORM: u32 = 66;
    pub const R9G9B9E5_SHAREDEXP: u32 = 67;
    pub const R8G8_B8G8_UNORM: u32 = 68;
    pub const G8R8_G8B8_UNORM: u32 = 69;
    pub const BC1_TYPELESS: u32 = 70;
    pub const BC1_UNORM: u32 = 71;
    pub const BC1_UNORM_SRGB: u32 = 72;
    pub const BC2_TYPELESS: u32 = 73;
    pub const BC2_UNORM: u32 = 74;
    pub const BC2_UNORM_SRGB: u32 = 75;
    pub const BC3_TYPELESS: u32 = 76;
    pub const BC3_UNORM: u32 = 77;
    pub const BC3_UNORM_SRGB: u32 = 78;
    pub const BC4_TYPELESS: u32 = 79;
    pub const BC4_UNORM: u32 = 80;
    pub const BC4_SNORM: u32 = 81;
    pub const BC5_TYPELESS: u32 = 82;
    pub const BC5_UNORM: u32 = 83;
    pub const BC5_SNORM: u32 = 84;
    pub const B5G6R5_UNORM: u32 = 85;
    pub const B5G5R5A1_UNORM: u32 = 86;
    pub const B8G8R8A8_UNORM: u32 = 87;
    pub const B8G8R8X8_UNORM: u32 = 88;
    pub const B8G8R8A8_UNORM_SRGB: u32 = 91;
    pub const B8G8R8X8_UNORM_SRGB: u32 = 93;
    pub const BC6H_TYPELESS: u32 = 94;
    pub const BC6H_UF16: u32 = 95;
    pub const BC7_UNORM: u32 = 98;
    pub const BC7_UNORM_SRGB: u32 = 99;
    pub const AYUV: u32 = 100;
    pub const Y410: u32 = 101;
    pub const Y416: u32 = 102;
    pub const NV12: u32 = 103;
    pub const P010: u32 = 104;
    pub const P016: u32 = 105;
    pub const OPAQUE_420: u32 = 106;
    pub const YUY2: u32 = 107;
    pub const Y210: u32 = 108;
    pub const Y216: u32 = 109;
    pub const NV11: u32 = 110;
    pub const AI44: u32 = 111;
    pub const IA44: u32 = 112;
    pub const P8: u32 = 113;
    pub const A8P8: u32 = 114;
    pub const B4G4R4A4_UNORM: u32 = 115;
    pub const XBOX_R10G10B10_7E3_A2_FLOAT: u32 = 116;
    pub const XBOX_R10G10B10_6E4_A2_FLOAT: u32 = 117;
    pub const XBOX_D16_UNORM_S8_UINT: u32 = 118;
    pub const XBOX_R16_UNORM_X8_TYPELESS: u32 = 119;
    pub const XBOX_X16_TYPELESS_G8_UINT: u32 = 120;
    pub const WIN10_P208: u32 = 130;
    pub const WIN10_V208: u32 = 131;
    pub const WIN10_V408: u32 = 132;
    pub const XBOX_R10G10B10_SNORM_A2_UNORM: u32 = 189;
    pub const XBOX_R4G4_UNORM: u32 = 190;
}

use dxgi::*;

/// The engine's texture formats (`RedForge.Core.Engine.TextureFormat`); each
/// value is the DXGI format it stands for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum TextureFormat {
    #[default]
    Unknown = 0,
    BC1 = 71,
    BC2 = 74,
    BC3 = 77,
    BC4 = 80,
    BC5 = 83,
    BC6H = 95,
    BC7 = 98,
    BC1SRGB = 72,
    BC2SRGB = 75,
    BC3SRGB = 78,
    BC7SRGB = 99,
    L8 = 61,
    A8 = 65,
    R8G8U = 49,
    A1R5G5B5 = 86,
    A8R8G8B8 = 87,
    A8B8G8R8 = 28,
    A8R8G8B8SRGB = 91,
    A8B8G8R8SRGB = 29,
    R11G11B10F = 26,
    R10G10B10A2 = 24,
    R16G16B16A16F = 10,
    R16G16B16A16U = 11,
    R16F = 54,
    R16U = 56,
    R32G32B32A32F = 2,
    R32F = 41,
    R32UI = 42,
}

impl TextureFormat {
    /// `DDSIO.GetDXGIFormat`.
    pub fn dxgi(self) -> u32 {
        self as u32
    }

    /// `DDSIO.GetTextureFormat`: `Unknown` for the DXGI formats the engine
    /// doesn't use.
    pub fn from_dxgi(f: u32) -> Self {
        use TextureFormat::*;
        match f {
            BC1_UNORM => BC1,
            BC2_UNORM => BC2,
            BC3_UNORM => BC3,
            BC4_UNORM => BC4,
            BC5_UNORM => BC5,
            BC6H_UF16 => BC6H,
            BC7_UNORM => BC7,
            BC1_UNORM_SRGB => BC1SRGB,
            BC2_UNORM_SRGB => BC2SRGB,
            BC3_UNORM_SRGB => BC3SRGB,
            BC7_UNORM_SRGB => BC7SRGB,
            B5G5R5A1_UNORM => A1R5G5B5,
            A8_UNORM => A8,
            R8G8B8A8_UNORM => A8B8G8R8,
            R8_UNORM => L8,
            B8G8R8A8_UNORM | B8G8R8X8_UNORM => A8R8G8B8,
            R8G8_UNORM => R8G8U,
            B8G8R8A8_UNORM_SRGB => A8R8G8B8SRGB,
            R8G8B8A8_UNORM_SRGB => A8B8G8R8SRGB,
            R11G11B10_FLOAT => R11G11B10F,
            R10G10B10A2_UNORM => R10G10B10A2,
            R16G16B16A16_FLOAT => R16G16B16A16F,
            R16G16B16A16_UNORM => R16G16B16A16U,
            R16_FLOAT => R16F,
            R16_UNORM => R16U,
            R32G32B32A32_FLOAT => R32G32B32A32F,
            R32_FLOAT => R32F,
            R32_UINT => R32UI,
            _ => Unknown,
        }
    }
}

/// The C#'s `DXTex.assert`, which throws an exception without a message.
fn assert(b: bool) -> Result<()> {
    if b { Ok(()) } else { Err(Error::Format("DDS: assertion failed (invalid format)".into())) }
}

pub fn is_valid(fmt: u32) -> bool {
    (1..=190).contains(&fmt)
}

pub fn is_compressed(fmt: u32) -> bool {
    matches!(fmt, BC1_TYPELESS..=BC5_SNORM | BC6H_TYPELESS..=BC7_UNORM_SRGB)
}

pub fn is_planar(fmt: u32) -> bool {
    matches!(
        fmt,
        NV12 | P010
            | P016
            | OPAQUE_420
            | NV11
            | WIN10_P208
            | WIN10_V208
            | WIN10_V408
            | XBOX_D16_UNORM_S8_UINT
            | XBOX_R16_UNORM_X8_TYPELESS
            | XBOX_X16_TYPELESS_G8_UINT
    )
}

pub fn is_packed(fmt: u32) -> bool {
    matches!(fmt, R8G8_B8G8_UNORM | G8R8_G8B8_UNORM | YUY2 | Y210 | Y216)
}

pub fn is_palettized(fmt: u32) -> bool {
    matches!(fmt, AI44 | IA44 | P8 | A8P8)
}

/// `DXTex.BitsPerPixel` (0 for formats it doesn't know).
pub fn bits_per_pixel(fmt: u32) -> i32 {
    match fmt {
        R32G32B32A32_TYPELESS..=R32G32B32A32_SINT => 128,
        R32G32B32_TYPELESS..=R32G32B32_SINT => 96,
        R16G16B16A16_TYPELESS..=X32_TYPELESS_G8X24_UINT | Y416 | Y210 | Y216 => 64,
        R10G10B10A2_TYPELESS..=X24_TYPELESS_G8_UINT
        | R9G9B9E5_SHAREDEXP
        | R8G8_B8G8_UNORM
        | G8R8_G8B8_UNORM
        | B8G8R8A8_UNORM..=B8G8R8X8_UNORM_SRGB
        | AYUV
        | Y410
        | YUY2
        | XBOX_R10G10B10_7E3_A2_FLOAT
        | XBOX_R10G10B10_6E4_A2_FLOAT
        | XBOX_R10G10B10_SNORM_A2_UNORM => 32,
        P010 | P016 | XBOX_D16_UNORM_S8_UINT | XBOX_R16_UNORM_X8_TYPELESS | XBOX_X16_TYPELESS_G8_UINT | WIN10_V408 => {
            24
        }
        R8G8_TYPELESS..=R16_SINT | B5G6R5_UNORM | B5G5R5A1_UNORM | A8P8 | B4G4R4A4_UNORM | WIN10_P208 | WIN10_V208 => {
            16
        }
        NV12 | OPAQUE_420 | NV11 => 12,
        R8_TYPELESS..=A8_UNORM | AI44 | IA44 | P8 | XBOX_R4G4_UNORM => 8,
        R1_UNORM => 1,
        BC1_TYPELESS..=BC1_UNORM_SRGB | BC4_TYPELESS..=BC4_SNORM => 4,
        BC2_TYPELESS..=BC3_UNORM_SRGB | BC5_TYPELESS..=BC5_SNORM | BC6H_TYPELESS..=BC7_UNORM_SRGB => 8,
        _ => 0,
    }
}

/// `DXTex.ComputePitch` with no flags (byte-aligned rows): the row pitch and
/// slice pitch of one image. Fails, like the C#'s assertions, for an invalid
/// format.
pub fn compute_pitch(fmt: u32, width: i32, height: i32) -> Result<(i32, i32)> {
    let blocks = |n: i32| ((n + 3) / 4).max(1);
    Ok(match fmt {
        BC1_TYPELESS..=BC1_UNORM_SRGB | BC4_TYPELESS..=BC4_SNORM => {
            let row = blocks(width) * 8;
            (row, row * blocks(height))
        }
        BC2_TYPELESS..=BC3_UNORM_SRGB | BC5_TYPELESS..=BC5_SNORM | BC6H_TYPELESS..=BC7_UNORM_SRGB => {
            let row = blocks(width) * 16;
            (row, row * blocks(height))
        }
        R8G8_B8G8_UNORM | G8R8_G8B8_UNORM | YUY2 => {
            let row = ((width + 1) >> 1) * 4;
            (row, row * height)
        }
        Y210 | Y216 => {
            let row = ((width + 1) >> 1) * 8;
            (row, row * height)
        }
        NV12 | OPAQUE_420 => {
            let row = ((width + 1) >> 1) * 2;
            (row, row * (height + ((height + 1) >> 1)))
        }
        P010 | P016 | XBOX_D16_UNORM_S8_UINT | XBOX_R16_UNORM_X8_TYPELESS | XBOX_X16_TYPELESS_G8_UINT => {
            let row = ((width + 1) >> 1) * 4;
            (row, row * (height + ((height + 1) >> 1)))
        }
        NV11 => {
            let row = ((width + 3) >> 2) * 4;
            (row, row * height * 2)
        }
        WIN10_P208 => {
            let row = ((width + 1) >> 1) * 2;
            (row, row * height * 2)
        }
        WIN10_V208 => (width, width * (height + (((height + 1) >> 1) * 2))),
        WIN10_V408 => (width, width * (height + ((height >> 1) * 4))),
        _ => {
            assert(is_valid(fmt))?;
            assert(!is_compressed(fmt) && !is_packed(fmt) && !is_planar(fmt))?;
            let row = (width * bits_per_pixel(fmt) + 7) / 8;
            (row, row * height)
        }
    })
}

/// `DXTex.ComputeScanlines`: the rows of data in an image `height` pixels high.
pub fn compute_scanlines(fmt: u32, height: i32) -> Result<i32> {
    Ok(match fmt {
        BC1_TYPELESS..=BC5_SNORM | BC6H_TYPELESS..=BC7_UNORM_SRGB => ((height + 3) / 4).max(1),
        NV11 | WIN10_P208 => height * 2,
        WIN10_V208 => height + (((height + 1) >> 1) * 2),
        WIN10_V408 => height + ((height >> 1) * 4),
        NV12
        | P010
        | P016
        | OPAQUE_420
        | XBOX_D16_UNORM_S8_UINT
        | XBOX_R16_UNORM_X8_TYPELESS
        | XBOX_X16_TYPELESS_G8_UINT => height + ((height + 1) >> 1),
        _ => {
            assert(is_valid(fmt))?;
            assert(!is_compressed(fmt) && !is_planar(fmt))?;
            height
        }
    })
}

/// A dimension at a mip level, as the C# computes it: `size / div` with an
/// `int` divisor doubled each level, which overflows to 0 past level 31.
pub fn mip_size(size: u16, level: usize) -> Result<i32> {
    let div = if level < 32 { 1i32.wrapping_shl(level as u32) } else { 0 };
    if div == 0 {
        return Err(Error::Format("DDS: division by zero computing a mip size".into()));
    }
    Ok(size as i32 / div)
}

pub const DDS_MAGIC: u32 = 0x2053_4444; // "DDS "

const DDS_HEADER_FLAGS_TEXTURE: u32 = 0x0000_1007;
const DDS_HEADER_FLAGS_MIPMAP: u32 = 0x0002_0000;
const DDS_HEADER_FLAGS_VOLUME: u32 = 0x0080_0000;
const DDS_HEADER_FLAGS_PITCH: u32 = 0x0000_0008;
const DDS_HEADER_FLAGS_LINEARSIZE: u32 = 0x0008_0000;
const DDS_HEIGHT: u32 = 0x0000_0002;
const DDS_SURFACE_FLAGS_TEXTURE: u32 = 0x0000_1000;
const DDS_SURFACE_FLAGS_MIPMAP: u32 = 0x0040_0008;
const DDS_SURFACE_FLAGS_CUBEMAP: u32 = 0x0000_0008;
const DDS_CUBEMAP_ALLFACES: u32 = 0x0000_FE00;
const DDS_CUBEMAP: u32 = 0x0000_0200;
const DDS_FLAGS_VOLUME: u32 = 0x0020_0000;
const DDS_FLAGS_FORCE_DX10_EXT: u32 = 0x10000;
const DDS_FLAGS_FORCE_DX10_EXT_MISC2: u32 = 0x20000;
const TEX_MISC_TEXTURECUBE: u32 = 0x4;
const TEX_MISC2_ALPHA_MODE_MASK: u32 = 0x7;
const TEX_ALPHA_MODE_PREMULTIPLIED: u32 = 2;

pub const TEX_DIMENSION_TEXTURE1D: u32 = 2;
pub const TEX_DIMENSION_TEXTURE2D: u32 = 3;
pub const TEX_DIMENSION_TEXTURE3D: u32 = 4;

const DDPF_FOURCC: u32 = 0x0000_0004;
const DDPF_RGB: u32 = 0x0000_0040;
const DDPF_RGBA: u32 = 0x0000_0041;
const DDPF_LUMINANCE: u32 = 0x0002_0000;
const DDPF_LUMINANCEA: u32 = 0x0002_0001;
const DDPF_ALPHA: u32 = 0x0000_0002;

const fn fourcc(c: &[u8; 4]) -> u32 {
    u32::from_le_bytes(*c)
}

/// `DDS_PIXELFORMAT`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DdsPixelFormat {
    pub size: u32,
    pub flags: u32,
    pub four_cc: u32,
    pub rgb_bit_count: u32,
    pub r_mask: u32,
    pub g_mask: u32,
    pub b_mask: u32,
    pub a_mask: u32,
}

const fn pf(flags: u32, four_cc: u32, bits: u32, r: u32, g: u32, b: u32, a: u32) -> DdsPixelFormat {
    DdsPixelFormat { size: 32, flags, four_cc, rgb_bit_count: bits, r_mask: r, g_mask: g, b_mask: b, a_mask: a }
}

const fn pf_fourcc(c: &[u8; 4]) -> DdsPixelFormat {
    pf(DDPF_FOURCC, fourcc(c), 0, 0, 0, 0, 0)
}

const fn pf_d3dfmt(n: u32) -> DdsPixelFormat {
    pf(DDPF_FOURCC, n, 0, 0, 0, 0, 0)
}

impl DdsPixelFormat {
    fn is_bitmask(&self, r: u32, g: u32, b: u32, a: u32) -> bool {
        self.r_mask == r && self.g_mask == g && self.b_mask == b && self.a_mask == a
    }

    /// `DDS_PIXELFORMAT.GetDXGIFormat`: the format of a legacy (non-`DX10`) header.
    pub fn dxgi_format(&self) -> u32 {
        if self.flags & DDPF_RGB != 0 {
            match self.rgb_bit_count {
                32 => {
                    if self.is_bitmask(0xff, 0xff00, 0xff_0000, 0xff00_0000) {
                        return R8G8B8A8_UNORM;
                    }
                    if self.is_bitmask(0xff_0000, 0xff00, 0xff, 0xff00_0000) {
                        return B8G8R8A8_UNORM;
                    }
                    if self.is_bitmask(0xff_0000, 0xff00, 0xff, 0) {
                        return B8G8R8X8_UNORM;
                    }
                    if self.is_bitmask(0x3ff0_0000, 0x000f_fc00, 0x3ff, 0xc000_0000) {
                        return R10G10B10A2_UNORM;
                    }
                    if self.is_bitmask(0xffff, 0xffff_0000, 0, 0) {
                        return R16G16_UNORM;
                    }
                    if self.is_bitmask(0xffff_ffff, 0, 0, 0) {
                        return R32_FLOAT;
                    }
                }
                16 => {
                    if self.is_bitmask(0x7c00, 0x03e0, 0x001f, 0x8000) {
                        return B5G5R5A1_UNORM;
                    }
                    if self.is_bitmask(0xf800, 0x07e0, 0x001f, 0) {
                        return B5G6R5_UNORM;
                    }
                    if self.is_bitmask(0x0f00, 0x00f0, 0x000f, 0xf000) {
                        return B4G4R4A4_UNORM;
                    }
                }
                _ => {}
            }
        } else if self.flags & DDPF_LUMINANCE != 0 {
            if self.rgb_bit_count == 8 && self.is_bitmask(0xff, 0, 0, 0) {
                return R8_UNORM;
            }
            if self.rgb_bit_count == 16 {
                if self.is_bitmask(0xffff, 0, 0, 0) {
                    return R16_UNORM;
                }
                if self.is_bitmask(0xff, 0, 0, 0xff00) {
                    return R8G8_UNORM;
                }
            }
        } else if self.flags & DDPF_ALPHA != 0 {
            if self.rgb_bit_count == 8 {
                return A8_UNORM;
            }
        } else if self.flags & DDPF_FOURCC != 0 {
            let c = self.four_cc;
            let known = [
                (b"DXT1", BC1_UNORM),
                (b"DXT3", BC2_UNORM),
                (b"DXT5", BC3_UNORM),
                (b"DXT2", BC2_UNORM),
                (b"DXT4", BC3_UNORM),
                (b"ATI1", BC4_UNORM),
                (b"BC4U", BC4_UNORM),
                (b"BC4S", BC4_SNORM),
                (b"ATI2", BC5_UNORM),
                (b"BC5U", BC5_UNORM),
                (b"BC5S", BC5_SNORM),
                (b"RGBG", R8G8_B8G8_UNORM),
                (b"GRGB", G8R8_G8B8_UNORM),
            ];
            if let Some((_, f)) = known.iter().find(|(cc, _)| fourcc(cc) == c) {
                return *f;
            }
            return match c {
                36 => R16G16B16A16_UNORM,
                110 => R16G16B16A16_SNORM,
                111 => R16_FLOAT,
                112 => R16G16_FLOAT,
                113 => R16G16B16A16_FLOAT,
                114 => R32_FLOAT,
                115 => R32G32_FLOAT,
                116 => R32G32B32A32_FLOAT,
                _ => UNKNOWN,
            };
        }
        UNKNOWN
    }
}

/// `DDS_HEADER` (without the magic).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DdsHeader {
    pub size: u32,
    pub flags: u32,
    pub height: u32,
    pub width: u32,
    pub pitch_or_linear_size: u32,
    pub depth: u32,
    pub mip_map_count: u32,
    pub pixel_format: DdsPixelFormat,
    pub caps: u32,
    pub caps2: u32,
    pub caps3: u32,
    pub caps4: u32,
    pub reserved2: u32,
}

/// `DDS_HEADER_DXT10`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DdsHeaderDxt10 {
    pub dxgi_format: u32,
    pub resource_dimension: u32,
    pub misc_flag: u32,
    pub array_size: u32,
    pub misc_flags2: u32,
}

/// DirectXTex's `TexMetadata`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TexMetadata {
    pub width: i32,
    pub height: i32,
    pub depth: i32,
    pub array_size: i32,
    pub mip_levels: i32,
    pub misc_flags: u32,
    pub misc_flags2: u32,
    pub format: u32,
    pub dimension: u32,
}

impl TexMetadata {
    pub fn is_cubemap(&self) -> bool {
        self.misc_flags & TEX_MISC_TEXTURECUBE != 0
    }
    pub fn is_pm_alpha(&self) -> bool {
        self.misc_flags2 & TEX_MISC2_ALPHA_MODE_MASK == TEX_ALPHA_MODE_PREMULTIPLIED
    }
}

/// `DXTex._EncodeDDSHeader`: the magic, the header and, when the format has no
/// legacy pixel format (or it's an array), the `DX10` header. `None` for an
/// invalid or palettized format or an unknown dimension.
pub fn encode_dds_header(m: &TexMetadata, mut flags: u32) -> Result<Option<Vec<u8>>> {
    if !is_valid(m.format) || is_palettized(m.format) {
        return Ok(None);
    }
    if m.array_size > 1 && (m.array_size != 6 || m.dimension != TEX_DIMENSION_TEXTURE2D || !m.is_cubemap()) {
        flags |= DDS_FLAGS_FORCE_DX10_EXT;
    }
    if flags & DDS_FLAGS_FORCE_DX10_EXT_MISC2 != 0 {
        flags |= DDS_FLAGS_FORCE_DX10_EXT;
    }

    let mut ddpf = DdsPixelFormat::default();
    if flags & DDS_FLAGS_FORCE_DX10_EXT == 0 {
        ddpf = match m.format {
            R8G8B8A8_UNORM => pf(DDPF_RGBA, 0, 32, 0xff, 0xff00, 0xff_0000, 0xff00_0000),
            R16G16_UNORM => pf(DDPF_RGB, 0, 32, 0xffff, 0xffff_0000, 0, 0),
            R8G8_UNORM => pf(DDPF_LUMINANCEA, 0, 16, 0xff, 0, 0, 0xff00),
            R16_UNORM => pf(DDPF_LUMINANCE, 0, 16, 0xffff, 0, 0, 0),
            R8_UNORM => pf(DDPF_LUMINANCE, 0, 8, 0xff, 0, 0, 0),
            A8_UNORM => pf(DDPF_ALPHA, 0, 8, 0, 0, 0, 0xff),
            R8G8_B8G8_UNORM => pf_fourcc(b"RGBG"),
            G8R8_G8B8_UNORM => pf_fourcc(b"GRGB"),
            BC1_UNORM => pf_fourcc(b"DXT1"),
            BC2_UNORM => pf_fourcc(if m.is_pm_alpha() { b"DXT2" } else { b"DXT3" }),
            BC3_UNORM => pf_fourcc(if m.is_pm_alpha() { b"DXT4" } else { b"DXT5" }),
            BC4_UNORM => pf_fourcc(b"BC4U"),
            BC4_SNORM => pf_fourcc(b"BC4S"),
            BC5_UNORM => pf_fourcc(b"BC5U"),
            BC5_SNORM => pf_fourcc(b"BC5S"),
            B5G6R5_UNORM => pf(DDPF_RGB, 0, 16, 0xf800, 0x07e0, 0x001f, 0),
            B5G5R5A1_UNORM => pf(DDPF_RGBA, 0, 16, 0x7c00, 0x03e0, 0x001f, 0x8000),
            B8G8R8A8_UNORM => pf(DDPF_RGBA, 0, 32, 0xff_0000, 0xff00, 0xff, 0xff00_0000),
            B8G8R8X8_UNORM => pf(DDPF_RGB, 0, 32, 0xff_0000, 0xff00, 0xff, 0),
            B4G4R4A4_UNORM => pf(DDPF_RGBA, 0, 16, 0x0f00, 0x00f0, 0x000f, 0xf000),
            YUY2 => pf_fourcc(b"YUY2"),
            // Legacy D3DX formats, with the D3DFMT number as the FourCC.
            R32G32B32A32_FLOAT => pf_d3dfmt(116),
            R16G16B16A16_FLOAT => pf_d3dfmt(113),
            R16G16B16A16_UNORM => pf_d3dfmt(36),
            R16G16B16A16_SNORM => pf_d3dfmt(110),
            R32G32_FLOAT => pf_d3dfmt(115),
            R16G16_FLOAT => pf_d3dfmt(112),
            R32_FLOAT => pf_d3dfmt(114),
            R16_FLOAT => pf_d3dfmt(111),
            _ => DdsPixelFormat::default(),
        };
    }

    let mut h =
        DdsHeader { size: 124, flags: DDS_HEADER_FLAGS_TEXTURE, caps: DDS_SURFACE_FLAGS_TEXTURE, ..Default::default() };
    if m.mip_levels > 0 {
        h.flags |= DDS_HEADER_FLAGS_MIPMAP;
        h.mip_map_count = m.mip_levels as u32;
        if h.mip_map_count > 1 {
            h.caps |= DDS_SURFACE_FLAGS_MIPMAP;
        }
    }
    match m.dimension {
        TEX_DIMENSION_TEXTURE1D => {
            h.width = m.width as u32;
            h.height = 1;
            h.depth = 1;
        }
        TEX_DIMENSION_TEXTURE2D => {
            h.height = m.height as u32;
            h.width = m.width as u32;
            h.depth = 1;
            if m.is_cubemap() {
                h.caps |= DDS_SURFACE_FLAGS_CUBEMAP;
                h.caps2 |= DDS_CUBEMAP_ALLFACES;
            }
        }
        TEX_DIMENSION_TEXTURE3D => {
            h.flags |= DDS_HEADER_FLAGS_VOLUME;
            h.caps2 |= DDS_FLAGS_VOLUME;
            h.height = m.height as u32;
            h.width = m.width as u32;
            h.depth = m.depth as u32;
        }
        _ => return Ok(None),
    }

    let (row_pitch, slice_pitch) = compute_pitch(m.format, m.width, m.height)?;
    if is_compressed(m.format) {
        h.flags |= DDS_HEADER_FLAGS_LINEARSIZE;
        h.pitch_or_linear_size = slice_pitch as u32;
    } else {
        h.flags |= DDS_HEADER_FLAGS_PITCH;
        h.pitch_or_linear_size = row_pitch as u32;
    }

    let mut ext = DdsHeaderDxt10::default();
    let use_ext = ddpf.size == 0;
    if use_ext {
        h.pixel_format = pf_fourcc(b"DX10");
        ext.dxgi_format = m.format;
        ext.resource_dimension = m.dimension;
        ext.misc_flag = m.misc_flags & !TEX_MISC_TEXTURECUBE;
        if m.is_cubemap() {
            ext.misc_flag |= TEX_MISC_TEXTURECUBE;
            assert(m.array_size % 6 == 0)?;
            ext.array_size = (m.array_size / 6) as u32;
        } else {
            ext.array_size = m.array_size as u32;
        }
        if flags & DDS_FLAGS_FORCE_DX10_EXT_MISC2 != 0 {
            ext.misc_flags2 = m.misc_flags2;
        }
    } else {
        h.pixel_format = ddpf;
    }

    let mut out = Vec::with_capacity(148);
    let p = h.pixel_format;
    let mut words =
        vec![DDS_MAGIC, h.size, h.flags, h.height, h.width, h.pitch_or_linear_size, h.depth, h.mip_map_count];
    words.extend([0; 11]);
    words.extend([p.size, p.flags, p.four_cc, p.rgb_bit_count, p.r_mask, p.g_mask, p.b_mask, p.a_mask]);
    words.extend([h.caps, h.caps2, h.caps3, h.caps4, h.reserved2]);
    if use_ext {
        words.extend([ext.dxgi_format, ext.resource_dimension, ext.misc_flag, ext.array_size, ext.misc_flags2]);
    }
    for w in words {
        out.extend_from_slice(&w.to_le_bytes());
    }
    Ok(Some(out))
}

struct WordReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl WordReader<'_> {
    fn u32(&mut self) -> Result<u32> {
        let Some(b) = self.data.get(self.pos..self.pos + 4) else {
            return Err(Error::Format("DDS: unexpected end of the file".into()));
        };
        self.pos += 4;
        Ok(u32::from_le_bytes(b.try_into().unwrap()))
    }
}

/// `DXTex._ReadDDSHeader`: the header, the `DX10` header if there is one, and
/// where the data starts.
pub fn read_dds_header(data: &[u8]) -> Result<(DdsHeader, Option<DdsHeaderDxt10>, usize)> {
    let mut r = WordReader { data, pos: 0 };
    if r.u32()? != DDS_MAGIC {
        return Err(Error::Format("Invalid DDS magic!".into()));
    }
    let mut h = DdsHeader {
        size: r.u32()?,
        flags: r.u32()?,
        height: r.u32()?,
        width: r.u32()?,
        pitch_or_linear_size: r.u32()?,
        depth: r.u32()?,
        mip_map_count: r.u32()?,
        ..Default::default()
    };
    for _ in 0..11 {
        r.u32()?;
    }
    h.pixel_format = DdsPixelFormat {
        size: r.u32()?,
        flags: r.u32()?,
        four_cc: r.u32()?,
        rgb_bit_count: r.u32()?,
        r_mask: r.u32()?,
        g_mask: r.u32()?,
        b_mask: r.u32()?,
        a_mask: r.u32()?,
    };
    h.caps = r.u32()?;
    h.caps2 = r.u32()?;
    h.caps3 = r.u32()?;
    h.caps4 = r.u32()?;
    h.reserved2 = r.u32()?;
    let ext = if h.pixel_format.flags & DDPF_FOURCC != 0 && h.pixel_format.four_cc == fourcc(b"DX10") {
        Some(DdsHeaderDxt10 {
            dxgi_format: r.u32()?,
            resource_dimension: r.u32()?,
            misc_flag: r.u32()?,
            array_size: r.u32()?,
            misc_flags2: r.u32()?,
        })
    } else {
        None
    };
    if h.size != 124 || h.pixel_format.size != 32 {
        return Err(Error::Format("Invalid DDS header size".into()));
    }
    Ok((h, ext, r.pos))
}

/// A texture read from a DDS file: what `DDSIO.GetTexture` fills in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DdsTexture {
    pub format: TextureFormat,
    pub width: u16,
    pub height: u16,
    /// The array size (6 for a legacy cube map), or the depth of a volume.
    pub depth: u16,
    pub mip_levels: u8,
    pub stride: u16,
    /// Everything after the header.
    pub data: Vec<u8>,
}

fn unsupported(what: impl std::fmt::Display) -> Error {
    Error::Format(format!("DDS: {what}"))
}

/// `DDSIO.GetTexture`: reads a DDS file.
pub fn read_dds(dds: &[u8]) -> Result<DdsTexture> {
    let (h, ext, start) = read_dds_header(dds)?;
    let width = h.width;
    let mut height = h.height;
    let mut depth = h.depth;
    let mip_count = if h.mip_map_count == 0 { 1 } else { h.mip_map_count };
    let mut array_size = 1u32;
    let format;
    if let Some(ext) = ext {
        array_size = ext.array_size;
        format = ext.dxgi_format;
        if array_size == 0 {
            return Err(unsupported("ArraySize was 0! This isn't supported..."));
        }
        if is_palettized(format) || bits_per_pixel(format) == 0 {
            return Err(unsupported(format_args!("DXGI format {format} is not supported")));
        }
        match ext.resource_dimension {
            TEX_DIMENSION_TEXTURE1D => {
                if h.flags & DDS_HEIGHT != 0 && height != 1 {
                    return Err(unsupported("1D texture's height wasn't 1!"));
                }
                height = 1;
                depth = array_size;
            }
            TEX_DIMENSION_TEXTURE2D => {
                if ext.misc_flag & TEX_MISC_TEXTURECUBE != 0 {
                    array_size = array_size.wrapping_mul(6);
                }
                depth = array_size;
            }
            TEX_DIMENSION_TEXTURE3D => {
                if h.flags & DDS_HEADER_FLAGS_VOLUME == 0 {
                    return Err(unsupported("3D texture without volume flag!"));
                }
                if array_size > 1 {
                    return Err(unsupported("3D texture with ArraySize > 1!"));
                }
            }
            _ => return Err(unsupported("Unknown resource dimension!")),
        }
    } else {
        format = h.pixel_format.dxgi_format();
        if format == UNKNOWN {
            return Err(unsupported("Unknown DDS format."));
        }
        if h.flags & DDS_HEADER_FLAGS_VOLUME == 0 {
            if h.caps2 & DDS_CUBEMAP != 0 {
                if h.caps2 & DDS_CUBEMAP_ALLFACES != DDS_CUBEMAP_ALLFACES {
                    return Err(unsupported("Not all faces in cubemap exist!"));
                }
                array_size = 6;
            }
            depth = array_size;
        }
        if bits_per_pixel(format) == 0 {
            return Err(unsupported(format_args!("DXGI format {format} is not supported")));
        }
    }

    let data = dds[start..].to_vec();
    let (_, slice_pitch) = compute_pitch(format, width as i32, height as i32)?;
    if height == 0 {
        return Err(unsupported("division by zero (the height is 0)"));
    }
    // An int divided by a uint: done in 64 bits.
    let stride = (slice_pitch as i64 / height as i64) as u16;
    // The C# also walks the mips here, which can only fail for formats that
    // already failed above.
    let (mut w, mut hh) = (width as i32, height as i32);
    for _ in 0..mip_count {
        compute_pitch(format, w, hh)?;
        compute_scanlines(format, hh)?;
        w /= 2;
        hh /= 2;
    }

    Ok(DdsTexture {
        format: TextureFormat::from_dxgi(format),
        width: width as u16,
        height: height as u16,
        depth: depth as u16,
        mip_levels: mip_count as u8,
        stride,
        data,
    })
}

/// `DDSIO.GetDDSFile`: a DDS file for a texture's data. The data holds, for
/// each of the `depth` array slices, each mip in turn; it's written as far as it
/// goes. Fails like the C# for an unknown format, or a mip with no rows or
/// columns of data (only possible for uncompressed formats).
pub fn write_dds(
    format: TextureFormat,
    width: u16,
    height: u16,
    depth: u16,
    mip_levels: u8,
    data: &[u8],
) -> Result<Vec<u8>> {
    let fmt = format.dxgi();
    // GetMipmapImages: the images' sizes and offsets, slice by slice.
    struct Image {
        width: i32,
        height: i32,
        row_pitch: i32,
        slice_pitch: i32,
        pixels: i32,
    }
    let mut images = Vec::with_capacity(mip_levels as usize * depth as usize);
    let mut add = 0i32;
    for _ in 0..depth {
        for i in 0..mip_levels as usize {
            let (w, h) = (mip_size(width, i)?, mip_size(height, i)?);
            let (row_pitch, slice_pitch) = compute_pitch(fmt, w, h)?;
            images.push(Image { width: w, height: h, row_pitch, slice_pitch, pixels: add });
            add = add.wrapping_add(slice_pitch);
        }
    }
    // GetImageMetadata: always a 2D texture, the depth as the array size.
    let meta = TexMetadata {
        width: width as i32,
        height: height as i32,
        depth: 1,
        array_size: depth as i32,
        mip_levels: mip_levels as i32,
        format: fmt,
        dimension: TEX_DIMENSION_TEXTURE2D,
        ..Default::default()
    };
    let Some(mut out) = encode_dds_header(&meta, 0)? else {
        return Err(Error::Format("Couldn't make DDS header".into()));
    };
    for img in &images {
        if img.row_pitch <= 0 {
            return Err(Error::Format("Invalid row pitch.".into()));
        }
        if img.slice_pitch <= 0 {
            return Err(Error::Format("Invalid slice pitch.".into()));
        }
        // The DDS pitch is the same as the image's, so the C# copies the
        // slice at once, cut short at the end of the data.
        let (_, dds_slice_pitch) = compute_pitch(fmt, img.width, img.height)?;
        let start = img.pixels as i64;
        let len = (dds_slice_pitch as i64).min(data.len() as i64 - start);
        if len > 0 {
            out.extend_from_slice(&data[start as usize..(start + len) as usize]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitches_like_directxtex() {
        assert_eq!(compute_pitch(BC1_UNORM, 256, 128).unwrap(), (512, 16384));
        assert_eq!(compute_pitch(BC7_UNORM, 1, 1).unwrap(), (16, 16));
        assert_eq!(compute_pitch(R8G8B8A8_UNORM, 3, 2).unwrap(), (12, 24));
        assert_eq!(compute_pitch(A8_UNORM, 0, 4).unwrap(), (0, 0));
        assert!(compute_pitch(UNKNOWN, 4, 4).is_err());
        assert_eq!(bits_per_pixel(BC6H_UF16), 8);
        assert_eq!(bits_per_pixel(B5G5R5A1_UNORM), 16);
        assert_eq!(bits_per_pixel(R32G32B32A32_FLOAT), 128);
    }

    #[test]
    fn dds_round_trips() {
        for (format, depth, ext) in [
            (TextureFormat::BC1, 1, false),
            (TextureFormat::BC7, 1, true),
            (TextureFormat::A8, 1, false),
            (TextureFormat::A1R5G5B5, 1, false),
            (TextureFormat::BC3, 6, true),
            (TextureFormat::R16G16B16A16F, 1, false),
        ] {
            let fmt = format.dxgi();
            let size: i32 = (0..3).map(|i| compute_pitch(fmt, 16 >> i, 8 >> i).unwrap().1).sum::<i32>() * depth;
            let data: Vec<u8> = (0..size).map(|i| i as u8).collect();
            let dds = write_dds(format, 16, 8, depth as u16, 3, &data).unwrap();
            assert_eq!(dds.len(), 128 + if ext { 20 } else { 0 } + data.len(), "{format:?}");
            let t = read_dds(&dds).unwrap();
            assert_eq!((t.format, t.width, t.height, t.depth, t.mip_levels), (format, 16, 8, depth as u16, 3));
            assert_eq!(t.data, data);
        }
    }

    #[test]
    fn short_data_and_empty_mips() {
        // Data shorter than the mips: written as far as it goes.
        let dds = write_dds(TextureFormat::BC1, 8, 8, 1, 2, &[1; 20]).unwrap();
        assert_eq!(dds.len(), 128 + 20);
        // An uncompressed mip with no rows fails, as in the C#.
        assert!(write_dds(TextureFormat::A8, 4, 1, 1, 2, &[0; 6]).is_err());
    }
}
