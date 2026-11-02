//! Reading bounds out of a resource the way the C# `Rsc8DataReader` does, with
//! its `BlockPool`: every block and array read is remembered by position, so a
//! second read at the same position returns the same object.
//!
//! That matters in three places: a composite's two flag arrays are compared by
//! reference; an array read again at a position keeps the length it was first
//! read with; and a bound referenced twice is one object, whose transform and
//! flags are the ones the last composite to reference it gave it.

use std::collections::{HashMap, HashSet};

use bytemuck::Pod;

use super::bounds::{Bounds, BoundsShape};
use super::material::CompositeFlags;
use super::math::Mat4;
use crate::rsc8::{BlockMap, Rsc8Reader};
use crate::{Result, bail};

/// The C# type of a pooled array or block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tag {
    U8,
    U32,
    U64,
    Vector3S,
    Colour,
    Material,
    Matrix,
    Flags,
    BvhNode,
    BlockMap,
    Bvh,
    Bounds,
}

#[derive(Clone, Copy)]
enum Pooled {
    /// An array of `len` items; `id` stands for the C# object.
    Array {
        tag: Tag,
        len: usize,
        id: u64,
    },
    Block(Tag),
}

/// What a composite gives each of its children.
#[derive(Clone, Copy)]
pub(crate) struct ChildInfo {
    pub transform: Mat4,
    pub flags1: CompositeFlags,
    pub flags2: CompositeFlags,
}

pub(crate) struct BoundsReader<'r, 'a> {
    pub r: &'r mut Rsc8Reader<'a>,
    pool: HashMap<u64, Pooled>,
    next_id: u64,
    /// Bounds read, by position; `None` while being read. Composites are kept
    /// without their children, which are in `children`.
    objects: HashMap<u64, Option<Bounds>>,
    children: HashMap<u64, Vec<u64>>,
    infos: HashMap<u64, ChildInfo>,
}

impl<'r, 'a> BoundsReader<'r, 'a> {
    pub fn new(r: &'r mut Rsc8Reader<'a>) -> Self {
        Self {
            r,
            pool: HashMap::new(),
            next_id: 1,
            objects: HashMap::new(),
            children: HashMap::new(),
            infos: HashMap::new(),
        }
    }

    /// Runs `f` with the position at `ptr`, then comes back.
    pub fn at<T>(&mut self, ptr: u64, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let saved = self.r.pos;
        self.r.pos = ptr;
        let res = f(self);
        self.r.pos = saved;
        res
    }

    /// `ReadArray<T>(count)` at `ptr`, through the pool; also returns the
    /// identity of the C# array object.
    pub fn array_id<T: Pod>(&mut self, tag: Tag, ptr: u64, count: usize) -> Result<(Option<Vec<T>>, u64)> {
        if ptr == 0 {
            return Ok((None, 0));
        }
        let (len, id, cache) = match self.pool.get(&ptr) {
            Some(&Pooled::Array { tag: t, len, id }) if t == tag => (len, id, false),
            Some(_) => (count, self.new_id(), false),
            None => (count, self.new_id(), true),
        };
        let Some(items) = self.r.array_at::<T>(ptr, len)? else { unreachable!() };
        if cache {
            self.pool.insert(ptr, Pooled::Array { tag, len, id });
        }
        Ok((Some(items), id))
    }

    pub fn array<T: Pod>(&mut self, tag: Tag, ptr: u64, count: usize) -> Result<Option<Vec<T>>> {
        Ok(self.array_id(tag, ptr, count)?.0)
    }

    fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// `ReadBlock<T>` of a block other than a bound: pooled, but reading it
    /// again gives the same data.
    pub fn block<T>(&mut self, tag: Tag, ptr: u64, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<Option<T>> {
        if ptr == 0 {
            return Ok(None);
        }
        self.pool.insert(ptr, Pooled::Block(tag));
        self.at(ptr, f).map(Some)
    }

    pub fn block_map(&mut self, ptr: u64) -> Result<Option<BlockMap>> {
        self.block(Tag::BlockMap, ptr, |br| BlockMap::read(br.r))
    }

    /// `ReadBlock(Rsc8Bounds.Create)` at `ptr`: reads the bound unless the pool
    /// already has one there.
    pub fn load(&mut self, ptr: u64) -> Result<()> {
        if ptr == 0 || matches!(self.pool.get(&ptr), Some(Pooled::Block(Tag::Bounds))) {
            return Ok(());
        }
        self.pool.insert(ptr, Pooled::Block(Tag::Bounds));
        self.objects.insert(ptr, None);
        let (b, children) = self.at(ptr, Bounds::read_block)?;
        if let Some(c) = children {
            self.children.insert(ptr, c);
        }
        self.objects.insert(ptr, Some(b));
        Ok(())
    }

    /// A composite gives a child its transform and flags (the last one wins).
    pub fn set_info(&mut self, child: u64, info: ChildInfo) {
        self.infos.insert(child, info);
    }

    /// The bound at `ptr` with its children, as the C# objects end up.
    pub fn assemble(&self, ptr: u64) -> Result<Bounds> {
        self.assemble_in(ptr, &mut HashSet::new())
    }

    fn assemble_in(&self, ptr: u64, visiting: &mut HashSet<u64>) -> Result<Bounds> {
        let Some(Some(b)) = self.objects.get(&ptr) else { bail!("bounds at 0x{ptr:X} weren't read") };
        if !visiting.insert(ptr) {
            bail!("bounds at 0x{ptr:X} contain themselves");
        }
        let mut b = b.clone();
        if let Some(info) = self.infos.get(&ptr) {
            b.transform = info.transform;
            b.composite_flags1 = info.flags1;
            b.composite_flags2 = info.flags2;
        }
        if let BoundsShape::Composite(c) = &mut b.shape {
            let ptrs = self.children.get(&ptr).map(Vec::as_slice).unwrap_or_default();
            let mut kids = Vec::with_capacity(ptrs.len());
            for &p in ptrs {
                kids.push(if p == 0 { None } else { Some(self.assemble_in(p, visiting)?) });
            }
            c.children = Some(kids);
        }
        visiting.remove(&ptr);
        Ok(b)
    }
}
