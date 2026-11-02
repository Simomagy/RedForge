//! Keeping a ymap's LOD tree right when entities are removed: `parentIndex`
//! is an index into the map's `entities` (or, with `LodInParentMap`, into the
//! parent map's), and `numChildren` counts them. The C# RedForge doesn't fix
//! them, so it hides entities instead of deleting them; this does.

use std::collections::HashMap;

use rf_formats::formats::databag::{DataBag, Value};

/// `RDR2EntityFlags.LodInParentMap`: `parentIndex` is in the parent map.
pub const LOD_IN_PARENT_MAP: u32 = 8;
/// `"entities"`.
const ENTITIES: u32 = 0xCCAB_9307;

/// A `CMapData`'s entities, made if missing.
pub fn entities_mut(map: &mut DataBag) -> &mut Vec<Option<DataBag>> {
    if map.bags_mut(ENTITIES).is_none() {
        map.set_object(ENTITIES, Some(Value::Bags(Vec::new())));
    }
    map.bags_mut(ENTITIES).expect("entities")
}

pub fn entities(map: &DataBag) -> &[Option<DataBag>] {
    map.bags(ENTITIES).unwrap_or_default()
}

/// The entity's LOD parent: its index, and whether it's in the parent map.
pub fn lod_parent(def: &DataBag) -> Option<(u32, bool)> {
    let p = def.i32("parentIndex");
    (p >= 0).then(|| (p as u32, def.u32("flags") & LOD_IN_PARENT_MAP != 0))
}

/// The first of `removed` that still has LOD children in its own map, or
/// counts some (`numChildren`) that may be in child maps.
pub fn with_children(map: &DataBag, removed: &[u32]) -> Option<u32> {
    let list = entities(map);
    removed.iter().copied().find(|&r| {
        let counted = list.get(r as usize).and_then(Option::as_ref).is_some_and(|d| d.u32("numChildren") > 0);
        counted
            || list.iter().enumerate().any(|(i, d)| {
                !removed.contains(&(i as u32))
                    && d.as_ref().and_then(lod_parent).is_some_and(|(p, other_map)| !other_map && p == r)
            })
    })
}

/// Each old index's new one after removing `removed`; `None` for the removed.
pub fn remap(count: usize, removed: &[u32]) -> Vec<Option<u32>> {
    let mut next = 0;
    (0..count as u32)
        .map(|i| {
            if removed.contains(&i) {
                None
            } else {
                next += 1;
                Some(next - 1)
            }
        })
        .collect()
}

/// What removing entities from a map does to the maps around it.
#[derive(Debug, Default)]
pub struct Removed {
    /// Each old index's new one.
    pub remap: Vec<Option<u32>>,
    /// Entities of the parent map that lost children, with how many.
    pub parent_lost: HashMap<u32, u32>,
}

/// Removes entities from a `CMapData` (none may have LOD children: check with
/// [`with_children`]). Their parents in the map count one child fewer each,
/// and the others' `parentIndex` in the map follows the new indices.
pub fn remove(map: &mut DataBag, removed: &[u32]) -> Removed {
    let list = entities_mut(map);
    let remap = remap(list.len(), removed);
    let mut lost: HashMap<u32, u32> = HashMap::new();
    let mut parent_lost: HashMap<u32, u32> = HashMap::new();
    for &r in removed {
        if let Some((p, other_map)) = list.get(r as usize).and_then(Option::as_ref).and_then(lod_parent) {
            *if other_map { &mut parent_lost } else { &mut lost }.entry(p).or_default() += 1;
        }
    }
    for (p, n) in lost {
        if let Some(Some(d)) = list.get_mut(p as usize) {
            d.set_u32("numChildren", d.u32("numChildren").saturating_sub(n));
        }
    }
    let mut kept = Vec::with_capacity(list.len());
    for (i, d) in std::mem::take(list).into_iter().enumerate() {
        if remap[i].is_none() {
            continue;
        }
        let d = d.map(|mut d| {
            if let Some((p, false)) = lod_parent(&d) {
                d.set_i32("parentIndex", remap.get(p as usize).copied().flatten().map_or(-1, |n| n as i32));
            }
            d
        });
        kept.push(d);
    }
    *list = kept;
    Removed { remap, parent_lost }
}

/// Points a child map's entities with `LodInParentMap` at their parents' new
/// indices. Returns whether anything changed.
pub fn follow_parent_map(child: &mut DataBag, remap: &[Option<u32>]) -> bool {
    let mut changed = false;
    for d in entities_mut(child).iter_mut().flatten() {
        if let Some((p, true)) = lod_parent(d) {
            let n = remap.get(p as usize).copied().flatten().map_or(-1, |n| n as i32);
            if n != p as i32 {
                d.set_i32("parentIndex", n);
                changed = true;
            }
        }
    }
    changed
}

/// Takes children off the parent map's entities that lost some.
pub fn drop_children(parent: &mut DataBag, lost: &HashMap<u32, u32>) {
    let list = entities_mut(parent);
    for (&p, &n) in lost {
        if let Some(Some(d)) = list.get_mut(p as usize) {
            d.set_u32("numChildren", d.u32("numChildren").saturating_sub(n));
        }
    }
}

#[cfg(test)]
mod tests {
    use rf_formats::formats::meta::Meta;

    use super::*;

    fn entity(parent: i32, in_parent_map: bool, children: u32) -> DataBag {
        let mut d = Meta::create("CEntityDef").unwrap();
        d.set_i32("parentIndex", parent);
        d.set_u32("flags", if in_parent_map { LOD_IN_PARENT_MAP } else { 0 });
        d.set_u32("numChildren", children);
        d
    }

    fn map(list: Vec<DataBag>) -> DataBag {
        let mut m = Meta::create("CMapData").unwrap();
        *entities_mut(&mut m) = list.into_iter().map(Some).collect();
        m
    }

    fn parents(m: &DataBag) -> Vec<i32> {
        entities(m).iter().flatten().map(|d| d.i32("parentIndex")).collect()
    }

    #[test]
    fn removing_shifts_the_indices_after() {
        // 0: LOD with children 1 and 3; 2: orphan; 4: child of parent map entity 7.
        let mut m = map(vec![
            entity(-1, false, 2),
            entity(0, false, 0),
            entity(-1, false, 0),
            entity(0, false, 0),
            entity(7, true, 0),
        ]);
        assert_eq!(with_children(&m, &[0]), Some(0));
        assert_eq!(with_children(&m, &[1, 2, 4]), None);
        let r = remove(&mut m, &[1, 2, 4]);
        assert_eq!(parents(&m), [-1, 0]);
        assert_eq!(entities(&m)[0].as_ref().unwrap().u32("numChildren"), 1);
        assert_eq!(r.remap, [Some(0), None, None, Some(1), None]);
        assert_eq!(r.parent_lost, HashMap::from([(7, 1)]));

        let mut child = map(vec![entity(3, true, 0), entity(0, true, 0), entity(0, false, 0)]);
        assert!(follow_parent_map(&mut child, &r.remap));
        assert_eq!(parents(&child), [1, 0, 0]);
        assert!(!follow_parent_map(&mut child, &remap(4, &[])));

        let mut parent = map((0..8).map(|_| entity(-1, false, 3)).collect());
        drop_children(&mut parent, &r.parent_lost);
        assert_eq!(entities(&parent)[7].as_ref().unwrap().u32("numChildren"), 2);
    }
}
