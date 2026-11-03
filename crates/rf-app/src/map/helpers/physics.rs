//! "Physics": the collisions around the camera, as the C#'s `StreamPhysics`
//! holds them: the world's static bounds (the ybns of `StaticBoundsStore`
//! within [`RANGE`]), the bounds of the interiors there (each interior's ybns,
//! where it's placed) and those of the entities in range (their models'). Each
//! bound with a shape is a part ([`bounds::parts`]), in its material's colour.
//!
//! The files are read and made into parts on worker threads ([`loader`]),
//! uploaded a few at a time, and kept a while after they go out of range.

mod bounds;
mod loader;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::{Mat4, Vec3};
use rf_formats::rpf8::FileRef;
use rf_game::GameData;
use rf_render::HelperMesh;

use self::bounds::{Shape, max_scale};
use self::loader::{Job, Loader, Owner};
use super::shapes::Shapes;
use super::{Collect, HelperKey, HelperPart, Mode, PartInfo};

/// `StreamPhysicsRange`: how far from the camera collisions show.
pub const RANGE: f32 = 100.0;
/// How long a collision stays loaded out of range.
const KEEP: Duration = Duration::from_secs(30);
/// How long a frame may spend uploading collisions.
const UPLOAD_BUDGET: Duration = Duration::from_millis(4);

/// What a key's group says the part is of (its high bits): a static ybn's
/// (the key's source is the ybn's name hash), or an interior's (the source
/// is the interior's ymap, the group's low bits its entity index). An
/// entity's group is its index in its ymap (the source).
const STATIC: u32 = 1 << 31;
const INTERIOR: u32 = 1 << 30;

/// `RDR2BoundsStoreItem`: a static ybn and its box.
struct StoreItem {
    hash: u32,
    file: FileRef,
    min: Vec3,
    max: Vec3,
}

/// The static bounds store: the streamed ybns in the bounds cache that aren't
/// an interior's.
fn static_store(data: &GameData) -> Vec<StoreItem> {
    data.stream_entries("ybn")
        .filter(|(hash, _)| !data.interior_bounds_lookup.contains_key(hash))
        .filter_map(|(hash, file)| {
            let b = data.bounds_cache.get(&hash)?;
            Some(StoreItem { hash, file, min: b.min.into(), max: b.max.into() })
        })
        .collect()
}

/// A part on the GPU, in the space of its bounds.
struct Part {
    mesh: Arc<HelperMesh>,
    transform: Mat4,
    colour: [u8; 4],
    centre: Vec3,
    radius: f32,
    info: Arc<PartInfo>,
}

impl Part {
    /// The part through `place` (its owner's transform).
    fn placed(&self, key: HelperKey, place: &Mat4) -> HelperPart {
        HelperPart {
            key,
            mesh: self.mesh.clone(),
            transform: *place * self.transform,
            colour: self.colour,
            centre: place.transform_point3(self.centre),
            radius: self.radius * max_scale(place),
            info: self.info.clone(),
        }
    }
}

enum Slot {
    Loading,
    Ready(Arc<Vec<Part>>),
}

struct Entry {
    slot: Slot,
    last_used: Instant,
}

/// The collisions loaded and loading.
struct Cache {
    loader: Loader,
    entries: HashMap<Owner, Entry>,
    /// The game data's version they're from.
    data_version: u64,
}

impl Cache {
    /// A collision's parts if they're loaded; asks for them the first time
    /// (`job` gives none when there's nothing to load).
    fn get(&mut self, owner: Owner, now: Instant, job: impl FnOnce() -> Option<Job>) -> Option<Arc<Vec<Part>>> {
        if let Some(e) = self.entries.get_mut(&owner) {
            e.last_used = now;
            return match &e.slot {
                Slot::Ready(parts) => Some(parts.clone()),
                Slot::Loading => None,
            };
        }
        let slot = match job() {
            Some(job) => {
                self.loader.request(job);
                Slot::Loading
            }
            None => Slot::Ready(Arc::default()),
        };
        self.entries.insert(owner, Entry { slot, last_used: now });
        None
    }

    /// Uploads the collisions the workers made, for up to the budget.
    fn upload(&mut self, c: &Collect, shapes: &mut Shapes) {
        let start = Instant::now();
        while start.elapsed() < UPLOAD_BUDGET {
            let Ok(done) = self.loader.done.try_recv() else { break };
            // Dropped while it was loading: not wanted any more.
            let Some(entry) = self.entries.get_mut(&done.owner) else { continue };
            let parts = done.parts.into_iter().filter_map(|p| {
                let mesh = match p.shape {
                    Shape::UnitBox => shapes.unit_box(c.gpu, false),
                    Shape::UnitSphere => shapes.unit_sphere(c.gpu, false),
                    Shape::Mesh(g) => g.upload(c.gpu, false)?,
                };
                let info = Arc::new(PartInfo { owner: done.name.clone(), ..p.info });
                Some(Part { mesh, transform: p.transform, colour: p.colour, centre: p.centre, radius: p.radius, info })
            });
            entry.slot = Slot::Ready(Arc::new(parts.collect()));
        }
    }

    /// Forgets what's waiting to load but wasn't asked for this frame, and
    /// what hasn't been used for a while.
    fn trim(&mut self, now: Instant) {
        let entries = &self.entries;
        for owner in self.loader.cancel_unless(|o| entries.get(&o).is_some_and(|e| e.last_used == now)) {
            self.entries.remove(&owner);
        }
        self.entries.retain(|_, e| matches!(e.slot, Slot::Loading) || now.duration_since(e.last_used) < KEEP);
    }
}

#[derive(Default)]
pub struct Physics {
    cache: Option<Cache>,
    store: Vec<StoreItem>,
}

fn key(source: u32, group: u32, index: usize) -> HelperKey {
    HelperKey { mode: Mode::Physics, source, group, index: index as u32 }
}

impl Physics {
    pub fn collect(&mut self, c: &Collect, shapes: &mut Shapes, out: &mut Vec<HelperPart>) {
        let now = Instant::now();
        let data = c.game.data();
        let version = c.game.data_version();
        if self.cache.as_ref().is_none_or(|cache| cache.data_version != version) {
            // New game data (a project's files changed): everything again.
            let threads = std::thread::available_parallelism().map_or(2, |n| n.get() / 4).clamp(1, 4);
            let loader = Loader::new(c.game.clone(), threads);
            self.cache = Some(Cache { loader, entries: HashMap::new(), data_version: version });
            self.store = static_store(&data);
        }
        let cache = self.cache.as_mut().unwrap();
        cache.upload(c, shapes);

        let (lo, hi) = (c.eye - Vec3::splat(RANGE), c.eye + Vec3::splat(RANGE));
        let in_range = |min: Vec3, max: Vec3| min.cmple(hi).all() && max.cmpge(lo).all();

        // The ground's and the buildings' collisions. As in the C# (which
        // wants its box to intersect the range's), not those whose box holds
        // the whole range: the map-wide ones.
        let holds_range = |s: &StoreItem| s.min.cmple(lo).all() && s.max.cmpge(hi).all();
        for s in self.store.iter().filter(|s| in_range(s.min, s.max) && !holds_range(s)) {
            let Some(parts) = cache.get(Owner::Ybn(s.hash), now, || Some(Job::Ybn(s.hash, s.file))) else { continue };
            for (i, p) in parts.iter().enumerate() {
                out.push(p.placed(key(s.hash, STATIC, i), &Mat4::IDENTITY));
            }
        }

        // The interiors' (`RDR2MloInstance.BoundsEntities`), whose boxes in
        // the cache are in the interior's space.
        for (map, helpers) in c.world.map_helpers() {
            for interior in &helpers.interiors {
                let Some(files) = data.interiors.get(&interior.archetype).and_then(|i| i.bounds.as_ref()) else {
                    continue;
                };
                let place = Mat4::from_cols_array(&interior.transform);
                for (n, &hash) in files.iter().enumerate() {
                    let near = match data.bounds_cache.get(&hash) {
                        Some(b) => {
                            let (min, max) = world_box(&place, b.min.into(), b.max.into());
                            in_range(min, max)
                        }
                        None => place.w_axis.truncate().distance(c.eye) <= RANGE,
                    };
                    if !near {
                        continue;
                    }
                    let job = || data.stream_entry("ybn", hash).map(|f| Job::Ybn(hash, f));
                    let Some(parts) = cache.get(Owner::Ybn(hash), now, job) else { continue };
                    let group = INTERIOR | (interior.entity & (INTERIOR - 1));
                    for (i, p) in parts.iter().enumerate() {
                        out.push(p.placed(key(map, group, (n << 16) | (i & 0xFFFF)), &place));
                    }
                }
            }
        }

        // The entities' own, as the C# streams them (instanced ones aren't
        // among these).
        for e in c.world.visible().iter().filter(|e| e.distance <= RANGE) {
            let archetype = e.archetype;
            let job = || rf_world::model_files(&data, archetype).map(|mf| Job::Model(archetype, mf));
            let Some(parts) = cache.get(Owner::Archetype(archetype), now, job) else { continue };
            let place = Mat4::from_cols_array(&e.transform);
            let inner = (e.id.interior_entity.wrapping_add(1) & 0xFFFF) as usize;
            for (i, p) in parts.iter().enumerate() {
                out.push(p.placed(key(e.id.node, e.id.entity & (INTERIOR - 1), (inner << 16) | (i & 0xFFFF)), &place));
            }
        }

        cache.trim(now);
    }
}

/// The world box around a box through a transform.
fn world_box(m: &Mat4, min: Vec3, max: Vec3) -> (Vec3, Vec3) {
    (0..8).fold((Vec3::MAX, Vec3::MIN), |(lo, hi), i| {
        let corner = Vec3::new(
            if i & 1 != 0 { max.x } else { min.x },
            if i & 2 != 0 { max.y } else { min.y },
            if i & 4 != 0 { max.z } else { min.z },
        );
        let p = m.transform_point3(corner);
        (lo.min(p), hi.max(p))
    })
}
