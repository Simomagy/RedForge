//! RDR2's world: which map data (ymaps) and entities are around a camera, as
//! the C# RedForge's `RDR2Map` streams them (`StreamingInit`,
//! `StreamingUpdate`, `RecurseAddStreamEntity`).
//!
//! The renderer only sees [`World::update`] and [`World::visible`]: entities
//! with their archetype and final transform. [`model_files`] says which files
//! make an archetype's model and textures.
//!
//! ```ignore
//! let mut world = World::new(fs, hashes, data.clone());
//! loop {
//!     world.update(camera_position, hour); // never blocks: maps load on worker threads
//!     for e in world.visible() {
//!         let files = model_files(&data, e.archetype); // load, then draw with e.transform
//!     }
//! }
//! ```
//!
//! # How it streams
//!
//! - **Map nodes** (`RDR2MapNode`): one per ymap stream entry, from the map
//!   data cache (every ymap of the game is in it): its streaming box and parent
//!   ymap, in a quadtree over the streaming boxes (`StreamingBVH`).
//! - **Streamed nodes**: those whose streaming box contains the camera (in 3D,
//!   so flying high leaves the HD maps' boxes), unless disabled or culled by a
//!   cull box the camera is in, and all their parents. Worked out again when the
//!   camera has moved 1 m, as in the C#.
//! - **Loading**: streamed nodes that aren't loaded are read on a few worker
//!   threads, parents first: the ymap is extracted and read, its entities placed
//!   (`RDR2MapData`, `RDR2Entity`) with its interiors' entities
//!   (`RDR2MloInstance`, from the MLO archetype, read once). [`World::update`]
//!   takes what's done and asks for what's missing. A map that's no longer
//!   streamed is dropped [a while later](World::set_evict_after).
//! - **LOD tree**: an entity's `parentIndex` is its LOD parent in the same map,
//!   or with `LodInParentMap` in the parent map, linked while both are streamed
//!   and loaded (`LevelLod.SetParent`).
//! - **Visible entities** (`RecurseAddStreamEntity`), from the streamed maps'
//!   roots: an entity in range (`lodDist`) shows its children instead when all
//!   are loaded and the camera is within its `childLodDist`, else itself; one
//!   out of range still shows while its LOD parent is switched to its children.
//!   Time archetypes only show at their hours. An interior shows its entities
//!   and those of its default entity sets. Distances are from the streamed
//!   camera position, 0 inside an entity's box.
//! - **Cull boxes** (`common:/data/mapdatacullboxes_new.meta`) and **map states**
//!   (`data/rdr2/mapstates.txt`, the C#'s `RedForge.Games.RDR2.mapstates.txt`),
//!   applied at startup as in the C# (`EnableIPL`, `EnableCullBox`).
//!
//! # Differences from the C#
//!
//! - Loading is asynchronous: until a map is loaded, its LOD parents show in
//!   place of its entities (the C# loads everything before it draws).
//! - An entity whose LOD parent is in the parent map is only reached through
//!   that parent once the maps are linked. The C# also walks it as a root of its
//!   own map, so between the parent's `childLodDist` and the child's `lodDist`
//!   (a barrel's LOD switching at 50 m, its HD reaching 90 m) both show: about a
//!   hundred such pairs around a town. Nothing else changes: every entity this
//!   leaves out has its LOD parent (or an ancestor) shown.
//! - An entity's box is its archetype's box through its transform; the C# uses
//!   the loaded model's (the same box), so it has none before the model loads,
//!   nor for an interior instance.
//! - An LOD parent's distance is worked out when needed; the C# may use the one
//!   from the previous update when the child is visited first.
//! - Unreadable entities keep their place, so `parentIndex` counts them as the
//!   game does; the C# skips them, shifting the indices after them (there are
//!   none in the game's files).
//!
//! # Not ported
//!
//! Trees' imposters past their last LOD (`RDR2ImposterManager`), physics and
//! static bounds, navmeshes and paths, lights, time cycle modifiers, occluders,
//! AMV zones, scenario regions, interior rooms and portals, the map data
//! groups' bounds, `EnableIPL` for a ymap missing from the map data cache (the
//! C# loads it then; the game has none), and the editing hooks
//! (`UpdateProjectItem`, pinned project ymaps...). `EnableInteriorEntitySet`
//! (from the map states) only records a state the C# never reads, so it's left
//! out.
//!
//! # Known issues
//!
//! - Scripted ymaps (`flags & 1`: lockdowns, gang camps, story and MP states)
//!   show as in the C#, where only ymaps missing from the cache are disabled
//!   for being scripted. The map states turn off the Valentine shootout
//!   variants; others, like `val_03_lockdown`, show along with the default
//!   `val_03_default_jail`. [`World::set_hide_scripted`] hides them all, but the
//!   default states are scripted too (the game's scripts request them).
//! - An LOD whose children aren't all loaded shows instead of them, so a place
//!   whose LOD has children in a ymap that isn't streamed stays at the LOD.
//! - A ymap's parents are streamed with it even when disabled, as in the C#, so
//!   a disabled parent of a streamed ymap still shows.

mod batch;
mod edit;
mod map;
mod math;
mod nodes;
mod places;
mod stream;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use rf_formats::formats::databag::DataBag;
use rf_formats::hash::{HashIndex, joaat};
use rf_formats::rpf8::{FileRef, Rpf8FileSystem};
use rf_game::{AssetType, GameData};

pub use crate::batch::{BatchCell, InstanceBatch, read_batches};
pub use crate::edit::EntityInfo;
use crate::map::{MapData, Sources};
pub use crate::math::Aabb;
use crate::nodes::{Bvh, MAP_STATES, Node};
pub use crate::nodes::{CullBox, MapStateEntitySet, MapStateGroup, parse_map_states};
use crate::stream::{EntityRef, Loaded};

/// A batch of instanced props or grass in range of the camera.
#[derive(Clone, Debug)]
pub struct VisibleBatch {
    /// Its map's name and its index in the map, which stay the same when the
    /// map is loaded again.
    pub key: (u32, u32),
    pub batch: Arc<InstanceBatch>,
}

/// An entity to draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VisibleEntity {
    /// The archetype's name hash.
    pub archetype: u32,
    /// Model to world, column-major (as `glam::Mat4::from_cols_array`), as
    /// the C# engine builds it from the entity's position, rotation and scale
    /// (and its interior's, for an entity in an interior).
    pub transform: [f32; 16],
    /// From the camera, as the C# streaming computes it (0 inside the
    /// entity's bounds); for picking the drawable's own LOD.
    pub distance: f32,
    /// `lodLevel`: 0 orphan HD, 1 HD, 2 LOD, 3+ SLOD.
    pub lod_level: u8,
    pub tint: u32,
    /// Which map node and entity this is, for selecting it later.
    pub id: EntityId,
}

/// A map node (ymap) and an entity's index in it; interior entities also carry
/// their index in the interior.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EntityId {
    /// The ymap's name hash.
    pub node: u32,
    /// The entity's index in the ymap's `entities` (for an interior entity, the
    /// interior instance's).
    pub entity: u32,
    /// For an entity of an interior (MLO): its index there, else `u32::MAX`.
    /// The interior archetype's `entities` come first, then each entity set's.
    pub interior_entity: u32,
}

#[derive(Clone, Debug, Default)]
pub struct WorldStats {
    pub nodes: usize,
    /// Loaded, whether still streamed or about to be dropped.
    pub nodes_loaded: usize,
    pub nodes_loading: usize,
    pub entities_visible: usize,
    /// The nodes streamed now: around the camera, and their parents.
    pub nodes_streamed: usize,
    /// The interiors whose entities show.
    pub interiors_visible: usize,
    /// The cull boxes the camera is in.
    pub cull_boxes_active: usize,
}

/// The kind of file an archetype's model is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModelKind {
    Ydr,
    /// A drawable dictionary; the model is its entry named `ModelFiles::entry`.
    Ydd,
    Yft,
}

/// What an archetype's model is made of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelFiles {
    pub kind: ModelKind,
    pub file: FileRef,
    /// For a ydd: the entry's name hash (the archetype's `assetName`); 0 for
    /// the others.
    pub entry: u32,
    /// The texture dictionaries to look in, best first (`GetTxds`).
    pub txds: Vec<FileRef>,
}

/// Where an archetype's model and textures are (`RDR2MapFileCache.GetPiece`):
/// a drawable is the ydr named by `assetName`, a drawable dictionary's the
/// entry `assetName` of the ydd `drawableDictionary`, a fragment the yft named
/// by `assetName`. `None` for an unknown or assetless archetype, or when the
/// file isn't streamed.
pub fn model_files(data: &GameData, archetype: u32) -> Option<ModelFiles> {
    let a = data.archetypes.get(&archetype)?;
    let (kind, ext, name, entry) = match a.asset_type {
        AssetType::DRAWABLE => (ModelKind::Ydr, "ydr", a.asset_name, 0),
        AssetType::DRAWABLE_DICTIONARY => (ModelKind::Ydd, "ydd", a.drawable_dictionary, a.asset_name),
        AssetType::FRAGMENT => (ModelKind::Yft, "yft", a.asset_name, 0),
        _ => return None,
    };
    let file = data.stream_entry(ext, name)?;
    let txds = data.txds(a.asset_type, a.asset_name, a.texture_dictionary);
    Some(ModelFiles { kind, file, entry, txds })
}

/// How long a map stays loaded after it stops being streamed, by default.
const EVICT_AFTER: Duration = Duration::from_secs(20);
/// `StreamingUpdateDistance`: how far the camera moves before re-streaming.
const UPDATE_DISTANCE: f32 = 1.0;

/// A finished load.
enum Done {
    Loaded(u32, Option<MapData>),
    Cancelled(u32),
}

/// The loading threads.
pub(crate) struct Loader {
    pool: rayon::ThreadPool,
    tx: mpsc::Sender<Done>,
    rx: mpsc::Receiver<Done>,
    src: Arc<Sources>,
}

impl Loader {
    pub(crate) fn new(src: Arc<Sources>) -> Self {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads((cores / 2).clamp(2, 8))
            .thread_name(|i| format!("rf-world-{i}"))
            .build()
            .expect("map loading threads");
        let (tx, rx) = mpsc::channel();
        Loader { pool, tx, rx, src }
    }

    /// Loads a ymap, unless cancelled before its turn comes.
    fn request(&self, node: u32, file: FileRef, cancel: Arc<AtomicBool>) {
        let (tx, src) = (self.tx.clone(), self.src.clone());
        self.pool.spawn_fifo(move || {
            let done = if cancel.load(Ordering::Relaxed) {
                Done::Cancelled(node)
            } else {
                Done::Loaded(node, MapData::load(&src, file))
            };
            let _ = tx.send(done);
        });
    }
}

/// The map around a camera. See the [crate docs](crate).
pub struct World {
    nodes: Vec<Node>,
    by_name: HashMap<u32, u32>,
    bvh: Bvh,
    cull_boxes: Vec<CullBox>,
    map_states: Vec<MapStateGroup>,
    places: Vec<(String, [f32; 3])>,
    filter_by_time: bool,
    hide_scripted: bool,
    evict_after: Duration,

    loader: Loader,
    loaded: HashMap<u32, Loaded>,
    /// Edited or new map data, by node: used instead of the file, and never
    /// dropped.
    overrides: HashMap<u32, Arc<DataBag>>,
    /// Entities hidden in the editor (they're still in their maps).
    hidden: HashSet<EntityId>,
    /// Requested loads, with the flag that cancels them.
    pending: HashMap<u32, Arc<AtomicBool>>,
    /// The streamed nodes, sorted.
    streamed: Vec<u32>,
    /// The position streamed around (it follows the camera in steps of
    /// [`UPDATE_DISTANCE`]).
    camera: Option<[f32; 3]>,
    hour: i32,
    /// What has to be worked out again on the next update.
    restream: bool,
    relink: bool,
    rewalk: bool,
    stamp: u32,
    visible: Vec<VisibleEntity>,
    visible_batches: Vec<VisibleBatch>,
    interiors_visible: usize,
    cull_boxes_active: usize,
    last_evict: Instant,
}

impl World {
    pub fn new(fs: Arc<Rpf8FileSystem>, hashes: Arc<HashIndex>, data: Arc<GameData>) -> Self {
        // `StreamingInit`: a node per ymap, all from the map data cache.
        let mut nodes = Vec::new();
        for (hash, file) in data.stream_entries("ymap") {
            if let Some(c) = data.map_data_cache.get(&hash) {
                let path = fs.path(file);
                let name = path.rsplit('/').next().unwrap_or(&path);
                let name = name.rsplit_once('.').map_or(name, |(s, _)| s).to_lowercase();
                nodes.push(Node::cached(file, name, c));
            } else {
                // The C# makes a disabled node, which only `EnableIPL` loads.
                log::debug!("{}: not in the map data cache", fs.path(file));
            }
        }
        let by_name: HashMap<u32, u32> = nodes.iter().enumerate().map(|(i, n)| (n.name, i as u32)).collect();
        for n in &mut nodes {
            n.parent_index = (n.parent != 0).then(|| by_name.get(&n.parent).copied()).flatten();
        }
        for i in 0..nodes.len() {
            let chain = std::iter::successors(nodes[i].parent_index, |&j| nodes[j as usize].parent_index);
            nodes[i].depth = chain.take(64).count() as u32;
        }
        let bvh = build_bvh(&nodes);

        let path = "common:/data/mapdatacullboxes_new.meta";
        let cull_boxes = match data.meta_load(&fs, &hashes, path) {
            Some(doc) => CullBox::read_file(&doc.bag),
            None => {
                log::warn!("{path}: not found");
                Vec::new()
            }
        };
        let places = places::places(&nodes);
        let src = Arc::new(Sources { fs, hashes, data, interiors: Mutex::new(HashMap::new()) });
        let mut world = World {
            nodes,
            by_name,
            bvh,
            cull_boxes,
            map_states: Vec::new(),
            places,
            filter_by_time: true,
            hide_scripted: false,
            evict_after: EVICT_AFTER,
            loader: Loader::new(src),
            loaded: HashMap::new(),
            overrides: HashMap::new(),
            hidden: HashSet::new(),
            pending: HashMap::new(),
            streamed: Vec::new(),
            camera: None,
            hour: -1,
            restream: true,
            relink: false,
            rewalk: false,
            stamp: 0,
            visible: Vec::new(),
            visible_batches: Vec::new(),
            interiors_visible: 0,
            cull_boxes_active: 0,
            last_evict: Instant::now(),
        };
        world.set_map_states(parse_map_states(MAP_STATES));
        log::info!(
            "world: {} map nodes, {} cull boxes, {} map state groups",
            world.nodes.len(),
            world.cull_boxes.len(),
            world.map_states.len()
        );
        world
    }

    /// Streams around `camera` at hour `hour` (0-24): starts loading the map
    /// data that comes into range on worker threads, drops what went out of
    /// range, and works out the visible entities from what's loaded. Never
    /// blocks on loading; call it every frame (it's cheap when nothing moved).
    pub fn update(&mut self, camera: [f32; 3], hour: f32) {
        self.receive();
        let moved = self.camera.is_none_or(|c| math::distance(c, camera) >= UPDATE_DISTANCE);
        if moved {
            self.camera = Some(camera);
        }
        if moved || self.restream {
            self.stream_nodes();
        }
        // `TimeOfDayHour`.
        let hour = if self.filter_by_time { (hour as i32) % 24 } else { -1 };
        if hour != self.hour {
            self.hour = hour;
            self.rewalk = true;
        }
        if self.relink {
            let nodes = &self.nodes;
            stream::link(&mut self.loaded, &self.streamed, &|n| nodes[n as usize].parent_index);
            self.relink = false;
            self.rewalk = true;
        }
        if self.rewalk
            && let Some(camera) = self.camera
        {
            self.stamp = self.stamp.wrapping_add(1).max(1);
            let nodes = &self.nodes;
            let names = |n: u32| nodes[n as usize].name;
            self.interiors_visible =
                stream::walk(&self.loaded, &self.streamed, &names, camera, self.hour, self.stamp, &mut self.visible);
            if !self.hidden.is_empty() {
                self.visible.retain(|e| !self.hidden.contains(&e.id));
            }
            self.visible_batches.clear();
            for &node in &self.streamed {
                let Some(l) = self.loaded.get(&node) else { continue };
                for (i, b) in l.map.batches.iter().enumerate() {
                    let inside = b.bounds.contains(camera);
                    if inside || math::distance(b.bounds.centre(), camera) < b.lod_dist_max {
                        let key = (nodes[node as usize].name, i as u32);
                        self.visible_batches.push(VisibleBatch { key, batch: b.clone() });
                    }
                }
            }
            self.rewalk = false;
        }
        self.evict();
    }

    pub fn visible(&self) -> &[VisibleEntity] {
        &self.visible
    }

    /// The prop and grass batches in range (`RecurseAddStreamEntity`'s
    /// batches): their instances show by cell, each cell at its LOD.
    pub fn visible_batches(&self) -> &[VisibleBatch] {
        &self.visible_batches
    }

    pub fn stats(&self) -> WorldStats {
        WorldStats {
            nodes: self.nodes.len(),
            nodes_loaded: self.loaded.len(),
            nodes_loading: self.pending.len(),
            entities_visible: self.visible.len(),
            nodes_streamed: self.streamed.len(),
            interiors_visible: self.interiors_visible,
            cull_boxes_active: self.cull_boxes_active,
        }
    }

    /// Named places to jump to: towns and landmarks, with a camera position.
    /// Each is found from its ymaps (`val_*` for Valentine...): the middle of
    /// their streaming boxes, 30 m up.
    pub fn places(&self) -> Vec<(String, [f32; 3])> {
        self.places.clone()
    }

    /// The LOD parent of an entity of a loaded map (for a parent in the parent
    /// map, while the maps are linked). `None` for interior entities.
    pub fn lod_parent(&self, id: EntityId) -> Option<EntityId> {
        if id.interior_entity != u32::MAX {
            return None;
        }
        let node = *self.by_name.get(&id.node)?;
        let e = self.loaded.get(&node)?.map.entities.get(id.entity as usize)?;
        let p = stream::lod_parent(&self.loaded, EntityRef { node, entity: id.entity }, e)?;
        Some(EntityId { node: self.nodes[p.node as usize].name, entity: p.entity, interior_entity: u32::MAX })
    }

    // -----------------------------------------------------------------------
    // Settings

    /// `FilterEntitiesByTime` (on by default): hide time archetypes outside
    /// their hours.
    pub fn set_filter_by_time(&mut self, on: bool) {
        self.filter_by_time = on;
        self.rewalk = true;
    }

    /// Hide the ymaps found to be scripted (`flags & 1`) once loaded, unless
    /// enabled with [`World::set_ipl_enabled`]. Off by default, as in the C#:
    /// the default states (`val_03_default_jail`...) are scripted too.
    pub fn set_hide_scripted(&mut self, on: bool) {
        self.hide_scripted = on;
        self.restream = true;
    }

    /// How long a map that's no longer streamed stays loaded (20 s by default),
    /// in case the camera comes back.
    pub fn set_evict_after(&mut self, after: Duration) {
        self.evict_after = after;
    }

    /// `EnableIPL` (the game's `REQUEST_IPL`/`REMOVE_IPL`): turns a ymap on or
    /// off by name hash. As in the C#, its parents are turned the same way, up
    /// to the first one already so.
    pub fn set_ipl_enabled(&mut self, name: u32, enabled: bool) {
        let Some(&i) = self.by_name.get(&name) else {
            if enabled {
                log::warn!("EnableIPL: 0x{name:08X} isn't in the map data cache");
            }
            return;
        };
        let n = &mut self.nodes[i as usize];
        n.enabled = enabled;
        n.requested = enabled;
        let mut p = n.parent_index;
        for _ in 0..64 {
            let Some(j) = p else { break };
            let n = &mut self.nodes[j as usize];
            if n.enabled == enabled {
                break;
            }
            n.enabled = enabled;
            p = n.parent_index;
        }
        self.restream = true;
    }

    /// Whether a ymap is streamed when the camera is in its box: not turned off
    /// (by the map states or [`World::set_ipl_enabled`]), nor scripted and
    /// hidden. `None` for an unknown ymap.
    pub fn ipl_enabled(&self, name: u32) -> Option<bool> {
        let n = &self.nodes[*self.by_name.get(&name)? as usize];
        Some(n.streamable(self.hide_scripted))
    }

    pub fn cull_boxes(&self) -> &[CullBox] {
        &self.cull_boxes
    }

    /// `EnableCullBox` (the game's `SET_MAPDATACULLBOX_ENABLED`): every cull box
    /// of that name (some names are used twice).
    pub fn set_cull_box_enabled(&mut self, name: &str, enabled: bool) {
        let h = joaat(name);
        for c in &mut self.cull_boxes {
            if c.name_hash() == h {
                c.enabled = enabled;
            }
        }
        self.restream = true;
    }

    pub fn map_states(&self) -> &[MapStateGroup] {
        &self.map_states
    }

    /// `LoadMapStates`: applies each group (`EnableMapStateGroup`): its cull
    /// box, and its IPLs, each on while its state has its value.
    pub fn set_map_states(&mut self, groups: Vec<MapStateGroup>) {
        for g in &groups {
            if let Some(c) = g.cull_box.as_deref().filter(|c| !c.is_empty()) {
                self.set_cull_box_enabled(&c.to_lowercase(), g.enabled);
            }
            for (ipl, state, value) in &g.ipls {
                if !ipl.is_empty() {
                    let on = g.is_state_active(state.as_deref(), value.as_deref());
                    self.set_ipl_enabled(joaat(&ipl.to_lowercase()), on);
                }
            }
        }
        self.map_states = groups;
    }

    // -----------------------------------------------------------------------
    // Streaming

    /// Takes the finished loads.
    fn receive(&mut self) {
        while let Ok(done) = self.loader.rx.try_recv() {
            match done {
                Done::Loaded(node, map) => {
                    self.pending.remove(&node);
                    if self.overrides.contains_key(&node) {
                        continue;
                    }
                    let n = &mut self.nodes[node as usize];
                    let map = map.unwrap_or_else(|| {
                        log::warn!("{}: couldn't be read", n.file_name);
                        MapData::default()
                    });
                    n.scripted = Some(map.is_scripted());
                    if self.loaded.contains_key(&node) {
                        continue;
                    }
                    let mut l = Loaded::new(map);
                    if self.streamed.binary_search(&node).is_ok() {
                        self.relink = true;
                        if !n.streamable(self.hide_scripted) {
                            self.restream = true; // hidden, now it's known to be scripted
                        }
                    } else {
                        l.unwanted_since = Some(Instant::now());
                    }
                    self.loaded.insert(node, l);
                }
                Done::Cancelled(node) => {
                    self.pending.remove(&node);
                    if self.streamed.binary_search(&node).is_ok() {
                        self.restream = true; // wanted again since
                    }
                }
            }
        }
    }

    /// The first half of `StreamingUpdate`: the streamed nodes, and loading
    /// the missing ones.
    fn stream_nodes(&mut self) {
        self.restream = false;
        let Some(camera) = self.camera else { return };
        let mut culled = HashSet::new();
        self.cull_boxes_active = 0;
        for c in self.cull_boxes.iter().filter(|c| c.enabled && c.contains(camera)) {
            self.cull_boxes_active += 1;
            culled.extend(c.culled.iter().copied());
        }
        let mut hits = Vec::new();
        self.bvh.query(camera, &mut hits);
        let mut wanted = HashSet::new();
        for hit in hits {
            let node = &self.nodes[hit as usize];
            if node.name == 0 || !node.streamable(self.hide_scripted) {
                continue;
            }
            let chain = || std::iter::successors(Some(hit), |&i| self.nodes[i as usize].parent_index).take(64);
            // Culled with its children.
            if chain().any(|i| culled.contains(&self.nodes[i as usize].name)) {
                continue;
            }
            for i in chain() {
                if !wanted.insert(i) {
                    break;
                }
            }
        }
        let mut wanted: Vec<u32> = wanted.into_iter().collect();
        wanted.sort_unstable();
        if wanted == self.streamed {
            // Loads cancelled while the camera was away.
            self.request(wanted.iter().copied().filter(|n| !self.loaded.contains_key(n)).collect());
            return;
        }

        let now = Instant::now();
        for &n in &self.streamed {
            if wanted.binary_search(&n).is_err() {
                if let Some(l) = self.loaded.get_mut(&n) {
                    l.unwanted_since = Some(now);
                }
                if let Some(c) = self.pending.get(&n) {
                    c.store(true, Ordering::Relaxed);
                }
            }
        }
        let mut missing = Vec::new();
        for &n in &wanted {
            if let Some(l) = self.loaded.get_mut(&n) {
                l.unwanted_since = None;
            } else {
                missing.push(n);
            }
        }
        self.request(missing);
        self.streamed = wanted;
        self.relink = true;
    }

    /// Loads what isn't loading yet, parents first so something shows soon.
    fn request(&mut self, mut nodes: Vec<u32>) {
        nodes.sort_by_key(|&n| self.nodes[n as usize].depth);
        for n in nodes {
            if let Some(c) = self.pending.get(&n) {
                c.store(false, Ordering::Relaxed);
                continue;
            }
            let Some(file) = self.nodes[n as usize].file else { continue };
            let cancel = Arc::new(AtomicBool::new(false));
            self.loader.request(n, file, cancel.clone());
            self.pending.insert(n, cancel);
        }
    }

    /// Drops the maps no longer streamed for a while.
    fn evict(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_evict) < Duration::from_secs(1) {
            return;
        }
        self.last_evict = now;
        let after = self.evict_after;
        let overrides = &self.overrides;
        self.loaded
            .retain(|n, l| overrides.contains_key(n) || l.unwanted_since.is_none_or(|t| now.duration_since(t) < after));
    }

    fn rebuild_bvh(&mut self) {
        self.bvh = build_bvh(&self.nodes);
    }
}

/// The streaming BVH over the nodes that have a streaming box.
fn build_bvh(nodes: &[Node]) -> Bvh {
    let boxes = nodes.iter().map(|n| n.streaming).collect();
    Bvh::build(boxes, (0..nodes.len() as u32).filter(|&i| nodes[i as usize].has_streaming_box()))
}

impl Drop for World {
    fn drop(&mut self) {
        for c in self.pending.values() {
            c.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_is_send() {
        fn send<T: Send>() {}
        send::<World>();
    }

    #[test]
    fn models_by_asset_type() {
        let mut gd =
            GameData { stream_entries: vec![Default::default(); rf_game::STREAM_TABLES], ..Default::default() };
        let f = |entry| FileRef { archive: 0, entry };
        let index = |ext| rf_game::stream_index(rf_game::ext_id(ext).unwrap());
        gd.stream_entries[index("ydr")].insert(joaat("rock"), f(1));
        gd.stream_entries[index("ydd")].insert(joaat("trees"), f(2));
        gd.stream_entries[index("yft")].insert(joaat("door"), f(3));
        gd.stream_entries[index("ytd")].insert(joaat("rock_txd"), f(4));
        let mut add = |name: &str, t: AssetType, dict: &str, txd: &str| {
            let mut a = map::tests::sample_archetype(name);
            a.asset_type = t;
            a.drawable_dictionary = joaat(dict);
            a.texture_dictionary = joaat(txd);
            gd.archetypes.insert(a.name, a);
        };
        add("rock", AssetType::DRAWABLE, "", "rock_txd");
        add("tree", AssetType::DRAWABLE_DICTIONARY, "trees", "");
        add("door", AssetType::FRAGMENT, "", "");
        add("marker", AssetType::ASSETLESS, "", "");
        add("lost", AssetType::DRAWABLE, "", "");
        let m = model_files(&gd, joaat("rock")).unwrap();
        assert_eq!((m.kind, m.file, m.entry, m.txds), (ModelKind::Ydr, f(1), 0, vec![f(4)]));
        let m = model_files(&gd, joaat("tree")).unwrap();
        assert_eq!((m.kind, m.file, m.entry), (ModelKind::Ydd, f(2), joaat("tree")));
        assert_eq!(model_files(&gd, joaat("door")).unwrap().kind, ModelKind::Yft);
        assert_eq!(model_files(&gd, joaat("marker")), None);
        assert_eq!(model_files(&gd, joaat("lost")), None);
        assert_eq!(model_files(&gd, joaat("nothing")), None);
    }
}
