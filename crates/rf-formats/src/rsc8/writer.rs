use std::collections::HashMap;

use bytemuck::Pod;

use super::reader::{PHYSICAL_BASE, VIRTUAL_BASE};
use super::{flags_for, write_loose};

/// A block that has been, or will be, added to a [`Rsc8Writer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(usize);

struct Fixup {
    /// Where in the block the pointer goes.
    at: usize,
    target: BlockId,
    /// Added to the target's address (a pointer into the middle of a block).
    add: u64,
}

/// The bytes of one struct (or array) being written, and its pointers.
#[derive(Default)]
pub struct BlockBuf {
    pub data: Vec<u8>,
    fixups: Vec<Fixup>,
}

macro_rules! write_le {
    ($($name:ident: $t:ty),*) => {$(
        pub fn $name(&mut self, v: $t) {
            self.data.extend_from_slice(&v.to_le_bytes());
        }
    )*};
}

impl BlockBuf {
    pub fn with_capacity(n: usize) -> Self {
        Self { data: Vec::with_capacity(n), fixups: Vec::new() }
    }

    write_le!(u8: u8, i8: i8, u16: u16, i16: i16, u32: u32, i32: i32, u64: u64, i64: i64, f32: f32);

    pub fn len(&self) -> usize {
        self.data.len()
    }
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.data.extend_from_slice(b);
    }
    pub fn zeros(&mut self, n: usize) {
        self.data.resize(self.data.len() + n, 0);
    }
    /// Pads with zeros up to `len` bytes.
    pub fn pad_to(&mut self, len: usize) {
        if self.data.len() < len {
            self.data.resize(len, 0);
        }
    }
    pub fn vec2(&mut self, v: [f32; 2]) {
        v.into_iter().for_each(|f| self.f32(f));
    }
    pub fn vec3(&mut self, v: [f32; 3]) {
        v.into_iter().for_each(|f| self.f32(f));
    }
    pub fn vec4(&mut self, v: [f32; 4]) {
        v.into_iter().for_each(|f| self.f32(f));
    }
    pub fn floats(&mut self, v: &[f32]) {
        v.iter().for_each(|f| self.f32(*f));
    }
    pub fn pod<T: Pod>(&mut self, v: &T) {
        self.data.extend_from_slice(bytemuck::bytes_of(v));
    }

    /// A pointer: 0, or the address of `target` once the file is laid out.
    pub fn ptr(&mut self, target: Option<BlockId>) {
        self.ptr_offset(target, 0);
    }

    /// A pointer `add` bytes into `target`.
    pub fn ptr_offset(&mut self, target: Option<BlockId>, add: u64) {
        if let Some(target) = target {
            self.fixups.push(Fixup { at: self.data.len(), target, add });
        }
        self.u64(0);
    }

    /// Overwrites a u32 already written at `at`.
    pub fn set_u32(&mut self, at: usize, v: u32) {
        self.data[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
}

struct Block {
    buf: BlockBuf,
    physical: bool,
    position: u64,
}

/// Collects a resource's blocks, lays them out and builds the file, the way
/// RedForge's C# `Rsc8DataWriter` does: one page per segment, the root block
/// first, then the others from largest to smallest, 16-byte aligned.
#[derive(Default)]
pub struct Rsc8Writer {
    blocks: Vec<Option<Block>>,
    strings: HashMap<String, BlockId>,
    /// The block map, whose page counts are filled in when the file is built.
    pub block_map: Option<BlockId>,
}

impl Rsc8Writer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserves a block to fill in later, so a struct can be added before the
    /// blocks it points to (the first block reserved is the root).
    pub fn reserve(&mut self) -> BlockId {
        self.blocks.push(None);
        BlockId(self.blocks.len() - 1)
    }

    pub fn set(&mut self, id: BlockId, buf: BlockBuf, physical: bool) {
        self.blocks[id.0] = Some(Block { buf, physical, position: 0 });
    }

    pub fn add(&mut self, buf: BlockBuf, physical: bool) -> BlockId {
        let id = self.reserve();
        self.set(id, buf, physical);
        id
    }

    /// A NUL-terminated string. Equal strings are written once.
    pub fn string(&mut self, s: &str) -> BlockId {
        if let Some(&id) = self.strings.get(s) {
            return id;
        }
        let mut b = BlockBuf::with_capacity(s.len() + 1);
        b.bytes(s.as_bytes());
        b.u8(0);
        let id = self.add(b, false);
        self.strings.insert(s.to_owned(), id);
        id
    }

    pub fn opt_string(&mut self, s: Option<&str>) -> Option<BlockId> {
        s.map(|s| self.string(s))
    }

    /// An array of plain data in the virtual segment (`None` stays a null pointer).
    pub fn array<T: Pod>(&mut self, items: Option<&[T]>) -> Option<BlockId> {
        self.array_in(items, false)
    }

    pub fn array_in<T: Pod>(&mut self, items: Option<&[T]>, physical: bool) -> Option<BlockId> {
        let items = items?;
        let mut b = BlockBuf::with_capacity(size_of_val(items));
        b.bytes(bytemuck::cast_slice(items));
        Some(self.add(b, physical))
    }

    /// An array of pointers to blocks.
    pub fn pointer_array(&mut self, targets: &[Option<BlockId>]) -> BlockId {
        let mut b = BlockBuf::with_capacity(targets.len() * 8);
        for t in targets {
            b.ptr(*t);
        }
        self.add(b, false)
    }

    /// Lays out one segment: returns its size and page count.
    fn layout(&mut self, physical: bool) -> (u64, u32) {
        let mut ids: Vec<usize> = (0..self.blocks.len())
            // Empty blocks are laid out too: like the C#, an empty array keeps a
            // non-null pointer.
            .filter(|&i| self.blocks[i].as_ref().is_some_and(|b| b.physical == physical))
            .collect();
        if ids.is_empty() {
            return (0, 0);
        }
        let mut pos = 0u64;
        // The first virtual block is the root, at address 0.
        if !physical {
            let root = ids.remove(0);
            let b = self.blocks[root].as_mut().unwrap();
            b.position = 0;
            pos = b.buf.len() as u64;
        }
        ids.sort_by_key(|&i| std::cmp::Reverse(self.blocks[i].as_ref().unwrap().buf.len()));
        for i in ids {
            pos += (16 - pos % 16) % 16;
            let b = self.blocks[i].as_mut().unwrap();
            b.position = pos;
            pos += b.buf.len() as u64;
        }
        let align = if pos > 0x8000 { 0x10000 } else { 16 };
        pos += (align - pos % align) % align;
        (pos, 1)
    }

    /// Builds the loose file: lays out the blocks, fixes up the pointers and
    /// deflates the segments behind an `RSC8` header.
    pub fn build(self, version: u32) -> Vec<u8> {
        let (vflags, pflags, data) = self.build_segments(version);
        write_loose(vflags, pflags, &data)
    }

    /// Lays out the blocks; returns the two flag words and the segments.
    pub fn build_segments(mut self, version: u32) -> (u32, u32, Vec<u8>) {
        let (vlen, vpages) = self.layout(false);
        let (plen, ppages) = self.layout(true);

        if let Some(bm) = self.block_map
            && let Some(b) = self.blocks[bm.0].as_mut()
            && b.buf.data.len() >= 10
        {
            b.buf.data[8] = vpages as u8;
            b.buf.data[9] = ppages as u8;
        }

        let address = |b: &Block| if b.physical { PHYSICAL_BASE + b.position } else { VIRTUAL_BASE + b.position };
        let addresses: Vec<u64> = self.blocks.iter().map(|b| b.as_ref().map(address).unwrap_or(0)).collect();

        let mut data = vec![0u8; (vlen + plen) as usize];
        for b in self.blocks.iter_mut().flatten() {
            for f in &b.buf.fixups {
                let target = addresses[f.target.0];
                let v = if target == 0 { 0 } else { target + f.add };
                b.buf.data[f.at..f.at + 8].copy_from_slice(&v.to_le_bytes());
            }
            let start = (b.position + if b.physical { vlen } else { 0 }) as usize;
            data[start..start + b.buf.data.len()].copy_from_slice(&b.buf.data);
        }
        let (vflags, pflags) = flags_for(version, vlen as usize, plen as usize);
        (vflags, pflags, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rsc8::{Rsc8Reader, read_loose};

    #[test]
    fn pointers_survive_layout() {
        let mut w = Rsc8Writer::new();
        let root = w.reserve();
        let name = w.string("hello");
        let nums = w.array(Some(&[1u32, 2, 3][..]));
        let pix = w.array_in(Some(&[7u8; 32][..]), true);
        let mut b = BlockBuf::default();
        b.u64(0xABCD);
        b.ptr(Some(name));
        b.ptr(nums);
        b.ptr(pix);
        b.ptr(None);
        w.set(root, b, false);
        let file = w.build(0x12);

        let (h, data) = read_loose(&file).unwrap();
        assert_eq!(h.version(), 0x12);
        let mut r = Rsc8Reader::new(&data, h.virtual_size());
        assert_eq!(r.u64().unwrap(), 0xABCD);
        let p = r.u64().unwrap();
        assert_eq!(r.string_at(p).unwrap().as_deref(), Some("hello"));
        let p = r.u64().unwrap();
        assert_eq!(r.array_at::<u32>(p, 3).unwrap(), Some(vec![1, 2, 3]));
        let p = r.u64().unwrap();
        assert!(Rsc8Reader::is_physical(p));
        assert_eq!(r.array_at::<u8>(p, 32).unwrap(), Some(vec![7u8; 32]));
        assert_eq!(r.u64().unwrap(), 0);
    }
}
