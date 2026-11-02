//! RBF metadata ("RBF0" files, some of the game's `.ymt`), from `RbfFile.cs`.
//!
//! An RBF file is a stream of records describing an XML-like tree: structures
//! (elements) with attributes and children, typed values (`UInt32`, booleans,
//! floats, `Float3`, strings), and raw content bytes. Element names are stored
//! once, in a descriptor table built as they first appear.
//!
//! Like the C#, converting to XML goes through a [`DataBag`] made up from the
//! tree (`CreateDataBag`), so the XML is what `DataBag2.ToXml` writes; converting
//! back reads the XML straight into a tree (`Traverse`, not `DataBag2.FromXml`)
//! and writes that.
//!
//! C# behaviours kept on purpose:
//! - fields with the same name in a structure are renamed `Name1`, `Name2`... when
//!   the data bag is made, and the names are added to the hash index;
//! - a structure's children that all have the same name are an array, and
//!   `Item` children always are; an array of other names gets `itemType`;
//! - from XML, a `value` attribute is `True`/`False`, `0x` hex (a `UInt32`), a
//!   float, or else a string, so integers come back as floats; every other
//!   attribute (including `type`) becomes an attribute of the structure;
//! - from XML, text content is written as bytes with a terminating 0, unless the
//!   element has a `content="char_array"`/`"short_array"` attribute.
//!
//! So the XML isn't stable through round trips, as in the C# (e.g. an
//! `<Item key="0" value="10" />` comes back as a float, which an array of
//! structures then drops), and content without a name is written as `<>`, which
//! can't be read back (by the C# either).
//!
//! The C#'s `Save` doesn't reproduce the game's files byte for byte (descriptors
//! are keyed by name only, and the structure header's two unknown shorts are
//! written as 0), but what it writes reads back to the same tree.
//!
//! Known gaps: `xmlns` declarations aren't counted as attributes and CDATA is read
//! as text, unlike `XDocument`; `NumberStyles.Any` float parsing (`value`
//! attributes) is reproduced for the usual forms, not currency symbols.

use std::collections::HashMap;
use std::sync::Arc;

use super::XmlContext;
use super::databag::{ClassInfo, DataBag, SchemaClass, SchemaField, Value, ValueType, ascii_bytes, ascii_string};
use crate::hash::{HashIndex, joaat};
use crate::{Error, Result, bail};

/// `RBF_IDENT`: "RBF0".
pub const MAGIC: u32 = 0x3046_4252;

/// `RbfDataType`: the type byte of a record.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RbfDataType {
    /// A structure, or content bytes.
    None = 0,
    UInt32 = 0x10,
    BoolTrue = 0x20,
    BoolFalse = 0x30,
    Float = 0x40,
    Float3 = 0x50,
    String = 0x60,
}

/// One node of the tree (`IRbfType`).
#[derive(Clone, Debug, PartialEq)]
pub struct RbfItem {
    /// `None` for content bytes read from a file (the C#'s null name).
    pub name: Option<String>,
    pub value: RbfValue,
}

/// The kinds of node.
#[derive(Clone, Debug, PartialEq)]
pub enum RbfValue {
    /// `RbfBytes`: content bytes.
    Bytes(Vec<u8>),
    /// `RbfBytesString`: content bytes read as a string (without the final 0).
    BytesString(String),
    /// `RbfBytesShortArray`: content bytes read as `short`s.
    BytesShortArray(Vec<i16>),
    UInt32(u32),
    Boolean(bool),
    Float(f32),
    Float3([f32; 3]),
    String(String),
    Structure(RbfStructure),
}

/// `RbfStructure`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RbfStructure {
    /// `None` for XML nodes the C# can't convert (comments...): writing fails then.
    pub children: Vec<Option<RbfItem>>,
    pub attributes: Vec<RbfItem>,
    /// The index in `children` of the content bytes (the C#'s `Content`).
    pub content: Option<usize>,
    pending_attributes: i16,
}

impl RbfStructure {
    /// `AddChild`: the first values after the header are its attributes.
    fn add_child(&mut self, item: RbfItem) {
        if self.pending_attributes > 0 {
            self.pending_attributes -= 1;
            self.attributes.push(item);
        } else {
            self.children.push(Some(item));
        }
    }

    fn content_value(&self) -> Option<&RbfValue> {
        self.content.and_then(|i| self.children.get(i)).and_then(|c| c.as_ref()).map(|c| &c.value)
    }
}

impl RbfItem {
    pub fn data_type(&self) -> RbfDataType {
        match &self.value {
            RbfValue::Bytes(_) | RbfValue::BytesString(_) | RbfValue::BytesShortArray(_) | RbfValue::Structure(_) => {
                RbfDataType::None
            }
            RbfValue::UInt32(_) => RbfDataType::UInt32,
            RbfValue::Boolean(true) => RbfDataType::BoolTrue,
            RbfValue::Boolean(false) => RbfDataType::BoolFalse,
            RbfValue::Float(_) => RbfDataType::Float,
            RbfValue::Float3(_) => RbfDataType::Float3,
            RbfValue::String(_) => RbfDataType::String,
        }
    }

    fn structure(&self) -> Option<&RbfStructure> {
        match &self.value {
            RbfValue::Structure(s) => Some(s),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Reading

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn eof() -> Error {
        Error::Format("RBF: unexpected end of stream".into())
    }
    fn u8(&mut self) -> Result<u8> {
        let b = *self.data.get(self.pos).ok_or_else(Self::eof)?;
        self.pos += 1;
        Ok(b)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let b = self.data.get(self.pos..self.pos + N).ok_or_else(Self::eof)?;
        self.pos += N;
        Ok(b.try_into().unwrap())
    }
    fn i16(&mut self) -> Result<i16> {
        self.array().map(i16::from_le_bytes)
    }
    fn i32(&mut self) -> Result<i32> {
        self.array().map(i32::from_le_bytes)
    }
    fn u32(&mut self) -> Result<u32> {
        self.array().map(u32::from_le_bytes)
    }
    fn f32(&mut self) -> Result<f32> {
        self.array().map(f32::from_le_bytes)
    }
    /// `BinaryReader.ReadBytes`: fewer bytes at the end of the data.
    fn bytes(&mut self, n: i32) -> Result<&[u8]> {
        let n = usize::try_from(n).map_err(|_| Error::Format(format!("RBF: negative length {n}")))?;
        let end = (self.pos + n).min(self.data.len());
        let b = &self.data[self.pos..end];
        self.pos = end;
        Ok(b)
    }
}

/// `RbfFile.Load`: the tree of a file (its root structure).
pub fn read(data: &[u8]) -> Result<RbfItem> {
    let mut r = Reader { data, pos: 0 };
    if r.i32()? as u32 != MAGIC {
        bail!("The file identifier does not match.");
    }
    // The open structures; the last is the current one.
    let mut stack: Vec<RbfItem> = Vec::new();
    let mut descriptors: Vec<(String, u8)> = Vec::new();
    while r.pos < data.len() {
        let index = r.u8()?;
        if index == 0xFF {
            let b = r.u8()?;
            if b != 0xFF {
                bail!("Expected 0xFF but was {b:02X}");
            }
            let Some(done) = stack.pop() else { bail!("RBF: a close tag without a structure") };
            match stack.last_mut() {
                Some(RbfItem { value: RbfValue::Structure(parent), .. }) => parent.add_child(done),
                Some(_) => unreachable!(),
                None => {
                    if r.pos != data.len() {
                        bail!("Expected end of stream but was not.");
                    }
                    return Ok(done);
                }
            }
        } else if index == 0xFD {
            let b = r.u8()?;
            if b != 0xFF {
                bail!("Expected 0xFF but was {b:02X}");
            }
            let len = r.i32()?;
            let bytes = r.bytes(len)?.to_vec();
            let Some(RbfItem { value: RbfValue::Structure(cur), .. }) = stack.last_mut() else {
                bail!("RBF: content outside a structure");
            };
            let content = cur.attributes.iter().find(|a| a.name.as_deref() == Some("content"));
            let datatype = match content {
                Some(RbfItem { value: RbfValue::String(s), .. }) => s.as_str(),
                _ => "string",
            };
            let value = match datatype {
                "short_array" => {
                    let n = (len / 2) as usize;
                    if bytes.len() < n * 2 {
                        bail!("RBF: short array past the end of the data");
                    }
                    RbfValue::BytesShortArray(
                        bytes.as_chunks::<2>().0.iter().take(n).map(|c| i16::from_le_bytes(*c)).collect(),
                    )
                }
                "string" => {
                    if bytes.is_empty() {
                        bail!("RBF: empty string content");
                    }
                    RbfValue::BytesString(ascii_string(&bytes[..bytes.len() - 1]))
                }
                _ => RbfValue::Bytes(bytes),
            };
            cur.children.push(Some(RbfItem { name: None, value }));
            cur.content = Some(cur.children.len() - 1);
        } else {
            let data_type = r.u8()?;
            let index = index as usize;
            if index == descriptors.len() {
                let n = r.i16()?;
                let name = ascii_string(r.bytes(n as i32)?);
                descriptors.push((name, data_type));
            }
            let Some((name, _)) = descriptors.get(index) else {
                bail!("RBF: descriptor {index} out of range");
            };
            let name = Some(name.clone());
            let value = match data_type {
                0x00 => {
                    r.i16()?;
                    r.i16()?;
                    let pending_attributes = r.i16()?;
                    let s = RbfStructure { pending_attributes, ..Default::default() };
                    stack.push(RbfItem { name, value: RbfValue::Structure(s) });
                    continue;
                }
                0x10 => RbfValue::UInt32(r.u32()?),
                0x20 => RbfValue::Boolean(true),
                0x30 => RbfValue::Boolean(false),
                0x40 => RbfValue::Float(r.f32()?),
                0x50 => RbfValue::Float3([r.f32()?, r.f32()?, r.f32()?]),
                0x60 => {
                    let n = r.i16()?;
                    RbfValue::String(ascii_string(r.bytes(n as i32)?))
                }
                _ => bail!("Unsupported data type."),
            };
            let Some(RbfItem { value: RbfValue::Structure(cur), .. }) = stack.last_mut() else {
                bail!("RBF: a value outside a structure");
            };
            cur.add_child(RbfItem { name, value });
        }
    }
    bail!("Unexpected end of stream.")
}

// ---------------------------------------------------------------------------
// To a data bag (CreateDataBag / CreateSchemaClass)

/// `GetEngineType`.
fn engine_type(t: RbfDataType) -> ValueType {
    match t {
        RbfDataType::None => ValueType::None,
        RbfDataType::UInt32 => ValueType::UInt32,
        RbfDataType::BoolTrue | RbfDataType::BoolFalse => ValueType::Boolean,
        RbfDataType::Float => ValueType::Float,
        RbfDataType::Float3 => ValueType::Float3,
        RbfDataType::String => ValueType::String,
    }
}

fn name_hash(name: &Option<String>) -> u32 {
    name.as_deref().map(joaat).unwrap_or(0)
}

fn ensure(hashes: &HashIndex, name: &Option<String>) {
    if let Some(n) = name.as_deref().filter(|n| !n.is_empty()) {
        hashes.ensure(n);
    }
}

/// `CreateSchemaClass`: a class for a structure, renaming its fields that have the
/// same name as an earlier one.
fn create_schema_class(item: &mut RbfItem, hashes: &HashIndex) -> SchemaClass {
    let cname = name_hash(&item.name);
    ensure(hashes, &item.name);
    let RbfValue::Structure(s) = &mut item.value else { unreachable!() };
    let nattrs = s.attributes.len();
    let mut fields = Vec::new();
    let mut fielddict: HashMap<u32, ()> = HashMap::new();
    let mut slen = 0u32;
    let items = s.attributes.iter_mut().chain(s.children.iter_mut().flatten());
    for (icnt, item) in items.enumerate() {
        let et = engine_type(item.data_type());
        let mut fld = SchemaField {
            name: name_hash(&item.name),
            data_type: et,
            is_attribute: icnt < nattrs,
            offset: slen as i32,
            ..Default::default()
        };
        slen += et.size();
        let iname = item.name.clone().unwrap_or_default();
        let mut ncnt = 1;
        while fielddict.contains_key(&fld.name) {
            let n = format!("{iname}{ncnt}");
            fld.name = joaat(&n);
            item.name = Some(n);
            ncnt += 1;
        }
        fielddict.insert(fld.name, ());
        ensure(hashes, &item.name);

        if item.data_type() == RbfDataType::None {
            let cstruc = item.structure();
            let mut isarray = false;
            if let Some(cs) = cstruc
                && !cs.children.is_empty()
            {
                let names: Vec<Option<&str>> =
                    cs.children.iter().map(|c| c.as_ref().and_then(|c| c.name.as_deref())).collect();
                let ccname = names[0];
                isarray = ccname.is_some() && cs.content.is_none();
                if names[1..].iter().any(|n| *n != ccname) {
                    isarray = false;
                }
                let isitemarray = matches!(ccname, Some("Item" | "item"));
                isarray = isarray || isitemarray;
                if cs.children.len() == 1 {
                    isarray = isitemarray;
                }
                if isarray && !isitemarray {
                    fld.type_name = ccname.map(joaat).unwrap_or(0);
                }
            }
            if isarray {
                fld.data_type = ValueType::Array;
                fld.array_type = ValueType::Object;
            } else if let Some(content) = cstruc.and_then(|c| c.content_value()) {
                match content {
                    RbfValue::BytesString(_) => {
                        fld.data_type = ValueType::String;
                        fld.array_type = ValueType::UInt8;
                    }
                    RbfValue::BytesShortArray(_) => {
                        fld.data_type = ValueType::Array;
                        fld.array_type = ValueType::Int16;
                    }
                    _ => {
                        fld.data_type = ValueType::Array;
                        fld.array_type = ValueType::UInt8;
                    }
                }
            } else if cstruc.is_some() {
                fld.data_type = ValueType::Object;
            } else if let RbfValue::BytesString(_) = item.value {
                fld.data_type = ValueType::String;
            }
        }
        fields.push(fld);
    }
    SchemaClass::new(cname, slen, Some(fields), ClassInfo::Plain)
}

/// `CreateDataBag`: a bag for a structure.
fn create_data_bag(item: &mut RbfItem, hashes: &HashIndex) -> Option<DataBag> {
    item.structure()?;
    let cls = Arc::new(create_schema_class(item, hashes));
    let mut bag = DataBag::new(cls.clone());
    let RbfValue::Structure(s) = &mut item.value else { unreachable!() };
    for item in s.attributes.iter_mut().chain(s.children.iter_mut().flatten()) {
        ensure(hashes, &item.name);
        let f = name_hash(&item.name);
        let Some(fld) = cls.field(f) else { continue };
        match fld.data_type {
            ValueType::Array => {
                let Some(cs) = (match &mut item.value {
                    RbfValue::Structure(cs) if !cs.children.is_empty() => Some(cs),
                    _ => None,
                }) else {
                    continue;
                };
                match fld.array_type {
                    ValueType::UInt8 => {
                        let v = match cs.content_value() {
                            Some(RbfValue::Bytes(b)) => Some(Value::Bytes(b.clone())),
                            _ => None,
                        };
                        bag.set_object(f, v);
                    }
                    ValueType::Int16 => {
                        let v = match cs.content_value() {
                            Some(RbfValue::BytesShortArray(a)) => Some(Value::I16s(a.clone())),
                            _ => None,
                        };
                        bag.set_object(f, v);
                    }
                    ValueType::Object => {
                        let arr = cs.children.iter_mut().map(|c| c.as_mut().and_then(|c| create_data_bag(c, hashes)));
                        let arr = arr.collect();
                        bag.set_object(f, Some(Value::Bags(arr)));
                    }
                    _ => {}
                }
            }
            ValueType::String => {
                let s = match &item.value {
                    RbfValue::Structure(cs) => match cs.content_value() {
                        Some(RbfValue::BytesString(s)) => Some(s),
                        _ => None,
                    },
                    RbfValue::String(s) | RbfValue::BytesString(s) => Some(s),
                    _ => None,
                };
                if let Some(s) = s {
                    bag.set_string(f, s.clone());
                }
            }
            ValueType::UInt32 => {
                if let RbfValue::UInt32(v) = item.value {
                    bag.set_u32(f, v);
                }
            }
            ValueType::Float => {
                if let RbfValue::Float(v) = item.value {
                    bag.set_f32(f, v);
                }
            }
            ValueType::Float3 => {
                if let RbfValue::Float3(v) = item.value {
                    bag.set_vec3(f, v);
                }
            }
            ValueType::None | ValueType::Object => {
                if let Some(cs) = item.structure()
                    && (!cs.children.is_empty() || !cs.attributes.is_empty())
                {
                    let child = create_data_bag(item, hashes);
                    bag.set_bag(f, child);
                }
            }
            ValueType::Boolean => {
                if let RbfValue::Boolean(v) = item.value {
                    bag.set_bool(f, v);
                }
            }
            _ => {}
        }
    }
    Some(bag)
}

/// The data bag of a tree, as `RbfFile.Load` makes it (renaming duplicate fields
/// in the tree).
pub fn data_bag(root: &mut RbfItem, hashes: &HashIndex) -> Option<DataBag> {
    create_data_bag(root, hashes)
}

/// An RBF file as XML (`DataBag2.ToXml` of its data bag).
pub fn to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let mut root = read(file)?;
    let bag = data_bag(&mut root, cx.hashes).ok_or_else(|| Error::Format("RBF: the root isn't a structure".into()))?;
    Ok(bag.to_xml(cx.hashes, None))
}

// ---------------------------------------------------------------------------
// From XML (Traverse)

/// `float.TryParse(s, NumberStyles.Any, CultureInfo.InvariantCulture)`, as far as
/// game data needs: white space, a leading or trailing sign, parentheses,
/// thousands separators and an exponent.
fn dotnet_parse_float(s: &str) -> Option<f32> {
    let t = s.trim_matches(|c: char| c.is_ascii_whitespace() || c == '\u{b}');
    match t {
        "NaN" => return Some(f32::NAN),
        "Infinity" | "∞" | "+Infinity" | "+∞" => return Some(f32::INFINITY),
        "-Infinity" | "-∞" => return Some(f32::NEG_INFINITY),
        _ => {}
    }
    let (mut neg, mut t) = (false, t);
    if let Some(i) = t.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        neg = true;
        t = i.trim();
    }
    if let Some(i) = t.strip_prefix('-') {
        neg = !neg;
        t = i.trim_start();
    } else if let Some(i) = t.strip_prefix('+') {
        t = i.trim_start();
    } else if let Some(i) = t.strip_suffix('-') {
        neg = !neg;
        t = i.trim_end();
    } else if let Some(i) = t.strip_suffix('+') {
        t = i.trim_end();
    }
    let b = t.as_bytes();
    let mut i = 0;
    let mut num = String::with_capacity(t.len() + 1);
    if neg {
        num.push('-');
    }
    let mut digits = 0;
    while i < b.len() && (b[i].is_ascii_digit() || (b[i] == b',' && digits > 0)) {
        if b[i] != b',' {
            num.push(b[i] as char);
            digits += 1;
        }
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        num.push('.');
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            num.push(b[i] as char);
            digits += 1;
            i += 1;
        }
    }
    if digits == 0 {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        let mut exp = String::from("e");
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            exp.push(b[j] as char);
            j += 1;
        }
        let start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            exp.push(b[j] as char);
            j += 1;
        }
        if j > start {
            num.push_str(&exp);
            i = j;
        }
    }
    if i != b.len() {
        return None;
    }
    num.parse().ok()
}

/// `CreateValueNode`.
fn create_value_node(name: &str, val: &str) -> RbfItem {
    let name = Some(name.to_owned());
    let value = if val == "True" {
        RbfValue::Boolean(true)
    } else if val == "False" {
        RbfValue::Boolean(false)
    } else if let Some(hex) = val.strip_prefix("0x") {
        let h = hex.trim_matches(|c: char| c.is_ascii_whitespace());
        let v = if !h.is_empty() && h.bytes().all(|c| c.is_ascii_hexdigit()) {
            u32::from_str_radix(h, 16).unwrap_or(0)
        } else {
            0
        };
        RbfValue::UInt32(v)
    } else if let Some(f) = dotnet_parse_float(val) {
        RbfValue::Float(f)
    } else {
        RbfValue::String(val.to_owned())
    };
    RbfItem { name, value }
}

/// `Convert.ToByte`/`Convert.ToUInt16` of each number in the text.
fn parse_numbers<T: std::str::FromStr>(text: &str) -> Result<Vec<T>> {
    text.split(char::is_whitespace)
        .filter(|s| !s.is_empty())
        .map(|s| {
            let t = s.strip_prefix('+').unwrap_or(s);
            t.parse().map_err(|_| Error::Format(format!("RBF: invalid number \"{s}\"")))
        })
        .collect()
}

/// A text node's text as `XmlDocument.LoadXml` reads it: without end-of-line
/// normalization, so `\r\n` stays (roxmltree turns it into `\n`).
fn node_text<'a>(n: roxmltree::Node<'a, '_>, input: &'a str) -> std::borrow::Cow<'a, str> {
    let text = n.text().unwrap_or("");
    let raw = input.get(n.range()).unwrap_or("");
    if !raw.contains('\r') || raw.contains('<') {
        return text.into();
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let Some(end) = rest[i..].find(';') else { return text.into() };
        let ent = &rest[i + 1..i + end];
        let c = match ent {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => match ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")) {
                Some(h) => u32::from_str_radix(h, 16).ok().and_then(char::from_u32),
                None => ent.strip_prefix('#').and_then(|d| d.parse().ok()).and_then(char::from_u32),
            },
        };
        let Some(c) = c else { return text.into() };
        out.push(c);
        rest = &rest[i + end + 1..];
    }
    out.push_str(rest);
    out.into()
}

/// The text nodes an `XmlDocument` keeps (it drops whitespace-only ones).
fn is_kept_text(n: roxmltree::Node) -> bool {
    n.is_text() && !n.text().unwrap_or("").chars().all(|c| matches!(c, ' ' | '\t' | '\r' | '\n'))
}

/// `Traverse`: an XML node as a tree node, `None` for nodes it doesn't convert.
fn traverse(node: roxmltree::Node, input: &str) -> Result<Option<RbfItem>> {
    if node.is_element() {
        let name = node.tag_name().name();
        let attrs: Vec<_> = node.attributes().collect();
        let has_elements = node.children().any(|c| c.is_element());
        if let Some(val) = node.attribute("value") {
            if !val.is_empty() {
                return Ok(Some(create_value_node(name, val)));
            }
        } else if attrs.len() == 3
            && let (Some(x), Some(y), Some(z)) = (node.attribute("x"), node.attribute("y"), node.attribute("z"))
        {
            let f = |s: &str| dotnet_parse_float(s).unwrap_or(0.0);
            return Ok(Some(RbfItem { name: Some(name.to_owned()), value: RbfValue::Float3([f(x), f(y), f(z)]) }));
        } else if !has_elements && attrs.is_empty() && !input[node.range()].ends_with("/>") {
            let text: String = node.children().filter(|c| c.is_text()).map(|c| node_text(c, input)).collect();
            let mut bytes = ascii_bytes(&text);
            bytes.push(0);
            let s = RbfStructure {
                children: vec![Some(RbfItem { name: None, value: RbfValue::Bytes(bytes) })],
                ..Default::default()
            };
            return Ok(Some(RbfItem { name: Some(name.to_owned()), value: RbfValue::Structure(s) }));
        }
        let mut s = RbfStructure::default();
        for c in node.children() {
            if c.is_element() || is_kept_text(c) || c.is_comment() || c.is_pi() {
                s.children.push(traverse(c, input)?);
            }
        }
        for a in attrs {
            s.attributes.push(create_value_node(a.name(), a.value()));
        }
        return Ok(Some(RbfItem { name: Some(name.to_owned()), value: RbfValue::Structure(s) }));
    }
    if node.is_text() {
        let text = node_text(node, input);
        let bytes = match node.parent().and_then(|p| p.attribute("content")) {
            Some("char_array") => (!text.is_empty()).then(|| parse_numbers::<u8>(&text)).transpose()?,
            Some("short_array") => {
                Some(parse_numbers::<u16>(&text)?.into_iter().flat_map(|v| v.to_le_bytes()).collect())
            }
            Some(_) => None,
            None => {
                let mut b = ascii_bytes(&text);
                b.push(0);
                Some(b)
            }
        };
        return Ok(bytes.map(|b| RbfItem { name: Some(String::new()), value: RbfValue::Bytes(b) }));
    }
    Ok(None)
}

/// `new RbfFile(xml)`: the tree of an XML document.
pub fn from_xml_tree(xml: &str) -> Result<RbfItem> {
    let text = xml.strip_prefix('\u{feff}').unwrap_or(xml);
    let opt = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(text, opt).map_err(|e| Error::Format(format!("RBF XML: {e}")))?;
    match traverse(doc.root_element(), text)? {
        Some(root) if root.structure().is_some() => Ok(root),
        _ => bail!("RBF XML: the root element isn't a structure"),
    }
}

// ---------------------------------------------------------------------------
// Writing

struct Writer {
    out: Vec<u8>,
    descriptors: HashMap<String, usize>,
}

impl Writer {
    /// `WriteRecordId`.
    fn record_id(&mut self, item: &RbfItem) {
        let name = item.name.clone().unwrap_or_default();
        let n = self.descriptors.len();
        let (idx, is_new) = match self.descriptors.get(&name) {
            Some(&i) => (i, false),
            None => {
                self.descriptors.insert(name.clone(), n);
                (n, true)
            }
        };
        self.out.push(idx as u8);
        self.out.push(item.data_type() as u8);
        if is_new {
            let b = ascii_bytes(&name);
            self.out.extend_from_slice(&(b.len() as u16).to_le_bytes());
            self.out.extend_from_slice(&b);
        }
    }

    fn content(&mut self, len: i32, bytes: &[u8]) {
        self.out.extend_from_slice(&[0xFD, 0xFF]);
        self.out.extend_from_slice(&len.to_le_bytes());
        self.out.extend_from_slice(bytes);
    }

    fn item(&mut self, item: &RbfItem) -> Result<()> {
        match &item.value {
            RbfValue::Structure(s) => {
                self.record_id(item);
                self.out.extend_from_slice(&[0; 4]);
                self.out.extend_from_slice(&(s.attributes.len() as i16).to_le_bytes());
                for a in &s.attributes {
                    self.item(a)?;
                }
                for c in &s.children {
                    let Some(c) = c else { bail!("RBF: a node that can't be written (a comment?)") };
                    self.item(c)?;
                }
                self.out.extend_from_slice(&[0xFF, 0xFF]);
            }
            RbfValue::Bytes(b) => self.content(b.len() as i32, b),
            RbfValue::BytesString(s) => {
                let mut b: Vec<u8> = s.encode_utf16().map(|c| c as u8).collect();
                b.push(0);
                self.content(b.len() as i32, &b);
            }
            RbfValue::BytesShortArray(a) => {
                let b: Vec<u8> = a.iter().flat_map(|v| v.to_le_bytes()).collect();
                self.content(b.len() as i32, &b);
            }
            RbfValue::UInt32(v) => {
                self.record_id(item);
                self.out.extend_from_slice(&v.to_le_bytes());
            }
            RbfValue::Boolean(_) => self.record_id(item),
            RbfValue::Float(v) => {
                self.record_id(item);
                self.out.extend_from_slice(&v.to_le_bytes());
            }
            RbfValue::Float3(v) => {
                self.record_id(item);
                for f in v {
                    self.out.extend_from_slice(&f.to_le_bytes());
                }
            }
            RbfValue::String(s) => {
                self.record_id(item);
                let b = ascii_bytes(s);
                self.out.extend_from_slice(&(b.len() as i16).to_le_bytes());
                self.out.extend_from_slice(&b);
            }
        }
        Ok(())
    }
}

/// `RbfFile.Save`: a tree as an RBF file.
pub fn write(root: &RbfItem) -> Result<Vec<u8>> {
    let mut w = Writer { out: MAGIC.to_le_bytes().to_vec(), descriptors: HashMap::new() };
    w.item(root)?;
    Ok(w.out)
}

/// XML back to an RBF file.
pub fn from_xml(xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    write(&from_xml_tree(xml)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx(h: &HashIndex) -> XmlContext<'_> {
        XmlContext { hashes: h, folder: None }
    }

    #[test]
    fn binary_round_trip_and_xml() {
        let xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Root kind=\"a\">\n <Name>hello</Name>\n \
                   <Count value=\"0x10\" />\n <Scale value=\"1.5\" />\n <On value=\"True\" />\n \
                   <Pos x=\"1\" y=\"2\" z=\"3\" />\n <List>\n  <Item>\n   <A>x</A>\n  </Item>\n  <Item>\n   \
                   <A>y</A>\n  </Item>\n </List>\n <Dup>1</Dup>\n <Dup>2</Dup>\n</Root>\n";
        let h = HashIndex::new();
        let bin = from_xml(xml, &cx(&h)).unwrap();
        assert_eq!(&bin[..4], b"RBF0");
        let tree = read(&bin).unwrap();
        assert_eq!(write(&tree).unwrap(), bin);
        let out = to_xml(&bin, &cx(&h)).unwrap();
        let want = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Root>\n <Name>hello</Name>\n \
                    <Count value=\"16\" />\n <Scale value=\"1.5\" />\n <On value=\"True\" />\n \
                    <Pos x=\"1\" y=\"2\" z=\"3\" />\n <List>\n  <Item>\n   <A>x</A>\n  </Item>\n  <Item>\n   \
                    <A>y</A>\n  </Item>\n </List>\n <Dup>1</Dup>\n <Dup1>2</Dup1>\n</Root>\n";
        assert_eq!(out, want);
        assert_eq!(h.get(joaat("Dup1")).as_deref(), Some("Dup1"));
    }

    #[test]
    fn text_keeps_crlf() {
        let tree = from_xml_tree("<R>\r\n<M>a\r\nb &amp; c</M></R>").unwrap();
        let RbfValue::Structure(r) = &tree.value else { panic!() };
        let RbfValue::Structure(m) = &r.children[0].as_ref().unwrap().value else { panic!() };
        assert_eq!(m.children[0].as_ref().unwrap().value, RbfValue::Bytes(b"a\r\nb & c\0".to_vec()));
    }

    #[test]
    fn value_parsing_like_dotnet() {
        let v = |s: &str| create_value_node("v", s).value;
        assert_eq!(v("12"), RbfValue::Float(12.0));
        assert_eq!(v("1,000"), RbfValue::Float(1000.0));
        assert_eq!(v("0xZZ"), RbfValue::UInt32(0));
        assert_eq!(v("0xFF"), RbfValue::UInt32(255));
        assert_eq!(v("inf"), RbfValue::String("inf".into()));
        assert_eq!(v("1e3"), RbfValue::Float(1000.0));
        assert_eq!(v("-2.5"), RbfValue::Float(-2.5));
        assert_eq!(v(".5"), RbfValue::Float(0.5));
        assert_eq!(v("abc"), RbfValue::String("abc".into()));
    }
}
