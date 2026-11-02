//! Frame filter dictionaries (yfd), from `Rsc8Filter.cs`: named lists of bone
//! weights for animations.

use super::XmlContext;
use super::yldb::{StrA, gen_hash, read_string, trim_name};
use crate::rsc8::{ArrayHeader, BlockBuf, BlockId, BlockMap, Rsc8Reader, Rsc8Writer, read_resource};
use crate::xml::{XmlDocument, XmlNode, XmlWriter};
use crate::{Result, bail};

/// The resource version `YfdFile.Save` builds with.
const VERSION: u32 = 5;

/// `Rsc8FrameFilterDictionary`.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameFilterDictionary {
    pub vft: u64,
    pub block_map: Option<BlockMap>,
    pub unknown_10h: u32,
    pub unknown_14h: u32,
    pub unknown_18h: u32,
    pub unknown_1ch: u32,
    /// The filters' names hashed, as read. Writing recomputes them.
    pub filter_name_hashes: Option<Vec<u32>>,
    pub filter_name_hashes_header: ArrayHeader,
    /// `None` for a null pointer or a count of 0; items for null pointers are `None`.
    pub filters: Option<Vec<Option<FrameFilterMultiWeight>>>,
    pub filters_header: ArrayHeader,
}

impl Default for FrameFilterDictionary {
    fn default() -> Self {
        Self {
            vft: 0,
            block_map: None,
            unknown_10h: 0,
            unknown_14h: 0,
            unknown_18h: 1,
            unknown_1ch: 0,
            filter_name_hashes: None,
            filter_name_hashes_header: ArrayHeader::default(),
            filters: None,
            filters_header: ArrayHeader::default(),
        }
    }
}

/// `Rsc8FrameFilterMultiWeight`.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameFilterMultiWeight {
    pub unknown_00h: u32,
    pub unknown_04h: u32,
    pub ref_count: u32,
    pub signature: u32,
    /// `Rsc8FrameFilterType`: 14 (MultiWeight).
    pub filter_type: u32,
    pub unknown_14h: u32,
    pub name: StrA,
    /// `None` for a null pointer. Items are only `None` when read from XML
    /// (`<Item type="null" />`).
    pub entries: Option<Vec<Option<FrameFilterTrackIdIndex>>>,
    pub entries_header: ArrayHeader,
    pub unknown_38h: u64,
}

impl Default for FrameFilterMultiWeight {
    fn default() -> Self {
        Self {
            unknown_00h: 0,
            unknown_04h: 0,
            ref_count: 1,
            signature: 0,
            filter_type: 14,
            unknown_14h: 0,
            name: StrA::default(),
            entries: None,
            entries_header: ArrayHeader::default(),
            unknown_38h: 0,
        }
    }
}

/// `Rsc8FrameFilterTrackIdIndex`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameFilterTrackIdIndex {
    pub unknown_00h: u8,
    /// `rage::crTrack`.
    pub track: u8,
    /// `rage::crId`.
    pub bone_id: u16,
    pub weight_value: f32,
    pub unknown_08h: StrA,
    pub unknown_18h: f32,
    pub unknown_1ch: f32,
    pub unknown_20h: f32,
    pub unknown_24h: f32,
}

/// The header of a `Rsc8Arr`/`Rsc8Lst`/`Rsc8PtrArr` built from `n` items in C#.
fn header_for(n: usize) -> ArrayHeader {
    ArrayHeader { ptr: 0, count: n as u16, capacity: n as u16, unknown: 0 }
}

/// The hash `ConstructHashList` gives a filter: its name without the first 6
/// and the last 4 characters.
fn name_hash(name: Option<&str>) -> Result<u32> {
    let Some(name) = name else { bail!("yfd: a filter without a name (the C# fails on it)") };
    match trim_name(name, 6, 4) {
        Some(n) => Ok(gen_hash(&n)),
        None => bail!("yfd: filter name {name:?} is too short (the C# fails on it)"),
    }
}

impl FrameFilterDictionary {
    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let vft = r.u64()?;
        let block_map = r.ptr(BlockMap::read)?;
        let unknown_10h = r.u32()?;
        let unknown_14h = r.u32()?;
        let unknown_18h = r.u32()?;
        let unknown_1ch = r.u32()?;
        let hh = ArrayHeader::read(r)?;
        let filter_name_hashes = if hh.count == 0 { None } else { r.array_at::<u32>(hh.ptr, hh.count as usize)? };
        let fh = ArrayHeader::read(r)?;
        let filters = read_pointer_array(r, fh, FrameFilterMultiWeight::read)?;
        Ok(Self {
            vft,
            block_map,
            unknown_10h,
            unknown_14h,
            unknown_18h,
            unknown_1ch,
            filter_name_hashes,
            filter_name_hashes_header: hh,
            filters,
            filters_header: fh,
        })
    }

    /// `Write(Rsc8DataWriter)`, which first rebuilds the name hashes and sorts
    /// the filters by them (`ConstructHashList` and `Sort`).
    pub fn write(&self) -> Result<Vec<u8>> {
        let Some(filters) = &self.filters else {
            bail!("yfd: no filters (the C# fails with a NullReferenceException)")
        };
        let mut sorted = Vec::with_capacity(filters.len());
        for f in filters {
            let Some(f) = f else { bail!("yfd: a null filter (the C# fails on it)") };
            sorted.push((name_hash(f.name.value.as_deref())?, f));
        }
        sorted.sort_by_key(|(h, _)| *h);
        if sorted.windows(2).any(|p| p[0].0 == p[1].0) {
            bail!("yfd: two filters with the same name hash (the C# fails adding them to a Dictionary)");
        }

        let mut w = Rsc8Writer::new();
        let root = w.reserve();
        let block_map = self.block_map.clone().unwrap_or_default().write(&mut w);
        let hashes: Vec<u32> = sorted.iter().map(|(h, _)| *h).collect();
        let hashes_id = w.array(Some(&hashes));
        let ids: Vec<Option<BlockId>> = sorted.iter().map(|(_, f)| Some(f.write(&mut w))).collect();
        let filters_id = (!ids.is_empty()).then(|| w.pointer_array(&ids));

        let mut b = BlockBuf::with_capacity(0x40);
        b.u64(self.vft);
        b.ptr(Some(block_map));
        b.u32(self.unknown_10h);
        b.u32(self.unknown_14h);
        b.u32(self.unknown_18h);
        b.u32(self.unknown_1ch);
        write_header(&mut b, hashes_id, header_for(hashes.len()));
        write_header(&mut b, filters_id, header_for(ids.len()));
        w.set(root, b, false);
        Ok(w.build(VERSION))
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.node_array_opt("WeightFilters", self.filters.as_deref(), |w, f| f.write_xml(w));
    }

    pub fn read_xml(node: XmlNode) -> Self {
        let filters = node
            .node_array("WeightFilters")
            .map(|v| v.into_iter().map(|n| n.map(FrameFilterMultiWeight::read_xml)).collect::<Vec<_>>());
        let filters_header = header_for(filters.as_ref().map_or(0, Vec::len));
        Self { filters, filters_header, ..Default::default() }
    }
}

impl FrameFilterMultiWeight {
    fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let unknown_00h = r.u32()?;
        let unknown_04h = r.u32()?;
        let ref_count = r.u32()?;
        let signature = r.u32()?;
        let filter_type = r.u32()?;
        let unknown_14h = r.u32()?;
        let name = StrA::read(r)?;
        let eh = ArrayHeader::read(r)?;
        let entries = if eh.ptr == 0 {
            None
        } else {
            r.items_at(eh.ptr, eh.count as usize, |r| FrameFilterTrackIdIndex::read(r).map(Some))?
        };
        let unknown_38h = r.u64()?;
        Ok(Self {
            unknown_00h,
            unknown_04h,
            ref_count,
            signature,
            filter_type,
            unknown_14h,
            name,
            entries,
            entries_header: eh,
            unknown_38h,
        })
    }

    fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        // `WriteBlocks`: all the entries in one block, none when the first is null.
        let entries = match self.entries.as_deref() {
            Some(items @ [Some(_), ..]) => {
                let mut b = BlockBuf::with_capacity(items.len() * 0x28);
                for e in items {
                    match e {
                        Some(e) => e.write(&mut b, w),
                        None => b.zeros(0x28),
                    }
                }
                Some(w.add(b, false))
            }
            _ => None,
        };
        let mut b = BlockBuf::with_capacity(0x40);
        b.u32(self.unknown_00h);
        b.u32(self.unknown_04h);
        b.u32(self.ref_count);
        b.u32(self.signature);
        b.u32(self.filter_type);
        b.u32(self.unknown_14h);
        self.name.write(&mut b, w);
        write_header(&mut b, entries, self.entries_header);
        b.u64(self.unknown_38h);
        w.add(b, false)
    }

    fn write_xml(&self, w: &mut XmlWriter) {
        w.string("Name", self.name.value.as_deref().unwrap_or(""));
        w.node_array_opt("Entries", self.entries.as_deref(), |w, e| e.write_xml(w));
    }

    fn read_xml(node: XmlNode) -> Self {
        let entries = node
            .node_array("Entries")
            .map(|v| v.into_iter().map(|n| n.map(FrameFilterTrackIdIndex::read_xml)).collect::<Vec<_>>());
        let entries_header = header_for(entries.as_ref().map_or(0, Vec::len));
        Self { name: StrA::new(read_string(node, "Name")), entries, entries_header, ..Default::default() }
    }
}

impl FrameFilterTrackIdIndex {
    fn read(r: &mut Rsc8Reader) -> Result<Self> {
        Ok(Self {
            unknown_00h: r.u8()?,
            track: r.u8()?,
            bone_id: r.u16()?,
            weight_value: r.f32()?,
            unknown_08h: StrA::read(r)?,
            unknown_18h: r.f32()?,
            unknown_1ch: r.f32()?,
            unknown_20h: r.f32()?,
            unknown_24h: r.f32()?,
        })
    }

    fn write(&self, b: &mut BlockBuf, w: &mut Rsc8Writer) {
        b.u8(self.unknown_00h);
        b.u8(self.track);
        b.u16(self.bone_id);
        b.f32(self.weight_value);
        self.unknown_08h.write(b, w);
        b.f32(self.unknown_18h);
        b.f32(self.unknown_1ch);
        b.f32(self.unknown_20h);
        b.f32(self.unknown_24h);
    }

    fn write_xml(&self, w: &mut XmlWriter) {
        w.u8("Unknown00", self.unknown_00h);
        w.u8("Track", self.track);
        w.u16("BoneId", self.bone_id);
        w.f32("Weight", self.weight_value);
        w.opt_string("Unknown08", self.unknown_08h.value.as_deref());
    }

    fn read_xml(n: XmlNode) -> Self {
        Self {
            unknown_00h: n.u8("Unknown00", 0),
            track: n.u8("Track", 0),
            bone_id: n.u16("BoneId", 0),
            weight_value: n.f32("Weight", 0.0),
            unknown_08h: StrA::new(read_string(n, "Unknown08")),
            ..Default::default()
        }
    }
}

/// `Rsc8PtrArr.Read`: `None` for a null pointer or a count of 0.
pub(crate) fn read_pointer_array<T>(
    r: &mut Rsc8Reader,
    h: ArrayHeader,
    mut read: impl FnMut(&mut Rsc8Reader) -> Result<T>,
) -> Result<Option<Vec<Option<T>>>> {
    if h.count == 0 {
        return Ok(None);
    }
    let Some(ptrs) = r.array_at::<u64>(h.ptr, h.count as usize)? else { return Ok(None) };
    ptrs.into_iter().map(|p| r.follow(p, &mut read)).collect::<Result<Vec<_>>>().map(Some)
}

/// An array header pointing to `ptr`.
pub(crate) fn write_header(b: &mut BlockBuf, ptr: Option<BlockId>, h: ArrayHeader) {
    b.ptr(ptr);
    b.u16(h.count);
    b.u16(h.capacity);
    b.u32(h.unknown);
}

pub fn read(file: &[u8]) -> Result<FrameFilterDictionary> {
    read_resource(file, |r, _| FrameFilterDictionary::read(r))
}

pub fn to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let d = read(file)?;
    Ok(XmlWriter::document(cx.hashes, "RDR2FrameFilterDictionary", None, |w| d.write_xml(w)))
}

pub fn from_xml(xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    let d = XmlDocument::new(xml).with_root(None, FrameFilterDictionary::read_xml)?;
    d.write()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::{HashIndex, joaat};

    #[test]
    fn filters_sort_by_name_hash() {
        let hashes = HashIndex::new();
        let cx = XmlContext { hashes: &hashes, folder: None };
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<RDR2FrameFilterDictionary>
 <WeightFilters>
  <Item>
   <Name>prefixzebra.ext</Name>
   <Entries>
    <Item>
     <Unknown00 value="0" />
     <Track value="2" />
     <BoneId value="11816" />
     <Weight value="0.5" />
    </Item>
   </Entries>
  </Item>
  <Item>
   <Name>prefixapple.ext</Name>
   <Entries />
  </Item>
 </WeightFilters>
</RDR2FrameFilterDictionary>
"#;
        let d = read(&from_xml(xml, &cx).unwrap()).unwrap();
        let (a, z) = (joaat("apple"), joaat("zebra"));
        let mut want = vec![a, z];
        want.sort();
        assert_eq!(d.filter_name_hashes, Some(want.clone()));
        let names: Vec<u32> = d
            .filters
            .as_ref()
            .unwrap()
            .iter()
            .map(|f| joaat(&f.as_ref().unwrap().name.value.as_deref().unwrap()[6..11]))
            .collect();
        assert_eq!(names, want);
        let zebra = d.filters.unwrap().into_iter().flatten().find(|f| f.entries.is_some()).unwrap();
        assert_eq!(zebra.ref_count, 1);
        assert_eq!(zebra.filter_type, 14);
        let e = zebra.entries.unwrap()[0].clone().unwrap();
        assert_eq!((e.track, e.bone_id, e.weight_value), (2, 11816, 0.5));
    }
}
