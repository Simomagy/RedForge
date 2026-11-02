//! Cloth dictionaries (yld), from `Rsc8Cloth.cs` and `YldFile.cs`: the
//! simulated clothing of characters (peds, horses), by name hash.
//!
//! # Structs
//!
//! - [`ClothDictionary`] (the root) has [`CharacterCloth`]s sorted by name
//!   hash.
//! - A [`CharacterCloth`] has a [`CharacterClothController`], composite
//!   [`Bounds`] of capsules the cloth collides with, and the tags of the bones
//!   (in the character's [`Skeleton`](crate::formats::ydr::Skeleton), see
//!   [`Skeleton::bone_by_tag`](crate::formats::ydr::Skeleton::bone_by_tag))
//!   each capsule follows. [`CharacterCloth::capsule_bones`] pairs them up.
//! - A [`ClothController`] (environment cloth in fragments uses it as is,
//!   characters through [`CharacterClothController`]) has up to 3 LODs of
//!   [`VerletCloth`] (the simulated vertices and their constraints), a
//!   [`ClothBridgeSimGfx`] and a [`MorphController`]. Fragments (yft) embed
//!   these, and [`ClothInstanceTuning`].
//! - [`Arr`] and [`Arr32`] are the C#'s `Rsc8Arr`/`Rsc8Arr32` of plain data.
//!
//! Every struct has `read` (at the reader's position), `write` (adds its
//! block, returns its id), `write_xml` (into the current node) and `read_xml`.
//!
//! # C# behaviour kept
//!
//! - A cloth's `Name` is its hash's name (or made-up collision name) and is
//!   hashed back as text; the names aren't added to the hash index.
//! - `ClothBridgeSimGfx` writes `UnknownA0` twice and never `Unknown100`, so
//!   `Unknown_100h` is lost through XML.
//! - A controller's name is cut to 32 characters and anything after its NUL
//!   in the 56 bytes is lost.
//! - Values the XML doesn't have get the C#'s XML defaults (`Unknown9C` 0.04,
//!   `UnknownC0` 4, `UnknownFC` 1), and the verlet cloth's constraint count is
//!   cut to 16 bits.
//!
//! # Known issues
//!
//! - The C#'s `List.Sort` isn't stable: clothes with equal name hashes may
//!   come out in another order.
//! - Arrays the C#'s block pool shares (two pointers to one array, read with
//!   different counts) are read with each count here. None was seen in the
//!   game's files.
//! - The C# fails on a dictionary without clothes, or with null ones, from
//!   XML; so does this port.
//! - Written files aren't byte-identical to the C#'s: blocks of equal size
//!   are laid out in another order (the C#'s sort isn't stable). All 1037
//!   game ylds read back the same with the C#.
//! - `Rsc8ClothInstance` (the C#'s helper skinning cloth vertices for
//!   rendering) isn't ported; [`CharacterClothController::bones`] and
//!   [`CharacterCloth::capsule_bones`] give the bones it uses.

mod cloth;
pub(crate) mod util;

pub use cloth::{
    CharacterClothController, ClothBoneWeightsInds, ClothBridgeSimGfx, ClothController, ClothControllerVertex,
    ClothInstanceTuning, MorphController, MorphControllerUnk1, VerletCloth, VerletClothConstraint,
};
pub use util::{Arr, Arr32};

use super::XmlContext;
use crate::formats::ybn::{Bounds, BoundsShape};
use crate::formats::ydr::util::{ArrHeader, node_items, read_ptr_arr, write_ptr_arr};
use crate::formats::ydr::{Bone, Skeleton};
use crate::hash::joaat;
use crate::rsc8::{BlockBuf, BlockId, BlockMap, Rsc8Reader, Rsc8Writer, read_resource};
use crate::xml::{XmlDocument, XmlNode, XmlWriter};
use crate::{Result, bail};
use util::node_array_with;

/// The resource version of yld files.
pub const VERSION: u32 = 11;

/// `Rsc8CharacterCloth` (0xD0 bytes): one piece of a character's clothing.
#[derive(Clone, Debug, PartialEq)]
pub struct CharacterCloth {
    pub vft: u64,
    pub unknown_8h: u64,
    pub unknown_10h: Arr<[f32; 4]>,
    pub controller: Option<CharacterClothController>,
    /// A composite of the capsules the cloth collides with.
    pub bounds: Option<Bounds>,
    /// The tag of the bone each of the bounds' children follows.
    pub bone_ids: Arr<u32>,
    pub unknown_40h: u64,
    pub unknown_48h: u64,
    pub transform: [f32; 16],
    pub unknown_90h: Arr<u32>,
    pub unknown_a0h: u64,
    pub unknown_a8h: u64,
    pub unknown_b0h: u64,
    pub unknown_b8h: u64,
    pub unknown_c0h: u64,
    pub unknown_c8h: u64,
    /// From the dictionary's hashes, or the XML's `Name` (not in the struct).
    pub name_hash: u32,
}

impl Default for CharacterCloth {
    fn default() -> Self {
        Self {
            vft: 0,
            unknown_8h: 0,
            unknown_10h: Arr::default(),
            controller: None,
            bounds: None,
            bone_ids: Arr::default(),
            unknown_40h: 0,
            unknown_48h: 0,
            transform: [0.0; 16],
            unknown_90h: Arr::default(),
            unknown_a0h: 0,
            unknown_a8h: 0,
            unknown_b0h: 0,
            unknown_b8h: 0,
            unknown_c0h: 0,
            unknown_c8h: 0,
            name_hash: 0,
        }
    }
}

impl CharacterCloth {
    pub const BLOCK_LENGTH: usize = 0xD0;

    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let mut c = Self { vft: r.u64()?, unknown_8h: r.u64()?, unknown_10h: Arr::read(r)?, ..Default::default() };
        c.controller = r.ptr(CharacterClothController::read)?;
        let bounds_ptr = r.u64()?;
        c.bounds = Bounds::read_at(r, bounds_ptr)?;
        c.bone_ids = Arr::read(r)?;
        c.unknown_40h = r.u64()?;
        c.unknown_48h = r.u64()?;
        c.transform = r.floats()?;
        c.unknown_90h = Arr::read(r)?;
        c.unknown_a0h = r.u64()?;
        c.unknown_a8h = r.u64()?;
        c.unknown_b0h = r.u64()?;
        c.unknown_b8h = r.u64()?;
        c.unknown_c0h = r.u64()?;
        c.unknown_c8h = r.u64()?;
        Ok(c)
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> Result<BlockId> {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u64(self.vft);
        b.u64(self.unknown_8h);
        self.unknown_10h.write(&mut b, w);
        b.ptr(self.controller.as_ref().map(|c| c.write(w)));
        b.ptr(self.bounds.as_ref().map(|x| x.write(w)).transpose()?);
        self.bone_ids.write(&mut b, w);
        b.u64(self.unknown_40h);
        b.u64(self.unknown_48h);
        b.floats(&self.transform);
        self.unknown_90h.write(&mut b, w);
        b.u64(self.unknown_a0h);
        b.u64(self.unknown_a8h);
        b.u64(self.unknown_b0h);
        b.u64(self.unknown_b8h);
        b.u64(self.unknown_c0h);
        b.u64(self.unknown_c8h);
        w.set(id, b, false);
        Ok(id)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.hash("Name", self.name_hash);
        w.matrix4x4("Transform", &self.transform);
        w.vec4_array("Unknown10", self.unknown_10h.items.as_deref());
        w.u32_array("Unknown90", self.unknown_90h.items.as_deref());
        if let Some(c) = &self.controller {
            w.node("Controller", |w| c.write_xml(w));
        }
        if let Some(b) = &self.bounds {
            w.node("Bounds", |w| b.write_xml(w));
        }
        w.u32_array("BoneIds", self.bone_ids.items.as_deref());
    }

    pub fn read_xml(n: XmlNode) -> Result<Self> {
        Ok(Self {
            name_hash: n.string("Name").map_or(0, |s| joaat(&s)),
            transform: n.matrix("Transform").unwrap_or([0.0; 16]),
            unknown_10h: Arr::new(n.vec4_array("Unknown10")),
            unknown_90h: Arr::new(n.u32_array("Unknown90")),
            controller: n.node("Controller").map(CharacterClothController::read_xml).transpose()?,
            bounds: n.node("Bounds").map(|b| Bounds::read_xml(&b)).transpose()?,
            bone_ids: Arr::new(n.u32_array("BoneIds")),
            ..Default::default()
        })
    }

    /// Each child of the bounds with the bone (index and bone) of its tag in
    /// `skeleton`, if there is one.
    pub fn capsule_bones<'a>(&'a self, skeleton: &'a Skeleton) -> Vec<(&'a Bounds, Option<(usize, &'a Bone)>)> {
        let Some(BoundsShape::Composite(c)) = self.bounds.as_ref().map(|b| &b.shape) else { return Vec::new() };
        let kids = c.children.iter().flatten().enumerate();
        kids.filter_map(|(i, b)| {
            let b = b.as_ref()?;
            let bone = self.bone_ids.items().get(i).and_then(|&tag| skeleton.bone_by_tag(tag as u16));
            Some((b, bone))
        })
        .collect()
    }
}

/// `Rsc8ClothDictionary` (64 bytes): character cloth sorted by name hash.
#[derive(Clone, Debug, PartialEq)]
pub struct ClothDictionary {
    pub vft: u64,
    pub block_map: Option<BlockMap>,
    pub unknown_10h: u64,
    pub unknown_18h: u64,
    pub hashes: Arr<u32>,
    /// `None` for a null pointer or no clothes.
    pub clothes: Option<Vec<Option<CharacterCloth>>>,
    pub clothes_capacity: u16,
    pub clothes_unknown: u32,
}

impl Default for ClothDictionary {
    fn default() -> Self {
        Self {
            vft: 0,
            block_map: None,
            unknown_10h: 0,
            unknown_18h: 1,
            hashes: Arr::default(),
            clothes: None,
            clothes_capacity: 0,
            clothes_unknown: 0,
        }
    }
}

impl ClothDictionary {
    pub const BLOCK_LENGTH: usize = 0x40;

    /// Reads the dictionary; each cloth gets its hash as `name_hash`.
    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let mut d = Self { vft: r.u64()?, ..Default::default() };
        d.block_map = r.ptr(BlockMap::read)?;
        d.unknown_10h = r.u64()?;
        d.unknown_18h = r.u64()?;
        d.hashes = Arr::read(r)?;
        let h = ArrHeader::read(r)?;
        d.clothes = read_ptr_arr(r, &h, CharacterCloth::read)?;
        d.clothes_capacity = h.capacity;
        d.clothes_unknown = h.unknown;
        if let (Some(clothes), Some(hashes)) = (&mut d.clothes, &d.hashes.items) {
            for (i, c) in clothes.iter_mut().enumerate() {
                if let Some(c) = c {
                    c.name_hash = hashes.get(i).copied().unwrap_or(0);
                }
            }
        }
        Ok(d)
    }

    /// `Build`: a dictionary of these clothes, sorted by name hash.
    pub fn build(mut clothes: Vec<CharacterCloth>) -> Self {
        clothes.sort_by_key(|c| c.name_hash);
        let n = clothes.len() as u16;
        Self {
            hashes: Arr::new(Some(clothes.iter().map(|c| c.name_hash).collect())),
            clothes: Some(clothes.into_iter().map(Some).collect()),
            clothes_capacity: n,
            ..Default::default()
        }
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> Result<BlockId> {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u64(self.vft);
        b.ptr(self.block_map.as_ref().map(|m| m.write(w)));
        b.u64(self.unknown_10h);
        b.u64(self.unknown_18h);
        self.hashes.write(&mut b, w);
        let clothes = match &self.clothes {
            Some(v) => Some(v.iter().map(|c| c.as_ref().map(|c| c.write(w)).transpose()).collect::<Result<Vec<_>>>()?),
            None => None,
        };
        write_ptr_arr(&mut b, w, clothes.as_deref(), self.clothes_capacity, self.clothes_unknown);
        w.set(id, b, false);
        Ok(id)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) -> Result<()> {
        w.u32("@version", 0);
        node_array_with(w, "Clothes", self.clothes.as_deref(), |w, c| {
            c.write_xml(w);
            Ok(())
        })
    }

    pub fn read_xml(n: XmlNode) -> Result<Self> {
        let Some(clothes) = node_items(n, "Clothes", CharacterCloth::read_xml)? else {
            bail!("a cloth dictionary without Clothes (the C# fails on it)");
        };
        Ok(Self::build(clothes))
    }

    /// The clothes that aren't null, with their name hashes.
    pub fn clothes(&self) -> impl Iterator<Item = &CharacterCloth> {
        self.clothes.iter().flatten().flatten()
    }

    /// The cloth with this name hash.
    pub fn get(&self, name_hash: u32) -> Option<&CharacterCloth> {
        self.clothes().find(|c| c.name_hash == name_hash)
    }
}

/// Reads a yld (a loose resource).
pub fn read(file: &[u8]) -> Result<ClothDictionary> {
    read_resource(file, |r, _| ClothDictionary::read(r))
}

/// Builds a yld, as `YldFile.Save` does (with a block map).
pub fn write(dict: &ClothDictionary) -> Result<Vec<u8>> {
    let mut w = Rsc8Writer::new();
    if dict.block_map.is_some() {
        dict.write(&mut w)?;
    } else {
        let mut d = dict.clone();
        d.block_map = Some(BlockMap::default());
        d.write(&mut w)?;
    }
    Ok(w.build(VERSION))
}

pub fn to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let d = read(file)?;
    let mut res = Ok(());
    let xml = XmlWriter::document(cx.hashes, "RDR2ClothDictionary", None, |w| res = d.write_xml(w));
    res.map(|_| xml)
}

pub fn from_xml(xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    let doc = XmlDocument::new(xml);
    let d = doc.with_root(None, ClothDictionary::read_xml)??;
    write(&d)
}
