//! Language databases (yldb), from `Rsc8Language.cs`: hash → text tables, stored
//! as a hash map (`Rsc8DataMap`) of entries.
//!
//! Also home to small RSC8 pieces the yfd and ypmd ports share: `Rsc8StrA`, the
//! `Rsc8DataMap` bucket builder, and the text escaping and reading that the C#
//! does through `System.Xml`.

use super::XmlContext;
use crate::hash::joaat_bytes;
use crate::rsc8::{ArrayHeader32, BlockBuf, BlockId, BlockMap, Rsc8Reader, Rsc8Writer, read_resource};
use crate::xml::{XmlDocument, XmlNode, XmlWriter, parse_hash};
use crate::{Result, bail};

/// The resource version `YldbFile.Save` builds with.
const VERSION: u32 = 5;

/// `Rsc8LanguageDatabase`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LanguageDatabase {
    pub vft: u64,
    pub block_map: Option<BlockMap>,
    /// The hash map's buckets: each is the chain of entries linked by `Next`.
    pub language_map: Vec<Vec<LanguageEntry>>,
    /// The bucket array's capacity field (0 when the C# builds the map).
    pub language_map_capacity: u32,
    pub language_map_count: u32,
    /// Always 0x1000000 in the game files.
    pub unknown_24h: u32,
    pub unknown_28h: u64,
}

/// `Rsc8LanguageEntry`, without its `Next` pointer (the bucket's order is the chain).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LanguageEntry {
    pub hash: u32,
    pub unknown_04h: u32,
    pub item: Option<LanguageItem>,
    pub unknown_18h: u64,
}

/// `Rsc8LanguageItem`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LanguageItem {
    pub unknown_00h: u32,
    pub unknown_04h: u32,
    pub unknown_08h: u32,
    pub unknown_0ch: u32,
    pub string: StrA,
}

impl LanguageDatabase {
    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let vft = r.u64()?;
        let block_map = r.ptr(BlockMap::read)?;
        let map = ArrayHeader32::read(r)?;
        let language_map_count = r.u32()?;
        let unknown_24h = r.u32()?;
        let unknown_28h = r.u64()?;
        if map.ptr == 0 && map.count > 0 {
            bail!("yldb: a null language map of {} buckets (the C# fails on it)", map.count);
        }
        let heads = r.array_at::<u64>(map.ptr, map.count as usize)?.unwrap_or_default();
        let mut language_map = Vec::with_capacity(heads.len());
        for head in heads {
            language_map.push(read_chain(r, head, LanguageEntry::read)?);
        }
        Ok(Self {
            vft,
            block_map,
            language_map,
            language_map_capacity: map.capacity,
            language_map_count,
            unknown_24h,
            unknown_28h,
        })
    }

    pub fn write(&self) -> Vec<u8> {
        let mut w = Rsc8Writer::new();
        let root = w.reserve();
        // `YldbFile.Save` adds a block map when there is none.
        let block_map = self.block_map.clone().unwrap_or_default().write(&mut w);
        let heads: Vec<Option<BlockId>> = self.language_map.iter().map(|chain| write_chain(&mut w, chain)).collect();
        let count = self.language_map.len() as u32;
        // `Rsc8PtrArr32.Write`: the pointer array has max(count, capacity) slots.
        let slots = count.max(self.language_map_capacity) as usize;
        let map = (slots > 0).then(|| {
            let mut ptrs = heads.clone();
            ptrs.resize(slots, None);
            w.pointer_array(&ptrs)
        });

        let mut b = BlockBuf::with_capacity(0x30);
        b.u64(self.vft);
        b.ptr(Some(block_map));
        b.ptr(map);
        b.u32(count);
        b.u32(self.language_map_capacity);
        b.u32(self.language_map_count);
        b.u32(self.unknown_24h);
        b.u64(self.unknown_28h);
        w.set(root, b, false);
        w.build(VERSION)
    }

    /// `Write(MetaNodeWriter)`.
    pub fn write_xml(&self, w: &mut XmlWriter) {
        let entries: Vec<&LanguageEntry> = self.language_map.iter().flatten().collect();
        w.node_array("Entries", Some(&entries), |w, e| {
            w.hash("@hash", e.hash);
            let text = e.item.as_ref().and_then(|i| i.string.value.as_deref()).unwrap_or("");
            w.string("Text", &escape_text(text.trim_matches('\0')));
        });
    }

    /// `Read(MetaNodeReader)`.
    pub fn read_xml(node: XmlNode, cx: &XmlContext) -> Result<Self> {
        let Some(items) = node.node_array("Entries") else {
            bail!("yldb: no Entries (the C# fails with a NullReferenceException)")
        };
        let mut entries = Vec::with_capacity(items.len());
        for item in items {
            let Some(item) = item else { bail!("yldb: a null entry (the C# fails on it)") };
            entries.push(LanguageEntry::read_xml(item, cx));
        }
        let count = entries.len() as u32;
        let language_map = if entries.is_empty() {
            vec![Vec::new(); 11]
        } else {
            match build_map(entries, |e| e.hash) {
                Some(m) => m,
                None => bail!("yldb: a single entry with hash 0 (the C# fails on it)"),
            }
        };
        Ok(Self {
            vft: 0,
            block_map: None,
            language_map,
            language_map_capacity: 0,
            language_map_count: count,
            unknown_24h: 0x100_0000,
            unknown_28h: 0,
        })
    }
}

impl LanguageEntry {
    /// Reads the entry and returns its `Next` pointer too.
    fn read(r: &mut Rsc8Reader) -> Result<(Self, u64)> {
        let hash = r.u32()?;
        let unknown_04h = r.u32()?;
        let item = r.ptr(LanguageItem::read)?;
        let next = r.u64()?;
        let unknown_18h = r.u64()?;
        Ok((Self { hash, unknown_04h, item, unknown_18h }, next))
    }

    fn write(&self, w: &mut Rsc8Writer, next: Option<BlockId>) -> BlockId {
        let item = self.item.as_ref().map(|i| i.write(w));
        let mut b = BlockBuf::with_capacity(0x20);
        b.u32(self.hash);
        b.u32(self.unknown_04h);
        b.ptr(item);
        b.ptr(next);
        b.u64(self.unknown_18h);
        w.add(b, false)
    }

    fn read_xml(node: XmlNode, cx: &XmlContext) -> Self {
        let hash = read_hash(node, "@hash", cx);
        let text = read_string(node, "Text");
        let item = LanguageItem { string: StrA::new(text), ..Default::default() };
        Self { hash, unknown_04h: 0, item: Some(item), unknown_18h: 0 }
    }
}

impl LanguageItem {
    fn read(r: &mut Rsc8Reader) -> Result<Self> {
        Ok(Self {
            unknown_00h: r.u32()?,
            unknown_04h: r.u32()?,
            unknown_08h: r.u32()?,
            unknown_0ch: r.u32()?,
            string: StrA::read(r)?,
        })
    }

    fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let mut b = BlockBuf::with_capacity(0x20);
        b.u32(self.unknown_00h);
        b.u32(self.unknown_04h);
        b.u32(self.unknown_08h);
        b.u32(self.unknown_0ch);
        self.string.write(&mut b, w);
        w.add(b, false)
    }
}

pub fn read(file: &[u8]) -> Result<LanguageDatabase> {
    read_resource(file, |r, _| LanguageDatabase::read(r))
}

pub fn to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let db = read(file)?;
    Ok(XmlWriter::document(cx.hashes, "RDR2LanguageDatabase", None, |w| db.write_xml(w)))
}

pub fn from_xml(xml: &str, cx: &XmlContext) -> Result<Vec<u8>> {
    let doc = XmlDocument::new(xml);
    let db = doc.with_root(None, |root| LanguageDatabase::read_xml(root, cx))??;
    Ok(db.write())
}

// --- Shared with the other small RSC8 formats ---------------------------------

/// `Rsc8StrA`: a string pointer with u16 length and capacity.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StrA {
    pub length: u16,
    pub capacity: u16,
    pub unknown: u32,
    pub value: Option<String>,
}

impl StrA {
    /// `new Rsc8StrA(str)`. Like the C#, the length counts UTF-16 units, not the
    /// UTF-8 bytes written, so non-ASCII strings come back cut short when read.
    pub fn new(value: Option<String>) -> Self {
        let length = value.as_deref().map_or(0, |s| s.encode_utf16().count() as u16);
        let capacity = if length > 0 { (length as u32 + 1) as u16 } else { 0 };
        Self { length, capacity, unknown: 0, value }
    }

    /// `Rsc8StrA.Read`: `length` bytes of UTF-8 (none for a length of 0).
    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let ptr = r.u64()?;
        let length = r.u16()?;
        let capacity = r.u16()?;
        let unknown = r.u32()?;
        let value = if ptr == 0 { None } else { read_fixed_string(r, ptr, length as usize)? };
        Ok(Self { length, capacity, unknown, value })
    }

    /// The 16 bytes of the struct; the string goes in its own block.
    pub fn write(&self, b: &mut BlockBuf, w: &mut Rsc8Writer) {
        let s = self.value.as_deref().map(|s| w.string(s));
        b.ptr(s);
        b.u16(self.length);
        b.u16(self.capacity);
        b.u32(self.unknown);
    }
}

/// `ReadStringFixedLength`: at most `len` bytes (fewer at the end of the data),
/// decoded as UTF-8; `None` when there are none.
fn read_fixed_string(r: &mut Rsc8Reader, ptr: u64, len: usize) -> Result<Option<String>> {
    r.offset_of(ptr)?;
    if len == 0 {
        return Ok(None);
    }
    r.at(ptr, |r| {
        let bytes = match r.bytes(len) {
            Ok(b) => b.to_vec(),
            Err(_) => {
                let mut v = Vec::new();
                while v.len() < len {
                    match r.u8() {
                        Ok(c) => v.push(c),
                        Err(_) => break,
                    }
                }
                v
            }
        };
        Ok((!bytes.is_empty()).then(|| String::from_utf8_lossy(&bytes).into_owned()))
    })
}

/// Reads a chain of map entries linked by their `Next` pointers (`read` returns
/// the entry and its `Next`).
pub(crate) fn read_chain<T>(
    r: &mut Rsc8Reader,
    mut ptr: u64,
    mut read: impl FnMut(&mut Rsc8Reader) -> Result<(T, u64)>,
) -> Result<Vec<T>> {
    let mut chain = Vec::new();
    while ptr != 0 {
        if chain.len() > 0x10_0000 {
            bail!("a map's chain of entries loops at 0x{ptr:X}");
        }
        let (e, next) = r.at(ptr, &mut read)?;
        chain.push(e);
        ptr = next;
    }
    Ok(chain)
}

/// Writes a chain of map entries back to front, each pointing to the next one,
/// and returns the first.
fn write_chain(w: &mut Rsc8Writer, chain: &[LanguageEntry]) -> Option<BlockId> {
    chain.iter().rev().fold(None, |next, e| Some(e.write(w, next)))
}

/// The bucket counts `Rsc8DataMap.Build` chooses from.
const BUCKET_COUNTS: [usize; 42] = [
    11, 29, 53, 103, 211, 409, 647, 821, 1031, 1291, 1627, 2053, 2591, 3251, 4099, 5167, 6521, 8209, 10331, 13007,
    16411, 20663, 26017, 32771, 41299, 52021, 65537, 82571, 104033, 131101, 165161, 208067, 262147, 330287, 416147,
    524309, 660563, 832291, 1048583, 1321139, 1664543, 2097169,
];

/// `Rsc8DataMap.Build` (no sorting): entries in buckets by `key % bucket count`,
/// each bucket in the given order. `None` where the C# returns null: no
/// entries, or a single one with key 0.
pub(crate) fn build_map<T>(entries: Vec<T>, key: impl Fn(&T) -> u32) -> Option<Vec<Vec<T>>> {
    if entries.is_empty() || (entries.len() == 1 && key(&entries[0]) == 0) {
        return None;
    }
    let n = BUCKET_COUNTS.iter().copied().find(|&b| entries.len() < b).unwrap_or(2097211);
    let mut buckets: Vec<Vec<T>> = (0..n).map(|_| Vec::new()).collect();
    for e in entries {
        let b = key(&e) as usize % n;
        buckets[b].push(e);
    }
    Some(buckets)
}

/// `Xml.Escape`: the text as `System.Xml` writes it inside an element.
pub(crate) fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {
                out.push_str(&format!("&#x{:X};", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// `ReadString` as the C# sees it: `XmlDocument` drops text that is only
/// whitespace, so such an element reads as empty.
pub(crate) fn read_string(node: XmlNode, id: &str) -> Option<String> {
    if id.starts_with('@') {
        return node.string(id);
    }
    let text = node.child(id)?.text();
    Some(if text.chars().all(|c| matches!(c, ' ' | '\t' | '\n' | '\r')) { String::new() } else { text })
}

/// `name.Remove(0, start).Remove(len - end, end)`, counting UTF-16 units like
/// the C#; `None` where the C# throws (a name too short).
pub(crate) fn trim_name(name: &str, start: usize, end: usize) -> Option<String> {
    let units: Vec<u16> = name.encode_utf16().collect();
    if units.len() < start + end {
        return None;
    }
    Some(String::from_utf16_lossy(&units[start..units.len() - end]))
}

/// `JenkHash.GenHash` (`new JenkHash(str)`): each UTF-16 unit truncated to a byte.
pub(crate) fn gen_hash(s: &str) -> u32 {
    let bytes: Vec<u8> = s.encode_utf16().map(|u| u as u8).collect();
    joaat_bytes(&bytes)
}

/// `ReadJenkHash`, which adds the name it reads to the hash index.
pub(crate) fn read_hash(node: XmlNode, id: &str, cx: &XmlContext) -> u32 {
    match read_string(node, id) {
        Some(s) => {
            cx.hashes.ensure(&s);
            parse_hash(&s)
        }
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::HashIndex;

    fn cx(hashes: &HashIndex) -> XmlContext<'_> {
        XmlContext { hashes, folder: None }
    }

    #[test]
    fn escapes_like_system_xml() {
        assert_eq!(escape_text("a<b>&c \"'\t\n"), "a&lt;b&gt;&amp;c \"'\t\n");
        assert_eq!(escape_text("x\u{1}y"), "x&#x1;y");
    }

    #[test]
    fn map_buckets_by_hash() {
        let m = build_map(vec![3u32, 14, 25, 1], |e| *e).unwrap();
        assert_eq!(m.len(), 11);
        assert_eq!(m[3], vec![3, 14, 25]);
        assert_eq!(m[1], vec![1]);
        assert!(build_map(vec![0u32], |e| *e).is_none());
        assert_eq!(build_map((0..11u32).collect(), |e| *e).unwrap().len(), 29);
    }

    #[test]
    fn round_trips_through_xml() {
        let hashes = HashIndex::new();
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<RDR2LanguageDatabase>
 <Entries>
  <Item hash="0x0000000C">
   <Text>~z~Hello &amp; goodbye</Text>
  </Item>
  <Item hash="0x00000017">
   <Text>Sporgerò</Text>
  </Item>
  <Item hash="0x00000001">
   <Text> </Text>
  </Item>
 </Entries>
</RDR2LanguageDatabase>
"#;
        let file = from_xml(xml, &cx(&hashes)).unwrap();
        let db = read(&file).unwrap();
        assert_eq!(db.language_map.len(), 11);
        assert_eq!(db.language_map_count, 3);
        assert_eq!(db.unknown_24h, 0x100_0000);
        // 12, 23 and 1 all go in bucket 1 (of 11), chained in the XML's order.
        let chain: Vec<u32> = db.language_map[1].iter().map(|e| e.hash).collect();
        assert_eq!(chain, vec![12, 23, 1]);
        let text = |i: usize| db.language_map[1][i].item.as_ref().unwrap().string.value.clone();
        assert_eq!(text(0).as_deref(), Some("~z~Hello & goodbye"));
        // The length counts UTF-16 units, so the last byte of "ò" is cut off.
        assert_eq!(text(1).as_deref(), Some("Sporger\u{FFFD}"));
        // Whitespace-only text reads as empty, and an empty string as none.
        assert_eq!(text(2), None);

        let out = to_xml(&file, &cx(&hashes)).unwrap();
        assert!(out.contains("   <Text>~z~Hello &amp; goodbye</Text>\n"), "{out}");
        assert!(out.contains("   <Text />\n"), "{out}");
    }
}
