//! The engine's vertex layout (`VertexLayout.cs`) and the text form of vertex
//! data in XML (`Mesh.GetVertexDataString`/`SetVertexDataString`), with the
//! packed formats' conversions from `FloatUtil.cs`.

use crate::xml::number::{parse_f32, push_f32};
use crate::{Result, bail};

/// `VertexElementFormat`: the engine's element formats, one per nibble of
/// [`VertexLayout::formats`].
pub mod vertex_format {
    pub const NONE: u8 = 0;
    pub const FLOAT: u8 = 1;
    pub const FLOAT2: u8 = 2;
    pub const FLOAT3: u8 = 3;
    pub const FLOAT4: u8 = 4;
    pub const COLOUR: u8 = 5;
    pub const UBYTE4: u8 = 6;
    pub const HALF2: u8 = 7;
    pub const HALF4: u8 = 8;
    /// Signed, normalized 10:10:10:2.
    pub const DEC3N: u8 = 9;
    /// Signed, normalized.
    pub const SHORT4N: u8 = 10;
    /// Integers, e.g. bone indices.
    pub const USHORT2: u8 = 11;
    /// Signed, normalized.
    pub const SHORT2N: u8 = 12;
}

use vertex_format as vf;

/// `VertexElementFormats.GetSizeInBytes`.
pub fn format_size(f: u8) -> usize {
    match f {
        vf::FLOAT | vf::COLOUR | vf::UBYTE4 | vf::HALF2 | vf::DEC3N | vf::USHORT2 | vf::SHORT2N => 4,
        vf::FLOAT2 | vf::HALF4 | vf::SHORT4N => 8,
        vf::FLOAT3 => 12,
        vf::FLOAT4 => 16,
        _ => 0,
    }
}

/// The engine's semantics, one per 3 bits of each nibble of
/// [`VertexLayout::semantics`].
pub mod semantic {
    pub const POSITION: u8 = 0;
    pub const BLEND_WEIGHTS: u8 = 1;
    pub const BLEND_INDICES: u8 = 2;
    pub const NORMAL: u8 = 3;
    pub const COLOR: u8 = 4;
    pub const TEXCOORD: u8 = 5;
    pub const TANGENT: u8 = 6;
    pub const BINORMAL: u8 = 7;
}

const FORMAT_CODES: &[u8; 16] = b"0123456789ABCDEF";
const SEMANTIC_CODES: &[u8; 8] = b"PWINCTXY";

/// `VertexLayout`: up to 16 elements, each a format nibble and a semantic
/// nibble (3 bits). Bit 3 of the semantics marks non-interleaved data, where
/// each element's values are stored together, element after element.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct VertexLayout {
    pub formats: u64,
    pub semantics: u64,
}

impl VertexLayout {
    pub fn is_empty(&self) -> bool {
        self.formats == 0
    }

    pub fn non_interleaved(&self) -> bool {
        self.semantics & 8 != 0
    }

    pub fn set_non_interleaved(&mut self, v: bool) {
        self.semantics = (self.semantics & !8) | if v { 8 } else { 0 };
    }

    /// The elements up to the first `NONE` format.
    pub fn element_count(&self) -> usize {
        (0..16).take_while(|i| (self.formats >> (i * 4)) & 0xF != 0).count()
    }

    /// The format of element `e` (see [`vertex_format`]).
    pub fn format(&self, e: usize) -> u8 {
        if e < 16 { ((self.formats >> (e * 4)) & 0xF) as u8 } else { vf::FLOAT3 }
    }

    /// The semantic of element `e` (see [`semantic`]).
    pub fn semantic(&self, e: usize) -> u8 {
        if e < 16 { ((self.semantics >> (e * 4)) & 0x7) as u8 } else { semantic::POSITION }
    }

    /// The size of one vertex: the elements up to the first `NONE` format.
    pub fn stride(&self) -> usize {
        (0..self.element_count()).map(|e| format_size(self.format(e))).sum()
    }

    /// Where element `e` starts within a vertex (`GetElementOffset`).
    pub fn element_offset(&self, e: usize) -> usize {
        (0..e.min(16)).map(|i| format_size(self.format(i))).sum()
    }

    /// The first `POSITION` element: its offset and format
    /// (`GetPositionOffset`/`GetPositionFormat`).
    pub fn position(&self) -> Option<(usize, u8)> {
        (0..16).find(|&e| self.semantic(e) == semantic::POSITION).map(|e| (self.element_offset(e), self.format(e)))
    }

    /// `SemanticsString`, as in XML: `PNCT`...
    pub fn semantics_string(&self) -> String {
        (0..self.element_count()).map(|e| SEMANTIC_CODES[self.semantic(e) as usize] as char).collect()
    }

    /// `FormatsString`, as in XML: one hex digit per element.
    pub fn formats_string(&self) -> String {
        (0..self.element_count()).map(|e| FORMAT_CODES[self.format(e) as usize] as char).collect()
    }

    /// The `SemanticsString`/`FormatsString` setters: up to 16 characters,
    /// unknown ones as 0 (`POSITION`, `NONE`).
    pub fn from_strings(semantics: &str, formats: &str) -> Self {
        let mut l = Self::default();
        for (i, c) in semantics.chars().take(16).enumerate() {
            let s = SEMANTIC_CODES.iter().position(|&x| x as char == c).unwrap_or(0) as u64;
            l.semantics = l.semantics.wrapping_add((s & 7) << (i * 4));
        }
        for (i, c) in formats.chars().take(16).enumerate() {
            let f = FORMAT_CODES.iter().position(|&x| x as char == c).unwrap_or(0) as u64;
            l.formats = l.formats.wrapping_add((f & 0xF) << (i * 4));
        }
        l
    }
}

// ----- packed formats (FloatUtil) -----

/// `System.Half` to float (exact).
pub fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = (h >> 10) & 0x1F;
    let man = (h & 0x3FF) as u32;
    match exp {
        0 => sign * man as f32 * (1.0 / 16_777_216.0),
        0x1F if man == 0 => sign * f32::INFINITY,
        0x1F => f32::from_bits(((h as u32 & 0x8000) << 16) | 0x7FC0_0000 | (man << 13)),
        _ => f32::from_bits(((h as u32 & 0x8000) << 16) | ((exp as u32 + 112) << 23) | (man << 13)),
    }
}

/// Float to `System.Half`, rounding to nearest even.
pub fn f32_to_half(value: f32) -> u16 {
    let x = value.to_bits();
    let sign = x & 0x8000_0000;
    let exp = x & 0x7F80_0000;
    let man = x & 0x007F_FFFF;
    let half_sign = sign >> 16;
    if exp == 0x7F80_0000 {
        let nan_bit = if man == 0 { 0 } else { 0x0200 };
        return (half_sign | 0x7C00 | nan_bit | (man >> 13)) as u16;
    }
    let half_exp = ((exp >> 23) as i32) - 127 + 15;
    if half_exp >= 0x1F {
        return (half_sign | 0x7C00) as u16;
    }
    if half_exp <= 0 {
        if 14 - half_exp > 24 {
            return half_sign as u16;
        }
        let man = man | 0x0080_0000;
        let mut half_man = man >> (14 - half_exp);
        let round_bit = 1 << (13 - half_exp);
        if (man & round_bit) != 0 && (man & (3 * round_bit - 1)) != 0 {
            half_man += 1;
        }
        return (half_sign | half_man) as u16;
    }
    let v = half_sign | ((half_exp as u32) << 10) | (man >> 13);
    let round_bit = 0x1000;
    if (man & round_bit) != 0 && (man & (3 * round_bit - 1)) != 0 { (v + 1) as u16 } else { v as u16 }
}

/// `FloatUtil.Dec3NToVector4`.
pub fn dec3n_to_vec4(u: u32) -> [f32; 4] {
    let comp = |v: u32| {
        let s = v & 0x200 != 0;
        ((if s { v } else { !v }) & 0x1FF) as f32 / if s { 511.0 } else { -511.0 }
    };
    let uw = (u >> 30) & 3;
    [comp(u & 0x3FF), comp((u >> 10) & 0x3FF), comp((u >> 20) & 0x3FF), if uw == 3 { -1.0 } else { uw as f32 }]
}

/// `FloatUtil.Vector4ToDec3N`.
pub fn vec4_to_dec3n(v: [f32; 4]) -> u32 {
    let comp = |f: f32| {
        let s = f >= 0.0;
        let x = ((f.abs() * 511.0) as u32).min(511);
        ((if s { x } else { !x }) & 0x1FF) + if s { 0x200 } else { 0 }
    };
    let w = (v[3] as u32).min(2);
    let uw = if v[3] >= 0.0 { w } else { 3 };
    comp(v[0]).wrapping_add(comp(v[1]) << 10).wrapping_add(comp(v[2]) << 20).wrapping_add(uw << 30)
}

fn short_n(u: u64) -> f32 {
    let s = u & 0x8000 != 0;
    ((if s { !u } else { u }) & 0x7FFF) as f32 / if s { -32767.0 } else { 32767.0 }
}

/// `FloatUtil.Short4NToVector4`.
pub fn short4n_to_vec4(u: u64) -> [f32; 4] {
    [short_n(u), short_n(u >> 16), short_n(u >> 32), short_n(u >> 48)]
}

/// `FloatUtil.Short2NToVector2`.
pub fn short2n_to_vec2(u: u32) -> [f32; 2] {
    [short_n(u as u64), short_n((u >> 16) as u64)]
}

/// One component of `Vector4ToShort4N`/`Vector2ToShort2N`, with the sign `s`
/// given separately: the C# takes every component's sign from X.
fn to_short_n(f: f32, s: bool) -> u64 {
    let x = ((f.abs() * 32767.0) as u32).min(32767) as u64;
    ((if s { !x } else { x }) & 0x7FFF) + if s { 0x8000 } else { 0 }
}

/// `FloatUtil.Vector4ToShort4N`, which takes every component's sign from X.
pub fn vec4_to_short4n(v: [f32; 4]) -> u64 {
    let s = v[0] < 0.0;
    to_short_n(v[0], s) + (to_short_n(v[1], s) << 16) + (to_short_n(v[2], s) << 32) + (to_short_n(v[3], s) << 48)
}

/// `FloatUtil.Vector2ToShort2N`, which takes both signs from X.
pub fn vec2_to_short2n(v: [f32; 2]) -> u32 {
    let s = v[0] < 0.0;
    (to_short_n(v[0], s) + (to_short_n(v[1], s) << 16)) as u32
}

/// `FloatUtil.UShort2ToVector2`.
pub fn ushort2_to_vec2(u: u32) -> [f32; 2] {
    [(u & 0xFFFF) as f32 / 65535.0, (u >> 16) as f32 / 65535.0]
}

// ----- element values -----

fn read_le<const N: usize>(data: &[u8], o: usize) -> Result<[u8; N]> {
    // The C# reads past the end of the array without a check once the start is
    // in it; what's there can't be reproduced, zeros are the closest.
    if o >= data.len() {
        bail!("vertex data too short: element at {o} of {}", data.len());
    }
    let mut b = [0u8; N];
    let n = (data.len() - o).min(N);
    b[..n].copy_from_slice(&data[o..o + n]);
    Ok(b)
}

fn f32_at(data: &[u8], o: usize) -> Result<f32> {
    Ok(f32::from_le_bytes(read_le(data, o)?))
}

/// `Mesh.GetVertexValue`: element value at byte offset `o`, as up to 4 floats
/// (colours as 0-1, integers as they are).
pub fn element_value(data: &[u8], o: usize, f: u8) -> Result<[f32; 4]> {
    let u32_at = |o| read_le::<4>(data, o).map(u32::from_le_bytes);
    let u16_at = |o| read_le::<2>(data, o).map(u16::from_le_bytes);
    Ok(match f {
        vf::FLOAT => [f32_at(data, o)?, 0.0, 0.0, 0.0],
        vf::FLOAT2 => [f32_at(data, o)?, f32_at(data, o + 4)?, 0.0, 0.0],
        vf::FLOAT3 => [f32_at(data, o)?, f32_at(data, o + 4)?, f32_at(data, o + 8)?, 0.0],
        vf::FLOAT4 => [f32_at(data, o)?, f32_at(data, o + 4)?, f32_at(data, o + 8)?, f32_at(data, o + 12)?],
        vf::COLOUR | vf::UBYTE4 => read_le::<4>(data, o)?.map(|b| b as f32 / 255.0),
        vf::HALF2 => [half_to_f32(u16_at(o)?), half_to_f32(u16_at(o + 2)?), 0.0, 0.0],
        vf::HALF4 => [
            half_to_f32(u16_at(o)?),
            half_to_f32(u16_at(o + 2)?),
            half_to_f32(u16_at(o + 4)?),
            half_to_f32(u16_at(o + 6)?),
        ],
        vf::DEC3N => dec3n_to_vec4(u32_at(o)?),
        vf::SHORT4N => short4n_to_vec4(u64::from_le_bytes(read_le(data, o)?)),
        vf::USHORT2 => {
            let [x, y] = ushort2_to_vec2(u32_at(o)?);
            [x, y, 0.0, 0.0]
        }
        vf::SHORT2N => {
            let [x, y] = short2n_to_vec2(u32_at(o)?);
            [x, y, 0.0, 0.0]
        }
        _ => [0.0; 4],
    })
}

fn push_floats(out: &mut String, v: &[f32]) {
    for (i, f) in v.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        push_f32(out, *f);
    }
}

/// `Mesh.GetVertexElementString`: the element at byte offset `o`, components
/// separated by spaces.
pub fn push_element_string(out: &mut String, data: &[u8], o: usize, f: u8) -> Result<()> {
    match f {
        vf::FLOAT => push_f32(out, f32_at(data, o)?),
        vf::FLOAT2 | vf::FLOAT3 | vf::FLOAT4 | vf::HALF2 | vf::HALF4 | vf::DEC3N | vf::SHORT4N | vf::SHORT2N => {
            let n = match f {
                vf::FLOAT2 | vf::HALF2 | vf::SHORT2N => 2,
                vf::FLOAT3 => 3,
                _ => 4,
            };
            let v = element_value(data, o, f)?;
            push_floats(out, &v[..n]);
        }
        vf::COLOUR | vf::UBYTE4 => {
            let [r, g, b, a] = read_le::<4>(data, o)?;
            out.push_str(&format!("{r} {g} {b} {a}"));
        }
        vf::USHORT2 => {
            let u = u32::from_le_bytes(read_le(data, o)?);
            out.push_str(&format!("{} {}", u & 0xFFFF, u >> 16));
        }
        _ => {}
    }
    Ok(())
}

/// `FloatUtil.ParseVector4String(s, ' ')` and friends: split on single
/// spaces (empty parts parse as 0), at most `N` components.
fn parse_floats<const N: usize>(s: &str) -> [f32; N] {
    let mut v = [0.0; N];
    if s.is_empty() {
        return v;
    }
    for (d, part) in v.iter_mut().zip(s.split(' ')) {
        *d = parse_f32(part.trim()).unwrap_or(0.0);
    }
    v
}

fn parse_u8s(s: &str) -> [u8; 4] {
    let mut v = [0u8; 4];
    if s.is_empty() {
        return v;
    }
    for (d, part) in v.iter_mut().zip(s.split(' ')) {
        let t = part.trim();
        *d = t.strip_prefix('+').unwrap_or(t).parse().unwrap_or(0);
    }
    v
}

/// `Mesh.SetVertexElementString`: the bytes of one element parsed from text.
pub fn parse_element(s: &str, f: u8) -> Vec<u8> {
    let floats = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
    match f {
        vf::FLOAT => parse_f32(s).unwrap_or(0.0).to_le_bytes().to_vec(),
        vf::FLOAT2 => floats(&parse_floats::<2>(s)),
        vf::FLOAT3 => floats(&parse_floats::<3>(s)),
        vf::FLOAT4 => floats(&parse_floats::<4>(s)),
        vf::COLOUR | vf::UBYTE4 => parse_u8s(s).to_vec(),
        vf::HALF2 => parse_floats::<2>(s).iter().flat_map(|f| f32_to_half(*f).to_le_bytes()).collect(),
        vf::HALF4 => parse_floats::<4>(s).iter().flat_map(|f| f32_to_half(*f).to_le_bytes()).collect(),
        vf::DEC3N => vec4_to_dec3n(parse_floats::<4>(s)).to_le_bytes().to_vec(),
        vf::SHORT4N => vec4_to_short4n(parse_floats::<4>(s)).to_le_bytes().to_vec(),
        vf::USHORT2 => {
            let mut v = [0u16; 2];
            if !s.is_empty() {
                for (d, part) in v.iter_mut().zip(s.split(' ')) {
                    let t = part.trim();
                    *d = t.strip_prefix('+').unwrap_or(t).parse().unwrap_or(0);
                }
            }
            (v[0] as u32 + ((v[1] as u32) << 16)).to_le_bytes().to_vec()
        }
        vf::SHORT2N => vec2_to_short2n(parse_floats::<2>(s)).to_le_bytes().to_vec(),
        _ => Vec::new(),
    }
}

/// Where element `e` of vertex `v` starts (`Mesh.GetVertexElementOffset`),
/// with `eo` the element's offset within a vertex and `es` its size.
pub fn element_offset(non_interleaved: bool, stride: usize, count: usize, v: usize, es: usize, eo: usize) -> usize {
    if non_interleaved { es * v + eo * count } else { stride * v + eo }
}

/// `Mesh.GetVertexDataString`: one line per vertex, elements separated by
/// tabs, each line ending in a newline.
pub fn vertex_data_string(layout: &VertexLayout, data: Option<&[u8]>, count: usize, stride: usize) -> Result<String> {
    let ec = layout.element_count();
    let mut elems = Vec::with_capacity(ec);
    let mut o = 0;
    for e in 0..ec {
        let es = format_size(layout.format(e));
        elems.push((es, o, layout.format(e)));
        o += es;
    }
    let ni = layout.non_interleaved();
    let mut out = String::with_capacity(count * ec * 24);
    if count > 0 && ec > 0 && data.is_none() {
        bail!("a mesh with {count} vertices and no vertex data (the C# fails on it)");
    }
    let data = data.unwrap_or(&[]);
    for v in 0..count {
        for (c, &(es, eo, f)) in elems.iter().enumerate() {
            if c > 0 {
                out.push('\t');
            }
            push_element_string(&mut out, data, element_offset(ni, stride, count, v, es, eo), f)?;
        }
        out.push('\n');
    }
    Ok(out)
}

/// `Mesh.SetVertexDataString`: the vertex data, count and stride from the
/// text (nothing for an empty layout).
pub fn parse_vertex_data(layout: &VertexLayout, text: &str) -> (Vec<u8>, usize, usize) {
    let lines: Vec<&str> = text.trim().split('\n').map(str::trim).filter(|l| !l.is_empty()).collect();
    let vc = lines.len();
    let ec = layout.element_count();
    let mut elems = Vec::with_capacity(ec);
    let mut o = 0;
    for e in 0..ec {
        let es = format_size(layout.format(e));
        elems.push((es, o, layout.format(e)));
        o += es;
    }
    let vs = layout.stride();
    let ni = layout.non_interleaved();
    let mut data = vec![0u8; vc * vs];
    for (i, line) in lines.iter().enumerate() {
        let parts: Vec<&str> = line.split('\t').map(str::trim).filter(|p| !p.is_empty()).collect();
        for (&(es, eo, f), part) in elems.iter().zip(&parts) {
            let bytes = parse_element(part, f);
            let at = element_offset(ni, vs, vc, i, es, eo);
            let n = bytes.len().min(data.len().saturating_sub(at));
            data[at..at + n].copy_from_slice(&bytes[..n]);
        }
    }
    (data, vc, vs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_strings() {
        let l = VertexLayout::from_strings("PNXCTT", "399522");
        assert_eq!(l.semantics_string(), "PNXCTT");
        assert_eq!(l.formats_string(), "399522");
        assert_eq!(l.stride(), 12 + 4 + 4 + 4 + 8 + 8);
        assert_eq!(l.element_offset(3), 20);
        let mut l2 = l;
        l2.set_non_interleaved(true);
        assert!(l2.non_interleaved() && !l.non_interleaved());
        assert_eq!(l2.semantics_string(), "PNXCTT");
    }

    #[test]
    fn halves() {
        for h in [0u16, 0x3C00, 0xBC00, 0x7BFF, 0x0001, 0x8001, 0x03FF, 0x7C00, 0x3555] {
            assert_eq!(f32_to_half(half_to_f32(h)), h, "{h:04X}");
        }
        assert_eq!(f32_to_half(1.0 + 1.0 / 2048.0), 0x3C00); // tie to even
        assert_eq!(f32_to_half(1.0 + 3.0 / 2048.0), 0x3C02);
        assert_eq!(f32_to_half(70000.0), 0x7C00);
    }

    #[test]
    fn packed_like_the_csharp() {
        let u = vec4_to_dec3n([-0.5, 0.25, 1.0, -1.0]);
        let v = dec3n_to_vec4(u);
        assert_eq!(v[3], -1.0);
        assert!((v[0] + 0.5).abs() < 0.003 && (v[1] - 0.25).abs() < 0.003 && v[2] == 1.0);
        // Every sign comes from X.
        let s = vec4_to_short4n([0.5, -0.5, 0.5, 0.5]);
        assert_eq!(short4n_to_vec4(s)[1], short4n_to_vec4(s)[0]);
        let s = vec4_to_short4n([-0.5, 0.5, 0.5, 0.5]);
        assert!(short4n_to_vec4(s).iter().all(|f| *f < 0.0));
    }

    #[test]
    fn vertex_text_round_trip() {
        let mut l = VertexLayout::from_strings("PNCTW", "39578");
        l.set_non_interleaved(true);
        let text = "1 2 3\t-1 0 0 0\t255 0 128 7\t0.5 0.25\t1 0 0.5 -2\n4 5 6\t0 1 0 1\t1 2 3 4\t0 1\t0 0 0 0\n";
        let (data, vc, vs) = parse_vertex_data(&l, text);
        assert_eq!((vc, vs), (2, 12 + 4 + 4 + 4 + 8));
        assert_eq!(vertex_data_string(&l, Some(&data), vc, vs).unwrap(), text);
    }
}
