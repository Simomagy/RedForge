//! Vehicle records (yvr) and waypoint records (ywr), from `Rsc8Records.cs`:
//! arrays of plain structs, written to XML field by field the way the C#'s
//! reflection-based `WriteStructArray` does (a field only when it isn't 0).

use bytemuck::{Pod, Zeroable};

use super::XmlContext;
use crate::Result;
use crate::rsc8::{ArrayHeader, BlockBuf, BlockMap, Rsc8Reader, Rsc8Writer, read_resource};
use crate::xml::{XmlDocument, XmlNode, XmlWriter};

/// The resource version `YvrFile.Save` and `YwrFile.Save` build with.
const VERSION: u32 = 1;

/// `Rsc8VehicleRecordList`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VehicleRecordList {
    pub vft: u64,
    pub block_map: Option<BlockMap>,
    /// `None` for a null pointer or a count of 0, as the C# reads them.
    pub entries: Option<Vec<VehicleRecord>>,
    pub entries_count: u16,
    pub entries_capacity: u16,
    pub entries_unknown: u32,
}

/// `Rsc8VehicleRecord`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct VehicleRecord {
    pub time: u32,
    pub velocity_x: i16,
    pub velocity_y: i16,
    pub velocity_z: i16,
    pub right_x: i8,
    pub right_y: i8,
    pub right_z: i8,
    pub forward_x: i8,
    pub forward_y: i8,
    pub forward_z: i8,
    pub steering_angle: i8,
    pub gas_pedal_power: i8,
    pub brake_pedal_power: i8,
    pub handbrake_used: u8,
    pub position: [f32; 3],
}

/// `Rsc8WaypointRecordList`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WaypointRecordList {
    pub vft: u64,
    pub block_map: Option<BlockMap>,
    pub unknown_10h: u32,
    pub unknown_14h: u32,
    /// `None` for a null pointer or a count of 0, as the C# reads them.
    pub entries: Option<Vec<WaypointRecord>>,
    pub entries_count: u32,
    pub unknown_24h: u32,
    pub unknown_28h: u32,
    pub unknown_2ch: u32,
}

/// `Rsc8WaypointRecord`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct WaypointRecord {
    pub position: [f32; 3],
    pub flags: u16,
    pub heading: u8,
    pub move_blend_ratio: u8,
    pub free_space_on_left: u8,
    pub free_space_on_right: u8,
    pub unused: u16,
}

/// `ReadArray`: `None` for a null pointer, and (unlike `array_at`) for a count of 0.
fn read_array<T: Pod>(r: &Rsc8Reader, ptr: u64, count: usize) -> Result<Option<Vec<T>>> {
    if count == 0 {
        return Ok(None);
    }
    r.array_at(ptr, count)
}

impl VehicleRecordList {
    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let vft = r.u64()?;
        let block_map = r.ptr(BlockMap::read)?;
        let h = ArrayHeader::read(r)?;
        Ok(Self {
            vft,
            block_map,
            entries: read_array(r, h.ptr, h.count as usize)?,
            entries_count: h.count,
            entries_capacity: h.capacity,
            entries_unknown: h.unknown,
        })
    }

    pub fn write(&self) -> Vec<u8> {
        let mut w = Rsc8Writer::new();
        let root = w.reserve();
        let block_map = self.block_map.clone().unwrap_or_default().write(&mut w);
        let entries = w.array(self.entries.as_deref());
        let mut b = BlockBuf::with_capacity(0x20);
        b.u64(self.vft);
        b.ptr(Some(block_map));
        b.ptr(entries);
        b.u16(self.entries_count);
        b.u16(self.entries_capacity);
        b.u32(self.entries_unknown);
        w.set(root, b, false);
        w.build(VERSION)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.node_array("Entries", self.entries.as_deref(), |w, e| e.write_xml(w));
    }

    pub fn read_xml(node: XmlNode) -> Self {
        let entries = read_struct_array(node, "Entries", VehicleRecord::read_xml);
        // `new Rsc8Arr<T>(items)`.
        let count = entries.as_ref().map_or(0, |e| e.len() as u16);
        Self { entries, entries_count: count, entries_capacity: count, ..Default::default() }
    }
}

impl VehicleRecord {
    fn write_xml(&self, w: &mut XmlWriter) {
        nonzero(w, "Time", self.time, XmlWriter::u32);
        nonzero(w, "VelocityX", self.velocity_x, XmlWriter::i16);
        nonzero(w, "VelocityY", self.velocity_y, XmlWriter::i16);
        nonzero(w, "VelocityZ", self.velocity_z, XmlWriter::i16);
        nonzero(w, "RightX", self.right_x, XmlWriter::i8);
        nonzero(w, "RightY", self.right_y, XmlWriter::i8);
        nonzero(w, "RightZ", self.right_z, XmlWriter::i8);
        nonzero(w, "ForwardX", self.forward_x, XmlWriter::i8);
        nonzero(w, "ForwardY", self.forward_y, XmlWriter::i8);
        nonzero(w, "ForwardZ", self.forward_z, XmlWriter::i8);
        nonzero(w, "SteeringAngle", self.steering_angle, XmlWriter::i8);
        nonzero(w, "GasPedalPower", self.gas_pedal_power, XmlWriter::i8);
        nonzero(w, "BrakePedalPower", self.brake_pedal_power, XmlWriter::i8);
        nonzero(w, "HandbrakeUsed", self.handbrake_used, XmlWriter::u8);
        nonzero_vec3(w, "Position", self.position);
    }

    fn read_xml(n: XmlNode) -> Self {
        Self {
            time: n.u32("Time", 0),
            velocity_x: n.i16("VelocityX", 0),
            velocity_y: n.i16("VelocityY", 0),
            velocity_z: n.i16("VelocityZ", 0),
            right_x: n.i8("RightX", 0),
            right_y: n.i8("RightY", 0),
            right_z: n.i8("RightZ", 0),
            forward_x: n.i8("ForwardX", 0),
            forward_y: n.i8("ForwardY", 0),
            forward_z: n.i8("ForwardZ", 0),
            steering_angle: n.i8("SteeringAngle", 0),
            gas_pedal_power: n.i8("GasPedalPower", 0),
            brake_pedal_power: n.i8("BrakePedalPower", 0),
            handbrake_used: n.u8("HandbrakeUsed", 0),
            position: n.vec3("Position"),
        }
    }
}

impl WaypointRecordList {
    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let vft = r.u64()?;
        let block_map = r.ptr(BlockMap::read)?;
        let unknown_10h = r.u32()?;
        let unknown_14h = r.u32()?;
        let ptr = r.u64()?;
        let entries_count = r.u32()?;
        let unknown_24h = r.u32()?;
        let unknown_28h = r.u32()?;
        let unknown_2ch = r.u32()?;
        Ok(Self {
            vft,
            block_map,
            unknown_10h,
            unknown_14h,
            entries: read_array(r, ptr, entries_count as usize)?,
            entries_count,
            unknown_24h,
            unknown_28h,
            unknown_2ch,
        })
    }

    pub fn write(&self) -> Vec<u8> {
        let mut w = Rsc8Writer::new();
        let root = w.reserve();
        let block_map = self.block_map.clone().unwrap_or_default().write(&mut w);
        let entries = w.array(self.entries.as_deref());
        let mut b = BlockBuf::with_capacity(0x30);
        b.u64(self.vft);
        b.ptr(Some(block_map));
        b.u32(self.unknown_10h);
        b.u32(self.unknown_14h);
        b.ptr(entries);
        b.u32(self.entries_count);
        b.u32(self.unknown_24h);
        b.u32(self.unknown_28h);
        b.u32(self.unknown_2ch);
        w.set(root, b, false);
        w.build(VERSION)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.node_array("Entries", self.entries.as_deref(), |w, e| e.write_xml(w));
    }

    pub fn read_xml(node: XmlNode) -> Self {
        let entries = read_struct_array(node, "Entries", WaypointRecord::read_xml);
        let entries_count = entries.as_ref().map_or(0, |e| e.len() as u32);
        Self { entries, entries_count, ..Default::default() }
    }
}

impl WaypointRecord {
    fn write_xml(&self, w: &mut XmlWriter) {
        nonzero_vec3(w, "Position", self.position);
        nonzero(w, "Flags", self.flags, XmlWriter::u16);
        nonzero(w, "Heading", self.heading, XmlWriter::u8);
        nonzero(w, "MoveBlendRatio", self.move_blend_ratio, XmlWriter::u8);
        nonzero(w, "FreeSpaceOnLeft", self.free_space_on_left, XmlWriter::u8);
        nonzero(w, "FreeSpaceOnRight", self.free_space_on_right, XmlWriter::u8);
        nonzero(w, "Unused", self.unused, XmlWriter::u16);
    }

    fn read_xml(n: XmlNode) -> Self {
        Self {
            position: n.vec3("Position"),
            flags: n.u16("Flags", 0),
            heading: n.u8("Heading", 0),
            move_blend_ratio: n.u8("MoveBlendRatio", 0),
            free_space_on_left: n.u8("FreeSpaceOnLeft", 0),
            free_space_on_right: n.u8("FreeSpaceOnRight", 0),
            unused: n.u16("Unused", 0),
        }
    }
}

/// `WriteBasicType` for an integer field: written only when it isn't 0.
pub(crate) fn nonzero<'a, T: Default + PartialEq>(
    w: &mut XmlWriter<'a>,
    id: &str,
    v: T,
    f: fn(&mut XmlWriter<'a>, &str, T),
) {
    if v != T::default() {
        f(w, id, v);
    }
}

/// `WriteBasicType` for a `Vector3`: written unless it equals (0, 0, 0).
pub(crate) fn nonzero_vec3(w: &mut XmlWriter, id: &str, v: [f32; 3]) {
    if v != [0.0; 3] {
        w.vec3(id, v);
    }
}

/// `ReadStructArray`: the `Item` children of `id` (an empty array when there
/// are none, `None` when `id` is absent).
pub(crate) fn read_struct_array<T>(node: XmlNode, id: &str, f: impl Fn(XmlNode) -> T) -> Option<Vec<T>> {
    let c = node.child(id)?;
    Some(c.children("Item").map(f).collect())
}

pub fn read_yvr(file: &[u8]) -> Result<VehicleRecordList> {
    read_resource(file, |r, _| VehicleRecordList::read(r))
}

pub fn read_ywr(file: &[u8]) -> Result<WaypointRecordList> {
    read_resource(file, |r, _| WaypointRecordList::read(r))
}

pub fn yvr_to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let list = read_yvr(file)?;
    Ok(XmlWriter::document(cx.hashes, "RDR2VehicleRecords", None, |w| list.write_xml(w)))
}

pub fn yvr_from_xml(xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    let list = XmlDocument::new(xml).with_root(None, VehicleRecordList::read_xml)?;
    Ok(list.write())
}

pub fn ywr_to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let list = read_ywr(file)?;
    Ok(XmlWriter::document(cx.hashes, "RDR2WaypointRecords", None, |w| list.write_xml(w)))
}

pub fn ywr_from_xml(xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    let list = XmlDocument::new(xml).with_root(None, WaypointRecordList::read_xml)?;
    Ok(list.write())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::HashIndex;

    #[test]
    fn struct_sizes_match_the_game() {
        assert_eq!(size_of::<VehicleRecord>(), 32);
        assert_eq!(size_of::<WaypointRecord>(), 20);
    }

    #[test]
    fn records_round_trip() {
        let hashes = HashIndex::new();
        let cx = XmlContext { hashes: &hashes, folder: None };
        let list = WaypointRecordList {
            entries: Some(vec![
                WaypointRecord { position: [1.0, -2.5, 0.0], flags: 3, heading: 200, ..Default::default() },
                WaypointRecord::default(),
            ]),
            entries_count: 2,
            ..Default::default()
        };
        let xml = XmlWriter::document(&hashes, "RDR2WaypointRecords", None, |w| list.write_xml(w));
        assert!(xml.contains("  <Item />\n"), "{xml}");
        let back = read_ywr(&ywr_from_xml(&xml, &cx).unwrap()).unwrap();
        assert_eq!(back.entries, list.entries);
        assert_eq!(back.entries_count, 2);

        let v = VehicleRecordList {
            entries: Some(vec![VehicleRecord { time: 5, right_x: -127, handbrake_used: 1, ..Default::default() }]),
            entries_count: 1,
            entries_capacity: 1,
            ..Default::default()
        };
        let xml = XmlWriter::document(&hashes, "RDR2VehicleRecords", None, |w| v.write_xml(w));
        assert!(xml.contains("<RightX value=\"-127\" />"), "{xml}");
        let back = read_yvr(&yvr_from_xml(&xml, &cx).unwrap()).unwrap();
        assert_eq!(back.entries, v.entries);
        assert_eq!((back.entries_count, back.entries_capacity), (1, 1));
    }
}
