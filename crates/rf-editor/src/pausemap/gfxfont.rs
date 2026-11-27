//! The game's own fonts, from its Scaleform font library
//! (`scaleform_frontend.rpf/font_lib_efigs.gfx`): RDR Lino, Hapna Slab Serif
//! and the others, in Scaleform's compacted font format (tag 1005), which
//! nothing documents. Found by reading the files:
//!
//! - The `.gfx` is a SWF with a `GFX` signature (`CFX`: zlib after the
//!   first 8 bytes); tags as in SWF.
//! - Tag 1005 (DefineCompactedFont): the font's id (u16), then from its
//!   name on what's below; offsets count from the name's first byte.
//! - Name (NUL-terminated), flags (u16; 2 is bold), nominal size (u16, the
//!   em: 256), ascent, descent, leading (i16), glyph count (u32), glyph data
//!   size (u32), the glyph data, then per glyph: code (u16), advance (i16),
//!   offset (u32); then the kerning pairs (count as a u30, then char, char,
//!   adjustment as u16, u16, i16).
//! - A glyph: its box (4 s15), its contour count (u15), and each contour:
//!   the start (2 s15), the edge count (u30, times 2; an odd one is instead
//!   the offset of another contour's edge count and edges, shared), and the
//!   edges.
//! - s15/u15: one byte (bit 0 clear, 7 bits) or two (bit 0 set, 15 bits).
//!   u30: 1 to 4 bytes, their count less one in the first's low 2 bits.
//! - An edge: little-endian bits, the low 4 its kind, then signed fields:
//!   0/1 horizontal (12/20 bits), 2/3 vertical (12/20), 4-7 a line (two of 6,
//!   10, 14, 18), 8-15 a quadratic curve to its control point and from it to
//!   the end (four of 5, 7, ... 19 bits).
//! - Coordinates are y downwards, the baseline at 0.
//!
//! Checked by drawing RDR Lino, Hapna Slab Serif DemiBold and Redemption, and
//! by every glyph's data ending where the next begins.

use std::collections::HashMap;

use super::fonts::Glyph;
use super::path::Point;

/// The game's font library, patched first.
pub const LIBRARIES: [&str; 2] = [
    "update_4.rpf/x64/patch/data/cdimages/scaleform_frontend.rpf/font_lib_efigs.gfx",
    "data_0.rpf/data/cdimages/scaleform_frontend.rpf/font_lib_efigs.gfx",
];

/// Curves are cut into this many chords.
const CURVE_STEPS: usize = 8;

pub struct GameFont {
    pub name: String,
    pub bold: bool,
    pub glyphs: HashMap<char, Glyph>,
}

struct Reader<'a> {
    d: &'a [u8],
    p: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.d.get(self.p)?;
        self.p += 1;
        Some(v)
    }
    fn s15(&mut self) -> Option<i32> {
        let b0 = self.u8()? as i32;
        if b0 & 1 == 0 {
            let v = b0 >> 1;
            return Some(if v & 0x40 != 0 { v - 0x80 } else { v });
        }
        let v = (b0 | (self.u8()? as i32) << 8) >> 1;
        Some(if v & 0x4000 != 0 { v - 0x8000 } else { v })
    }
    fn u15(&mut self) -> Option<u32> {
        let b0 = self.u8()? as u32;
        if b0 & 1 == 0 {
            return Some(b0 >> 1);
        }
        Some((b0 | (self.u8()? as u32) << 8) >> 1)
    }
    fn u30(&mut self) -> Option<u32> {
        let n = (*self.d.get(self.p)? & 3) as usize + 1;
        let bytes = self.d.get(self.p..self.p + n)?;
        self.p += n;
        Some(bytes.iter().rev().fold(0u32, |v, &b| v << 8 | b as u32) >> 2)
    }
}

fn signed(v: u64, bits: u32) -> i32 {
    let v = (v & ((1 << bits) - 1)) as i64;
    (if v & (1 << (bits - 1)) != 0 { v - (1 << bits) } else { v }) as i32
}

/// Bytes each edge kind takes.
const EDGE_BYTES: [usize; 16] = [2, 3, 2, 3, 2, 3, 4, 5, 3, 4, 5, 6, 7, 8, 9, 10];

/// Reads `n` edges from `(x, y)`, adding the points (curves cut in chords),
/// in font units.
fn edges(r: &mut Reader, n: u32, mut x: i32, mut y: i32, pts: &mut Vec<Point>) -> Option<()> {
    for _ in 0..n {
        let kind = (*r.d.get(r.p)? & 15) as usize;
        let len = EDGE_BYTES[kind];
        let bytes = r.d.get(r.p..r.p + len)?;
        r.p += len;
        let v = (bytes.iter().rev().fold(0u128, |v, &b| v << 8 | b as u128) >> 4) as u64;
        match kind {
            0 | 1 => x += signed(v, if kind == 0 { 12 } else { 20 }),
            2 | 3 => y += signed(v, if kind == 2 { 12 } else { 20 }),
            4..=7 => {
                let bits = [6, 10, 14, 18][kind - 4];
                x += signed(v, bits);
                y += signed(v >> bits, bits);
            }
            _ => {
                let bits = [5, 7, 9, 11, 13, 15, 17, 19][kind - 8];
                let f = |i: u32| signed(v >> (bits * i), bits);
                let (cx, cy) = (x + f(0), y + f(1));
                let (ax, ay) = (cx + f(2), cy + f(3));
                for s in 1..CURVE_STEPS {
                    let t = s as f64 / CURVE_STEPS as f64;
                    let u = 1.0 - t;
                    pts.push([
                        u * u * x as f64 + 2.0 * u * t * cx as f64 + t * t * ax as f64,
                        u * u * y as f64 + 2.0 * u * t * cy as f64 + t * t * ay as f64,
                    ]);
                }
                (x, y) = (ax, ay);
            }
        }
        pts.push([x as f64, y as f64]);
    }
    Some(())
}

fn glyph(d: &[u8], at: usize, base: usize, nominal: f64) -> Option<Vec<Vec<Point>>> {
    let mut r = Reader { d, p: at.checked_sub(base)? };
    for _ in 0..4 {
        r.s15()?;
    }
    let count = r.u15()?;
    let mut contours = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let (x, y) = (r.s15()?, r.s15()?);
        let n = r.u30()?;
        let mut pts = vec![[x as f64, y as f64]];
        if n & 1 == 1 {
            let mut shared = Reader { d, p: ((n >> 1) as usize).checked_sub(base)? };
            let m = shared.u30()? >> 1;
            edges(&mut shared, m, x, y, &mut pts)?;
        } else {
            edges(&mut r, n >> 1, x, y, &mut pts)?;
        }
        contours.push(pts.into_iter().map(|[x, y]| [x / nominal, y / nominal]).collect());
    }
    Some(contours)
}

/// One compacted font (a tag 1005's data after its id).
fn font(body: &[u8]) -> Option<GameFont> {
    let end = body.iter().position(|&b| b == 0)?;
    let name = String::from_utf8_lossy(&body[..end]).split_whitespace().collect::<Vec<_>>().join(" ");
    let h = &body[end + 1..];
    let u16_at = |i: usize| Some(u16::from_le_bytes(h.get(i..i + 2)?.try_into().ok()?));
    let u32_at = |i: usize| Some(u32::from_le_bytes(h.get(i..i + 4)?.try_into().ok()?));
    let flags = u16_at(0)?;
    let nominal = u16_at(2)? as f64;
    let count = u32_at(10)? as usize;
    let data_len = u32_at(14)? as usize;
    let data = h.get(18..18 + data_len)?;
    // Offsets count from the name: the glyph data starts this far in.
    let base = end + 1 + 18;
    let table = 18 + data_len;
    let mut glyphs = HashMap::with_capacity(count);
    for i in 0..count {
        let e = table + i * 8;
        let code = u16_at(e)?;
        let advance = u16_at(e + 2)? as i16 as f64;
        let offset = u32_at(e + 4)? as usize;
        let Some(ch) = char::from_u32(code as u32) else { continue };
        let contours = glyph(data, offset, base, nominal)?;
        glyphs.insert(ch, Glyph { contours, advance: advance / nominal, even_odd: true });
    }
    Some(GameFont { name, bold: flags & 2 != 0, glyphs })
}

/// The fonts in a Scaleform library; `Err` when it isn't one.
pub fn read_library(file: &[u8]) -> Result<Vec<GameFont>, String> {
    let inflated;
    let d: &[u8] = match file.get(..3) {
        Some(b"GFX") | Some(b"FWS") => file,
        Some(b"CFX") | Some(b"CWS") => {
            let mut out = file[..8].to_vec();
            let mut z = flate2::read::ZlibDecoder::new(&file[8..]);
            std::io::Read::read_to_end(&mut z, &mut out).map_err(|e| e.to_string())?;
            inflated = out;
            &inflated
        }
        _ => return Err("not a Scaleform file".into()),
    };
    let rect_bits = 5 + 4 * (*d.get(8).ok_or("too short")? >> 3) as usize;
    let mut p = 8 + rect_bits.div_ceil(8) + 4;
    let mut fonts = Vec::new();
    while p + 2 <= d.len() {
        let h = u16::from_le_bytes([d[p], d[p + 1]]);
        p += 2;
        let (code, mut len) = (h >> 6, (h & 0x3f) as usize);
        if len == 0x3f {
            len = u32::from_le_bytes(d.get(p..p + 4).ok_or("cut short")?.try_into().unwrap()) as usize;
            p += 4;
        }
        let body = d.get(p..p + len).ok_or("cut short")?;
        p += len;
        match code {
            0 => break,
            1005 if body.len() > 2 => match font(&body[2..]) {
                Some(f) => fonts.push(f),
                None => log::warn!("a font of the game's that couldn't be read"),
            },
            _ => {}
        }
    }
    Ok(fonts)
}
