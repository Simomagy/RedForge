//! Resource metadata: ymap, ytyp and the RSC8 ymt files (`Rsc8Meta.cs`).
//!
//! A meta file is a list of data blocks, each holding structs (or plain arrays)
//! of one type, plus the schema of those structs: the classes and enums the file
//! uses. Reading follows the pointers from the root block into a tree of
//! [`DataBag`]s, which write themselves to XML.
//!
//! Writing (from XML, or from edited bags) needs the classes of every type the
//! data uses, so it goes through the schema of all RDR2 classes and enums in
//! `data/rdr2/schemas.txt` ([`rsc_schema`]); the file's own schema is then made
//! from the classes actually written, as the C# does.
//!
//! ```ignore
//! let mut meta = Meta::read(&ymap_file)?;
//! let root = meta.root.as_mut().unwrap();           // CMapData
//! for e in root.bags_mut("entities").into_iter().flatten().flatten() {
//!     e.set_f32("lodDist", 200.0);
//! }
//! let file = meta.save()?;                           // a loose RSC8 file
//! ```
//!
//! C# behaviour kept on purpose: binary map (lookup) entry classes get their item
//! type from the map field while reading; strings are written without a NUL;
//! lookup entry classes are named 0x100, 0x101... with the sizes and keys the C#
//! hard-codes for RDR2's maps; a file whose first enum isn't in the schema loses
//! all its enums (the C#'s `WriteBlocks` stops at a null first item).
//!
//! Known differences from the C#, none seen in game files: reads past the end of
//! a struct or block give zeros (the C# reads the neighbouring bytes, or throws);
//! arrays read twice from one position with different counts aren't shared as
//! the C#'s block pool would; writing bags read from a binary file (the editor's
//! round trip) has no C# counterpart to compare with, so it is only checked by
//! this crate's tests.

mod reader;
mod schemas;
mod writer;

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};

pub use schemas::{PsoClassDef, PsoEntryDef, PsoEnumDef, RscClassDef, RscEnumDef, SchemaFile, rsc_schema, schema_file};

use super::XmlContext;
use super::databag::{
    ClassInfo, DataBag, EnumInfo, EnumValue, Schema, SchemaClass, SchemaEnum, SchemaField, ValueType,
};
use crate::hash::HashIndex;
use crate::rsc8::{BlockBuf, BlockId, BlockMap, Rsc8Reader, Rsc8Writer, VIRTUAL_BASE, read_loose};
use crate::{Error, Result, bail};

/// `Rsc8MetaDataType`: the type of a class entry, as stored in the file. Kept as
/// the raw byte so unknown types survive.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Pod, Zeroable)]
pub struct RscDataType(pub u8);

impl RscDataType {
    pub const BOOLEAN: Self = Self(0x01);
    pub const SIGNED_BYTE: Self = Self(0x10);
    pub const UNSIGNED_BYTE: Self = Self(0x11);
    pub const SIGNED_SHORT: Self = Self(0x12);
    pub const UNSIGNED_SHORT: Self = Self(0x13);
    pub const SIGNED_INT: Self = Self(0x14);
    pub const UNSIGNED_INT: Self = Self(0x15);
    pub const FLOAT: Self = Self(0x21);
    pub const FLOAT_XYZ: Self = Self(0x33);
    pub const FLOAT_XYZW: Self = Self(0x34);
    pub const BYTE_ENUM: Self = Self(0x60);
    pub const INT_ENUM: Self = Self(0x62);
    pub const SHORT_FLAGS: Self = Self(0x64);
    pub const INT_FLAGS1: Self = Self(0x63);
    pub const INT_FLAGS2: Self = Self(0x65);
    pub const HASH: Self = Self(0x4A);
    pub const ARRAY: Self = Self(0x52);
    pub const ARRAY_OF_CHARS: Self = Self(0x40);
    pub const ARRAY_OF_BYTES: Self = Self(0x50);
    pub const DATA_BLOCK_POINTER: Self = Self(0x59);
    pub const CHAR_POINTER: Self = Self(0x44);
    pub const STRUCTURE_POINTER: Self = Self(0x07);
    pub const STRUCTURE: Self = Self(0x05);
    pub const GUID: Self = Self(0x08);
    pub const UNSIGNED_LONG: Self = Self(0x17);
    pub const VEC3V: Self = Self(0x31);
    pub const FIXED_ARRAY: Self = Self(0x53);
    pub const FLOAT16: Self = Self(0x20);
    pub const MAT34V: Self = Self(0x36);
    pub const BINARY_MAP: Self = Self(0x71);

    const NAMES: [(Self, &'static str); 30] = [
        (Self::BOOLEAN, "Boolean"),
        (Self::SIGNED_BYTE, "SignedByte"),
        (Self::UNSIGNED_BYTE, "UnsignedByte"),
        (Self::SIGNED_SHORT, "SignedShort"),
        (Self::UNSIGNED_SHORT, "UnsignedShort"),
        (Self::SIGNED_INT, "SignedInt"),
        (Self::UNSIGNED_INT, "UnsignedInt"),
        (Self::FLOAT, "Float"),
        (Self::FLOAT_XYZ, "Float_XYZ"),
        (Self::FLOAT_XYZW, "Float_XYZW"),
        (Self::BYTE_ENUM, "ByteEnum"),
        (Self::INT_ENUM, "IntEnum"),
        (Self::SHORT_FLAGS, "ShortFlags"),
        (Self::INT_FLAGS1, "IntFlags1"),
        (Self::INT_FLAGS2, "IntFlags2"),
        (Self::HASH, "Hash"),
        (Self::ARRAY, "Array"),
        (Self::ARRAY_OF_CHARS, "ArrayOfChars"),
        (Self::ARRAY_OF_BYTES, "ArrayOfBytes"),
        (Self::DATA_BLOCK_POINTER, "DataBlockPointer"),
        (Self::CHAR_POINTER, "CharPointer"),
        (Self::STRUCTURE_POINTER, "StructurePointer"),
        (Self::STRUCTURE, "Structure"),
        (Self::GUID, "GUID"),
        (Self::UNSIGNED_LONG, "UnsignedLong"),
        (Self::VEC3V, "Vec3V"),
        (Self::FIXED_ARRAY, "FixedArray"),
        (Self::FLOAT16, "Float16"),
        (Self::MAT34V, "Mat34V"),
        (Self::BINARY_MAP, "BinaryMap"),
    ];

    /// The C# enum name.
    pub fn name(self) -> Option<&'static str> {
        Self::NAMES.iter().find(|(t, _)| *t == self).map(|(_, n)| *n)
    }

    /// `Enum.Parse<Rsc8MetaDataType>`: a name, or a number.
    pub fn parse(s: &str) -> Option<Self> {
        if let Some((t, _)) = Self::NAMES.iter().find(|(_, n)| *n == s) {
            return Some(*t);
        }
        s.trim().parse::<u8>().ok().map(Self)
    }

    /// `Rsc8Meta.GetEngineType`: how the data bag stores a value of this type.
    pub fn engine_type(self) -> ValueType {
        match self {
            Self::BOOLEAN => ValueType::Boolean,
            Self::SIGNED_BYTE => ValueType::Int8,
            Self::UNSIGNED_BYTE => ValueType::UInt8,
            Self::SIGNED_SHORT => ValueType::Int16,
            Self::UNSIGNED_SHORT => ValueType::UInt16,
            Self::SIGNED_INT => ValueType::Int32,
            Self::UNSIGNED_INT => ValueType::UInt32,
            Self::FLOAT => ValueType::Float,
            Self::FLOAT_XYZ => ValueType::Float3,
            Self::FLOAT_XYZW => ValueType::Float4,
            Self::BYTE_ENUM => ValueType::Enum8,
            Self::INT_ENUM => ValueType::Enum32,
            Self::SHORT_FLAGS => ValueType::Enum16,
            Self::INT_FLAGS1 | Self::INT_FLAGS2 => ValueType::Flags32,
            Self::HASH => ValueType::Hash,
            Self::ARRAY | Self::ARRAY_OF_CHARS | Self::ARRAY_OF_BYTES | Self::FIXED_ARRAY => ValueType::Array,
            Self::DATA_BLOCK_POINTER | Self::STRUCTURE_POINTER => ValueType::Object,
            Self::CHAR_POINTER => ValueType::String,
            Self::STRUCTURE => ValueType::Struct,
            Self::BINARY_MAP => ValueType::Lookup,
            Self::UNSIGNED_LONG | Self::GUID => ValueType::UInt64,
            // Half floats are kept as their bits.
            Self::FLOAT16 => ValueType::UInt16,
            Self::VEC3V => ValueType::Float4,
            Self::MAT34V => ValueType::Float4x4,
            _ => ValueType::None,
        }
    }
}

/// `Rsc8MetaClassEntry`: one entry of a class in the file (a field, or the
/// item type of an array field).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct RscClassEntry {
    pub name: u32,
    pub offset: i32,
    pub data_type: RscDataType,
    pub unknown_9h: u8,
    pub reference_type_index: u16,
    pub type_name: u32,
}

/// The resource meta details of a class (`Rsc8MetaClass`), besides its name,
/// size and fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RscClassInfo {
    pub structure_key: u32,
    pub unknown_8h: u32,
    pub unknown_ch: u32,
    pub unknown_1ch: u16,
    /// `None` when the file has no entry list.
    pub entries: Option<Vec<RscClassEntry>>,
}

/// The resource meta details of an enum (`Rsc8MetaEnum`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RscEnumInfo {
    pub key: u32,
    pub unknown_14h: u32,
    /// Each value's `Unknown_0Ch`, in the order of the values.
    pub value_unknowns: Vec<u32>,
}

/// `Rsc8MetaData`: a block of data of one type (a class name, or a plain type
/// such as `UnsignedByte` for strings and raw arrays).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RscDataBlock {
    pub data_type: u32,
    /// `None` when the block is empty.
    pub data: Option<Vec<u8>>,
}

/// `Rsc8MetaUnkData1`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnkData1 {
    pub unknown_0h: u32,
    pub unknown_4h: u32,
    pub data: Option<Vec<u8>>,
}

/// `Rsc8MetaUnkData2`: 16-byte records.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnkData2 {
    pub data: Vec<u8>,
}

/// `Rsc8MetaArray`: a pointer into a block, with a count.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct MetaArray {
    pub pointer: u64,
    pub count: u16,
    pub capacity: u16,
    pub unknown: u32,
}

/// `Rsc8MetaLookup`: a binary map's entries.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct MetaLookup {
    pub unknown_0: u64,
    pub pointer: u64,
    pub count: u16,
    pub capacity: u16,
    pub unknown_1: u32,
}

/// `Rsc8MetaPointer`: the block id (1-based) in the low 16 bits, the offset in
/// the block in bits 20 to 39.
pub mod pointer {
    pub fn block_id(p: u64) -> i32 {
        (p & 0xFFFF) as i32
    }
    pub fn data_offset(p: u64) -> usize {
        ((p >> 20) & 0xFFFFF) as usize
    }
    pub fn with_data_offset(p: u64, v: usize) -> u64 {
        (p & 0xFFFF_FF00_000F_FFFF).wrapping_add(((v as u64) & 0xFFFFF) << 20)
    }
    /// A pointer to `offset` in the block at `index` (0-based).
    pub fn new(index: usize, offset: usize) -> u64 {
        ((index as u64 + 1) & 0xFFFF) | (((offset as u64) & 0xFFFFF) << 20)
    }
}

/// `BuildFieldsFromEntries`: a class's fields from its entries (entries named
/// 0x100 describe array items and aren't fields).
pub fn fields_from_entries(class_name: u32, entries: &[RscClassEntry]) -> Result<Vec<SchemaField>> {
    let ismapinfo = class_name == 0x100 || class_name == 0x101;
    let get = |i: usize| {
        entries
            .get(i)
            .copied()
            .ok_or_else(|| Error::Format(format!("class 0x{class_name:08X}: entry index {i} is out of range")))
    };
    let mut fields = Vec::with_capacity(entries.len());
    for e in entries.iter().filter(|e| e.name != 0x100) {
        let mut f = SchemaField {
            offset: e.offset,
            name: e.name,
            type_name: e.type_name,
            data_type: e.data_type.engine_type(),
            ..Default::default()
        };
        match e.data_type {
            RscDataType::ARRAY => {
                if ismapinfo {
                    // In map info classes the reference indexes refer to the parent class.
                    f.array_type = ValueType::Struct;
                } else {
                    let a = get(e.reference_type_index as usize)?;
                    f.array_type = a.data_type.engine_type();
                    if matches!(a.data_type, RscDataType::STRUCTURE | RscDataType::STRUCTURE_POINTER) {
                        f.type_name = a.type_name;
                    }
                }
            }
            RscDataType::ARRAY_OF_BYTES => {
                f.array_size = e.type_name;
                f.array_type = ValueType::UInt8;
                f.type_name = 0;
                if !ismapinfo {
                    f.array_type = get(e.reference_type_index as usize)?.data_type.engine_type();
                }
            }
            RscDataType::ARRAY_OF_CHARS => {
                f.array_size = e.type_name;
                f.array_type = ValueType::Char;
                f.type_name = 0;
            }
            RscDataType::FIXED_ARRAY => {
                f.array_size = e.type_name;
                let a = get(e.reference_type_index as usize)?;
                f.array_type = a.data_type.engine_type();
                if matches!(a.data_type, RscDataType::STRUCTURE | RscDataType::STRUCTURE_POINTER) {
                    f.type_name = a.type_name;
                }
            }
            RscDataType::BINARY_MAP => {
                let k = get((e.type_name & 0xFFFF) as usize)?;
                let i = get(((e.type_name >> 16) & 0xFFFF) as usize)?;
                f.array_type = k.data_type.engine_type();
                // e.g. 0x100: still used as a block type name.
                f.type_name = i.type_name;
                if i.data_type == RscDataType::ARRAY {
                    let ia = get(i.reference_type_index as usize)?;
                    match ia.data_type {
                        RscDataType::STRUCTURE => f.type_name = ia.type_name,
                        RscDataType::UNSIGNED_LONG => f.type_name = ia.data_type.0 as u32,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        fields.push(f);
    }
    Ok(fields)
}

/// `new Rsc8MetaClass(name, key, unknown, length, entries)`.
pub fn rsc_class(name: u32, key: u32, unknown_8h: u32, size: u32, entries: Vec<RscClassEntry>) -> Result<SchemaClass> {
    let fields = fields_from_entries(name, &entries)?;
    let info = RscClassInfo { structure_key: key, unknown_8h, entries: Some(entries), ..Default::default() };
    Ok(SchemaClass::new(name, size, Some(fields), ClassInfo::Rsc(info)))
}

/// `new Rsc8MetaEnum(name, key, values)`.
pub fn rsc_enum(name: u32, key: u32, values: Vec<EnumValue>) -> SchemaEnum {
    let info = RscEnumInfo { key, unknown_14h: 0, value_unknowns: vec![0; values.len()] };
    SchemaEnum { name, values: Some(values), info: EnumInfo::Rsc(info) }
}

/// A resource meta file (`Rsc8Meta`).
#[derive(Clone, Debug)]
pub struct Meta {
    pub vft: u64,
    pub block_map: Option<BlockMap>,
    /// "PRD0".
    pub unknown_10h: u32,
    /// The platform?
    pub unknown_14h: u16,
    pub unknown_16h: u8,
    pub unknown_17h: u8,
    pub unknown_18h: u64,
    pub root_block_id: u32,
    pub unknown_24h: u32,
    /// The classes in the file (`ClassRefs`), as read.
    pub classes: Option<Vec<Arc<SchemaClass>>>,
    /// The enums in the file (`EnumRefs`). The C# writer can add a null one.
    pub enums: Option<Vec<Option<Arc<SchemaEnum>>>>,
    pub blocks: Option<Vec<RscDataBlock>>,
    pub unk_data1: Option<UnkData1>,
    pub unknown_4eh: u16,
    pub unk_data2: Option<UnkData2>,
    pub unknown_58h: u64,
    /// The schema the bags' classes come from: the file's own when read, the
    /// RDR2 schema ([`rsc_schema`]) when made from XML. Enum names come from it.
    pub schema: Schema,
    /// The data. `None` when the root block couldn't be read.
    pub root: Option<DataBag>,
}

impl Meta {
    /// A new file for this data (`new Rsc8Meta(xml)`), with the RDR2 schema.
    pub fn new(root: Option<DataBag>) -> Self {
        Self {
            vft: 0,
            block_map: Some(BlockMap { virtual_pages: 1, ..Default::default() }),
            unknown_10h: 0x5052_4430,
            unknown_14h: 0x0079,
            unknown_16h: 0,
            unknown_17h: 0,
            unknown_18h: 0,
            root_block_id: 0,
            unknown_24h: 0,
            classes: None,
            enums: None,
            blocks: None,
            unk_data1: Some(UnkData1::default()),
            unknown_4eh: 0,
            unk_data2: None,
            unknown_58h: 0,
            schema: rsc_schema().clone(),
            root,
        }
    }

    /// A new, empty instance of an RDR2 class (`CMapData`, `CEntityDef`...).
    pub fn create(class: &str) -> Option<DataBag> {
        rsc_schema().create(class)
    }

    /// Reads a loose resource (with its RSC8 header).
    pub fn read(file: &[u8]) -> Result<Self> {
        let (header, data) = read_loose(file)?;
        let mut r = Rsc8Reader::new(&data, header.virtual_size());
        let mut m = Self::read_header(&mut r, &data)?;

        let mut schema = Schema::new(m.classes.iter().flatten().cloned(), m.enums.iter().flatten().flatten().cloned());
        let blocks = m.blocks.as_deref().unwrap_or(&[]);
        let root =
            reader::MetaReader::new(blocks, &mut schema).read_object_at((m.root_block_id & 0xFFFF) as u64, None)?;
        m.schema = schema;
        m.root = root;
        Ok(m)
    }

    fn read_header(r: &mut Rsc8Reader, data: &[u8]) -> Result<Self> {
        r.pos = VIRTUAL_BASE;
        let vft = r.u64()?;
        let bm = r.u64()?;
        let block_map = r.follow(bm, BlockMap::read)?;
        let unknown_10h = r.u32()?;
        let unknown_14h = r.u16()?;
        let unknown_16h = r.u8()?;
        let unknown_17h = r.u8()?;
        let unknown_18h = r.u64()?;
        let root_block_id = r.u32()?;
        let unknown_24h = r.u32()?;
        let classes_ptr = r.u64()?;
        let enums_ptr = r.u64()?;
        let blocks_ptr = r.u64()?;
        let unk1 = r.u64()?;
        let unk_data1 = r.follow(unk1, |r| {
            let unknown_0h = r.u32()?;
            let unknown_4h = r.u32()?;
            let len = r.u32()? as usize;
            let data = if len > 0 { Some(r.bytes(len)?.to_vec()) } else { None };
            Ok(UnkData1 { unknown_0h, unknown_4h, data })
        })?;
        let class_count = r.u16()?;
        let enum_count = r.u16()?;
        let block_count = r.u16()?;
        let unknown_4eh = r.u16()?;
        let unk2 = r.u64()?;
        let unk_data2 = r.follow(unk2, |r| {
            let n = r.u32()? as usize;
            Ok(UnkData2 { data: r.bytes(n * 16)?.to_vec() })
        })?;
        let unknown_58h = r.u64()?;

        let classes = r.items_at(classes_ptr, class_count as usize, |r| Self::read_class(r, data))?;
        let enums = r.items_at(enums_ptr, enum_count as usize, |r| Self::read_enum(r).map(|e| Some(Arc::new(e))))?;
        let blocks = r.items_at(blocks_ptr, block_count as usize, |r| {
            let data_type = r.u32()?;
            let len = r.u32()? as usize;
            let p = r.u64()?;
            Ok(RscDataBlock { data_type, data: read_array_at(r, data, p, len)? })
        })?;
        Ok(Self {
            vft,
            block_map,
            unknown_10h,
            unknown_14h,
            unknown_16h,
            unknown_17h,
            unknown_18h,
            root_block_id,
            unknown_24h,
            classes,
            enums,
            blocks,
            unk_data1,
            unknown_4eh,
            unk_data2,
            unknown_58h,
            schema: Schema::default(),
            root: None,
        })
    }

    fn read_class(r: &mut Rsc8Reader, data: &[u8]) -> Result<Arc<SchemaClass>> {
        let name = r.u32()?;
        let structure_key = r.u32()?;
        let unknown_8h = r.u32()?;
        let unknown_ch = r.u32()?;
        let p = r.u64()?;
        let data_size = r.u32()?;
        let unknown_1ch = r.u16()?;
        let count = r.u16()? as usize;
        let entries = read_array_at(r, data, p, count * size_of::<RscClassEntry>())?
            .map(|b| bytemuck::pod_collect_to_vec::<u8, RscClassEntry>(&b));
        let fields = match &entries {
            Some(e) => Some(fields_from_entries(name, e)?),
            None => None,
        };
        let info = RscClassInfo { structure_key, unknown_8h, unknown_ch, unknown_1ch, entries };
        Ok(Arc::new(SchemaClass::new(name, data_size, fields, ClassInfo::Rsc(info))))
    }

    fn read_enum(r: &mut Rsc8Reader) -> Result<SchemaEnum> {
        let name = r.u32()?;
        let key = r.u32()?;
        let p = r.u64()?;
        let count = r.u32()? as usize;
        let unknown_14h = r.u32()?;
        let mut value_unknowns = Vec::new();
        let values = r.items_at(p, count, |r| {
            let value = r.i64()?;
            let name = r.u32()?;
            value_unknowns.push(r.u32()?);
            Ok(EnumValue { name, value })
        })?;
        Ok(SchemaEnum { name, values, info: EnumInfo::Rsc(RscEnumInfo { key, unknown_14h, value_unknowns }) })
    }

    /// The data as XML (`Bag.ToXml()`).
    pub fn to_xml(&self, hashes: &HashIndex) -> Option<String> {
        self.root.as_ref().map(|b| b.to_xml(hashes, Some(&self.schema)))
    }

    /// Reads XML with the RDR2 schema (`new Rsc8Meta(xml)`).
    pub fn from_xml(xml: &str, hashes: &HashIndex) -> Result<Self> {
        let root = DataBag::from_xml(xml, Some(rsc_schema()), hashes)?;
        Ok(Self::new(root))
    }

    /// Writes the file (`Save`): the data is laid out in blocks again, and the
    /// file's classes and enums are those it uses. A loose RSC8 file, version 3.
    pub fn save(&self) -> Result<Vec<u8>> {
        let (classes, enums, blocks, root_block_id) = match &self.root {
            Some(root) => {
                let mut w = writer::MetaWriter::new(&self.schema);
                w.write_object(root)?;
                let (c, e, b) = w.finish();
                (c, e, b, 1)
            }
            None => (self.classes.clone(), self.enums.clone(), self.blocks.clone(), self.root_block_id),
        };

        let mut w = Rsc8Writer::new();
        let root = w.reserve();
        let block_map = self.block_map.as_ref().map(|bm| bm.write(&mut w));
        let classes_id = classes.as_ref().map(|c| write_classes(&mut w, c));
        let enums_id = enums.as_ref().and_then(|e| write_enums(&mut w, e));
        let blocks_id = blocks.as_ref().map(|b| write_blocks(&mut w, b));
        let unk1_id = self.unk_data1.as_ref().map(|u| {
            let mut b = BlockBuf::default();
            b.u32(u.unknown_0h);
            b.u32(u.unknown_4h);
            let d = u.data.as_deref().unwrap_or(&[]);
            b.u32(d.len() as u32);
            b.bytes(d);
            w.add(b, false)
        });
        let unk2_id = self.unk_data2.as_ref().map(|u| {
            let mut b = BlockBuf::default();
            b.u32((u.data.len() / 16) as u32);
            b.bytes(&u.data);
            w.add(b, false)
        });

        let mut b = BlockBuf::with_capacity(0x60);
        b.u64(self.vft);
        b.ptr(block_map);
        b.u32(self.unknown_10h);
        b.u16(self.unknown_14h);
        b.u8(self.unknown_16h);
        b.u8(self.unknown_17h);
        b.u64(self.unknown_18h);
        b.u32(root_block_id);
        b.u32(self.unknown_24h);
        b.ptr(classes_id);
        b.ptr(enums_id);
        b.ptr(blocks_id);
        b.ptr(unk1_id);
        b.u16(classes.as_ref().map_or(0, |c| c.len()) as u16);
        b.u16(enums.as_ref().map_or(0, |e| e.len()) as u16);
        b.u16(blocks.as_ref().map_or(0, |b| b.len()) as u16);
        b.u16(self.unknown_4eh);
        b.ptr(unk2_id);
        b.u64(self.unknown_58h);
        w.set(root, b, false);
        Ok(w.build(3))
    }
}

/// `BufferUtil.ReadArray<byte>`: `len` bytes at `p`, zero-filled past the end
/// of the data; `None` for a null pointer or no bytes.
fn read_array_at(r: &Rsc8Reader, data: &[u8], p: u64, len: usize) -> Result<Option<Vec<u8>>> {
    if p == 0 || len == 0 {
        return Ok(None);
    }
    let off = r.offset_of(p)?;
    if off > data.len() {
        bail!("array at 0x{p:X} is past the end of the data");
    }
    let mut v = vec![0u8; len];
    let n = len.min(data.len() - off);
    v[..n].copy_from_slice(&data[off..off + n]);
    Ok(Some(v))
}

fn write_classes(w: &mut Rsc8Writer, classes: &[Arc<SchemaClass>]) -> BlockId {
    let mut b = BlockBuf::with_capacity(classes.len() * 32);
    for c in classes {
        let info = c.rsc().cloned().unwrap_or_default();
        let entries = info.entries.as_deref();
        let entries_id = entries.map(|e| w.array(Some(e)).unwrap());
        b.u32(c.name);
        b.u32(info.structure_key);
        b.u32(info.unknown_8h);
        b.u32(info.unknown_ch);
        b.ptr(entries_id);
        b.u32(c.data_size);
        b.u16(info.unknown_1ch);
        b.u16(entries.map_or(0, |e| e.len()) as u16);
    }
    w.add(b, false)
}

/// The enums, or no block at all when the first is null (the C#'s `WriteBlocks`
/// gives up when the first item is).
fn write_enums(w: &mut Rsc8Writer, enums: &[Option<Arc<SchemaEnum>>]) -> Option<BlockId> {
    enums.first()?.as_ref()?;
    let mut b = BlockBuf::with_capacity(enums.len() * 24);
    for e in enums {
        let Some(e) = e else {
            b.zeros(24);
            continue;
        };
        let (key, unknown_14h, unknowns) = match &e.info {
            EnumInfo::Rsc(i) => (i.key, i.unknown_14h, i.value_unknowns.as_slice()),
            EnumInfo::Plain => (0, 0, &[][..]),
        };
        let values = e.values.as_deref().unwrap_or(&[]);
        let values_id = (!values.is_empty()).then(|| {
            let mut vb = BlockBuf::with_capacity(values.len() * 16);
            for (i, v) in values.iter().enumerate() {
                vb.i64(v.value);
                vb.u32(v.name);
                vb.u32(unknowns.get(i).copied().unwrap_or(0));
            }
            w.add(vb, false)
        });
        b.u32(e.name);
        b.u32(key);
        b.ptr(values_id);
        b.u32(e.values.as_ref().map_or(0, |v| v.len()) as u32);
        b.u32(unknown_14h);
    }
    Some(w.add(b, false))
}

fn write_blocks(w: &mut Rsc8Writer, blocks: &[RscDataBlock]) -> BlockId {
    let mut b = BlockBuf::with_capacity(blocks.len() * 16);
    for d in blocks {
        let id = d.data.as_deref().and_then(|d| w.array(Some(d)));
        b.u32(d.data_type);
        b.u32(d.data.as_ref().map_or(0, |d| d.len()) as u32);
        b.ptr(id);
    }
    w.add(b, false)
}

/// A meta file to XML. Like the C#, a file whose root can't be read has no XML.
pub fn to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let meta = Meta::read(file)?;
    meta.to_xml(cx.hashes).ok_or_else(|| Error::Format("the root block couldn't be read: no XML".into()))
}

pub fn from_xml(xml: &str, cx: &XmlContext) -> Result<Vec<u8>> {
    Meta::from_xml(xml, cx.hashes)?.save()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_has_map_classes() {
        let s = rsc_schema();
        let map = s.class("CMapData").expect("CMapData");
        assert!(map.field("entities").is_some());
        let ent = s.class("CEntityDef").expect("CEntityDef");
        assert_eq!(ent.field("position").map(|f| f.data_type), Some(ValueType::Float3));
        // Enum fields find their enum in the schema.
        let lod = ent.field("lodLevel").expect("lodLevel");
        assert!(lod.data_type.is_enum());
        assert!(s.enumeration(lod.type_name).is_some_and(|e| e.values.as_ref().is_some_and(|v| !v.is_empty())));
    }

    #[test]
    fn new_ymap_round_trips() {
        let hashes = HashIndex::new();
        for n in ["CMapData", "name", "CEntityDef", "archetypeName", "entities", "position", "lodDist"] {
            hashes.ensure(n);
        }
        let mut map = Meta::create("CMapData").unwrap();
        map.set_hash("name", hashes.ensure("my_map"));
        let mut e = Meta::create("CEntityDef").unwrap();
        e.set_hash("archetypeName", hashes.ensure("p_bench09x"));
        e.set_vec3("position", [1.0, 2.0, 3.0]);
        e.set_f32("lodDist", 100.0);
        map.set_object("entities", Some(super::super::databag::Value::Bags(vec![Some(e)])));
        let file = Meta::new(Some(map)).save().unwrap();

        let back = Meta::read(&file).unwrap();
        let root = back.root.as_ref().unwrap();
        assert_eq!(hashes.xml_name(root.hash("name")), "my_map");
        let e = root.bags("entities").unwrap()[0].as_ref().unwrap();
        assert_eq!(e.vec3("position"), [1.0, 2.0, 3.0]);
        assert_eq!(e.f32("lodDist"), 100.0);
        assert_eq!(hashes.xml_name(e.hash("archetypeName")), "p_bench09x");

        // XML → file → XML gives the same XML.
        let xml = back.to_xml(&hashes).unwrap();
        let again = Meta::read(&Meta::from_xml(&xml, &hashes).unwrap().save().unwrap()).unwrap();
        assert_eq!(again.to_xml(&hashes).unwrap(), xml);
    }
}
