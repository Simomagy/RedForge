//! Reading and writing helpers shared by the drawable structs: the C#'s
//! `Rsc8PtrArr`, `Rsc8Arr`, `ReadArray` semantics and XML struct arrays.

use bytemuck::Pod;

use crate::rsc8::{BlockBuf, BlockId, Rsc8Reader, Rsc8Writer};
use crate::xml::number::f32_to_string;
use crate::xml::{XmlNode, XmlWriter};
use crate::{Result, bail};

/// `ReadArray<T>(count)` at `ptr`: `None` for a null pointer or no items, and
/// zeros for any bytes past the end of the data (`BufferUtil.ReadArray`).
pub fn read_array<T: Pod>(r: &Rsc8Reader, ptr: u64, count: usize) -> Result<Option<Vec<T>>> {
    if ptr == 0 || count == 0 {
        return Ok(None);
    }
    let bytes = r.bytes_at_padded(ptr, count * size_of::<T>())?;
    Ok(Some(bytemuck::pod_collect_to_vec(&bytes)))
}

/// The header of a `Rsc8PtrArr`/`Rsc8Arr`: pointer, u16 count, u16 capacity,
/// u32.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArrHeader {
    pub ptr: u64,
    pub count: u16,
    pub capacity: u16,
    pub unknown: u32,
}

impl ArrHeader {
    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        Ok(Self { ptr: r.u64()?, count: r.u16()?, capacity: r.u16()?, unknown: r.u32()? })
    }
}

/// `Rsc8PtrArr.Read`: the items its pointers point to (`None` items for null
/// pointers); `None` when the pointer is null or the count 0.
pub fn read_ptr_arr<T>(
    r: &mut Rsc8Reader,
    h: &ArrHeader,
    mut f: impl FnMut(&mut Rsc8Reader) -> Result<T>,
) -> Result<Option<Vec<Option<T>>>> {
    let Some(ptrs) = read_array::<u64>(r, h.ptr, h.count as usize)? else { return Ok(None) };
    let mut v = Vec::with_capacity(ptrs.len());
    for p in ptrs {
        v.push(r.follow(p, &mut f)?);
    }
    Ok(Some(v))
}

/// `Rsc8PtrArr.Write`: room for the capacity (if larger than the count), and a
/// null pointer when there's no room at all.
pub fn write_ptr_arr(
    b: &mut BlockBuf,
    w: &mut Rsc8Writer,
    items: Option<&[Option<BlockId>]>,
    capacity: u16,
    unknown: u32,
) {
    let count = items.map_or(0, |i| i.len() as u16);
    let slots = count.max(capacity) as usize;
    let ptrs = match items {
        Some(items) if slots > 0 => {
            let mut ids = items.to_vec();
            ids.resize(slots, None);
            Some(w.pointer_array(&ids))
        }
        _ => None,
    };
    b.ptr(ptrs);
    b.u16(count);
    b.u16(capacity);
    b.u32(unknown);
}

/// `Rsc8Arr.Write` of plain data: a pointer to the items (non-null even when
/// empty), the count and capacity.
pub fn write_arr<T: Pod>(b: &mut BlockBuf, w: &mut Rsc8Writer, items: Option<&[T]>, capacity: u16, unknown: u32) {
    b.ptr(w.array(items));
    b.u16(items.map_or(0, |i| i.len() as u16));
    b.u16(capacity);
    b.u32(unknown);
}

/// The items of an XML node array, failing on `type="null"` items where the
/// C# would fail on them later.
pub fn node_items<'a, 'i, T>(
    node: XmlNode<'a, 'i>,
    id: &str,
    mut f: impl FnMut(XmlNode<'a, 'i>) -> Result<T>,
) -> Result<Option<Vec<T>>> {
    let Some(items) = node.node_array(id) else { return Ok(None) };
    let mut v = Vec::with_capacity(items.len());
    for item in items {
        let Some(item) = item else { bail!("a null item in {id} (the C# fails on it)") };
        v.push(f(item)?);
    }
    Ok(Some(v))
}

/// The items of an XML node array, keeping `type="null"` items as `None`.
pub fn node_items_opt<'a, 'i, T>(
    node: XmlNode<'a, 'i>,
    id: &str,
    mut f: impl FnMut(XmlNode<'a, 'i>) -> Result<T>,
) -> Result<Option<Vec<Option<T>>>> {
    let Some(items) = node.node_array(id) else { return Ok(None) };
    let mut v = Vec::with_capacity(items.len());
    for item in items {
        v.push(item.map(&mut f).transpose()?);
    }
    Ok(Some(v))
}

/// One field of `WriteStruct`: written only when not the default.
pub enum StructField<'a> {
    Vec4(&'a str, [f32; 4]),
    U16(&'a str, u16),
    U32(&'a str, u32),
}

/// `WriteStructArray`: nothing for `None`, `<id />` when empty, otherwise one
/// `Item` per struct with its non-default fields.
pub fn write_struct_array<T>(
    w: &mut XmlWriter,
    id: &str,
    items: Option<&[T]>,
    fields: impl Fn(&T) -> Vec<StructField<'static>>,
) {
    let Some(items) = items else { return };
    if items.is_empty() {
        w.element(&format!("<{id} />"));
        return;
    }
    w.node(id, |w| {
        for item in items {
            w.node("Item", |w| {
                for f in fields(item) {
                    match f {
                        StructField::Vec4(n, v) if v != [0.0; 4] => {
                            let [x, y, z, ww] = v.map(f32_to_string);
                            w.element(&format!("<{n} x=\"{x}\" y=\"{y}\" z=\"{z}\" w=\"{ww}\" />"));
                        }
                        StructField::U16(n, v) if v != 0 => w.u16(n, v),
                        StructField::U32(n, v) if v != 0 => w.u32(n, v),
                        _ => {}
                    }
                }
            });
        }
    });
}

/// `ReadStructArray`: `None` when the node is missing, otherwise one struct
/// per `Item` (an empty node gives an empty array).
pub fn read_struct_array<'a, 'i, T>(
    node: XmlNode<'a, 'i>,
    id: &str,
    f: impl Fn(XmlNode<'a, 'i>) -> T,
) -> Option<Vec<T>> {
    let c = node.child(id)?;
    Some(c.children("Item").map(f).collect())
}

/// .NET's `ToString` of a `[Flags]` enum: the names of the set flags in
/// ascending order, `names[0]` (value 0) for 0, or the number when some bits
/// have no name.
pub fn flags_name(names: &[(&str, u32)], v: u32) -> String {
    if v == 0 {
        return names.iter().find(|(_, x)| *x == 0).map_or("0".into(), |(n, _)| n.to_string());
    }
    let mut rest = v;
    let mut used = Vec::new();
    for &(n, x) in names.iter().rev() {
        if x != 0 && rest & x == x {
            used.push(n);
            rest &= !x;
        }
    }
    if rest != 0 {
        return v.to_string();
    }
    used.reverse();
    used.join(", ")
}

/// .NET's `Enum.TryParse` of a `[Flags]` enum: a number, or comma-separated
/// names (case-sensitive) or-ed together.
pub fn parse_flags(names: &[(&str, u32)], s: Option<&str>) -> Option<u32> {
    let s = s?.trim();
    if s.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+') {
        let t = s.strip_prefix('+').unwrap_or(s);
        return t.parse::<u32>().ok().or_else(|| t.parse::<i32>().ok().map(|v| v as u32));
    }
    let mut v = 0;
    for part in s.split(',') {
        let part = part.trim();
        v |= names.iter().find(|(n, _)| *n == part)?.1;
    }
    Some(v)
}

/// `ReadVector4(id, def)`: the default when the node is missing, and each
/// missing attribute from the default.
pub fn read_vec4_or(node: XmlNode, id: &str, def: [f32; 4]) -> [f32; 4] {
    let Some(c) = node.child(id) else { return def };
    let mut v = def;
    for (d, a) in v.iter_mut().zip(["x", "y", "z", "w"]) {
        if let Some(f) = c.attr(a).and_then(crate::xml::number::parse_f32) {
            *d = f;
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: &[(&str, u32)] = &[("None", 0), ("A", 1), ("B", 2), ("C", 4)];

    #[test]
    fn flags_like_dotnet() {
        assert_eq!(flags_name(NAMES, 0), "None");
        assert_eq!(flags_name(NAMES, 5), "A, C");
        assert_eq!(flags_name(NAMES, 9), "9");
        assert_eq!(parse_flags(NAMES, Some("A, C")), Some(5));
        assert_eq!(parse_flags(NAMES, Some("12")), Some(12));
        assert_eq!(parse_flags(NAMES, Some("a")), None);
    }
}
