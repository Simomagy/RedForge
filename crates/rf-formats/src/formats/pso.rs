//! PSO metadata ("PSIN" files: most of the game's non-resource `.ymt`, and `.ymf`,
//! `.ych`, `.yas`), from `PsoFile.cs`.
//!
//! A PSO file is a list of big-endian sections: the data (`PSIN`, the structs in
//! blocks), the data map (`PMAP`, where each block is and what it holds), the
//! schema (`PSCH`, the classes and enums used), and some optional ones (strings,
//! signature, checksum). Pointers are a block id and an offset in that block.
//!
//! Reading makes a [`DataBag`] tree with classes from the file's own schema and
//! writes it with `DataBag2.ToXml`. XML is read back with `DataBag2.FromXml`
//! against the schemas of every PSO class in the game ([`pso_schema`], from
//! `schemas.txt`), and written with the C#'s `PsoWriter`, which rebuilds the
//! blocks, the map and a schema of the classes used.
//!
//! C# behaviours kept on purpose:
//! - all the bags read from a file share one zeroed buffer (`new byte[length]`)
//!   that only the values read are copied into, so a value written past the end
//!   of its struct (a 64-byte `Mat34V`) shows in the struct after it, and padding
//!   and unread fields are 0;
//! - arrays of structs behind a pointer are read up to their capacity, not count;
//! - reading a map (lookup) resolves the item type of its key/item class in place,
//!   which changes that class for everything read afterwards;
//! - the strings of the `STRF`/`STRS` sections are added to the hash index;
//! - writing makes lookup classes named 0x100, 0x101... and shares them between
//!   maps whose key and item types and reference keys match (not their sizes);
//! - a map's string keys are lost on writing (`GetLookupValue` doesn't copy them);
//! - empty strings are null pointers, but the string block is still made.
//!
//! Writing uses the classes of `schemas.txt`, which can have more or fewer
//! fields than the file's own (other game versions), so a file written from XML
//! can gain fields (with default values) or lose some, as with the C#.
//!
//! Known differences and issues:
//! - the C# fails to write most PSO files: when the data doesn't end on a 16-byte
//!   boundary, padding the data section throws `EndOfStreamException`. Here the
//!   padding is written; the C# reads those files back to the same XML;
//! - files whose root class isn't in `schemas.txt` (the parsed data `.ymt`,
//!   `rage__sysParsedDataFile__Data`) can't be written back: the C# writes an
//!   empty file, this returns an error;
//! - invalid UTF-8 in strings is replaced like `String::from_utf8_lossy` does,
//!   which may differ from .NET in rare cases.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use super::XmlContext;
use super::databag::{
    ClassInfo, DataBag, EnumInfo, EnumValue, ITEM_HASH, KEY_HASH, Schema, SchemaClass, SchemaEnum, SchemaField, Value,
    ValueType,
};
use super::meta::schema_file;
use crate::hash::HashIndex;
use crate::{Error, Result, bail};

/// The section idents (`PsoSection`).
pub mod section {
    pub const PSIN: u32 = 0x5053_494E;
    pub const PMAP: u32 = 0x504D_4150;
    pub const PSCH: u32 = 0x5053_4348;
    pub const PSIG: u32 = 0x5053_4947;
    pub const STRF: u32 = 0x5354_5246;
    pub const STRS: u32 = 0x5354_5253;
    pub const STRE: u32 = 0x5354_5245;
    pub const CHKS: u32 = 0x4348_4B53;
}

/// `PsoDataType`: the type of a class entry.
pub mod data_type {
    pub const BOOL: u8 = 0x00;
    pub const SBYTE: u8 = 0x01;
    pub const UBYTE: u8 = 0x02;
    pub const SSHORT: u8 = 0x03;
    pub const USHORT: u8 = 0x04;
    pub const SINT: u8 = 0x05;
    pub const UINT: u8 = 0x06;
    pub const FLOAT: u8 = 0x07;
    pub const FLOAT2: u8 = 0x08;
    pub const FLOAT3: u8 = 0x09;
    pub const FLOAT4: u8 = 0x0a;
    pub const STRING: u8 = 0x0b;
    pub const STRUCTURE: u8 = 0x0c;
    pub const ARRAY: u8 = 0x0d;
    pub const ENUM: u8 = 0x0e;
    pub const FLAGS: u8 = 0x0f;
    pub const MAP: u8 = 0x10;
    pub const VEC2V: u8 = 0x13;
    pub const FLOAT3A: u8 = 0x14;
    pub const FLOAT4A: u8 = 0x15;
    pub const MAT34V: u8 = 0x17;
    pub const HFLOAT: u8 = 0x1e;
    pub const LONG: u8 = 0x1f;
    pub const ULONG: u8 = 0x20;
    pub const QUATV: u8 = 0x24;

    /// `Enum.Parse<PsoDataType>`, for `schemas.txt`.
    pub fn parse(s: &str) -> Option<u8> {
        Some(match s {
            "Bool" => BOOL,
            "SByte" => SBYTE,
            "UByte" => UBYTE,
            "SShort" => SSHORT,
            "UShort" => USHORT,
            "SInt" => SINT,
            "UInt" => UINT,
            "Float" => FLOAT,
            "Float2" => FLOAT2,
            "Float3" => FLOAT3,
            "Float4" => FLOAT4,
            "String" => STRING,
            "Structure" => STRUCTURE,
            "Array" => ARRAY,
            "Enum" => ENUM,
            "Flags" => FLAGS,
            "Map" => MAP,
            "Float3a" => FLOAT3A,
            "Float4a" => FLOAT4A,
            "HFloat" => HFLOAT,
            "ULong" => ULONG,
            "Vec2V" => VEC2V,
            "Mat34V" => MAT34V,
            "Long" => LONG,
            "QuatV" => QUATV,
            _ => return s.parse().ok(),
        })
    }
}

use data_type as dt;

/// `PsoStructureEntryInfo`: one entry of a class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PsoEntry {
    pub name: u32,
    pub data_type: u8,
    pub unk_5h: u8,
    pub offset: u16,
    pub reference_key: u32,
}

/// `PsoStructureInfo`: a class.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PsoStruct {
    /// The name in the schema index (`IndexInfo.Name`).
    pub name: u32,
    pub ty: u8,
    pub unk: u8,
    pub data_size: u32,
    pub unk_ch: u32,
    pub entries: Vec<PsoEntry>,
}

/// `PsoEnumEntryInfo`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PsoEnumEntry {
    pub name: u32,
    pub unk1: u32,
    pub value: i64,
}

/// `PsoEnumInfo`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PsoEnum {
    pub name: u32,
    pub ty: u8,
    pub entries: Vec<PsoEnumEntry>,
}

/// An element of the schema section.
#[derive(Clone, Debug, PartialEq)]
pub enum PsoElement {
    Struct(PsoStruct),
    Enum(PsoEnum),
}

/// `PsoDataMappingEntry`: one block of the data section.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PsoMapEntry {
    pub name_hash: u32,
    pub offset: i32,
    pub unknown_8h: i32,
    pub length: i32,
}

/// `PsoDataMapSection`.
#[derive(Clone, Debug, PartialEq)]
pub struct PsoDataMap {
    pub root_id: i32,
    pub unknown_0ch: u32,
    pub unknown_12h: i16,
    pub unknown_14h: i16,
    pub unknown_16h: i16,
    pub entries: Vec<PsoMapEntry>,
}

impl Default for PsoDataMap {
    fn default() -> Self {
        Self {
            root_id: 0,
            unknown_0ch: 0,
            unknown_12h: 0x7070,
            unknown_14h: 0x7070,
            unknown_16h: 0x7070,
            entries: vec![],
        }
    }
}

/// `PsoCHKSSection`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PsoChecksum {
    pub file_size: u32,
    pub checksum: u32,
    pub unk0: u32,
}

/// A PSO file's sections (`PsoFile`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PsoFile {
    /// The data section, including its 8-byte header (block offsets count from
    /// its start).
    pub data: Option<Vec<u8>>,
    pub data_map: Option<PsoDataMap>,
    pub schema: Option<Vec<PsoElement>>,
    pub strf: Option<Vec<String>>,
    pub strs: Option<Vec<String>>,
    /// The `STRE` and `PSIG` sections' data (empty when they have none).
    pub stre: Option<Vec<u8>>,
    pub psig: Option<Vec<u8>>,
    pub chks: Option<PsoChecksum>,
}

// ---------------------------------------------------------------------------
// Classes and fields

/// `PsoFile.GetEngineType`.
fn engine_type(t: u8) -> ValueType {
    use ValueType as V;
    match t {
        dt::BOOL => V::Boolean,
        dt::SBYTE => V::Int8,
        dt::UBYTE => V::UInt8,
        dt::SSHORT => V::Int16,
        dt::USHORT => V::UInt16,
        dt::SINT => V::Int32,
        dt::UINT => V::UInt32,
        dt::FLOAT => V::Float,
        dt::FLOAT2 => V::Float2,
        dt::FLOAT3 => V::Float3,
        dt::FLOAT4 => V::Float4,
        dt::STRING => V::String,
        dt::STRUCTURE => V::Struct,
        dt::ARRAY => V::Array,
        dt::ENUM => V::Enum32,
        dt::FLAGS => V::Flags32,
        dt::MAP => V::Lookup,
        dt::FLOAT3A | dt::FLOAT4A | dt::VEC2V | dt::QUATV => V::Float4,
        dt::HFLOAT => V::UInt16,
        dt::ULONG => V::UInt64,
        dt::MAT34V => V::Float4x4,
        dt::LONG => V::Int64,
        _ => V::None,
    }
}

fn entry_at(entries: &[PsoEntry], i: u32, name: u32) -> Result<&PsoEntry> {
    entries.get(i as usize).ok_or_else(|| Error::Format(format!("PSO: 0x{name:08X}: entry index {i} out of range")))
}

/// `PsoStructureInfo.BuildFieldsFromEntries`: the data bag fields of a class.
pub fn build_fields(name: u32, entries: &[PsoEntry]) -> Result<Vec<SchemaField>> {
    let ismapinfo = (0x100..=0x110).contains(&name);
    let mut fields = Vec::with_capacity(entries.len());
    for e in entries {
        if e.name == 0x100 {
            continue; // ArrayInfo
        }
        let mut f = SchemaField {
            offset: e.offset as i32,
            name: e.name,
            type_name: e.reference_key,
            data_type: engine_type(e.data_type),
            ..Default::default()
        };
        match e.data_type {
            dt::ARRAY => {
                if ismapinfo {
                    f.array_type = ValueType::Struct;
                } else {
                    let arrentry = entry_at(entries, e.reference_key & 0x3FFF, e.name)?;
                    f.array_type = engine_type(arrentry.data_type);
                    f.array_size = (e.reference_key >> 16) & 0xFFFF;
                    f.type_name = arrentry.reference_key;
                    match e.unk_5h {
                        0 | 1 | 2 | 4 | 5 => {}
                        6 => f.data_type = ValueType::Object,
                        129 => f.offset += 131072,
                        _ => bail!("0x{:08X} - Unknown array type!", e.name),
                    }
                    match arrentry.data_type {
                        dt::STRUCTURE => {
                            if matches!(arrentry.unk_5h, 3 | 4) {
                                f.array_type = ValueType::Object;
                            }
                        }
                        dt::FLOAT3 => f.array_type = ValueType::Float4,
                        dt::STRING => f.array_type = ValueType::Hash,
                        dt::ARRAY => {
                            let aarrentry = entry_at(entries, arrentry.reference_key & 0x3FFF, e.name)?;
                            f.type_name = match aarrentry.data_type {
                                dt::STRUCTURE => aarrentry.reference_key,
                                dt::STRING => ValueType::Hash as u32,
                                t => t as u32,
                            };
                        }
                        _ => {}
                    }
                }
            }
            dt::MAP => {
                let valentry = entry_at(entries, e.reference_key & 0xFFFF, e.name)?;
                let keyentry = entry_at(entries, (e.reference_key >> 16) & 0xFFFF, e.name)?;
                f.type_name = valentry.reference_key;
                match valentry.data_type {
                    dt::MAP => {
                        entry_at(entries, valentry.reference_key & 0xFFFF, e.name)?;
                        entry_at(entries, (valentry.reference_key >> 16) & 0xFFFF, e.name)?;
                        f.type_name = dt::MAP as u32;
                    }
                    dt::ARRAY => {
                        f.type_name = entry_at(entries, valentry.reference_key & 0xFFFF, e.name)?.reference_key
                    }
                    dt::FLAGS => {
                        f.type_name = entry_at(entries, valentry.reference_key & 0xFFFF, e.name)?.reference_key
                    }
                    _ => {}
                }
                match keyentry.data_type {
                    dt::STRING if matches!(keyentry.unk_5h, 7 | 8) => f.array_type = ValueType::Hash,
                    dt::SSHORT => f.array_type = ValueType::Int16,
                    dt::ENUM => f.array_type = ValueType::Enum32,
                    dt::UINT => f.array_type = ValueType::UInt32,
                    _ => {}
                }
            }
            dt::STRING => match e.unk_5h {
                0 => {
                    f.array_size = (e.reference_key >> 16) & 0xFFFF;
                    f.type_name = 0;
                }
                1 | 2 => {}
                3 => f.array_type = ValueType::Char,
                7 | 8 | 11 | 13 => f.data_type = ValueType::Hash,
                _ => bail!("0x{:08X} - Unknown string type!", e.name),
            },
            dt::STRUCTURE => match e.unk_5h {
                0 => {}
                3 | 4 => f.data_type = ValueType::Object,
                _ => bail!("0x{:08X} - Unknown struct embed type!", e.name),
            },
            dt::FLAGS => {
                if !ismapinfo {
                    f.type_name = entry_at(entries, e.reference_key & 0xFFFF, e.name)?.reference_key;
                }
            }
            dt::ENUM => match e.unk_5h {
                1 => f.data_type = ValueType::Enum32,
                2 => f.data_type = ValueType::Enum16,
                3 => f.data_type = ValueType::Enum8,
                _ => {}
            },
            dt::FLOAT3A | dt::FLOAT4A | dt::QUATV | dt::VEC2V => f.type_name = e.data_type as u32,
            _ => {}
        }
        fields.push(f);
    }
    Ok(fields)
}

fn schema_class(s: &PsoStruct) -> Result<SchemaClass> {
    Ok(SchemaClass::new(s.name, s.data_size, Some(build_fields(s.name, &s.entries)?), ClassInfo::Plain))
}

fn schema_enum(e: &PsoEnum) -> SchemaEnum {
    let values = e.entries.iter().map(|v| EnumValue { name: v.name, value: v.value }).collect();
    SchemaEnum { name: e.name, values: Some(values), info: EnumInfo::Plain }
}

/// Every PSO class and enum of the game (`Rpf8Schemas.PsoSchema`), from
/// `schemas.txt`: what XML is read with.
pub struct PsoSchema {
    pub schema: Schema,
    pub structs: HashMap<u32, PsoStruct>,
    pub enums: HashMap<u32, PsoEnum>,
}

/// The PSO schema, made on first use.
pub fn pso_schema() -> &'static PsoSchema {
    static SCHEMA: OnceLock<PsoSchema> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let f = schema_file();
        let mut structs = HashMap::new();
        let mut classes = Vec::new();
        for c in &f.pso_classes {
            let entries: Option<Vec<PsoEntry>> = c
                .entries
                .iter()
                .map(|e| {
                    let e = e.as_ref()?;
                    Some(PsoEntry {
                        name: e.name,
                        data_type: dt::parse(&e.data_type)?,
                        unk_5h: e.unknown_5h,
                        offset: e.offset,
                        reference_key: e.reference_key,
                    })
                })
                .collect();
            let Some(entries) = entries else {
                log::warn!("schemas.txt: PSO class 0x{:08X} has a bad entry", c.name);
                continue;
            };
            let s = PsoStruct { name: c.name, ty: 0, unk: 0, data_size: c.size, unk_ch: 0, entries };
            match schema_class(&s) {
                Ok(cls) => {
                    classes.push(Arc::new(cls));
                    structs.insert(s.name, s);
                }
                Err(e) => log::warn!("schemas.txt: PSO class 0x{:08X}: {e}", c.name),
            }
        }
        let mut enums = HashMap::new();
        for e in &f.pso_enums {
            let entries = e.values.iter().flatten().map(|v| PsoEnumEntry { name: v.name, unk1: 0, value: v.value });
            enums.insert(e.name, PsoEnum { name: e.name, ty: 1, entries: entries.collect() });
        }
        let schema = Schema::new(classes, enums.values().map(|e| Arc::new(schema_enum(e))));
        PsoSchema { schema, structs, enums }
    })
}

// ---------------------------------------------------------------------------
// Pointers

/// `PsoPointer.BlockId`.
fn ptr_block_id(p: u64) -> i32 {
    (p & 0xFFFFF) as i32
}

/// `PsoPointer.DataOffset`.
fn ptr_offset(p: u64) -> i32 {
    let p1 = (p >> 20) & 0xFFF;
    let p2 = (p >> 44) & 0xFF000;
    let p3 = (p >> 28) & 0xFF00000;
    (p1 | p2 | p3) as i32
}

/// A pointer to block `index` (0-based), at `offset` in it.
fn make_ptr(index: usize, offset: i32) -> u64 {
    let v = offset as i64 as u64;
    (((index + 1) as u64) & 0xFFFFF) | ((v & 0xFFF) << 20) | ((v & 0xFF000) << 44) | ((v & 0xFF00000) << 28)
}

// ---------------------------------------------------------------------------
// Reading the sections

/// A big-endian reader over a section, failing at its end like `DataReader`.
struct Be<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Be<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn eof() -> Error {
        Error::Format("PSO: unexpected end of stream".into())
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let b = self.data.get(self.pos..self.pos + n).ok_or_else(Self::eof)?;
        self.pos += n;
        Ok(b)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.bytes(N)?.try_into().unwrap())
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        self.array().map(u16::from_be_bytes)
    }
    fn i16(&mut self) -> Result<i16> {
        self.array().map(i16::from_be_bytes)
    }
    fn u32(&mut self) -> Result<u32> {
        self.array().map(u32::from_be_bytes)
    }
    fn i32(&mut self) -> Result<i32> {
        self.array().map(i32::from_be_bytes)
    }
    fn i64(&mut self) -> Result<i64> {
        self.array().map(i64::from_be_bytes)
    }
    /// `BinaryReader.ReadBytes`: fewer at the end of the data.
    fn read_bytes(&mut self, n: i32) -> Result<&'a [u8]> {
        let n = usize::try_from(n).map_err(|_| Error::Format(format!("PSO: negative length {n}")))?;
        let end = (self.pos + n).min(self.data.len());
        let b = &self.data[self.pos..end];
        self.pos = end;
        Ok(b)
    }
    /// `ReadString(true)`: up to a 0 byte, as UTF-8.
    fn cstring(&mut self) -> Result<String> {
        let rest = self.data.get(self.pos..).unwrap_or(&[]);
        let n = rest.iter().position(|&b| b == 0).ok_or_else(Self::eof)?;
        self.pos += n + 1;
        Ok(String::from_utf8_lossy(&rest[..n]).into_owned())
    }
}

fn read_strings(sec: &[u8], hashes: &HashIndex) -> Result<Vec<String>> {
    let mut r = Be::new(sec);
    r.i32()?;
    r.i32()?;
    let mut strs = Vec::new();
    while r.pos < sec.len() {
        strs.push(r.cstring()?);
    }
    for s in &strs {
        hashes.ensure(s);
        hashes.ensure(&s.to_lowercase());
    }
    Ok(strs)
}

fn read_schema(sec: &[u8]) -> Result<Vec<PsoElement>> {
    let mut r = Be::new(sec);
    r.i32()?;
    r.i32()?;
    let mut count = r.u32()?;
    let trim = count > 0xFFF;
    if trim {
        count &= 0xFF;
    }
    let mut index = Vec::with_capacity(count as usize);
    for _ in 0..count {
        index.push((r.u32()?, r.i32()?));
    }
    let mut elements = Vec::with_capacity(count as usize);
    for (name, offset) in index {
        let offset = if trim { offset & 0xFFFF } else { offset };
        r.pos = usize::try_from(offset).map_err(|_| Error::Format("PSO: negative schema offset".into()))?;
        let ty = r.u8()?;
        r.pos -= 1;
        match ty {
            0 => {
                let x = r.u32()?;
                let mut count = (x & 0xFFFF) as i16;
                let unk = ((x >> 16) & 0xFF) as u8;
                let data_size = r.u32()?;
                let unk_ch = r.u32()?;
                if unk_ch != 0 {
                    count = count.wrapping_sub(unk_ch as i16);
                }
                if count < 0 {
                    bail!("PSO: class 0x{name:08X} has {count} entries");
                }
                let mut entries = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    entries.push(PsoEntry {
                        name: r.u32()?,
                        data_type: r.u8()?,
                        unk_5h: r.u8()?,
                        offset: r.u16()?,
                        reference_key: r.u32()?,
                    });
                }
                let s = PsoStruct { name, ty: (x >> 24) as u8, unk, data_size, unk_ch, entries };
                build_fields(s.name, &s.entries)?;
                elements.push(PsoElement::Struct(s));
            }
            1 => {
                let x = r.u32()?;
                let n = x & 0x00FF_FFFF;
                let mut entries = Vec::new();
                for _ in 0..n {
                    entries.push(PsoEnumEntry { name: r.u32()?, unk1: r.u32()?, value: r.i64()? });
                }
                elements.push(PsoElement::Enum(PsoEnum { name, ty: (x >> 24) as u8, entries }));
            }
            _ => bail!("unknown type!"),
        }
    }
    Ok(elements)
}

fn read_data_map(sec: &[u8]) -> Result<PsoDataMap> {
    let mut r = Be::new(sec);
    r.i32()?;
    r.i32()?;
    let root_id = r.i32()?;
    let unknown_0ch = r.u32()?;
    let mut count = r.i16()?;
    let (unknown_12h, unknown_14h, unknown_16h) = (r.i16()?, r.i16()?, r.i16()?);
    if unknown_0ch == 0x3D7215 {
        count = (((sec.len() - r.pos) / 4) / 4) as i16;
    }
    if count < 0 {
        bail!("PSO: the data map has {count} entries");
    }
    let mut entries = Vec::with_capacity(count as usize);
    for _ in 0..count {
        entries.push(PsoMapEntry { name_hash: r.u32()?, offset: r.i32()?, unknown_8h: r.i32()?, length: r.i32()? });
    }
    Ok(PsoDataMap { root_id, unknown_0ch, unknown_12h, unknown_14h, unknown_16h, entries })
}

impl PsoFile {
    /// `PsoFile.Load`: the sections (the strings sections' strings go into the
    /// hash index).
    pub fn read(data: &[u8], hashes: &HashIndex) -> Result<Self> {
        let mut f = PsoFile::default();
        let mut r = Be::new(data);
        while r.pos < data.len() {
            let ident = r.u32()?;
            let length = r.i32()?;
            r.pos -= 8;
            let sec = r.read_bytes(length)?;
            if sec.is_empty() {
                bail!("PSO: an empty section at {}", r.pos);
            }
            match ident {
                section::PSIN => {
                    let mut s = Be::new(sec);
                    s.i32()?;
                    let len = s.i32()?;
                    s.pos = 0;
                    f.data = Some(s.read_bytes(len)?.to_vec());
                }
                section::PMAP => f.data_map = Some(read_data_map(sec)?),
                section::PSCH => f.schema = Some(read_schema(sec)?),
                section::STRF => f.strf = Some(read_strings(sec, hashes)?),
                section::STRS => f.strs = Some(read_strings(sec, hashes)?),
                section::STRE | section::PSIG => {
                    let mut s = Be::new(sec);
                    s.i32()?;
                    let len = s.i32()?;
                    let d = Some(if len > 8 { s.read_bytes(len - 8)?.to_vec() } else { Vec::new() });
                    if ident == section::STRE {
                        f.stre = d;
                    } else {
                        f.psig = d;
                    }
                }
                section::CHKS => {
                    let mut s = Be::new(sec);
                    s.i32()?;
                    let len = s.i32()?;
                    let mut c = PsoChecksum { file_size: 0, checksum: 0, unk0: 0x7970_7070 };
                    if len == 20 {
                        c = PsoChecksum { file_size: s.u32()?, checksum: s.u32()?, unk0: s.u32()? };
                    }
                    f.chks = Some(c);
                }
                _ => {}
            }
        }
        Ok(f)
    }

    /// `ReadRootObject`: the root bag, and the schema of the file to write it
    /// to XML with. `None` when the file has no data or map, or the root can't
    /// be read.
    pub fn root(&self) -> Result<Option<(DataBag, Schema)>> {
        let (Some(data), Some(map)) = (&self.data, &self.data_map) else { return Ok(None) };
        let mut rd = Reader::new(self, data, map)?;
        let root = rd.object_at_ptr(map.root_id as u32 as u64 & 0xFFFFF)?;
        let enums: Vec<Arc<SchemaEnum>> = (self.schema.iter().flatten())
            .filter_map(|e| match e {
                PsoElement::Enum(e) => Some(Arc::new(schema_enum(e))),
                _ => None,
            })
            .collect();
        let classes: HashMap<u32, Arc<SchemaClass>> =
            rd.classes.into_iter().map(|(k, c)| (k, Arc::new(c.expect("class")))).collect();
        let schema = Schema::new(classes.values().cloned(), enums);
        let buf = rd.buf;
        Ok(root.map(|b| (finish(b, &classes, &buf), schema)))
    }
}

// ---------------------------------------------------------------------------
// Reading the data (PsoReader)

/// A bag being read: its values go into the shared buffer, and become a
/// [`DataBag`] once everything is read.
struct RBag {
    class: u32,
    offset: usize,
    objects: Vec<(u32, RVal)>,
}

enum RVal {
    Bag(Option<RBag>),
    Bags(Vec<Option<RBag>>),
    BagArrays(Vec<Option<Vec<Option<RBag>>>>),
    Value(Value),
}

impl RBag {
    /// `SetObject`: replaces a value of the same name.
    fn set(&mut self, name: u32, v: RVal) {
        match self.objects.iter_mut().find(|(n, _)| *n == name) {
            Some(o) => o.1 = v,
            None => self.objects.push((name, v)),
        }
    }
}

fn finish(b: RBag, classes: &HashMap<u32, Arc<SchemaClass>>, buf: &[u8]) -> DataBag {
    let cls = classes[&b.class].clone();
    let mut data = vec![0u8; cls.data_size as usize];
    if b.offset < buf.len() {
        let n = data.len().min(buf.len() - b.offset);
        data[..n].copy_from_slice(&buf[b.offset..b.offset + n]);
    }
    let mut bag = DataBag::with_data(cls, data);
    let bags = |v: Vec<Option<RBag>>| v.into_iter().map(|o| o.map(|b| finish(b, classes, buf))).collect();
    for (k, v) in b.objects {
        let v = match v {
            RVal::Bag(o) => o.map(|b| Value::Bag(Box::new(finish(b, classes, buf)))),
            RVal::Bags(v) => Some(Value::Bags(bags(v))),
            RVal::BagArrays(v) => Some(Value::BagArrays(v.into_iter().map(|a| a.map(bags)).collect())),
            RVal::Value(v) => Some(v),
        };
        bag.set_object(k, v);
    }
    bag
}

struct Reader<'a> {
    map: &'a PsoDataMap,
    data: &'a [u8],
    pos: i64,
    /// The bags' shared data (`new byte[DataSection.Data.Length]`).
    buf: Vec<u8>,
    /// The file's classes by name (the last of the same name), changed while
    /// reading. `None` only while being changed.
    classes: HashMap<u32, Option<SchemaClass>>,
    entries: HashMap<u32, &'a [PsoEntry]>,
    enums: HashSet<u32>,
    has_schema: bool,
}

/// `PsoArray`: pointer, count, capacity, unknown.
struct PsoArray {
    ptr: u64,
    count: u16,
    capacity: u16,
}

impl<'a> Reader<'a> {
    fn new(file: &'a PsoFile, data: &'a [u8], map: &'a PsoDataMap) -> Result<Self> {
        let mut classes = HashMap::new();
        let mut entries = HashMap::new();
        let mut enums = HashSet::new();
        for e in file.schema.iter().flatten() {
            match e {
                PsoElement::Struct(s) => {
                    classes.insert(s.name, Some(schema_class(s)?));
                    entries.insert(s.name, &s.entries[..]);
                }
                PsoElement::Enum(e) => {
                    enums.insert(e.name);
                }
            }
        }
        Ok(Self {
            map,
            data,
            pos: 0,
            buf: vec![0; data.len()],
            classes,
            entries,
            enums,
            has_schema: file.schema.is_some(),
        })
    }

    fn eof() -> Error {
        Error::Format("PSO: unexpected end of stream".into())
    }

    fn bytes<const N: usize>(&mut self) -> Result<[u8; N]> {
        let p = usize::try_from(self.pos).map_err(|_| Self::eof())?;
        let b = self.data.get(p..p + N).ok_or_else(Self::eof)?;
        self.pos += N as i64;
        Ok(b.try_into().unwrap())
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        self.bytes().map(u16::from_be_bytes)
    }
    fn u32(&mut self) -> Result<u32> {
        self.bytes().map(u32::from_be_bytes)
    }
    fn u64(&mut self) -> Result<u64> {
        self.bytes().map(u64::from_be_bytes)
    }
    fn f32(&mut self) -> Result<f32> {
        self.bytes().map(f32::from_be_bytes)
    }
    fn ptr(&mut self) -> Result<u64> {
        let lo = self.u32()? as u64;
        let hi = self.u32()? as u64;
        Ok(lo | (hi << 32))
    }
    fn array_info(&mut self) -> Result<PsoArray> {
        let ptr = self.ptr()?;
        let count = self.u16()?;
        let capacity = self.u16()?;
        self.u32()?;
        Ok(PsoArray { ptr, count, capacity })
    }
    fn cstring(&mut self) -> Result<String> {
        let p = usize::try_from(self.pos).map_err(|_| Self::eof())?;
        let rest = self.data.get(p..).unwrap_or(&[]);
        let n = rest.iter().position(|&b| b == 0).ok_or_else(Self::eof)?;
        self.pos += n as i64 + 1;
        Ok(String::from_utf8_lossy(&rest[..n]).into_owned())
    }

    /// `GetBlock`: the map entry of a block id.
    fn block(&self, id: i32) -> Option<PsoMapEntry> {
        let i = usize::try_from(id - 1).ok()?;
        self.map.entries.get(i).copied()
    }

    /// `SchemaSection.GetClass`.
    fn class(&self, name: u32) -> Result<Option<u32>> {
        if !self.has_schema {
            bail!("PSO: the file has no schema");
        }
        Ok(self.classes.contains_key(&name).then_some(name))
    }

    fn cls(&self, name: u32) -> &SchemaClass {
        self.classes[&name].as_ref().expect("class")
    }

    fn size(&self, name: u32) -> i64 {
        self.cls(name).data_size as i64
    }

    /// Copies a value into the shared buffer, at a field of a bag (`SetXxx`).
    fn set(&mut self, bag: &RBag, name: u32, bytes: &[u8]) {
        let Some(f) = self.cls(bag.class).field(name) else { return };
        let Ok(o) = usize::try_from(bag.offset as i64 + f.offset as i64) else { return };
        if o < self.buf.len() {
            let n = bytes.len().min(self.buf.len() - o);
            self.buf[o..o + n].copy_from_slice(&bytes[..n]);
        }
    }

    /// `SetEmbeddedArray`.
    fn set_embedded(&mut self, bag: &RBag, name: u32, bytes: &[u8]) {
        let Some(f) = self.cls(bag.class).field(name) else { return };
        if f.data_type == ValueType::Array && f.array_size != 0 {
            self.set(bag, name, bytes);
        }
    }

    /// `ReadObject(PsoPointer)`.
    fn object_at_ptr(&mut self, ptr: u64) -> Result<Option<RBag>> {
        let Some(block) = self.block(ptr_block_id(ptr)) else { return Ok(None) };
        let Some(cls) = self.class(block.name_hash)? else { return Ok(None) };
        let offset = ptr_offset(ptr) as i64 + block.offset as i64;
        if offset + self.size(cls) > self.data.len() as i64 {
            return Ok(None);
        }
        self.object(Some(cls), offset)
    }

    /// `ReadArray`: `None` for a null pointer, no items or a missing block.
    fn array<T>(&mut self, arr: &PsoArray, mut f: impl FnMut(&mut Self) -> Result<T>) -> Result<Option<Vec<T>>> {
        if arr.ptr == 0 || arr.count == 0 {
            return Ok(None);
        }
        let Some(block) = self.block(ptr_block_id(arr.ptr)) else { return Ok(None) };
        self.pos = block.offset as i64 + ptr_offset(arr.ptr) as i64;
        (0..arr.count).map(|_| f(self)).collect::<Result<Vec<T>>>().map(Some)
    }

    fn n_items<T>(&mut self, n: u32, mut f: impl FnMut(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        (0..n).map(|_| f(self)).collect()
    }

    /// `ReadObject(cls, offset)`.
    fn object(&mut self, cls: Option<u32>, offset: i64) -> Result<Option<RBag>> {
        let Some(cname) = cls else { return Ok(None) };
        if !self.cls(cname).has_fields() {
            return Ok(None);
        }
        let offset_u = usize::try_from(offset).map_err(|_| Self::eof())?;
        let mut bag = RBag { class: cname, offset: offset_u, objects: Vec::new() };
        let nfields = self.cls(cname).fields().len();
        for i in 0..nfields {
            let field = self.cls(cname).fields()[i].clone();
            let fname = field.name;
            self.pos = offset + field.offset as i64;
            let mut obj: Option<RVal> = None;
            match field.data_type {
                ValueType::Object => {
                    let p = self.ptr()?;
                    if p != 0 {
                        let o = self.object_at_ptr(p)?;
                        bag.set(fname, RVal::Bag(o));
                    }
                }
                ValueType::Struct => {
                    if let Some(c) = self.class(field.type_name)? {
                        let cbag = self.object(Some(c), offset + field.offset as i64)?;
                        bag.set(fname, RVal::Bag(cbag));
                    }
                }
                ValueType::Array if field.array_size == 0 => {
                    let arr = self.array_info()?;
                    if arr.ptr != 0 {
                        obj = self.pointer_array(&field, &arr)?;
                    }
                }
                ValueType::Array => obj = self.embedded_array(&mut bag, &field, offset)?,
                ValueType::Lookup => obj = self.lookup(cname)?,
                ValueType::String => {
                    if field.array_size == 0 {
                        let p = self.ptr()?;
                        if p != 0
                            && let Some(blk) = self.block(ptr_block_id(p))
                        {
                            let opos = self.pos;
                            self.pos = blk.offset as i64 + ptr_offset(p) as i64;
                            let s = self.cstring()?;
                            bag.set(fname, RVal::Value(Value::String(s)));
                            self.pos = opos;
                        }
                    } else {
                        let s = self.cstring()?;
                        bag.set(fname, RVal::Value(Value::String(s)));
                    }
                }
                ValueType::Boolean => {
                    let v = (self.u8()? == 1) as u8;
                    self.set(&bag, fname, &[v]);
                }
                ValueType::Int8 | ValueType::UInt8 | ValueType::Enum8 => {
                    let v = self.u8()?;
                    self.set(&bag, fname, &[v]);
                }
                ValueType::Int16 | ValueType::UInt16 | ValueType::Enum16 => {
                    let v = self.u16()?;
                    self.set(&bag, fname, &v.to_le_bytes());
                }
                ValueType::Int32 | ValueType::UInt32 | ValueType::Hash | ValueType::Enum32 | ValueType::Flags32 => {
                    let v = self.u32()?;
                    self.set(&bag, fname, &v.to_le_bytes());
                }
                ValueType::Int64 | ValueType::UInt64 => {
                    let v = self.u64()?;
                    self.set(&bag, fname, &v.to_le_bytes());
                }
                ValueType::Float | ValueType::Float2 | ValueType::Float3 | ValueType::Float4 => {
                    let n = match field.data_type {
                        ValueType::Float => 1,
                        ValueType::Float2 => 2,
                        ValueType::Float3 => 3,
                        _ => 4,
                    };
                    let mut b = Vec::with_capacity(n * 4);
                    for _ in 0..n {
                        b.extend_from_slice(&self.f32()?.to_le_bytes());
                    }
                    self.set(&bag, fname, &b);
                }
                ValueType::Float4x4 => {
                    // Read M11, M21, M31, M41, M12...: stored transposed.
                    let mut m = [0f32; 16];
                    for c in 0..4 {
                        for r in 0..4 {
                            m[r * 4 + c] = self.f32()?;
                        }
                    }
                    let b: Vec<u8> = m.iter().flat_map(|f| f.to_le_bytes()).collect();
                    self.set(&bag, fname, &b);
                }
                _ => {}
            }
            if let Some(o) = obj {
                bag.set(fname, o);
            }
        }
        Ok(Some(bag))
    }

    fn pointer_array(&mut self, field: &SchemaField, arr: &PsoArray) -> Result<Option<RVal>> {
        use ValueType as V;
        let v = |v: Option<Value>| Ok(v.map(RVal::Value));
        match field.array_type {
            V::Struct => {
                let scls = self.class(field.type_name)?;
                let Some(sblk) = self.block(ptr_block_id(arr.ptr)) else {
                    bail!("PSO: an array points to a missing block");
                };
                let scls = match scls {
                    Some(c) => Some(c),
                    None => self.class(sblk.name_hash)?,
                };
                let mut soffset = sblk.offset as i64 + ptr_offset(arr.ptr) as i64;
                let mut items = Vec::with_capacity(arr.capacity as usize);
                for _ in 0..arr.capacity {
                    items.push(self.object(scls, soffset)?);
                    let Some(c) = scls else { bail!("PSO: an array of an unknown class") };
                    soffset += self.size(c);
                }
                Ok(Some(RVal::Bags(items)))
            }
            V::Object => {
                let Some(ptrs) = self.array(arr, |r| r.ptr())? else { return Ok(None) };
                let mut items = Vec::with_capacity(ptrs.len());
                for p in ptrs {
                    items.push(self.object_at_ptr(p)?);
                }
                Ok(Some(RVal::Bags(items)))
            }
            V::Int8 => v(self.array(arr, |r| Ok(r.u8()? as i8))?.map(Value::SBytes)),
            V::Int32 => v(self.array(arr, |r| Ok(r.u32()? as i32))?.map(Value::I32s)),
            V::UInt8 => v(self.array(arr, |r| r.u8())?.map(Value::Bytes)),
            V::UInt16 => v(self.array(arr, |r| r.u16())?.map(Value::U16s)),
            V::UInt32 | V::Enum32 => v(self.array(arr, |r| r.u32())?.map(Value::U32s)),
            V::UInt64 => v(self.array(arr, |r| r.u64())?.map(Value::U64s)),
            V::Hash => v(self.array(arr, |r| r.u32())?.map(Value::Hashes)),
            V::Float => v(self.array(arr, |r| r.f32())?.map(Value::F32s)),
            V::Float2 => v(self.array(arr, |r| Ok([r.f32()?, r.f32()?]))?.map(Value::Vec2s)),
            V::Float4 => v(self.array(arr, |r| Ok([r.f32()?, r.f32()?, r.f32()?, r.f32()?]))?.map(Value::Vec4s)),
            V::Boolean => v(self.array(arr, |r| Ok(r.u8()? != 0))?.map(Value::Bools)),
            _ => Ok(None),
        }
    }

    fn embedded_array(&mut self, bag: &mut RBag, field: &SchemaField, offset: i64) -> Result<Option<RVal>> {
        use ValueType as V;
        let (n, fname) = (field.array_size, field.name);
        let le = |v: Vec<[u8; 4]>| v.concat();
        match field.array_type {
            V::Struct => {
                let Some(scls) = self.class(field.type_name)? else { return Ok(None) };
                let mut offs = offset + field.offset as i64;
                let mut items = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    items.push(self.object(Some(scls), offs)?);
                    offs += self.size(scls);
                }
                Ok(Some(RVal::Bags(items)))
            }
            V::Array => {
                let acls = self.class(field.type_name)?;
                let mut dtype = ValueType::Struct as u32;
                if acls.is_none() && field.type_name < 0x25 {
                    dtype = field.type_name;
                }
                let infos = self.n_items(n, |r| r.array_info())?;
                if dtype == ValueType::Struct as u32 {
                    let mut aarr = Vec::with_capacity(infos.len());
                    for arr in &infos {
                        let mut item = None;
                        if arr.ptr != 0
                            && arr.count > 0
                            && let Some(blk2) = self.block(ptr_block_id(arr.ptr))
                        {
                            let mut offs = blk2.offset as i64 + ptr_offset(arr.ptr) as i64;
                            let mut arr2 = Vec::with_capacity(arr.count as usize);
                            for _ in 0..arr.count {
                                arr2.push(self.object(acls, offs)?);
                                if let Some(c) = acls {
                                    offs += self.size(c);
                                }
                            }
                            item = Some(arr2);
                        }
                        aarr.push(item);
                    }
                    Ok(Some(RVal::BagArrays(aarr)))
                } else if dtype == ValueType::Hash as u32 {
                    let mut aarr = Vec::with_capacity(infos.len());
                    for arr in &infos {
                        let mut item = None;
                        if arr.ptr != 0 && arr.count > 0 {
                            item = self.array(arr, |r| r.u32())?;
                        }
                        aarr.push(item);
                    }
                    Ok(Some(RVal::Value(Value::HashArrays(aarr))))
                } else {
                    Ok(None)
                }
            }
            V::UInt8 | V::Boolean => {
                let mut b = self.n_items(n, |r| r.u8())?;
                if field.array_type == V::Boolean {
                    b.iter_mut().for_each(|v| *v = (*v != 0) as u8);
                }
                self.set_embedded(bag, fname, &b);
                Ok(None)
            }
            V::UInt16 => {
                let b: Vec<u8> = self.n_items(n, |r| r.u16())?.iter().flat_map(|v| v.to_le_bytes()).collect();
                self.set_embedded(bag, fname, &b);
                Ok(None)
            }
            V::UInt32 | V::Hash => {
                let b = le(self.n_items(n, |r| r.u32().map(u32::to_le_bytes))?);
                self.set_embedded(bag, fname, &b);
                Ok(None)
            }
            V::UInt64 => {
                let b: Vec<u8> = self.n_items(n, |r| r.u64())?.iter().flat_map(|v| v.to_le_bytes()).collect();
                self.set_embedded(bag, fname, &b);
                Ok(None)
            }
            V::Float => {
                let b = le(self.n_items(n, |r| r.f32().map(f32::to_le_bytes))?);
                self.set_embedded(bag, fname, &b);
                Ok(None)
            }
            V::Float4 => {
                let b = le(self.n_items(n * 4, |r| r.f32().map(f32::to_le_bytes))?);
                self.set_embedded(bag, fname, &b);
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// A map: its items, after resolving the item type of their class in place.
    fn lookup(&mut self, pcls: u32) -> Result<Option<RVal>> {
        self.ptr()?; // Unk1, Unk2
        let p = self.ptr()?;
        let count = self.u16()?;
        self.u16()?;
        self.u32()?;
        let Some(xblk) = self.block(ptr_block_id(p)) else { return Ok(None) };
        let Some(xcls) = self.class(xblk.name_hash)? else { return Ok(None) };
        let cls = self.cls(xcls);
        if let Some(idx) = cls.field_index(ITEM_HASH) {
            let itm = cls.fields()[idx].clone();
            let pentries = self.entries.get(&pcls).copied().unwrap_or(&[]);
            let resolve = |tn: u32| pentries.get((tn & 0xFFFF) as usize).map(|e| e.reference_key);
            let new_type = match itm.data_type {
                ValueType::Flags32 if !self.enums.contains(&itm.type_name) => resolve(itm.type_name),
                ValueType::Array => resolve(itm.type_name),
                _ => None,
            };
            if let Some(t) = new_type {
                let mut c = self.classes.get_mut(&xcls).unwrap().take().unwrap();
                c.field_at_mut(idx).unwrap().type_name = t;
                *self.classes.get_mut(&xcls).unwrap() = Some(c);
            }
        }
        let mut xoffs = xblk.offset as i64 + ptr_offset(p) as i64;
        let mut items = Vec::with_capacity(count as usize);
        for _ in 0..count {
            items.push(self.object(Some(xcls), xoffs)?);
            xoffs += self.size(xcls);
        }
        Ok(Some(RVal::Bags(items)))
    }
}

/// A PSO file as XML (`DataBag2.ToXml` of its root).
pub fn to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let pso = PsoFile::read(file, cx.hashes)?;
    let Some((root, schema)) = pso.root()? else { bail!("PSO: no root object") };
    Ok(root.to_xml(cx.hashes, Some(&schema)))
}

// ---------------------------------------------------------------------------
// Writing (PsoWriter)

/// `PsoWriter.Block`.
struct Block {
    ty: u32,
    index: usize,
    length: i32,
    alignment: i32,
    position: i32,
    /// What was written (the block's `MemoryStream`).
    buf: Vec<u8>,
}

impl Block {
    /// `AddObject`: room for `length` bytes, aligned.
    fn add_object(&mut self, length: i32) -> u64 {
        if self.alignment != 0 {
            self.length += (self.alignment - (self.length % self.alignment)) % self.alignment;
        }
        let p = make_ptr(self.index, self.length);
        self.length += length;
        p
    }

    fn write(&mut self, pos: usize, bytes: &[u8]) {
        if self.buf.len() < pos + bytes.len() {
            self.buf.resize(pos + bytes.len(), 0);
        }
        self.buf[pos..pos + bytes.len()].copy_from_slice(bytes);
    }
}

/// A class `WriteFields` knows the PSO entries of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PsoCls {
    /// From the PSO schema, by name.
    Static(u32),
    /// A lookup class made while writing.
    Lookup(usize),
}

struct LookupClass {
    info: PsoStruct,
    class: Arc<SchemaClass>,
}

struct Writer<'s> {
    schema: &'s PsoSchema,
    blocks: Vec<Block>,
    structs: Vec<PsoCls>,
    structs_set: HashSet<PsoCls>,
    enums: Vec<u32>,
    lookups: Vec<LookupClass>,
    /// `LookupClasses`: (class, field index) → lookup class, in insertion order.
    lookup_of_field: Vec<((usize, usize), usize)>,
}

fn pso_array_bytes(ptr: u64, count: u16) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[0..4].copy_from_slice(&(ptr as u32).to_be_bytes());
    b[4..8].copy_from_slice(&((ptr >> 32) as u32).to_be_bytes());
    b[8..10].copy_from_slice(&count.to_be_bytes());
    b[10..12].copy_from_slice(&count.to_be_bytes());
    b
}

fn ptr_bytes(p: u64) -> [u8; 8] {
    let mut b = [0u8; 8];
    b[0..4].copy_from_slice(&(p as u32).to_be_bytes());
    b[4..8].copy_from_slice(&((p >> 32) as u32).to_be_bytes());
    b
}

/// A pointer array's items as big-endian bytes, and the item size the C# counts.
enum Items {
    None,
    Some(Vec<u8>, usize, i32),
}

impl<'s> Writer<'s> {
    fn new(schema: &'s PsoSchema) -> Self {
        Self {
            schema,
            blocks: Vec::new(),
            structs: Vec::new(),
            structs_set: HashSet::new(),
            enums: Vec::new(),
            lookups: Vec::new(),
            lookup_of_field: Vec::new(),
        }
    }

    /// `GetBlock`.
    fn block(&mut self, ty: u32) -> usize {
        if let Some(b) = self.blocks.iter().find(|b| b.ty == ty && b.length < 0xFFFFF) {
            return b.index;
        }
        let index = self.blocks.len();
        self.blocks.push(Block {
            ty,
            index,
            length: 0,
            alignment: if ty < 8 { 0 } else { 8 },
            position: 0,
            buf: Vec::new(),
        });
        index
    }

    fn pso_class(&self, cls: &Arc<SchemaClass>) -> Option<PsoCls> {
        if let Some(c) = self.schema.schema.classes.get(&cls.name)
            && Arc::ptr_eq(c, cls)
        {
            return Some(PsoCls::Static(cls.name));
        }
        self.lookups.iter().position(|l| Arc::ptr_eq(&l.class, cls)).map(PsoCls::Lookup)
    }

    fn info(&self, c: PsoCls) -> &PsoStruct {
        match c {
            PsoCls::Static(n) => &self.schema.structs[&n],
            PsoCls::Lookup(i) => &self.lookups[i].info,
        }
    }

    fn class_of(&self, c: PsoCls) -> Arc<SchemaClass> {
        match c {
            PsoCls::Static(n) => self.schema.schema.classes[&n].clone(),
            PsoCls::Lookup(i) => self.lookups[i].class.clone(),
        }
    }

    /// `AddStruct`.
    fn add_struct(&mut self, c: PsoCls) {
        if !self.structs_set.insert(c) {
            return;
        }
        self.structs.push(c);
        let cls = self.class_of(c);
        for f in cls.fields() {
            if f.type_name == 0 {
                continue;
            }
            match f.data_type {
                ValueType::Enum8 | ValueType::Enum16 | ValueType::Enum32 | ValueType::Flags32 => {
                    if self.schema.enums.contains_key(&f.type_name) && !self.enums.contains(&f.type_name) {
                        self.enums.push(f.type_name);
                    }
                }
                ValueType::Struct | ValueType::Lookup => self.add_static(f.type_name),
                ValueType::Array if f.array_size > 0 => self.add_static(f.type_name),
                _ => {}
            }
        }
    }

    fn add_static(&mut self, name: u32) {
        if self.schema.structs.contains_key(&name) {
            self.add_struct(PsoCls::Static(name));
        }
    }

    /// `WriteObject`: the pointer to a new copy of the bag.
    fn object(&mut self, bag: Option<&DataBag>) -> Result<u64> {
        let Some(bag) = bag else { return Ok(0) };
        let bi = self.block(bag.class.name);
        let p = self.blocks[bi].add_object(bag.class.data_size as i32);
        self.fields(bag, bi, ptr_offset(p) as i64)?;
        Ok(p)
    }

    /// `WriteArray`.
    fn items(&mut self, items: Items, data_type: u8) -> [u8; 16] {
        let Items::Some(bytes, n, tsiz) = items else { return [0; 16] };
        if n == 0 {
            return [0; 16];
        }
        let bi = self.block(data_type as u32);
        let tsiz = if data_type == dt::BOOL { 1 } else { tsiz };
        let p = self.blocks[bi].add_object(tsiz * n as i32);
        self.blocks[bi].write(ptr_offset(p) as usize, &bytes);
        pso_array_bytes(p, n as u16)
    }

    /// `WriteFields`: a bag's fields at `offset` in block `bi`.
    fn fields(&mut self, bag: &DataBag, bi: usize, offset: i64) -> Result<()> {
        use ValueType as V;
        let cls = bag.class.clone();
        if !cls.has_fields() {
            return Ok(());
        }
        let psocls = self.pso_class(&cls);
        if let Some(c) = psocls {
            self.add_struct(c);
        }
        for (fi, field) in cls.fields().iter().enumerate() {
            let foff = offset + field.offset as i64;
            let Ok(foff) = usize::try_from(foff) else { bail!("PSO: a field at a negative offset") };
            let f = field.name;
            let put = |w: &mut Self, bytes: &[u8]| w.blocks[bi].write(foff, bytes);
            match field.data_type {
                V::Object => {
                    let p = self.object(bag.bag(f))?;
                    put(self, &ptr_bytes(p));
                }
                V::Struct => {
                    if let Some(s) = bag.bag(f) {
                        self.fields(s, bi, foff as i64)?;
                    }
                }
                V::Array if field.array_size == 0 => {
                    let aptr = self.pointer_array(bag, field, psocls)?;
                    put(self, &aptr);
                }
                V::Array => self.embedded_array(bag, field, bi, foff)?,
                V::Lookup => {
                    let mut lu = [0u8; 24];
                    if let Some(larr) = bag.bags(f).filter(|a| !a.is_empty()) {
                        let key = (Arc::as_ptr(&cls) as usize, fi);
                        let Some(kvp0) = &larr[0] else { bail!("PSO: an empty map item") };
                        let Some(li) = self.lookup_class(psocls, key, field, &kvp0.class)? else {
                            bail!("PSO: a map item without Key and Item");
                        };
                        let lcls = self.lookups[li].class.clone();
                        let lblk = self.block(lcls.name);
                        let lsiz = lcls.data_size as i32;
                        let lptr = self.blocks[lblk].add_object(lsiz * larr.len() as i32);
                        for (i, item) in larr.iter().enumerate() {
                            let Some(item) = item else { bail!("PSO: an empty map item") };
                            let lv = lookup_value(item, &lcls);
                            self.fields(&lv, lblk, ptr_offset(lptr) as i64 + (lsiz as i64) * i as i64)?;
                        }
                        lu[8..16].copy_from_slice(&ptr_bytes(lptr));
                        let n = (larr.len() as u16).to_be_bytes();
                        lu[16..18].copy_from_slice(&n);
                        lu[18..20].copy_from_slice(&n);
                        self.add_struct(PsoCls::Lookup(li));
                    }
                    lu[0..4].copy_from_slice(&0x0100_0000u32.to_be_bytes());
                    put(self, &lu);
                }
                V::String => {
                    let s = bag.string(f).unwrap_or("");
                    if field.array_size == 0 {
                        let sb = self.block(1);
                        let mut p = 0;
                        if !s.is_empty() {
                            p = self.blocks[sb].add_object(s.len() as i32 + 1);
                            let mut b = s.as_bytes().to_vec();
                            b.push(0);
                            self.blocks[sb].write(ptr_offset(p) as usize, &b);
                        }
                        put(self, &ptr_bytes(p));
                    } else {
                        let mut b = s.as_bytes().to_vec();
                        b.push(0);
                        put(self, &b);
                    }
                }
                V::Boolean => put(self, &[bag.bool(f) as u8]),
                V::Int8 | V::UInt8 | V::Enum8 => put(self, &[bag.u8(f)]),
                V::Int16 | V::UInt16 | V::Enum16 => put(self, &bag.u16(f).to_be_bytes()),
                V::Int32 | V::UInt32 | V::Hash | V::Enum32 | V::Flags32 => put(self, &bag.u32(f).to_be_bytes()),
                V::Int64 | V::UInt64 => put(self, &bag.u64(f).to_be_bytes()),
                V::Float => put(self, &bag.f32(f).to_be_bytes()),
                V::Float2 => put(self, &floats_be(&bag.vec2(f))),
                V::Float3 => put(self, &floats_be(&bag.vec3(f))),
                V::Float4 => put(self, &floats_be(&bag.vec4(f))),
                V::Float4x4 => {
                    let m = bag.matrix4x4(f);
                    let t: Vec<f32> = (0..16).map(|i| m[(i % 4) * 4 + i / 4]).collect();
                    put(self, &floats_be(&t));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn pointer_array(&mut self, bag: &DataBag, field: &SchemaField, psocls: Option<PsoCls>) -> Result<[u8; 16]> {
        use ValueType as V;
        let f = field.name;
        let entry = psocls.and_then(|c| self.info(c).entries.iter().find(|e| e.name == f).copied());
        let arrid = (entry.map(|e| e.reference_key).unwrap_or(0) & 0xFFFF) as usize;
        let arrtyp = match psocls {
            Some(c) => {
                let entries = &self.info(c).entries;
                let e = entries.get(if arrid < entries.len() { arrid } else { 0 });
                e.ok_or_else(|| Error::Format("PSO: a class without entries".into()))?.data_type
            }
            None => dt::UINT,
        };
        let be = |b: Vec<u8>, n: usize, s: i32| Items::Some(b, n, s);
        let items = match (field.array_type, bag.object(f)) {
            (V::Struct, Some(Value::Bags(sarr))) => {
                let size = sarr.first().and_then(|b| b.as_ref()).map(|b| b.class.data_size).unwrap_or(0);
                if size == 0 {
                    return Ok([0; 16]);
                }
                let cname = sarr[0].as_ref().unwrap().class.name;
                let blk = self.block(cname);
                let p = self.blocks[blk].add_object(sarr.len() as i32 * size as i32);
                for (i, item) in sarr.iter().enumerate() {
                    if let Some(item) = item {
                        self.fields(item, blk, ptr_offset(p) as i64 + size as i64 * i as i64)?;
                    }
                }
                return Ok(pso_array_bytes(p, sarr.len() as u16));
            }
            (V::Object, Some(Value::Bags(oarr))) => {
                if oarr.is_empty() {
                    return Ok([0; 16]);
                }
                let pblk = self.block(12);
                let p = self.blocks[pblk].add_object(oarr.len() as i32 * 8);
                for (i, item) in oarr.iter().enumerate() {
                    let ip = self.object(item.as_ref())?;
                    self.blocks[pblk].write(ptr_offset(p) as usize + i * 8, &ptr_bytes(ip));
                }
                return Ok(pso_array_bytes(p, oarr.len() as u16));
            }
            (V::Int8, Some(Value::SBytes(a))) => be(a.iter().map(|v| *v as u8).collect(), a.len(), 1),
            (V::Int8 | V::UInt8, Some(Value::Bytes(a))) => be(a.clone(), a.len(), 1),
            (V::UInt8, Some(Value::SBytes(a))) => be(a.iter().map(|v| *v as u8).collect(), a.len(), 1),
            (V::Int32 | V::UInt32 | V::Enum32, Some(Value::I32s(a))) => {
                be(a.iter().flat_map(|v| v.to_be_bytes()).collect(), a.len(), 4)
            }
            (V::Int32 | V::UInt32 | V::Enum32, Some(Value::U32s(a))) => {
                be(a.iter().flat_map(|v| v.to_be_bytes()).collect(), a.len(), 4)
            }
            (V::UInt16, Some(Value::U16s(a))) => be(a.iter().flat_map(|v| v.to_be_bytes()).collect(), a.len(), 2),
            (V::UInt16, Some(Value::I16s(a))) => be(a.iter().flat_map(|v| v.to_be_bytes()).collect(), a.len(), 2),
            (V::UInt64, Some(Value::U64s(a))) => be(a.iter().flat_map(|v| v.to_be_bytes()).collect(), a.len(), 8),
            (V::UInt64, Some(Value::I64s(a))) => be(a.iter().flat_map(|v| v.to_be_bytes()).collect(), a.len(), 8),
            (V::Hash, Some(Value::Hashes(a))) => be(a.iter().flat_map(|v| v.to_be_bytes()).collect(), a.len(), 4),
            (V::Float, Some(Value::F32s(a))) => be(floats_be(a), a.len(), 4),
            (V::Float2, Some(Value::Vec2s(a))) => be(floats_be(a.as_flattened()), a.len(), 8),
            (V::Float4, Some(Value::Vec4s(a))) => be(floats_be(a.as_flattened()), a.len(), 16),
            (V::Boolean, Some(Value::Bools(a))) => be(a.iter().map(|v| *v as u8).collect(), a.len(), 4),
            _ => Items::None,
        };
        let data_type = match field.array_type {
            V::Hash | V::Enum32 => dt::UINT,
            _ => arrtyp,
        };
        Ok(self.items(items, data_type))
    }

    fn embedded_array(&mut self, bag: &DataBag, field: &SchemaField, bi: usize, foff: usize) -> Result<()> {
        use ValueType as V;
        let f = field.name;
        let put = |w: &mut Self, bytes: &[u8]| w.blocks[bi].write(foff, bytes);
        match field.array_type {
            V::Struct => {
                if let Some(sarr) = bag.bags(f).filter(|a| !a.is_empty()) {
                    let ssiz = sarr[0].as_ref().map(|b| b.class.data_size).unwrap_or(0) as usize;
                    if ssiz > 0 {
                        for (i, item) in sarr.iter().enumerate() {
                            if let Some(item) = item {
                                self.fields(item, bi, (foff + i * ssiz) as i64)?;
                            }
                        }
                    }
                }
            }
            V::Array => {
                let dtype = if field.type_name < 0x25 { field.type_name } else { V::Struct as u32 };
                if dtype == V::Struct as u32 {
                    if let Some(Value::BagArrays(aarr)) = bag.object(f) {
                        for (i, arr) in aarr.iter().enumerate() {
                            let mut aptr = [0u8; 16];
                            if let Some(arr) = arr.as_ref().filter(|a| !a.is_empty()) {
                                let cls = arr[0].as_ref().map(|b| b.class.clone());
                                if let Some(cls) = cls.filter(|c| c.data_size > 0) {
                                    let size = cls.data_size as i64;
                                    let blk = self.block(cls.name);
                                    let p = self.blocks[blk].add_object((arr.len() as i64 * size) as i32);
                                    for (j, item) in arr.iter().enumerate() {
                                        if let Some(item) = item {
                                            self.fields(item, blk, ptr_offset(p) as i64 + size * j as i64)?;
                                        }
                                    }
                                    aptr = pso_array_bytes(p, arr.len() as u16);
                                }
                            }
                            self.blocks[bi].write(foff + i * 16, &aptr);
                        }
                    }
                } else if dtype == V::Hash as u32
                    && let Some(Value::HashArrays(aarr)) = bag.object(f)
                {
                    for (i, arr) in aarr.iter().enumerate() {
                        let mut aptr = [0u8; 16];
                        if let Some(arr) = arr.as_ref().filter(|a| !a.is_empty()) {
                            let blk = self.block(6);
                            let p = self.blocks[blk].add_object(arr.len() as i32 * 4);
                            let b: Vec<u8> = arr.iter().flat_map(|v| v.to_be_bytes()).collect();
                            self.blocks[blk].write(ptr_offset(p) as usize, &b);
                            aptr = pso_array_bytes(p, arr.len() as u16);
                        }
                        self.blocks[bi].write(foff + i * 16, &aptr);
                    }
                }
            }
            V::UInt8 => {
                if let Some(a) = bag.embedded_array::<u8>(f) {
                    put(self, &a);
                }
            }
            V::Boolean => {
                if let Some(a) = bag.embedded_array::<u8>(f) {
                    put(self, &a.iter().map(|v| (*v != 0) as u8).collect::<Vec<_>>());
                }
            }
            V::UInt16 => {
                if let Some(a) = bag.embedded_array::<u16>(f) {
                    put(self, &a.iter().flat_map(|v| v.to_be_bytes()).collect::<Vec<_>>());
                }
            }
            V::UInt32 | V::Hash => {
                if let Some(a) = bag.embedded_array::<u32>(f) {
                    put(self, &a.iter().flat_map(|v| v.to_be_bytes()).collect::<Vec<_>>());
                }
            }
            V::UInt64 => {
                if let Some(a) = bag.embedded_array::<u64>(f) {
                    put(self, &a.iter().flat_map(|v| v.to_be_bytes()).collect::<Vec<_>>());
                }
            }
            V::Float => {
                if let Some(a) = bag.embedded_array::<f32>(f) {
                    put(self, &floats_be(&a));
                }
            }
            V::Float4 => {
                if let Some(a) = bag.embedded_array::<[f32; 4]>(f) {
                    put(self, &floats_be(a.as_flattened()));
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// `GetLookupClass`: the key/item class for a map field, made the first time.
    fn lookup_class(
        &mut self,
        psocls: Option<PsoCls>,
        key: (usize, usize),
        field: &SchemaField,
        kvpcls: &SchemaClass,
    ) -> Result<Option<usize>> {
        use ValueType as V;
        let (Some(kfld), Some(ifld)) = (kvpcls.field(KEY_HASH), kvpcls.field(ITEM_HASH)) else { return Ok(None) };
        let Some(psocls) = psocls else { bail!("PSO: a map in a class that isn't a PSO class") };
        // XML reads these as objects, but they are embedded structs.
        let itype = if ifld.data_type == V::Object && field.type_name != 0 { V::Struct } else { ifld.data_type };
        if let Some((_, li)) = self.lookup_of_field.iter().find(|(k, _)| *k == key) {
            return Ok(Some(*li));
        }
        let mut name = 0x100u32;
        for (_, li) in &self.lookup_of_field {
            let c = self.lookups[*li].info.name;
            if c >= name {
                name = c + 1;
            }
        }
        let mut kent = PsoEntry { name: KEY_HASH, data_type: dt::STRING, ..Default::default() };
        let mut ient = PsoEntry { name: ITEM_HASH, data_type: dt::STRUCTURE, offset: 8, ..Default::default() };
        let mut slen = 16u32;
        let mut i32 = false;
        match kfld.data_type {
            V::Hash => kent.unk_5h = 7,
            V::Int16 => kent.data_type = dt::SSHORT,
            V::Enum32 => {
                kent.data_type = dt::ENUM;
                kent.unk_5h = 1;
                kent.reference_key = kfld.type_name;
            }
            V::UInt32 => kent.data_type = dt::UINT,
            _ => {}
        }
        let entries = self.info(psocls).entries.clone();
        let index_of = |t: u32| entries.iter().position(|e| e.reference_key == t).unwrap_or(0) as u32;
        match itype {
            V::Hash => {
                ient.data_type = dt::STRING;
                ient.unk_5h = 7;
                i32 = true;
            }
            V::Float => {
                ient.data_type = dt::FLOAT;
                i32 = true;
            }
            V::Enum32 => {
                ient.data_type = dt::ENUM;
                ient.unk_5h = 1;
                ient.reference_key = ifld.type_name;
                i32 = true;
            }
            V::String => {
                ient.data_type = dt::STRING;
                ient.unk_5h = 3;
            }
            V::UInt32 => {
                ient.data_type = dt::UINT;
                i32 = true;
            }
            V::Object => ient.unk_5h = 3,
            V::Struct => {
                ient.reference_key = ifld.type_name;
                slen = match self.schema.schema.classes.get(&ifld.type_name) {
                    Some(c) => 8 + c.data_size,
                    None => 32,
                };
            }
            V::Flags32 => {
                ient.data_type = dt::FLAGS;
                i32 = true;
                ient.reference_key = (0x18 << 16) + (index_of(ifld.type_name) & 0xFFFF);
            }
            V::Float4 => {
                ient.data_type = if ifld.type_name != 0 { ifld.type_name as u8 } else { dt::FLOAT4 };
                ient.offset = 16;
                slen = 32;
            }
            V::Array => {
                ient.data_type = dt::ARRAY;
                ient.reference_key = index_of(ifld.type_name) & 0xFFFF;
                slen = 24;
            }
            _ => {}
        }
        if i32 {
            ient.offset = 4;
            slen = 8;
        }
        for (_, li) in &self.lookup_of_field {
            let e = &self.lookups[*li].info.entries;
            let (lk, lv) = (e[0], e[1]);
            if lk.data_type == kent.data_type
                && lk.reference_key == kent.reference_key
                && lv.data_type == ient.data_type
                && lv.reference_key == ient.reference_key
            {
                let li = *li;
                self.lookup_of_field.push((key, li));
                return Ok(Some(li));
            }
        }
        let info = PsoStruct { name, ty: 0, unk: 2, data_size: slen, unk_ch: 0, entries: vec![kent, ient] };
        let class = Arc::new(schema_class(&info)?);
        self.lookups.push(LookupClass { info, class });
        let li = self.lookups.len() - 1;
        self.lookup_of_field.push((key, li));
        Ok(Some(li))
    }

    /// `CreateDataSection`, `CreateDataMapSection` and `CreateSchemaSection`.
    fn finish(mut self) -> PsoFile {
        let mut data = vec![0u8; 16];
        for b in &mut self.blocks {
            b.position = data.len() as i32;
            data.extend_from_slice(&b.buf);
            data.resize(data.len().next_multiple_of(16), 0);
        }
        let entries = (self.blocks.iter())
            .map(|b| PsoMapEntry { name_hash: b.ty, offset: b.position, unknown_8h: 0, length: b.length })
            .collect();
        let map = PsoDataMap { root_id: 1, entries, ..Default::default() };
        let mut schema: Vec<PsoElement> =
            self.structs.iter().map(|c| PsoElement::Struct(self.info(*c).clone())).collect();
        schema.extend(self.enums.iter().map(|e| PsoElement::Enum(self.schema.enums[e].clone())));
        PsoFile { data: Some(data), data_map: Some(map), schema: Some(schema), ..Default::default() }
    }
}

fn floats_be(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_be_bytes()).collect()
}

/// `GetLookupValue`: a map item as a bag of its lookup class.
fn lookup_value(orig: &DataBag, lcls: &Arc<SchemaClass>) -> DataBag {
    use ValueType as V;
    let mut bag = DataBag::new(lcls.clone());
    match lcls.field(KEY_HASH).map(|f| f.data_type) {
        Some(V::Hash) => bag.set_hash(KEY_HASH, orig.hash(KEY_HASH)),
        Some(V::Int16) => bag.set_i16(KEY_HASH, orig.i16(KEY_HASH)),
        Some(V::Enum32 | V::UInt32) => bag.set_u32(KEY_HASH, orig.u32(KEY_HASH)),
        _ => {}
    }
    match lcls.field(ITEM_HASH).map(|f| f.data_type) {
        Some(V::Hash) => bag.set_hash(ITEM_HASH, orig.hash(ITEM_HASH)),
        Some(V::Float) => bag.set_f32(ITEM_HASH, orig.f32(ITEM_HASH)),
        Some(V::Float4) => bag.set_vec4(ITEM_HASH, orig.vec4(ITEM_HASH)),
        Some(V::Enum32 | V::Flags32 | V::UInt32) => bag.set_u32(ITEM_HASH, orig.u32(ITEM_HASH)),
        Some(V::String | V::Object | V::Struct | V::Array) => {
            bag.set_object(ITEM_HASH, orig.object(ITEM_HASH).cloned())
        }
        _ => {}
    }
    bag
}

impl PsoFile {
    /// `WriteRootObject`: a file with the data, map and schema of a bag tree whose
    /// classes are from [`pso_schema`].
    pub fn from_root(bag: &DataBag) -> Result<Self> {
        let mut w = Writer::new(pso_schema());
        w.object(Some(bag))?;
        Ok(w.finish())
    }

    /// `PsoFile.Save`: the sections, in the C#'s order.
    pub fn write(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let header = |out: &mut Vec<u8>, ident: u32, len: usize| {
            out.extend_from_slice(&ident.to_be_bytes());
            out.extend_from_slice(&(len as u32).to_be_bytes());
        };
        if let Some(d) = &self.data {
            let start = out.len();
            out.extend_from_slice(d);
            if d.len() >= 8 {
                out[start..start + 4].copy_from_slice(&section::PSIN.to_be_bytes());
                out[start + 4..start + 8].copy_from_slice(&(d.len() as u32).to_be_bytes());
            }
        }
        if let Some(m) = &self.data_map {
            header(&mut out, section::PMAP, 24 + 16 * m.entries.len());
            out.extend_from_slice(&m.root_id.to_be_bytes());
            out.extend_from_slice(&m.unknown_0ch.to_be_bytes());
            out.extend_from_slice(&(m.entries.len() as i16).to_be_bytes());
            for v in [m.unknown_12h, m.unknown_14h, m.unknown_16h] {
                out.extend_from_slice(&v.to_be_bytes());
            }
            for e in &m.entries {
                for v in [e.name_hash as i32, e.offset, e.unknown_8h, e.length] {
                    out.extend_from_slice(&v.to_be_bytes());
                }
            }
        }
        if let Some(elements) = &self.schema {
            let mut index = Vec::new();
            let mut body = Vec::new();
            let base = 12 + 8 * elements.len();
            for e in elements {
                let name = match e {
                    PsoElement::Struct(s) => s.name,
                    PsoElement::Enum(e) => e.name,
                };
                index.extend_from_slice(&name.to_be_bytes());
                index.extend_from_slice(&((base + body.len()) as i32).to_be_bytes());
                match e {
                    PsoElement::Struct(s) => {
                        let x = ((s.unk as u32) << 16) | (s.entries.len() as u16 as u32);
                        body.extend_from_slice(&x.to_be_bytes());
                        body.extend_from_slice(&s.data_size.to_be_bytes());
                        body.extend_from_slice(&s.unk_ch.to_be_bytes());
                        for e in &s.entries {
                            body.extend_from_slice(&e.name.to_be_bytes());
                            body.push(e.data_type);
                            body.push(e.unk_5h);
                            body.extend_from_slice(&e.offset.to_be_bytes());
                            body.extend_from_slice(&e.reference_key.to_be_bytes());
                        }
                    }
                    PsoElement::Enum(e) => {
                        let x = (1u32 << 24) | e.entries.len() as u32;
                        body.extend_from_slice(&x.to_be_bytes());
                        for v in &e.entries {
                            body.extend_from_slice(&v.name.to_be_bytes());
                            body.extend_from_slice(&v.unk1.to_be_bytes());
                            body.extend_from_slice(&v.value.to_be_bytes());
                        }
                    }
                }
            }
            header(&mut out, section::PSCH, 12 + body.len() + index.len());
            out.extend_from_slice(&(elements.len() as u32).to_be_bytes());
            out.extend_from_slice(&index);
            out.extend_from_slice(&body);
        }
        for (ident, strs) in [(section::STRF, &self.strf), (section::STRS, &self.strs)] {
            if let Some(strs) = strs {
                let mut b = Vec::new();
                for s in strs {
                    b.extend(s.encode_utf16().map(|c| c as u8));
                    b.push(0);
                }
                header(&mut out, ident, b.len() + 8);
                out.extend_from_slice(&b);
            }
        }
        for (ident, d) in [(section::STRE, &self.stre), (section::PSIG, &self.psig)] {
            if let Some(d) = d {
                header(&mut out, ident, d.len() + 8);
                out.extend_from_slice(d);
            }
        }
        if let Some(c) = &self.chks {
            header(&mut out, section::CHKS, 20);
            for v in [c.file_size, c.checksum, c.unk0] {
                out.extend_from_slice(&v.to_be_bytes());
            }
        }
        out
    }
}

/// XML back to a PSO file (`new PsoFile(xml).Save()`).
pub fn from_xml(xml: &str, cx: &XmlContext) -> Result<Vec<u8>> {
    let schema = pso_schema();
    let Some(root) = DataBag::from_xml(xml, Some(&schema.schema), cx.hashes)? else {
        bail!("PSO XML: the root isn't a known PSO class");
    };
    Ok(PsoFile::from_root(&root)?.write())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointers() {
        for (i, o) in [(0usize, 0i32), (3, 0x10), (7, 0x12345), (0x123, 0x0FFF_FFF0)] {
            let p = make_ptr(i, o);
            assert_eq!(ptr_block_id(p), i as i32 + 1);
            assert_eq!(ptr_offset(p), o);
        }
    }

    #[test]
    fn schema_builds() {
        let s = pso_schema();
        assert!(s.structs.len() > 2000, "{}", s.structs.len());
        assert!(!s.enums.is_empty());
    }
}
