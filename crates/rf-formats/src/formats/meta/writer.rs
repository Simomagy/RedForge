//! `Rsc8MetaWriter`: lays bags out in data blocks, and collects the classes and
//! enums they use.

use std::collections::HashSet;
use std::sync::Arc;

use bytemuck::Pod;

use super::pointer::{self, data_offset};
use super::{MetaArray, MetaLookup, RscClassEntry, RscDataBlock, RscDataType, rsc_class};
use crate::formats::databag::{
    DataBag, EnumInfo, ITEM_HASH, KEY_HASH, Schema, SchemaClass, SchemaEnum, SchemaField, Value, ValueType, ascii_bytes,
};
use crate::{Error, Result, bail};

/// Blocks hold at most this much, as offsets in pointers are 20 bits.
const MAX_BLOCK_LENGTH: usize = 0xFFFFF;

struct Block {
    data_type: u32,
    /// The space taken, including space reserved but not (yet) written.
    length: usize,
    alignment: usize,
    /// What's written (a `MemoryStream`: writing past the end extends it).
    data: Vec<u8>,
}

pub(super) struct MetaWriter<'s> {
    /// Where classes find the classes and enums of their fields.
    schema: &'s Schema,
    blocks: Vec<Block>,
    structs: Vec<Arc<SchemaClass>>,
    struct_set: HashSet<usize>,
    enums: Vec<Option<Arc<SchemaEnum>>>,
    enum_set: HashSet<usize>,
    /// The entry classes made for binary maps, by (class, field index) of the map
    /// field, in the order they were made.
    lookup_classes: Vec<((usize, usize), Arc<SchemaClass>)>,
}

fn set_struct<T: Pod>(data: &mut [u8], cls: &SchemaClass, name: u32, v: &T) {
    let Some(o) = cls.field(name).and_then(|f| usize::try_from(f.offset).ok()) else { return };
    let b = bytemuck::bytes_of(v);
    let n = data.len().saturating_sub(o).min(b.len());
    if n > 0 {
        data[o..o + n].copy_from_slice(&b[..n]);
    }
}

impl<'s> MetaWriter<'s> {
    pub fn new(schema: &'s Schema) -> Self {
        Self {
            schema,
            blocks: Vec::new(),
            structs: Vec::new(),
            struct_set: HashSet::new(),
            enums: Vec::new(),
            enum_set: HashSet::new(),
            lookup_classes: Vec::new(),
        }
    }

    /// The file's classes, enums and data blocks (`GetClasses`, `GetEnums`, `GetBlocks`).
    #[allow(clippy::type_complexity)]
    pub fn finish(
        self,
    ) -> (Option<Vec<Arc<SchemaClass>>>, Option<Vec<Option<Arc<SchemaEnum>>>>, Option<Vec<RscDataBlock>>) {
        let classes = (!self.structs.is_empty()).then_some(self.structs);
        let enums = (!self.enums.is_empty()).then_some(self.enums);
        let blocks: Vec<RscDataBlock> =
            self.blocks.into_iter().map(|b| RscDataBlock { data_type: b.data_type, data: Some(b.data) }).collect();
        (classes, enums, (!blocks.is_empty()).then_some(blocks))
    }

    /// `GetBlock`: the first block of this type with room, or a new one.
    fn get_block(&mut self, data_type: u32, len: usize, force_create: bool) -> usize {
        if !force_create
            && let Some(i) =
                self.blocks.iter().position(|b| b.data_type == data_type && b.length + len < MAX_BLOCK_LENGTH)
        {
            return i;
        }
        let alignment = if data_type < 0x20 { 0 } else { 8 };
        self.blocks.push(Block { data_type, length: 0, alignment, data: Vec::new() });
        self.blocks.len() - 1
    }

    /// `AddObject`: reserves `len` bytes (aligned) in a block.
    fn add_object(&mut self, blk: usize, len: usize) -> u64 {
        let b = &mut self.blocks[blk];
        if b.alignment != 0 {
            b.length += (b.alignment - b.length % b.alignment) % b.alignment;
        }
        let p = pointer::new(blk, b.length);
        b.length += len;
        p
    }

    fn write_at(&mut self, blk: usize, pos: usize, bytes: &[u8]) {
        let d = &mut self.blocks[blk].data;
        if d.len() < pos + bytes.len() {
            d.resize(pos + bytes.len(), 0);
        }
        d[pos..pos + bytes.len()].copy_from_slice(bytes);
    }

    /// `AddStruct`: a class the file uses, with the classes and enums of its fields.
    fn add_struct(&mut self, s: Option<&Arc<SchemaClass>>) {
        let Some(s) = s else { return };
        if s.rsc().is_none() || !self.struct_set.insert(Arc::as_ptr(s) as usize) {
            return;
        }
        self.structs.push(s.clone());
        let schema = self.schema;
        for f in s.fields() {
            if f.type_name == 0 {
                continue;
            }
            match f.data_type {
                ValueType::Enum8 | ValueType::Enum16 | ValueType::Enum32 | ValueType::Flags32 => {
                    let e = schema.enums.get(&f.type_name).filter(|e| matches!(e.info, EnumInfo::Rsc(_))).cloned();
                    let key = e.as_ref().map_or(0, |e| Arc::as_ptr(e) as usize);
                    if self.enum_set.insert(key) {
                        self.enums.push(e);
                    }
                }
                ValueType::Struct => self.add_struct(schema.classes.get(&f.type_name)),
                ValueType::Array if f.array_size > 0 => self.add_struct(schema.classes.get(&f.type_name)),
                _ => {}
            }
        }
    }

    /// `WriteObject`: a pointer to the bag, laid out in a block of its class.
    pub fn write_object(&mut self, bag: &DataBag) -> Result<u64> {
        let size = bag.class.data_size as usize;
        let blk = self.get_block(bag.class.name, size, false);
        let p = self.add_object(blk, size);
        self.write_fields(bag, blk, data_offset(p))?;
        Ok(p)
    }

    /// `WriteArray`: plain items in a block of their type.
    fn write_array<T: Pod>(&mut self, items: &[T], data_type: RscDataType) -> MetaArray {
        if items.is_empty() {
            return MetaArray::default();
        }
        let bytes: &[u8] = bytemuck::cast_slice(items);
        let blk = self.get_block(data_type.0 as u32, bytes.len(), false);
        let p = self.add_object(blk, bytes.len());
        self.write_at(blk, data_offset(p), bytes);
        MetaArray { pointer: p, count: items.len() as u16, capacity: items.len() as u16, unknown: 0 }
    }

    /// `WriteFields`: writes the bag's struct at `pos` in a block, after writing
    /// what its fields point to and setting the pointers. Returns the struct's
    /// final bytes.
    fn write_fields(&mut self, bag: &DataBag, blk: usize, pos: usize) -> Result<Vec<u8>> {
        let cls = &bag.class;
        if !cls.has_fields() {
            return Ok(bag.data.clone());
        }
        if cls.rsc().is_some() {
            self.add_struct(Some(cls));
        }
        let clen = cls.data_size as usize;
        if clen == 0 {
            return Ok(bag.data.clone());
        }
        let mut data = bag.data.clone();
        if data.len() < clen {
            bail!("class 0x{:08X}: the data is shorter than the class", cls.name);
        }
        for (fi, field) in cls.fields().iter().enumerate() {
            let name = field.name;
            let foff = (pos as i64 + field.offset as i64) as usize;
            match field.data_type {
                ValueType::String => {
                    let mut a = MetaArray::default();
                    if let Some(s) = bag.string(name)
                        && !s.is_empty()
                    {
                        let bytes = ascii_bytes(s);
                        let sblk = self.get_block(RscDataType::SIGNED_BYTE.0 as u32, bytes.len(), false);
                        let p = self.add_object(sblk, bytes.len());
                        self.write_at(sblk, data_offset(p), &bytes);
                        a = MetaArray {
                            pointer: p,
                            count: bytes.len() as u16,
                            capacity: bytes.len() as u16,
                            unknown: 0,
                        };
                    }
                    set_struct(&mut data, cls, name, &a);
                }
                ValueType::Struct => {
                    if let Some(sobj) = bag.bag(name) {
                        let child = self.write_fields(sobj, blk, foff)?;
                        let ds = sobj.class.data_size as usize;
                        let at = usize::try_from(field.offset).ok();
                        match at {
                            Some(at) if at + ds <= data.len() && ds <= child.len() => {
                                data[at..at + ds].copy_from_slice(&child[..ds])
                            }
                            _ => bail!("class 0x{:08X}: an embedded struct doesn't fit", cls.name),
                        }
                    }
                }
                ValueType::Object => {
                    let p = match bag.object(name) {
                        Some(Value::Bytes(b)) => {
                            let bb = self.get_block(RscDataType::UNSIGNED_BYTE.0 as u32, b.len(), true);
                            let p = self.add_object(bb, b.len());
                            self.write_at(bb, data_offset(p), b);
                            p
                        }
                        Some(Value::Bag(o)) => self.write_object(o)?,
                        _ => 0,
                    };
                    set_struct(&mut data, cls, name, &p);
                }
                ValueType::Array if field.array_size == 0 => {
                    let a = match bag.object(name) {
                        Some(aobj) => self.write_pointer_array(field, aobj)?,
                        None => MetaArray::default(),
                    };
                    set_struct(&mut data, cls, name, &a);
                }
                ValueType::Lookup => {
                    let mut linf = MetaLookup::default();
                    if let Some(Value::Bags(larr)) = bag.object(name)
                        && !larr.is_empty()
                    {
                        let kvp0 = larr[0].as_ref().map(|b| &b.class);
                        let lcls = self
                            .lookup_class(cls, fi, field, kvp0)?
                            .ok_or_else(|| Error::Format("a binary map's entries have no Key and Item".into()))?;
                        let lsiz = lcls.data_size as usize;
                        let tsiz = lsiz * larr.len();
                        let lblk = self.get_block(lcls.name, tsiz, false);
                        let lptr = self.add_object(lblk, tsiz);
                        for (i, item) in larr.iter().enumerate() {
                            let Some(item) = item else { bail!("a binary map has a null entry") };
                            self.lookup_class(cls, fi, field, Some(&item.class))?;
                            let lv = lookup_value(item, &lcls)?;
                            self.write_fields(&lv, lblk, data_offset(lptr) + lsiz * i)?;
                        }
                        linf.pointer = lptr;
                        linf.count = larr.len() as u16;
                        linf.capacity = linf.count;
                        self.add_struct(Some(&lcls));
                    }
                    linf.unknown_0 = 1;
                    set_struct(&mut data, cls, name, &linf);
                }
                _ => {}
            }
        }
        self.write_at(blk, pos, &data[..clen]);
        Ok(data)
    }

    fn write_pointer_array(&mut self, field: &SchemaField, aobj: &Value) -> Result<MetaArray> {
        // The CLR's array covariance lets `as uint[]` take an int[], and so on.
        Ok(match (field.array_type, aobj) {
            (ValueType::Object, Value::Bags(oarr)) => {
                let mut ptrs = Vec::with_capacity(oarr.len());
                for o in oarr {
                    ptrs.push(match o {
                        Some(o) => self.write_object(o)?,
                        None => 0u64,
                    });
                }
                self.write_array(&ptrs, RscDataType::STRUCTURE_POINTER)
            }
            (ValueType::Struct, Value::Bags(sarr)) if !sarr.is_empty() => {
                let Some(scls) = sarr[0].as_ref().map(|b| b.class.clone()) else { return Ok(MetaArray::default()) };
                let ssiz = scls.data_size as usize;
                if ssiz == 0 {
                    return Ok(MetaArray::default());
                }
                let tsiz = sarr.len() * ssiz;
                let sblk = self.get_block(scls.name, tsiz, false);
                let sptr = self.add_object(sblk, tsiz);
                let so = data_offset(sptr);
                for (i, item) in sarr.iter().enumerate() {
                    if let Some(item) = item {
                        self.write_fields(item, sblk, so + i * ssiz)?;
                    }
                }
                MetaArray { pointer: sptr, count: sarr.len() as u16, capacity: sarr.len() as u16, unknown: 0 }
            }
            (ValueType::Float, Value::F32s(v)) => self.write_array(v, RscDataType::FLOAT),
            // Packed as float4s.
            (ValueType::Float3, Value::Vec4s(v)) => self.write_array(v, RscDataType::FLOAT_XYZ),
            (ValueType::UInt8, Value::Bytes(v)) => self.write_array(v, RscDataType::UNSIGNED_BYTE),
            (ValueType::UInt8, Value::SBytes(v)) => self.write_array(v, RscDataType::UNSIGNED_BYTE),
            (ValueType::UInt16, Value::U16s(v)) => self.write_array(v, RscDataType::UNSIGNED_SHORT),
            (ValueType::UInt16, Value::I16s(v)) => self.write_array(v, RscDataType::UNSIGNED_SHORT),
            (ValueType::UInt32, Value::U32s(v)) => self.write_array(v, RscDataType::UNSIGNED_INT),
            (ValueType::UInt32, Value::I32s(v)) => self.write_array(v, RscDataType::UNSIGNED_INT),
            (ValueType::UInt64, Value::U64s(v)) => self.write_array(v, RscDataType::UNSIGNED_LONG),
            (ValueType::UInt64, Value::I64s(v)) => self.write_array(v, RscDataType::UNSIGNED_LONG),
            (ValueType::Hash, Value::Hashes(v)) => self.write_array(v, RscDataType::HASH),
            _ => MetaArray::default(),
        })
    }

    /// `GetLookupClass`: the class of a binary map's entries in the file: a key
    /// and an item, named 0x100, 0x101... in the order they're made, and shared
    /// by maps with the same key and item types.
    fn lookup_class(
        &mut self,
        parent: &Arc<SchemaClass>,
        fi: usize,
        field: &SchemaField,
        kvpcls: Option<&Arc<SchemaClass>>,
    ) -> Result<Option<Arc<SchemaClass>>> {
        let Some(kvpcls) = kvpcls else { return Ok(None) };
        let (Some(kfld), Some(ifld)) = (kvpcls.field(KEY_HASH), kvpcls.field(ITEM_HASH)) else { return Ok(None) };
        if parent.rsc().is_none() {
            bail!("a binary map in a class that isn't a resource meta class");
        }
        // XML reads entry items as objects, but they're embedded structs.
        let item_type = if ifld.data_type == ValueType::Object && field.type_name != 0 {
            ValueType::Struct
        } else {
            ifld.data_type
        };
        let key = (Arc::as_ptr(parent) as usize, fi);
        if let Some((_, c)) = self.lookup_classes.iter().find(|(k, _)| *k == key) {
            return Ok(Some(c.clone()));
        }
        let mut name_hash = 0x100u32;
        for (_, c) in &self.lookup_classes {
            if c.name >= name_hash {
                name_hash = c.name.wrapping_add(1);
            }
        }
        let mut kent = RscClassEntry { name: KEY_HASH, offset: 0, data_type: RscDataType::HASH, ..Default::default() };
        let mut ient =
            RscClassEntry { name: ITEM_HASH, offset: 8, data_type: RscDataType::STRUCTURE, ..Default::default() };
        let mut slen = 16u32;
        match kfld.data_type {
            ValueType::Hash => kent.data_type = RscDataType::HASH,
            ValueType::UInt32 => kent.data_type = RscDataType::UNSIGNED_INT,
            _ => {}
        }
        match item_type {
            ValueType::Struct => {
                ient.data_type = RscDataType::STRUCTURE;
                ient.type_name = ifld.type_name;
            }
            ValueType::Array => ient.data_type = RscDataType::ARRAY,
            _ => {}
        }
        let matching = self.lookup_classes.iter().find_map(|(_, c)| {
            let e = c.rsc()?.entries.as_deref()?;
            let (lk, li) = (e.first()?, e.get(1)?);
            (lk.data_type == kent.data_type
                && lk.type_name == kent.type_name
                && li.data_type == ient.data_type
                && li.type_name == ient.type_name
                && li.reference_type_index == ient.reference_type_index)
                .then(|| c.clone())
        });
        if let Some(c) = matching {
            self.set_lookup_class(key, c.clone());
            return Ok(Some(c));
        }

        let sunk = 0x300u32;
        let mut skey = 0u32;
        let mut iref = 0u16;
        let mut i32_item = false;
        // The sizes and keys of the maps RDR2 has.
        match field.name {
            0x2e90_8cb7 | 0xa449_9c0a => {
                slen = 0x28;
                skey = 0x3752_3b0d;
            }
            0xd417_93f6 => {
                slen = 0x18;
                skey = 0xe914_f3fe;
                iref = 1;
            }
            0xb4a5_58a9 => {
                slen = 0x08;
                skey = 0xe12e_8810;
                i32_item = true;
            }
            0xdcfa_c098 => {
                slen = 0x18;
                skey = 0xc9b3_254f;
                iref = 5;
            }
            0x54c2_d719 => {
                slen = 0x18;
                skey = 0x3fa7_ab67;
                iref = 16;
            }
            _ => {}
        }
        ient.reference_type_index = iref;
        if iref != 0 {
            ient.data_type = RscDataType::ARRAY;
            ient.type_name = 0;
        }
        if i32_item {
            ient.offset = 4;
            slen = 8;
        }
        let mut cls = rsc_class(name_hash, skey, sunk, slen, vec![kent, ient])?;
        let Some(itm) = cls.field_at_mut(1) else { bail!("binary map entry class without an item field") };
        if field.name == 0x54c2_d719 {
            // A ulong array.
            itm.data_type = ValueType::Array;
            itm.array_type = ValueType::UInt64;
            itm.type_name = 0;
        }
        if field.name == 0xd417_93f6 || field.name == 0xdcfa_c098 {
            itm.data_type = ValueType::Array;
            itm.array_type = ValueType::Struct;
            itm.type_name = ifld.type_name;
        }
        let cls = Arc::new(cls);
        self.set_lookup_class(key, cls.clone());
        Ok(Some(cls))
    }

    fn set_lookup_class(&mut self, key: (usize, usize), cls: Arc<SchemaClass>) {
        match self.lookup_classes.iter_mut().find(|(k, _)| *k == key) {
            Some(e) => e.1 = cls,
            None => self.lookup_classes.push((key, cls)),
        }
    }
}

/// `GetLookupValue`: a map entry as a bag of the file's entry class.
fn lookup_value(orig: &DataBag, cls: &Arc<SchemaClass>) -> Result<DataBag> {
    let (Some(kfld), Some(ifld)) = (cls.field(KEY_HASH), cls.field(ITEM_HASH)) else {
        bail!("binary map entry class without Key and Item")
    };
    let mut bag = DataBag::new(cls.clone());
    match kfld.data_type {
        ValueType::Hash => bag.set_hash(KEY_HASH, orig.hash(KEY_HASH)),
        ValueType::UInt32 => bag.set_u32(KEY_HASH, orig.u32(KEY_HASH)),
        _ => {}
    }
    if matches!(ifld.data_type, ValueType::Struct | ValueType::Array) {
        bag.set_object(ITEM_HASH, orig.object(ITEM_HASH).cloned());
    }
    Ok(bag)
}
