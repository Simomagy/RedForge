//! Pose matcher dictionaries (ypmd), from `Rsc8PoseMatcher.cs`: a hash map of
//! pose matchers, each a set of clip samples with point clouds.

use bytemuck::{Pod, Zeroable};

use super::XmlContext;
use super::yfd::{read_pointer_array, write_header};
use super::yldb::{StrA, build_map, gen_hash, read_chain, read_string, trim_name};
use super::yrecords::{nonzero, read_struct_array};
use crate::rsc8::{ArrayHeader, BlockBuf, BlockId, BlockMap, Rsc8Reader, Rsc8Writer, read_resource};
use crate::xml::number::{f32_to_string, parse_f32};
use crate::xml::{XmlDocument, XmlNode, XmlWriter};
use crate::{Result, bail};

/// The resource version `YpmdFile.Save` builds with.
const VERSION: u32 = 11;

/// `Rsc8PoseMatcherDictionary`.
#[derive(Clone, Debug, PartialEq)]
pub struct PoseMatcherDictionary {
    pub vft: u64,
    pub block_map: Option<BlockMap>,
    pub unknown_00h: u64,
    pub unknown_08h: u64,
    pub unknown_10h: u64,
    pub unknown_18h: u64,
    pub unknown_20h: u64,
    pub name: StrA,
    /// The hash map's buckets: each is the chain of entries linked by `Next`.
    pub pose_matcher_map: Vec<Vec<PoseMatcherEntry>>,
    pub pose_matcher_map_header: ArrayHeader,
    pub pose_matcher_map_count: u32,
    /// Always 0x1000000 in the game files.
    pub unknown_4ch: u32,
    pub unknown_50h: u64,
    pub name_hash: u32,
    pub unknown_5ch: u32,
}

impl Default for PoseMatcherDictionary {
    fn default() -> Self {
        Self {
            vft: 0,
            block_map: None,
            unknown_00h: 0,
            unknown_08h: 0,
            unknown_10h: 0,
            unknown_18h: 0,
            unknown_20h: 0,
            name: StrA::default(),
            pose_matcher_map: Vec::new(),
            pose_matcher_map_header: ArrayHeader::default(),
            pose_matcher_map_count: 0,
            unknown_4ch: 0x100_0000,
            unknown_50h: 0,
            name_hash: 0,
            unknown_5ch: 0,
        }
    }
}

/// `Rsc8PoseMatcherEntry`, without its `Next` pointer (the bucket's order is the chain).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PoseMatcherEntry {
    pub hash: u32,
    pub unknown_04h: u32,
    pub item: Option<PoseMatcherItem>,
    pub unknown_18h: u64,
}

/// `Rsc8PoseMatcherItem`.
#[derive(Clone, Debug, PartialEq)]
pub struct PoseMatcherItem {
    pub unknown_00h: u64,
    pub unknown_08h: u64,
    pub usage_count: u64,
    /// The dictionary: a back-pointer, kept raw (the C# writes it back as read).
    pub owner: u64,
    /// `None` for a null pointer or a count of 0.
    pub samples: Option<Vec<Option<PoseMatcherSample>>>,
    pub samples_header: ArrayHeader,
    /// `None` for a null pointer or a count of 0.
    pub weights: Option<Vec<PoseMatcherWeightSet>>,
    pub weights_header: ArrayHeader,
    pub name: StrA,
    pub unknown_50h: u64,
    pub unknown_58h: u64,
    pub unknown_60h: Option<PoseMatcherUnknownData>,
    pub unknown_68h: f32,
    /// 1 or 2.
    pub unknown_6ch: u32,
    pub signature: u32,
    pub unknown_74h: u32,
    pub unknown_78h: u64,
}

impl Default for PoseMatcherItem {
    fn default() -> Self {
        Self {
            unknown_00h: 0,
            unknown_08h: 0,
            usage_count: 1,
            owner: 0,
            samples: None,
            samples_header: ArrayHeader::default(),
            weights: None,
            weights_header: ArrayHeader::default(),
            name: StrA::default(),
            unknown_50h: 0,
            unknown_58h: 0,
            unknown_60h: None,
            unknown_68h: 0.033333,
            unknown_6ch: 0,
            signature: 0,
            unknown_74h: 0,
            unknown_78h: 0,
        }
    }
}

/// `Rsc8PoseMatcherSample`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PoseMatcherSample {
    pub clip_set: u32,
    pub clip: u32,
    pub offset: f32,
    /// 7 or 6.
    pub unknown_0ch: u32,
    pub unknown_10h: u64,
    pub unknown_18h: u64,
    pub point_cloud: Option<PoseMatcherPointCloud>,
    pub unknown_28h: u64,
}

/// `Rsc8PoseMatcherPointCloud`.
#[derive(Clone, Debug, PartialEq)]
pub struct PoseMatcherPointCloud {
    pub points: Option<Vec<PoseMatcherPoint>>,
    pub points_header: ArrayHeader,
    pub weights: Option<Vec<f32>>,
    pub weights_header: ArrayHeader,
    pub bounds_min: [f32; 3],
    /// Always 0x7F7FFFFF.
    pub unknown_2ch: u32,
    pub bounds_max: [f32; 3],
    /// Always 0xFF7FFFFF.
    pub unknown_3ch: u32,
    pub weights_sum: f32,
    /// Written to XML as `Signature`.
    pub unknown_44h: u32,
    pub unknown_48h: u64,
}

impl Default for PoseMatcherPointCloud {
    fn default() -> Self {
        Self {
            points: None,
            points_header: ArrayHeader::default(),
            weights: None,
            weights_header: ArrayHeader::default(),
            bounds_min: [0.0; 3],
            unknown_2ch: 0x7F7F_FFFF,
            bounds_max: [0.0; 3],
            unknown_3ch: 0xFF7F_FFFF,
            weights_sum: 0.0,
            unknown_44h: 0,
            unknown_48h: 0,
        }
    }
}

/// `Rsc8PoseMatcherUnknownData`, which only points back to its item.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PoseMatcherUnknownData {
    pub unknown_00h: u64,
    pub unknown_08h: u64,
    /// The item: a back-pointer, kept raw.
    pub owner: u64,
    pub unknown_18h: u64,
    pub unknown_20h: u64,
    pub unknown_28h: u64,
}

/// `Rsc8PoseMatcherPoint`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct PoseMatcherPoint {
    pub point: [f32; 3],
    /// Flags?
    pub unknown_0ch: u32,
}

/// `Rsc8PoseMatcherWeightSet`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct PoseMatcherWeightSet {
    pub tag: u32,
    pub weight: f32,
}

/// The header of a `Rsc8Arr`/`Rsc8PtrArr` built from `n` items in C#.
fn header_for(n: usize) -> ArrayHeader {
    ArrayHeader { ptr: 0, count: n as u16, capacity: n as u16, unknown: 0 }
}

/// `ReadArray`: `None` for a null pointer or a count of 0.
fn read_array<T: Pod>(r: &Rsc8Reader, h: ArrayHeader) -> Result<Option<Vec<T>>> {
    if h.count == 0 {
        return Ok(None);
    }
    r.array_at(h.ptr, h.count as usize)
}

impl PoseMatcherDictionary {
    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let vft = r.u64()?;
        let block_map = r.ptr(BlockMap::read)?;
        let unknown_00h = r.u64()?;
        let unknown_08h = r.u64()?;
        let unknown_10h = r.u64()?;
        let unknown_18h = r.u64()?;
        let unknown_20h = r.u64()?;
        let name = StrA::read(r)?;
        let mh = ArrayHeader::read(r)?;
        let pose_matcher_map_count = r.u32()?;
        let unknown_4ch = r.u32()?;
        let unknown_50h = r.u64()?;
        let name_hash = r.u32()?;
        let unknown_5ch = r.u32()?;
        let heads = read_array::<u64>(r, mh)?.unwrap_or_default();
        let mut pose_matcher_map = Vec::with_capacity(heads.len());
        for head in heads {
            pose_matcher_map.push(read_chain(r, head, PoseMatcherEntry::read)?);
        }
        Ok(Self {
            vft,
            block_map,
            unknown_00h,
            unknown_08h,
            unknown_10h,
            unknown_18h,
            unknown_20h,
            name,
            pose_matcher_map,
            pose_matcher_map_header: mh,
            pose_matcher_map_count,
            unknown_4ch,
            unknown_50h,
            name_hash,
            unknown_5ch,
        })
    }

    pub fn write(&self) -> Vec<u8> {
        let mut w = Rsc8Writer::new();
        let root = w.reserve();
        let block_map = self.block_map.clone().unwrap_or_default().write(&mut w);
        let mut b = BlockBuf::with_capacity(0x70);
        b.u64(self.vft);
        b.ptr(Some(block_map));
        b.u64(self.unknown_00h);
        b.u64(self.unknown_08h);
        b.u64(self.unknown_10h);
        b.u64(self.unknown_18h);
        b.u64(self.unknown_20h);
        self.name.write(&mut b, &mut w);
        let heads: Vec<Option<BlockId>> = self
            .pose_matcher_map
            .iter()
            .map(|chain| chain.iter().rev().fold(None, |next, e| Some(e.write(&mut w, next))))
            .collect();
        // A map read with no buckets was null in the C#, which then writes no pointers.
        let map =
            if heads.is_empty() { None } else { write_pointer_array(&mut w, &heads, self.pose_matcher_map_header) };
        write_header(&mut b, map, self.pose_matcher_map_header);
        b.u32(self.pose_matcher_map_count);
        b.u32(self.unknown_4ch);
        b.u64(self.unknown_50h);
        b.u32(self.name_hash);
        b.u32(self.unknown_5ch);
        w.set(root, b, false);
        w.build(VERSION)
    }

    /// `Write(MetaNodeWriter)`. The C# fails on entries without an item or
    /// samples without a point cloud.
    pub fn write_xml(&self, w: &mut XmlWriter) -> Result<()> {
        let entries: Vec<&PoseMatcherItem> =
            self.pose_matcher_map.iter().flatten().filter_map(|e| e.item.as_ref()).collect();
        if entries.len() != self.pose_matcher_map.iter().map(Vec::len).sum::<usize>() {
            bail!("ypmd: an entry without an item (the C# fails with a NullReferenceException)");
        }
        let no_cloud =
            entries.iter().filter_map(|i| i.samples.as_ref()).flatten().flatten().any(|s| s.point_cloud.is_none());
        if no_cloud {
            bail!("ypmd: a sample without a point cloud (the C# fails with a NullReferenceException)");
        }
        w.string("Name", self.name.value.as_deref().unwrap_or(""));
        w.node_array("PoseMatchers", Some(&entries), |w, i| i.write_xml(w));
        Ok(())
    }

    /// `Read(MetaNodeReader)`.
    pub fn read_xml(node: XmlNode) -> Result<Self> {
        let name = StrA::new(read_string(node, "Name"));
        let Some(items) = node.node_array("PoseMatchers") else {
            bail!("ypmd: no PoseMatchers (the C# fails with a NullReferenceException)")
        };
        let mut entries = Vec::with_capacity(items.len());
        for item in items {
            let Some(item) = item else { bail!("ypmd: a null pose matcher (the C# fails on it)") };
            entries.push(PoseMatcherEntry::read_xml(item)?);
        }
        let count = entries.len() as u32;
        let pose_matcher_map = if entries.is_empty() {
            vec![Vec::new(); 11]
        } else {
            match build_map(entries, |e| e.hash) {
                Some(m) => m,
                None => bail!("ypmd: a single pose matcher with hash 0 (the C# fails on it)"),
            }
        };
        // `new Rsc8PtrArr<T>(buckets) { Capacity = 0 }`.
        let n = pose_matcher_map.len() as u16;
        Ok(Self {
            name,
            pose_matcher_map,
            pose_matcher_map_header: ArrayHeader { ptr: 0, count: n, capacity: 0, unknown: 0 },
            pose_matcher_map_count: count,
            ..Default::default()
        })
    }
}

/// `Rsc8PtrArr.Write`: max(count, capacity) pointers, and no pointer to them
/// when that's 0.
fn write_pointer_array(w: &mut Rsc8Writer, ids: &[Option<BlockId>], h: ArrayHeader) -> Option<BlockId> {
    let slots = h.count.max(h.capacity) as usize;
    if slots == 0 {
        return None;
    }
    let mut ptrs = ids.to_vec();
    ptrs.resize(slots, None);
    Some(w.pointer_array(&ptrs))
}

impl PoseMatcherEntry {
    /// Reads the entry and returns its `Next` pointer too.
    fn read(r: &mut Rsc8Reader) -> Result<(Self, u64)> {
        let hash = r.u32()?;
        let unknown_04h = r.u32()?;
        let item = r.ptr(PoseMatcherItem::read)?;
        let next = r.u64()?;
        let unknown_18h = r.u64()?;
        Ok((Self { hash, unknown_04h, item, unknown_18h }, next))
    }

    fn write(&self, w: &mut Rsc8Writer, next: Option<BlockId>) -> BlockId {
        let item = self.item.as_ref().map(|i| i.write(w));
        let mut b = BlockBuf::with_capacity(0x20);
        b.u32(self.hash);
        b.u32(self.unknown_04h);
        b.ptr(item);
        b.ptr(next);
        b.u64(self.unknown_18h);
        w.add(b, false)
    }

    /// The hash is the item's name without its first 6 and last 5 characters.
    fn read_xml(node: XmlNode) -> Result<Self> {
        let item = PoseMatcherItem::read_xml(node);
        let Some(name) = item.name.value.as_deref() else {
            bail!("ypmd: a pose matcher without a name (the C# fails with a NullReferenceException)")
        };
        let Some(key) = trim_name(name, 6, 5) else {
            bail!("ypmd: pose matcher name {name:?} is too short (the C# fails on it)")
        };
        Ok(Self { hash: gen_hash(&key), unknown_04h: 0, item: Some(item), unknown_18h: 0 })
    }
}

impl PoseMatcherItem {
    fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let unknown_00h = r.u64()?;
        let unknown_08h = r.u64()?;
        let usage_count = r.u64()?;
        let owner = r.u64()?;
        let sh = ArrayHeader::read(r)?;
        let samples = read_pointer_array(r, sh, PoseMatcherSample::read)?;
        let wh = ArrayHeader::read(r)?;
        let weights = read_array(r, wh)?;
        let name = StrA::read(r)?;
        let unknown_50h = r.u64()?;
        let unknown_58h = r.u64()?;
        let unknown_60h = r.ptr(PoseMatcherUnknownData::read)?;
        Ok(Self {
            unknown_00h,
            unknown_08h,
            usage_count,
            owner,
            samples,
            samples_header: sh,
            weights,
            weights_header: wh,
            name,
            unknown_50h,
            unknown_58h,
            unknown_60h,
            unknown_68h: r.f32()?,
            unknown_6ch: r.u32()?,
            signature: r.u32()?,
            unknown_74h: r.u32()?,
            unknown_78h: r.u64()?,
        })
    }

    fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let mut b = BlockBuf::with_capacity(0x80);
        b.u64(self.unknown_00h);
        b.u64(self.unknown_08h);
        b.u64(self.usage_count);
        b.u64(self.owner);
        // `Rsc8PtrArr.Write` adds the pointer array whenever there are items.
        let samples: Vec<Option<BlockId>> =
            self.samples.iter().flatten().map(|s| s.as_ref().map(|s| s.write(w))).collect();
        let samples_id =
            if self.samples.is_some() { write_pointer_array(w, &samples, self.samples_header) } else { None };
        write_header(&mut b, samples_id, self.samples_header);
        let weights = w.array(self.weights.as_deref());
        write_header(&mut b, weights, self.weights_header);
        self.name.write(&mut b, w);
        b.u64(self.unknown_50h);
        b.u64(self.unknown_58h);
        let unknown_60h = self.unknown_60h.as_ref().map(|u| u.write(w));
        b.ptr(unknown_60h);
        b.f32(self.unknown_68h);
        b.u32(self.unknown_6ch);
        b.u32(self.signature);
        b.u32(self.unknown_74h);
        b.u64(self.unknown_78h);
        w.add(b, false)
    }

    fn write_xml(&self, w: &mut XmlWriter) {
        w.string("Name", self.name.value.as_deref().unwrap_or(""));
        w.u32("Signature", self.signature);
        w.u32("Unknown6C", self.unknown_6ch);
        w.node_array("Weights", self.weights.as_deref(), |w, s| {
            nonzero(w, "Tag", s.tag, XmlWriter::u32);
            if s.weight != 0.0 {
                w.f32("Weight", s.weight);
            }
        });
        w.node_array_opt("Samples", self.samples.as_deref(), |w, s| s.write_xml(w));
    }

    fn read_xml(n: XmlNode) -> Self {
        let weights = read_struct_array(n, "Weights", |i| PoseMatcherWeightSet {
            tag: i.u32("Tag", 0),
            weight: i.f32("Weight", 0.0),
        });
        let samples = n
            .node_array("Samples")
            .map(|v| v.into_iter().map(|s| s.map(PoseMatcherSample::read_xml)).collect::<Vec<_>>());
        Self {
            name: StrA::new(read_string(n, "Name")),
            signature: n.u32("Signature", 0),
            unknown_6ch: n.u32("Unknown6C", 0),
            weights_header: header_for(weights.as_ref().map_or(0, Vec::len)),
            weights,
            samples_header: header_for(samples.as_ref().map_or(0, Vec::len)),
            samples,
            ..Default::default()
        }
    }
}

impl PoseMatcherSample {
    fn read(r: &mut Rsc8Reader) -> Result<Self> {
        Ok(Self {
            clip_set: r.u32()?,
            clip: r.u32()?,
            offset: r.f32()?,
            unknown_0ch: r.u32()?,
            unknown_10h: r.u64()?,
            unknown_18h: r.u64()?,
            point_cloud: r.ptr(PoseMatcherPointCloud::read)?,
            unknown_28h: r.u64()?,
        })
    }

    fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let cloud = self.point_cloud.as_ref().map(|c| c.write(w));
        let mut b = BlockBuf::with_capacity(0x30);
        b.u32(self.clip_set);
        b.u32(self.clip);
        b.f32(self.offset);
        b.u32(self.unknown_0ch);
        b.u64(self.unknown_10h);
        b.u64(self.unknown_18h);
        b.ptr(cloud);
        b.u64(self.unknown_28h);
        w.add(b, false)
    }

    /// The point cloud's fields go in the sample's node.
    fn write_xml(&self, w: &mut XmlWriter) {
        // `JenkHash.Str`, written as a plain string.
        let clip_set = w.hashes.xml_name(self.clip_set);
        let clip = w.hashes.xml_name(self.clip);
        w.string("ClipSet", &clip_set);
        w.string("Clip", &clip);
        w.f32("Offset", self.offset);
        w.u32("Unknown0C", self.unknown_0ch);
        if let Some(c) = &self.point_cloud {
            c.write_xml(w);
        }
    }

    fn read_xml(n: XmlNode) -> Self {
        Self {
            clip_set: read_string(n, "ClipSet").map_or(0, |s| gen_hash(&s)),
            clip: read_string(n, "Clip").map_or(0, |s| gen_hash(&s)),
            offset: n.f32("Offset", 0.0),
            unknown_0ch: n.u32("Unknown0C", 0),
            point_cloud: Some(PoseMatcherPointCloud::read_xml(n)),
            ..Default::default()
        }
    }
}

impl PoseMatcherPointCloud {
    fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let ph = ArrayHeader::read(r)?;
        let wh = ArrayHeader::read(r)?;
        let points = read_array(r, ph)?;
        if points.is_none() {
            bail!("ypmd: a point cloud without points (the C# fails with a NullReferenceException)");
        }
        Ok(Self {
            points,
            points_header: ph,
            weights: read_array(r, wh)?,
            weights_header: wh,
            bounds_min: r.vec3()?,
            unknown_2ch: r.u32()?,
            bounds_max: r.vec3()?,
            unknown_3ch: r.u32()?,
            weights_sum: r.f32()?,
            unknown_44h: r.u32()?,
            unknown_48h: r.u64()?,
        })
    }

    fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let points = w.array(self.points.as_deref());
        let weights = w.array(self.weights.as_deref());
        let mut b = BlockBuf::with_capacity(0x50);
        write_header(&mut b, points, self.points_header);
        write_header(&mut b, weights, self.weights_header);
        b.vec3(self.bounds_min);
        b.u32(self.unknown_2ch);
        b.vec3(self.bounds_max);
        b.u32(self.unknown_3ch);
        b.f32(self.weights_sum);
        b.u32(self.unknown_44h);
        b.u64(self.unknown_48h);
        w.add(b, false)
    }

    fn write_xml(&self, w: &mut XmlWriter) {
        let points = self.points.as_deref().unwrap_or_default();
        w.u32("Signature", self.unknown_44h);
        w.vec3("BoundsMin", self.bounds_min);
        w.vec3("BoundsMax", self.bounds_max);
        // `ConvertVector3ArrayToString`: one "x y z" line per point.
        let mut text = String::new();
        for p in points {
            let [x, y, z] = p.point.map(f32_to_string);
            text.push_str(&format!("{x} {y} {z}\n"));
        }
        w.string("Points", &text);
        let flags: Vec<u32> = points.iter().map(|p| p.unknown_0ch).collect();
        w.u32_array("PointFlags", Some(&flags));
        w.f32_array("Weights", self.weights.as_deref());
    }

    fn read_xml(n: XmlNode) -> Self {
        let vectors = read_string(n, "Points").map(|s| parse_points(&s)).unwrap_or_default();
        let flags = n.u32_array("PointFlags").unwrap_or_default();
        let points: Vec<PoseMatcherPoint> = vectors
            .iter()
            .enumerate()
            .map(|(i, &point)| PoseMatcherPoint { point, unknown_0ch: flags.get(i).copied().unwrap_or(0) })
            .collect();
        let weights = n.f32_array("Weights");
        let weights_sum = weights.iter().flatten().fold(0.0f32, |s, w| s + w);
        Self {
            unknown_44h: n.u32("Signature", 0),
            bounds_min: n.vec3("BoundsMin"),
            bounds_max: n.vec3("BoundsMax"),
            points_header: header_for(points.len()),
            points: Some(points),
            weights_header: header_for(weights.as_ref().map_or(0, Vec::len)),
            weights,
            weights_sum,
            ..Default::default()
        }
    }

    /// The checks the C#'s `Read(MetaNodeReader)` fails on.
    fn check_xml(n: XmlNode) -> Result<()> {
        let Some(text) = read_string(n, "Points") else {
            bail!("ypmd: a sample without Points (the C# fails with a NullReferenceException)")
        };
        let count = parse_points(&text).len();
        let flags = n.u32_array("PointFlags").map_or(0, |f| f.len());
        if count > 0 && flags < count {
            bail!("ypmd: {count} points but {flags} point flags (the C# fails on it)");
        }
        Ok(())
    }
}

impl PoseMatcherUnknownData {
    fn read(r: &mut Rsc8Reader) -> Result<Self> {
        Ok(Self {
            unknown_00h: r.u64()?,
            unknown_08h: r.u64()?,
            owner: r.u64()?,
            unknown_18h: r.u64()?,
            unknown_20h: r.u64()?,
            unknown_28h: r.u64()?,
        })
    }

    fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let mut b = BlockBuf::with_capacity(0x30);
        b.u64(self.unknown_00h);
        b.u64(self.unknown_08h);
        b.u64(self.owner);
        b.u64(self.unknown_18h);
        b.u64(self.unknown_20h);
        b.u64(self.unknown_28h);
        w.add(b, false)
    }
}

/// `ConvertVector3ArrayStringToArray`: a vector per non-blank line, each parsed
/// with `FloatUtil.ParseVector3String`, which splits on commas. The C# writes
/// the values separated by spaces, so they don't parse and come back as 0.
fn parse_points(text: &str) -> Vec<[f32; 3]> {
    text.split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let mut v = [0.0f32; 3];
            for (d, s) in v.iter_mut().zip(l.split(',')) {
                *d = parse_f32(s.trim()).unwrap_or(0.0);
            }
            v
        })
        .collect()
}

/// Checks what the C# fails on while reading the XML, before reading it.
fn check_xml(root: XmlNode) -> Result<()> {
    for item in root.node_array("PoseMatchers").into_iter().flatten().flatten() {
        for sample in item.node_array("Samples").into_iter().flatten().flatten() {
            PoseMatcherPointCloud::check_xml(sample)?;
        }
    }
    Ok(())
}

pub fn read(file: &[u8]) -> Result<PoseMatcherDictionary> {
    read_resource(file, |r, _| PoseMatcherDictionary::read(r))
}

pub fn to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let d = read(file)?;
    let mut result = Ok(());
    let xml = XmlWriter::document(cx.hashes, "RDR2PoseMatcherDictionary", None, |w| result = d.write_xml(w));
    result.map(|_| xml)
}

pub fn from_xml(xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    let doc = XmlDocument::new(xml);
    let d = doc.with_root(None, |root| check_xml(root).and_then(|_| PoseMatcherDictionary::read_xml(root)))??;
    Ok(d.write())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::HashIndex;

    #[test]
    fn points_written_with_spaces_read_back_as_zero() {
        assert_eq!(parse_points("\n1 2 3\n  -4 5 6\n   "), vec![[0.0; 3], [0.0; 3]]);
        assert_eq!(parse_points("1, 2,3"), vec![[1.0, 2.0, 3.0]]);
        assert!(parse_points("").is_empty());
    }

    #[test]
    fn round_trips_through_xml() {
        let hashes = HashIndex::new();
        let cx = XmlContext { hashes: &hashes, folder: None };
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<RDR2PoseMatcherDictionary>
 <Name>pack:/dict</Name>
 <PoseMatchers>
  <Item>
   <Name>pack:/matcher.pmdx</Name>
   <Signature value="123" />
   <Unknown6C value="2" />
   <Weights>
    <Item>
     <Tag value="7" />
     <Weight value="0.5" />
    </Item>
   </Weights>
   <Samples>
    <Item>
     <ClipSet>sets</ClipSet>
     <Clip>clip</Clip>
     <Offset value="0.25" />
     <Unknown0C value="7" />
     <Signature value="9" />
     <BoundsMin x="-1" y="-2" z="-3" />
     <BoundsMax x="1" y="2" z="3" />
     <Points>
1 2 3
4 5 6
     </Points>
     <PointFlags>10 11</PointFlags>
     <Weights>0.5 0.25</Weights>
    </Item>
   </Samples>
  </Item>
 </PoseMatchers>
</RDR2PoseMatcherDictionary>
"#;
        let d = read(&from_xml(xml, &cx).unwrap()).unwrap();
        assert_eq!(d.pose_matcher_map.len(), 11);
        assert_eq!(d.unknown_4ch, 0x100_0000);
        let e = d.pose_matcher_map.iter().flatten().next().unwrap();
        assert_eq!(e.hash, gen_hash("matcher"));
        let item = e.item.as_ref().unwrap();
        assert_eq!((item.usage_count, item.unknown_68h, item.signature), (1, 0.033333, 123));
        let s = item.samples.as_ref().unwrap()[0].as_ref().unwrap();
        assert_eq!(s.clip_set, gen_hash("sets"));
        let c = s.point_cloud.as_ref().unwrap();
        assert_eq!(c.points.as_ref().unwrap()[1], PoseMatcherPoint { point: [0.0; 3], unknown_0ch: 11 });
        assert_eq!(c.weights_sum, 0.75);
        assert_eq!((c.unknown_2ch, c.unknown_3ch), (0x7F7F_FFFF, 0xFF7F_FFFF));

        let out = to_xml(&from_xml(xml, &cx).unwrap(), &cx).unwrap();
        assert!(out.contains("     <Points>\n0 0 0\n0 0 0\n     </Points>\n"), "{out}");
        assert!(out.contains("     <PointFlags>10 11</PointFlags>\n"), "{out}");
    }
}
