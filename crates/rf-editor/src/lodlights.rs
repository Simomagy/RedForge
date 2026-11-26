//! A ymap's LOD lights: what the game draws of lights far away, and of the
//! lights of interiors seen from outside.
//!
//! They come as pairs of ymaps (`lodlights.rpf`): `lodlights_<size><n>` with
//! a `LODLightsSOA` (each light's direction, falloff, cone, flags...) and its
//! parent `distlodlights_<size><n>` with a `DistantLODLightsSOA` (position,
//! colour, hash). Both are structs of arrays, one item a light, in the same
//! order: a light removed from one is removed from the other.

use glam::Vec3;
use rf_formats::formats::databag::{DataBag, Value, ValueType};

const LOD: &str = "LODLightsSOA";
const DISTANT: &str = "DistantLODLightsSOA";

/// An array value's length; `None` for values that aren't arrays of items.
fn len(v: &Value) -> Option<usize> {
    Some(match v {
        Value::Hashes(a) | Value::U32s(a) => a.len(),
        Value::Bytes(a) => a.len(),
        Value::SBytes(a) => a.len(),
        Value::U16s(a) => a.len(),
        Value::I16s(a) => a.len(),
        Value::I32s(a) => a.len(),
        Value::U64s(a) => a.len(),
        Value::I64s(a) => a.len(),
        Value::F32s(a) => a.len(),
        Value::Vec2s(a) => a.len(),
        Value::Vec3s(a) => a.len(),
        Value::Vec4s(a) => a.len(),
        Value::Bools(a) => a.len(),
        Value::Bags(a) => a.len(),
        _ => return None,
    })
}

fn keep<T>(a: &mut Vec<T>, gone: &[bool]) {
    let mut i = 0;
    a.retain(|_| {
        i += 1;
        !gone[i - 1]
    });
}

fn cut(v: &mut Value, gone: &[bool]) {
    match v {
        Value::Hashes(a) | Value::U32s(a) => keep(a, gone),
        Value::Bytes(a) => keep(a, gone),
        Value::SBytes(a) => keep(a, gone),
        Value::U16s(a) => keep(a, gone),
        Value::I16s(a) => keep(a, gone),
        Value::I32s(a) => keep(a, gone),
        Value::U64s(a) => keep(a, gone),
        Value::I64s(a) => keep(a, gone),
        Value::F32s(a) => keep(a, gone),
        Value::Vec2s(a) => keep(a, gone),
        Value::Vec3s(a) => keep(a, gone),
        Value::Vec4s(a) => keep(a, gone),
        Value::Bools(a) => keep(a, gone),
        Value::Bags(a) => keep(a, gone),
        _ => {}
    }
}

/// Where a `distlodlights` ymap's lights are.
pub fn positions(map: &DataBag) -> Vec<Vec3> {
    match map.bag(DISTANT).and_then(|d| d.object("position")) {
        // `FloatXYZ` structs, as the game's ymaps have them.
        Some(Value::Bags(p)) => {
            p.iter().map(|b| b.as_ref().map_or(Vec3::ZERO, |b| Vec3::new(b.f32("x"), b.f32("y"), b.f32("z")))).collect()
        }
        Some(Value::Vec4s(p)) => p.iter().map(|v| Vec3::new(v[0], v[1], v[2])).collect(),
        Some(Value::Vec3s(p)) => p.iter().map(|v| Vec3::from(*v)).collect(),
        _ => Vec::new(),
    }
}

/// How many lights a ymap of either kind has.
pub fn count(map: &DataBag) -> usize {
    let lod = map.bag(LOD).and_then(|l| l.object("direction")).and_then(len).unwrap_or(0);
    lod.max(positions(map).len())
}

/// Removes the lights marked in `gone` (one flag a light) from a ymap of
/// either kind: from every array of its lights. The street lights come first
/// in a `distlodlights` ymap, counted by `numStreetLights`, which follows.
/// Returns how many arrays were cut.
pub fn remove(map: &mut DataBag, gone: &[bool]) -> usize {
    let mut arrays = 0;
    for soa in [LOD, DISTANT] {
        let Some(bag) = map.bag_mut(soa) else { continue };
        let names: Vec<u32> = bag.objects().filter(|(_, v)| len(v) == Some(gone.len())).map(|(n, _)| n).collect();
        for name in &names {
            if let Some(v) = bag.object_mut(*name) {
                cut(v, gone);
                arrays += 1;
            }
        }
        if soa == DISTANT && !names.is_empty() {
            let wide = bag.class.field("numStreetLights").is_some_and(|f| f.data_type == ValueType::UInt32);
            let street = if wide { bag.u32("numStreetLights") } else { bag.u16("numStreetLights") as u32 };
            let lost = gone.iter().take(street as usize).filter(|g| **g).count() as u32;
            let left = street.saturating_sub(lost);
            if wide {
                bag.set_u32("numStreetLights", left);
            } else {
                bag.set_u16("numStreetLights", left as u16);
            }
        }
    }
    arrays
}
