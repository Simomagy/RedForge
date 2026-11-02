//! RSC8, RDR2's resource format: a graph of structs in two segments (virtual =
//! CPU data, physical = GPU data such as pixels), linked by pointers.
//!
//! Outside an archive a resource is a "loose" file: a 16-byte `RSC8` header
//! followed by the compressed segments.

mod reader;
mod writer;

pub use reader::{ArrayHeader, ArrayHeader32, PHYSICAL_BASE, Rsc8Reader, VIRTUAL_BASE};
pub use writer::{BlockBuf, BlockId, Rsc8Writer};

use crate::rpf8::{Rpf8Error, compression, rsc8_header};

pub const MAGIC: [u8; 4] = *b"RSC8";

/// The header of a loose resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rsc8Header {
    pub virtual_flags: u32,
    pub physical_flags: u32,
    /// 1 = deflate, 2 = Oodle Kraken.
    pub compressor: u8,
}

impl Rsc8Header {
    /// The resource version, from the low nibbles of the flags.
    pub fn version(&self) -> u32 {
        ((self.virtual_flags & 0xF) << 4) | (self.physical_flags & 0xF)
    }
    pub fn virtual_size(&self) -> usize {
        (self.virtual_flags & 0xFFFF_FFF0) as usize
    }
    pub fn physical_size(&self) -> usize {
        (self.physical_flags & 0xFFFF_FFF0) as usize
    }
}

/// The segment sizes and version packed into the two flag words.
pub fn flags_for(version: u32, virtual_size: usize, physical_size: usize) -> (u32, u32) {
    let v = (virtual_size as u32 & 0xFFFF_FFF0) | ((version >> 4) & 0xF);
    let p = (physical_size as u32 & 0xFFFF_FFF0) | (version & 0xF);
    (v, p)
}

pub fn is_loose_resource(data: &[u8]) -> bool {
    data.len() >= 16 && data[0..4] == MAGIC
}

/// Reads a loose resource's header and decompresses its segments.
pub fn read_loose(data: &[u8]) -> Result<(Rsc8Header, Vec<u8>), Rpf8Error> {
    if !is_loose_resource(data) {
        return Err(Rpf8Error::Format("no RSC8 header".into()));
    }
    let word = |i: usize| u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
    let header =
        Rsc8Header { virtual_flags: word(8), physical_flags: word(12), compressor: ((word(4) >> 8) & 0x1F) as u8 + 1 };
    let size = header.virtual_size() + header.physical_size();
    let body = &data[16..];
    let segments = match header.compressor {
        1 => compression::inflate(body, size)?,
        2 => compression::oodle_decompress(body, size)?,
        c => return Err(Rpf8Error::Decompress(format!("unknown compressor {c}"))),
    };
    if segments.len() < size {
        return Err(Rpf8Error::Format(format!("{} bytes of segments, flags say {size}", segments.len())));
    }
    Ok((header, segments))
}

/// Builds a loose resource, deflate-compressed as RedForge has always saved them.
pub fn write_loose(virtual_flags: u32, physical_flags: u32, segments: &[u8]) -> Vec<u8> {
    let version = ((virtual_flags & 0xF) << 4) | (physical_flags & 0xF);
    let mut out = rsc8_header(version, virtual_flags, physical_flags, 1).to_vec();
    out.extend_from_slice(&compression::deflate(segments));
    out
}

/// The block map every resource file points to (page counts of each segment).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockMap {
    pub unknown_0h: u64,
    pub virtual_pages: u8,
    pub physical_pages: u8,
    pub unknown_ah: u16,
    pub unknown_ch: u32,
}

impl BlockMap {
    pub fn read(r: &mut Rsc8Reader) -> crate::Result<Self> {
        Ok(Self {
            unknown_0h: r.u64()?,
            virtual_pages: r.u8()?,
            physical_pages: r.u8()?,
            unknown_ah: r.u16()?,
            unknown_ch: r.u32()?,
        })
    }

    /// Adds the block map; the page counts are filled in when the file is built.
    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let mut b = BlockBuf::with_capacity(48);
        b.u64(self.unknown_0h);
        b.u8(self.virtual_pages);
        b.u8(self.physical_pages);
        b.u16(self.unknown_ah);
        b.u32(self.unknown_ch);
        // Room for the page entries, as the C# allocates the largest size.
        b.pad_to(48);
        let id = w.add(b, false);
        w.block_map = Some(id);
        id
    }
}

/// Decompresses a loose resource and reads its root with `f`.
pub fn read_resource<T>(
    file: &[u8],
    f: impl FnOnce(&mut Rsc8Reader, &Rsc8Header) -> crate::Result<T>,
) -> crate::Result<T> {
    let (header, data) = read_loose(file)?;
    let mut r = Rsc8Reader::new(&data, header.virtual_size());
    f(&mut r, &header)
}
