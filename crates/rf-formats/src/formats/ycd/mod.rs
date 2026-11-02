//! Clip dictionaries (ycd) and animation dictionaries (yad), from the C#'s
//! `Rsc8Animation.cs`, `YcdFile.cs` and `YadFile.cs`.
//!
//! The types, for the other formats that use them (expression dictionaries
//! have clip property maps):
//! - [`ClipDictionary`]: clips by name, the animation dictionary they play from
//!   (embedded, or other yads by index), and properties.
//! - [`AnimDictionary`]: animations by name; the root of a yad.
//! - [`Clip`] ([`ClipKind::Single`] or [`ClipKind::Multi`]), with a
//!   [`ClipTagList`] and a [`ClipPropertyMap`] of [`ClipProperty`]s, each a list
//!   of [`Attribute`]s.
//! - [`Anim`]: bone ids and [`Sequence`]s, whose packed data unpacks into
//!   [`Track`]s of [`Channel`]s.
//!
//! Each has `read(r, ptr, ...)` (binary, with a [`ReadCx`]), `write_xml`,
//! `read_xml` and `write(w, ...)` (binary, adding its blocks to a
//! [`Rsc8Writer`]). For a property map: [`ClipPropertyMap::read`],
//! [`write_properties_xml`], [`read_properties_xml`] and
//! [`ClipPropertyMap::new`] + [`ClipPropertyMap::write`].
//!
//! As in the C#, animations from XML are packed again when written, with the
//! quantisation worked out from their values and signatures computed as the
//! game's: the animation's from its bone ids, each sequence's from its data.
//! Sequences read from a file keep their data and are written back unchanged.
//!
//! Known differences from the C#, none of which the game's files hit:
//! - Of the C#'s block pool, only what changes the XML for real files is
//!   emulated (animations read once; a property where a tag was read is that
//!   tag). Other sharing (two strings at one address with different lengths, a
//!   clip filed under two entries, owner pointers to other kinds of block) reads
//!   each reference afresh.
//! - Reads just past the end of a sequence's data give zeros, where the C#
//!   reads whatever memory follows.
//! - `LinearFloat` channels (unused by the game) are ported as written, the C#'s
//!   chunk mismatch included, but untested against it.

mod anim;
mod channel;
mod clip;
pub(crate) mod util;

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};

pub use anim::{Anim, BoneId, FramePosition, Sequence, Track};
pub use channel::{CHANNEL_TYPES, Channel, ChannelData, ChannelRef};
pub use clip::{
    ATTRIBUTE_TYPES, AnimRefs, Attribute, AttributeValue, CLIP_TYPES, Clip, ClipKind, ClipMulti, ClipMultiClip,
    ClipMultiPart, ClipProperty, ClipPropertyMap, ClipPropertyMapItem, ClipSingle, ClipTagList, TagPhase,
    read_properties_xml, write_properties_xml,
};
use util::{StrA, build_buckets, file_name_without_extension, read_array, write_map, write_str_a};

use super::XmlContext;
use crate::hash::{HashIndex, joaat};
use crate::rsc8::{ArrayHeader, BlockBuf, BlockId, BlockMap, Rsc8Reader, Rsc8Writer, VIRTUAL_BASE, read_resource};
use crate::xml::{XmlDocument, XmlNode, XmlWriter};
use crate::{Result, bail};

/// What reading a clip or animation dictionary keeps track of, as the C#'s
/// block pool does where it matters: the names learnt (`hashes`), animations
/// already read, and which addresses were read as tags.
pub struct ReadCx<'a> {
    pub hashes: &'a HashIndex,
    /// `Some` until a dictionary takes it.
    anims: HashMap<u64, Option<Anim>>,
    tags: HashMap<u64, bool>,
}

impl<'a> ReadCx<'a> {
    pub fn new(hashes: &'a HashIndex) -> Self {
        Self { hashes, anims: HashMap::new(), tags: HashMap::new() }
    }

    /// Reads the animation a clip points to, as the C# does (it learns its
    /// name), unless it was read already.
    pub fn touch_anim(&mut self, r: &mut Rsc8Reader, ptr: u64) -> Result<()> {
        if ptr == 0 || self.anims.contains_key(&ptr) {
            return Ok(());
        }
        let a = Anim::read(r, ptr, self)?;
        self.anims.insert(ptr, Some(a));
        Ok(())
    }

    /// The animation at `ptr`, read once where possible.
    pub fn anim(&mut self, r: &mut Rsc8Reader, ptr: u64) -> Result<Option<Anim>> {
        if ptr == 0 {
            return Ok(None);
        }
        if let Some(a) = self.anims.get_mut(&ptr).and_then(Option::take) {
            return Ok(Some(a));
        }
        let a = Anim::read(r, ptr, self)?;
        self.anims.insert(ptr, None);
        Ok(Some(a))
    }
}

/// Follows a `Rsc8PtrArr` of hash buckets and the chains in them, calling `f`
/// with each entry's address in order (the C#'s `Rsc8DataMap.Flatten`). `f`
/// returns the next entry's address.
fn read_chains(
    r: &mut Rsc8Reader,
    h: ArrayHeader,
    what: &str,
    mut f: impl FnMut(&mut Rsc8Reader, u64) -> Result<u64>,
) -> Result<()> {
    for head in read_array::<u64>(r, h.ptr, h.count as usize)?.unwrap_or_default() {
        let mut p = head;
        let mut seen = std::collections::HashSet::new();
        while p != 0 {
            if !seen.insert(p) {
                bail!("{what} chain loops at 0x{p:X}");
            }
            p = f(r, p)?;
        }
    }
    Ok(())
}

/// The entries' indices in the order the C# lists them once it has built its
/// map from them (by bucket, then along each bucket's chain).
fn map_order(keys: &[u64], what: &str) -> Result<Vec<usize>> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let Some(b) = build_buckets(keys) else {
        bail!("{what}: a single entry with hash 0 (the C# can't build that map)")
    };
    Ok(b.into_iter().flatten().collect())
}

/// `Rsc8AnimDictionary`, 0x70 bytes: animations by the hash of their names.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimDictionary {
    pub vft: u64,
    /// The block map's address, as read.
    pub block_map: u64,
    pub usage_count: u64,
    pub unknown_18h: u64,
    pub unknown_20h: u64,
    pub unknown_28h: u64,
    pub unknown_30h: u64,
    /// `None` only when it came from XML without one.
    pub name: Option<String>,
    /// The entries in order (by bucket, then along each chain).
    pub entries: Vec<AnimDictionaryEntry>,
    pub entries_count: u32,
    pub unknown_5ch: u32,
    pub unknown_60h: u64,
    pub name_hash: u32,
    pub unknown_6ch: u32,
}

/// `Rsc8AnimDictionaryEntry`, 0x20 bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimDictionaryEntry {
    pub hash: u32,
    pub unknown_04h: u32,
    pub anim: Option<Anim>,
    pub unknown_18h: u64,
}

impl Default for AnimDictionary {
    fn default() -> Self {
        Self {
            vft: 0,
            block_map: 0,
            usage_count: 0,
            unknown_18h: 0,
            unknown_20h: 0,
            unknown_28h: 0,
            unknown_30h: 0,
            name: None,
            entries: Vec::new(),
            entries_count: 0,
            unknown_5ch: 0x0100_0000,
            unknown_60h: 0,
            name_hash: 0,
            unknown_6ch: 0,
        }
    }
}

impl AnimDictionary {
    pub fn read(r: &mut Rsc8Reader, ptr: u64, cx: &mut ReadCx) -> Result<Self> {
        r.at(ptr, |r| {
            let mut d = AnimDictionary {
                vft: r.u64()?,
                block_map: r.u64()?,
                usage_count: r.u64()?,
                unknown_18h: r.u64()?,
                unknown_20h: r.u64()?,
                unknown_28h: r.u64()?,
                unknown_30h: r.u64()?,
                ..Default::default()
            };
            let name = StrA::read(r)?.value.unwrap_or_default();
            let h = ArrayHeader::read(r)?;
            let mut entries = Vec::new();
            read_chains(r, h, "animation dictionary", |r, p| {
                r.at(p, |r| {
                    let hash = r.u32()?;
                    let unknown_04h = r.u32()?;
                    let a = r.u64()?;
                    let anim = cx.anim(r, a)?;
                    let next = r.u64()?;
                    entries.push(AnimDictionaryEntry { hash, unknown_04h, anim, unknown_18h: r.u64()? });
                    Ok(next)
                })
            })?;
            d.entries = entries;
            d.entries_count = r.u32()?;
            d.unknown_5ch = r.u32()?;
            d.unknown_60h = r.u64()?;
            d.name_hash = r.u32()?;
            d.unknown_6ch = r.u32()?;
            cx.hashes.ensure(file_name_without_extension(&name));
            d.name = Some(name);
            Ok(d)
        })
    }

    /// Fails where the C# fails writing the XML.
    pub fn check_xml(&self) -> Result<()> {
        self.entries.iter().filter_map(|e| e.anim.as_ref()).try_for_each(Anim::check_xml)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.u32("@version", 0);
        w.string("Name", self.name.as_deref().unwrap_or(""));
        let anims: Vec<Option<&Anim>> = self.entries.iter().map(|e| e.anim.as_ref()).collect();
        w.node_array_opt("Animations", Some(&anims), |w, a| a.write_xml(w));
    }

    pub fn read_xml(n: &XmlNode) -> Result<Self> {
        let name = n.string("Name");
        let Some(items) = n.node_array("Animations") else { bail!("animation dictionary without animations") };
        let mut entries = Vec::with_capacity(items.len());
        for i in items {
            let Some(i) = i else { bail!("null animation") };
            let anim = Anim::read_xml(&i)?;
            let hash = joaat(file_name_without_extension(anim.name.as_deref().unwrap_or("")));
            entries.push(AnimDictionaryEntry { hash, anim: Some(anim), ..Default::default() });
        }
        let keys: Vec<u64> = entries.iter().map(|e| e.hash as u64).collect();
        map_order(&keys, "animation dictionary")?;
        Ok(AnimDictionary {
            name_hash: name.as_deref().map_or(0, joaat),
            entries_count: entries.len() as u32,
            name,
            entries,
            ..Default::default()
        })
    }

    /// The entry each hash finds (`Dict` in the C#: the last in map order).
    pub fn lookup(&self) -> Result<HashMap<u32, usize>> {
        let keys: Vec<u64> = self.entries.iter().map(|e| e.hash as u64).collect();
        let mut m = HashMap::new();
        for i in map_order(&keys, "animation dictionary")? {
            if self.entries[i].anim.is_some() {
                m.insert(self.entries[i].hash, i);
            }
        }
        Ok(m)
    }

    /// Packs the animations and adds the dictionary's blocks at `id` (with a
    /// block map when it's a file's root). Returns the animations' blocks by
    /// hash, for the clips that play them.
    pub fn write(
        &mut self,
        w: &mut Rsc8Writer,
        id: BlockId,
        block_map: Option<BlockId>,
    ) -> Result<HashMap<u32, BlockId>> {
        let lookup = self.lookup()?;
        let owned: std::collections::HashSet<usize> = lookup.values().copied().collect();
        let mut ids: Vec<Option<BlockId>> = Vec::with_capacity(self.entries.len());
        for (i, e) in self.entries.iter_mut().enumerate() {
            ids.push(match e.anim.as_mut() {
                Some(a) => {
                    a.build()?;
                    let aid = w.reserve();
                    a.write(w, aid, owned.contains(&i).then_some(id));
                    Some(aid)
                }
                None => None,
            });
        }
        let name = self.name.as_deref();
        let mut b = BlockBuf::with_capacity(0x70);
        b.u64(self.vft);
        b.ptr(block_map);
        b.u64(self.usage_count);
        b.u64(self.unknown_18h);
        b.u64(self.unknown_20h);
        b.u64(self.unknown_28h);
        b.u64(self.unknown_30h);
        write_str_a(w, &mut b, name);
        let keys: Vec<u64> = self.entries.iter().map(|e| e.hash as u64).collect();
        write_map(w, &mut b, &keys, "animation dictionary", |w, i, next| {
            let e = &self.entries[i];
            let mut eb = BlockBuf::with_capacity(0x20);
            eb.u32(e.hash);
            eb.u32(e.unknown_04h);
            eb.ptr(ids[i]);
            eb.ptr(next);
            eb.u64(e.unknown_18h);
            w.add(eb, false)
        })?;
        b.u32(self.entries_count);
        b.u32(self.unknown_5ch);
        b.u64(self.unknown_60h);
        b.u32(name.map_or(0, joaat));
        b.u32(self.unknown_6ch);
        w.set(id, b, false);
        Ok(lookup.into_iter().filter_map(|(h, i)| ids[i].map(|id| (h, id))).collect())
    }
}

/// The animation dictionary references of a clip dictionary that has one
/// (`Rsc8AnimDictionaryRefArray`): 16-byte records of a pointer and metadata
/// (the dictionary's name hash, and flags in the upper half).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimDictionaryRefs {
    pub ptr: u64,
    pub count: u16,
    pub capacity: u16,
    pub unknown: u32,
    pub pointers: Vec<u64>,
    pub metadata: Vec<u64>,
    pub items: Vec<Option<AnimDictionary>>,
}

impl AnimDictionaryRefs {
    /// One embedded dictionary, as the C# makes it from XML.
    pub fn new(dict: AnimDictionary) -> Self {
        Self {
            count: 1,
            capacity: 1,
            pointers: vec![0],
            metadata: vec![1 << 32],
            items: vec![Some(dict)],
            ..Default::default()
        }
    }
}

/// `Rsc8ClipAnimDictImport`: another animation dictionary, by hash.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct ClipAnimDictImport {
    pub unk0: u64,
    pub hash: u32,
    pub unk2: u32,
}

/// The `Rsc8Arr` of animation dictionary imports (when there isn't exactly one
/// embedded dictionary).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipAnimDictImports {
    pub ptr: u64,
    pub count: u16,
    pub capacity: u16,
    pub unknown: u32,
    pub items: Option<Vec<ClipAnimDictImport>>,
}

/// `Rsc8ClipDictionaryEntry`, 0x20 bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipDictionaryEntry {
    pub clip_hash: u32,
    pub dict_hash: u32,
    pub clip: Option<Clip>,
    pub unknown_18h: u64,
}

/// `Rsc8ClipDictionaryAnimEntry`, 0x10 bytes: an animation the clips play, and
/// which animation dictionary has it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClipDictionaryAnimEntry {
    pub name: u32,
    pub dict_index: u32,
}

/// `Rsc8ClipDictionary`, 0xB0 bytes: the root of a ycd.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipDictionary {
    pub vft: u64,
    /// The block map's address, as read.
    pub block_map: u64,
    pub unknown_10h: u64,
    pub unknown_18h: u64,
    pub unknown_20h: u64,
    pub unknown_28h: u64,
    pub unknown_30h: u64,
    /// `None` only when it came from XML without one.
    pub name: Option<String>,
    /// The entries in order (by bucket, then along each chain).
    pub entries: Vec<ClipDictionaryEntry>,
    pub entries_count: u32,
    pub unknown_5ch: u32,
    pub property_map: Option<ClipPropertyMap>,
    pub name_hash: u32,
    /// 2 or 3.
    pub unknown_6ch: u32,
    /// When the array at 0x70 has one item: the embedded animation dictionary.
    pub anim_dicts: Option<AnimDictionaryRefs>,
    /// Otherwise: imports of other dictionaries.
    pub anim_dict_imports: Option<ClipAnimDictImports>,
    /// The animation map's entries in order.
    pub anim_map: Vec<ClipDictionaryAnimEntry>,
    pub anim_map_count: u32,
    pub unknown_94h: u32,
    pub unknown_98h: u64,
    /// Always an empty dictionary named like this one.
    pub anim_dict2: Option<AnimDictionary>,
    pub unknown_a8h: u64,
}

impl Default for ClipDictionary {
    fn default() -> Self {
        Self {
            vft: 0,
            block_map: 0,
            unknown_10h: 0,
            unknown_18h: 0,
            unknown_20h: 0,
            unknown_28h: 0,
            unknown_30h: 0,
            name: None,
            entries: Vec::new(),
            entries_count: 0,
            unknown_5ch: 0x0100_0000,
            property_map: None,
            name_hash: 0,
            unknown_6ch: 0,
            anim_dicts: None,
            anim_dict_imports: None,
            anim_map: Vec::new(),
            anim_map_count: 0,
            unknown_94h: 0x0100_0000,
            unknown_98h: 0,
            anim_dict2: None,
            unknown_a8h: 0,
        }
    }
}

impl ClipDictionary {
    pub fn read(r: &mut Rsc8Reader, ptr: u64, cx: &mut ReadCx) -> Result<Self> {
        r.at(ptr, |r| {
            let mut d = ClipDictionary {
                vft: r.u64()?,
                block_map: r.u64()?,
                unknown_10h: r.u64()?,
                unknown_18h: r.u64()?,
                unknown_20h: r.u64()?,
                unknown_28h: r.u64()?,
                unknown_30h: r.u64()?,
                ..Default::default()
            };
            let name = StrA::read(r)?.value.unwrap_or_default();
            let h = ArrayHeader::read(r)?;
            let mut entries = Vec::new();
            read_chains(r, h, "clip dictionary", |r, p| {
                r.at(p, |r| {
                    let clip_hash = r.u32()?;
                    let dict_hash = r.u32()?;
                    let c = r.u64()?;
                    let mut clip = if c == 0 { None } else { Some(Clip::read(r, c, cx)?) };
                    if let Some(clip) = &mut clip {
                        clip.name_hash = clip_hash;
                        clip.dict_hash = dict_hash;
                    }
                    let next = r.u64()?;
                    entries.push(ClipDictionaryEntry { clip_hash, dict_hash, clip, unknown_18h: r.u64()? });
                    Ok(next)
                })
            })?;
            d.entries = entries;
            d.entries_count = r.u32()?;
            d.unknown_5ch = r.u32()?;
            let map = r.u64()?;
            d.property_map = if map == 0 { None } else { Some(ClipPropertyMap::read(r, map, cx)?) };
            d.name_hash = r.u32()?;
            d.unknown_6ch = r.u32()?;
            let at = r.pos;
            let count = r.at(at + 8, |r| r.u16())?;
            let h = ArrayHeader::read(r)?;
            if count == 1 {
                let Some(records) = read_array::<u64>(r, h.ptr, h.count as usize * 2)? else {
                    bail!("clip dictionary's animation dictionary array has no records");
                };
                let mut refs = AnimDictionaryRefs {
                    ptr: h.ptr,
                    count: h.count,
                    capacity: h.capacity,
                    unknown: h.unknown,
                    ..Default::default()
                };
                for i in 0..h.count as usize {
                    refs.pointers.push(records[i * 2]);
                    refs.metadata.push(records[i * 2 + 1]);
                    let p = records[i * 2];
                    refs.items.push(if p == 0 { None } else { Some(AnimDictionary::read(r, p, cx)?) });
                }
                d.anim_dicts = Some(refs);
            } else {
                d.anim_dict_imports = Some(ClipAnimDictImports {
                    ptr: h.ptr,
                    count: h.count,
                    capacity: h.capacity,
                    unknown: h.unknown,
                    items: read_array(r, h.ptr, h.count as usize)?,
                });
            }
            let h = ArrayHeader::read(r)?;
            let mut anim_map = Vec::new();
            read_chains(r, h, "clip dictionary animation map", |r, p| {
                r.at(p, |r| {
                    anim_map.push(ClipDictionaryAnimEntry { name: r.u32()?, dict_index: r.u32()? });
                    r.u64()
                })
            })?;
            d.anim_map = anim_map;
            d.anim_map_count = r.u32()?;
            d.unknown_94h = r.u32()?;
            d.unknown_98h = r.u64()?;
            let p = r.u64()?;
            d.anim_dict2 = if p == 0 { None } else { Some(AnimDictionary::read(r, p, cx)?) };
            d.unknown_a8h = r.u64()?;
            cx.hashes.ensure(&name);
            d.name = Some(name);
            Ok(d)
        })
    }

    /// The embedded animation dictionary, if any.
    pub fn anim_dict(&self) -> Option<&AnimDictionary> {
        self.anim_dicts.as_ref().and_then(|a| a.items.first()).and_then(Option::as_ref)
    }

    /// The embedded dictionary's animations by hash, which is how clips name
    /// the animations they play (empty when they're in other dictionaries).
    pub fn anims(&self) -> HashMap<u32, &Anim> {
        let Some(d) = self.anim_dict() else { return HashMap::new() };
        d.lookup().unwrap_or_default().into_iter().filter_map(|(h, i)| Some((h, d.entries[i].anim.as_ref()?))).collect()
    }

    /// The clips, sorted by name as the C#'s viewer lists them (ordinal,
    /// ignoring case).
    pub fn clips(&self) -> Vec<&Clip> {
        let mut clips: Vec<&Clip> = self.entries.iter().filter_map(|e| e.clip.as_ref()).collect();
        clips.sort_by_cached_key(|c| c.name.as_deref().unwrap_or("").to_uppercase());
        clips
    }

    /// The dictionary the XML has: only with no more than one in the array
    /// (the C# never writes the imports).
    fn xml_anim_dict(&self) -> Option<&AnimDictionary> {
        let a = self.anim_dicts.as_ref()?;
        if a.count > 1 {
            return None;
        }
        a.items.first()?.as_ref()
    }

    /// Fails where the C# fails writing the XML.
    pub fn check_xml(&self) -> Result<()> {
        self.xml_anim_dict().map_or(Ok(()), AnimDictionary::check_xml)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.u32("@version", 0);
        w.string("Name", self.name.as_deref().unwrap_or(""));
        let clips: Vec<Option<&Clip>> = self.entries.iter().map(|e| e.clip.as_ref()).collect();
        w.node_array_opt("Clips", Some(&clips), |w, c| c.write_xml(w));
        w.node_array("Animations", Some(&self.anim_map), |w, a| {
            w.hash("Name", a.name);
            w.u32("DictIndex", a.dict_index);
        });
        if let Some(d) = self.xml_anim_dict() {
            w.node("AnimDict", |w| d.write_xml(w));
        }
        if self.property_map.is_some() {
            write_properties_xml(w, self.property_map.as_ref());
        }
        if self.unknown_6ch != 2 {
            w.u32("Unknown_6C", self.unknown_6ch);
        }
    }

    /// `Read(MetaNodeReader)`: builds the dictionary as the C# does, failing
    /// where it fails (a clip playing an animation the embedded dictionary
    /// doesn't have, missing lists, ...).
    pub fn read_xml(n: &XmlNode, cx: &XmlContext) -> Result<Self> {
        let name = n.string("Name");
        let Some(items) = n.node_array("Clips") else { bail!("clip dictionary without clips") };
        let mut entries = Vec::new();
        for i in items.into_iter().flatten() {
            let clip = Clip::read_xml(i.attr("type"), &i, cx)?;
            entries.push(ClipDictionaryEntry {
                clip_hash: clip.name_hash,
                dict_hash: clip.dict_hash,
                clip: Some(clip),
                unknown_18h: 0,
            });
        }
        let keys: Vec<u64> = entries.iter().map(|e| ((e.dict_hash as u64) << 32) | e.clip_hash as u64).collect();
        map_order(&keys, "clip dictionary")?;

        let Some(items) = n.node_array("Animations") else { bail!("clip dictionary without animations") };
        let mut anim_map = Vec::with_capacity(items.len());
        for i in items {
            let Some(i) = i else { bail!("null animation map entry") };
            anim_map.push(ClipDictionaryAnimEntry {
                name: i.string("Name").map_or(0, |s| joaat(&s)),
                dict_index: i.u32("DictIndex", 0),
            });
        }
        map_order(&anim_map.iter().map(|a| a.name as u64).collect::<Vec<_>>(), "clip dictionary animations")?;

        let mut d = ClipDictionary {
            name_hash: name.as_deref().map_or(0, joaat),
            name,
            entries_count: entries.len() as u32,
            entries,
            anim_map_count: anim_map.len() as u32,
            anim_map,
            ..Default::default()
        };
        match n.node("AnimDict") {
            Some(a) => d.anim_dicts = Some(AnimDictionaryRefs::new(AnimDictionary::read_xml(&a)?)),
            None => {
                let imports: Vec<ClipAnimDictImport> = n
                    .string_array("AnimDictImports")
                    .unwrap_or_default()
                    .iter()
                    .map(|s| ClipAnimDictImport { hash: joaat(s), ..Default::default() })
                    .collect();
                let c = imports.len() as u16;
                d.anim_dict_imports =
                    Some(ClipAnimDictImports { count: c, capacity: c, items: Some(imports), ..Default::default() });
            }
        }
        d.property_map = read_properties_xml(n)?.map(ClipPropertyMap::new);
        d.unknown_6ch = n.u32("Unknown_6C", 2);
        d.anim_dict2 = Some(AnimDictionary { name: d.name.clone(), ..Default::default() });
        d.resolve_and_count_usages()?;
        Ok(d)
    }

    /// `ResolveEmbeddedAnimationRefs` and `UpdateUsageCounts`: every animation a
    /// clip plays must be in the embedded dictionary, if there is one; each
    /// animation's usage count is its clips plus one.
    fn resolve_and_count_usages(&mut self) -> Result<()> {
        let clips: Vec<&Clip> = {
            let keys: Vec<u64> =
                self.entries.iter().map(|e| ((e.dict_hash as u64) << 32) | e.clip_hash as u64).collect();
            map_order(&keys, "clip dictionary")?.into_iter().filter_map(|i| self.entries[i].clip.as_ref()).collect()
        };
        let mut usages: HashMap<u32, u32> = HashMap::new();
        for c in &clips {
            for h in c.animation_hashes() {
                *usages.entry(h).or_default() += 1;
            }
        }
        let hashes: Vec<Vec<u32>> = clips.iter().map(|c| c.animation_hashes()).collect();
        let Some(dict) = self.anim_dicts.as_mut().and_then(|a| a.items.first_mut()).and_then(Option::as_mut) else {
            return Ok(());
        };
        let lookup = dict.lookup()?;
        for h in hashes.iter().flatten() {
            if !lookup.contains_key(h) {
                bail!("clip references missing animation {h:08X}");
            }
        }
        let mut total = 0u32;
        for (h, i) in lookup {
            let u = usages.get(&h).copied().unwrap_or(0);
            total = total.wrapping_add(u);
            if let Some(a) = dict.entries[i].anim.as_mut() {
                a.usage_count = (u + 1) as u64;
            }
        }
        dict.usage_count = total as u64;
        Ok(())
    }

    /// Builds the file: packs the animations and lays out every block.
    pub fn write_file(&mut self) -> Result<Vec<u8>> {
        let mut w = Rsc8Writer::new();
        let root = w.reserve();
        let bm = BlockMap::default().write(&mut w);
        let name = self.name.clone();

        let mut anim_ids = None;
        let mut records = None;
        if let Some(refs) = self.anim_dicts.as_mut().filter(|a| a.count == 1) {
            let n = (refs.count.max(refs.capacity)) as usize;
            let mut rb = BlockBuf::with_capacity(n * 16);
            for i in 0..n {
                let meta = refs.metadata.get(i).copied().unwrap_or(0);
                match refs.items.get_mut(i).and_then(Option::as_mut) {
                    Some(dict) => {
                        let id = w.reserve();
                        let ids = dict.write(&mut w, id, None)?;
                        if i == 0 {
                            anim_ids = Some(ids);
                        }
                        let hash = dict.name.as_deref().map_or(0, joaat);
                        rb.ptr(Some(id));
                        rb.u64((meta & 0xFFFF_FFFF_0000_0000) | hash as u64);
                    }
                    None => {
                        rb.u64(0);
                        rb.u64(meta);
                    }
                }
            }
            records = Some((w.add(rb, false), refs.count, refs.capacity, refs.unknown));
        }

        let mut b = BlockBuf::with_capacity(0xB0);
        b.u64(self.vft);
        b.ptr(Some(bm));
        b.u64(self.unknown_10h);
        b.u64(self.unknown_18h);
        b.u64(self.unknown_20h);
        b.u64(self.unknown_28h);
        b.u64(self.unknown_30h);
        write_str_a(&mut w, &mut b, name.as_deref());
        let keys: Vec<u64> = self.entries.iter().map(|e| ((e.dict_hash as u64) << 32) | e.clip_hash as u64).collect();
        let mut err = None;
        write_map(&mut w, &mut b, &keys, "clip dictionary", |w, i, next| {
            let e = &self.entries[i];
            let clip = match e.clip.as_ref().map(|c| c.write(w, anim_ids.as_ref())).transpose() {
                Ok(c) => c,
                Err(x) => {
                    err.get_or_insert(x);
                    None
                }
            };
            let mut eb = BlockBuf::with_capacity(0x20);
            eb.u32(e.clip_hash);
            eb.u32(e.dict_hash);
            eb.ptr(clip);
            eb.ptr(next);
            eb.u64(e.unknown_18h);
            w.add(eb, false)
        })?;
        if let Some(e) = err {
            return Err(e);
        }
        b.u32(self.entries_count);
        b.u32(self.unknown_5ch);
        let map = self.property_map.as_ref().map(|m| m.write(&mut w, None)).transpose()?;
        b.ptr(map);
        b.u32(name.as_deref().map_or(0, joaat));
        b.u32(self.unknown_6ch);
        match records {
            Some((id, count, capacity, unknown)) => {
                b.ptr(Some(id));
                b.u16(count);
                b.u16(capacity);
                b.u32(unknown);
            }
            None => {
                let imp = self.anim_dict_imports.clone().unwrap_or_default();
                b.ptr(w.array(imp.items.as_deref()));
                b.u16(imp.count);
                b.u16(imp.capacity);
                b.u32(imp.unknown);
            }
        }
        let keys: Vec<u64> = self.anim_map.iter().map(|a| a.name as u64).collect();
        write_map(&mut w, &mut b, &keys, "clip dictionary animations", |w, i, next| {
            let a = &self.anim_map[i];
            let mut eb = BlockBuf::with_capacity(0x10);
            eb.u32(a.name);
            eb.u32(a.dict_index);
            eb.ptr(next);
            w.add(eb, false)
        })?;
        b.u32(self.anim_map_count);
        b.u32(self.unknown_94h);
        b.u64(self.unknown_98h);
        let d2 = match self.anim_dict2.as_mut() {
            Some(d) => {
                let id = w.reserve();
                d.write(&mut w, id, None)?;
                Some(id)
            }
            None => None,
        };
        b.ptr(d2);
        b.u64(self.unknown_a8h);
        w.set(root, b, false);
        Ok(w.build(58))
    }
}

/// A ycd (loose, with its RSC8 header) to XML.
/// Reads a ycd (a loose resource).
pub fn read(file: &[u8], hashes: &HashIndex) -> Result<ClipDictionary> {
    read_resource(file, |r, _| ClipDictionary::read(r, VIRTUAL_BASE, &mut ReadCx::new(hashes)))
}

pub fn to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let d = read(file, cx.hashes)?;
    d.check_xml()?;
    Ok(XmlWriter::document(cx.hashes, "RDR2ClipDictionary", None, |w| d.write_xml(w)))
}

/// XML to a ycd.
pub fn from_xml(xml: &str, cx: &XmlContext) -> Result<Vec<u8>> {
    let mut d = XmlDocument::new(xml).with_root(None, |n| ClipDictionary::read_xml(&n, cx))??;
    d.write_file()
}

/// A yad (loose, with its RSC8 header) to XML.
pub fn yad_to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let d = read_resource(file, |r, _| AnimDictionary::read(r, VIRTUAL_BASE, &mut ReadCx::new(cx.hashes)))?;
    d.check_xml()?;
    Ok(XmlWriter::document(cx.hashes, "RDR2AnimDictionary", None, |w| d.write_xml(w)))
}

/// XML to a yad.
pub fn yad_from_xml(xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    let mut d = XmlDocument::new(xml).with_root(None, |n| AnimDictionary::read_xml(&n))??;
    let mut w = Rsc8Writer::new();
    let root = w.reserve();
    let bm = BlockMap::default().write(&mut w);
    d.write(&mut w, root, Some(bm))?;
    Ok(w.build(35))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rsc8::read_loose;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<RDR2ClipDictionary version="0">
 <Name>test_dict</Name>
 <Clips>
  <Item type="Single">
   <Name>pack:/walk.clip</Name>
   <Dict>test_dict</Dict>
   <Tags>
    <Item>
     <Name>block</Name>
     <Attributes>
      <Item type="Float"><Name>x</Name><Value value="0.5" /></Item>
      <Item type="Container"><Name>c</Name><Signature value="7" /><Attributes><Item type="Int"><Name>i</Name><Value value="-3" /></Item></Attributes></Item>
     </Attributes>
     <StartPhase value="0.25" />
     <EndPhase value="0.75" />
    </Item>
   </Tags>
   <Properties>
    <Item><Name>prop</Name><Attributes><Item type="String"><Name>s</Name><Value>hello</Value></Item></Attributes></Item>
   </Properties>
   <Animation>walk</Animation>
   <StartTime value="0" />
   <EndTime value="1" />
   <Speed value="1" />
  </Item>
 </Clips>
 <Animations />
 <AnimDict version="0">
  <Name>test_dict</Name>
  <Animations>
   <Item>
    <Name>pack:/walk.anim</Name>
    <Flags value="256" />
    <FrameCount value="4" />
    <Duration value="0.1" />
    <BoneIds>
     <Item><TypeId value="1" /><TrackId value="6" /></Item>
     <Item><BoneId value="11816" /><TrackId value="5" /></Item>
     <Item><BoneId value="1" /><TypeId value="2" /><TrackId value="2" /></Item>
    </BoneIds>
    <Sequences>
     <Item>
      <FrameCount value="4" />
      <Tracks>
       <Item><Channels>
        <Item type="QuantizeFloat"><Values>0.1 0.2 0.3 0.4</Values></Item>
        <Item type="IndirectQuantizeFloat"><Values>1 2 1 2</Values></Item>
        <Item type="StaticFloat"><Value value="0.5" /></Item>
        <Item type="CachedQuaternion1"><QuatIndex value="3" /></Item>
       </Channels></Item>
       <Item><Channels><Item type="StaticVector3"><Value x="1" y="2" z="3" /></Item></Channels></Item>
       <Item><Channels><Item type="IncrementalFloat"><Values>5 6 7 8</Values></Item></Channels></Item>
      </Tracks>
     </Item>
    </Sequences>
   </Item>
  </Animations>
 </AnimDict>
</RDR2ClipDictionary>"#;

    fn channel(s: &Sequence, t: usize, c: usize) -> &Channel {
        s.tracks.as_ref().unwrap()[t].as_ref().unwrap().channels.as_ref().unwrap()[c].as_ref().unwrap()
    }

    #[test]
    fn clip_dictionary_round_trips_through_xml() {
        let hashes = HashIndex::new();
        let cx = XmlContext { hashes: &hashes, folder: None };
        let file = from_xml(XML, &cx).unwrap();
        let (h, _) = read_loose(&file).unwrap();
        assert_eq!(h.version(), 58);
        let d = read_resource(&file, |r, _| ClipDictionary::read(r, VIRTUAL_BASE, &mut ReadCx::new(&hashes))).unwrap();
        assert_eq!(d.name.as_deref(), Some("test_dict"));
        assert_eq!(d.unknown_6ch, 2);

        let clip = d.entries[0].clip.as_ref().unwrap();
        assert_eq!(clip.name.as_deref(), Some("pack:/walk.clip"));
        assert_eq!(d.entries[0].clip_hash, joaat("walk"));
        let tags = clip.tag_list.as_ref().unwrap();
        assert_eq!(tags.has_block_tag, 1);
        let tag = tags.tags.as_ref().unwrap()[0].as_ref().unwrap();
        assert_eq!((tag.property_type, tag.tag.unwrap().end_phase), (2, 0.75));
        let attrs = tag.attributes.as_ref().unwrap();
        assert!(matches!(attrs[0].as_ref().unwrap().value, AttributeValue::Float { value: 0.5, .. }));
        let AttributeValue::Container { attributes: Some(inner), signature: 7, unknown_50h: 3, .. } =
            &attrs[1].as_ref().unwrap().value
        else {
            panic!("container {:?}", attrs[1])
        };
        assert!(matches!(inner[0].as_ref().unwrap().value, AttributeValue::Int { value: -3, .. }));
        let prop = clip.property_map.as_ref().unwrap().entries[0].property.as_ref().unwrap();
        let s = &prop.attributes.as_ref().unwrap()[0].as_ref().unwrap().value;
        assert!(matches!(s, AttributeValue::String { value: Some(v), .. } if v == "hello"));

        let dict = d.anim_dict().unwrap();
        let anim = dict.entries.iter().find_map(|e| e.anim.as_ref()).unwrap();
        // One clip plays it.
        assert_eq!((anim.usage_count, dict.usage_count), (2, 1));
        assert_eq!(anim.signature, anim.calc_signature());
        assert_eq!(anim.sequence_size, 15);
        let seq = anim.sequences.as_ref().unwrap()[0].as_ref().unwrap();
        assert_eq!(seq.signature, Sequence::calc_signature(seq.data.as_ref().unwrap()));
        assert_eq!(anim.max_seq_block_length, seq.block_length());

        let ChannelData::QuantizeFloat { values: Some(v), .. } = &channel(seq, 0, 0).data else { panic!() };
        for (a, b) in v.iter().zip([0.1, 0.2, 0.3, 0.4]) {
            assert!((a - b).abs() < 1e-5, "{v:?}");
        }
        assert_eq!(channel(seq, 0, 1).frame_values().unwrap(), Some(vec![1.0, 2.0, 1.0, 2.0]));
        assert!(matches!(channel(seq, 0, 2).data, ChannelData::StaticFloat { value: 0.5 }));
        assert!(matches!(channel(seq, 0, 3).data, ChannelData::CachedQuaternion { second: false, quat_index: 3 }));
        assert!(matches!(channel(seq, 1, 0).data, ChannelData::StaticVector3 { value: [1.0, 2.0, 3.0] }));
        // Incremental floats have no encoder: they're written raw.
        let ChannelData::RawFloat { values: Some(v) } = &channel(seq, 2, 0).data else { panic!() };
        assert_eq!(v, &[5.0, 6.0, 7.0, 8.0]);
        // The root motion: rotation (track id 6) and position (5), by channel type.
        let types = |r: &Option<Vec<ChannelRef>>| r.iter().flatten().map(|r| r.channel_type).collect::<Vec<_>>();
        assert_eq!(types(&seq.root_rotation_refs), [2, 4, 5, 7]);
        assert_eq!(types(&seq.root_position_refs), [1]);

        // And to XML again.
        let xml = to_xml(&file, &cx).unwrap();
        assert!(xml.contains("<Item type=\"RawFloat\">"), "{xml}");
        assert!(xml.contains("<Value>hello</Value>"));
    }

    #[test]
    fn animations_evaluate_like_the_csharp() {
        let hashes = HashIndex::new();
        let cx = XmlContext { hashes: &hashes, folder: None };
        let file = from_xml(XML, &cx).unwrap();
        let d = read_resource(&file, |r, _| ClipDictionary::read(r, VIRTUAL_BASE, &mut ReadCx::new(&hashes))).unwrap();
        let clip = d.clips()[0];
        assert_eq!(clip.duration(), 1.0);
        let anims = d.anims();
        let anim = anims[&clip.animation_hashes()[0]];
        let close = |a: [f32; 4], b: [f32; 4]| a.iter().zip(&b).all(|(a, b)| (a - b).abs() < 1e-4);

        // 4 frames over 0.1 s, the last being the first again: 0.05 s is halfway
        // between frames 1 and 2.
        let p = anim.frame_position(0.05);
        assert_eq!((p.frame0, p.frame1), (1, 2));
        assert!((p.alpha1 - 0.5).abs() < 1e-5);
        assert_eq!(anim.evaluate(&p, 1), [1.0, 2.0, 3.0, 0.0]);
        assert!(close(anim.evaluate(&p, 2), [6.5, 0.0, 0.0, 0.0]));
        // A cached quaternion puts back its left-out component (w) and
        // normalises: (0.2, 2, 0.5, 0) and (0.3, 1, 0.5, 0), blended.
        let norm = |v: [f32; 4]| {
            let l = v.iter().map(|c| c * c).sum::<f32>().sqrt();
            v.map(|c| c / l)
        };
        let (a, b) = (norm([0.2, 2.0, 0.5, 0.0]), norm([0.3, 1.0, 0.5, 0.0]));
        assert!(close(anim.evaluate(&p, 0), std::array::from_fn(|i| (a[i] + b[i]) * 0.5)));
        // The last frame's other side wraps round to the first.
        let p = anim.frame_position(0.1);
        assert_eq!((p.frame0, p.alpha1), (3, 0.0));
        assert!(close(anim.evaluate(&p, 2), [8.0, 0.0, 0.0, 0.0]));
        // No such track.
        assert_eq!(anim.evaluate(&p, 7), [0.0; 4]);
    }

    #[test]
    fn anim_dictionary_round_trips_through_xml() {
        let hashes = HashIndex::new();
        let cx = XmlContext { hashes: &hashes, folder: None };
        let start = XML.find("<AnimDict").unwrap();
        let end = XML.find("</AnimDict>").unwrap() + "</AnimDict>".len();
        let yad = XML[start..end].replace("AnimDict", "RDR2AnimDictionary");
        let file = yad_from_xml(&yad, &cx).unwrap();
        let xml = yad_to_xml(&file, &cx).unwrap();
        let d = XmlDocument::new(xml.as_str()).with_root(None, |n| AnimDictionary::read_xml(&n)).unwrap().unwrap();
        let anim = d.entries[0].anim.as_ref().unwrap();
        assert_eq!(anim.name.as_deref(), Some("pack:/walk.anim"));
        assert_eq!(anim.bone_ids.as_ref().unwrap()[1].bone_id, 11816);
        assert!(xml.contains("<Name>pack:/walk.anim</Name>"));
    }
}
