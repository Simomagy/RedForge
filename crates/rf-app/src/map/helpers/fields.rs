//! A data bag's fields as Properties shows a part's: each value as text, by
//! the type its class gives it.

use rf_formats::formats::databag::{DataBag, Schema, ValueType as T};
use rf_formats::hash::HashIndex;

fn floats(v: &[f32]) -> String {
    v.iter().map(|f| f.to_string()).collect::<Vec<_>>().join(", ")
}

/// A field's value (`None` for a field the bag's class doesn't have, or one
/// that isn't a value: an array, a struct...).
pub fn text(bag: &DataBag, field: u32, schema: Option<&Schema>, hashes: &HashIndex) -> Option<String> {
    let f = bag.class.field(field)?;
    Some(match f.data_type {
        T::Float => bag.f32(field).to_string(),
        T::Float2 => floats(&bag.vec2(field)),
        T::Float3 => floats(&bag.vec3(field)),
        T::Float4 => floats(&bag.vec4(field)),
        T::UInt8 => bag.u8(field).to_string(),
        T::UInt16 => bag.u16(field).to_string(),
        T::UInt32 => bag.u32(field).to_string(),
        T::UInt64 => bag.u64(field).to_string(),
        T::Int8 => bag.i8(field).to_string(),
        T::Int16 => bag.i16(field).to_string(),
        T::Int32 => bag.i32(field).to_string(),
        T::Int64 => bag.i64(field).to_string(),
        T::Double => bag.f64(field).to_string(),
        T::Boolean => bag.bool(field).to_string(),
        T::Char => (bag.u8(field) as char).to_string(),
        T::Hash => match bag.hash(field) {
            0 => String::new(),
            h => hashes.name(h),
        },
        T::FnvHash => format!("0x{:016X}", bag.fnv_hash(field)),
        T::String => bag.string(field)?.to_owned(),
        T::Enum8 | T::Enum16 | T::Enum32 | T::Flags32 => {
            bag.enum_string(field, schema, hashes).unwrap_or_else(|| bag.enum_value(field).to_string())
        }
        T::None | T::Float3x4 | T::Float4x4 | T::Array | T::Object | T::Struct | T::Lookup => return None,
    })
}

/// Every value field of a bag, in its class's order, named by the strings.
pub fn all(bag: &DataBag, schema: Option<&Schema>, hashes: &HashIndex) -> Vec<(String, String)> {
    bag.class.fields().iter().filter_map(|f| Some((hashes.name(f.name), text(bag, f.name, schema, hashes)?))).collect()
}
