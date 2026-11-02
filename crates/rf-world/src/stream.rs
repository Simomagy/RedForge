//! The loaded maps around the camera, their LOD links, and
//! `RecurseAddStreamEntity`: which of their entities to draw, walking the LOD
//! tree down from the roots.

use std::cell::Cell;
use std::collections::HashMap;
use std::time::Instant;

use crate::map::{Entity, MapData};
use crate::math::Vec3;
use crate::{EntityId, VisibleEntity};

/// A map node's index and an entity's index in its map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EntityRef {
    pub node: u32,
    pub entity: u32,
}

/// A loaded map and what changes with the maps around it.
pub(crate) struct Loaded {
    pub map: MapData,
    /// Since when it hasn't been streamed (it's dropped a while after).
    pub unwanted_since: Option<Instant>,
    /// The parent map its roots are linked to (`LevelLod.SetParent`): while
    /// both maps are streamed and loaded.
    pub linked: Option<u32>,
    /// Per entity of this map: its LOD children in the child maps linked to it.
    pub ext_children: HashMap<u32, Vec<EntityRef>>,
    /// `SetParentNode`'s fix: an MLO's LOD parent without a `childLodDist`
    /// switches at the MLO's `lodDist`.
    pub child_lod_dist_fix: HashMap<u32, f32>,
    /// The walk an entity was last visited in.
    visited: Vec<Cell<u32>>,
}

impl Loaded {
    pub fn new(map: MapData) -> Self {
        let visited = (0..map.entities.len()).map(|_| Cell::new(0)).collect();
        Loaded {
            map,
            unwanted_since: None,
            linked: None,
            ext_children: HashMap::new(),
            child_lod_dist_fix: HashMap::new(),
            visited,
        }
    }

    /// `LodDistMin`.
    fn child_lod_dist(&self, entity: u32) -> f32 {
        self.child_lod_dist_fix.get(&entity).copied().unwrap_or(self.map.entities[entity as usize].child_lod_dist)
    }
}

/// `SetParentNode` for every streamed node: links each streamed, loaded map's
/// roots that have a parent in the parent map to that parent, when the parent
/// map is streamed and loaded too. `streamed` is sorted.
pub(crate) fn link(loaded: &mut HashMap<u32, Loaded>, streamed: &[u32], parents: &dyn Fn(u32) -> Option<u32>) {
    for l in loaded.values_mut() {
        l.linked = None;
        l.ext_children.clear();
        l.child_lod_dist_fix.clear();
    }
    for &node in streamed {
        let Some(parent) = parents(node) else { continue };
        if parent == node || !loaded.contains_key(&parent) || streamed.binary_search(&parent).is_err() {
            continue;
        }
        let Some(child) = loaded.get(&node) else { continue };
        let parent_count = loaded[&parent].map.entities.len();
        let mut links = Vec::new();
        for &r in &child.map.roots {
            let e = &child.map.entities[r as usize];
            if e.parent_in_parent_map && e.parent_index >= 0 && (e.parent_index as usize) < parent_count {
                links.push((e.parent_index as u32, r, e.interior.is_some().then_some(e.lod_dist)));
            }
        }
        loaded.get_mut(&node).unwrap().linked = Some(parent);
        let p = loaded.get_mut(&parent).unwrap();
        for (pi, r, mlo_lod_dist) in links {
            p.ext_children.entry(pi).or_default().push(EntityRef { node, entity: r });
            if let Some(d) = mlo_lod_dist
                && p.child_lod_dist(pi) == 0.0
            {
                p.child_lod_dist_fix.insert(pi, d);
            }
        }
    }
}

/// `LodParent` of the entity `e` at `r`: in the same map, or in the parent map
/// when linked.
pub(crate) fn lod_parent(loaded: &HashMap<u32, Loaded>, r: EntityRef, e: &Entity) -> Option<EntityRef> {
    if e.parent_index < 0 {
        return None;
    }
    let node = if e.parent_in_parent_map { loaded.get(&r.node)?.linked? } else { r.node };
    let parent = loaded.get(&node)?.map.entities.get(e.parent_index as usize)?;
    parent.valid.then_some(EntityRef { node, entity: e.parent_index as u32 })
}

/// The second half of `StreamingUpdate`: the entities to draw from the
/// streamed maps, walking down from their roots. `names` gives a node's ymap
/// name hash. Returns how many interiors show.
///
/// Unlike the C#, a root whose LOD parent (in the parent map) is linked is
/// only reached through that parent: the C# also starts from it, so between
/// the parent's `childLodDist` and the child's `lodDist` both show.
#[allow(clippy::too_many_arguments)]
pub(crate) fn walk(
    loaded: &HashMap<u32, Loaded>,
    streamed: &[u32],
    names: &dyn Fn(u32) -> u32,
    camera: Vec3,
    hour: i32,
    stamp: u32,
    out: &mut Vec<VisibleEntity>,
) -> usize {
    out.clear();
    let mut w = Walk { loaded, names, camera, hour, stamp, out, interiors: 0 };
    for &node in streamed {
        let Some(l) = loaded.get(&node) else { continue };
        for &entity in &l.map.roots {
            let r = EntityRef { node, entity };
            if lod_parent(loaded, r, &l.map.entities[entity as usize]).is_none() {
                w.visit(r);
            }
        }
    }
    w.interiors
}

struct Walk<'a> {
    loaded: &'a HashMap<u32, Loaded>,
    names: &'a dyn Fn(u32) -> u32,
    camera: Vec3,
    /// `TimeOfDayHour`, or -1 to show everything.
    hour: i32,
    stamp: u32,
    out: &'a mut Vec<VisibleEntity>,
    interiors: usize,
}

impl Walk<'_> {
    /// `RecurseAddStreamEntity`. What it finds for an entity doesn't depend on
    /// how it was reached, so each is visited once per walk.
    fn visit(&mut self, r: EntityRef) {
        let loaded = self.loaded;
        let l = &loaded[&r.node];
        let e = &l.map.entities[r.entity as usize];
        if !e.is_active(self.hour) {
            return;
        }
        let seen = &l.visited[r.entity as usize];
        if seen.get() == self.stamp {
            return;
        }
        seen.set(self.stamp);

        let d = e.distance(self.camera);
        if d < e.lod_dist {
            let own = l.map.children(r.entity);
            let ext = l.ext_children.get(&r.entity).map(Vec::as_slice).unwrap_or_default();
            let count = own.len() + ext.len();
            if count > 0 && count >= e.num_children as usize && d < l.child_lod_dist(r.entity) {
                for &c in own {
                    self.visit(EntityRef { node: r.node, entity: c });
                }
                for &c in ext {
                    self.visit(c);
                }
            } else {
                self.add(r, e, d, u32::MAX);
                if let Some(i) = e.interior {
                    self.interiors += 1;
                    let int = &l.map.interiors[i as usize];
                    for ie in int.shown() {
                        let ent = &int.entities[ie as usize];
                        // No LOD tree in an interior: only the in-range branch.
                        if ent.valid && ent.is_active(self.hour) {
                            let d = ent.distance(self.camera);
                            if d < ent.lod_dist {
                                self.add(r, ent, d, ie);
                            }
                        }
                    }
                }
            }
        } else if let Some(p) = lod_parent(loaded, r, e) {
            let pl = &loaded[&p.node];
            let pd = pl.map.entities[p.entity as usize].distance(self.camera);
            // The C#'s filter for HD water drawables until they're close.
            if pd < pl.child_lod_dist(p.entity) && (d < e.lod_dist * 2.0 || e.lod_level > 1) {
                self.add(r, e, d, u32::MAX);
            }
        }
    }

    fn add(&mut self, r: EntityRef, e: &Entity, distance: f32, interior_entity: u32) {
        // The interior instance itself has nothing to draw.
        if e.interior.is_some() && interior_entity == u32::MAX {
            return;
        }
        self.out.push(VisibleEntity {
            archetype: e.archetype,
            transform: e.transform,
            distance,
            lod_level: e.lod_level,
            tint: e.tint,
            id: EntityId { node: (self.names)(r.node), entity: r.entity, interior_entity },
        });
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;

    use rf_formats::formats::databag::Value;
    use rf_formats::hash::joaat;

    use super::*;
    use crate::map::tests::{entity_bag, map_bag};
    use crate::map::{EntityDef, InteriorDef};

    const PIP: u32 = 8; // LodInParentMap

    /// Node 0, the parent map: an SLOD over two LODs, at x = 100 and x = 1000.
    /// Node 1, the child map: the HD under each LOD.
    fn maps() -> HashMap<u32, Loaded> {
        let parent = map_bag(vec![
            entity_bag("CEntityDef", "slod", [500.0, 0.0, 0.0], -1, 0, (3000.0, 600.0, 2)),
            entity_bag("CEntityDef", "lod_a", [100.0, 0.0, 0.0], 0, 0, (500.0, 100.0, 1)),
            entity_bag("CEntityDef", "lod_b", [1000.0, 0.0, 0.0], 0, 0, (500.0, 100.0, 1)),
        ]);
        let child = map_bag(vec![
            entity_bag("CEntityDef", "hd_a", [100.0, 0.0, 0.0], 1, PIP, (150.0, 0.0, 0)),
            entity_bag("CEntityDef", "hd_b", [1000.0, 0.0, 0.0], 2, PIP, (150.0, 0.0, 0)),
        ]);
        let mut loaded = HashMap::new();
        let mut p = MapData::build(&parent, &HashMap::new(), &|_| None);
        p.entities[0].num_children = 2;
        p.entities[1].num_children = 1;
        p.entities[2].num_children = 1;
        loaded.insert(0, Loaded::new(p));
        loaded.insert(1, Loaded::new(MapData::build(&child, &HashMap::new(), &|_| None)));
        loaded
    }

    fn names(loaded: &HashMap<u32, Loaded>, streamed: &[u32], camera: Vec3, stamp: u32) -> HashSet<String> {
        let mut out = Vec::new();
        walk(loaded, streamed, &|n| n, camera, 12, stamp, &mut out);
        let all = ["slod", "lod_a", "lod_b", "hd_a", "hd_b"];
        let ids: HashSet<String> =
            out.iter().map(|e| all.iter().find(|n| joaat(n) == e.archetype).unwrap().to_string()).collect();
        assert_eq!(ids.len(), out.len(), "each entity once");
        ids
    }

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn lod_switching() {
        let mut loaded = maps();
        let streamed = [0, 1];
        link(&mut loaded, &streamed, &|n| (n == 1).then_some(0));
        assert_eq!(loaded[&1].linked, Some(0));
        assert_eq!(loaded[&0].ext_children[&1], [EntityRef { node: 1, entity: 0 }]);

        // Far away: nothing; further in: the SLOD.
        assert!(names(&loaded, &streamed, [5000.0, 0.0, 0.0], 1).is_empty());
        assert_eq!(names(&loaded, &streamed, [500.0, 0.0, 800.0], 2), set(&["slod"]));
        // At A: its HD, and B's LOD as the SLOD switched to its children.
        assert_eq!(names(&loaded, &streamed, [100.0, 0.0, 0.0], 3), set(&["hd_a", "lod_b"]));
        // Between A's switch distance and its HD's range: the LOD alone (the C#
        // shows both).
        assert_eq!(names(&loaded, &streamed, [100.0, 120.0, 0.0], 4), set(&["lod_a", "lod_b"]));
    }

    #[test]
    fn unlinked_children_are_roots() {
        // The parent map isn't streamed: the HDs stand alone.
        let mut loaded = maps();
        link(&mut loaded, &[1], &|n| (n == 1).then_some(0));
        assert_eq!(loaded[&1].linked, None);
        assert_eq!(names(&loaded, &[1], [100.0, 0.0, 0.0], 1), set(&["hd_a"]));
        // Without the child map, the LODs show for it.
        let mut loaded = maps();
        loaded.remove(&1);
        link(&mut loaded, &[0, 1], &|n| (n == 1).then_some(0));
        assert_eq!(names(&loaded, &[0, 1], [100.0, 0.0, 0.0], 1), set(&["lod_a", "lod_b"]));
    }

    #[test]
    fn interiors_under_a_lod() {
        // An LOD without a switch distance over an interior with one entity.
        let parent = map_bag(vec![entity_bag("CEntityDef", "shell_lod", [0.0; 3], -1, 0, (500.0, 0.0, 1))]);
        let mut mlo = entity_bag("CMloInstanceDef", "int", [0.0; 3], 0, PIP, (100.0, 0.0, 0));
        mlo.set_vec4("rotation", [0.0, 0.0, 0.0, 1.0]);
        mlo.set_object("defaultEntitySets", Some(Value::Hashes(vec![])));
        let chair = entity_bag("CEntityDef", "chair", [1.0, 0.0, 0.0], -1, 0, (30.0, 0.0, 5));
        let def = Arc::new(InteriorDef { entities: vec![Some(EntityDef::read(&chair))], sets: vec![] });
        let mut loaded = HashMap::new();
        let mut p = MapData::build(&parent, &HashMap::new(), &|_| None);
        p.entities[0].num_children = 1;
        loaded.insert(0, Loaded::new(p));
        loaded.insert(1, Loaded::new(MapData::build(&map_bag(vec![mlo]), &HashMap::new(), &|_| Some(def.clone()))));
        link(&mut loaded, &[0, 1], &|n| (n == 1).then_some(0));
        // The LOD switches at the interior's lodDist.
        assert_eq!(loaded[&0].child_lod_dist(0), 100.0);
        let mut out = Vec::new();
        assert_eq!(walk(&loaded, &[0, 1], &|n| n, [0.0, 10.0, 0.0], 12, 1, &mut out), 1);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].archetype, joaat("chair"));
        assert_eq!(out[0].id, EntityId { node: 1, entity: 0, interior_entity: 0 });
        assert_eq!(&out[0].transform[12..15], &[1.0, 0.0, 0.0]);
        // Out of the interior's range, the LOD.
        assert_eq!(walk(&loaded, &[0, 1], &|n| n, [0.0, 200.0, 0.0], 12, 2, &mut out), 0);
        assert_eq!(out.iter().map(|e| e.archetype).collect::<Vec<_>>(), [joaat("shell_lod")]);
    }
}
