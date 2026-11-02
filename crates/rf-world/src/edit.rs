//! Edited map data: a `CMapData` that replaces a ymap's file, or a new ymap,
//! and what an editor needs to know about the entities it shows.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use rf_formats::formats::databag::DataBag;
use rf_formats::rpf8::FileRef;

use crate::map::{Entity, InteriorDef, MapData};
use crate::math::{self, Aabb};
use crate::nodes::Node;
use crate::stream::Loaded;
use crate::{EntityId, World};

/// An entity as the world places it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntityInfo {
    pub archetype: u32,
    /// Model to world, column-major.
    pub transform: [f32; 16],
    /// `lodLevel`: 0 orphan HD, 1 HD, 2 LOD, 3+ SLOD.
    pub lod_level: u8,
    /// Whether it's an interior (MLO) instance.
    pub is_interior: bool,
}

/// What a node's streaming box is when it has none (the map data cache's
/// "none").
const NO_BOX: Aabb = Aabb { min: [-1.0; 3], max: [-1.0; 3] };

impl World {
    /// Uses `bag` (a `CMapData`) for the ymap `name` instead of its file; for a
    /// name the world doesn't know, adds it as a new ymap called `file_name`.
    /// The map is rebuilt now and stays loaded until [`World::clear_map`];
    /// its streaming box grows to take in its entities (each one's box and
    /// `lodDist` around it).
    pub fn set_map(&mut self, name: u32, file_name: &str, bag: Arc<DataBag>) {
        let node = match self.by_name.get(&name) {
            Some(&n) => n,
            None => self.add_node(name, file_name, &bag),
        };
        if let Some(cancel) = self.pending.remove(&node) {
            cancel.store(true, Ordering::Relaxed);
        }
        let src = &self.loader.src;
        let map = MapData::build(&bag, &src.data.archetypes, &|a| src.interior(a));

        let n = &mut self.nodes[node as usize];
        n.scripted = Some(map.is_scripted());
        n.enabled = true;
        let mut reach = if n.has_streaming_box() { n.streaming } else { Aabb::EMPTY };
        for e in map.entities.iter().filter(|e| e.valid && e.bounds != Aabb::EMPTY) {
            let d = e.lod_dist.max(0.0);
            reach.extend(e.bounds.min.map(|v| v - d));
            reach.extend(e.bounds.max.map(|v| v + d));
        }
        let grown = reach != Aabb::EMPTY && reach != n.streaming;
        if grown {
            n.streaming = reach;
        }

        let mut l = Loaded::new(map);
        if self.streamed.binary_search(&node).is_err() {
            l.unwanted_since = Some(Instant::now());
        }
        self.loaded.insert(node, l);
        self.overrides.insert(node, bag);
        if grown {
            self.rebuild_bvh();
        }
        self.restream = true;
        self.relink = true;
    }

    /// Uses other game data (the game's with a project's files): new loads,
    /// and the edited maps placed again now, use it. Maps already loaded stay
    /// as they are (their archetypes' boxes and LOD distances only change for
    /// archetypes a project replaces).
    pub fn set_data(&mut self, data: Arc<rf_game::GameData>) {
        for cancel in self.pending.values() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.pending.clear();
        let old = &self.loader.src;
        let src = Arc::new(crate::map::Sources {
            fs: old.fs.clone(),
            hashes: old.hashes.clone(),
            data,
            interiors: std::sync::Mutex::new(std::collections::HashMap::new()),
        });
        self.loader = crate::Loader::new(src);
        let overrides: Vec<(u32, Arc<DataBag>)> = self.overrides.iter().map(|(n, b)| (*n, b.clone())).collect();
        for (node, bag) in overrides {
            let src = &self.loader.src;
            let map = MapData::build(&bag, &src.data.archetypes, &|a| src.interior(a));
            let mut l = Loaded::new(map);
            if self.streamed.binary_search(&node).is_err() {
                l.unwanted_since = Some(Instant::now());
            }
            self.loaded.insert(node, l);
        }
        self.restream = true;
        self.relink = true;
    }

    /// Goes back to the ymap's file (or, for a new ymap, removes it). With the
    /// file's data in `original`, the map is rebuilt from it now; else it's
    /// read again like any other.
    pub fn clear_map(&mut self, name: u32, original: Option<&DataBag>) {
        let Some(&node) = self.by_name.get(&name) else { return };
        if self.overrides.remove(&node).is_none() {
            return;
        }
        match original {
            Some(bag) if self.nodes[node as usize].file.is_some() => {
                let src = &self.loader.src;
                let mut l = Loaded::new(MapData::build(bag, &src.data.archetypes, &|a| src.interior(a)));
                if self.streamed.binary_search(&node).is_err() {
                    l.unwanted_since = Some(Instant::now());
                }
                self.loaded.insert(node, l);
            }
            _ => {
                self.loaded.remove(&node);
            }
        }
        if self.nodes[node as usize].file.is_none() {
            self.nodes[node as usize].enabled = false;
        }
        self.restream = true;
        self.relink = true;
    }

    /// Uses `def` (a `CMloArchetypeDef`) for an interior archetype instead of
    /// its ytyp's, or with `None` goes back to the ytyp; every placed instance
    /// is placed again now.
    pub fn set_interior(&mut self, archetype: u32, def: Option<&DataBag>) {
        let src = &self.loader.src;
        let def = match def {
            Some(bag) => Some(Arc::new(InteriorDef::read(bag))),
            None => {
                src.interiors.lock().unwrap().remove(&archetype);
                src.interior(archetype)
            }
        };
        src.interiors.lock().unwrap().insert(archetype, def.clone());
        let Some(def) = def else { return };
        for l in self.loaded.values_mut() {
            let MapData { entities, interiors, .. } = &mut l.map;
            for e in entities.iter().filter(|e| e.archetype == archetype) {
                if let Some(i) = e.interior.and_then(|i| interiors.get_mut(i as usize)) {
                    i.rebuild(&def, &src.data.archetypes);
                }
            }
        }
        self.rewalk = true;
    }

    /// Hides an entity from [`World::visible`] (or shows it again): for the
    /// editor's view only.
    pub fn set_hidden(&mut self, id: EntityId, hidden: bool) {
        let changed = if hidden { self.hidden.insert(id) } else { self.hidden.remove(&id) };
        self.rewalk |= changed;
    }

    pub fn is_hidden(&self, id: EntityId) -> bool {
        self.hidden.contains(&id)
    }

    /// Shows every hidden entity again.
    pub fn show_all(&mut self) {
        self.rewalk |= !self.hidden.is_empty();
        self.hidden.clear();
    }

    /// The map data used for a ymap instead of its file, if any.
    pub fn map_override(&self, name: u32) -> Option<&Arc<DataBag>> {
        self.overrides.get(self.by_name.get(&name)?)
    }

    /// A ymap's file, for a ymap of the game.
    pub fn map_file(&self, name: u32) -> Option<FileRef> {
        self.nodes[*self.by_name.get(&name)? as usize].file
    }

    /// A ymap's file name: lowercase, without folder or extension.
    pub fn map_file_name(&self, name: u32) -> Option<&str> {
        Some(&self.nodes[*self.by_name.get(&name)? as usize].file_name)
    }

    /// The ymap a ymap's entities can have LOD parents in (`parent`), if any.
    pub fn parent_map(&self, name: u32) -> Option<u32> {
        let n = &self.nodes[*self.by_name.get(&name)? as usize];
        (n.parent != 0).then_some(n.parent)
    }

    /// The ymaps whose parent map is `name`: their entities with
    /// `LodInParentMap` point into it by index.
    pub fn child_maps(&self, name: u32) -> Vec<u32> {
        self.nodes.iter().filter(|n| n.parent == name && n.name != name).map(|n| n.name).collect()
    }

    /// An entity of a loaded map, as placed.
    pub fn entity(&self, id: EntityId) -> Option<EntityInfo> {
        let e = self.find(id)?;
        Some(EntityInfo {
            archetype: e.archetype,
            transform: e.transform,
            lod_level: e.lod_level,
            is_interior: e.interior.is_some() && id.interior_entity == u32::MAX,
        })
    }

    /// Shows an entity with another transform until its map changes (for
    /// dragging, without rebuilding the map every frame).
    pub fn preview_transform(&mut self, id: EntityId, transform: [f32; 16]) {
        let Some(&node) = self.by_name.get(&id.node) else { return };
        let archetypes = &self.loader.src.data.archetypes;
        let Some(e) = self.loaded.get_mut(&node).and_then(|l| find_mut(&mut l.map, id)) else { return };
        e.transform = transform;
        e.position = [transform[12], transform[13], transform[14]];
        if let Some(a) = archetypes.get(&e.archetype) {
            let mut b = Aabb::EMPTY;
            for c in 0..8 {
                let p = [0, 1, 2].map(|i| if c & (1 << i) != 0 { a.bb_max[i] } else { a.bb_min[i] });
                b.extend(math::transform_point(&transform, p));
            }
            e.bounds = b;
        }
        self.rewalk = true;
    }

    fn find(&self, id: EntityId) -> Option<&Entity> {
        let l = self.loaded.get(self.by_name.get(&id.node)?)?;
        let e = l.map.entities.get(id.entity as usize)?;
        if id.interior_entity == u32::MAX {
            return e.valid.then_some(e);
        }
        let interior = l.map.interiors.get(e.interior? as usize)?;
        interior.entities.get(id.interior_entity as usize).filter(|e| e.valid)
    }

    /// A node for a ymap that isn't in the game.
    fn add_node(&mut self, name: u32, file_name: &str, bag: &DataBag) -> u32 {
        let parent = bag.hash("parent");
        let parent_index = (parent != 0).then(|| self.by_name.get(&parent).copied()).flatten();
        let depth = std::iter::successors(parent_index, |&j| self.nodes[j as usize].parent_index).take(64).count();
        let index = self.nodes.len() as u32;
        self.nodes.push(Node {
            name,
            parent,
            parent_index,
            file: None,
            file_name: file_name.to_lowercase(),
            streaming: NO_BOX,
            enabled: true,
            requested: true,
            scripted: None,
            depth: depth as u32,
        });
        self.by_name.insert(name, index);
        index
    }
}

fn find_mut(map: &mut MapData, id: EntityId) -> Option<&mut Entity> {
    let e = map.entities.get(id.entity as usize)?;
    if id.interior_entity == u32::MAX {
        return map.entities.get_mut(id.entity as usize).filter(|e| e.valid);
    }
    let i = e.interior? as usize;
    map.interiors.get_mut(i)?.entities.get_mut(id.interior_entity as usize).filter(|e| e.valid)
}
