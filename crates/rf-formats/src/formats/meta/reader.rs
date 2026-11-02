//! `Rsc8MetaReader`: follows the meta pointers from the root block into bags.

use std::sync::Arc;

use bytemuck::Pod;

use super::pointer::{block_id, data_offset, with_data_offset};
use super::{MetaArray, MetaLookup, RscDataBlock, RscDataType};
use crate::formats::databag::{DataBag, ITEM_HASH, Schema, SchemaClass, Value, ValueType, ascii_string};
use crate::{Result, bail};

pub(super) struct MetaReader<'a> {
    blocks: &'a [RscDataBlock],
    /// The file's schema. Reading a binary map sets the item type of its entry
    /// class, as the C# does.
    schema: &'a mut Schema,
}

/// `BufferUtil.ReadArray`: `count` items at `offset`, zero-filled past the end;
/// `None` for no items.
fn read_items<T: Pod>(data: &[u8], offset: usize, count: usize) -> Result<Option<Vec<T>>> {
    if count == 0 {
        return Ok(None);
    }
    if offset > data.len() {
        bail!("array at offset {offset} is past the end of its block");
    }
    let size = count * size_of::<T>();
    let mut bytes = vec![0u8; size];
    let n = size.min(data.len() - offset);
    bytes[..n].copy_from_slice(&data[offset..offset + n]);
    Ok(Some(bytemuck::pod_collect_to_vec(&bytes)))
}

/// A bag's own copy of its struct's bytes.
fn struct_data(data: &[u8], offset: usize, size: usize) -> Vec<u8> {
    let mut v = vec![0u8; size];
    if offset < data.len() {
        let n = size.min(data.len() - offset);
        v[..n].copy_from_slice(&data[offset..offset + n]);
    }
    v
}

impl<'a> MetaReader<'a> {
    pub fn new(blocks: &'a [RscDataBlock], schema: &'a mut Schema) -> Self {
        Self { blocks, schema }
    }

    /// `GetBlock(id)`: blocks are numbered from 1.
    fn block(&self, id: i32) -> Option<&'a RscDataBlock> {
        let i = usize::try_from(id - 1).ok()?;
        self.blocks.get(i)
    }

    fn class(&self, name: u32) -> Option<Arc<SchemaClass>> {
        self.schema.classes.get(&name).cloned()
    }

    /// `ReadObject(pointer, cls)`: the object a pointer points to, of the given
    /// class or else the class its block holds. `None` when there's no such
    /// block or class, or the object doesn't fit in the block.
    pub fn read_object_at(&mut self, p: u64, cls: Option<Arc<SchemaClass>>) -> Result<Option<DataBag>> {
        let Some(block) = self.block(block_id(p)) else { return Ok(None) };
        let Some(data) = block.data.as_deref() else { return Ok(None) };
        let offset = data_offset(p);
        let cls = match cls {
            Some(c) => c,
            None => match self.class(block.data_type) {
                Some(c) => c,
                None => return Ok(None),
            },
        };
        if offset + cls.data_size as usize > data.len() {
            return Ok(None);
        }
        self.read_object(&cls, data, offset)
    }

    fn read_array<T: Pod>(&self, a: &MetaArray) -> Result<Option<Vec<T>>> {
        let Some(block) = self.block(block_id(a.pointer)) else { return Ok(None) };
        let Some(data) = block.data.as_deref() else { return Ok(None) };
        read_items(data, data_offset(a.pointer), a.count as usize)
    }

    /// `ReadObject(cls, data, offset)`.
    fn read_object(&mut self, cls: &Arc<SchemaClass>, data: &'a [u8], offset: usize) -> Result<Option<DataBag>> {
        if !cls.has_fields() {
            return Ok(None);
        }
        let mut bag = DataBag::with_data(cls.clone(), struct_data(data, offset, cls.data_size as usize));
        for field in cls.fields() {
            let name = field.name;
            match field.data_type {
                ValueType::String => {
                    let a: MetaArray = bag.get_struct(name);
                    if a.pointer != 0
                        && let Some(sd) = self.block(block_id(a.pointer)).and_then(|b| b.data.as_deref())
                    {
                        let (o, n) = (data_offset(a.pointer), a.count as usize);
                        if o + n > sd.len() {
                            bail!("string at offset {o} ({n} bytes) is past the end of its block");
                        }
                        bag.set_string(name, ascii_string(&sd[o..o + n]));
                    }
                }
                ValueType::Struct => {
                    if let Some(ccls) = self.class(field.type_name) {
                        let Ok(foff) = usize::try_from(offset as i64 + field.offset as i64) else {
                            bail!("embedded struct at a negative offset");
                        };
                        let cbag = self.read_object(&ccls, data, foff)?;
                        bag.set_bag(name, cbag);
                    }
                }
                ValueType::Object => {
                    let p: u64 = bag.get_struct(name);
                    if p != 0 {
                        let Some(oblk) = self.block(block_id(p)) else {
                            bail!("pointer 0x{p:X} to a block that doesn't exist");
                        };
                        if self.schema.classes.contains_key(&oblk.data_type) {
                            let obj = self.read_object_at(p, None)?;
                            bag.set_bag(name, obj);
                        } else if RscDataType(oblk.data_type as u8) == RscDataType::UNSIGNED_BYTE {
                            // A raw data block: the whole block.
                            bag.set_object(name, oblk.data.clone().map(Value::Bytes));
                        }
                    }
                }
                ValueType::Array if field.array_size == 0 => {
                    let a: MetaArray = bag.get_struct(name);
                    if a.pointer == 0 {
                        continue;
                    }
                    let obj = match field.array_type {
                        ValueType::Object => match self.read_array::<u64>(&a)? {
                            Some(ptrs) => {
                                let mut arr = Vec::with_capacity(ptrs.len());
                                for p in ptrs {
                                    arr.push(self.read_object_at(p, None)?);
                                }
                                Some(Value::Bags(arr))
                            }
                            None => None,
                        },
                        ValueType::Struct => {
                            let scls = self.class(field.type_name);
                            let mut arr = Vec::with_capacity(a.count as usize);
                            let mut p = a.pointer;
                            for _ in 0..a.count {
                                let o = self.read_object_at(p, scls.clone())?;
                                let size = o.as_ref().map(|o| o.class.data_size).or(scls.as_ref().map(|c| c.data_size));
                                if let Some(size) = size {
                                    p = with_data_offset(p, data_offset(p) + size as usize);
                                }
                                arr.push(o);
                            }
                            Some(Value::Bags(arr))
                        }
                        ValueType::Float => self.read_array::<f32>(&a)?.map(Value::F32s),
                        // These are packed as float4s.
                        ValueType::Float3 => self.read_array::<[f32; 4]>(&a)?.map(Value::Vec4s),
                        ValueType::UInt8 => self.read_array::<u8>(&a)?.map(Value::Bytes),
                        ValueType::UInt16 => self.read_array::<u16>(&a)?.map(Value::U16s),
                        ValueType::UInt32 => self.read_array::<u32>(&a)?.map(Value::U32s),
                        ValueType::UInt64 => self.read_array::<u64>(&a)?.map(Value::U64s),
                        ValueType::Hash => self.read_array::<u32>(&a)?.map(Value::Hashes),
                        _ => None,
                    };
                    if obj.is_some() {
                        bag.set_object(name, obj);
                    }
                }
                ValueType::Lookup => {
                    let l: MetaLookup = bag.get_struct(name);
                    if l.count == 0 {
                        continue;
                    }
                    let Some(lublk) = self.block(block_id(l.pointer)) else { continue };
                    let Some(entry) = self.schema.classes.get_mut(&lublk.data_type) else { continue };
                    // The entry class doesn't know its item type: the map field does.
                    if let Some(i) = entry.field_index(ITEM_HASH) {
                        let f = &entry.fields()[i];
                        let (mut tn, mut at) = (field.type_name, f.array_type);
                        if field.type_name == RscDataType::UNSIGNED_LONG.0 as u32 {
                            at = RscDataType::UNSIGNED_LONG.engine_type();
                            tn = 0;
                        }
                        if f.type_name != tn || f.array_type != at {
                            let f = Arc::make_mut(entry).field_at_mut(i).unwrap();
                            f.type_name = tn;
                            f.array_type = at;
                        }
                    }
                    let lucls = entry.clone();
                    let mut arr = Vec::with_capacity(l.count as usize);
                    let mut p = l.pointer;
                    for _ in 0..l.count {
                        arr.push(self.read_object_at(p, Some(lucls.clone()))?);
                        p = with_data_offset(p, data_offset(p) + lucls.data_size as usize);
                    }
                    bag.set_object(name, Some(Value::Bags(arr)));
                }
                _ => {}
            }
        }
        Ok(Some(bag))
    }
}
