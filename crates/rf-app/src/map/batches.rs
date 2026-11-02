//! The map's instanced props and grass ([`rf_world::VisibleBatch`]): each
//! batch's instances go to the GPU once, and each frame its cells in view are
//! drawn at their LOD, as the C#'s `EntityBatch.StreamingUpdate` picks it.

use std::collections::HashMap;
use std::ops::Range;
use std::time::{Duration, Instant};

use eframe::wgpu;
use glam::{Mat4, Vec3};
use rf_render::{GpuModel, Instance};
use rf_world::{InstanceBatch, VisibleBatch};

use crate::viewers::Gpu;

/// Batches' instance buffers, by [`VisibleBatch::key`].
#[derive(Default)]
pub struct BatchBuffers {
    buffers: HashMap<(u32, u32), Entry>,
}

struct Entry {
    buffer: wgpu::Buffer,
    used: Instant,
}

impl BatchBuffers {
    /// Uploads the batches in range that have no buffer yet, for up to
    /// `budget`.
    pub fn upload(&mut self, gpu: &Gpu, visible: &[VisibleBatch], budget: Duration) {
        let start = Instant::now();
        let now = Instant::now();
        for v in visible {
            if let Some(e) = self.buffers.get_mut(&v.key) {
                e.used = now;
                continue;
            }
            if start.elapsed() > budget {
                break;
            }
            let b = &v.batch;
            let instances: Vec<Instance> = (0..b.len())
                .map(|i| {
                    let inst = Instance::new(Mat4::from_cols_array(&b.transform(i)), 0, 0);
                    match b.normal(i) {
                        Some(n) => inst.with_normal(Vec3::from(n)),
                        None => inst,
                    }
                })
                .collect();
            let buffer = gpu.renderer.create_instance_buffer(&gpu.ctx, &instances);
            self.buffers.insert(v.key, Entry { buffer, used: now });
        }
    }

    pub fn buffer(&self, key: (u32, u32)) -> Option<&wgpu::Buffer> {
        self.buffers.get(&key).map(|e| &e.buffer)
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
