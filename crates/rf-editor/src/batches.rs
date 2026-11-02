//! Editing the instances of a ymap's prop and grass batches
//! ([`rf_world::EntityId::instance`]): as the C#'s `EntityBatch`
//! (`SetInstanceTransform`, `DeleteInstances`, `CreateInstances`), with the
//! batch's population grid rebuilt as `RDR2EntityBatch.RebuildPopulationGrid`
//! does before saving. A prop batch's box also grows to its instances (the
//! C# keeps the old one).

use glam::{Mat4, Quat, Vec3};
use rf_formats::formats::databag::DataBag;
use rf_game::Archetype;
use rf_world::batch::{
    batch_bounds, batch_list, batch_list_mut, instance_list, instance_list_mut, instance_transform,
    set_instance_transform,
};

use crate::placement::Placement;

const BATCH_AABB: u32 = 0x6ECE_BA6E;
const MIN: u32 = 0xFE30_E6F3;
const MAX: u32 = 0x6060_68C4;
const POPULATION_GRID_DATA: u32 = 0xAB13_0376;
const BITSET: u32 = 0x9928_256B;
const CELL_SIZE: u32 = 0x0C70_D409;
const GRID_SIZE_X: u32 = 0x6BC7_D2C4;
const GRID_SIZE_Y: u32 = 0x1E6D_3810;
const ROW_SPAN: u32 = 0xE88B_A592;

/// An instance: (grass, the batch's place in its list, the instance's in the batch).
pub type InstanceRef = (bool, u32, u32);

pub fn batch(root: &DataBag, grass: bool, batch: u32) -> Option<&DataBag> {
    batch_list(root, grass)?.get(batch as usize)?.as_ref()
}

/// An instance's `InstanceList` item, and its batch.
pub fn instance(root: &DataBag, (grass, b, i): InstanceRef) -> Option<(&DataBag, &DataBag)> {
    let batch = batch(root, grass, b)?;
    Some((batch, instance_list(batch)?.get(i as usize)?.as_ref()?))
}

/// Where an instance is: a prop's transform as it is; grass's position, turn
/// about Z and scale.
pub fn placement(root: &DataBag, r: InstanceRef) -> Option<Placement> {
    let (batch, item) = instance(root, r)?;
    let m = Mat4::from_cols_array(&instance_transform(batch, r.0, item));
    if r.0 {
        let yaw = m.x_axis.y.atan2(m.x_axis.x);
        return Some(Placement {
            position: m.w_axis.truncate(),
            orientation: Quat::from_rotation_z(yaw),
            scale: Vec3::splat(m.z_axis.z),
        });
    }
    let (scale, orientation, position) = m.to_scale_rotation_translation();
    Some(Placement { position, orientation, scale })
}

/// The archetype's box and bounding sphere's radius, for fitting a batch.
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
    pub radius: f32,
}

impl Bounds {
    pub fn of(a: &Archetype) -> Self {
        Bounds { min: a.bb_min.into(), max: a.bb_max.into(), radius: a.bs_radius }
    }
}

/// Moves instances, then refits their batches.
pub fn set(root: &mut DataBag, edits: &[(InstanceRef, Placement)], bounds: &dyn Fn(&DataBag) -> Option<Bounds>) {
    let mut touched = Vec::new();
    for &((grass, b, i), p) in edits {
        let Some(batch) = batch_list_mut(root, grass).and_then(|l| l.get_mut(b as usize)).and_then(Option::as_mut)
        else {
            continue;
        };
        let snapshot = batch.clone();
        if let Some(Some(item)) = instance_list_mut(batch).and_then(|l| l.get_mut(i as usize)) {
            set_instance_transform(&snapshot, grass, item, &p.matrix().to_cols_array());
        }
        if !touched.contains(&(grass, b)) {
            touched.push((grass, b));
        }
    }
    refit(root, &touched, bounds);
}

/// Removes instances; batches left empty go too. The ids of the batches
/// after them change.
pub fn remove(root: &mut DataBag, ids: &[InstanceRef], bounds: &dyn Fn(&DataBag) -> Option<Bounds>) {
    let mut batches: Vec<(bool, u32)> = ids.iter().map(|&(g, b, _)| (g, b)).collect();
    batches.sort_unstable();
    batches.dedup();
    let mut emptied = Vec::new();
    for &(grass, b) in &batches {
        let Some(batch) = batch_list_mut(root, grass).and_then(|l| l.get_mut(b as usize)).and_then(Option::as_mut)
        else {
            continue;
        };
        let Some(list) = instance_list_mut(batch) else { continue };
        let mut gone: Vec<usize> =
            ids.iter().filter(|r| (r.0, r.1) == (grass, b)).map(|r| r.2 as usize).filter(|&i| i < list.len()).collect();
        gone.sort_unstable();
        gone.dedup();
        for i in gone.into_iter().rev() {
            list.remove(i);
        }
        if list.is_empty() {
            emptied.push((grass, b));
        }
    }
    let kept: Vec<(bool, u32)> = batches.iter().copied().filter(|k| !emptied.contains(k)).collect();
    refit(root, &kept, bounds);
    for (grass, b) in emptied.into_iter().rev() {
        if let Some(l) = batch_list_mut(root, grass)
            && (b as usize) < l.len()
        {
            l.remove(b as usize);
        }
    }
}

/// Copies instances into their batches, where they are; gives the copies.
pub fn duplicate(
    root: &mut DataBag,
    ids: &[InstanceRef],
    bounds: &dyn Fn(&DataBag) -> Option<Bounds>,
) -> Vec<InstanceRef> {
    let mut out = Vec::new();
    let mut touched = Vec::new();
    for &(grass, b, i) in ids {
        let Some(batch) = batch_list_mut(root, grass).and_then(|l| l.get_mut(b as usize)).and_then(Option::as_mut)
        else {
            continue;
        };
        let Some(list) = instance_list_mut(batch) else { continue };
        let Some(Some(item)) = list.get(i as usize).cloned() else { continue };
        list.push(Some(item));
        out.push((grass, b, list.len() as u32 - 1));
        if !touched.contains(&(grass, b)) {
            touched.push((grass, b));
        }
    }
    refit(root, &touched, bounds);
    out
}

/// A prop batch's box grown to its instances, and every batch's population
/// grid rebuilt.
fn refit(root: &mut DataBag, batches: &[(bool, u32)], bounds: &dyn Fn(&DataBag) -> Option<Bounds>) {
    for &(grass, b) in batches {
        let Some(batch) = batch_list_mut(root, grass).and_then(|l| l.get_mut(b as usize)).and_then(Option::as_mut)
        else {
            continue;
        };
        let Some(arch) = bounds(batch) else { continue };
        let transforms: Vec<Mat4> = instance_list(batch)
            .into_iter()
            .flatten()
            .flatten()
            .map(|i| Mat4::from_cols_array(&instance_transform(batch, grass, i)))
            .collect();
        if !grass {
            let old = batch_bounds(batch);
            let (mut lo, mut hi) = (Vec3::from(old.min), Vec3::from(old.max));
            for m in &transforms {
                for c in 0..8 {
                    let p = Vec3::select(glam::BVec3::new(c & 1 != 0, c & 2 != 0, c & 4 != 0), arch.max, arch.min);
                    let w = m.transform_point3(p);
                    lo = lo.min(w);
                    hi = hi.max(w);
                }
            }
            if let Some(aabb) = batch.bag_mut(BATCH_AABB) {
                aabb.set_vec4(MIN, lo.extend(0.0).to_array());
                aabb.set_vec4(MAX, hi.extend(0.0).to_array());
            }
        }
        rebuild_population_grid(batch, &transforms, arch.radius);
    }
}

/// `RebuildPopulationGrid`: a grid over the batch's box (64 cells along its
/// longer side, or 32 when the bits don't fit), each cell set where an
/// instance's bounding sphere covers it.
fn rebuild_population_grid(batch: &mut DataBag, transforms: &[Mat4], radius: f32) {
    let aabb = batch_bounds(batch);
    let (min, max) = (Vec3::from(aabb.min), Vec3::from(aabb.max));
    let size = max - min;
    let (min_dim, max_dim) = (size.x.min(size.y), size.x.max(size.y));
    let longer_x = size.x >= size.y;
    let mut grid_max = 64;
    let (mut cell, mut gx, mut gy, mut row_span);
    loop {
        cell = max_dim / grid_max as f32;
        let grid_min = (min_dim / cell).ceil() as i32;
        (gx, gy) = if longer_x { (grid_max, grid_min) } else { (grid_min, grid_max) };
        row_span = if gx <= 16 {
            16
        } else if gx <= 32 {
            32
        } else {
            64
        };
        if row_span * gy <= 512 && row_span < 32 {
            row_span *= 2;
        }
        if row_span * gy <= 1024 || grid_max <= 32 {
            break;
        }
        grid_max /= 2;
    }
    let (gx, gy) = (gx.max(1), gy.max(1));
    let mut bitmap = vec![false; (gx * gy) as usize];
    for m in transforms {
        let p = m.transform_point3(Vec3::ZERO);
        let r = radius * m.transform_vector3(Vec3::ONE).length();
        let pxy = (p - min).truncate();
        let lo = (p - Vec3::splat(r) - min) / cell;
        let hi = (p + Vec3::splat(r) - min) / cell;
        let cx = |v: f32| (v as i32).clamp(0, gx - 1);
        let cy = |v: f32| (v as i32).clamp(0, gy - 1);
        for x in cx(lo.x)..=cx(hi.x) {
            for y in cy(lo.y)..=cy(hi.y) {
                if (glam::Vec2::new(x as f32, y as f32) * cell - pxy).length() <= r {
                    bitmap[(y * gx + x) as usize] = true;
                }
            }
        }
    }
    let mut bits = [0u64; 16];
    let rows_per = 64 / row_span;
    for y in 0..gy {
        let word = (y / rows_per) as usize;
        let offset = (y % rows_per) * row_span;
        for x in 0..gx {
            if bitmap[(y * gx + x) as usize] && word < 16 {
                bits[word] |= 1u64 << (offset + x);
            }
        }
    }
    if let Some(grid) = batch.bag_mut(POPULATION_GRID_DATA) {
        grid.set_embedded_array(BITSET, &bits);
        grid.set_f32(CELL_SIZE, cell);
        grid.set_i8(GRID_SIZE_X, gx as i8);
        grid.set_i8(GRID_SIZE_Y, gy as i8);
        grid.set_i8(ROW_SPAN, row_span as i8);
    }
}
