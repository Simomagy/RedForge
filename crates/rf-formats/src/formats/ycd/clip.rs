//! Clips (`Rsc8Clip`): what plays an animation (a single one, or parts of
//! layered ones), with tags (events along the clip) and properties, both lists
//! of named, typed attributes.

use super::ReadCx;
use super::util::{
    StrA, enum_name, file_name_without_extension, folder_name, parse_enum, read_array, write_map, write_ptr_arr,
    write_str_a,
};
use crate::hash::joaat;
use crate::rsc8::{ArrayHeader, BlockBuf, BlockId, Rsc8Reader, Rsc8Writer};
use crate::xml::{XmlNode, XmlWriter};
use crate::{Result, bail};

/// `Rsc8ClipType`.
pub const CLIP_TYPES: [(&str, i64); 2] = [("Single", 1), ("Multi", 2)];

/// `Rsc8ClipPropertyAttributeType`.
pub const ATTRIBUTE_TYPES: [(&str, i64); 10] = [
    ("Float", 1),
    ("Int", 2),
    ("Bool", 3),
    ("String", 4),
    ("Vector3", 6),
    ("Unknown7", 7),
    ("Vector4", 8),
    ("HashString", 12),
    ("Container", 13),
    ("Int128", 14),
];

/// The hash of "block": a tag list with a tag of that name says so.
const BLOCK_TAG: u32 = 0xE433D77D;

/// Where clips find the blocks of the animations they play, by hash (`None`
/// when their animations are in other files).
pub type AnimRefs<'a> = Option<&'a std::collections::HashMap<u32, BlockId>>;

fn anim_ref(anims: AnimRefs, hash: u32) -> Result<Option<BlockId>> {
    match anims {
        None => Ok(None),
        Some(m) => match m.get(&hash) {
            Some(id) => Ok(Some(*id)),
            None => bail!("clip references missing animation {hash:08X}"),
        },
    }
}

/// `Rsc8Clip`: 0x68 bytes, then the single or multi part.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub unknown_00h: u64,
    pub unknown_08h: u64,
    pub unknown_10h: u64,
    pub unknown_18h: u64,
    pub clip_type: u32,
    pub unknown_24h: u32,
    /// `None` only when it came from XML without one.
    pub name: Option<String>,
    pub signature: u32,
    pub unknown_3ch: u32,
    pub tag_list: Option<ClipTagList>,
    pub property_map: Option<ClipPropertyMap>,
    pub unknown_50h: u64,
    pub unknown_58h: u64,
    pub unknown_60h: u64,
    /// The hashes the dictionary files the clip under.
    pub name_hash: u32,
    pub dict_hash: u32,
    pub kind: ClipKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClipKind {
    Single(ClipSingle),
    Multi(ClipMulti),
}

/// A clip playing one animation (0xA0 bytes in all).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipSingle {
    pub animation_hash: u32,
    pub unknown_74h: u32,
    pub unknown_78h: u64,
    pub unknown_80h: u64,
    pub unknown_88h: u64,
    pub start_time: f32,
    pub end_time: f32,
    pub speed: f32,
    pub unknown_9ch: u32,
}

/// A clip of parts played one after the other, each of layered clips (0x90 bytes).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipMulti {
    pub parts: Option<Vec<ClipMultiPart>>,
    /// The parts' durations added up.
    pub duration: f32,
    pub unknown_7ch: u32,
    pub unknown_80h: u64,
    pub unknown_88h: u64,
}

/// `Rsc8ClipMultiPart`, 0x18 bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipMultiPart {
    pub clips: Option<Vec<ClipMultiClip>>,
    pub duration: f32,
    pub unknown_14h: u32,
}

/// `Rsc8ClipMultiClip`, 0x38 bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipMultiClip {
    pub animation_hash: u32,
    pub unknown_0ch: u32,
    pub unknown_10h: Option<Vec<[f32; 4]>>,
    pub unknown_20h: u64,
    pub start_time: f32,
    pub end_time: f32,
    pub speed: f32,
    pub unknown_34h: u32,
}

impl Clip {
    /// The clip at `ptr`; fails (as the C# does) for an unknown type.
    pub fn read(r: &mut Rsc8Reader, ptr: u64, cx: &mut ReadCx) -> Result<Self> {
        let ty = r.at(ptr + 32, |r| r.u8())?;
        if ty != 1 && ty != 2 {
            bail!("unknown clip type {ty}");
        }
        r.at(ptr, |r| {
            let unknown_00h = r.u64()?;
            let unknown_08h = r.u64()?;
            let unknown_10h = r.u64()?;
            let unknown_18h = r.u64()?;
            let clip_type = r.u32()?;
            let unknown_24h = r.u32()?;
            let name = StrA::read(r)?.value.unwrap_or_default();
            let signature = r.u32()?;
            let unknown_3ch = r.u32()?;
            let tags_ptr = r.u64()?;
            let tag_list = if tags_ptr == 0 { None } else { Some(ClipTagList::read(r, tags_ptr, cx)?) };
            let map_ptr = r.u64()?;
            let property_map = if map_ptr == 0 { None } else { Some(ClipPropertyMap::read(r, map_ptr, cx)?) };
            let unknown_50h = r.u64()?;
            let unknown_58h = r.u64()?;
            let unknown_60h = r.u64()?;
            cx.hashes.ensure(file_name_without_extension(&name));
            if let Some(f) = folder_name(&name) {
                cx.hashes.ensure(f);
            }
            let kind = if ty == 1 {
                let anim = r.u64()?;
                cx.touch_anim(r, anim)?;
                ClipKind::Single(ClipSingle {
                    animation_hash: r.u32()?,
                    unknown_74h: r.u32()?,
                    unknown_78h: r.u64()?,
                    unknown_80h: r.u64()?,
                    unknown_88h: r.u64()?,
                    start_time: r.f32()?,
                    end_time: r.f32()?,
                    speed: r.f32()?,
                    unknown_9ch: r.u32()?,
                })
            } else {
                let h = ArrayHeader::read(r)?;
                let parts = if h.ptr == 0 {
                    None
                } else {
                    let mut v = Vec::with_capacity(h.count as usize);
                    for i in 0..h.count as u64 {
                        v.push(ClipMultiPart::read(r, h.ptr + i * 0x18, cx)?);
                    }
                    Some(v)
                };
                ClipKind::Multi(ClipMulti {
                    parts,
                    duration: r.f32()?,
                    unknown_7ch: r.u32()?,
                    unknown_80h: r.u64()?,
                    unknown_88h: r.u64()?,
                })
            };
            Ok(Clip {
                unknown_00h,
                unknown_08h,
                unknown_10h,
                unknown_18h,
                clip_type,
                unknown_24h,
                name: Some(name),
                signature,
                unknown_3ch,
                tag_list,
                property_map,
                unknown_50h,
                unknown_58h,
                unknown_60h,
                name_hash: 0,
                dict_hash: 0,
                kind,
            })
        })
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.string("@type", &enum_name(self.clip_type as i64, &CLIP_TYPES));
        w.string("Name", self.name.as_deref().unwrap_or(""));
        w.hash("Dict", self.dict_hash);
        if self.signature != 0 {
            w.u32("Signature", self.signature);
        }
        if self.unknown_3ch != 0 {
            w.u32("Unknown_3C", self.unknown_3ch);
        }
        let tags = self.tag_list.as_ref().and_then(|t| t.tags.as_deref());
        w.node_array_opt("Tags", tags, |w, t| t.write_xml(w));
        write_properties_xml(w, self.property_map.as_ref());
        match &self.kind {
            ClipKind::Single(s) => {
                w.hash("Animation", s.animation_hash);
                w.f32("StartTime", s.start_time);
                w.f32("EndTime", s.end_time);
                w.f32("Speed", s.speed);
            }
            ClipKind::Multi(m) => w.node_array("Parts", m.parts.as_deref(), |w, p| p.write_xml(w)),
        }
    }

    /// `Read(MetaNodeReader)` for a clip of type `ty` (the `type` attribute).
    pub fn read_xml(ty: Option<&str>, n: &XmlNode, cx: &crate::formats::XmlContext) -> Result<Self> {
        let clip_type = match parse_enum(ty, &CLIP_TYPES) {
            Some(1) => 1,
            Some(2) => 2,
            _ => bail!("unknown clip type {ty:?}"),
        };
        let name = n.string("Name");
        let dict_hash = n.string("Dict").map_or(0, |s| joaat(&s));
        let mut tags = Vec::new();
        let tag_items = n.node_array("Tags");
        if let Some(items) = &tag_items {
            for i in items {
                let Some(i) = i else { bail!("null clip tag") };
                tags.push(Some(ClipProperty::read_xml(*i, true)?));
            }
        }
        let tag_list = ClipTagList::new(tag_items.map(|_| tags));
        let Some(props) = read_properties_xml(n)? else { bail!("clip without properties (the C# needs them)") };
        let name_s = name.as_deref();
        let name_hash = name_s.map_or(0, |s| joaat(file_name_without_extension(s)));
        if let Some(s) = name_s {
            cx.hashes.ensure(file_name_without_extension(s));
            if let Some(f) = folder_name(s) {
                cx.hashes.ensure(f);
            }
        }
        let kind = if clip_type == 1 {
            ClipKind::Single(ClipSingle {
                animation_hash: n.string("Animation").map_or(0, |s| joaat(&s)),
                start_time: n.f32("StartTime", 0.0),
                end_time: n.f32("EndTime", 0.0),
                speed: n.f32("Speed", 0.0),
                ..Default::default()
            })
        } else {
            let mut parts = None;
            if let Some(items) = n.node_array("Parts") {
                let mut v = Vec::with_capacity(items.len());
                for i in items {
                    let Some(i) = i else { bail!("null clip part") };
                    v.push(ClipMultiPart::read_xml(&i)?);
                }
                parts = Some(v);
            }
            let duration = parts.iter().flatten().fold(0f32, |d, p| d + p.duration);
            ClipKind::Multi(ClipMulti { parts, duration, ..Default::default() })
        };
        Ok(Clip {
            unknown_00h: 0,
            unknown_08h: 0,
            unknown_10h: 1,
            unknown_18h: 0x5000_0000,
            clip_type,
            unknown_24h: 0,
            name,
            signature: n.u32("Signature", 0),
            unknown_3ch: n.u32("Unknown_3C", 0),
            tag_list: Some(tag_list),
            property_map: Some(ClipPropertyMap::new(props)),
            unknown_50h: 0,
            unknown_58h: 0,
            unknown_60h: 0,
            name_hash,
            dict_hash,
            kind,
        })
    }

    /// The animations the clip plays, by hash.
    pub fn animation_hashes(&self) -> Vec<u32> {
        match &self.kind {
            ClipKind::Single(s) => vec![s.animation_hash],
            ClipKind::Multi(m) => {
                m.parts.iter().flatten().flat_map(|p| p.clips.iter().flatten()).map(|c| c.animation_hash).collect()
            }
        }
    }

    /// Adds the clip's blocks; `anims` resolves the animations it plays.
    pub fn write(&self, w: &mut Rsc8Writer, anims: AnimRefs) -> Result<BlockId> {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(0xA0);
        b.u64(self.unknown_00h);
        b.u64(self.unknown_08h);
        b.u64(self.unknown_10h);
        b.u64(self.unknown_18h);
        b.u32(self.clip_type);
        b.u32(self.unknown_24h);
        write_str_a(w, &mut b, self.name.as_deref());
        b.u32(self.signature);
        b.u32(self.unknown_3ch);
        let tags = self.tag_list.as_ref().map(|t| t.write(w, id));
        b.ptr(tags);
        let map = self.property_map.as_ref().map(|m| m.write(w, Some(id))).transpose()?;
        b.ptr(map);
        b.u64(self.unknown_50h);
        b.u64(self.unknown_58h);
        b.u64(self.unknown_60h);
        match &self.kind {
            ClipKind::Single(s) => {
                b.ptr(anim_ref(anims, s.animation_hash)?);
                b.u32(s.animation_hash);
                b.u32(s.unknown_74h);
                b.u64(s.unknown_78h);
                b.u64(s.unknown_80h);
                b.u64(s.unknown_88h);
                b.f32(s.start_time);
                b.f32(s.end_time);
                b.f32(s.speed);
                b.u32(s.unknown_9ch);
            }
            ClipKind::Multi(m) => {
                let parts = m.parts.as_deref().unwrap_or(&[]);
                let ptr = if parts.is_empty() {
                    None
                } else {
                    let mut pb = BlockBuf::with_capacity(parts.len() * 0x18);
                    for p in parts {
                        p.write(w, &mut pb, anims)?;
                    }
                    Some(w.add(pb, false))
                };
                b.ptr(ptr);
                b.u16(parts.len() as u16);
                b.u16(parts.len() as u16);
                b.u32(0);
                b.f32(m.duration);
                b.u32(m.unknown_7ch);
                b.u64(m.unknown_80h);
                b.u64(m.unknown_88h);
            }
        }
        w.set(id, b, false);
        Ok(id)
    }
}

impl ClipMultiPart {
    fn read(r: &mut Rsc8Reader, ptr: u64, cx: &mut ReadCx) -> Result<Self> {
        r.at(ptr, |r| {
            let h = ArrayHeader::read(r)?;
            let clips = if h.ptr == 0 {
                None
            } else {
                let mut v = Vec::with_capacity(h.count as usize);
                for i in 0..h.count as u64 {
                    v.push(ClipMultiClip::read(r, h.ptr + i * 0x38, cx)?);
                }
                Some(v)
            };
            Ok(Self { clips, duration: r.f32()?, unknown_14h: r.u32()? })
        })
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.f32("Duration", self.duration);
        w.node_array("Clips", self.clips.as_deref(), |w, c| c.write_xml(w));
    }

    pub fn read_xml(n: &XmlNode) -> Result<Self> {
        let mut clips = None;
        if let Some(items) = n.node_array("Clips") {
            let mut v = Vec::with_capacity(items.len());
            for i in items {
                let Some(i) = i else { bail!("null clip in a part") };
                v.push(ClipMultiClip::read_xml(&i));
            }
            clips = Some(v);
        }
        Ok(Self { clips, duration: n.f32("Duration", 0.0), unknown_14h: 0 })
    }

    /// Appends the part (0x18 bytes) to the parts' block.
    fn write(&self, w: &mut Rsc8Writer, b: &mut BlockBuf, anims: AnimRefs) -> Result<()> {
        let clips = self.clips.as_deref().unwrap_or(&[]);
        let ptr = if clips.is_empty() {
            None
        } else {
            let mut cb = BlockBuf::with_capacity(clips.len() * 0x38);
            for c in clips {
                cb.ptr(anim_ref(anims, c.animation_hash)?);
                cb.u32(c.animation_hash);
                cb.u32(c.unknown_0ch);
                let n = c.unknown_10h.as_ref().map_or(0, |v| v.len()) as u16;
                cb.ptr(w.array(c.unknown_10h.as_deref()));
                cb.u16(n);
                cb.u16(n);
                cb.u32(0);
                cb.u64(c.unknown_20h);
                cb.f32(c.start_time);
                cb.f32(c.end_time);
                cb.f32(c.speed);
                cb.u32(c.unknown_34h);
            }
            Some(w.add(cb, false))
        };
        b.ptr(ptr);
        b.u16(clips.len() as u16);
        b.u16(clips.len() as u16);
        b.u32(0);
        b.f32(self.duration);
        b.u32(self.unknown_14h);
        Ok(())
    }
}

impl ClipMultiClip {
    fn read(r: &mut Rsc8Reader, ptr: u64, cx: &mut ReadCx) -> Result<Self> {
        r.at(ptr, |r| {
            let anim = r.u64()?;
            cx.touch_anim(r, anim)?;
            let animation_hash = r.u32()?;
            let unknown_0ch = r.u32()?;
            let h = ArrayHeader::read(r)?;
            Ok(Self {
                animation_hash,
                unknown_0ch,
                unknown_10h: read_array::<[f32; 4]>(r, h.ptr, h.count as usize)?,
                unknown_20h: r.u64()?,
                start_time: r.f32()?,
                end_time: r.f32()?,
                speed: r.f32()?,
                unknown_34h: r.u32()?,
            })
        })
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.hash("Animation", self.animation_hash);
        w.f32("StartTime", self.start_time);
        w.f32("EndTime", self.end_time);
        w.f32("Speed", self.speed);
        if let Some(v) = &self.unknown_10h {
            w.vec4_array("Unknown_10", Some(v));
        }
    }

    pub fn read_xml(n: &XmlNode) -> Self {
        Self {
            animation_hash: n.string("Animation").map_or(0, |s| joaat(&s)),
            start_time: n.f32("StartTime", 0.0),
            end_time: n.f32("EndTime", 0.0),
            speed: n.f32("Speed", 0.0),
            unknown_10h: n.vec4_array("Unknown_10"),
            ..Default::default()
        }
    }
}

/// `Rsc8ClipTagList`, 0x30 bytes: a clip's tags.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipTagList {
    pub unknown_00h: u64,
    pub unknown_08h: u64,
    /// The owning clip's address, as read.
    pub owner: u64,
    pub tags: Option<Vec<Option<ClipProperty>>>,
    pub has_block_tag: u64,
}

impl ClipTagList {
    /// A tag list as the C# makes one for a clip from XML.
    pub fn new(tags: Option<Vec<Option<ClipProperty>>>) -> Self {
        let has_block = tags.iter().flatten().flatten().any(|t| t.name == BLOCK_TAG);
        Self { tags, has_block_tag: has_block as u64, ..Default::default() }
    }

    fn read(r: &mut Rsc8Reader, ptr: u64, cx: &mut ReadCx) -> Result<Self> {
        r.at(ptr, |r| {
            let unknown_00h = r.u64()?;
            let unknown_08h = r.u64()?;
            let owner = r.u64()?;
            let h = ArrayHeader::read(r)?;
            let tags = match read_array::<u64>(r, h.ptr, h.count as usize)? {
                None => None,
                Some(ptrs) => {
                    let mut v = Vec::with_capacity(ptrs.len());
                    for p in ptrs {
                        v.push(if p == 0 { None } else { Some(ClipProperty::read(r, p, true, cx)?) });
                    }
                    Some(v)
                }
            };
            Ok(Self { unknown_00h, unknown_08h, owner, tags, has_block_tag: r.u64()? })
        })
    }

    fn write(&self, w: &mut Rsc8Writer, clip: BlockId) -> BlockId {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(0x30);
        b.u64(self.unknown_00h);
        b.u64(self.unknown_08h);
        b.ptr(Some(clip));
        let tags: Option<Vec<Option<BlockId>>> =
            self.tags.as_ref().map(|v| v.iter().map(|t| t.as_ref().map(|t| t.write(w, Some(id), 2))).collect());
        write_ptr_arr(w, &mut b, tags.as_deref());
        b.u64(self.has_block_tag);
        w.set(id, b, false);
        id
    }
}

/// The extra fields of a `Rsc8ClipTag` (0x50 bytes) over a property (0x40).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TagPhase {
    pub start_phase: f32,
    pub end_phase: f32,
    pub unknown_48h: u64,
}

/// `Rsc8ClipProperty` (property type 1), or a `Rsc8ClipTag` (type 2, with
/// `tag` set): a named list of attributes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipProperty {
    pub unknown_00h: u64,
    pub unknown_08h: u64,
    pub attributes: Option<Vec<Option<Attribute>>>,
    /// The owner's address (a property map or tag list), as read.
    pub owner: u64,
    pub property_type: u32,
    pub signature: u32,
    pub unknown_30h: u64,
    pub name: u32,
    pub unknown_3ch: u32,
    pub tag: Option<TagPhase>,
}

impl ClipProperty {
    /// The property (or tag, with `tag`) at `ptr`. Like the C#'s block pool, a
    /// property where a tag was read before is that tag.
    pub fn read(r: &mut Rsc8Reader, ptr: u64, tag: bool, cx: &mut ReadCx) -> Result<Self> {
        let tag = if tag {
            cx.tags.insert(ptr, true);
            true
        } else {
            *cx.tags.entry(ptr).or_insert(false)
        };
        r.at(ptr, |r| {
            let unknown_00h = r.u64()?;
            let unknown_08h = r.u64()?;
            let h = ArrayHeader::read(r)?;
            let attributes = read_attributes(r, h)?;
            Ok(Self {
                unknown_00h,
                unknown_08h,
                attributes,
                owner: r.u64()?,
                property_type: r.u32()?,
                signature: r.u32()?,
                unknown_30h: r.u64()?,
                name: r.u32()?,
                unknown_3ch: r.u32()?,
                tag: if tag {
                    Some(TagPhase { start_phase: r.f32()?, end_phase: r.f32()?, unknown_48h: r.u64()? })
                } else {
                    None
                },
            })
        })
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.hash("Name", self.name);
        if self.signature != 0 {
            w.u32("Signature", self.signature);
        }
        if let Some(a) = &self.attributes
            && !a.is_empty()
        {
            w.node_array_opt("Attributes", Some(a), |w, a| a.write_xml(w));
        }
        if let Some(t) = &self.tag {
            w.f32("StartPhase", t.start_phase);
            w.f32("EndPhase", t.end_phase);
        }
    }

    /// `Read(MetaNodeReader)` of a property, or of a tag with `tag`.
    pub fn read_xml(n: XmlNode, tag: bool) -> Result<Self> {
        let mut p = ClipProperty {
            name: n.string("Name").map_or(0, |s| joaat(&s)),
            signature: n.u32("Signature", 0),
            property_type: if tag { 2 } else { 1 },
            ..Default::default()
        };
        if let Some(items) = n.node_array("Attributes") {
            let mut v = Vec::with_capacity(items.len());
            for i in items {
                let Some(i) = i else { bail!("null clip property attribute") };
                v.push(Some(Attribute::read_xml(&i)?));
            }
            p.attributes = Some(v);
        }
        if tag {
            p.tag = Some(TagPhase {
                start_phase: n.f32("StartPhase", 0.0),
                end_phase: n.f32("EndPhase", 0.0),
                unknown_48h: 0,
            });
        }
        Ok(p)
    }

    /// Adds the property's blocks, with `property_type` (1 in a property map, 2
    /// in a tag list) and pointing to `owner`.
    pub fn write(&self, w: &mut Rsc8Writer, owner: Option<BlockId>, property_type: u32) -> BlockId {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(0x50);
        b.u64(self.unknown_00h);
        b.u64(self.unknown_08h);
        let attrs: Option<Vec<Option<BlockId>>> =
            self.attributes.as_ref().map(|v| v.iter().map(|a| a.as_ref().map(|a| a.write(w, id, 0))).collect());
        write_ptr_arr(w, &mut b, attrs.as_deref());
        b.ptr(owner);
        b.u32(property_type);
        b.u32(self.signature);
        b.u64(self.unknown_30h);
        b.u32(self.name);
        b.u32(self.unknown_3ch);
        if let Some(t) = &self.tag {
            b.f32(t.start_phase);
            b.f32(t.end_phase);
            b.u64(t.unknown_48h);
        }
        w.set(id, b, false);
        id
    }
}

/// Reads a property's (or container's) attributes from its `Rsc8PtrArr`.
fn read_attributes(r: &mut Rsc8Reader, h: ArrayHeader) -> Result<Option<Vec<Option<Attribute>>>> {
    let Some(ptrs) = read_array::<u64>(r, h.ptr, h.count as usize)? else { return Ok(None) };
    let mut v = Vec::with_capacity(ptrs.len());
    for p in ptrs {
        v.push(if p == 0 { None } else { Some(Attribute::read(r, p)?) });
    }
    Ok(Some(v))
}

/// `Rsc8ClipPropertyAttribute`: 0x28 bytes, then the value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attribute {
    pub unknown_00h: u64,
    /// The owner's address, as read.
    pub owner: u64,
    pub attribute_type: u32,
    pub unknown_14h: u32,
    pub unknown_18h: u64,
    pub name: u32,
    pub unknown_24h: u32,
    pub value: AttributeValue,
}

/// The attribute types and their values; `None` for types the C# doesn't know.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum AttributeValue {
    #[default]
    None,
    Float {
        value: f32,
        unknown_2ch: u32,
    },
    Int {
        value: i32,
        unknown_2ch: u32,
    },
    Bool {
        value: u32,
        unknown_2ch: u32,
    },
    String {
        value: Option<String>,
        unknown_38h: u64,
    },
    /// Types 6 (`Vector3`), 7 (`Unknown7`) and 8 (`Vector4`), all four floats.
    Vector {
        unknown_28h: u64,
        value: [f32; 4],
    },
    HashString {
        value: u32,
        unknown_2ch: u32,
    },
    Container {
        unknown_28h: u64,
        unknown_30h: u64,
        attributes: Option<Vec<Option<Attribute>>>,
        /// The address of the container itself, as read.
        self_ref: u64,
        unknown_50h: u32,
        signature: u32,
        unknown_58h: u64,
    },
    Int128 {
        low: u64,
        high: u64,
        unknown_38h: u64,
    },
}

impl AttributeValue {
    /// `Rsc8ClipPropertyAttribute.Create`: an empty value of a type.
    fn new(ty: u32) -> Self {
        match ty {
            1 => Self::Float { value: 0.0, unknown_2ch: 0 },
            2 => Self::Int { value: 0, unknown_2ch: 0 },
            3 => Self::Bool { value: 0, unknown_2ch: 0 },
            4 => Self::String { value: None, unknown_38h: 0 },
            6..=8 => Self::Vector { unknown_28h: 0, value: [0.0; 4] },
            12 => Self::HashString { value: 0, unknown_2ch: 0 },
            13 => Self::Container {
                unknown_28h: 0,
                unknown_30h: 0,
                attributes: None,
                self_ref: 0,
                unknown_50h: 3,
                signature: 0,
                unknown_58h: 0,
            },
            14 => Self::Int128 { low: 0, high: 0, unknown_38h: 0 },
            _ => Self::None,
        }
    }
}

impl Attribute {
    pub fn read(r: &mut Rsc8Reader, ptr: u64) -> Result<Self> {
        // The type is chosen by the low byte of the type field.
        let ty = r.at(ptr + 16, |r| r.u8())?;
        r.at(ptr, |r| {
            let mut a = Attribute {
                unknown_00h: r.u64()?,
                owner: r.u64()?,
                attribute_type: r.u32()?,
                unknown_14h: r.u32()?,
                unknown_18h: r.u64()?,
                name: r.u32()?,
                unknown_24h: r.u32()?,
                value: AttributeValue::new(ty as u32),
            };
            match &mut a.value {
                AttributeValue::None => {}
                AttributeValue::Float { value, unknown_2ch } => {
                    *value = r.f32()?;
                    *unknown_2ch = r.u32()?;
                }
                AttributeValue::Int { value, unknown_2ch } => {
                    *value = r.i32()?;
                    *unknown_2ch = r.u32()?;
                }
                AttributeValue::Bool { value, unknown_2ch } | AttributeValue::HashString { value, unknown_2ch } => {
                    *value = r.u32()?;
                    *unknown_2ch = r.u32()?;
                }
                AttributeValue::String { value, unknown_38h } => {
                    *value = StrA::read(r)?.value;
                    *unknown_38h = r.u64()?;
                }
                AttributeValue::Vector { unknown_28h, value } => {
                    *unknown_28h = r.u64()?;
                    *value = r.vec4()?;
                }
                AttributeValue::Container {
                    unknown_28h,
                    unknown_30h,
                    attributes,
                    self_ref,
                    unknown_50h,
                    signature,
                    unknown_58h,
                } => {
                    *unknown_28h = r.u64()?;
                    *unknown_30h = r.u64()?;
                    let h = ArrayHeader::read(r)?;
                    *attributes = read_attributes(r, h)?;
                    *self_ref = r.u64()?;
                    *unknown_50h = r.u32()?;
                    *signature = r.u32()?;
                    *unknown_58h = r.u64()?;
                }
                AttributeValue::Int128 { low, high, unknown_38h } => {
                    *low = r.u64()?;
                    *high = r.u64()?;
                    *unknown_38h = r.u64()?;
                }
            }
            Ok(a)
        })
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.string("@type", &enum_name(self.attribute_type as i64, &ATTRIBUTE_TYPES));
        w.hash("Name", self.name);
        match &self.value {
            AttributeValue::None => {}
            AttributeValue::Float { value, .. } => w.f32("Value", *value),
            AttributeValue::Int { value, .. } => w.i32("Value", *value),
            AttributeValue::Bool { value, .. } => w.u32("Value", *value),
            AttributeValue::String { value, .. } => w.string("Value", value.as_deref().unwrap_or("")),
            AttributeValue::Vector { value, .. } => w.vec4("Value", *value),
            AttributeValue::HashString { value, .. } => w.hash("Value", *value),
            AttributeValue::Container { attributes, signature, .. } => {
                w.u32("Signature", *signature);
                w.node_array_opt("Attributes", attributes.as_deref(), |w, a| a.write_xml(w));
            }
            AttributeValue::Int128 { low, high, .. } => {
                w.u64("ValueLow", *low);
                w.u64("ValueHigh", *high);
            }
        }
    }

    pub fn read_xml(n: &XmlNode) -> Result<Self> {
        let ty_text = n.attr("type");
        let Some(ty) = parse_enum(ty_text, &ATTRIBUTE_TYPES).and_then(|t| u32::try_from(t).ok()) else {
            bail!("unknown clip property attribute type {ty_text:?}")
        };
        let mut a = Attribute {
            attribute_type: ty,
            name: n.string("Name").map_or(0, |s| joaat(&s)),
            value: AttributeValue::new(ty),
            ..Default::default()
        };
        match &mut a.value {
            AttributeValue::None => {}
            AttributeValue::Float { value, .. } => *value = n.f32("Value", 0.0),
            AttributeValue::Int { value, .. } => *value = n.i32("Value", 0),
            AttributeValue::Bool { value, .. } => *value = n.u32("Value", 0),
            AttributeValue::String { value, .. } => *value = n.string("Value"),
            AttributeValue::Vector { value, .. } => *value = n.vec4("Value"),
            AttributeValue::HashString { value, .. } => *value = n.string("Value").map_or(0, |s| joaat(&s)),
            AttributeValue::Container { attributes, signature, .. } => {
                *signature = n.u32("Signature", 0);
                if let Some(items) = n.node_array("Attributes") {
                    let mut v = Vec::with_capacity(items.len());
                    for i in items {
                        let Some(i) = i else { bail!("null attribute in a container") };
                        v.push(Some(Attribute::read_xml(&i)?));
                    }
                    *attributes = Some(v);
                }
            }
            AttributeValue::Int128 { low, high, .. } => {
                *low = n.u64("ValueLow", 0);
                *high = n.u64("ValueHigh", 0);
            }
        }
        Ok(a)
    }

    /// Adds the attribute's blocks, pointing to `owner` (`offset` bytes into it:
    /// 0x28 for a container's attributes).
    pub fn write(&self, w: &mut Rsc8Writer, owner: BlockId, offset: u64) -> BlockId {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(0x60);
        b.u64(self.unknown_00h);
        b.ptr_offset(Some(owner), offset);
        b.u32(self.attribute_type);
        b.u32(self.unknown_14h);
        b.u64(self.unknown_18h);
        b.u32(self.name);
        b.u32(self.unknown_24h);
        match &self.value {
            AttributeValue::None => {}
            AttributeValue::Float { value, unknown_2ch } => {
                b.f32(*value);
                b.u32(*unknown_2ch);
            }
            AttributeValue::Int { value, unknown_2ch } => {
                b.i32(*value);
                b.u32(*unknown_2ch);
            }
            AttributeValue::Bool { value, unknown_2ch } | AttributeValue::HashString { value, unknown_2ch } => {
                b.u32(*value);
                b.u32(*unknown_2ch);
            }
            AttributeValue::String { value, unknown_38h } => {
                write_str_a(w, &mut b, value.as_deref());
                b.u64(*unknown_38h);
            }
            AttributeValue::Vector { unknown_28h, value } => {
                b.u64(*unknown_28h);
                b.vec4(*value);
            }
            AttributeValue::Container {
                unknown_28h,
                unknown_30h,
                attributes,
                unknown_50h,
                signature,
                unknown_58h,
                ..
            } => {
                b.u64(*unknown_28h);
                b.u64(*unknown_30h);
                let attrs: Option<Vec<Option<BlockId>>> =
                    attributes.as_ref().map(|v| v.iter().map(|a| a.as_ref().map(|a| a.write(w, id, 0x28))).collect());
                write_ptr_arr(w, &mut b, attrs.as_deref());
                b.ptr(Some(id));
                b.u32(*unknown_50h);
                b.u32(*signature);
                b.u64(*unknown_58h);
            }
            AttributeValue::Int128 { low, high, unknown_38h } => {
                b.u64(*low);
                b.u64(*high);
                b.u64(*unknown_38h);
            }
        }
        w.set(id, b, false);
        id
    }
}

/// `Rsc8ClipPropertyMap`, 0x30 bytes: properties by name hash.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipPropertyMap {
    pub unknown_00h: u64,
    pub unknown_08h: u64,
    /// The owner's address, as read.
    pub owner_ptr: u64,
    /// The map's entries in order (by bucket, then along each bucket's chain).
    pub entries: Vec<ClipPropertyMapItem>,
    pub items_count: u32,
    pub flags: u32,
}

/// `Rsc8ClipPropertyMapItem`, 0x20 bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClipPropertyMapItem {
    pub hash: u32,
    pub unknown_04h: u32,
    pub property: Option<ClipProperty>,
    pub unknown_18h: u64,
}

impl ClipPropertyMap {
    /// A map of properties, as the C# builds one from XML.
    pub fn new(properties: Vec<ClipProperty>) -> Self {
        let entries: Vec<ClipPropertyMapItem> = properties
            .into_iter()
            .map(|p| ClipPropertyMapItem { hash: p.name, property: Some(p), ..Default::default() })
            .collect();
        Self { items_count: entries.len() as u32, entries, flags: 0x0100_0000, ..Default::default() }
    }

    /// The properties in order, as the C# lists them (`Properties`).
    pub fn properties(&self) -> Vec<Option<ClipProperty>> {
        self.entries.iter().map(|e| e.property.clone()).collect()
    }

    pub fn read(r: &mut Rsc8Reader, ptr: u64, cx: &mut ReadCx) -> Result<Self> {
        r.at(ptr, |r| {
            let unknown_00h = r.u64()?;
            let unknown_08h = r.u64()?;
            let owner_ptr = r.u64()?;
            let h = ArrayHeader::read(r)?;
            let mut entries = Vec::new();
            for head in read_array::<u64>(r, h.ptr, h.count as usize)?.unwrap_or_default() {
                let mut p = head;
                let mut seen = std::collections::HashSet::new();
                while p != 0 {
                    if !seen.insert(p) {
                        bail!("clip property map chain loops at 0x{p:X}");
                    }
                    let (item, next) = r.at(p, |r| {
                        let hash = r.u32()?;
                        let unknown_04h = r.u32()?;
                        let prop = r.u64()?;
                        let property = if prop == 0 { None } else { Some(ClipProperty::read(r, prop, false, cx)?) };
                        let next = r.u64()?;
                        Ok((ClipPropertyMapItem { hash, unknown_04h, property, unknown_18h: r.u64()? }, next))
                    })?;
                    entries.push(item);
                    p = next;
                }
            }
            Ok(Self { unknown_00h, unknown_08h, owner_ptr, entries, items_count: r.u32()?, flags: r.u32()? })
        })
    }

    /// Adds the map's blocks, pointing to `owner` (the clip or expression it
    /// belongs to; `None` for a clip dictionary's, whose properties point to the
    /// map instead). Fails for a single property named "", which the C# can't
    /// build a map of.
    pub fn write(&self, w: &mut Rsc8Writer, owner: Option<BlockId>) -> Result<BlockId> {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(0x30);
        b.u64(self.unknown_00h);
        b.u64(self.unknown_08h);
        b.ptr(owner);
        let keys: Vec<u64> = self.entries.iter().map(|e| e.hash as u64).collect();
        let prop_owner = if owner.is_none() { Some(id) } else { None };
        write_map(w, &mut b, &keys, "clip properties", |w, i, next| {
            let e = &self.entries[i];
            let prop = e.property.as_ref().map(|p| p.write(w, prop_owner, 1));
            let mut eb = BlockBuf::with_capacity(0x20);
            eb.u32(e.hash);
            eb.u32(e.unknown_04h);
            eb.ptr(prop);
            eb.ptr(next);
            eb.u64(e.unknown_18h);
            w.add(eb, false)
        })?;
        b.u32(self.items_count);
        b.u32(if self.items_count == 0 { 0 } else { 0x0100_0000 });
        w.set(id, b, false);
        Ok(id)
    }
}

/// Reads the `Properties` of a clip, clip dictionary or expression from XML:
/// `None` when absent; fails on a null item, as the C# does.
pub fn read_properties_xml(n: &XmlNode) -> Result<Option<Vec<ClipProperty>>> {
    let Some(items) = n.node_array("Properties") else { return Ok(None) };
    let mut v = Vec::with_capacity(items.len());
    for i in items {
        let Some(i) = i else { bail!("null clip property") };
        v.push(ClipProperty::read_xml(i, false)?);
    }
    if v.len() == 1 && v[0].name == 0 {
        bail!("a single property named \"\" (the C# can't build that map)");
    }
    Ok(Some(v))
}

/// Writes a property map's `Properties` (nothing without a map).
pub fn write_properties_xml(w: &mut XmlWriter, map: Option<&ClipPropertyMap>) {
    let props = map.map(|m| m.properties());
    w.node_array_opt("Properties", props.as_deref(), |w, p| p.write_xml(w));
}
