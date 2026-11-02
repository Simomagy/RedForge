//! Bits of .NET and of RedForge's C# helpers that the animation formats depend on
//! for their exact output: `Rsc8DataMap` hash buckets, Linux `Path` functions,
//! `Enum.TryParse`, the small-array `List.Sort`, LINQ `Min`/`Max`.

use crate::rsc8::{BlockBuf, BlockId, Rsc8Reader, Rsc8Writer};
use crate::{Result, bail};

/// `Rsc8DataMap.Build`'s bucket count for `n` keys.
pub fn bucket_count(n: usize) -> usize {
    const SIZES: [usize; 42] = [
        11, 29, 53, 103, 211, 409, 647, 821, 1031, 1291, 1627, 2053, 2591, 3251, 4099, 5167, 6521, 8209, 10331, 13007,
        16411, 20663, 26017, 32771, 41299, 52021, 65537, 82571, 104033, 131101, 165161, 208067, 262147, 330287, 416147,
        524309, 660563, 832291, 1048583, 1321139, 1664543, 2097169,
    ];
    SIZES.iter().copied().find(|&s| n < s).unwrap_or(2097211)
}

/// `Rsc8DataMap.Build`: the buckets, each the chain of entry indices in order.
/// `None` where the C# returns null (no entries, or a single one with key 0),
/// which its callers then crash on.
pub fn build_buckets(keys: &[u64]) -> Option<Vec<Vec<usize>>> {
    if keys.is_empty() || (keys.len() == 1 && keys[0] == 0) {
        return None;
    }
    let n = bucket_count(keys.len());
    let mut buckets = vec![Vec::new(); n];
    for (i, k) in keys.iter().enumerate() {
        buckets[(k % n as u64) as usize].push(i);
    }
    Some(buckets)
}

/// Writes a hash map the way the C# builds one from a list (`Rsc8DataMap.Build`
/// with no sorting): one entry block per item, chained per bucket, and the
/// `Rsc8PtrArr` of bucket heads (`Count` = buckets, `Capacity` = 0) into `b`.
/// With no items, the array is 11 null pointers. `entry` writes an entry block
/// given the entry's index and the next entry in its chain.
pub fn write_map(
    w: &mut Rsc8Writer,
    b: &mut BlockBuf,
    keys: &[u64],
    what: &str,
    mut entry: impl FnMut(&mut Rsc8Writer, usize, Option<BlockId>) -> BlockId,
) -> Result<()> {
    let heads: Vec<Option<BlockId>> = if keys.is_empty() {
        vec![None; 11]
    } else {
        let Some(buckets) = build_buckets(keys) else {
            bail!("{what}: a single entry with hash 0 (the C# can't build that map)")
        };
        buckets
            .iter()
            .map(|chain| {
                let mut next = None;
                for &i in chain.iter().rev() {
                    next = Some(entry(w, i, next));
                }
                next
            })
            .collect()
    };
    let arr = w.pointer_array(&heads);
    b.ptr(Some(arr));
    b.u16(heads.len() as u16);
    b.u16(0);
    b.u32(0);
    Ok(())
}

/// `Rsc8PtrArr.Write` for a list built from items (`Count` = `Capacity` = the
/// length): the header into `b`, and the pointer array when there are items.
pub fn write_ptr_arr(w: &mut Rsc8Writer, b: &mut BlockBuf, items: Option<&[Option<BlockId>]>) {
    let count = items.map_or(0, |v| v.len());
    let arr = match items {
        Some(v) if !v.is_empty() => Some(w.pointer_array(v)),
        _ => None,
    };
    b.ptr(arr);
    b.u16(count as u16);
    b.u16(count as u16);
    b.u32(0);
}

/// `Rsc8StrA.Write`: pointer, length and capacity (in UTF-16 units, as the C#
/// counts them), 0.
pub fn write_str_a(w: &mut Rsc8Writer, b: &mut BlockBuf, s: Option<&str>) {
    let len = s.map_or(0, |s| s.encode_utf16().count()) as u16;
    b.ptr(w.opt_string(s));
    b.u16(len);
    b.u16(if len > 0 { len + 1 } else { 0 });
    b.u32(0);
}

/// A `Rsc8StrA`: pointer, length, capacity and the string, read like the C#
/// (`length` bytes, no string for a length of 0).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StrA {
    pub ptr: u64,
    pub length: u16,
    pub capacity: u16,
    pub unknown: u32,
    pub value: Option<String>,
}

impl StrA {
    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let ptr = r.u64()?;
        let length = r.u16()?;
        let capacity = r.u16()?;
        let unknown = r.u32()?;
        let value = if ptr == 0 { None } else { fixed_string(r, ptr, length as usize)? };
        Ok(Self { ptr, length, capacity, unknown, value })
    }
}

/// `ReadStringFixedLength`: `len` bytes (fewer at the end of the data) as UTF-8,
/// embedded NULs included; no string for a length of 0.
fn fixed_string(r: &mut Rsc8Reader, ptr: u64, len: usize) -> Result<Option<String>> {
    if len == 0 {
        return Ok(None);
    }
    let bytes: Vec<u8> = match r.at(ptr, |r| r.bytes(len).map(<[u8]>::to_vec)) {
        Ok(b) => b,
        Err(_) => {
            // Past the end of the data: the C# reads what there is.
            r.offset_of(ptr)?;
            let mut v = Vec::new();
            r.at(ptr, |r| {
                while v.len() < len {
                    match r.u8() {
                        Ok(b) => v.push(b),
                        Err(_) => break,
                    }
                }
                Ok(())
            })?;
            v
        }
    };
    if bytes.is_empty() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// A `Rsc8Str`: a pointer to a NUL-terminated string.
pub fn read_str(r: &mut Rsc8Reader) -> Result<Option<String>> {
    let ptr = r.u64()?;
    r.string_at(ptr)
}

/// `Rsc8Arr`/`Rsc8PtrArr` headers read their items with `ReadArray`, which gives
/// null for a count of 0 (and for a null pointer).
pub fn read_array<T: bytemuck::Pod>(r: &Rsc8Reader, ptr: u64, count: usize) -> Result<Option<Vec<T>>> {
    if ptr == 0 || count == 0 {
        return Ok(None);
    }
    r.array_at(ptr, count)
}

/// .NET's `Path.GetFileName` on Linux: what follows the last `/`.
pub fn file_name(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// .NET's `Path.GetFileNameWithoutExtension` on Linux.
pub fn file_name_without_extension(path: &str) -> &str {
    let f = file_name(path);
    match f.rfind('.') {
        Some(i) => &f[..i],
        None => f,
    }
}

/// `Path.GetFileName(Path.GetDirectoryName(path))` on Linux: the name of the
/// folder a file is in; `None` where `GetDirectoryName` gives null.
pub fn folder_name(path: &str) -> Option<&str> {
    let b = path.as_bytes();
    if b.is_empty() {
        return None;
    }
    let root = usize::from(b[0] == b'/');
    let mut end = b.len();
    if end <= root {
        return None;
    }
    while end > root {
        end -= 1;
        if b[end] == b'/' {
            break;
        }
    }
    while end > root && b[end - 1] == b'/' {
        end -= 1;
    }
    Some(file_name(&path[..end]))
}

/// `Enum.TryParse`: a name, a number, or names separated by commas (or-ed).
pub fn parse_enum(text: Option<&str>, names: &[(&str, i64)]) -> Option<i64> {
    let t = text?.trim();
    let first = t.chars().next()?;
    if first.is_ascii_digit() || first == '-' || first == '+' {
        return t.strip_prefix('+').unwrap_or(t).parse().ok();
    }
    let mut v = 0;
    for part in t.split(',') {
        let p = part.trim();
        v |= names.iter().find(|(n, _)| *n == p)?.1;
    }
    Some(v)
}

/// An enum value's name as .NET's `ToString` gives it: the name, or the number.
pub fn enum_name(v: i64, names: &[(&str, i64)]) -> String {
    match names.iter().find(|(_, x)| *x == v) {
        Some((n, _)) => (*n).to_owned(),
        None => v.to_string(),
    }
}

/// .NET's `List<T>.Sort` for up to 16 items (its introsort's small cases):
/// 2 and 3 items with compare-and-swaps, more with an insertion sort.
pub fn net_sort<T>(v: &mut [T], cmp: impl Fn(&T, &T) -> std::cmp::Ordering) {
    use std::cmp::Ordering::{Greater, Less};
    let swap_if_greater = |v: &mut [T], i: usize, j: usize| {
        if cmp(&v[i], &v[j]) == Greater {
            v.swap(i, j);
        }
    };
    match v.len() {
        0 | 1 => {}
        2 => swap_if_greater(v, 0, 1),
        3 => {
            swap_if_greater(v, 0, 1);
            swap_if_greater(v, 0, 2);
            swap_if_greater(v, 1, 2);
        }
        n => {
            for i in 0..n - 1 {
                let mut j = i + 1;
                while j > 0 && cmp(&v[j], &v[j - 1]) == Less {
                    v.swap(j, j - 1);
                    j -= 1;
                }
            }
        }
    }
}

/// LINQ `Min` over floats: NaN if there is one, otherwise the first smallest.
pub fn linq_min(v: &[f32]) -> f32 {
    let mut m = v[0];
    if m.is_nan() {
        return m;
    }
    for &x in &v[1..] {
        if x < m {
            m = x;
        } else if x.is_nan() {
            return x;
        }
    }
    m
}

/// LINQ `Max` over floats: NaNs are skipped unless all are NaN.
pub fn linq_max(v: &[f32]) -> f32 {
    let mut i = 0;
    let mut m = v[0];
    while m.is_nan() {
        i += 1;
        if i >= v.len() {
            return m;
        }
        m = v[i];
    }
    for &x in &v[i + 1..] {
        if x > m {
            m = x;
        }
    }
    m
}

/// The number of bits needed for a value (`Rsc8AnimSequenceWriter.BitCount`).
pub fn bit_count(v: u32) -> i32 {
    32 - v.leading_zeros() as i32
}

/// `BitCount` of the largest of the values.
pub fn bit_count_max(v: &[u32]) -> i32 {
    bit_count(v.iter().copied().max().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_like_dotnet_on_linux() {
        assert_eq!(file_name_without_extension("pack:/exit_left.clip"), "exit_left");
        assert_eq!(file_name_without_extension("a.b/c"), "c");
        assert_eq!(folder_name("pack:/exit_left.clip"), Some("pack:"));
        assert_eq!(folder_name("x/y//z.clip"), Some("y"));
        assert_eq!(folder_name("z.clip"), Some(""));
        assert_eq!(folder_name("/z.clip"), Some(""));
        assert_eq!(folder_name(""), None);
    }

    #[test]
    fn enums_parse_like_dotnet() {
        let names = [("Single", 1), ("Multi", 2)];
        assert_eq!(parse_enum(Some("Multi"), &names), Some(2));
        assert_eq!(parse_enum(Some(" 7 "), &names), Some(7));
        assert_eq!(parse_enum(Some("Single, Multi"), &names), Some(3));
        assert_eq!(parse_enum(Some("multi"), &names), None);
        assert_eq!(parse_enum(None, &names), None);
        assert_eq!(enum_name(5, &names), "5");
    }

    #[test]
    fn small_sorts_like_dotnet() {
        // Three items: compare-and-swaps, which aren't stable.
        let mut v = [(1, 'x'), (1, 'y'), (0, 'z')];
        net_sort(&mut v, |a, b| a.0.cmp(&b.0));
        assert_eq!(v, [(0, 'z'), (1, 'y'), (1, 'x')]);
        let mut v = [(2, 'a'), (1, 'b'), (1, 'c'), (0, 'd')];
        net_sort(&mut v, |a, b| a.0.cmp(&b.0));
        assert_eq!(v, [(0, 'd'), (1, 'b'), (1, 'c'), (2, 'a')]);
    }
}
