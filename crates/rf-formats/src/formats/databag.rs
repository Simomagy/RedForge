//! The generic, schema-driven bag of values that resource metadata (ymap, ytyp,
//! ymt), PSO and RBF files are read into, with its XML: RedForge's `DataBag2`,
//! `DataSchema` and friends (`RedForge.Core/Engine/DataBag.cs`).
//!
//! A [`DataBag`] is one struct instance: its [`SchemaClass`], the struct's bytes
//! as stored in the file (every fixed-size field: numbers, vectors, hashes, enums,
//! embedded arrays and structs), and the values that live outside the struct
//! (strings, pointed-to objects, arrays), keyed by field name hash.
//!
//! ```ignore
//! let meta = Meta::read(&file)?;                       // a ymap
//! let mut root = meta.root.unwrap();                   // CMapData
//! for e in root.bags_mut("entities").into_iter().flatten().flatten() {
//!     let name = e.hash("archetypeName");
//!     let [x, y, z] = e.vec3("position");
//!     e.set_vec3("position", [x, y, z + 1.0]);
//! }
//! ```
//!
//! Field names are joaat hashes; every accessor takes either the name (`"position"`)
//! or its hash. Getters return zero for fields the class doesn't have, as the C#.
//!
//! Known gaps: reading XML without a schema (`CreateSchemaClass`, for RBF and
//! plain XML) is ported but only checked by unit tests so far; CDATA sections
//! are read as text and `xmlns` declarations aren't counted as attributes, unlike
//! `XmlDocument`; text with control characters is escaped as `XmlNode.InnerXml`
//! does it as far as known.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use bytemuck::Pod;
use roxmltree::{Node, NodeId};

use super::meta::{RscClassInfo, RscEnumInfo};
use crate::hash::{HashIndex, JenkHash, joaat};
use crate::xml::number::{f32_to_string, f64_to_string, parse_f32, push_f32};
use crate::xml::parse_hash;
use crate::{Error, Result};

/// The XML header every document starts with (`Xml.Header`).
pub const XML_HEADER: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>";

/// `"type"`: never written as an attribute a second time.
const TYPE_HASH: u32 = 0x3165_B05D;
/// `"Key"` / `"Item"`: the fields of a lookup (binary map) entry.
pub const KEY_HASH: u32 = 0x6098_A50E;
pub const ITEM_HASH: u32 = 0x063F_A3F2;
const KEY_LOWER_HASH: u32 = 0xE980_6307;
const ITEM_LOWER_HASH: u32 = 0xD3B3_60C4;
/// `"KeyValuePair"`: the class of the lookup entries read from XML.
pub const KEY_VALUE_PAIR_HASH: u32 = 0x6306_2CEA;

/// Something that names a field or class: its name, or its joaat hash.
pub trait FieldKey {
    fn key(self) -> u32;
}
impl FieldKey for u32 {
    fn key(self) -> u32 {
        self
    }
}
impl FieldKey for JenkHash {
    fn key(self) -> u32 {
        self.0
    }
}
impl FieldKey for &str {
    fn key(self) -> u32 {
        joaat(self)
    }
}
impl FieldKey for &String {
    fn key(self) -> u32 {
        joaat(self)
    }
}

/// `DataBagValueType`: how a field's value is stored and written.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ValueType {
    #[default]
    None = 0,
    Float = 1,
    Float2 = 2,
    Float3 = 3,
    Float4 = 4,
    Float3x4 = 5,
    Float4x4 = 6,
    UInt8 = 7,
    UInt16 = 8,
    UInt32 = 9,
    UInt64 = 10,
    Int8 = 11,
    Int16 = 12,
    Int32 = 13,
    Int64 = 14,
    Double = 15,
    String = 16,
    Array = 17,
    /// A pointer to another object.
    Object = 18,
    /// An object embedded in the struct.
    Struct = 19,
    Enum8 = 20,
    Enum16 = 21,
    Enum32 = 22,
    /// A flags enum: each value is a bit index.
    Flags32 = 23,
    Boolean = 24,
    Hash = 25,
    Char = 26,
    /// A binary map, stored as an array of key/item bags.
    Lookup = 27,
    FnvHash = 28,
}

impl ValueType {
    /// `DataBagValueTypes.GetLength`: the bytes the value takes in the struct, 0
    /// for values stored outside it (and for embedded structs).
    pub fn size(self) -> u32 {
        use ValueType::*;
        match self {
            None | String | Array | Lookup | Object | Struct => 0,
            Float => 4,
            Float2 => 8,
            Float3 => 12,
            Float4 => 16,
            Float3x4 => 48,
            Float4x4 => 64,
            UInt8 | Int8 | Enum8 | Boolean | Char => 1,
            UInt16 | Int16 | Enum16 => 2,
            UInt32 | Int32 | Enum32 | Flags32 | Hash => 4,
            UInt64 | Int64 | Double | FnvHash => 8,
        }
    }

    pub fn is_enum(self) -> bool {
        matches!(self, ValueType::Enum8 | ValueType::Enum16 | ValueType::Enum32 | ValueType::Flags32)
    }
}

/// `DataSchemaField`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SchemaField {
    /// The offset in the struct; -1 for values not stored in it (schema-less XML).
    pub offset: i32,
    pub name: u32,
    /// The class of a struct/object/array item, or the enum of an enum field.
    pub type_name: u32,
    pub data_type: ValueType,
    pub array_type: ValueType,
    /// For arrays embedded in the struct: the item count. 0 for arrays stored
    /// outside (behind a pointer).
    pub array_size: u32,
    pub is_attribute: bool,
    pub is_link: bool,
}

/// Format-specific details of a class, kept to write it back.
#[derive(Clone, Debug, Default)]
pub enum ClassInfo {
    #[default]
    Plain,
    /// A resource metadata (`Rsc8MetaClass`) class.
    Rsc(RscClassInfo),
}

/// `DataSchemaClass`: a struct type.
#[derive(Clone, Debug, Default)]
pub struct SchemaClass {
    pub name: u32,
    pub data_size: u32,
    /// `None` when the class has no field list (the C#'s null `Fields`).
    fields: Option<Vec<SchemaField>>,
    /// Field name → index in `fields`; the last of fields with the same name wins.
    lookup: HashMap<u32, usize>,
    pub info: ClassInfo,
}

impl SchemaClass {
    pub fn new(name: u32, data_size: u32, fields: Option<Vec<SchemaField>>, info: ClassInfo) -> Self {
        let mut c = Self { name, data_size, fields: None, lookup: HashMap::new(), info };
        c.set_fields(fields);
        c
    }

    /// Replaces the fields (`BuildFieldsLookup`).
    pub fn set_fields(&mut self, fields: Option<Vec<SchemaField>>) {
        self.lookup.clear();
        if let Some(f) = &fields {
            for (i, field) in f.iter().enumerate() {
                self.lookup.insert(field.name, i);
            }
        }
        self.fields = fields;
    }

    /// The fields, in struct order (empty when the class has none).
    pub fn fields(&self) -> &[SchemaField] {
        self.fields.as_deref().unwrap_or(&[])
    }

    /// Whether the class has a field list at all (possibly empty).
    pub fn has_fields(&self) -> bool {
        self.fields.is_some()
    }

    pub fn field_index(&self, name: impl FieldKey) -> Option<usize> {
        self.lookup.get(&name.key()).copied()
    }

    pub fn field(&self, name: impl FieldKey) -> Option<&SchemaField> {
        let i = self.field_index(name)?;
        self.fields.as_ref().map(|f| &f[i])
    }

    /// A field to modify in place. Its name must stay the same.
    pub fn field_at_mut(&mut self, index: usize) -> Option<&mut SchemaField> {
        self.fields.as_mut().and_then(|f| f.get_mut(index))
    }

    pub fn rsc(&self) -> Option<&RscClassInfo> {
        match &self.info {
            ClassInfo::Rsc(r) => Some(r),
            ClassInfo::Plain => None,
        }
    }

    /// `CompareTo`: same name, size and fields.
    pub fn same_layout(&self, b: &SchemaClass) -> bool {
        self.name == b.name && self.data_size == b.data_size && self.fields == b.fields
    }
}

/// One named value of an enum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EnumValue {
    pub name: u32,
    pub value: i64,
}

/// Format-specific details of an enum.
#[derive(Clone, Debug, Default)]
pub enum EnumInfo {
    #[default]
    Plain,
    Rsc(RscEnumInfo),
}

/// `DataSchemaEnum`.
#[derive(Clone, Debug, Default)]
pub struct SchemaEnum {
    pub name: u32,
    /// `None` when the file has no value list for it.
    pub values: Option<Vec<EnumValue>>,
    pub info: EnumInfo,
}

impl SchemaEnum {
    /// The name of a value, as `FormatEnumValue` finds it.
    pub fn value_name(&self, v: i64) -> Option<u32> {
        let values = self.values.as_ref()?;
        values
            .iter()
            .find(|e| e.value == v)
            .or_else(|| values.iter().find(|e| e.value == v as u32 as i64))
            .map(|e| e.name)
    }
}

/// `DataSchema`: classes and enums by name hash.
#[derive(Clone, Debug, Default)]
pub struct Schema {
    pub classes: HashMap<u32, Arc<SchemaClass>>,
    pub enums: HashMap<u32, Arc<SchemaEnum>>,
}

impl Schema {
    /// `BuildLookups`: later classes or enums with the same name replace earlier ones.
    pub fn new(
        classes: impl IntoIterator<Item = Arc<SchemaClass>>,
        enums: impl IntoIterator<Item = Arc<SchemaEnum>>,
    ) -> Self {
        let mut s = Self::default();
        for c in classes {
            s.classes.insert(c.name, c);
        }
        for e in enums {
            s.enums.insert(e.name, e);
        }
        s
    }

    pub fn class(&self, name: impl FieldKey) -> Option<&Arc<SchemaClass>> {
        self.classes.get(&name.key())
    }

    pub fn enumeration(&self, name: impl FieldKey) -> Option<&Arc<SchemaEnum>> {
        self.enums.get(&name.key())
    }

    /// `CreateInstance`: a new, zeroed instance of a class.
    pub fn create(&self, class: impl FieldKey) -> Option<DataBag> {
        self.class(class).map(|c| DataBag::new(c.clone()))
    }
}

/// A value stored outside a bag's struct data (the C#'s `Objects`).
#[derive(Clone, Debug)]
pub enum Value {
    String(String),
    /// A pointed-to or embedded object.
    Bag(Box<DataBag>),
    /// An array of objects; `None` for items that couldn't be read.
    Bags(Vec<Option<DataBag>>),
    BagArrays(Vec<Option<Vec<Option<DataBag>>>>),
    HashArrays(Vec<Option<Vec<u32>>>),
    Strings(Vec<String>),
    Hashes(Vec<u32>),
    /// Bytes: a `u8` array, or a raw data block.
    Bytes(Vec<u8>),
    SBytes(Vec<i8>),
    U16s(Vec<u16>),
    I16s(Vec<i16>),
    U32s(Vec<u32>),
    I32s(Vec<i32>),
    U64s(Vec<u64>),
    I64s(Vec<i64>),
    F32s(Vec<f32>),
    Vec2s(Vec<[f32; 2]>),
    Vec3s(Vec<[f32; 3]>),
    /// Also how `Float3` arrays are kept: they are stored as 4 floats each.
    Vec4s(Vec<[f32; 4]>),
    Bools(Vec<bool>),
    KeyValue(String, Option<Box<DataBag>>),
    KeyValues(Vec<(String, Option<DataBag>)>),
}

/// `DataBag2`: one struct instance.
#[derive(Clone, Debug)]
pub struct DataBag {
    pub class: Arc<SchemaClass>,
    /// The struct's bytes, `class.data_size` long.
    pub data: Vec<u8>,
    objects: Vec<(u32, Value)>,
}

fn read_pod<T: Pod>(data: &[u8], off: Option<usize>) -> T {
    let mut v = T::zeroed();
    if let Some(o) = off {
        let n = size_of::<T>();
        let avail = data.len().saturating_sub(o).min(n);
        if avail > 0 {
            bytemuck::bytes_of_mut(&mut v)[..avail].copy_from_slice(&data[o..o + avail]);
        }
    }
    v
}

fn write_bytes_at(data: &mut [u8], off: Option<usize>, bytes: &[u8]) {
    if let Some(o) = off {
        let avail = data.len().saturating_sub(o).min(bytes.len());
        if avail > 0 {
            data[o..o + avail].copy_from_slice(&bytes[..avail]);
        }
    }
}

macro_rules! scalar_accessors {
    ($($get:ident, $set:ident: $t:ty;)*) => {$(
        pub fn $get(&self, name: impl FieldKey) -> $t {
            read_pod(&self.data, self.offset_of(name))
        }
        pub fn $set(&mut self, name: impl FieldKey, v: $t) {
            let o = self.offset_of(name);
            write_bytes_at(&mut self.data, o, bytemuck::bytes_of(&v));
        }
    )*};
}

impl DataBag {
    /// A new instance with zeroed data.
    pub fn new(class: Arc<SchemaClass>) -> Self {
        let data = vec![0; class.data_size as usize];
        Self { class, data, objects: Vec::new() }
    }

    pub fn with_data(class: Arc<SchemaClass>, data: Vec<u8>) -> Self {
        Self { class, data, objects: Vec::new() }
    }

    pub fn class_name(&self) -> u32 {
        self.class.name
    }

    /// `GetOffset`: where a field is in `data`.
    pub fn offset_of(&self, name: impl FieldKey) -> Option<usize> {
        let f = self.class.field(name)?;
        usize::try_from(f.offset).ok()
    }

    scalar_accessors! {
        f32, set_f32: f32;
        vec2, set_vec2: [f32; 2];
        vec3, set_vec3: [f32; 3];
        vec4, set_vec4: [f32; 4];
        matrix3x4, set_matrix3x4: [f32; 12];
        matrix4x4, set_matrix4x4: [f32; 16];
        u8, set_u8: u8;
        u16, set_u16: u16;
        u32, set_u32: u32;
        u64, set_u64: u64;
        i8, set_i8: i8;
        i16, set_i16: i16;
        i32, set_i32: i32;
        i64, set_i64: i64;
        f64, set_f64: f64;
        hash, set_hash: u32;
        fnv_hash, set_fnv_hash: u64;
    }

    /// `GetBoolean`: true only for 1.
    pub fn bool(&self, name: impl FieldKey) -> bool {
        self.u8(name) == 1
    }
    pub fn set_bool(&mut self, name: impl FieldKey, v: bool) {
        self.set_u8(name, v as u8);
    }

    /// The raw bytes of a field of a plain-data type `T`.
    pub fn get_struct<T: Pod>(&self, name: impl FieldKey) -> T {
        read_pod(&self.data, self.offset_of(name))
    }
    pub fn set_struct<T: Pod>(&mut self, name: impl FieldKey, v: &T) {
        let o = self.offset_of(name);
        write_bytes_at(&mut self.data, o, bytemuck::bytes_of(v));
    }

    /// `GetEmbeddedArray`: the items of an array embedded in the struct, `None`
    /// for other fields.
    pub fn embedded_array<T: Pod>(&self, name: impl FieldKey) -> Option<Vec<T>> {
        let f = self.class.field(name)?;
        if f.data_type != ValueType::Array || f.array_size == 0 {
            return None;
        }
        let base = usize::try_from(f.offset).ok()?;
        let n = size_of::<T>();
        Some((0..f.array_size as usize).map(|i| read_pod(&self.data, Some(base + i * n))).collect())
    }

    /// `SetEmbeddedArray`: written only if the items fit in the array.
    pub fn set_embedded_array<T: Pod>(&mut self, name: impl FieldKey, items: &[T]) {
        let Some(f) = self.class.field(name) else { return };
        if f.data_type != ValueType::Array || f.array_size == 0 || (f.array_size as usize) < items.len() {
            return;
        }
        let o = usize::try_from(f.offset).ok();
        write_bytes_at(&mut self.data, o, bytemuck::cast_slice(items));
    }

    /// The values stored outside the struct, by field name hash.
    pub fn objects(&self) -> impl Iterator<Item = (u32, &Value)> {
        self.objects.iter().map(|(k, v)| (*k, v))
    }

    pub fn object(&self, name: impl FieldKey) -> Option<&Value> {
        let k = name.key();
        self.objects.iter().find(|(n, _)| *n == k).map(|(_, v)| v)
    }

    pub fn object_mut(&mut self, name: impl FieldKey) -> Option<&mut Value> {
        let k = name.key();
        self.objects.iter_mut().find(|(n, _)| *n == k).map(|(_, v)| v)
    }

    /// `SetObject`: `None` removes the value (as a C# null).
    pub fn set_object(&mut self, name: impl FieldKey, v: Option<Value>) {
        let k = name.key();
        match (self.objects.iter().position(|(n, _)| *n == k), v) {
            (Some(i), Some(v)) => self.objects[i].1 = v,
            (Some(i), None) => {
                self.objects.remove(i);
            }
            (None, Some(v)) => self.objects.push((k, v)),
            (None, None) => {}
        }
    }

    pub fn take_object(&mut self, name: impl FieldKey) -> Option<Value> {
        let k = name.key();
        let i = self.objects.iter().position(|(n, _)| *n == k)?;
        Some(self.objects.remove(i).1)
    }

    /// A string field's value.
    pub fn string(&self, name: impl FieldKey) -> Option<&str> {
        match self.object(name) {
            Some(Value::String(s)) => Some(s),
            _ => None,
        }
    }
    pub fn set_string(&mut self, name: impl FieldKey, s: impl Into<String>) {
        self.set_object(name, Some(Value::String(s.into())));
    }

    /// A pointed-to or embedded object.
    pub fn bag(&self, name: impl FieldKey) -> Option<&DataBag> {
        match self.object(name) {
            Some(Value::Bag(b)) => Some(b),
            _ => None,
        }
    }
    pub fn bag_mut(&mut self, name: impl FieldKey) -> Option<&mut DataBag> {
        match self.object_mut(name) {
            Some(Value::Bag(b)) => Some(b),
            _ => None,
        }
    }
    pub fn set_bag(&mut self, name: impl FieldKey, bag: Option<DataBag>) {
        self.set_object(name, bag.map(|b| Value::Bag(Box::new(b))));
    }

    /// An array of objects (entities, archetypes, extensions...).
    pub fn bags(&self, name: impl FieldKey) -> Option<&[Option<DataBag>]> {
        match self.object(name) {
            Some(Value::Bags(b)) => Some(b),
            _ => None,
        }
    }
    pub fn bags_mut(&mut self, name: impl FieldKey) -> Option<&mut Vec<Option<DataBag>>> {
        match self.object_mut(name) {
            Some(Value::Bags(b)) => Some(b),
            _ => None,
        }
    }

    /// A hash array stored outside the struct.
    pub fn hashes(&self, name: impl FieldKey) -> Option<&[u32]> {
        match self.object(name) {
            Some(Value::Hashes(h)) => Some(h),
            _ => None,
        }
    }

    /// An enum field's raw value, sign-extended as the C# reads it.
    pub fn enum_value(&self, name: impl FieldKey) -> i64 {
        let k = name.key();
        match self.class.field(k).map(|f| f.data_type) {
            Some(ValueType::Enum8) => self.i8(k) as i64,
            Some(ValueType::Enum16) => self.i16(k) as i64,
            Some(ValueType::Enum32 | ValueType::Flags32) => self.i32(k) as i64,
            _ => 0,
        }
    }

    /// Sets an enum (or flags) field's raw value, at the field's width.
    pub fn set_enum_value(&mut self, name: impl FieldKey, v: i64) {
        let k = name.key();
        match self.class.field(k).map(|f| f.data_type) {
            Some(ValueType::Enum8) => self.set_i8(k, v as i8),
            Some(ValueType::Enum16) => self.set_i16(k, v as i16),
            Some(ValueType::Enum32 | ValueType::Flags32) => self.set_i32(k, v as i32),
            _ => {}
        }
    }

    /// `GetString`: a string field, or an enum field's value names.
    pub fn enum_string(&self, name: impl FieldKey, schema: Option<&Schema>, hashes: &HashIndex) -> Option<String> {
        let k = name.key();
        if let Some(o) = self.object(k) {
            return match o {
                Value::String(s) => Some(s.clone()),
                _ => None,
            };
        }
        let f = self.class.field(k)?;
        if !f.data_type.is_enum() {
            return None;
        }
        Some(format_enum_value(f, self.enum_value(k), schema, &mut |h| hashes.xml_name(h)))
    }

    /// Sets an enum field from value names, as when read from XML.
    pub fn set_enum_string(&mut self, name: impl FieldKey, s: &str, schema: Option<&Schema>) {
        let k = name.key();
        let Some(f) = self.class.field(k).cloned() else { return };
        let v = parse_enum_value(&f, s, schema);
        match f.data_type {
            ValueType::Enum8 => self.set_u8(k, v as u8),
            ValueType::Enum16 => self.set_u16(k, v as u16),
            ValueType::Enum32 | ValueType::Flags32 => self.set_u32(k, v as u32),
            _ => {}
        }
    }

    /// `ToXml`: the whole document, with this bag as the root element.
    pub fn to_xml(&self, hashes: &HashIndex, schema: Option<&Schema>) -> String {
        let mut w = XmlOut { sb: String::new(), hashes, schema, names: HashMap::new() };
        w.sb.push_str(XML_HEADER);
        w.sb.push('\n');
        let name = w.name(self.class.name);
        w.open_tag(0, &name);
        w.bag(self, 1);
        w.close_tag(0, &name);
        w.sb
    }

    /// `ToXml(sb, indent)`: the bag's fields as XML elements.
    pub fn write_xml(&self, out: &mut String, indent: usize, hashes: &HashIndex, schema: Option<&Schema>) {
        let mut w = XmlOut { sb: std::mem::take(out), hashes, schema, names: HashMap::new() };
        w.bag(self, indent);
        *out = w.sb;
    }

    /// `DataBag2.FromXml`: reads a document into bags of the schema's classes, or
    /// with classes made up from the XML when there's no schema. `None` when the
    /// XML doesn't parse or its root class isn't in the schema.
    pub fn from_xml(xml: &str, schema: Option<&Schema>, hashes: &HashIndex) -> Result<Option<DataBag>> {
        let text = xml.strip_prefix('\u{feff}').unwrap_or(xml);
        let opt = || roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
        let doc = match roxmltree::Document::parse_with_options(text, opt()) {
            Ok(d) => d,
            Err(_) => {
                // TryLoadXmlDocument retries with the header in front.
                let t = text.trim();
                let retry = if t.len() >= XML_HEADER.len() && t[..XML_HEADER.len()].eq_ignore_ascii_case(XML_HEADER) {
                    t.to_owned()
                } else {
                    format!("{XML_HEADER}\r\n{t}")
                };
                match roxmltree::Document::parse_with_options(&retry, opt()) {
                    Ok(d) => return FromXml::new(schema, hashes).document(d.root_element()),
                    Err(_) => return Ok(None),
                }
            }
        };
        FromXml::new(schema, hashes).document(doc.root_element())
    }
}

// ---------------------------------------------------------------------------
// Enums

/// `FormatEnumValue`: an enum value as its name, or flags as names joined by
/// ", ", or the number when the enum isn't known.
pub fn format_enum_value(
    f: &SchemaField,
    v: i64,
    schema: Option<&Schema>,
    name_of: &mut dyn FnMut(u32) -> String,
) -> String {
    let e = schema.and_then(|s| s.enums.get(&f.type_name));
    if let Some(values) = e.and_then(|e| e.values.as_ref()) {
        if f.data_type == ValueType::Flags32 {
            let mut sb = String::new();
            for ev in values {
                let m = 1i32.wrapping_shl(ev.value as i32 as u32) as i64;
                if (v & m) > 0 {
                    if !sb.is_empty() {
                        sb.push_str(", ");
                    }
                    sb.push_str(&name_of(ev.name));
                }
            }
            return sb;
        }
        if let Some(ev) = values.iter().find(|ev| ev.value == v) {
            return name_of(ev.name);
        }
        let iv = v as u32 as i64;
        if let Some(ev) = values.iter().find(|ev| ev.value == iv) {
            return name_of(ev.name);
        }
        return v.to_string();
    }
    if v == 0 {
        return String::new();
    }
    format!("0x{:08X}", v as u64)
}

/// `long.TryParse`, 0 when it fails.
fn parse_long(s: &str) -> i64 {
    let t = s.trim();
    t.strip_prefix('+').unwrap_or(t).parse().unwrap_or(0)
}

/// `ParseEnumValue`: the reverse of [`format_enum_value`].
pub fn parse_enum_value(f: &SchemaField, s: &str, schema: Option<&Schema>) -> i64 {
    let e = schema.and_then(|s| s.enums.get(&f.type_name));
    let Some(values) = e.and_then(|e| e.values.as_ref()) else {
        return parse_long(s);
    };
    if f.data_type == ValueType::Flags32 {
        let mut v = 0i64;
        for part in s.split(',') {
            let t = part.trim();
            if t.is_empty() {
                continue;
            }
            let h = parse_hash(t);
            if let Some(ev) = values.iter().find(|ev| ev.name == h) {
                v |= 1i64.wrapping_shl(ev.value as i32 as u32);
            }
        }
        return v;
    }
    let h = parse_hash(s);
    match values.iter().find(|ev| ev.name == h) {
        Some(ev) => ev.value,
        None => parse_long(s),
    }
}

/// `FnvHash.GenHash`.
pub fn fnv_hash(text: &str) -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    for c in text.encode_utf16() {
        h = (h ^ c as u64).wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

/// `Encoding.ASCII.GetString`: bytes above 0x7F become `?`.
pub fn ascii_string(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| if b < 0x80 { b as char } else { '?' }).collect()
}

/// `Encoding.ASCII.GetBytes`: characters above 0x7F become `?`.
pub fn ascii_bytes(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        let n = c.len_utf16();
        let b = if (c as u32) < 0x80 { c as u8 } else { b'?' };
        // A character outside the BMP is two UTF-16 units, each replaced.
        for _ in 0..n {
            out.push(b);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// To XML (DataBag2.ToXml and the Xml.cs helpers it uses)

struct XmlOut<'a> {
    sb: String,
    hashes: &'a HashIndex,
    schema: Option<&'a Schema>,
    /// Names of hashes, so unknown ones are only turned into collision names once.
    names: HashMap<u32, String>,
}

/// `Xml.Escape`: the text as `XmlNode.InnerXml` writes it.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "&#x{:X};", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn vec2_xml([x, y]: [f32; 2]) -> String {
    format!("x=\"{}\" y=\"{}\"", f32_to_string(x), f32_to_string(y))
}
fn vec3_xml([x, y, z]: [f32; 3]) -> String {
    format!("x=\"{}\" y=\"{}\" z=\"{}\"", f32_to_string(x), f32_to_string(y), f32_to_string(z))
}
fn vec4_xml([x, y, z, w]: [f32; 4]) -> String {
    format!(
        "x=\"{}\" y=\"{}\" z=\"{}\" w=\"{}\"",
        f32_to_string(x),
        f32_to_string(y),
        f32_to_string(z),
        f32_to_string(w)
    )
}
/// `FloatUtil.GetVectorNString`: "x, y, ...".
fn push_floats(s: &mut String, v: &[f32]) {
    for (i, f) in v.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        push_f32(s, *f);
    }
}
fn push_hex_u64(s: &mut String, v: u64) {
    let _ = write!(s, "0x{v:016X}");
}
fn push_display<T: std::fmt::Display>(s: &mut String, v: &T) {
    let _ = write!(s, "{v}");
}
fn bool_str(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

impl XmlOut<'_> {
    /// `JenkHash.ToString()`: the name, a collision name, or "" for 0.
    fn name(&mut self, h: u32) -> String {
        if h == 0 {
            return String::new();
        }
        let hashes = self.hashes;
        self.names.entry(h).or_insert_with(|| hashes.xml_name(h)).clone()
    }

    fn indent(&mut self, n: usize) {
        self.sb.extend(std::iter::repeat_n(' ', n));
    }
    fn open_tag_nl(&mut self, ind: usize, name: &str, nl: bool) {
        self.indent(ind);
        self.sb.push('<');
        self.sb.push_str(name);
        self.sb.push('>');
        if nl {
            self.sb.push('\n');
        }
    }
    fn open_tag(&mut self, ind: usize, name: &str) {
        self.open_tag_nl(ind, name, true);
    }
    fn close_tag(&mut self, ind: usize, name: &str) {
        self.indent(ind);
        self.sb.push_str("</");
        self.sb.push_str(name);
        self.sb.push_str(">\n");
    }
    fn value_tag(&mut self, ind: usize, name: &str, val: &str) {
        self.indent(ind);
        self.sb.push('<');
        self.sb.push_str(name);
        self.sb.push_str(" value=\"");
        self.sb.push_str(val);
        self.sb.push_str("\" />\n");
    }
    fn one_line_tag(&mut self, ind: usize, name: &str, text: &str, attrs: &str) {
        self.indent(ind);
        self.sb.push('<');
        self.sb.push_str(name);
        self.sb.push_str(attrs);
        self.sb.push('>');
        self.sb.push_str(text);
        self.sb.push_str("</");
        self.sb.push_str(name);
        self.sb.push_str(">\n");
    }
    fn self_closing(&mut self, ind: usize, val: &str) {
        self.indent(ind);
        self.sb.push('<');
        self.sb.push_str(val);
        self.sb.push_str(" />\n");
    }
    /// `Xml.StringTag`.
    fn string_tag(&mut self, ind: usize, name: &str, text: Option<&str>, attrs: &str) {
        match text {
            Some(t) if !t.is_empty() => {
                if t.ends_with('\n') {
                    self.open_tag(ind, name);
                    self.sb.push_str(t);
                    self.close_tag(ind, name);
                } else {
                    self.one_line_tag(ind, name, t, attrs);
                }
            }
            _ => self.self_closing(ind, &format!("{name}{attrs}")),
        }
    }
    /// `Xml.WriteRawArray`.
    fn raw_array<T>(
        &mut self,
        ind: usize,
        name: &str,
        arr: Option<&[T]>,
        mut fmt: impl FnMut(&mut String, &T),
        row: usize,
        attrs: &str,
    ) {
        let arr = arr.unwrap_or(&[]);
        let tag = format!("{name}{attrs}");
        if arr.is_empty() {
            self.self_closing(ind, &tag);
            return;
        }
        if arr.len() <= row {
            self.open_tag_nl(ind, &tag, false);
            for (n, v) in arr.iter().enumerate() {
                if n > 0 {
                    self.sb.push(' ');
                }
                fmt(&mut self.sb, v);
            }
            self.close_tag(0, name);
        } else {
            self.open_tag(ind, &tag);
            let last = arr.len() - 1;
            for (n, v) in arr.iter().enumerate() {
                let col = n % row;
                if col == 0 {
                    self.indent(ind + 1);
                } else {
                    self.sb.push(' ');
                }
                fmt(&mut self.sb, v);
                if col == row - 1 || n == last {
                    self.sb.push('\n');
                }
            }
            self.close_tag(ind, name);
        }
    }
    fn display_array<T: std::fmt::Display>(&mut self, ind: usize, name: &str, arr: Option<&[T]>, row: usize) {
        self.raw_array(ind, name, arr, push_display, row, "");
    }
    /// `Xml.WriteHashItemArray`.
    fn hash_item_array(&mut self, ind: usize, name: &str, arr: Option<&[u32]>) {
        match arr {
            Some(a) if !a.is_empty() => {
                self.open_tag(ind, name);
                for h in a {
                    let s = self.name(*h);
                    self.string_tag(ind + 1, "Item", Some(&s), "");
                }
                self.close_tag(ind, name);
            }
            _ => self.self_closing(ind, name),
        }
    }
    /// `Xml.WriteStringItemArray`.
    fn string_item_array(&mut self, ind: usize, name: &str, arr: &[String]) {
        if arr.is_empty() {
            self.self_closing(ind, name);
            return;
        }
        self.open_tag(ind, name);
        for s in arr {
            self.string_tag(ind + 1, "Item", Some(s), "");
        }
        self.close_tag(ind, name);
    }

    fn format_enum(&mut self, f: &SchemaField, v: i64) -> String {
        let schema = self.schema;
        format_enum_value(f, v, schema, &mut |h| self.name(h))
    }

    /// The `" name=\"value\""` text of an attribute field.
    fn attribute(&mut self, bag: &DataBag, f: &SchemaField) -> String {
        let n = f.name;
        let val = match f.data_type {
            ValueType::Float => f32_to_string(bag.f32(n)),
            ValueType::String => bag.string(n).unwrap_or("").to_owned(),
            ValueType::UInt32 => bag.u32(n).to_string(),
            ValueType::Boolean => bool_str(bag.bool(n)).to_owned(),
            _ => String::new(),
        };
        format!(" {}=\"{}\"", self.name(n), val)
    }

    /// Items of an array of objects, for `Struct`/`Object` arrays.
    fn bag_items(&mut self, ind: usize, fs: &str, field: &SchemaField, arr: &[Option<DataBag>]) {
        let usefn = field.type_name != 0;
        let atyp = if usefn { format!(" itemType=\"{}\"", self.name(field.type_name)) } else { String::new() };
        self.open_tag(ind, &format!("{fs}{atyp}"));
        for item in arr {
            match item {
                Some(item) => {
                    let styp = if usefn { String::new() } else { format!(" type=\"{}\"", self.name(item.class.name)) };
                    self.open_tag(ind + 1, &format!("Item{styp}"));
                    self.bag(item, ind + 2);
                    self.close_tag(ind + 1, "Item");
                }
                None => self.self_closing(ind + 1, "Item"),
            }
        }
        self.close_tag(ind, fs);
    }

    fn bag(&mut self, bag: &DataBag, ind: usize) {
        let Some(fields) = bag.class.fields.as_deref() else { return };
        for field in fields {
            if field.is_attribute {
                continue;
            }
            let f = field.name;
            let fs = self.name(f);
            match field.data_type {
                ValueType::None => {
                    let v = match bag.object(f) {
                        Some(Value::String(s)) => s.clone(),
                        _ => String::new(),
                    };
                    self.value_tag(ind, &fs, &v)
                }
                ValueType::Float => self.value_tag(ind, &fs, &f32_to_string(bag.f32(f))),
                ValueType::Float2 => self.self_closing(ind, &format!("{fs} {}", vec2_xml(bag.vec2(f)))),
                ValueType::Float3 => self.self_closing(ind, &format!("{fs} {}", vec3_xml(bag.vec3(f)))),
                ValueType::Float4 => self.self_closing(ind, &format!("{fs} {}", vec4_xml(bag.vec4(f)))),
                ValueType::Float3x4 => {
                    self.raw_array(ind, &fs, Some(&bag.matrix3x4(f)[..]), |s, v| push_f32(s, *v), 4, "")
                }
                ValueType::Float4x4 => {
                    self.raw_array(ind, &fs, Some(&bag.matrix4x4(f)[..]), |s, v| push_f32(s, *v), 4, "")
                }
                ValueType::UInt8 => self.value_tag(ind, &fs, &bag.u8(f).to_string()),
                ValueType::UInt16 => self.value_tag(ind, &fs, &bag.u16(f).to_string()),
                ValueType::UInt32 => self.value_tag(ind, &fs, &bag.u32(f).to_string()),
                ValueType::UInt64 => self.value_tag(ind, &fs, &format!("0x{:016X}", bag.u64(f))),
                ValueType::Int8 => self.value_tag(ind, &fs, &bag.i8(f).to_string()),
                ValueType::Int16 => self.value_tag(ind, &fs, &bag.i16(f).to_string()),
                ValueType::Int32 => self.value_tag(ind, &fs, &bag.i32(f).to_string()),
                ValueType::Int64 => self.value_tag(ind, &fs, &bag.i64(f).to_string()),
                ValueType::Enum8 => {
                    let s = self.format_enum(field, bag.i8(f) as i64);
                    self.string_tag(ind, &fs, Some(&s), "")
                }
                ValueType::Enum16 => {
                    let s = self.format_enum(field, bag.i16(f) as i64);
                    self.string_tag(ind, &fs, Some(&s), "")
                }
                ValueType::Enum32 => {
                    let s = self.format_enum(field, bag.i32(f) as i64);
                    self.string_tag(ind, &fs, Some(&s), "")
                }
                ValueType::Flags32 => {
                    let s = self.format_enum(field, bag.u32(f) as i64);
                    self.string_tag(ind, &fs, Some(&s), "")
                }
                ValueType::Double => self.value_tag(ind, &fs, &f64_to_string(bag.f64(f))),
                ValueType::Boolean => self.value_tag(ind, &fs, bool_str(bag.bool(f))),
                ValueType::Hash => {
                    let s = self.name(bag.hash(f));
                    self.string_tag(ind, &fs, Some(&s), "")
                }
                ValueType::FnvHash => {
                    let h = bag.fnv_hash(f);
                    let s = if h == 0 || h == 0xCBF2_9CE4_8422_2325 { String::new() } else { format!("0x{h:016X}") };
                    self.string_tag(ind, &fs, Some(&s), "")
                }
                ValueType::Char => {
                    let c = bag.u8(f) as char;
                    self.value_tag(ind, &fs, &c.to_string())
                }
                ValueType::String => {
                    let s = bag.string(f).map(escape);
                    self.string_tag(ind, &fs, s.as_deref(), "")
                }
                ValueType::Array => {
                    if field.array_size != 0 {
                        self.embedded_array(bag, field, &fs, ind)
                    } else {
                        self.pointer_array(bag, field, &fs, ind)
                    }
                }
                ValueType::Object => self.object(bag, field, &fs, ind),
                ValueType::Struct => match bag.bag(f) {
                    Some(sobj) => {
                        let t = self.name(sobj.class.name);
                        self.open_tag(ind, &format!("{fs} type=\"{t}\""));
                        self.bag(sobj, ind + 1);
                        self.close_tag(ind, &fs);
                    }
                    None => self.self_closing(ind, &fs),
                },
                ValueType::Lookup => self.lookup(bag, field, &fs, ind),
            }
        }
    }

    fn embedded_array(&mut self, bag: &DataBag, field: &SchemaField, fs: &str, ind: usize) {
        let f = field.name;
        let pf = |s: &mut String, v: &f32| push_f32(s, *v);
        match field.array_type {
            ValueType::Float => self.raw_array(ind, fs, bag.embedded_array::<f32>(f).as_deref(), pf, 10, ""),
            ValueType::Float2 => {
                self.raw_array(ind, fs, bag.embedded_array::<[f32; 2]>(f).as_deref(), |s, v| push_floats(s, v), 1, "")
            }
            ValueType::Float3 => {
                self.raw_array(ind, fs, bag.embedded_array::<[f32; 3]>(f).as_deref(), |s, v| push_floats(s, v), 1, "")
            }
            ValueType::Float4 => {
                self.raw_array(ind, fs, bag.embedded_array::<[f32; 4]>(f).as_deref(), |s, v| push_floats(s, v), 1, "")
            }
            ValueType::UInt8 | ValueType::Enum8 | ValueType::Boolean => {
                self.display_array(ind, fs, bag.embedded_array::<u8>(f).as_deref(), 16)
            }
            ValueType::UInt16 | ValueType::Enum16 => {
                self.display_array(ind, fs, bag.embedded_array::<u16>(f).as_deref(), 10)
            }
            ValueType::UInt32 | ValueType::Enum32 | ValueType::Flags32 => {
                self.display_array(ind, fs, bag.embedded_array::<u32>(f).as_deref(), 10)
            }
            ValueType::UInt64 => {
                self.raw_array(ind, fs, bag.embedded_array::<u64>(f).as_deref(), |s, v| push_hex_u64(s, *v), 1, "")
            }
            ValueType::Int8 => self.display_array(ind, fs, bag.embedded_array::<i8>(f).as_deref(), 16),
            ValueType::Int16 => self.display_array(ind, fs, bag.embedded_array::<i16>(f).as_deref(), 10),
            ValueType::Int32 => self.display_array(ind, fs, bag.embedded_array::<i32>(f).as_deref(), 10),
            ValueType::Int64 => self.display_array(ind, fs, bag.embedded_array::<i64>(f).as_deref(), 1),
            ValueType::Hash => self.hash_item_array(ind, fs, bag.embedded_array::<u32>(f).as_deref()),
            ValueType::Char => {
                let b = bag.embedded_array::<u8>(f).unwrap_or_default();
                let s = ascii_string(&b).replace('\0', "");
                self.string_tag(ind, fs, Some(&s), "")
            }
            ValueType::Struct | ValueType::Object => match bag.object(f) {
                Some(Value::Bags(arr)) if !arr.is_empty() => self.bag_items(ind, fs, field, arr),
                _ => self.self_closing(ind, fs),
            },
            ValueType::Array => {
                if field.type_name == ValueType::Hash as u32 {
                    match bag.object(f) {
                        Some(Value::HashArrays(arrarr)) if !arrarr.is_empty() => {
                            self.open_tag(ind, &format!("{fs} itemType=\"Hash\""));
                            for arr2 in arrarr {
                                match arr2 {
                                    Some(a) if !a.is_empty() => {
                                        self.open_tag(ind + 1, "Item");
                                        for h in a {
                                            let s = self.name(*h);
                                            self.string_tag(ind + 2, "Item", Some(&s), "");
                                        }
                                        self.close_tag(ind + 1, "Item");
                                    }
                                    _ => self.self_closing(ind + 1, "Item"),
                                }
                            }
                            self.close_tag(ind, fs);
                        }
                        _ => self.self_closing(ind, fs),
                    }
                } else {
                    match bag.object(f) {
                        Some(Value::BagArrays(arrarr)) if !arrarr.is_empty() => {
                            let usefn = field.type_name != 0;
                            let atyp = if usefn {
                                format!(" itemType=\"{}\"", self.name(field.type_name))
                            } else {
                                String::new()
                            };
                            self.open_tag(ind, &format!("{fs}{atyp}"));
                            for arr2 in arrarr {
                                match arr2 {
                                    Some(a) if !a.is_empty() => {
                                        self.open_tag(ind + 1, "Item");
                                        for item in a {
                                            match item {
                                                Some(item) => {
                                                    let styp = if usefn {
                                                        String::new()
                                                    } else {
                                                        format!(" type=\"{}\"", self.name(item.class.name))
                                                    };
                                                    self.open_tag(ind + 2, &format!("Item{styp}"));
                                                    self.bag(item, ind + 3);
                                                    self.close_tag(ind + 2, "Item");
                                                }
                                                None => self.self_closing(ind + 2, "Item"),
                                            }
                                        }
                                        self.close_tag(ind + 1, "Item");
                                    }
                                    _ => self.self_closing(ind + 1, "Item"),
                                }
                            }
                            self.close_tag(ind, fs);
                        }
                        _ => self.self_closing(ind, fs),
                    }
                }
            }
            ValueType::String => match bag.object(f) {
                Some(Value::Strings(a)) if !a.is_empty() => {
                    self.open_tag(ind, fs);
                    for s in a {
                        self.string_tag(ind + 1, "Item", Some(s), "");
                    }
                    self.close_tag(ind, fs);
                }
                _ => self.self_closing(ind, fs),
            },
            _ => self.self_closing(ind, fs),
        }
    }

    fn pointer_array(&mut self, bag: &DataBag, field: &SchemaField, fs: &str, ind: usize) {
        let pf = |s: &mut String, v: &f32| push_f32(s, *v);
        match bag.object(field.name) {
            None => self.self_closing(ind, fs),
            Some(Value::Bags(tarr)) if !tarr.is_empty() => {
                let usefn = field.type_name != 0;
                let atyp = if usefn { format!(" itemType=\"{}\"", self.name(field.type_name)) } else { String::new() };
                self.open_tag(ind, &format!("{fs}{atyp}"));
                for item in tarr {
                    let Some(item) = item else {
                        self.self_closing(ind + 1, "Item");
                        continue;
                    };
                    let clsname = self.name(item.class.name);
                    let usecn = !usefn && !clsname.is_empty() && clsname != "Item";
                    let styp = if usecn { format!(" type=\"{clsname}\"") } else { String::new() };
                    let mut onlyattrs = false;
                    let mut attrstr = String::new();
                    let ifields = item.class.fields();
                    if !ifields.is_empty() {
                        onlyattrs = true;
                        for tfld in ifields {
                            if tfld.name == TYPE_HASH {
                                continue;
                            }
                            if !tfld.is_attribute {
                                onlyattrs = false;
                            } else {
                                attrstr.push_str(&self.attribute(item, tfld));
                            }
                        }
                    }
                    if onlyattrs {
                        self.self_closing(ind + 1, &format!("Item{styp}{attrstr}"));
                    } else {
                        self.open_tag(ind + 1, &format!("Item{styp}{attrstr}"));
                        self.bag(item, ind + 2);
                        self.close_tag(ind + 1, "Item");
                    }
                }
                self.close_tag(ind, fs);
            }
            Some(Value::Hashes(h)) if !h.is_empty() => self.hash_item_array(ind, fs, Some(h)),
            Some(Value::Strings(s)) if !s.is_empty() => self.string_item_array(ind, fs, s),
            // The CLR's array covariance lets `is byte[]` match an sbyte[], and so on.
            Some(Value::Bytes(b)) if !b.is_empty() => self.display_array(ind, fs, Some(b), 16),
            Some(Value::SBytes(b)) if !b.is_empty() => {
                let b: Vec<u8> = b.iter().map(|v| *v as u8).collect();
                self.display_array(ind, fs, Some(&b), 16)
            }
            Some(Value::U16s(a)) if !a.is_empty() => self.display_array(ind, fs, Some(a), 10),
            Some(Value::I16s(a)) if !a.is_empty() => {
                let a: Vec<u16> = a.iter().map(|v| *v as u16).collect();
                self.display_array(ind, fs, Some(&a), 10)
            }
            Some(Value::U32s(a)) if !a.is_empty() => self.display_array(ind, fs, Some(a), 10),
            Some(Value::I32s(a)) if !a.is_empty() => {
                let a: Vec<u32> = a.iter().map(|v| *v as u32).collect();
                self.display_array(ind, fs, Some(&a), 10)
            }
            Some(Value::U64s(a)) if !a.is_empty() => {
                self.raw_array(ind, fs, Some(a), |s, v| push_hex_u64(s, *v), 1, "")
            }
            Some(Value::I64s(a)) if !a.is_empty() => {
                self.raw_array(ind, fs, Some(a), |s, v| push_hex_u64(s, *v as u64), 1, "")
            }
            Some(Value::F32s(a)) if !a.is_empty() => self.raw_array(ind, fs, Some(a), pf, 10, ""),
            Some(Value::Vec2s(a)) if !a.is_empty() => self.raw_array(ind, fs, Some(a), |s, v| push_floats(s, v), 1, ""),
            Some(Value::Vec4s(a)) if !a.is_empty() => self.raw_array(ind, fs, Some(a), |s, v| push_floats(s, v), 1, ""),
            Some(Value::Bools(a)) if !a.is_empty() => {
                self.raw_array(ind, fs, Some(a), |s, v| s.push_str(bool_str(*v)), 10, "")
            }
            Some(Value::KeyValues(kvs)) if !kvs.is_empty() => {
                let usefn = field.type_name != 0;
                let atyp = if usefn { format!(" itemType=\"{}\"", self.name(field.type_name)) } else { String::new() };
                self.open_tag(ind, &format!("{fs}{atyp}"));
                for (k, v) in kvs {
                    match v {
                        Some(v) => {
                            self.open_tag(ind + 1, &format!("Item key=\"{k}\""));
                            self.bag(v, ind + 2);
                            self.close_tag(ind + 1, "Item");
                        }
                        None => self.string_tag(ind + 1, "Item", Some(&escape(k)), ""),
                    }
                }
                self.close_tag(ind, fs);
            }
            Some(_) => self.self_closing(ind, fs),
        }
    }

    fn object(&mut self, bag: &DataBag, field: &SchemaField, fs: &str, ind: usize) {
        if field.is_link {
            self.one_line_tag(ind, fs, "[Link]", "");
            return;
        }
        match bag.object(field.name) {
            Some(Value::Bytes(b)) if !b.is_empty() => {
                self.raw_array(ind, fs, Some(b), |s, v| push_display(s, &format_args!("{v:02X}")), 32, "")
            }
            Some(Value::Bag(obag)) => {
                let ofields = obag.class.fields();
                let mut attrcount = 0;
                let mut allattrs = false;
                if obag.class.has_fields() {
                    attrcount = ofields.iter().take_while(|f| f.is_attribute).count();
                    allattrs = attrcount == ofields.len();
                }
                let mut attrstr = String::new();
                for attrfld in &ofields[..attrcount] {
                    if attrfld.name == TYPE_HASH {
                        continue;
                    }
                    attrstr.push_str(&self.attribute(obag, attrfld));
                }
                if allattrs {
                    self.self_closing(ind, &format!("{fs}{attrstr}"));
                } else {
                    let t = self.name(obag.class.name);
                    self.open_tag(ind, &format!("{fs}{attrstr} type=\"{t}\""));
                    self.bag(obag, ind + 1);
                    self.close_tag(ind, fs);
                }
            }
            Some(Value::String(s)) => self.string_tag(ind, fs, Some(s), ""),
            Some(Value::KeyValue(k, v)) => match v {
                Some(v) => {
                    self.open_tag(ind, &format!("{fs} key=\"{k}\""));
                    self.bag(v, ind + 1);
                    self.close_tag(ind, fs);
                }
                None => self.string_tag(ind, fs, Some(&escape(k)), ""),
            },
            _ => self.self_closing(ind, fs),
        }
    }

    fn lookup(&mut self, bag: &DataBag, field: &SchemaField, fs: &str, ind: usize) {
        let Some(Value::Bags(larr)) = bag.object(field.name) else {
            self.self_closing(ind, fs);
            return;
        };
        self.open_tag(ind, fs);
        for kvp in larr {
            let Some(kvp) = kvp else {
                self.self_closing(ind + 1, "Item");
                continue;
            };
            let (kstr, item) = self.key_value_pair(kvp);
            let skey = format!(" key=\"{kstr}\"");
            match item {
                KvItem::None => self.self_closing(ind + 1, &format!("Item{skey}")),
                KvItem::Bag(ibag) => {
                    let styp = format!(" type=\"{}\"", self.name(ibag.class.name));
                    if !ibag.class.fields().is_empty() {
                        self.open_tag(ind + 1, &format!("Item{styp}{skey}"));
                        self.bag(ibag, ind + 2);
                        self.close_tag(ind + 1, "Item");
                    } else {
                        self.self_closing(ind + 1, &format!("Item{styp}{skey}"));
                    }
                }
                KvItem::Bags(iarr) => {
                    let styp = format!(" type=\"Array\" itemType=\"{}\"", self.name(field.type_name));
                    self.open_tag(ind + 1, &format!("Item{skey}{styp}"));
                    for aitem in iarr {
                        match aitem {
                            Some(a) if !a.class.fields().is_empty() => {
                                self.open_tag(ind + 2, "Item");
                                self.bag(a, ind + 3);
                                self.close_tag(ind + 2, "Item");
                            }
                            _ => self.self_closing(ind + 2, "Item"),
                        }
                    }
                    self.close_tag(ind + 1, "Item");
                }
                KvItem::ULongs(a) => self.raw_array(
                    ind + 1,
                    "Item",
                    Some(a),
                    |s, v| push_hex_u64(s, *v),
                    1,
                    &format!(" type=\"ULongs\"{skey}"),
                ),
                KvItem::Str(s) => self.string_tag(ind + 1, "Item", Some(&s), &format!(" type=\"String\"{skey}")),
                KvItem::UInt(u) => {
                    self.string_tag(ind + 1, "Item", Some(&u.to_string()), &format!(" type=\"Uint\"{skey}"))
                }
                KvItem::Float(f) => {
                    self.string_tag(ind + 1, "Item", Some(&f32_to_string(f)), &format!(" type=\"Float\"{skey}"))
                }
                KvItem::Float4(v) => self.self_closing(ind + 1, &format!("Item type=\"Float4\"{skey} {}", vec4_xml(v))),
                KvItem::Other => {}
            }
        }
        self.close_tag(ind, fs);
    }

    /// `GetKeyValuePair`.
    fn key_value_pair<'b>(&mut self, kvp: &'b DataBag) -> (String, KvItem<'b>) {
        let cls = &kvp.class;
        let (mut khash, mut kfld) = (KEY_HASH, cls.field(KEY_HASH));
        if kfld.is_none() {
            khash = KEY_LOWER_HASH;
            kfld = cls.field(khash);
        }
        let (mut ihash, mut ifld) = (ITEM_HASH, cls.field(ITEM_HASH));
        if ifld.is_none() {
            ihash = ITEM_LOWER_HASH;
            ifld = cls.field(ihash);
        }
        let key = match kfld.map(|f| f.data_type) {
            Some(ValueType::Hash) => self.name(kvp.hash(khash)),
            Some(ValueType::String) => kvp.string(khash).unwrap_or("").to_owned(),
            Some(ValueType::Int16) => kvp.i16(khash).to_string(),
            Some(ValueType::UInt32 | ValueType::Enum32) => kvp.u32(khash).to_string(),
            _ => String::new(),
        };
        let object = |o: Option<&'b Value>| match o {
            None => KvItem::None,
            Some(Value::Bag(b)) => KvItem::Bag(b),
            Some(Value::Bags(b)) => KvItem::Bags(b),
            Some(Value::U64s(u)) => KvItem::ULongs(u),
            Some(Value::String(s)) => KvItem::Str(s.clone()),
            Some(_) => KvItem::Other,
        };
        let item = match ifld.map(|f| f.data_type) {
            None => KvItem::None,
            Some(ValueType::Hash) => KvItem::Str(self.name(kvp.hash(ihash))),
            Some(ValueType::UInt32 | ValueType::Enum32 | ValueType::Flags32) => KvItem::UInt(kvp.u32(ihash)),
            Some(ValueType::Float) => KvItem::Float(kvp.f32(ihash)),
            Some(ValueType::Float4) => KvItem::Float4(kvp.vec4(ihash)),
            Some(ValueType::String) => match kvp.string(ihash) {
                Some(s) => KvItem::Str(s.to_owned()),
                None => KvItem::None,
            },
            Some(_) => object(kvp.object(ihash)),
        };
        (key, item)
    }
}

/// What a lookup entry's item is, as `GetKeyValuePair` returns it.
enum KvItem<'a> {
    None,
    Bag(&'a DataBag),
    Bags(&'a [Option<DataBag>]),
    ULongs(&'a [u64]),
    Str(String),
    UInt(u32),
    Float(f32),
    Float4([f32; 4]),
    /// A value the XML has no form for: nothing is written.
    Other,
}

// ---------------------------------------------------------------------------
// From XML (DataBag2.FromXml)

/// The child nodes an `XmlDocument` keeps (without whitespace-only text).
enum XChild<'a, 'i> {
    Element(Node<'a, 'i>),
    Text(&'a str),
    Other,
}

fn is_ws(s: &str) -> bool {
    s.chars().all(|c| matches!(c, ' ' | '\t' | '\r' | '\n'))
}

fn child_nodes<'a, 'i>(n: Node<'a, 'i>) -> Vec<XChild<'a, 'i>> {
    let mut v = Vec::new();
    for c in n.children() {
        if c.is_element() {
            v.push(XChild::Element(c));
        } else if c.is_text() {
            let t = c.text().unwrap_or("");
            if !is_ws(t) {
                v.push(XChild::Text(t));
            }
        } else if c.is_comment() || c.is_pi() {
            v.push(XChild::Other);
        }
    }
    v
}

/// `InnerText`, without the whitespace-only text an `XmlDocument` drops.
fn inner_text(n: Node) -> String {
    let mut s = String::new();
    for d in n.descendants() {
        if d.is_text() {
            let t = d.text().unwrap_or("");
            if !is_ws(t) {
                s.push_str(t);
            }
        }
    }
    s
}

/// The full element name (with its prefix), as `XmlElement.Name`.
fn element_name<'i>(n: Node<'_, 'i>) -> String {
    let local = n.tag_name().name();
    match n.tag_name().namespace().and_then(|ns| n.lookup_prefix(ns)) {
        Some(p) if !p.is_empty() => format!("{p}:{local}"),
        _ => local.to_owned(),
    }
}

/// Child elements named `name` (`SelectNodes(name)`).
fn select<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Vec<Node<'a, 'i>> {
    n.children().filter(|c| c.is_element() && c.tag_name().name() == name).collect()
}

/// `SelectNodes("Item")`, or `"item"` when there are none.
fn items<'a, 'i>(n: Node<'a, 'i>) -> Vec<Node<'a, 'i>> {
    let v = select(n, "Item");
    if v.is_empty() { select(n, "item") } else { v }
}

fn split_ws(s: &str) -> impl Iterator<Item = &str> {
    s.split(char::is_whitespace).filter(|t| !t.is_empty())
}

/// `int.TryParse`/`uint.TryParse`... with `NumberStyles.Integer`.
fn try_int<T: std::str::FromStr>(s: &str) -> Option<T> {
    let t = s.trim();
    t.strip_prefix('+').unwrap_or(t).parse().ok()
}

/// `Convert.ToUInt64(s, 16)`: failures are errors.
fn hex_u64(s: &str) -> Result<u64> {
    let t = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    u64::from_str_radix(t, 16).map_err(|e| Error::Format(format!("invalid number \"{s}\": {e}")))
}

/// `Convert.ToByte(s, base)` and friends: failures are errors. Hex numbers are
/// two's complement.
fn convert_int(s: &str, base: u32) -> Result<i64> {
    if base == 16 {
        return hex_u64(s).map(|v| v as i64);
    }
    i64::from_str_radix(s, base).map_err(|e| Error::Format(format!("invalid number \"{s}\": {e}")))
}

fn bool_parse(s: &str) -> Option<bool> {
    let t = s.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    if t.eq_ignore_ascii_case("true") {
        Some(true)
    } else if t.eq_ignore_ascii_case("false") {
        Some(false)
    } else {
        None
    }
}

/// Reads XML into bags (`DataBag2.FromXml` and `CreateSchemaClass`).
struct FromXml<'s> {
    schema: Option<&'s Schema>,
    hashes: &'s HashIndex,
    /// Elements whose `value="1.5f"` `GetFieldType` rewrote to `1.5`.
    stripped_f: HashSet<NodeId>,
}

/// The class a node is read as: its own name, or a type given by name or hash.
#[derive(Clone, Copy)]
enum TypeRef<'t> {
    Node,
    Name(&'t str),
    Hash(u32),
}

impl<'t> TypeRef<'t> {
    /// `itype2 ?? itype` with `itype` taken from an attribute or the field's type.
    fn from_attr(attr: Option<&'t str>) -> Self {
        match attr {
            Some(s) if !s.is_empty() => TypeRef::Name(s),
            _ => TypeRef::Node,
        }
    }
}

impl<'s> FromXml<'s> {
    fn new(schema: Option<&'s Schema>, hashes: &'s HashIndex) -> Self {
        Self { schema, hashes, stripped_f: HashSet::new() }
    }

    fn attr<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
        n.attribute(name)
    }

    /// `Xml.GetStringAttribute(elem, "itemType")` or the field's type name.
    fn item_type<'a>(elem: Node<'a, '_>, attr: &str, fld: &SchemaField) -> TypeRef<'a> {
        match Self::attr(elem, attr) {
            Some(s) if !s.is_empty() => TypeRef::Name(s),
            _ if fld.type_name != 0 => TypeRef::Hash(fld.type_name),
            _ => TypeRef::Node,
        }
    }

    /// `ityp2 ?? ityp`: the item's own `type`, or the array's.
    fn or_type<'a>(own: Option<&'a str>, fallback: TypeRef<'a>) -> TypeRef<'a> {
        match own {
            Some(s) => TypeRef::from_attr(Some(s)),
            None => fallback,
        }
    }

    fn document(mut self, root: Node) -> Result<Option<DataBag>> {
        let ft = self.field_type(root);
        if ft == ValueType::Array {
            // A wrapper object for a root that is an array.
            let name = element_name(root);
            self.hashes.ensure(&name);
            let h = joaat(&name);
            let fld = SchemaField { name: h, data_type: ft, array_type: ValueType::Object, ..Default::default() };
            let cls = Arc::new(SchemaClass::new(h, 0, Some(vec![fld]), ClassInfo::Plain));
            let mut bag = DataBag::new(cls);
            let mut list = Vec::new();
            for c in root.children().filter(|c| c.is_element()) {
                list.push(self.node(c, TypeRef::Node)?);
            }
            bag.set_object(h, Some(Value::Bags(list)));
            return Ok(Some(bag));
        }
        self.node(root, TypeRef::Node)
    }

    fn resolve_class(&mut self, node: Node, ty: TypeRef) -> Option<Arc<SchemaClass>> {
        match self.schema {
            Some(schema) => {
                let h = match ty {
                    TypeRef::Node | TypeRef::Name("") => joaat(&element_name(node)),
                    TypeRef::Name(s) => joaat(s),
                    TypeRef::Hash(h) => h,
                };
                schema.classes.get(&h).cloned()
            }
            None => Some(Arc::new(self.create_class(node, ty))),
        }
    }

    /// `FromXml(node, schema, type)`.
    fn node(&mut self, node: Node, ty: TypeRef) -> Result<Option<DataBag>> {
        let Some(cls) = self.resolve_class(node, ty) else { return Ok(None) };
        let mut bag = DataBag::new(cls.clone());
        if !cls.has_fields() {
            return Ok(Some(bag));
        }
        let mut fielddict: HashSet<u32> = HashSet::new();
        for a in node.attributes() {
            if let Some(fld) = cls.field(joaat(a.name()))
                && fld.data_type == ValueType::String
            {
                bag.set_string(fld.name, a.value());
            }
        }
        for c in child_nodes(node) {
            let elem = match c {
                XChild::Element(e) => e,
                XChild::Text(t) => {
                    bag.set_string(0u32, t);
                    continue;
                }
                XChild::Other => continue,
            };
            let ename = element_name(elem);
            let mut fname = joaat(&ename);
            let mut ncnt = 1;
            while fielddict.contains(&fname) {
                fname = joaat(&format!("{ename}{ncnt}"));
                ncnt += 1;
            }
            let Some(fld) = cls.field(fname) else { continue };
            fielddict.insert(fname);
            self.field(&mut bag, fname, fld, elem)?;
        }
        Ok(Some(bag))
    }

    fn value_attr<'a>(elem: Node<'a, '_>) -> Option<&'a str> {
        elem.attribute("value")
    }

    fn float_attr(&self, elem: Node, name: &str) -> f32 {
        let v = elem.attribute(name);
        if name == "value" && self.stripped_f.contains(&elem.id()) {
            return v.and_then(|s| s.strip_suffix('f')).and_then(parse_f32).unwrap_or(0.0);
        }
        v.and_then(parse_f32).unwrap_or(0.0)
    }

    fn uint_attr(elem: Node) -> Result<u64> {
        match Self::value_attr(elem) {
            Some(s) if s.starts_with("0x") => hex_u64(&s[2..]),
            Some(s) => Ok(try_int::<u64>(s).unwrap_or(0)),
            None => Ok(0),
        }
    }

    fn field(&mut self, bag: &mut DataBag, f: u32, fld: &SchemaField, elem: Node) -> Result<()> {
        let schema = self.schema;
        let v4 = |me: &Self| ["x", "y", "z", "w"].map(|a| me.float_attr(elem, a));
        match fld.data_type {
            ValueType::Float => bag.set_f32(f, self.float_attr(elem, "value")),
            ValueType::Float2 => bag.set_vec2(f, [self.float_attr(elem, "x"), self.float_attr(elem, "y")]),
            ValueType::Float3 => {
                bag.set_vec3(f, [self.float_attr(elem, "x"), self.float_attr(elem, "y"), self.float_attr(elem, "z")])
            }
            ValueType::Float4 => bag.set_vec4(f, v4(self)),
            ValueType::Float3x4 => {
                let v = raw_floats(elem);
                let mut m = [0f32; 12];
                for (d, s) in m.iter_mut().zip(v) {
                    *d = s;
                }
                bag.set_matrix3x4(f, m);
            }
            ValueType::Float4x4 => {
                let v = raw_floats(elem);
                let m: [f32; 16] = v.try_into().map_err(|_| {
                    Error::Format("There must be sixteen and only sixteen input values for Matrix.".into())
                })?;
                bag.set_matrix4x4(f, m);
            }
            ValueType::Int8 => bag.set_i8(f, Self::int_attr(elem) as i8),
            ValueType::Int16 => bag.set_i16(f, Self::int_attr(elem) as i16),
            ValueType::Int32 => bag.set_i32(f, Self::int_attr(elem)),
            ValueType::Int64 => {
                let v = match Self::value_attr(elem) {
                    Some(s) if s.starts_with("0x") => convert_int(&s[2..], 16)?,
                    Some(s) => try_int::<i64>(s).unwrap_or(0),
                    None => 0,
                };
                bag.set_i64(f, v)
            }
            ValueType::UInt8 => bag.set_u8(f, Self::uint32_attr(elem)? as u8),
            ValueType::UInt16 => bag.set_u16(f, Self::uint32_attr(elem)? as u16),
            ValueType::UInt32 => bag.set_u32(f, Self::uint32_attr(elem)?),
            ValueType::UInt64 => bag.set_u64(f, Self::uint_attr(elem)?),
            ValueType::Enum8 => bag.set_u8(f, parse_enum_value(fld, &inner_text(elem), schema) as u8),
            ValueType::Enum16 => bag.set_u16(f, parse_enum_value(fld, &inner_text(elem), schema) as u16),
            ValueType::Enum32 | ValueType::Flags32 => {
                bag.set_u32(f, parse_enum_value(fld, &inner_text(elem), schema) as u32)
            }
            ValueType::Double => {
                bag.set_f64(f, Self::value_attr(elem).and_then(|s| s.trim().parse().ok()).unwrap_or(0.0))
            }
            ValueType::Boolean => bag.set_bool(f, Self::value_attr(elem).and_then(bool_parse).unwrap_or(false)),
            ValueType::Hash => {
                let t = inner_text(elem);
                bag.set_hash(f, parse_hash(&t));
                self.hashes.ensure(&t);
            }
            ValueType::FnvHash => bag.set_fnv_hash(f, fnv_hash(&inner_text(elem))),
            ValueType::Char => {
                let c = Self::value_attr(elem).and_then(|s| s.encode_utf16().next()).unwrap_or(0);
                bag.set_u8(f, c as u8)
            }
            ValueType::String => bag.set_string(f, inner_text(elem)),
            ValueType::Array => self.array(bag, f, fld, elem)?,
            ValueType::Lookup => self.lookup(bag, f, fld, elem)?,
            ValueType::Object => {
                let otype = Self::item_type(elem, "type", fld);
                if fld.type_name == 2 {
                    let b = raw_bytes(elem, 16)?;
                    bag.set_object(f, Some(Value::Bytes(b)));
                } else {
                    let obag = self.node(elem, otype)?;
                    bag.set_bag(f, obag);
                }
            }
            ValueType::Struct => {
                let stype = Self::item_type(elem, "type", fld);
                let sbag = self.node(elem, stype)?;
                bag.set_bag(f, sbag);
            }
            ValueType::None => {}
        }
        Ok(())
    }

    fn int_attr(elem: Node) -> i32 {
        Self::value_attr(elem).and_then(try_int::<i32>).unwrap_or(0)
    }

    fn uint32_attr(elem: Node) -> Result<u32> {
        match Self::value_attr(elem) {
            Some(s) if s.starts_with("0x") => {
                let v = hex_u64(&s[2..])?;
                u32::try_from(v).map_err(|_| Error::Format(format!("{s} is too large for a UInt32")))
            }
            Some(s) => Ok(try_int::<u32>(s).unwrap_or(0)),
            None => Ok(0),
        }
    }

    fn array(&mut self, bag: &mut DataBag, f: u32, fld: &SchemaField, elem: Node) -> Result<()> {
        if matches!(fld.array_type, ValueType::Object | ValueType::Struct) {
            let itype = Self::item_type(elem, "itemType", fld);
            let mut list = Vec::new();
            for inode in items(elem) {
                let t = Self::or_type(inode.attribute("type"), itype);
                if let Some(ibag) = self.node(inode, t)? {
                    list.push(Some(ibag));
                }
            }
            bag.set_object(f, (!list.is_empty()).then_some(Value::Bags(list)));
            return Ok(());
        }
        if fld.array_size > 0 {
            match fld.array_type {
                ValueType::Float => bag.set_embedded_array(f, &raw_floats(elem)),
                ValueType::Float2 => bag.set_embedded_array(f, &raw_vectors::<2>(elem)),
                ValueType::Float3 => bag.set_embedded_array(f, &raw_vectors::<3>(elem)),
                ValueType::Float4 => bag.set_embedded_array(f, &raw_vectors::<4>(elem)),
                ValueType::UInt8 | ValueType::Enum8 | ValueType::Boolean => {
                    bag.set_embedded_array(f, &raw_bytes(elem, 10)?)
                }
                ValueType::UInt16 | ValueType::Enum16 => bag.set_embedded_array(f, &raw_parsed::<u16>(elem)),
                ValueType::UInt32 | ValueType::Enum32 | ValueType::Flags32 => {
                    bag.set_embedded_array(f, &raw_parsed::<u32>(elem))
                }
                ValueType::UInt64 => bag.set_embedded_array(f, &raw_u64s(elem)?),
                ValueType::Int8 => bag.set_embedded_array(f, &raw_sbytes(elem, 16)?),
                ValueType::Int16 => bag.set_embedded_array(f, &raw_parsed::<i16>(elem)),
                ValueType::Int32 => bag.set_embedded_array(f, &raw_parsed::<i32>(elem)),
                ValueType::Int64 => bag.set_embedded_array(f, &raw_parsed::<i64>(elem)),
                ValueType::Hash => bag.set_embedded_array(f, &self.hash_items(elem)),
                ValueType::Char => bag.set_embedded_array(f, &ascii_bytes(&inner_text(elem))),
                ValueType::Array => {
                    if fld.type_name == ValueType::Hash as u32 {
                        let mut list = Vec::new();
                        for inode in items(elem) {
                            let a: Vec<u32> =
                                select(inode, "Item").iter().map(|a| parse_hash(&inner_text(*a))).collect();
                            list.push((!a.is_empty()).then_some(a));
                        }
                        bag.set_object(f, (!list.is_empty()).then_some(Value::HashArrays(list)));
                    } else {
                        let itype = Self::item_type(elem, "itemType", fld);
                        let mut list = Vec::new();
                        for inode in items(elem) {
                            let mut a = Vec::new();
                            for anode in select(inode, "Item") {
                                let t = Self::or_type(anode.attribute("type"), itype);
                                if let Some(abag) = self.node(anode, t)? {
                                    a.push(Some(abag));
                                }
                            }
                            list.push((!a.is_empty()).then_some(a));
                        }
                        bag.set_object(f, (!list.is_empty()).then_some(Value::BagArrays(list)));
                    }
                }
                ValueType::String => {
                    let list: Vec<String> = items(elem).iter().map(|i| inner_text(*i)).collect();
                    bag.set_object(f, (!list.is_empty()).then_some(Value::Strings(list)));
                }
                _ => {}
            }
            return Ok(());
        }
        let v = match fld.array_type {
            ValueType::Hash => Value::Hashes(self.hash_items(elem)),
            ValueType::String => Value::Strings(items(elem).iter().map(|i| inner_text(*i)).collect()),
            ValueType::UInt8 => Value::Bytes(raw_bytes(elem, 10)?),
            ValueType::UInt16 => Value::U16s(raw_parsed(elem)),
            ValueType::UInt32 | ValueType::Enum32 => Value::U32s(raw_parsed(elem)),
            ValueType::UInt64 => Value::U64s(raw_u64s(elem)?),
            ValueType::Int8 => Value::SBytes(raw_sbytes(elem, 10)?),
            ValueType::Int16 => Value::I16s(raw_parsed(elem)),
            ValueType::Int32 => Value::I32s(raw_parsed(elem)),
            ValueType::Int64 => Value::I64s(raw_parsed(elem)),
            ValueType::Float => Value::F32s(raw_floats(elem)),
            ValueType::Float2 => Value::Vec2s(raw_vectors::<2>(elem)),
            // Float3 arrays are float4s.
            ValueType::Float3 | ValueType::Float4 => Value::Vec4s(raw_vectors::<4>(elem)),
            ValueType::Boolean => {
                Value::Bools(split_ws(&inner_text(elem)).map(|s| bool_parse(s).unwrap_or(false)).collect())
            }
            _ => return Ok(()),
        };
        bag.set_object(f, Some(v));
        Ok(())
    }

    /// `Xml.GetHashItemArray`: each item's text hashed (not parsed) and remembered.
    fn hash_items(&self, elem: Node) -> Vec<u32> {
        items(elem)
            .iter()
            .map(|i| {
                let t = inner_text(*i);
                self.hashes.ensure(&t);
                joaat(&t)
            })
            .collect()
    }

    fn lookup(&mut self, bag: &mut DataBag, f: u32, fld: &SchemaField, elem: Node) -> Result<()> {
        let itype = Self::item_type(elem, "itemType", fld);
        let mut list = Vec::new();
        for ielem in items(elem) {
            let ikey = ielem.attribute("key");
            let itype2 = ielem.attribute("type");
            let item = match itype2 {
                Some("String") => KvValue::Str(inner_text(ielem)),
                Some("Int") => KvValue::Int(try_int::<i32>(&inner_text(ielem)).unwrap_or(0)),
                Some("Uint") => KvValue::UInt(try_int::<u32>(&inner_text(ielem)).unwrap_or(0)),
                Some("Float") => KvValue::Float(parse_f32(&inner_text(ielem)).unwrap_or(0.0)),
                Some("Float4") => KvValue::Float4(["x", "y", "z", "w"].map(|a| self.float_attr(ielem, a))),
                Some("ULongs") => KvValue::ULongs(raw_u64s(ielem)?),
                Some("Array") => {
                    let itype3 = TypeRef::from_attr(ielem.attribute("itemType"));
                    let mut arr = Vec::new();
                    for anode in select(ielem, "Item") {
                        arr.push(self.node(anode, itype3)?);
                    }
                    KvValue::Bags(arr)
                }
                _ => match self.node(ielem, Self::or_type(itype2, itype))? {
                    Some(b) => KvValue::Bag(b),
                    None => KvValue::None,
                },
            };
            list.push(Some(make_key_value_pair(ikey, item, fld)));
        }
        bag.set_object(f, (!list.is_empty()).then_some(Value::Bags(list)));
        Ok(())
    }

    /// `GetFieldType`: what kind of value an element looks like.
    fn field_type(&mut self, elem: Node) -> ValueType {
        let children = child_nodes(elem);
        let attrs: Vec<_> = elem.attributes().collect();
        if children.is_empty() {
            match attrs.len() {
                0 => return ValueType::String,
                1 => {
                    let a0n = attrs[0].name().to_lowercase();
                    if a0n == "value" {
                        let val = attrs[0].value();
                        if val.starts_with("0x") {
                            return if val.len() == 18 { ValueType::UInt64 } else { ValueType::UInt32 };
                        }
                        if try_int::<i32>(val).is_some() {
                            return ValueType::Int32;
                        }
                        if parse_f32(val).is_some() {
                            return ValueType::Float;
                        }
                        if bool_parse(val).is_some() {
                            return ValueType::Boolean;
                        }
                        if let Some(t) = val.strip_suffix('f')
                            && parse_f32(t).is_some()
                        {
                            self.stripped_f.insert(elem.id());
                            return ValueType::Float;
                        }
                        return ValueType::String;
                    }
                    return ValueType::Object;
                }
                n @ 2..=4 => {
                    let names = ["x", "y", "z", "w"];
                    if attrs.iter().zip(names).all(|(a, n)| a.name().to_lowercase() == n) {
                        return match n {
                            2 => ValueType::Float2,
                            3 => ValueType::Float3,
                            _ => ValueType::Float4,
                        };
                    }
                }
                _ => {}
            }
            return ValueType::Object;
        }
        if children.len() == 1
            && let XChild::Text(_) = children[0]
        {
            return if elem.attribute("content") == Some("int_array") { ValueType::Array } else { ValueType::String };
        }
        let mut isarr = true;
        let mut islkup = false;
        for c in elem.children() {
            if c.is_element() {
                let n = c.tag_name().name();
                if n != "Item" && n != "item" {
                    isarr = false;
                    break;
                }
                if let Some(a) = c.attributes().next()
                    && a.name().to_lowercase() == "key"
                {
                    islkup = true;
                }
            } else if c.is_text() {
                if !is_ws(c.text().unwrap_or("")) {
                    isarr = false;
                    break;
                }
            } else if c.is_pi() {
                isarr = false;
                break;
            }
        }
        if isarr {
            return if islkup { ValueType::Lookup } else { ValueType::Array };
        }
        ValueType::Object
    }

    /// `CreateSchemaClass`: a class made up from the XML, when there's no schema.
    fn create_class(&mut self, node: Node, ty: TypeRef) -> SchemaClass {
        let name = match ty {
            TypeRef::Node | TypeRef::Name("") => element_name(node),
            TypeRef::Name(s) => s.to_owned(),
            TypeRef::Hash(h) => self.hashes.xml_name(h),
        };
        self.hashes.ensure(&name);
        let cname = joaat(&name);
        let mut fields = Vec::new();
        let mut fielddict: HashSet<u32> = HashSet::new();
        let mut slen = 0u32;
        for a in node.attributes() {
            self.hashes.ensure(a.name());
            fields.push(SchemaField {
                name: joaat(a.name()),
                data_type: ValueType::String,
                is_attribute: true,
                ..Default::default()
            });
        }
        for c in child_nodes(node) {
            match c {
                XChild::Element(elem) => {
                    let et = self.field_type(elem);
                    let fl = et.size();
                    let ename = element_name(elem);
                    self.hashes.ensure(&ename);
                    let mut fld = SchemaField {
                        name: joaat(&ename),
                        data_type: et,
                        offset: if fl == 0 && et != ValueType::Struct { -1 } else { slen as i32 },
                        ..Default::default()
                    };
                    slen += fl;
                    let mut ncnt = 1;
                    while fielddict.contains(&fld.name) {
                        let newname = format!("{ename}{ncnt}");
                        self.hashes.ensure(&newname);
                        fld.name = joaat(&newname);
                        ncnt += 1;
                    }
                    fielddict.insert(fld.name);
                    if et == ValueType::Array {
                        fld.array_type = ValueType::Object;
                        let item0 =
                            select(elem, "Item").into_iter().next().or_else(|| select(elem, "item").into_iter().next());
                        if let Some(item0) = item0 {
                            if item0.attributes().len() == 0 {
                                let ic = child_nodes(item0);
                                if ic.len() == 1 && matches!(ic[0], XChild::Text(_)) {
                                    fld.array_type = ValueType::String;
                                }
                            }
                        } else if elem.attribute("content") == Some("int_array") {
                            fld.array_type = ValueType::UInt32;
                        }
                    } else if et == ValueType::Lookup {
                        fld.array_type = ValueType::String;
                    }
                    fields.push(fld);
                }
                XChild::Text(_) => fields.push(SchemaField { data_type: ValueType::String, ..Default::default() }),
                XChild::Other => {}
            }
        }
        SchemaClass::new(cname, slen, Some(fields), ClassInfo::Plain)
    }
}

/// A lookup entry's item as read from XML.
enum KvValue {
    None,
    Str(String),
    Int(i32),
    UInt(u32),
    Float(f32),
    Float4([f32; 4]),
    ULongs(Vec<u64>),
    Bags(Vec<Option<DataBag>>),
    Bag(DataBag),
}

/// `MakeKeyValuePair`: a `KeyValuePair` bag with `Key` at 0 and `Item` at 8.
fn make_key_value_pair(key: Option<&str>, item: KvValue, fld: &SchemaField) -> DataBag {
    let mut kfld = SchemaField { name: KEY_HASH, data_type: ValueType::Hash, ..Default::default() };
    let mut ifld = SchemaField { name: ITEM_HASH, data_type: ValueType::Object, offset: 8, ..Default::default() };
    if let t @ (ValueType::Hash | ValueType::String | ValueType::Int16 | ValueType::UInt32 | ValueType::Enum32) =
        fld.array_type
    {
        kfld.data_type = t;
    }
    let mut size = 16u32;
    match &item {
        KvValue::Str(_) => ifld.data_type = ValueType::String,
        KvValue::Int(_) => ifld.data_type = ValueType::Int32,
        KvValue::UInt(_) => ifld.data_type = ValueType::UInt32,
        KvValue::Float(_) => ifld.data_type = ValueType::Float,
        KvValue::Float4(_) => {
            ifld.data_type = ValueType::Float4;
            size = 32;
        }
        KvValue::ULongs(_) => {
            ifld.data_type = ValueType::Array;
            ifld.array_type = ValueType::UInt64;
        }
        KvValue::Bags(_) => {
            ifld.data_type = ValueType::Array;
            ifld.array_type = ValueType::Struct;
        }
        KvValue::Bag(b) => size = 8 + b.class.data_size,
        KvValue::None => {}
    }
    ifld.type_name = fld.type_name;
    let cls = Arc::new(SchemaClass::new(KEY_VALUE_PAIR_HASH, size, Some(vec![kfld, ifld]), ClassInfo::Plain));
    let mut bag = DataBag::new(cls);
    match item {
        KvValue::Int(v) => bag.set_i32(ITEM_HASH, v),
        KvValue::UInt(v) => bag.set_u32(ITEM_HASH, v),
        KvValue::Float(v) => bag.set_f32(ITEM_HASH, v),
        KvValue::Float4(v) => bag.set_vec4(ITEM_HASH, v),
        KvValue::Str(s) => bag.set_object(ITEM_HASH, Some(Value::String(s))),
        KvValue::ULongs(u) => bag.set_object(ITEM_HASH, Some(Value::U64s(u))),
        KvValue::Bags(b) => bag.set_object(ITEM_HASH, Some(Value::Bags(b))),
        KvValue::Bag(b) => bag.set_bag(ITEM_HASH, Some(b)),
        KvValue::None => {}
    }
    match fld.array_type {
        ValueType::Hash => bag.set_hash(KEY_HASH, key.map(parse_hash).unwrap_or(0)),
        ValueType::String => {
            if let Some(k) = key {
                bag.set_string(KEY_HASH, k)
            }
        }
        ValueType::Int16 => bag.set_i16(KEY_HASH, key.and_then(try_int::<i16>).unwrap_or(0)),
        ValueType::UInt32 | ValueType::Enum32 => bag.set_u32(KEY_HASH, key.and_then(try_int::<u32>).unwrap_or(0)),
        _ => {}
    }
    bag
}

/// `Xml.GetRawFloatArray`.
fn raw_floats(n: Node) -> Vec<f32> {
    split_ws(&inner_text(n)).map(|s| parse_f32(s).unwrap_or(0.0)).collect()
}

/// `Xml.GetRawUshortArray` and the others that use `TryParse`.
fn raw_parsed<T: std::str::FromStr + Default>(n: Node) -> Vec<T> {
    split_ws(&inner_text(n)).map(|s| try_int::<T>(s).unwrap_or_default()).collect()
}

/// `Xml.GetRawByteArray(node, base)`.
fn raw_bytes(n: Node, base: u32) -> Result<Vec<u8>> {
    split_ws(&inner_text(n))
        .map(|s| {
            let v = convert_int(s, base)?;
            u8::try_from(v).map_err(|_| Error::Format(format!("{s} is out of range for a byte")))
        })
        .collect()
}

/// `Xml.GetRawSByteArray(node, base)`.
fn raw_sbytes(n: Node, base: u32) -> Result<Vec<i8>> {
    split_ws(&inner_text(n))
        .map(|s| {
            let v = convert_int(s, base)?;
            let v = if base == 16 && (0x80..=0xFF).contains(&v) { v - 0x100 } else { v };
            i8::try_from(v).map_err(|_| Error::Format(format!("{s} is out of range for an sbyte")))
        })
        .collect()
}

/// `Xml.GetRawUlongArray`: `0x` hex or decimal.
fn raw_u64s(n: Node) -> Result<Vec<u64>> {
    split_ws(&inner_text(n))
        .map(|s| match s.strip_prefix("0x") {
            Some(h) => hex_u64(h),
            None => Ok(try_int::<u64>(s).unwrap_or(0)),
        })
        .collect()
}

/// `Xml.GetRawVector2Array`/`3`/`4`: one vector per line, "x, y, z".
fn raw_vectors<const N: usize>(n: Node) -> Vec<[f32; N]> {
    let text = inner_text(n);
    let mut out = Vec::new();
    for line in text.split('\n') {
        let s = line.trim();
        if s.is_empty() {
            continue;
        }
        let mut v = [0f32; N];
        let mut c = 0;
        for part in s.split(',') {
            let t = part.trim();
            if t.is_empty() {
                continue;
            }
            let f = parse_f32(t).unwrap_or(0.0);
            if c < N {
                v[c] = f;
            }
            c += 1;
        }
        if c >= N {
            out.push(v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(name: &str, size: u32, fields: Vec<SchemaField>) -> Arc<SchemaClass> {
        Arc::new(SchemaClass::new(joaat(name), size, Some(fields), ClassInfo::Plain))
    }
    fn field(name: &str, offset: i32, t: ValueType) -> SchemaField {
        SchemaField { name: joaat(name), offset, data_type: t, ..Default::default() }
    }

    #[test]
    fn accessors_and_xml() {
        let hashes = HashIndex::new();
        for s in ["Thing", "pos", "count", "name", "flag", "kids", "Kid"] {
            hashes.ensure(s);
        }
        let kid = class("Kid", 4, vec![field("count", 0, ValueType::UInt32)]);
        let mut kids_f = field("kids", -1, ValueType::Array);
        kids_f.array_type = ValueType::Object;
        let thing = class(
            "Thing",
            20,
            vec![
                field("pos", 0, ValueType::Float3),
                field("count", 12, ValueType::UInt32),
                field("flag", 16, ValueType::Boolean),
                field("name", -1, ValueType::String),
                kids_f,
            ],
        );
        let schema = Schema::new([thing.clone(), kid.clone()], []);
        let mut b = schema.create("Thing").unwrap();
        b.set_vec3("pos", [1.0, -2.5, 0.1]);
        b.set_u32("count", 7);
        b.set_bool("flag", true);
        b.set_string("name", "a<b");
        let mut k = DataBag::new(kid);
        k.set_u32("count", 3);
        b.set_object("kids", Some(Value::Bags(vec![Some(k), None])));
        assert_eq!(b.vec3("pos"), [1.0, -2.5, 0.1]);
        assert_eq!(b.u32(joaat("count")), 7);
        assert_eq!(b.u32("missing"), 0);

        let xml = b.to_xml(&hashes, Some(&schema));
        let want = [
            r#"<?xml version="1.0" encoding="UTF-8"?>"#,
            "<Thing>",
            r#" <pos x="1" y="-2.5" z="0.1" />"#,
            r#" <count value="7" />"#,
            r#" <flag value="True" />"#,
            " <name>a&lt;b</name>",
            " <kids>",
            r#"  <Item type="Kid">"#,
            r#"   <count value="3" />"#,
            "  </Item>",
            "  <Item />",
            " </kids>",
            "</Thing>",
        ]
        .map(|l| format!("{l}\n"))
        .concat();
        assert_eq!(xml, want);

        let back = DataBag::from_xml(&xml, Some(&schema), &hashes).unwrap().unwrap();
        assert_eq!(back.data, b.data);
        assert_eq!(back.string("name"), Some("a<b"));
        assert_eq!(back.bags("kids").unwrap().len(), 1);
        assert_eq!(back.to_xml(&hashes, Some(&schema)).replace("  <Item />\n", ""), want.replace("  <Item />\n", ""));
    }

    #[test]
    fn enums_format_like_redforge() {
        let hashes = HashIndex::new();
        let (a, b) = (hashes.ensure("ValA"), hashes.ensure("ValB"));
        let mut names = |h: u32| hashes.xml_name(h);
        let e = Arc::new(SchemaEnum {
            name: 5,
            values: Some(vec![EnumValue { name: a, value: 0 }, EnumValue { name: b, value: 3 }]),
            info: EnumInfo::Plain,
        });
        let schema = Schema::new([], [e]);
        let f = SchemaField { type_name: 5, data_type: ValueType::Flags32, ..Default::default() };
        assert_eq!(format_enum_value(&f, 0b1001, Some(&schema), &mut names), "ValA, ValB");
        assert_eq!(parse_enum_value(&f, "ValA, ValB", Some(&schema)), 0b1001);
        let f = SchemaField { type_name: 5, data_type: ValueType::Enum32, ..Default::default() };
        assert_eq!(format_enum_value(&f, 3, Some(&schema), &mut names), "ValB");
        assert_eq!(format_enum_value(&f, 4, Some(&schema), &mut names), "4");
        assert_eq!(parse_enum_value(&f, "ValB", Some(&schema)), 3);
        assert_eq!(parse_enum_value(&f, "12", Some(&schema)), 12);
        let f = SchemaField { type_name: 6, data_type: ValueType::Enum8, ..Default::default() };
        assert_eq!(format_enum_value(&f, -1, Some(&schema), &mut names), "0xFFFFFFFFFFFFFFFF");
        assert_eq!(format_enum_value(&f, 0, Some(&schema), &mut names), "");
    }
}
