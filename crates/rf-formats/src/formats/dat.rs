//! `slodindexlist.dat` and `imposterextents.dat`, from `DatFiles.cs`: plain
//! arrays of structs (not resources), written to XML field by field like the
//! C#'s reflection-based struct writer (a field only when it isn't 0).

use bytemuck::{Pod, Zeroable};

use super::XmlContext;
use super::yldb::read_hash;
use super::yrecords::{nonzero, read_struct_array};
use crate::xml::{XmlDocument, XmlNode, XmlWriter};
use crate::{Result, bail};

/// `SlodIndexListItem`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct SlodIndexListItem {
    pub x_center: f32,
    pub x_scale: f32,
    pub y_center: f32,
    pub y_scale: f32,
    pub lod_dist_min: f32,
    pub lod_dist_max: f32,
    /// -1, 183, ...
    pub array_slice: i32,
    pub unk0: f32,
    pub unk1: f32,
    pub alpha_ref: f32,
    pub alpha_scale: f32,
}

/// The XML names of `SlodIndexListItem`'s float fields, in order.
const SLOD_FLOATS: [&str; 10] =
    ["XCenter", "XScale", "YCenter", "YScale", "LodDistMin", "LodDistMax", "Unk0", "Unk1", "AlphaRef", "AlphaScale"];

impl SlodIndexListItem {
    fn floats(&self) -> [f32; 10] {
        let s = self;
        [
            s.x_center,
            s.x_scale,
            s.y_center,
            s.y_scale,
            s.lod_dist_min,
            s.lod_dist_max,
            s.unk0,
            s.unk1,
            s.alpha_ref,
            s.alpha_scale,
        ]
    }

    fn write_xml(&self, w: &mut XmlWriter) {
        let f = self.floats();
        for (name, v) in SLOD_FLOATS[..6].iter().zip(&f[..6]) {
            nonzero_f32(w, name, *v);
        }
        nonzero(w, "ArraySlice", self.array_slice, XmlWriter::i32);
        for (name, v) in SLOD_FLOATS[6..].iter().zip(&f[6..]) {
            nonzero_f32(w, name, *v);
        }
    }

    fn read_xml(n: XmlNode) -> Self {
        let f = SLOD_FLOATS.map(|id| n.f32(id, 0.0));
        Self {
            x_center: f[0],
            x_scale: f[1],
            y_center: f[2],
            y_scale: f[3],
            lod_dist_min: f[4],
            lod_dist_max: f[5],
            array_slice: n.i32("ArraySlice", 0),
            unk0: f[6],
            unk1: f[7],
            alpha_ref: f[8],
            alpha_scale: f[9],
        }
    }
}

/// `ImposterExtentsItem`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct ImposterExtentsItem {
    pub hash: u32,
    /// `MinY`, `MinYb`, `MaxY`, `MaxYb`, then `MinX1`, `MinX1b`, `MaxX1`,
    /// `MaxX1b` and so on up to `MaxX8b` (see [`IMPOSTER_FIELDS`]).
    pub extents: [i16; 36],
}

impl Default for ImposterExtentsItem {
    fn default() -> Self {
        Self::zeroed()
    }
}

/// The XML names of `ImposterExtentsItem`'s 36 shorts, in order.
pub const IMPOSTER_FIELDS: [&str; 36] = [
    "MinY", "MinYb", "MaxY", "MaxYb", "MinX1", "MinX1b", "MaxX1", "MaxX1b", "MinX2", "MinX2b", "MaxX2", "MaxX2b",
    "MinX3", "MinX3b", "MaxX3", "MaxX3b", "MinX4", "MinX4b", "MaxX4", "MaxX4b", "MinX5", "MinX5b", "MaxX5", "MaxX5b",
    "MinX6", "MinX6b", "MaxX6", "MaxX6b", "MinX7", "MinX7b", "MaxX7", "MaxX7b", "MinX8", "MinX8b", "MaxX8", "MaxX8b",
];

impl ImposterExtentsItem {
    fn write_xml(&self, w: &mut XmlWriter) {
        if self.hash != 0 {
            w.hash("Hash", self.hash);
        }
        for (name, v) in IMPOSTER_FIELDS.iter().zip(self.extents) {
            nonzero(w, name, v, XmlWriter::i16);
        }
    }

    fn read_xml(n: XmlNode, cx: &XmlContext) -> Self {
        Self { hash: read_hash(n, "Hash", cx), extents: IMPOSTER_FIELDS.map(|id| n.i16(id, 0)) }
    }
}

/// `WriteBasicType` for a float: written unless it equals 0 (so not for -0 either).
fn nonzero_f32(w: &mut XmlWriter, id: &str, v: f32) {
    if v != 0.0 {
        w.f32(id, v);
    }
}

/// `BufferUtil.ReadArray`: `count` items from `data[at..]` (`None` for none),
/// zero-filled past the end of the data.
fn read_items<T: Pod>(data: &[u8], at: usize, count: usize) -> Option<Vec<T>> {
    if count == 0 {
        return None;
    }
    let mut bytes = vec![0u8; count * size_of::<T>()];
    let avail = data.len().saturating_sub(at).min(bytes.len());
    bytes[..avail].copy_from_slice(&data[at..at + avail]);
    Some(bytemuck::pod_collect_to_vec(&bytes))
}

/// `SlodIndexListDatFile.Load`: as many 44-byte items as fit.
pub fn read_slod_index_list(data: &[u8]) -> Option<Vec<SlodIndexListItem>> {
    read_items(data, 0, data.len() / size_of::<SlodIndexListItem>())
}

/// `ImposterExtentsDatFile.Load`: a count, then the items; `None` (an empty
/// XML) when the size doesn't match the count.
pub fn read_imposter_extents(data: &[u8]) -> Result<Option<Vec<ImposterExtentsItem>>> {
    let Some(head) = data.get(0..4) else { bail!("imposterextents.dat: {} bytes, too short for a count", data.len()) };
    let count = i32::from_le_bytes(head.try_into().unwrap());
    // The C# compares in 32-bit arithmetic.
    if data.len() as i32 != count.wrapping_mul(76).wrapping_add(4) {
        return Ok(None);
    }
    if count < 0 {
        bail!("imposterextents.dat: negative count {count}");
    }
    Ok(read_items(data, 4, count as usize))
}

pub fn slod_index_list_to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let items = read_slod_index_list(file);
    Ok(XmlWriter::document(cx.hashes, "RDR2SlodIndexList", None, |w| {
        // Each item gets its index as a `slodGUID` attribute.
        w.node_array_with(
            "Items",
            items.as_deref(),
            |_, _| None,
            |w, i, item| {
                w.i32("@slodGUID", i as i32);
                item.write_xml(w);
            },
            false,
        );
    }))
}

pub fn slod_index_list_from_xml(xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    let items =
        XmlDocument::new(xml).with_root(None, |root| read_struct_array(root, "Items", SlodIndexListItem::read_xml))?;
    let Some(items) = items else { bail!("slodindexlist.dat: no Items (the C# writes nothing)") };
    Ok(bytemuck::cast_slice(&items).to_vec())
}

pub fn imposter_extents_to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let items = read_imposter_extents(file)?;
    Ok(XmlWriter::document(cx.hashes, "RDR2ImposterExtents", None, |w| {
        w.node_array("Items", items.as_deref(), |w, item| item.write_xml(w));
    }))
}

pub fn imposter_extents_from_xml(xml: &str, cx: &XmlContext) -> Result<Vec<u8>> {
    let items = XmlDocument::new(xml)
        .with_root(None, |root| read_struct_array(root, "Items", |n| ImposterExtentsItem::read_xml(n, cx)))?;
    let Some(items) = items else { bail!("imposterextents.dat: no Items (the C# writes nothing)") };
    let mut out = (items.len() as i32).to_le_bytes().to_vec();
    out.extend_from_slice(bytemuck::cast_slice(&items));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::HashIndex;

    #[test]
    fn sizes_match_the_game() {
        assert_eq!(size_of::<SlodIndexListItem>(), 44);
        assert_eq!(size_of::<ImposterExtentsItem>(), 76);
    }

    #[test]
    fn round_trips() {
        let hashes = HashIndex::new();
        let cx = XmlContext { hashes: &hashes, folder: None };
        let slod = [SlodIndexListItem { x_center: -0.119, array_slice: -1, alpha_scale: 0.75, ..Default::default() }];
        let mut file = bytemuck::cast_slice(&slod).to_vec();
        file.extend_from_slice(&[1, 2, 3]); // ignored, like a partial item
        let xml = slod_index_list_to_xml(&file, &cx).unwrap();
        assert!(xml.contains("<Item slodGUID=\"0\">"), "{xml}");
        assert!(xml.contains("<ArraySlice value=\"-1\" />"), "{xml}");
        assert!(!xml.contains("XScale"), "{xml}");
        assert_eq!(slod_index_list_from_xml(&xml, &cx).unwrap(), file[..44]);

        let mut item = ImposterExtentsItem { hash: 0x1234_5678, ..Default::default() };
        item.extents[5] = -7;
        let mut file = 1i32.to_le_bytes().to_vec();
        file.extend_from_slice(bytemuck::bytes_of(&item));
        let xml = imposter_extents_to_xml(&file, &cx).unwrap();
        assert!(xml.contains("<MinX1b value=\"-7\" />"), "{xml}");
        assert_eq!(imposter_extents_from_xml(&xml, &cx).unwrap(), file);
        // A size that doesn't match the count gives an empty document.
        file.push(0);
        assert_eq!(imposter_extents_to_xml(&file, &cx).unwrap().lines().nth(1), Some("<RDR2ImposterExtents />"));
    }
}
