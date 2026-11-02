//! Texture dictionaries (ytd), from `Rsc8Texture.cs` and `YtdFile.cs`, with the
//! engine texture's XML from `Texture.cs`: a texture's `Name` goes in the XML and
//! its pixels in a `<Name>.dds` file next to it (see [`super::dds`]).
//!
//! The structs are meant to be reused by the formats that embed textures
//! (drawables have a [`TextureDictionary`] in their shader group, and their
//! shader parameters point to [`TextureBase`]s):
//!
//! ```ignore
//! // Binary: read at the reader's position (e.g. inside `r.ptr(..)`), write as a
//! // block and point to it.
//! let td = r.ptr(|r| TextureDictionary::read(r, cx.hashes))?;
//! let id: BlockId = td.write(&mut w);
//! // XML: the dictionary's contents go inside the node; DDS files go to
//! // `w.externals`. A DDS the C# couldn't make is reported in
//! // `w.external_errors` (the C# fails the whole conversion then).
//! w.node("TextureDictionary", |w| td.write_xml(w));
//! let td = TextureDictionary::read_xml(node.node("TextureDictionary").unwrap())?;
//! ```
//!
//! [`Texture`] (one dictionary entry) and [`TextureBase`] have the same four
//! functions. A dictionary made from XML gets its hashes and order from
//! [`TextureDictionary::build`], like the C#'s `Build`.
//!
//! Known differences from the C#, in cases no game ytd has: two textures sharing
//! one data pointer each read their own size (the C#'s block pool gives the
//! second the first one's array), and textures with equal name hashes keep
//! their order (the C#'s `List.Sort` isn't stable past 16 items).

use super::XmlContext;
use super::dds::{self, TextureFormat, compute_pitch, mip_size};
use crate::hash::{HashIndex, joaat};
use crate::rsc8::{BlockBuf, BlockId, BlockMap, Rsc8Reader, Rsc8Writer, read_resource};
use crate::xml::{XmlDocument, XmlNode, XmlWriter};
use crate::{Error, Result, bail};

const ROOT: &str = "RDR2TextureDictionary";

/// `Rsc8TextureFormat` (really `rage::sga::BufferFormat`, also used by
/// drawables' vertex buffers): the texture format as stored, in one byte.
pub mod buffer_format {
    pub const UNKNOWN: u8 = 0x0;
    pub const R32G32B32A32_TYPELESS: u8 = 0x1;
    pub const R32G32B32A32_FLOAT: u8 = 0x2;
    pub const R32G32B32A32_UINT: u8 = 0x3;
    pub const R32G32B32A32_SINT: u8 = 0x4;
    pub const R32G32B32_TYPELESS: u8 = 0x5;
    pub const R32G32B32_FLOAT: u8 = 0x6;
    pub const R32G32B32_UINT: u8 = 0x7;
    pub const R32G32B32_SINT: u8 = 0x8;
    pub const R16G16B16A16_TYPELESS: u8 = 0x9;
    pub const R16G16B16A16_FLOAT: u8 = 0xA;
    pub const R16G16B16A16_UNORM: u8 = 0xB;
    pub const R16G16B16A16_UINT: u8 = 0xC;
    pub const R16G16B16A16_SNORM: u8 = 0xD;
    pub const R16G16B16A16_SINT: u8 = 0xE;
    pub const R32G32_TYPELESS: u8 = 0xF;
    pub const R32G32_FLOAT: u8 = 0x10;
    pub const R32G32_UINT: u8 = 0x11;
    pub const R32G32_SINT: u8 = 0x12;
    pub const D32_FLOAT_S8X24_UINT: u8 = 0x14;
    pub const B10G10R10A2_UNORM: u8 = 0x15;
    pub const R10G10B10A2_SNORM: u8 = 0x16;
    pub const R10G10B10A2_TYPELESS: u8 = 0x17;
    pub const R10G10B10A2_UNORM: u8 = 0x18;
    pub const R10G10B10A2_UINT: u8 = 0x19;
    pub const R11G11B10_FLOAT: u8 = 0x1A;
    pub const R8G8B8A8_TYPELESS: u8 = 0x1B;
    pub const R8G8B8A8_UNORM: u8 = 0x1C;
    pub const R8G8B8A8_UNORM_SRGB: u8 = 0x1D;
    pub const R8G8B8A8_UINT: u8 = 0x1E;
    pub const R8G8B8A8_SNORM: u8 = 0x1F;
    pub const R8G8B8A8_SINT: u8 = 0x20;
    pub const R16G16_TYPELESS: u8 = 0x21;
    pub const R16G16_FLOAT: u8 = 0x22;
    pub const R16G16_UNORM: u8 = 0x23;
    pub const R16G16_UINT: u8 = 0x24;
    pub const R16G16_SNORM: u8 = 0x25;
    pub const R16G16_SINT: u8 = 0x26;
    pub const R32_TYPELESS: u8 = 0x27;
    pub const D32_FLOAT: u8 = 0x28;
    pub const R32_FLOAT: u8 = 0x29;
    pub const R32_UINT: u8 = 0x2A;
    pub const R32_SINT: u8 = 0x2B;
    pub const R8G8_TYPELESS: u8 = 0x30;
    pub const R8G8_UNORM: u8 = 0x31;
    pub const R8G8_UINT: u8 = 0x32;
    pub const R8G8_SNORM: u8 = 0x33;
    pub const R8G8_SINT: u8 = 0x34;
    pub const R16_TYPELESS: u8 = 0x35;
    pub const R16_FLOAT: u8 = 0x36;
    pub const D16_UNORM: u8 = 0x37;
    pub const R16_UNORM: u8 = 0x38;
    pub const R16_UINT: u8 = 0x39;
    pub const R16_SNORM: u8 = 0x3A;
    pub const R16_SINT: u8 = 0x3B;
    pub const R8_TYPELESS: u8 = 0x3C;
    pub const R8_UNORM: u8 = 0x3D;
    pub const R8_UINT: u8 = 0x3E;
    pub const R8_SNORM: u8 = 0x3F;
    pub const R8_SINT: u8 = 0x40;
    pub const A8_UNORM: u8 = 0x41;
    pub const R9G9B9E5_SHAREDEXP: u8 = 0x43;
    pub const BC1_TYPELESS: u8 = 0x46;
    pub const BC1_UNORM: u8 = 0x47;
    pub const BC1_UNORM_SRGB: u8 = 0x48;
    pub const BC2_TYPELESS: u8 = 0x49;
    pub const BC2_UNORM: u8 = 0x4A;
    pub const BC2_UNORM_SRGB: u8 = 0x4B;
    pub const BC3_TYPELESS: u8 = 0x4C;
    pub const BC3_UNORM: u8 = 0x4D;
    pub const BC3_UNORM_SRGB: u8 = 0x4E;
    pub const BC4_TYPELESS: u8 = 0x4F;
    pub const BC4_UNORM: u8 = 0x50;
    pub const BC4_SNORM: u8 = 0x51;
    pub const BC5_TYPELESS: u8 = 0x52;
    pub const BC5_UNORM: u8 = 0x53;
    pub const BC5_SNORM: u8 = 0x54;
    pub const B5G6R5_UNORM: u8 = 0x55;
    pub const B5G5R5A1_UNORM: u8 = 0x56;
    pub const B8G8R8A8_UNORM: u8 = 0x57;
    pub const B8G8R8A8_TYPELESS: u8 = 0x5A;
    pub const B8G8R8A8_UNORM_SRGB: u8 = 0x5B;
    pub const BC6H_TYPELESS: u8 = 0x5E;
    pub const BC6H_UF16: u8 = 0x5F;
    pub const BC6H_SF16: u8 = 0x60;
    pub const BC7_TYPELESS: u8 = 0x61;
    pub const BC7_UNORM: u8 = 0x62;
    pub const BC7_UNORM_SRGB: u8 = 0x63;
    pub const NV12: u8 = 0x67;
    pub const B4G4R4A4_UNORM: u8 = 0x73;
    pub const D16_UNORM_S8_UINT: u8 = 0x76;
    pub const R16_UNORM_X8_TYPELESS: u8 = 0x77;
    pub const X16_TYPELESS_G8_UINT: u8 = 0x78;
    pub const ETC1: u8 = 0x79;
    pub const ETC1_SRGB: u8 = 0x7A;
    pub const ETC1A: u8 = 0x7B;
    pub const ETC1A_SRGB: u8 = 0x7C;
    pub const R4G4_UNORM: u8 = 0x7F;
}

use buffer_format as bf;

/// `ConvertToEngineFormat`: formats the engine doesn't know become BC1.
pub fn to_engine_format(f: u8) -> TextureFormat {
    use TextureFormat::*;
    match f {
        bf::BC2_UNORM => BC2,
        bf::BC3_UNORM => BC3,
        bf::BC4_UNORM => BC4,
        bf::BC5_UNORM => BC5,
        bf::BC6H_UF16 => BC6H,
        bf::BC7_UNORM => BC7,
        bf::BC1_UNORM_SRGB => BC1SRGB,
        bf::BC2_UNORM_SRGB => BC2SRGB,
        bf::BC3_UNORM_SRGB => BC3SRGB,
        bf::BC7_UNORM_SRGB => BC7SRGB,
        bf::R8G8B8A8_UNORM => A8B8G8R8,
        bf::B8G8R8A8_UNORM => A8R8G8B8,
        bf::R8G8B8A8_UNORM_SRGB => A8B8G8R8SRGB,
        bf::B8G8R8A8_UNORM_SRGB => A8R8G8B8SRGB,
        bf::B5G5R5A1_UNORM => A1R5G5B5,
        bf::R10G10B10A2_UNORM => R10G10B10A2,
        bf::R16G16B16A16_UNORM => R16G16B16A16U,
        bf::R16G16B16A16_FLOAT => R16G16B16A16F,
        bf::R16_UNORM => R16U,
        bf::R16_FLOAT => R16F,
        bf::R8_UNORM => L8,
        bf::A8_UNORM => A8,
        bf::R32_FLOAT => R32F,
        bf::R32G32B32A32_FLOAT => R32G32B32A32F,
        bf::R11G11B10_FLOAT => R11G11B10F,
        _ => BC1,
    }
}

/// `ConvertToRsc8Format`: engine formats with no stored equivalent become BC1.
pub fn from_engine_format(f: TextureFormat) -> u8 {
    use TextureFormat::*;
    match f {
        BC2 => bf::BC2_UNORM,
        BC3 => bf::BC3_UNORM,
        BC4 => bf::BC4_UNORM,
        BC5 => bf::BC5_UNORM,
        BC6H => bf::BC6H_UF16,
        BC7 => bf::BC7_UNORM,
        BC1SRGB => bf::BC1_UNORM_SRGB,
        BC2SRGB => bf::BC2_UNORM_SRGB,
        BC3SRGB => bf::BC3_UNORM_SRGB,
        BC7SRGB => bf::BC7_UNORM_SRGB,
        A8B8G8R8 => bf::R8G8B8A8_UNORM,
        A8R8G8B8 => bf::B8G8R8A8_UNORM,
        A8B8G8R8SRGB => bf::R8G8B8A8_UNORM_SRGB,
        A8R8G8B8SRGB => bf::B8G8R8A8_UNORM_SRGB,
        A1R5G5B5 => bf::B5G5R5A1_UNORM,
        R10G10B10A2 => bf::R10G10B10A2_UNORM,
        R16G16B16A16U => bf::R16G16B16A16_UNORM,
        R16G16B16A16F => bf::R16G16B16A16_FLOAT,
        R16U => bf::R16_UNORM,
        R16F => bf::R16_FLOAT,
        L8 => bf::R8_UNORM,
        A8 => bf::A8_UNORM,
        R32F => bf::R32_FLOAT,
        R32G32B32A32F => bf::R32G32B32A32_FLOAT,
        R11G11B10F => bf::R11G11B10_FLOAT,
        _ => bf::BC1_UNORM,
    }
}

/// `GetBlockStride`: the pixel size of uncompressed formats, the block size of
/// compressed ones.
pub fn block_stride(f: u8) -> u32 {
    match f {
        bf::UNKNOWN => 1,
        bf::BC2_UNORM
        | bf::BC3_UNORM
        | bf::BC5_UNORM
        | bf::BC6H_UF16
        | bf::BC7_UNORM
        | bf::BC2_UNORM_SRGB
        | bf::BC3_UNORM_SRGB
        | bf::BC7_UNORM_SRGB
        | bf::R32G32B32A32_FLOAT => 16,
        bf::R8G8B8A8_UNORM
        | bf::B8G8R8A8_UNORM
        | bf::R8G8B8A8_UNORM_SRGB
        | bf::B8G8R8A8_UNORM_SRGB
        | bf::R10G10B10A2_UNORM
        | bf::R32_FLOAT
        | bf::R11G11B10_FLOAT => 4,
        bf::B5G5R5A1_UNORM | bf::R16_UNORM | bf::R16_FLOAT => 2,
        bf::R8_UNORM | bf::A8_UNORM => 1,
        _ => 8,
    }
}

/// `GetBlockPixelCount`: 4 for block-compressed formats, else 1.
pub fn block_pixel_count(f: u8) -> u32 {
    match f {
        bf::BC1_UNORM
        | bf::BC2_UNORM
        | bf::BC3_UNORM
        | bf::BC4_UNORM
        | bf::BC5_UNORM
        | bf::BC6H_UF16
        | bf::BC7_UNORM
        | bf::BC1_UNORM_SRGB
        | bf::BC2_UNORM_SRGB
        | bf::BC3_UNORM_SRGB
        | bf::BC7_UNORM_SRGB => 4,
        _ => 1,
    }
}

/// `GetBlockCount`: the blocks the texture's memory is sized in, with power of
/// two sizes when there are mips, rows and columns padded to 8 blocks (16 for
/// 1-byte formats), and 0x2000 more for flags 0x18228002.
pub fn block_count(f: u8, width: u16, height: u16, depth: u16, mips: u8, flags: u32) -> u32 {
    let bs = block_stride(f);
    let bp = block_pixel_count(f);
    let (width, height, depth) = (width as u32, height as u32, depth as u32);
    let (mut bw, mut bh, mut bd) = (width, height, depth);
    let align = if bs == 1 { 16 } else { 8 };
    if mips > 1 {
        bw = width.max(1).next_power_of_two();
        bh = height.max(1).next_power_of_two();
        bd = depth.max(1).next_power_of_two();
    }
    let mut bc = 0u32;
    for _ in 0..mips {
        let mut bx = bw.div_ceil(bp).max(1);
        let mut by = bh.div_ceil(bp).max(1);
        bx += (align - bx % align) % align;
        by += (align - by % align) % align;
        bc = bc.wrapping_add(bx.wrapping_mul(by).wrapping_mul(bd));
        bw /= 2;
        bh /= 2;
    }
    if flags == 0x1822_8002 {
        bc = bc.wrapping_add(0x2000);
    }
    bc
}

/// `Rsc8TextureDimension`.
pub mod dimension {
    pub const TEXTURE_2D: u8 = 1;
    pub const TEXTURE_CUBE: u8 = 2;
    pub const TEXTURE_3D: u8 = 3;
}

/// `Rsc8TextureTileMode`.
pub mod tile_mode {
    pub const DEPTH: u8 = 4;
    pub const LINEAR: u8 = 8;
    pub const DISPLAY: u8 = 10;
    pub const STANDARD: u8 = 13;
    pub const RENDER_TARGET: u8 = 14;
    pub const VOLUME_STANDARD: u8 = 19;
    pub const VOLUME_RENDER_TARGET: u8 = 20;
    pub const AUTO: u8 = 255;
}

/// `Rsc8ShaderResourceViewDimension`.
pub mod srv_dimension {
    pub const TEXTURE_2D: u64 = 0x401;
    pub const TEXTURE_2D_ARRAY: u64 = 0x601;
    pub const TEXTURE_CUBE: u64 = 0x802;
    pub const TEXTURE_3D: u64 = 0xa03;
    pub const BUFFER: u64 = 0x104;
}

const DIMENSION_NAMES: &[(&str, u8)] = &[
    ("Texture2D", dimension::TEXTURE_2D),
    ("TextureCube", dimension::TEXTURE_CUBE),
    ("Texture3D", dimension::TEXTURE_3D),
];

const TILE_MODE_NAMES: &[(&str, u8)] = &[
    ("Depth", tile_mode::DEPTH),
    ("Linear", tile_mode::LINEAR),
    ("Display", tile_mode::DISPLAY),
    ("Standard", tile_mode::STANDARD),
    ("RenderTarget", tile_mode::RENDER_TARGET),
    ("VolumeStandard", tile_mode::VOLUME_STANDARD),
    ("VolumeRenderTarget", tile_mode::VOLUME_RENDER_TARGET),
    ("Auto", tile_mode::AUTO),
];

/// .NET's `Enum.ToString` for a byte enum: the name, or the number.
pub fn enum_name(names: &[(&str, u8)], v: u8) -> String {
    names.iter().find(|(_, x)| *x == v).map(|(n, _)| n.to_string()).unwrap_or_else(|| v.to_string())
}

/// .NET's `Enum.TryParse` for a byte enum: a number, or names (comma-separated
/// names are or-ed together), case-sensitive.
pub fn parse_enum(names: &[(&str, u8)], s: Option<&str>) -> Option<u8> {
    let s = s?.trim();
    if s.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+') {
        return s.strip_prefix('+').unwrap_or(s).parse().ok();
    }
    let mut v = 0;
    for part in s.split(',') {
        let part = part.trim();
        v |= names.iter().find(|(n, _)| *n == part)?.1;
    }
    Some(v)
}

/// `BufferUtil.ReadArray` of bytes, as the texture data is read: `None` for a
/// null pointer or no bytes, and zeros past the end of the file's data.
fn read_data(r: &mut Rsc8Reader, ptr: u64, count: usize) -> Result<Option<Vec<u8>>> {
    if ptr == 0 || count == 0 {
        return Ok(None);
    }
    Ok(Some(r.bytes_at_padded(ptr, count)?))
}

/// `Rsc8ShaderResourceView` (64 bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShaderResourceView {
    pub vft: u64,
    /// Seems to be two u16s.
    pub unknown_08h: u64,
    /// See [`srv_dimension`].
    pub dimension: u64,
    pub unknown_18h: u64,
    pub unknown_20h: u64,
    pub unknown_28h: u64,
    pub unknown_30h: u64,
    pub unknown_38h: u64,
}

impl Default for ShaderResourceView {
    fn default() -> Self {
        Self {
            vft: 0x0000_0001_4091_0080,
            unknown_08h: 0,
            dimension: 0,
            unknown_18h: 5,
            unknown_20h: 0,
            unknown_28h: 0,
            unknown_30h: 0,
            unknown_38h: 0,
        }
    }
}

impl ShaderResourceView {
    pub const BLOCK_LENGTH: usize = 64;

    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        Ok(Self {
            vft: r.u64()?,
            unknown_08h: r.u64()?,
            dimension: r.u64()?,
            unknown_18h: r.u64()?,
            unknown_20h: r.u64()?,
            unknown_28h: r.u64()?,
            unknown_30h: r.u64()?,
            unknown_38h: r.u64()?,
        })
    }

    /// Writes the 64 bytes in place (it's embedded in the texture).
    pub fn write(&self, b: &mut BlockBuf) {
        for v in [
            self.vft,
            self.unknown_08h,
            self.dimension,
            self.unknown_18h,
            self.unknown_20h,
            self.unknown_28h,
            self.unknown_30h,
            self.unknown_38h,
        ] {
            b.u64(v);
        }
    }
}

/// `Rsc8TextureBase` (80 bytes, `rage::sga::Texture`), with the fields of the
/// engine texture it derives from (`format`, `stride`, `data`). This is what
/// drawable shader parameters point to; dictionaries hold [`Texture`]s.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureBase {
    pub vft: u64,
    pub block_count: u32,
    pub block_stride: u32,
    pub flags: u32,
    pub unknown_14h: u32,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    /// See [`dimension`].
    pub dimension: u8,
    /// The stored format, see [`buffer_format`].
    pub file_format: u8,
    /// See [`tile_mode`].
    pub tile_mode: u8,
    pub anti_alias_type: u8,
    pub mip_levels: u8,
    pub unknown_23h: u8,
    pub unknown_24h: u8,
    pub unknown_25h: u8,
    pub usage_count: u16,
    pub name: Option<String>,
    /// What the SRV pointer points to (C#'s `SRVRef`). When writing, a non-null
    /// pointer always points 0x68 bytes into the texture, where a [`Texture`]
    /// has its own.
    pub srv_ref: Option<ShaderResourceView>,
    pub unknown_40h: u32,
    pub unknown_44h: u32,
    pub unknown_48h: u64,
    /// The engine format (`Texture.Format`), from `file_format` when read.
    pub format: TextureFormat,
    /// The row pitch of the first mip (`Texture.Stride`); not stored.
    pub stride: u16,
    /// The pixels: each array slice (or cube face, or volume slice), each mip.
    pub data: Option<Vec<u8>>,
}

impl Default for TextureBase {
    fn default() -> Self {
        Self {
            vft: 0x0000_0001_4091_00b0,
            block_count: 0,
            block_stride: 0,
            flags: 0,
            unknown_14h: 0,
            width: 0,
            height: 0,
            depth: 1,
            dimension: 0,
            file_format: 0,
            tile_mode: 0,
            anti_alias_type: 0,
            mip_levels: 0,
            unknown_23h: 0,
            unknown_24h: 0,
            unknown_25h: 0,
            usage_count: 1,
            name: None,
            srv_ref: None,
            unknown_40h: 0,
            unknown_44h: 0,
            unknown_48h: 0,
            format: TextureFormat::Unknown,
            stride: 0,
            data: None,
        }
    }
}

impl TextureBase {
    pub const BLOCK_LENGTH: usize = 80;

    /// Reads the 80 bytes at the reader's position, then what they point to:
    /// the name, the SRV and the pixels (as many as `calc_data_size` says).
    /// Adds the name to `hashes`, like the C#'s `JenkIndex.Ensure`.
    pub fn read(r: &mut Rsc8Reader, hashes: &HashIndex) -> Result<Self> {
        let mut t = Self {
            vft: r.u64()?,
            block_count: r.u32()?,
            block_stride: r.u32()?,
            flags: r.u32()?,
            unknown_14h: r.u32()?,
            width: r.u16()?,
            height: r.u16()?,
            depth: r.u16()?,
            dimension: r.u8()?,
            file_format: r.u8()?,
            tile_mode: r.u8()?,
            anti_alias_type: r.u8()?,
            mip_levels: r.u8()?,
            unknown_23h: r.u8()?,
            unknown_24h: r.u8()?,
            unknown_25h: r.u8()?,
            usage_count: r.u16()?,
            ..Default::default()
        };
        let name_ptr = r.u64()?;
        let srv_ptr = r.u64()?;
        let data_ptr = r.u64()?;
        t.unknown_40h = r.u32()?;
        t.unknown_44h = r.u32()?;
        t.unknown_48h = r.u64()?;

        t.name = r.string_at(name_ptr)?;
        t.srv_ref = r.follow(srv_ptr, ShaderResourceView::read)?;
        t.format = to_engine_format(t.file_format);
        t.stride = t.calculate_stride()?;
        let size = t.calc_data_size()?;
        if size < 0 {
            bail!("texture {:?}: data size {size} overflows", t.name);
        }
        t.data = read_data(r, data_ptr, size as usize)?;
        if let Some(name) = &t.name {
            hashes.ensure(name);
        }
        Ok(t)
    }

    /// `CalcDataSize`: the bytes of all the mips, times the depth.
    pub fn calc_data_size(&self) -> Result<i32> {
        let fmt = self.format.dxgi();
        let mut len = 0i32;
        for i in 0..self.mip_levels as usize {
            let (_, slice) = compute_pitch(fmt, mip_size(self.width, i)?, mip_size(self.height, i)?)?;
            len = len.wrapping_add(slice);
        }
        Ok(len.wrapping_mul(self.depth as i32))
    }

    /// `CalculateStride`: the row pitch of the first mip.
    pub fn calculate_stride(&self) -> Result<u16> {
        let (row, _) = compute_pitch(self.format.dxgi(), self.width as i32, self.height as i32)?;
        Ok(row as u16)
    }

    /// Writes the 80 bytes into `b`, adding the name and the pixels (padded to
    /// `block_count` blocks, which the game needs). `srv` is the block whose
    /// offset 0x68 the SRV pointer points to (null without it).
    pub fn write_fields(&self, b: &mut BlockBuf, w: &mut Rsc8Writer, srv: Option<BlockId>) {
        let target = self.block_count.wrapping_mul(self.block_stride & 0xFFF) as usize;
        let data = self.data.as_ref().map(|d| {
            let mut d = d.clone();
            if d.len() < target {
                d.resize(target, 0);
            }
            d
        });
        b.u64(self.vft);
        b.u32(self.block_count);
        b.u32(self.block_stride);
        b.u32(self.flags);
        b.u32(self.unknown_14h);
        b.u16(self.width);
        b.u16(self.height);
        b.u16(self.depth);
        b.u8(self.dimension);
        b.u8(self.file_format);
        b.u8(self.tile_mode);
        b.u8(self.anti_alias_type);
        b.u8(self.mip_levels);
        b.u8(self.unknown_23h);
        b.u8(self.unknown_24h);
        b.u8(self.unknown_25h);
        b.u16(self.usage_count);
        b.ptr(w.opt_string(self.name.as_deref()));
        b.ptr_offset(srv, 0x68);
        b.ptr(w.array_in(data.as_deref(), true));
        b.u32(self.unknown_40h);
        b.u32(self.unknown_44h);
        b.u64(self.unknown_48h);
    }

    /// Writes the texture as an 80-byte block.
    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        self.write_fields(&mut b, w, self.srv_ref.as_ref().map(|_| id));
        w.set(id, b, false);
        id
    }

    /// The texture's XML (`Texture.Write` and `Rsc8TextureBase.Write`): the name,
    /// with the pixels as `<name>.dds`, and the fields that aren't the default.
    pub fn write_xml(&self, w: &mut XmlWriter) {
        if let Some(name) = self.name.as_deref().filter(|n| !n.is_empty()) {
            w.string("Name", name);
            if let Some(data) = &self.data {
                let file = format!("{name}.dds");
                match dds::write_dds(self.format, self.width, self.height, self.depth, self.mip_levels, data) {
                    Ok(dds) => w.external(&file, &dds),
                    Err(e) => w.external_errors.push(format!("{file}: {e}")),
                }
            }
        }
        if self.dimension != dimension::TEXTURE_2D {
            w.string("Dimension", &enum_name(DIMENSION_NAMES, self.dimension));
        }
        if self.tile_mode != tile_mode::STANDARD {
            w.string("TileMode", &enum_name(TILE_MODE_NAMES, self.tile_mode));
        }
        if self.flags != 0x1000_0000 {
            w.u32("Flags", self.flags);
        }
        if self.unknown_44h != 0 {
            w.u32("Unknown44", self.unknown_44h);
        }
    }

    /// Reads a texture's XML, and its DDS file if there is one: the format,
    /// size and mips come from the DDS, the stored format and block count
    /// follow from them.
    pub fn read_xml(node: XmlNode) -> Result<Self> {
        let mut t = Self { name: node.string("Name"), ..Default::default() };
        if let Some(name) = t.name.as_deref().filter(|n| !n.is_empty())
            && let Some(file) = node.external(&format!("{name}.dds"))
        {
            let tex = dds::read_dds(&file).map_err(|e| Error::Format(format!("{name}.dds: {e}")))?;
            t.format = tex.format;
            t.width = tex.width;
            t.height = tex.height;
            t.depth = tex.depth;
            t.stride = tex.stride;
            t.mip_levels = tex.mip_levels;
            t.data = Some(tex.data);
        }
        t.dimension = parse_enum(DIMENSION_NAMES, node.string("Dimension").as_deref()).unwrap_or(dimension::TEXTURE_2D);
        t.tile_mode = parse_enum(TILE_MODE_NAMES, node.string("TileMode").as_deref()).unwrap_or(tile_mode::STANDARD);
        t.flags = node.u32("Flags", 0x1000_0000);
        t.unknown_44h = node.u32("Unknown44", 0);
        t.file_format = from_engine_format(t.format);
        t.block_count = block_count(t.file_format, t.width, t.height, t.depth, t.mip_levels, t.flags);
        t.block_stride = block_stride(t.file_format);
        Ok(t)
    }
}

/// `Rsc8Texture` (176 bytes): a texture in a dictionary, with its shader
/// resource view embedded at 0x68.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Texture {
    pub base: TextureBase,
    pub unknown_50h: u64,
    pub unknown_58h: u64,
    pub unknown_60h: u64,
    pub srv: ShaderResourceView,
    pub unknown_a8h: u64,
}

impl Texture {
    pub const BLOCK_LENGTH: usize = 176;

    /// Reads the 176 bytes at the reader's position and what they point to.
    pub fn read(r: &mut Rsc8Reader, hashes: &HashIndex) -> Result<Self> {
        Ok(Self {
            base: TextureBase::read(r, hashes)?,
            unknown_50h: r.u64()?,
            unknown_58h: r.u64()?,
            unknown_60h: r.u64()?,
            srv: ShaderResourceView::read(r)?,
            unknown_a8h: r.u64()?,
        })
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        self.base.write_fields(&mut b, w, Some(id));
        b.u64(self.unknown_50h);
        b.u64(self.unknown_58h);
        b.u64(self.unknown_60h);
        self.srv.write(&mut b);
        b.u64(self.unknown_a8h);
        w.set(id, b, false);
        id
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        self.base.write_xml(w);
        if self.srv.unknown_08h != 0 {
            w.u64("SRVUnknown08", self.srv.unknown_08h);
        }
    }

    /// Reads a texture's XML; the SRV's dimension follows from the texture's.
    pub fn read_xml(node: XmlNode) -> Result<Self> {
        let base = TextureBase::read_xml(node)?;
        let dimension = match base.dimension {
            dimension::TEXTURE_2D if base.depth == 1 => srv_dimension::TEXTURE_2D,
            dimension::TEXTURE_2D => srv_dimension::TEXTURE_2D_ARRAY,
            dimension::TEXTURE_CUBE => srv_dimension::TEXTURE_CUBE,
            dimension::TEXTURE_3D => srv_dimension::TEXTURE_3D,
            _ => 0,
        };
        let srv = ShaderResourceView { dimension, unknown_08h: node.u64("SRVUnknown08", 0), ..Default::default() };
        Ok(Self { base, srv, ..Default::default() })
    }

    pub fn name(&self) -> Option<&str> {
        self.base.name.as_deref()
    }
}

/// `Rsc8TextureDictionary` (64 bytes): textures sorted by the hash of their
/// name, and the hashes.
#[derive(Clone, Debug, PartialEq)]
pub struct TextureDictionary {
    pub vft: u64,
    /// The resource's block map, when the dictionary is a file's root.
    pub block_map: Option<BlockMap>,
    pub unknown_10h: u64,
    pub unknown_18h: u64,
    /// `None` for a null pointer or no hashes, as the C# reads them.
    pub hashes: Option<Vec<u32>>,
    pub hashes_capacity: u16,
    pub hashes_unknown: u32,
    /// `None` for a null pointer or no textures, as the C# reads them; null
    /// texture pointers are `None` items.
    pub textures: Option<Vec<Option<Texture>>>,
    pub textures_capacity: u16,
    pub textures_unknown: u32,
}

impl Default for TextureDictionary {
    fn default() -> Self {
        Self {
            vft: 0,
            block_map: None,
            unknown_10h: 0,
            unknown_18h: 1,
            hashes: None,
            hashes_capacity: 0,
            hashes_unknown: 0,
            textures: None,
            textures_capacity: 0,
            textures_unknown: 0,
        }
    }
}

impl TextureDictionary {
    pub const BLOCK_LENGTH: usize = 64;

    /// Reads the 64 bytes at the reader's position and the textures.
    pub fn read(r: &mut Rsc8Reader, hashes: &HashIndex) -> Result<Self> {
        let vft = r.u64()?;
        let block_map = r.ptr(BlockMap::read)?;
        let unknown_10h = r.u64()?;
        let unknown_18h = r.u64()?;
        let (hashes_ptr, hashes_count, hashes_capacity, hashes_unknown) = (r.u64()?, r.u16()?, r.u16()?, r.u32()?);
        let (tex_ptr, tex_count, textures_capacity, textures_unknown) = (r.u64()?, r.u16()?, r.u16()?, r.u32()?);
        // ReadArray gives null for no items.
        let nonempty = |ptr: u64, count: u16| if count == 0 { 0 } else { ptr };
        let hash_list = r.array_at::<u32>(nonempty(hashes_ptr, hashes_count), hashes_count as usize)?;
        let textures = match r.array_at::<u64>(nonempty(tex_ptr, tex_count), tex_count as usize)? {
            None => None,
            Some(ptrs) => {
                let mut v = Vec::with_capacity(ptrs.len());
                for p in ptrs {
                    v.push(r.follow(p, |r| Texture::read(r, hashes))?);
                }
                Some(v)
            }
        };
        Ok(Self {
            vft,
            block_map,
            unknown_10h,
            unknown_18h,
            hashes: hash_list,
            hashes_capacity,
            hashes_unknown,
            textures,
            textures_capacity,
            textures_unknown,
        })
    }

    /// `Build`: a dictionary of these textures, sorted by the hash of their name.
    pub fn build(textures: Vec<Texture>) -> Self {
        let mut list: Vec<(u32, Texture)> =
            textures.into_iter().map(|t| (t.name().map(joaat).unwrap_or(0), t)).collect();
        list.sort_by_key(|(h, _)| *h);
        let n = list.len() as u16;
        let (hashes, textures): (Vec<u32>, Vec<Option<Texture>>) = list.into_iter().map(|(h, t)| (h, Some(t))).unzip();
        Self {
            hashes: Some(hashes),
            hashes_capacity: n,
            textures: Some(textures),
            textures_capacity: n,
            ..Default::default()
        }
    }

    /// Writes the dictionary, its textures and (if it has one) the block map.
    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        self.write_with_textures(w).0
    }

    /// [`write`](Self::write), also returning each texture's block (`None` for
    /// a null texture), for what points to them (drawables' shader parameters).
    pub fn write_with_textures(&self, w: &mut Rsc8Writer) -> (BlockId, Vec<Option<BlockId>>) {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u64(self.vft);
        let bm = self.block_map.as_ref().map(|m| m.write(w));
        b.ptr(bm);
        b.u64(self.unknown_10h);
        b.u64(self.unknown_18h);

        let hashes = w.array(self.hashes.as_deref());
        b.ptr(hashes);
        b.u16(self.hashes.as_ref().map_or(0, |h| h.len() as u16));
        b.u16(self.hashes_capacity);
        b.u32(self.hashes_unknown);

        // Rsc8PtrArr: room for the capacity, and a null pointer when that's 0.
        let count = self.textures.as_ref().map_or(0, |t| t.len() as u16);
        let slots = count.max(self.textures_capacity) as usize;
        let mut texture_ids = Vec::new();
        let ptrs = match &self.textures {
            Some(items) if slots > 0 => {
                let mut ids: Vec<Option<BlockId>> = items.iter().map(|t| t.as_ref().map(|t| t.write(w))).collect();
                texture_ids = ids.clone();
                ids.resize(slots, None);
                Some(w.pointer_array(&ids))
            }
            _ => None,
        };
        b.ptr(ptrs);
        b.u16(count);
        b.u16(self.textures_capacity);
        b.u32(self.textures_unknown);
        w.set(id, b, false);
        (id, texture_ids)
    }

    /// The dictionary's XML: a version attribute and the textures.
    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.u32("@version", 1);
        w.node_array_opt("Textures", self.textures.as_deref(), |w, t| t.write_xml(w));
    }

    /// Reads a dictionary's XML. Like the C#, there must be a `Textures` node.
    pub fn read_xml(node: XmlNode) -> Result<Self> {
        let Some(items) = node.node_array("Textures") else { bail!("the texture dictionary has no Textures") };
        let mut textures = Vec::with_capacity(items.len());
        for item in items {
            let Some(item) = item else { bail!("a null texture in the texture dictionary") };
            textures.push(Texture::read_xml(item)?);
        }
        Ok(Self::build(textures))
    }
}

pub fn to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let td = read_resource(file, |r, _| TextureDictionary::read(r, cx.hashes))?;
    let mut errors = Vec::new();
    let xml = XmlWriter::document(cx.hashes, ROOT, cx.folder.clone(), |w| {
        td.write_xml(w);
        errors = std::mem::take(&mut w.external_errors);
    });
    if !errors.is_empty() {
        bail!("{}", errors.join("; "));
    }
    Ok(xml)
}

pub fn from_xml(xml: &str, cx: &XmlContext) -> Result<Vec<u8>> {
    let doc = XmlDocument::new(xml);
    let mut td = doc.with_root(cx.folder.as_deref(), TextureDictionary::read_xml)??;
    // YtdFile.Save
    td.block_map.get_or_insert_default();
    let mut w = Rsc8Writer::new();
    td.write(&mut w);
    Ok(w.build(2))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texture(name: &str, format: TextureFormat, size: (u16, u16, u16), mips: u8, dim: u8) -> Texture {
        let (width, height, depth) = size;
        let fmt = format.dxgi();
        let mut len = 0;
        for i in 0..mips as usize {
            len += compute_pitch(fmt, mip_size(width, i).unwrap(), mip_size(height, i).unwrap()).unwrap().1;
        }
        let data = (0..len * depth as i32).map(|i| (i * 7 + 3) as u8).collect();
        let file_format = from_engine_format(format);
        let base = TextureBase {
            name: Some(name.into()),
            format,
            width,
            height,
            depth,
            mip_levels: mips,
            dimension: dim,
            tile_mode: tile_mode::STANDARD,
            flags: 0x1800_8002,
            file_format,
            block_stride: block_stride(file_format),
            block_count: block_count(file_format, width, height, depth, mips, 0x1800_8002),
            data: Some(data),
            ..Default::default()
        };
        let srv =
            ShaderResourceView { dimension: srv_dimension::TEXTURE_2D, unknown_08h: 0x47b_0080, ..Default::default() };
        Texture { base, srv, ..Default::default() }
    }

    fn sample() -> TextureDictionary {
        TextureDictionary::build(vec![
            texture("rock_d", TextureFormat::BC1, (64, 32, 1), 7, dimension::TEXTURE_2D),
            texture("Sky_Cube", TextureFormat::A8, (16, 16, 6), 5, dimension::TEXTURE_CUBE),
            texture("lut", TextureFormat::BC7SRGB, (8, 8, 4), 1, dimension::TEXTURE_3D),
        ])
    }

    #[test]
    fn block_counts_like_the_game() {
        assert_eq!(block_count(bf::BC1_UNORM, 512, 512, 1, 10, 0x1800_8002), 22144);
        assert_eq!(block_count(bf::A8_UNORM, 4, 4, 1, 1, 0), 256);
        assert_eq!(block_count(bf::BC1_UNORM, 4, 4, 1, 1, 0x1822_8002), 64 + 0x2000);
    }

    #[test]
    fn enums_like_dotnet() {
        assert_eq!(enum_name(TILE_MODE_NAMES, 255), "Auto");
        assert_eq!(enum_name(DIMENSION_NAMES, 7), "7");
        assert_eq!(parse_enum(DIMENSION_NAMES, Some("TextureCube")), Some(2));
        assert_eq!(parse_enum(DIMENSION_NAMES, Some(" 3 ")), Some(3));
        assert_eq!(parse_enum(DIMENSION_NAMES, Some("texturecube")), None);
        assert_eq!(parse_enum(DIMENSION_NAMES, None), None);
    }

    #[test]
    fn binary_round_trip() {
        let td = sample();
        let names: Vec<_> = td.textures.iter().flatten().flatten().map(|t| t.name().unwrap().to_owned()).collect();
        assert_eq!(td.hashes.as_ref().unwrap(), &names.iter().map(|n| joaat(n)).collect::<Vec<_>>());
        assert!(td.hashes.as_ref().unwrap().is_sorted());

        let mut root = td.clone();
        root.block_map = Some(BlockMap::default());
        let mut w = Rsc8Writer::new();
        root.write(&mut w);
        let file = w.build(2);
        let hashes = HashIndex::new();
        let back = read_resource(&file, |r, _| TextureDictionary::read(r, &hashes)).unwrap();
        assert_eq!(back.hashes, td.hashes);
        assert_eq!(back.block_map.unwrap().physical_pages, 1);
        for (a, b) in td.textures.iter().flatten().flatten().zip(back.textures.iter().flatten().flatten()) {
            let mut b = b.clone();
            // Read back, the SRV pointer is followed and the stride computed.
            assert_eq!(b.base.srv_ref.take(), Some(b.srv.clone()));
            b.base.stride = 0;
            assert_eq!(&b, a);
        }
        assert_eq!(hashes.get(joaat("Sky_Cube")).as_deref(), Some("Sky_Cube"));
    }

    #[test]
    fn xml_round_trip_with_dds() {
        let dir = std::env::temp_dir().join(format!("rf-ytd-test-{}", std::process::id()));
        let hashes = HashIndex::new();
        let td = sample();
        let xml = XmlWriter::document(&hashes, ROOT, Some(dir.clone()), |w| td.write_xml(w));
        assert!(xml.contains("<Dimension>TextureCube</Dimension>"), "{xml}");
        assert!(xml.contains("<SRVUnknown08 value=\"75169920\" />"), "{xml}");
        assert!(dir.join("Sky_Cube.dds").is_file());

        let doc = XmlDocument::new(xml.clone());
        let back = doc.with_root(Some(&dir), TextureDictionary::read_xml).unwrap().unwrap();
        for (a, b) in td.textures.iter().flatten().flatten().zip(back.textures.iter().flatten().flatten()) {
            let (a, b) = (&a.base, &b.base);
            assert_eq!(b.data, a.data);
            assert_eq!((b.width, b.height, b.depth, b.mip_levels), (a.width, a.height, a.depth, a.mip_levels));
            assert_eq!(
                (b.format, b.file_format, b.block_count, b.dimension),
                (a.format, a.file_format, a.block_count, a.dimension)
            );
        }
        let again = XmlWriter::document(&hashes, ROOT, None, |w| back.write_xml(w));
        assert_eq!(again, xml);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
