//! The map's instanced props and grass ([`rf_world::VisibleBatch`]): each
//! batch's instances go to the GPU once, and each frame its cells in view are
//! drawn at their LOD, as the C#'s `EntityBatch.StreamingUpdate` picks it.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::wgpu;
use glam::{Mat4, Vec3};
use rf_render::{GpuModel, Instance};
use rf_world::{EntityId, InstanceBatch, VisibleBatch};

use crate::viewers::Gpu;

/// Where the pick ids of batches' instances start: below are the entities'.
const FIRST_BATCH_ID: u32 = 1 << 30;

/// Batches' instance buffers, by [`VisibleBatch::key`]. Each batch's
/// instances have pick ids of their own.
pub struct BatchBuffers {
    buffers: HashMap<(u32, u32), Entry>,
    next_id: u32,
}

impl Default for BatchBuffers {
    fn default() -> Self {
        Self { buffers: HashMap::new(), next_id: FIRST_BATCH_ID }
    }
}

struct Entry {
    buffer: wgpu::Buffer,
    used: Instant,
    /// The batch uploaded, and its instances' first pick id.
    batch: Arc<InstanceBatch>,
    first_id: u32,
}

/// What each pick id of a frame stands for: an entity, or a batch's instance.
#[derive(Default)]
pub struct Picks {
    /// The entity of each id below the batches', minus 1.
    pub entities: Vec<EntityId>,
    /// Per batch: its instances' first id, its map, the batch.
    batches: Vec<(u32, u32, Arc<InstanceBatch>)>,
}

impl Picks {
    pub fn get(&self, id: u32) -> Option<EntityId> {
        if id < FIRST_BATCH_ID {
            return self.entities.get(id.checked_sub(1)? as usize).copied();
        }
        let (first, node, b) = self.batches.iter().find(|(f, _, b)| id >= *f && id - *f < b.len() as u32)?;
        Some(EntityId::instance(*node, b.grass, b.list_index, b.source_index((id - first) as usize)))
    }
}

impl BatchBuffers {
    /// Uploads the batches in range that have no buffer yet, for up to
    /// `budget`.
    pub fn upload(&mut self, gpu: &Gpu, visible: &[VisibleBatch], budget: Duration) {
        let start = Instant::now();
        let now = Instant::now();
        for v in visible {
            // Again when the batch changed (an instance moved).
            if let Some(e) = self.buffers.get_mut(&v.key)
                && Arc::ptr_eq(&e.batch, &v.batch)
            {
                e.used = now;
                continue;
            }
            if start.elapsed() > budget {
                break;
            }
            let first_id = self.next_id;
            self.next_id = self.next_id.wrapping_add(v.batch.len() as u32).max(FIRST_BATCH_ID);
            let b = &v.batch;
            let instances: Vec<Instance> = (0..b.len())
                .map(|i| {
                    let inst = Instance::new(Mat4::from_cols_array(&b.transform(i)), 0, first_id + i as u32);
                    match b.normal(i) {
                        Some(n) => inst.with_normal(Vec3::from(n)),
                        None => inst,
                    }
                })
                .collect();
            let buffer = gpu.renderer.create_instance_buffer(&gpu.ctx, &instances);
            self.buffers.insert(v.key, Entry { buffer, used: now, batch: v.batch.clone(), first_id });
        }
    }

    /// The pick ids of the batches' instances, with `entities` for the rest.
    pub fn picks(&self, entities: Vec<EntityId>) -> Picks {
        let batches = self.buffers.iter().map(|(k, e)| (e.first_id, k.0, e.batch.clone())).collect();
        Picks { entities, batches }
    }

    /// A batch's buffer, if it's the batch's as it is now.
    pub fn buffer(&self, v: &VisibleBatch) -> Option<&wgpu::Buffer> {
        self.buffers.get(&v.key).filter(|e| Arc::ptr_eq(&e.batch, &v.batch)).map(|e| &e.buffer)
    }

    /// Drops the buffers of batches out of range for a while.
    pub fn evict(&mut self, after: Duration) {
        self.buffers.retain(|_, e| e.used.elapsed() < after);
    }
}

/// How far each of a batch's LODs reaches (`InitLods`): the model's high and
/// medium distances (the archetype's for a medium of 9998, and no further
/// than the last), then the instance distance for grass, the model's low
/// distance for props.
pub fn lod_dists(batch: &InstanceBatch, model: &GpuModel, archetype_lod_dist: f32) -> [f32; 3] {
    let [high, med, low, _] = model.lod_dists;
    let last = if batch.grass { batch.instance_lod_dist } else { low };
    let med = if med == 9998.0 { archetype_lod_dist } else { med };
    [high, med.min(last), last]
}

/// The runs of a batch's instances to draw, each at a LOD of the model: its
/// cells whose LOD the camera at `eye` is in range of, that `seen` says are in
/// view (by their bounding sphere), consecutive ones with the same LOD
/// merged.
pub fn runs(
    batch: &InstanceBatch,
    model: &GpuModel,
    dists: [f32; 3],
    eye: Vec3,
    seen: &dyn Fn(Vec3, f32) -> bool,
) -> Vec<(usize, Range<u32>)> {
    let extent = (Vec3::from(model.max) - Vec3::from(model.min)).length() * 0.5;
    let mut out: Vec<(usize, Range<u32>)> = Vec::new();
    for cell in &batch.cells {
        let (min, max) = (Vec3::from(cell.bounds.min), Vec3::from(cell.bounds.max));
        // The first LOD whose reach (a cube about the eye) touches the cell.
        let Some(lod) = dists.iter().position(|&d| {
            let (lo, hi) = (eye - Vec3::splat(d), eye + Vec3::splat(d));
            lo.cmple(max).all() && hi.cmpge(min).all()
        }) else {
            continue;
        };
        if model.lods[lod].is_empty() {
            continue;
        }
        // Instances stick out of their cell by up to twice the model's size.
        if !seen((min + max) * 0.5, (max - min).length() * 0.5 + extent * 2.0) {
            continue;
        }
        match out.last_mut() {
            Some((l, r)) if *l == lod && r.end == cell.start => r.end = cell.end,
            _ => out.push((lod, cell.start..cell.end)),
        }
    }
    out
}
