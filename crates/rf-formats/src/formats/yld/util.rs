//! What cloth and fragment structs share: `Rsc8Arr`/`Rsc8Arr32` of plain data,
//! the XML of plain structs (`WriteStructArray`/`ReadStructArray`), ASCII
//! strings and fallible node helpers.

use bytemuck::Pod;

use crate::Result;
use crate::formats::ydr::util::{ArrHeader, read_array};
use crate::rsc8::{BlockBuf, Rsc8Reader, Rsc8Writer};
use crate::xml::{XmlNode, XmlWriter};

/// A `Rsc8Arr` of plain data: a pointer, a u16 count, a u16 capacity and a u32.
#[derive(Clone, Debug, PartialEq)]
pub struct Arr<T> {
    /// `None` for a null pointer or no items (`ReadArray` gives null then).
    pub items: Option<Vec<T>>,
    pub capacity: u16,
    pub unknown: u32,
}

impl<T> Default for Arr<T> {
    fn default() -> Self {
        Self { items: None, capacity: 0, unknown: 0 }
    }
}

impl<T: Pod> Arr<T> {
    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let h = ArrHeader::read(r)?;
        Ok(Self { items: read_array(r, h.ptr, h.count as usize)?, capacity: h.capacity, unknown: h.unknown })
    }

    /// As `new Rsc8Arr<T>(items)`: the capacity is the count.
    pub fn new(items: Option<Vec<T>>) -> Self {
        Self { capacity: items.as_ref().map_or(0, |i| i.len() as u16), items, unknown: 0 }
    }

    /// The pointer (not null for an empty array, as the C# writes it), count,
    /// capacity and last field.
    pub fn write(&self, b: &mut BlockBuf, w: &mut Rsc8Writer) {
        b.ptr(w.array(self.items.as_deref()));
        b.u16(self.items.as_ref().map_or(0, |i| i.len() as u16));
        b.u16(self.capacity);
        b.u32(self.unknown);
    }

    pub fn items(&self) -> &[T] {
        self.items.as_deref().unwrap_or(&[])
    }
}

/// A `Rsc8Arr32` of plain data: a pointer, a u32 count and a u32 capacity.
#[derive(Clone, Debug, PartialEq)]
pub struct Arr32<T> {
    /// `None` for a null pointer or no items.
    pub items: Option<Vec<T>>,
    pub capacity: u32,
}

impl<T> Default for Arr32<T> {
    fn default() -> Self {
        Self { items: None, capacity: 0 }
    }
}

impl<T: Pod> Arr32<T> {
    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let (ptr, count, capacity) = (r.u64()?, r.u32()?, r.u32()?);
        Ok(Self { items: read_array(r, ptr, count as usize)?, capacity })
    }

    /// As `new Rsc8Arr32<T>(items)`: the capacity is the count.
    pub fn new(items: Option<Vec<T>>) -> Self {
        Self { capacity: items.as_ref().map_or(0, |i| i.len() as u32), items }
    }

    pub fn write(&self, b: &mut BlockBuf, w: &mut Rsc8Writer) {
        b.ptr(w.array(self.items.as_deref()));
        b.u32(self.items.as_ref().map_or(0, |i| i.len() as u32));
        b.u32(self.capacity);
    }

    pub fn items(&self) -> &[T] {
        self.items.as_deref().unwrap_or(&[])
    }
}

/// `WriteStructArray`: nothing for `None`, `<id />` when empty, otherwise one
/// `Item` per struct, whose fields `f` writes (with the `field_*` helpers,
/// which leave out default values as `WriteStruct` does).
pub fn write_structs<T>(w: &mut XmlWriter, id: &str, items: Option<&[T]>, mut f: impl FnMut(&mut XmlWriter, &T)) {
    let Some(items) = items else { return };
    if items.is_empty() {
        w.element(&format!("<{id} />"));
        return;
    }
    w.node(id, |w| {
        for item in items {
            w.node("Item", |w| f(w, item));
        }
    });
}

/// `ReadStructArray`: `None` when the node is missing, otherwise one struct
/// per `Item` (an empty node gives an empty array).
pub fn read_structs<'a, 'i, T>(node: XmlNode<'a, 'i>, id: &str, f: impl Fn(XmlNode<'a, 'i>) -> T) -> Option<Vec<T>> {
    let c = node.child(id)?;
    Some(c.children("Item").map(f).collect())
}

pub fn field_f32(w: &mut XmlWriter, id: &str, v: f32) {
    if v != 0.0 {
        w.f32(id, v);
    }
}
pub fn field_u16(w: &mut XmlWriter, id: &str, v: u16) {
    if v != 0 {
        w.u16(id, v);
    }
}
pub fn field_u32(w: &mut XmlWriter, id: &str, v: u32) {
    if v != 0 {
        w.u32(id, v);
    }
}
pub fn field_u64(w: &mut XmlWriter, id: &str, v: u64) {
    if v != 0 {
        w.u64(id, v);
    }
}
pub fn field_vec3(w: &mut XmlWriter, id: &str, v: [f32; 3]) {
    if v != [0.0; 3] {
        w.vec3(id, v);
    }
}
pub fn field_vec4(w: &mut XmlWriter, id: &str, v: [f32; 4]) {
    if v != [0.0; 4] {
        w.vec4(id, v);
    }
}

/// .NET's `Encoding.ASCII.GetString`: bytes past 0x7F become `?`.
pub fn ascii_string(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| if b < 0x80 { b as char } else { '?' }).collect()
}

/// .NET's `Encoding.ASCII.GetBytes`: characters past 0x7F become `?`.
pub fn ascii_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| if (c as u32) < 0x80 { c as u8 } else { b'?' }).collect()
}

/// Runs `f` inside a node, returning its result.
pub fn node_with(w: &mut XmlWriter, id: &str, f: impl FnOnce(&mut XmlWriter) -> Result<()>) -> Result<()> {
    let mut res = Ok(());
    w.node(id, |w| res = f(w));
    res
}

/// `WriteNodeArray` of fallible items, keeping null items.
pub fn node_array_with<T>(
    w: &mut XmlWriter,
    id: &str,
    items: Option<&[Option<T>]>,
    mut f: impl FnMut(&mut XmlWriter, &T) -> Result<()>,
) -> Result<()> {
    let mut res = Ok(());
    w.node_array_opt(id, items, |w, x| {
        if res.is_ok() {
            res = f(w, x);
        }
    });
    res
}
