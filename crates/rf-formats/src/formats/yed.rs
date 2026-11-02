//! Expression dictionaries (yed), from the C#'s `Rsc8Expression.cs` and
//! `YedFile.cs`.
//!
//! An [`ExpressionDictionary`] holds [`Expression`]s by the hash of their short
//! names. An expression is a set of small stack programs ([`ExpressionStream`]s
//! of [`ExpressionOp`]s) that drive bone tracks from other tracks and from
//! variables, plus the tracks it touches ([`ExpressionTrack`]), springs
//! ([`ExpressionSpring`]) and a clip property map (from [`super::ycd`]).
//!
//! In the file, a stream's instructions are split over three buffers: one byte
//! per instruction for its type, and two data buffers that each type reads
//! from. [`ExpressionStream::encode`] lays them out again.
//!
//! As in the C#, an expression read from XML gets its variables list (the
//! distinct variable hashes, sorted, with each instruction's index into it),
//! its largest stream size and its jumps' buffer offsets computed; the
//! dictionary's name isn't in the XML, so files written from XML have none.
//!
//! C# behaviour this keeps:
//! - Files are written with resource version 25 (the game's are 30), and each
//!   expression's owner as the root's address, 0x50000000.
//! - A jump's offsets in the two data buffers are the distance from the jump's
//!   data to the target's. The game counts differently (its buffer 2 offset is
//!   usually 12 less, sometimes 4), so jumps in files written from XML don't
//!   match the game's. The XML only has the instruction offset, which is right.
//! - An expression from XML always has a variables array, empty or not (a
//!   pointer to an empty block when it has none).
//!
//! Where the C# fails reading the XML, this does too: no `Expressions`, an
//! expression without `Properties` (the XML of one with no property map; no
//! game file has one), null streams or instructions, unknown
//! instruction types, jumps past the end of their stream, and a dictionary of a
//! single expression whose short name hashes to 0.
//!
//! Known differences from the C#, none of which the game's files hit:
//! - Reading past the end of a stream's data fails here, where the C# zero-pads
//!   its struct array reads (`ReadStructs`) and only fails on scalar reads.
//! - The C#'s block pool isn't emulated: an array or stream referenced twice is
//!   read twice, which gives the same XML.

use std::collections::HashSet;

use bytemuck::{Pod, Zeroable};

use super::XmlContext;
use super::ycd::util::{StrA, build_buckets, enum_name, file_name_without_extension, parse_enum, read_array};
use super::ycd::util::{write_map, write_ptr_arr, write_str_a};
use super::ycd::{ClipPropertyMap, ReadCx, read_properties_xml, write_properties_xml};
use crate::hash::{HashIndex, joaat};
use crate::rsc8::{ArrayHeader, BlockBuf, BlockId, BlockMap, Rsc8Reader, Rsc8Writer, VIRTUAL_BASE, read_resource};
use crate::xml::{XmlDocument, XmlNode, XmlWriter};
use crate::{Result, bail};

/// The resource version of yed files.
pub const VERSION: u32 = 25;

/// Reads a loose yed file (with its RSC8 header), learning names as the C# does.
pub fn read(file: &[u8], hashes: &HashIndex) -> Result<ExpressionDictionary> {
    read_resource(file, |r, _| ExpressionDictionary::read(r, VIRTUAL_BASE, &mut ReadCx::new(hashes)))
}

/// A yed (loose, with its RSC8 header) to XML.
pub fn to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let d = read(file, cx.hashes)?;
    Ok(XmlWriter::document(cx.hashes, "RDR2ExpressionDictionary", None, |w| d.write_xml(w)))
}

/// XML to a yed.
pub fn from_xml(xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    let d = XmlDocument::new(xml).with_root(None, |n| ExpressionDictionary::read_xml(&n))??;
    d.write_file()
}

// --- Dictionary -----------------------------------------------------------------

/// `Rsc8ExpressionDictionary`, 0x70 bytes: the root of a yed.
#[derive(Clone, Debug, PartialEq)]
pub struct ExpressionDictionary {
    pub vft: u64,
    /// The block map's address, as read.
    pub block_map: u64,
    pub unknown_10h: u64,
    pub unknown_18h: u64,
    pub unknown_20h: u64,
    pub unknown_28h: u64,
    pub unknown_30h: u64,
    /// `None` when it came from XML, which doesn't have it.
    pub name: Option<String>,
    /// The entries in order (by bucket, then along each bucket's chain).
    pub entries: Vec<ExpressionDictionaryEntry>,
    pub entries_count: u32,
    pub unknown_5ch: u32,
    pub unknown_60h: u64,
    pub name_hash: u32,
    pub unknown_6ch: u32,
}

impl Default for ExpressionDictionary {
    fn default() -> Self {
        Self {
            vft: 0,
            block_map: 0,
            unknown_10h: 0,
            unknown_18h: 0,
            unknown_20h: 0,
            unknown_28h: 0,
            unknown_30h: 0,
            name: None,
            entries: Vec::new(),
            entries_count: 0,
            unknown_5ch: 0x0100_0000,
            unknown_60h: 0,
            name_hash: 0,
            unknown_6ch: 0,
        }
    }
}

/// `Rsc8ExpressionDictionaryEntry`, 0x20 bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExpressionDictionaryEntry {
    /// The hash of the expression's short name (its name without folder and
    /// extension).
    pub hash: u32,
    pub unknown_04h: u32,
    pub expression: Option<Expression>,
    pub unknown_18h: u64,
}

impl ExpressionDictionary {
    pub fn read(r: &mut Rsc8Reader, ptr: u64, cx: &mut ReadCx) -> Result<Self> {
        r.at(ptr, |r| {
            let mut d = ExpressionDictionary {
                vft: r.u64()?,
                block_map: r.u64()?,
                unknown_10h: r.u64()?,
                unknown_18h: r.u64()?,
                unknown_20h: r.u64()?,
                unknown_28h: r.u64()?,
                unknown_30h: r.u64()?,
                ..Default::default()
            };
            let name = StrA::read(r)?.value.unwrap_or_default();
            let h = ArrayHeader::read(r)?;
            // `Rsc8DataMap.Flatten`: each bucket's chain in turn.
            for head in read_array::<u64>(r, h.ptr, h.count as usize)?.unwrap_or_default() {
                let mut p = head;
                let mut seen = HashSet::new();
                while p != 0 {
                    if !seen.insert(p) {
                        bail!("expression dictionary chain loops at 0x{p:X}");
                    }
                    let (entry, next) = r.at(p, |r| {
                        let hash = r.u32()?;
                        let unknown_04h = r.u32()?;
                        let e = r.u64()?;
                        let expression = if e == 0 { None } else { Some(Expression::read(r, e, cx)?) };
                        let next = r.u64()?;
                        Ok((ExpressionDictionaryEntry { hash, unknown_04h, expression, unknown_18h: r.u64()? }, next))
                    })?;
                    d.entries.push(entry);
                    p = next;
                }
            }
            d.entries_count = r.u32()?;
            d.unknown_5ch = r.u32()?;
            d.unknown_60h = r.u64()?;
            d.name_hash = r.u32()?;
            d.unknown_6ch = r.u32()?;
            cx.hashes.ensure(file_name_without_extension(&name));
            d.name = Some(name);
            Ok(d)
        })
    }

    /// The expressions in order, as the C# lists them (`Expressions`).
    pub fn expressions(&self) -> Vec<Option<&Expression>> {
        self.entries.iter().map(|e| e.expression.as_ref()).collect()
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.u32("@version", 0);
        w.node_array_opt("Expressions", Some(&self.expressions()), |w, e| e.write_xml(w));
    }

    /// `Read(MetaNodeReader)`: `Build` from the expressions, failing where the
    /// C# fails. The entries end up in map order, as the C# lists them.
    pub fn read_xml(n: &XmlNode) -> Result<Self> {
        let Some(items) = n.node_array("Expressions") else { bail!("expression dictionary without expressions") };
        let mut entries = Vec::with_capacity(items.len());
        for i in items {
            let Some(i) = i else { bail!("null expression") };
            let e = Expression::read_xml(&i)?;
            let hash = joaat(file_name_without_extension(e.name.as_deref().unwrap_or("")));
            entries.push(ExpressionDictionaryEntry { hash, expression: Some(e), ..Default::default() });
        }
        let keys: Vec<u64> = entries.iter().map(|e| e.hash as u64).collect();
        let entries = if keys.is_empty() {
            entries
        } else {
            let Some(buckets) = build_buckets(&keys) else {
                bail!("expression dictionary: a single expression whose name hashes to 0 (the C# can't build that map)")
            };
            let mut slots: Vec<Option<ExpressionDictionaryEntry>> = entries.into_iter().map(Some).collect();
            buckets.into_iter().flatten().map(|i| slots[i].take().unwrap()).collect()
        };
        Ok(ExpressionDictionary { entries_count: entries.len() as u32, entries, ..Default::default() })
    }

    /// Builds the file, like `YedFile.Save`.
    pub fn write_file(&self) -> Result<Vec<u8>> {
        let mut w = Rsc8Writer::new();
        let root = w.reserve();
        let bm = BlockMap::default().write(&mut w);
        let mut ids = Vec::with_capacity(self.entries.len());
        for e in &self.entries {
            ids.push(e.expression.as_ref().map(|x| x.write(&mut w)).transpose()?);
        }
        let name = self.name.as_deref();
        let mut b = BlockBuf::with_capacity(0x70);
        b.u64(self.vft);
        b.ptr(Some(bm));
        b.u64(self.unknown_10h);
        b.u64(self.unknown_18h);
        b.u64(self.unknown_20h);
        b.u64(self.unknown_28h);
        b.u64(self.unknown_30h);
        write_str_a(&mut w, &mut b, name);
        let keys: Vec<u64> = self.entries.iter().map(|e| e.hash as u64).collect();
        write_map(&mut w, &mut b, &keys, "expression dictionary", |w, i, next| {
            let e = &self.entries[i];
            let mut eb = BlockBuf::with_capacity(0x20);
            eb.u32(e.hash);
            eb.u32(e.unknown_04h);
            eb.ptr(ids[i]);
            eb.ptr(next);
            eb.u64(e.unknown_18h);
            w.add(eb, false)
        })?;
        b.u32(self.entries_count);
        b.u32(self.unknown_5ch);
        b.u64(self.unknown_60h);
        b.u32(self.name_hash);
        b.u32(self.unknown_6ch);
        w.set(root, b, false);
        Ok(w.build(VERSION))
    }
}

// --- Expression -----------------------------------------------------------------

/// `Rsc8Expression`, 0xA0 bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Expression {
    pub vft: u64,
    pub unknown_08h: u64,
    pub usage_count: u64,
    /// The owner (the dictionary) as read; always written as 0x50000000.
    pub owner: u64,
    pub unknown_20h: u64,
    pub unknown_28h: u64,
    pub streams: Option<Vec<Option<ExpressionStream>>>,
    pub tracks: Option<Vec<ExpressionTrack>>,
    pub springs: Option<Vec<ExpressionSpring>>,
    /// The variables the instructions use, by hash (sorted when from XML).
    pub variables: Option<Vec<u32>>,
    pub name: Option<String>,
    pub unknown_80h: u32,
    pub signature: u32,
    /// The size of the largest stream block.
    pub max_stream_size: u32,
    pub unknown_8ch: u16,
    pub unknown_8eh: u16,
    pub property_map: Option<ClipPropertyMap>,
    pub unknown_98h: u64,
}

impl Default for Expression {
    fn default() -> Self {
        Self {
            vft: 0,
            unknown_08h: 0,
            usage_count: 1,
            owner: VIRTUAL_BASE,
            unknown_20h: 0,
            unknown_28h: 0,
            streams: None,
            tracks: None,
            springs: None,
            variables: None,
            name: None,
            unknown_80h: 0,
            signature: 0,
            max_stream_size: 0,
            unknown_8ch: 0,
            unknown_8eh: 0,
            property_map: None,
            unknown_98h: 0,
        }
    }
}

/// `Rsc8ExpressionTrack`: a bone track the expression reads or writes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct ExpressionTrack {
    pub bone_id: u16,
    pub track: u8,
    /// The type (0 Vector3, 1 Quaternion, 2 Float) in the low 7 bits, and a flag.
    pub type_and_flag: u8,
}

/// A vector and two values of an [`ExpressionSpring`] (`Vector01`, `Ushort01a`,
/// `Ushort01b` and so on).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct SpringPart {
    pub vector: [f32; 3],
    pub a: u16,
    pub b: u16,
}

/// `Rsc8ExpressionSpring`, 0xA0 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct ExpressionSpring {
    pub parts: [SpringPart; 9],
    pub gravity: [f32; 3],
    pub bone_id: u16,
    pub ushort10b: u16,
}

/// Reads an `Rsc8Arr` of plain data (null for a count of 0).
fn read_arr<T: Pod>(r: &mut Rsc8Reader) -> Result<Option<Vec<T>>> {
    let h = ArrayHeader::read(r)?;
    read_array(r, h.ptr, h.count as usize)
}

/// `Rsc8Arr.Write` for an array built from items: count = capacity = length,
/// and a pointer even to an empty array.
fn write_arr<T: Pod>(w: &mut Rsc8Writer, b: &mut BlockBuf, items: Option<&[T]>) {
    let n = items.map_or(0, <[T]>::len) as u16;
    b.ptr(w.array(items));
    b.u16(n);
    b.u16(n);
    b.u32(0);
}

impl Expression {
    pub fn read(r: &mut Rsc8Reader, ptr: u64, cx: &mut ReadCx) -> Result<Self> {
        r.at(ptr, |r| {
            let mut e = Expression {
                vft: r.u64()?,
                unknown_08h: r.u64()?,
                usage_count: r.u64()?,
                owner: r.u64()?,
                unknown_20h: r.u64()?,
                unknown_28h: r.u64()?,
                ..Default::default()
            };
            let h = ArrayHeader::read(r)?;
            if let Some(ptrs) = read_array::<u64>(r, h.ptr, h.count as usize)? {
                let mut v = Vec::with_capacity(ptrs.len());
                for p in ptrs {
                    v.push(if p == 0 { None } else { Some(ExpressionStream::read(r, p)?) });
                }
                e.streams = Some(v);
            }
            e.tracks = read_arr(r)?;
            e.springs = read_arr(r)?;
            e.variables = read_arr(r)?;
            e.name = StrA::read(r)?.value;
            e.unknown_80h = r.u32()?;
            e.signature = r.u32()?;
            e.max_stream_size = r.u32()?;
            e.unknown_8ch = r.u16()?;
            e.unknown_8eh = r.u16()?;
            let map = r.u64()?;
            e.property_map = if map == 0 { None } else { Some(ClipPropertyMap::read(r, map, cx)?) };
            e.unknown_98h = r.u64()?;
            cx.hashes.ensure(&e.short_name());
            Ok(e)
        })
    }

    /// The name without folder and extension, lowercase (`ShortName`).
    pub fn short_name(&self) -> String {
        file_name_without_extension(self.name.as_deref().unwrap_or("")).to_lowercase()
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.string("Name", self.name.as_deref().unwrap_or(""));
        w.u32("Signature", self.signature);
        w.u16("Unknown_8C", self.unknown_8ch);
        w.u16("Unknown_8E", self.unknown_8eh);
        w.node_array_opt("Streams", self.streams.as_deref(), |w, s| s.write_xml(w));
        w.node_array("Tracks", self.tracks.as_deref(), |w, t| {
            if t.bone_id != 0 {
                w.u16("BoneId", t.bone_id);
            }
            if t.track != 0 {
                w.u8("Track", t.track);
            }
            if t.type_and_flag != 0 {
                w.u8("TypeAndFlag", t.type_and_flag);
            }
        });
        w.node_array("Springs", self.springs.as_deref(), |w, s| {
            let vec3 = |w: &mut XmlWriter, id: &str, v: [f32; 3]| {
                // `Vector3 != default`: -0 counts as 0, NaN doesn't.
                if v.iter().any(|c| *c != 0.0) {
                    w.vec3(id, v);
                }
            };
            let u16 = |w: &mut XmlWriter, id: &str, v: u16| {
                if v != 0 {
                    w.u16(id, v);
                }
            };
            for (i, p) in s.parts.iter().enumerate() {
                vec3(w, &format!("Vector{:02}", i + 1), p.vector);
                u16(w, &format!("Ushort{:02}a", i + 1), p.a);
                u16(w, &format!("Ushort{:02}b", i + 1), p.b);
            }
            vec3(w, "Gravity", s.gravity);
            u16(w, "BoneId", s.bone_id);
            u16(w, "Ushort10b", s.ushort10b);
        });
        write_properties_xml(w, self.property_map.as_ref());
    }

    /// `Read(MetaNodeReader)`, then the C#'s `UpdateVariables`,
    /// `UpdateMaxStreamSize` and `UpdateJumpInstructions`.
    pub fn read_xml(n: &XmlNode) -> Result<Self> {
        let mut e = Expression {
            name: n.string("Name"),
            signature: n.u32("Signature", 0),
            unknown_8ch: n.u16("Unknown_8C", 0),
            unknown_8eh: n.u16("Unknown_8E", 0),
            ..Default::default()
        };
        if let Some(items) = n.node_array("Streams") {
            let mut v = Vec::with_capacity(items.len());
            for i in items {
                let Some(i) = i else { bail!("null expression stream") };
                v.push(Some(ExpressionStream::read_xml(&i)?));
            }
            e.streams = Some(v);
        }
        e.tracks = struct_array(n, "Tracks", |i| ExpressionTrack {
            bone_id: i.u16("BoneId", 0),
            track: i.u8("Track", 0),
            type_and_flag: i.u8("TypeAndFlag", 0),
        });
        e.springs = struct_array(n, "Springs", |i| {
            let mut s = ExpressionSpring::default();
            for (k, p) in s.parts.iter_mut().enumerate() {
                p.vector = i.vec3(&format!("Vector{:02}", k + 1));
                p.a = i.u16(&format!("Ushort{:02}a", k + 1), 0);
                p.b = i.u16(&format!("Ushort{:02}b", k + 1), 0);
            }
            s.gravity = i.vec3("Gravity");
            s.bone_id = i.u16("BoneId", 0);
            s.ushort10b = i.u16("Ushort10b", 0);
            s
        });
        let Some(props) = read_properties_xml(n)? else {
            bail!("expression {:?} without properties (the C# can't read that)", e.name.as_deref().unwrap_or(""))
        };
        e.property_map = Some(ClipPropertyMap::new(props));
        e.update_variables();
        e.update_max_stream_size();
        e.update_jumps()?;
        Ok(e)
    }

    fn streams_mut(&mut self) -> impl Iterator<Item = &mut ExpressionStream> {
        self.streams.iter_mut().flatten().flatten()
    }

    /// `UpdateVariables`: the distinct variables, sorted by hash, and each
    /// variable instruction's index into them.
    pub fn update_variables(&mut self) {
        let mut vars: Vec<u32> = self
            .streams
            .iter()
            .flatten()
            .flatten()
            .flat_map(|s| &s.instructions)
            .filter_map(|i| match i.data {
                OpData::Variable { variable, .. } => Some(variable),
                _ => None,
            })
            .collect();
        vars.sort_unstable();
        vars.dedup();
        for s in self.streams_mut() {
            for i in &mut s.instructions {
                if let OpData::Variable { variable, variable_index, .. } = &mut i.data {
                    *variable_index = vars.binary_search(variable).unwrap_or(0) as u16;
                }
            }
        }
        self.variables = Some(vars);
    }

    /// `UpdateMaxStreamSize`: the size of the largest stream block.
    pub fn update_max_stream_size(&mut self) {
        self.max_stream_size =
            self.streams.iter().flatten().flatten().map(ExpressionStream::block_length).max().unwrap_or(0);
    }

    /// `UpdateJumpInstructions`: each jump's offsets in the three buffers, from
    /// its instruction offset. Fails for a jump past the end, as the C# does.
    pub fn update_jumps(&mut self) -> Result<()> {
        for s in self.streams_mut() {
            let offsets = s.encode().offsets;
            let n = s.instructions.len();
            for (i, op) in s.instructions.iter_mut().enumerate() {
                if let OpData::Jump { data1_offset, data2_offset, data3_offset, instruction_offset } = &mut op.data {
                    let t = i as u64 + *instruction_offset as u64;
                    if t >= n as u64 {
                        bail!("jump at {i} to {t}, past the end of its stream ({n} instructions)");
                    }
                    let (t1, t2) = offsets[t as usize];
                    let (j1, j2) = offsets[i];
                    *data1_offset = (t1 as i64 - j1 as i64) as u32;
                    *data2_offset = (t2 as i64 - j2 as i64) as u32;
                    *data3_offset = *instruction_offset;
                }
            }
        }
        Ok(())
    }

    /// Adds the expression's blocks.
    pub fn write(&self, w: &mut Rsc8Writer) -> Result<BlockId> {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(0xA0);
        b.u64(self.vft);
        b.u64(self.unknown_08h);
        b.u64(self.usage_count);
        // The C# writes the root's address, not a pointer.
        b.u64(VIRTUAL_BASE);
        b.u64(self.unknown_20h);
        b.u64(self.unknown_28h);
        let streams: Option<Vec<Option<BlockId>>> =
            self.streams.as_ref().map(|v| v.iter().map(|s| s.as_ref().map(|s| s.write(w))).collect());
        write_ptr_arr(w, &mut b, streams.as_deref());
        write_arr(w, &mut b, self.tracks.as_deref());
        write_arr(w, &mut b, self.springs.as_deref());
        write_arr(w, &mut b, self.variables.as_deref());
        write_str_a(w, &mut b, self.name.as_deref());
        b.u32(self.unknown_80h);
        b.u32(self.signature);
        b.u32(self.max_stream_size);
        b.u16(self.unknown_8ch);
        b.u16(self.unknown_8eh);
        let map = self.property_map.as_ref().map(|m| m.write(w, Some(id))).transpose()?;
        b.ptr(map);
        b.u64(self.unknown_98h);
        w.set(id, b, false);
        Ok(id)
    }
}

/// `ReadStructArray`: the `Item`s of `id` (`None` when absent).
fn struct_array<T>(n: &XmlNode, id: &str, f: impl Fn(&XmlNode) -> T) -> Option<Vec<T>> {
    let c = n.child(id)?;
    Some(c.children("Item").map(|i| f(&i)).collect())
}

// --- Streams --------------------------------------------------------------------

/// `Rsc8ExpressionStream`: a program, 0x10 bytes of header and its buffers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExpressionStream {
    pub name_hash: u32,
    /// The stack depth it needs.
    pub depth: u16,
    pub instructions: Vec<ExpressionOp>,
}

/// A stream's three buffers, as [`ExpressionStream::encode`] lays them out.
pub struct EncodedStream {
    pub data1: Vec<u8>,
    pub data2: Vec<u8>,
    /// One type byte per instruction.
    pub data3: Vec<u8>,
    /// Each instruction's offsets in `data1` and `data2`.
    pub offsets: Vec<(usize, usize)>,
}

impl ExpressionStream {
    pub fn read(r: &mut Rsc8Reader, ptr: u64) -> Result<Self> {
        r.at(ptr, |r| {
            let name_hash = r.u32()?;
            let l1 = r.u32()? as usize;
            let l2 = r.u32()? as usize;
            let l3 = r.u16()? as usize;
            let depth = r.u16()?;
            let d1 = r.bytes(l1)?;
            let d2 = r.bytes(l2)?;
            let d3 = r.bytes(l3)?;
            let mut r1 = OpReader { data: d1, pos: 0 };
            let mut r2 = OpReader { data: d2, pos: 0 };
            let mut instructions = Vec::with_capacity(l3);
            for &t in d3 {
                let mut op = ExpressionOp::new(t);
                op.data.read(&mut r1, &mut r2)?;
                instructions.push(op);
            }
            Ok(Self { name_hash, depth, instructions })
        })
    }

    /// `WriteInstructions`: the three buffers.
    pub fn encode(&self) -> EncodedStream {
        let mut e = EncodedStream {
            data1: Vec::new(),
            data2: Vec::new(),
            data3: Vec::with_capacity(self.instructions.len()),
            offsets: Vec::with_capacity(self.instructions.len()),
        };
        for op in &self.instructions {
            e.offsets.push((e.data1.len(), e.data2.len()));
            e.data3.push(op.op_type);
            op.data.write(&mut e.data1, &mut e.data2);
        }
        e
    }

    /// `BlockLength`: the header and the buffers (the type count as a u16).
    pub fn block_length(&self) -> u32 {
        let e = self.encode();
        0x10u32
            .wrapping_add(e.data1.len() as u32)
            .wrapping_add(e.data2.len() as u32)
            .wrapping_add(e.data3.len() as u16 as u32)
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let e = self.encode();
        let mut b = BlockBuf::with_capacity(0x10 + e.data1.len() + e.data2.len() + e.data3.len());
        b.u32(self.name_hash);
        b.u32(e.data1.len() as u32);
        b.u32(e.data2.len() as u32);
        b.u16(e.data3.len() as u16);
        b.u16(self.depth);
        b.bytes(&e.data1);
        b.bytes(&e.data2);
        b.bytes(&e.data3);
        w.add(b, false)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.hash("Name", self.name_hash);
        w.u16("Depth", self.depth);
        w.node_array("Instructions", Some(&self.instructions), |w, op| op.write_xml(w));
    }

    pub fn read_xml(n: &XmlNode) -> Result<Self> {
        let Some(items) = n.node_array("Instructions") else { bail!("expression stream without instructions") };
        let mut instructions = Vec::with_capacity(items.len());
        for i in items {
            let Some(i) = i else { bail!("null expression instruction") };
            // `Rsc8ExpressionOp.Create(type)`: no type, or one that isn't a
            // byte value or name, gives null, which the C# then fails on.
            let ty = if i.has_attributes() { i.attr("type").or(Some("")) } else { None };
            let Some(t) = parse_enum(ty, OP_TYPES).filter(|t| (0..=255).contains(t)) else {
                bail!("unknown expression instruction type {ty:?}")
            };
            let mut op = ExpressionOp::new(t as u8);
            op.data.read_xml(&i);
            instructions.push(op);
        }
        Ok(Self { name_hash: n.hash("Name"), depth: n.u16("Depth", 0), instructions })
    }
}

// --- Instructions ---------------------------------------------------------------

/// `Rsc8ExpressionOpType`: names and values.
pub const OP_TYPES: &[(&str, i64)] = &[
    ("End", 0x00),
    ("Pop", 0x01),
    ("Dup", 0x02),
    ("Push0", 0x03),
    ("Push1", 0x04),
    ("PushFloat", 0x05),
    ("TrackGet", 0x06),
    ("TrackGetComp", 0x07),
    ("TrackGetOffset", 0x08),
    ("TrackGetOffsetComp", 0x09),
    ("TrackGetObject", 0x0A),
    ("PushVector", 0x0B),
    ("Unknown0C", 0x0C),
    ("Unknown0D", 0x0D),
    ("DefineSpring", 0x0E),
    ("VectorAbs", 0x0F),
    ("VectorNeg", 0x10),
    ("VectorRcp", 0x11),
    ("VectorSqrt", 0x12),
    ("Unknown14", 0x14),
    ("Unknown16", 0x16),
    ("Unknown18", 0x18),
    ("VectorNeg3", 0x1B),
    ("VectorSquare", 0x1C),
    ("VectorDeg2Rad", 0x1D),
    ("VectorRad2Deg", 0x1E),
    ("VectorSaturate", 0x1F),
    ("TrackValid", 0x20),
    ("FromEuler", 0x21),
    ("ToEuler", 0x22),
    ("ObjectConvertFrom", 0x23),
    ("Unknown24", 0x24),
    ("TrackSet", 0x26),
    ("TrackSetComp", 0x27),
    ("TrackSetOffset", 0x28),
    ("TrackSetOffsetComp", 0x29),
    ("TrackSetObject", 0x2A),
    ("Jump", 0x2B),
    ("JumpIfTrue", 0x2C),
    ("JumpIfFalse", 0x2D),
    ("VectorAdd", 0x2E),
    ("VectorSub", 0x2F),
    ("VectorMul", 0x30),
    ("VectorMin", 0x31),
    ("VectorMax", 0x32),
    ("QuatMul", 0x33),
    ("VectorGreaterThan", 0x35),
    ("VectorLessThan", 0x36),
    ("VectorGreaterEqual", 0x37),
    ("VectorLessEqual", 0x38),
    ("VectorClamp", 0x39),
    ("VectorLerp", 0x3A),
    ("VectorMad", 0x3B),
    ("QuatSlerp", 0x3C),
    ("ToVector", 0x3D),
    ("LookAt", 0x3E),
    ("PushTime", 0x3F),
    ("VectorTransform", 0x40),
    ("GetVariable", 0x42),
    ("SetVariable", 0x43),
    ("BlendVector", 0x44),
    ("BlendQuaternion", 0x45),
    ("PushDeltaTime", 0x46),
    ("VectorEqual", 0x48),
    ("VectorNotEqual", 0x49),
    ("Unknown4A", 0x4A),
    ("Unknown4B", 0x4B),
    ("Unknown4E", 0x4E),
    ("Unknown4F", 0x4F),
    ("Unknown50", 0x50),
    ("Unknown51", 0x51),
    ("Unknown52", 0x52),
    ("Unknown54", 0x54),
    ("Unknown56", 0x56),
    ("Unknown57", 0x57),
    ("Unknown59", 0x59),
    ("Unknown5A", 0x5A),
    ("Unknown5B", 0x5B),
    ("Unknown5C", 0x5C),
    ("Unknown5D", 0x5D),
    ("Unknown5E", 0x5E),
    ("Unknown5F", 0x5F),
    ("Unknown60", 0x60),
    ("Unknown61", 0x61),
    ("Unknown62", 0x62),
    ("Unknown63", 0x63),
    ("Unknown65", 0x65),
    ("Unknown67", 0x67),
    ("Unknown69", 0x69),
    ("Unknown6A", 0x6A),
    ("Unknown6C", 0x6C),
    ("Unknown6D", 0x6D),
    ("Unknown6E", 0x6E),
    ("Unknown70", 0x70),
    ("Unknown71", 0x71),
    ("Unknown72", 0x72),
    ("Unknown74", 0x74),
];

/// `Rsc8ExpressionOp`: an instruction's type and the data its class has.
#[derive(Clone, Debug, PartialEq)]
pub struct ExpressionOp {
    pub op_type: u8,
    pub data: OpData,
}

/// `Rsc8ExpressionOpBlend.SourceInfo`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct BlendSource {
    pub track_index: u16,
    pub component_offset: u16,
}

/// The data of each instruction class (`Rsc8ExpressionOpBone` and so on), with
/// the C#'s field names. Buffer 1 has vectors and larger structs, buffer 2
/// small values.
#[derive(Clone, Debug, PartialEq)]
pub enum OpData {
    /// `Rsc8ExpressionOpEmpty`.
    Empty,
    /// `Rsc8ExpressionOpBlend` (`BlendVector`), in buffer 1.
    Blend {
        byte_length: u32,
        source_count: u32,
        num_source_weights: u32,
        unknown1: u32,
        source_infos: Option<Vec<BlendSource>>,
        values: Option<Vec<[f32; 4]>>,
    },
    /// `Rsc8ExpressionOpBone`, in buffer 2.
    Bone { track_index: u16, bone_id: u16, track: u8, format: u8, component_index: u8, use_defaults: u8 },
    /// `Rsc8ExpressionOpVariable`, in buffer 2.
    Variable { variable: u32, variable_index: u16, unknown1: u16 },
    /// `Rsc8ExpressionOpJump`, in buffer 2: the offsets to the target in each
    /// buffer (the third is `instruction_offset`).
    Jump { data1_offset: u32, data2_offset: u32, data3_offset: u32, instruction_offset: u32 },
    /// `Rsc8ExpressionOpFloat`, in buffer 2.
    Float { value: f32 },
    /// `Rsc8ExpressionOpVector`, in buffer 1.
    Vector { value: [f32; 4] },
    /// `Rsc8ExpressionOpUnk50`, in buffer 1.
    Unk50 {
        byte_length: u32,
        item_count: u32,
        unknown1: u32,
        unknown2: u32,
        unknown3: [f32; 4],
        vectors: Option<Vec<[f32; 4]>>,
        ushorts: Option<Vec<u16>>,
        padding: u64,
    },
    /// `Rsc8ExpressionOpUnk5152`, in buffer 2.
    Unk5152 { unknown1: u32 },
    /// `Rsc8ExpressionOpUnk5B`, in buffer 2.
    Unk5B {
        unknown1: u16,
        unknown2: u16,
        float_count: u16,
        byte_length: u16,
        unknown5: f32,
        floats: Option<Vec<f32>>,
        ushorts: Option<Vec<u16>>,
    },
    /// `Rsc8ExpressionOpUnk5C`, in buffer 1.
    Unk5C { unknown1: [f32; 4], unknown2: u32, unknown3: u32, unknown4: u32, unknown5: u32 },
    /// `Rsc8ExpressionOpUnk577172`, in buffer 2.
    Unk577172 { unknown1: u16, unknown2: u16, unknown3: u32 },
    /// `Rsc8ExpressionOpUnk5E6061`, in buffer 2.
    Unk5E6061 { bone_id: u16, unknown2: u16, track_index: u32, unknown4: u32 },
    /// `Rsc8ExpressionOpUnk62636E`, in buffer 1: three vectors, eight u16s
    /// (`unknown4` to `unknown11`) and four u32s (`unknown12` to `unknown15`).
    Unk62636E { vectors: [[f32; 4]; 3], shorts: [u16; 8], ints: [u32; 4] },
    /// `Rsc8ExpressionOpUnk70`, in buffer 2.
    Unk70 { unknown1: u32, unknown2: u32, unknown3: u32 },
    /// `Rsc8ExpressionOpUnk74`, in buffer 2.
    Unk74 { item_count: u16, byte_count: u16, ushorts: Option<Vec<u16>> },
}

impl ExpressionOp {
    /// `Rsc8ExpressionOp.Create(type)`: the class for a type, with its defaults.
    pub fn new(op_type: u8) -> Self {
        let data = match op_type {
            0x44 => OpData::Blend {
                byte_length: 0,
                source_count: 0,
                num_source_weights: 0,
                unknown1: 0,
                source_infos: None,
                values: None,
            },
            0x06..=0x0A | 0x20 | 0x23 | 0x24 | 0x26..=0x2A | 0x45 | 0x5A | 0x6D => {
                OpData::Bone { track_index: 0, bone_id: 0, track: 0, format: 0, component_index: 0, use_defaults: 0 }
            }
            0x42 | 0x43 | 0x67 => OpData::Variable { variable: 0, variable_index: 0, unknown1: 0 },
            0x2B..=0x2D => OpData::Jump { data1_offset: 0, data2_offset: 0, data3_offset: 0, instruction_offset: 0 },
            0x05 => OpData::Float { value: 0.0 },
            0x0B => OpData::Vector { value: [0.0; 4] },
            0x50 => OpData::Unk50 {
                byte_length: 0,
                item_count: 0,
                unknown1: 0,
                unknown2: 0,
                unknown3: [0.0; 4],
                vectors: None,
                ushorts: None,
                padding: 0,
            },
            0x51 | 0x52 => OpData::Unk5152 { unknown1: 0 },
            0x5B => OpData::Unk5B {
                unknown1: 3,
                unknown2: 0,
                float_count: 0,
                byte_length: 0,
                unknown5: 0.0,
                floats: None,
                ushorts: None,
            },
            0x5C => OpData::Unk5C { unknown1: [0.0; 4], unknown2: 0, unknown3: 0, unknown4: 0, unknown5: 0 },
            0x57 | 0x71 | 0x72 => OpData::Unk577172 { unknown1: 0, unknown2: 0, unknown3: 0 },
            0x5E | 0x60 | 0x61 => OpData::Unk5E6061 { bone_id: 0, unknown2: 0, track_index: 0, unknown4: 0 },
            0x62 | 0x63 | 0x6E => OpData::Unk62636E { vectors: [[0.0; 4]; 3], shorts: [0; 8], ints: [0; 4] },
            0x70 => OpData::Unk70 { unknown1: 0, unknown2: 0, unknown3: 0 },
            0x74 => OpData::Unk74 { item_count: 0, byte_count: 0, ushorts: None },
            _ => OpData::Empty,
        };
        Self { op_type, data }
    }

    /// The type's name (`Type.ToString()`: the number when it has none).
    pub fn type_name(&self) -> String {
        enum_name(self.op_type as i64, OP_TYPES)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.string("@type", &self.type_name());
        self.data.write_xml(w);
    }
}

/// A `DataReader` over one of a stream's buffers.
struct OpReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl OpReader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let Some(s) = self.data.get(self.pos..self.pos + N) else {
            bail!("expression instruction data ends at {} (reading {N} bytes at {})", self.data.len(), self.pos)
        };
        self.pos += N;
        Ok(s.try_into().unwrap())
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take()?))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take()?))
    }
    fn vec4(&mut self) -> Result<[f32; 4]> {
        Ok([self.f32()?, self.f32()?, self.f32()?, self.f32()?])
    }
    /// `ReadStructs`: `count` items, null for 0.
    fn structs<T: Pod>(&mut self, count: i64) -> Result<Option<Vec<T>>> {
        if count == 0 {
            return Ok(None);
        }
        let Some(len) = usize::try_from(count).ok().and_then(|c| c.checked_mul(size_of::<T>())) else {
            bail!("expression instruction array of {count} items")
        };
        let Some(s) = self.data.get(self.pos..self.pos + len) else {
            bail!("expression instruction data ends at {} (reading {len} bytes at {})", self.data.len(), self.pos)
        };
        self.pos += len;
        Ok(Some(bytemuck::pod_collect_to_vec(s)))
    }
    /// `ReadUshorts`: null for 0.
    fn u16s(&mut self, count: u32) -> Result<Option<Vec<u16>>> {
        if count == 0 {
            return Ok(None);
        }
        (0..count).map(|_| self.u16()).collect::<Result<Vec<_>>>().map(Some)
    }
    /// `ReadFloats`: null for 0.
    fn f32s(&mut self, count: u32) -> Result<Option<Vec<f32>>> {
        if count == 0 {
            return Ok(None);
        }
        (0..count).map(|_| self.f32()).collect::<Result<Vec<_>>>().map(Some)
    }
}

fn put<T: Pod>(w: &mut Vec<u8>, v: &T) {
    w.extend_from_slice(bytemuck::bytes_of(v));
}

fn put_all<T: Pod>(w: &mut Vec<u8>, v: Option<&[T]>) {
    if let Some(v) = v {
        w.extend_from_slice(bytemuck::cast_slice(v));
    }
}

/// `Rsc8ExpressionOpBlend`'s number of vectors per 4 sources.
fn blend_values_per_source(num_source_weights: u32) -> u32 {
    6u32.wrapping_add(9u32.wrapping_mul(num_source_weights.wrapping_sub(1)))
}

impl OpData {
    /// `Read(DataReader r1, DataReader r2)`.
    fn read(&mut self, r1: &mut OpReader, r2: &mut OpReader) -> Result<()> {
        match self {
            OpData::Empty => {}
            OpData::Blend { byte_length, source_count, num_source_weights, unknown1, source_infos, values } => {
                *byte_length = r1.u32()?;
                *source_count = r1.u32()?;
                *num_source_weights = r1.u32()?;
                *unknown1 = r1.u32()?;
                *source_infos = r1.structs(*source_count as i32 as i64)?;
                let n = source_count.wrapping_mul(blend_values_per_source(*num_source_weights)) / 4;
                *values = r1.structs(n as i32 as i64)?;
            }
            OpData::Bone { track_index, bone_id, track, format, component_index, use_defaults } => {
                *track_index = r2.u16()?;
                *bone_id = r2.u16()?;
                *track = r2.u8()?;
                *format = r2.u8()?;
                *component_index = r2.u8()?;
                *use_defaults = r2.u8()?;
            }
            OpData::Variable { variable, variable_index, unknown1 } => {
                *variable = r2.u32()?;
                *variable_index = r2.u16()?;
                *unknown1 = r2.u16()?;
            }
            OpData::Jump { data1_offset, data2_offset, data3_offset, instruction_offset } => {
                *data1_offset = r2.u32()?;
                *data2_offset = r2.u32()?;
                *data3_offset = r2.u32()?;
                *instruction_offset = *data3_offset;
            }
            OpData::Float { value } => *value = r2.f32()?,
            OpData::Vector { value } => *value = r1.vec4()?,
            OpData::Unk50 { byte_length, item_count, unknown1, unknown2, unknown3, vectors, ushorts, padding } => {
                *byte_length = r1.u32()?;
                *item_count = r1.u32()?;
                *unknown1 = r1.u32()?;
                *unknown2 = r1.u32()?;
                *unknown3 = r1.vec4()?;
                *vectors = r1.structs((*item_count as i32).wrapping_mul(3) as i64)?;
                *ushorts = r1.u16s(item_count.wrapping_mul(4))?;
                if *item_count & 1 != 0 {
                    *padding = r1.u64()?;
                }
            }
            OpData::Unk5152 { unknown1 } => *unknown1 = r2.u32()?,
            OpData::Unk5B { unknown1, unknown2, float_count, byte_length, unknown5, floats, ushorts } => {
                *unknown1 = r2.u16()?;
                *unknown2 = r2.u16()?;
                *float_count = r2.u16()?;
                *byte_length = r2.u16()?;
                *unknown5 = r2.f32()?;
                *floats = r2.f32s(*float_count as u32)?;
                let rest = *byte_length as i32 - (12 + *float_count as i32 * 4);
                if rest != 0 {
                    if rest < 0 {
                        bail!("expression instruction Unknown5B with a byte length too small ({byte_length})");
                    }
                    *ushorts = r2.u16s(rest as u32 / 2)?;
                }
            }
            OpData::Unk5C { unknown1, unknown2, unknown3, unknown4, unknown5 } => {
                *unknown1 = r1.vec4()?;
                *unknown2 = r1.u32()?;
                *unknown3 = r1.u32()?;
                *unknown4 = r1.u32()?;
                *unknown5 = r1.u32()?;
            }
            OpData::Unk577172 { unknown1, unknown2, unknown3 } => {
                *unknown1 = r2.u16()?;
                *unknown2 = r2.u16()?;
                *unknown3 = r2.u32()?;
            }
            OpData::Unk5E6061 { bone_id, unknown2, track_index, unknown4 } => {
                *bone_id = r2.u16()?;
                *unknown2 = r2.u16()?;
                *track_index = r2.u32()?;
                *unknown4 = r2.u32()?;
            }
            OpData::Unk62636E { vectors, shorts, ints } => {
                for v in vectors.iter_mut() {
                    *v = r1.vec4()?;
                }
                for s in shorts.iter_mut() {
                    *s = r1.u16()?;
                }
                for i in ints.iter_mut() {
                    *i = r1.u32()?;
                }
            }
            OpData::Unk70 { unknown1, unknown2, unknown3 } => {
                *unknown1 = r2.u32()?;
                *unknown2 = r2.u32()?;
                *unknown3 = r2.u32()?;
            }
            OpData::Unk74 { item_count, byte_count, ushorts } => {
                *item_count = r2.u16()?;
                *byte_count = r2.u16()?;
                *ushorts = r2.u16s(*item_count as u32 * 4)?;
            }
        }
        Ok(())
    }

    /// `Write(DataWriter w1, DataWriter w2)`, with the counts and lengths worked
    /// out from the arrays as the C# does.
    fn write(&self, w1: &mut Vec<u8>, w2: &mut Vec<u8>) {
        match self {
            OpData::Empty => {}
            OpData::Blend { num_source_weights, unknown1, source_infos, values, .. } => {
                let count = source_infos.as_ref().map_or(0, Vec::len) as u32;
                let hlen = count.wrapping_mul(4).wrapping_add(16);
                let tlen = count.wrapping_mul(blend_values_per_source(*num_source_weights)).wrapping_mul(4);
                put(w1, &hlen.wrapping_add(tlen));
                put(w1, &count);
                put(w1, num_source_weights);
                put(w1, unknown1);
                put_all(w1, source_infos.as_deref());
                put_all(w1, values.as_deref());
            }
            OpData::Bone { track_index, bone_id, track, format, component_index, use_defaults } => {
                put(w2, track_index);
                put(w2, bone_id);
                w2.extend_from_slice(&[*track, *format, *component_index, *use_defaults]);
            }
            OpData::Variable { variable, variable_index, unknown1 } => {
                put(w2, variable);
                put(w2, variable_index);
                put(w2, unknown1);
            }
            OpData::Jump { data1_offset, data2_offset, data3_offset, .. } => {
                put(w2, data1_offset);
                put(w2, data2_offset);
                put(w2, data3_offset);
            }
            OpData::Float { value } => put(w2, value),
            OpData::Vector { value } => put(w1, value),
            OpData::Unk50 { unknown1, unknown2, unknown3, vectors, ushorts, padding, .. } => {
                let n = (vectors.as_ref().map_or(0, Vec::len) / 3) as u32;
                let len = 0x20 + n * 3 * 0x10 + n * 4 * 2 + (n & 1) * 8;
                put(w1, &len);
                put(w1, &n);
                put(w1, unknown1);
                put(w1, unknown2);
                put(w1, unknown3);
                put_all(w1, vectors.as_deref());
                put_all(w1, ushorts.as_deref());
                if n & 1 != 0 {
                    put(w1, padding);
                }
            }
            OpData::Unk5152 { unknown1 } => put(w2, unknown1),
            OpData::Unk5B { unknown1, unknown2, unknown5, floats, ushorts, .. } => {
                let nf = floats.as_ref().map_or(0, Vec::len) as u16;
                let ns = ushorts.as_ref().map_or(0, Vec::len) as u16;
                let len = 12u16.wrapping_add(nf.wrapping_mul(4)).wrapping_add(ns.wrapping_mul(2));
                put(w2, unknown1);
                put(w2, unknown2);
                put(w2, &nf);
                put(w2, &len);
                put(w2, unknown5);
                put_all(w2, floats.as_deref());
                put_all(w2, ushorts.as_deref());
            }
            OpData::Unk5C { unknown1, unknown2, unknown3, unknown4, unknown5 } => {
                put(w1, unknown1);
                put(w1, unknown2);
                put(w1, unknown3);
                put(w1, unknown4);
                put(w1, unknown5);
            }
            OpData::Unk577172 { unknown1, unknown2, unknown3 } => {
                put(w2, unknown1);
                put(w2, unknown2);
                put(w2, unknown3);
            }
            OpData::Unk5E6061 { bone_id, unknown2, track_index, unknown4 } => {
                put(w2, bone_id);
                put(w2, unknown2);
                put(w2, track_index);
                put(w2, unknown4);
            }
            OpData::Unk62636E { vectors, shorts, ints } => {
                put(w1, vectors);
                put(w1, shorts);
                put(w1, ints);
            }
            OpData::Unk70 { unknown1, unknown2, unknown3 } => {
                put(w2, unknown1);
                put(w2, unknown2);
                put(w2, unknown3);
            }
            OpData::Unk74 { ushorts, .. } => {
                let n = (ushorts.as_ref().map_or(0, Vec::len) / 4) as u16;
                put(w2, &n);
                put(w2, &4u16.wrapping_add(n.wrapping_mul(8)));
                put_all(w2, ushorts.as_deref());
            }
        }
    }

    fn write_xml(&self, w: &mut XmlWriter) {
        match self {
            OpData::Empty => {}
            OpData::Blend { num_source_weights, source_infos, values, .. } => {
                w.u32("NumSourceWeights", *num_source_weights);
                w.node_array("Sources", source_infos.as_deref(), |w, s| {
                    if s.track_index != 0 {
                        w.u16("TrackIndex", s.track_index);
                    }
                    if s.component_offset != 0 {
                        w.u16("ComponentOffset", s.component_offset);
                    }
                });
                w.vec4_array("Values", values.as_deref());
            }
            OpData::Bone { track_index, bone_id, track, format, component_index, use_defaults } => {
                w.u16("TrackIndex", *track_index);
                w.u16("BoneId", *bone_id);
                w.u8("Track", *track);
                w.u8("Format", *format);
                w.u8("ComponentIndex", *component_index);
                w.u8("UseDefaults", *use_defaults);
            }
            OpData::Variable { variable, unknown1, .. } => {
                w.hash("Variable", *variable);
                w.u16("Unknown1", *unknown1);
            }
            OpData::Jump { instruction_offset, .. } => w.u32("InstructionOffset", *instruction_offset),
            OpData::Float { value } => w.f32("Value", *value),
            OpData::Vector { value } => w.vec4("Value", *value),
            OpData::Unk50 { unknown3, vectors, ushorts, .. } => {
                w.vec4("Unknown3", *unknown3);
                w.vec4_array("Vectors", vectors.as_deref());
                w.u16_array("UShorts", ushorts.as_deref());
            }
            OpData::Unk5152 { unknown1 } => w.u32("Unknown1", *unknown1),
            OpData::Unk5B { unknown2, unknown5, floats, ushorts, .. } => {
                w.u16("Unknown2", *unknown2);
                w.f32("Unknown5", *unknown5);
                w.f32_array("Floats", floats.as_deref());
                w.u16_array("UShorts", ushorts.as_deref());
            }
            OpData::Unk5C { unknown1, unknown2, unknown3, unknown4, .. } => {
                w.vec4("Unknown1", *unknown1);
                w.u32("Unknown2", *unknown2);
                w.u32("Unknown3", *unknown3);
                w.u32("Unknown4", *unknown4);
            }
            OpData::Unk577172 { unknown1, unknown2, .. } => {
                w.u16("Unknown1", *unknown1);
                w.u16("Unknown2", *unknown2);
            }
            OpData::Unk5E6061 { bone_id, unknown2, track_index, unknown4 } => {
                w.u16("BoneId", *bone_id);
                w.u16("Unknown2", *unknown2);
                w.u32("TrackIndex", *track_index);
                w.u32("Unknown4", *unknown4);
            }
            OpData::Unk62636E { vectors, shorts, ints } => {
                for (i, v) in vectors.iter().enumerate() {
                    w.vec4(&format!("Unknown{}", i + 1), *v);
                }
                for (i, s) in shorts.iter().enumerate() {
                    w.u16(&format!("Unknown{}", i + 4), *s);
                }
                w.u32("Unknown14", ints[2]);
            }
            OpData::Unk70 { unknown1, unknown2, unknown3 } => {
                w.u32("Unknown1", *unknown1);
                w.u32("Unknown2", *unknown2);
                w.u32("Unknown3", *unknown3);
            }
            OpData::Unk74 { ushorts, .. } => w.u16_array("UShorts", ushorts.as_deref()),
        }
    }

    /// `Read(MetaNodeReader)`: what the XML has; the rest keeps its default.
    fn read_xml(&mut self, n: &XmlNode) {
        match self {
            OpData::Empty => {}
            OpData::Blend { num_source_weights, source_infos, values, .. } => {
                *num_source_weights = n.u32("NumSourceWeights", 0);
                *source_infos = struct_array(n, "Sources", |i| BlendSource {
                    track_index: i.u16("TrackIndex", 0),
                    component_offset: i.u16("ComponentOffset", 0),
                });
                *values = n.vec4_array("Values");
            }
            OpData::Bone { track_index, bone_id, track, format, component_index, use_defaults } => {
                *track_index = n.u16("TrackIndex", 0);
                *bone_id = n.u16("BoneId", 0);
                *track = n.u8("Track", 0);
                *format = n.u8("Format", 0);
                *component_index = n.u8("ComponentIndex", 0);
                *use_defaults = n.u8("UseDefaults", 0);
            }
            OpData::Variable { variable, unknown1, .. } => {
                *variable = n.hash("Variable");
                *unknown1 = n.u16("Unknown1", 0);
            }
            OpData::Jump { instruction_offset, .. } => *instruction_offset = n.u32("InstructionOffset", 0),
            OpData::Float { value } => *value = n.f32("Value", 0.0),
            OpData::Vector { value } => *value = n.vec4("Value"),
            OpData::Unk50 { unknown3, vectors, ushorts, .. } => {
                *unknown3 = n.vec4("Unknown3");
                *vectors = n.vec4_array("Vectors");
                *ushorts = n.u16_array("UShorts");
            }
            OpData::Unk5152 { unknown1 } => *unknown1 = n.u32("Unknown1", 0),
            OpData::Unk5B { unknown2, unknown5, floats, ushorts, .. } => {
                *unknown2 = n.u16("Unknown2", 0);
                *unknown5 = n.f32("Unknown5", 0.0);
                *floats = n.f32_array("Floats");
                *ushorts = n.u16_array("UShorts");
            }
            OpData::Unk5C { unknown1, unknown2, unknown3, unknown4, .. } => {
                *unknown1 = n.vec4("Unknown1");
                *unknown2 = n.u32("Unknown2", 0);
                *unknown3 = n.u32("Unknown3", 0);
                *unknown4 = n.u32("Unknown4", 0);
            }
            OpData::Unk577172 { unknown1, unknown2, .. } => {
                *unknown1 = n.u16("Unknown1", 0);
                *unknown2 = n.u16("Unknown2", 0);
            }
            OpData::Unk5E6061 { bone_id, unknown2, track_index, unknown4 } => {
                *bone_id = n.u16("BoneId", 0);
                *unknown2 = n.u16("Unknown2", 0);
                *track_index = n.u32("TrackIndex", 0);
                *unknown4 = n.u32("Unknown4", 0);
            }
            OpData::Unk62636E { vectors, shorts, ints } => {
                for (i, v) in vectors.iter_mut().enumerate() {
                    *v = n.vec4(&format!("Unknown{}", i + 1));
                }
                for (i, s) in shorts.iter_mut().enumerate() {
                    *s = n.u16(&format!("Unknown{}", i + 4), 0);
                }
                ints[2] = n.u32("Unknown14", 0);
            }
            OpData::Unk70 { unknown1, unknown2, unknown3 } => {
                *unknown1 = n.u32("Unknown1", 0);
                *unknown2 = n.u32("Unknown2", 0);
                *unknown3 = n.u32("Unknown3", 0);
            }
            OpData::Unk74 { ushorts, .. } => *ushorts = n.u16_array("UShorts"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rsc8::read_loose;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<RDR2ExpressionDictionary version="0">
 <Expressions>
  <Item>
   <Name>pack:/Face_Expr.expr</Name>
   <Signature value="1234" />
   <Unknown_8C value="3" />
   <Unknown_8E value="241" />
   <Streams>
    <Item>
     <Name>stream_a</Name>
     <Depth value="2" />
     <Instructions>
      <Item type="TrackGet"><TrackIndex value="1" /><BoneId value="4000" /><Track value="7" /><Format value="1" /></Item>
      <Item type="GetVariable"><Variable>var_b</Variable><Unknown1 value="5" /></Item>
      <Item type="JumpIfFalse"><InstructionOffset value="3" /></Item>
      <Item type="PushVector"><Value x="1" y="2" z="3" w="4" /></Item>
      <Item type="SetVariable"><Variable>var_a</Variable></Item>
      <Item type="Unknown5B"><Unknown2 value="12" /><Unknown5 value="0.5" /><Floats>1 2</Floats><UShorts>7 8 9</UShorts></Item>
      <Item type="Unknown74"><UShorts>1 2 3 4</UShorts></Item>
      <Item type="End" />
     </Instructions>
    </Item>
   </Streams>
   <Tracks>
    <Item><BoneId value="4000" /><Track value="7" /><TypeAndFlag value="129" /></Item>
    <Item />
   </Tracks>
   <Springs>
    <Item><Vector01 x="1" y="0" z="0" /><Ushort01a value="2" /><Gravity x="0" y="0" z="-9.8" /><BoneId value="17" /></Item>
   </Springs>
   <Properties>
    <Item><Name>prop</Name><Attributes><Item type="Float"><Name>x</Name><Value value="0.5" /></Item></Attributes></Item>
   </Properties>
  </Item>
 </Expressions>
</RDR2ExpressionDictionary>"#;

    #[test]
    fn expression_dictionary_round_trips_through_xml() {
        let hashes = HashIndex::new();
        hashes.ensure("stream_a");
        let cx = XmlContext { hashes: &hashes, folder: None };
        let file = from_xml(XML, &cx).unwrap();
        let (h, _) = read_loose(&file).unwrap();
        assert_eq!(h.version(), VERSION);

        let d = read(&file, &hashes).unwrap();
        assert_eq!(d.entries_count, 1);
        // The entry's hash keeps the case; the name learnt is lowercase.
        assert_eq!(d.entries[0].hash, joaat("Face_Expr"));
        assert_eq!(hashes.get(joaat("face_expr")).as_deref(), Some("face_expr"));
        let e = d.entries[0].expression.as_ref().unwrap();
        assert_eq!((e.signature, e.unknown_8ch, e.unknown_8eh, e.usage_count), (1234, 3, 241, 1));

        // Variables sorted by hash, and each instruction's index into them.
        let (a, b) = (joaat("var_a"), joaat("var_b"));
        let mut vars = [a, b];
        vars.sort();
        assert_eq!(e.variables.as_deref(), Some(&vars[..]));
        let s = e.streams.as_ref().unwrap()[0].as_ref().unwrap();
        let index_of = |i: usize| match s.instructions[i].data {
            OpData::Variable { variable_index, .. } => variable_index as usize,
            _ => panic!(),
        };
        assert_eq!((vars[index_of(1)], vars[index_of(4)]), (b, a));

        // The jump (at 2, to 5) skips a vector in buffer 1 and a variable in
        // buffer 2.
        let OpData::Jump { data1_offset, data2_offset, data3_offset, .. } = s.instructions[2].data else { panic!() };
        assert_eq!((data1_offset, data2_offset, data3_offset), (16, 8 + 12, 3));
        assert_eq!(e.max_stream_size, s.block_length());
        let OpData::Unk5B { unknown1, byte_length, ushorts, .. } = &s.instructions[5].data else { panic!() };
        assert_eq!((*unknown1, *byte_length, ushorts.as_deref()), (3, 12 + 8 + 6, Some(&[7u16, 8, 9][..])));

        let tracks = e.tracks.as_ref().unwrap();
        assert_eq!(tracks[0], ExpressionTrack { bone_id: 4000, track: 7, type_and_flag: 129 });
        assert_eq!(tracks[1], ExpressionTrack::default());
        let spring = e.springs.as_ref().unwrap()[0];
        assert_eq!((spring.parts[0].vector, spring.parts[0].a, spring.bone_id), ([1.0, 0.0, 0.0], 2, 17));
        assert_eq!(e.property_map.as_ref().unwrap().items_count, 1);

        // And to XML again: the same, but for the names learnt and
        // computed values.
        let xml = to_xml(&file, &cx).unwrap();
        assert!(xml.contains("<Name>stream_a</Name>"), "{xml}");
        assert!(xml.contains("<Item type=\"JumpIfFalse\">"), "{xml}");
        assert!(xml.contains("<UShorts>7 8 9</UShorts>"), "{xml}");
        assert!(xml.contains("<Item />"), "{xml}");
        let again = to_xml(&from_xml(&xml, &cx).unwrap(), &cx).unwrap();
        assert_eq!(xml, again);
    }

    #[test]
    fn fails_where_the_csharp_fails() {
        let hashes = HashIndex::new();
        let cx = XmlContext { hashes: &hashes, folder: None };
        let no_props = XML.replace("<Properties>", "<X>").replace("</Properties>", "</X>");
        assert!(from_xml(&no_props, &cx).is_err());
        let bad_jump = XML.replace("<InstructionOffset value=\"3\" />", "<InstructionOffset value=\"9\" />");
        assert!(from_xml(&bad_jump, &cx).is_err());
        let bad_type = XML.replace("type=\"End\"", "type=\"Nope\"");
        assert!(from_xml(&bad_type, &cx).is_err());
        // Numbers are fine, as with `Enum.TryParse`.
        let number = XML.replace("type=\"End\"", "type=\"19\"");
        let xml = to_xml(&from_xml(&number, &cx).unwrap(), &cx).unwrap();
        assert!(xml.contains("<Item type=\"19\" />"), "{xml}");
    }
}
