//! Interiors (MLO archetypes, in ytyps): their entities, and what points at
//! them by index. Rooms and portals list their entities in `attachedObjects`
//! (indices into the archetype's `entities`); an entity set gives each of its
//! entities a room in `locations`.

use std::collections::HashMap;

use rf_formats::formats::databag::{DataBag, Value};

use crate::lod;

/// `"archetypes"`, of a `CMapTypes`.
const ARCHETYPES: u32 = 0x018A_3B1B;
const ENTITY_SETS: &str = "entitySets";
const ATTACHED: &str = "attachedObjects";
const LOCATIONS: &str = "locations";

pub fn archetypes(ytyp: &DataBag) -> &[Option<DataBag>] {
    ytyp.bags(ARCHETYPES).unwrap_or_default()
}

pub fn archetypes_mut(ytyp: &mut DataBag) -> Option<&mut Vec<Option<DataBag>>> {
    ytyp.bags_mut(ARCHETYPES)
}

/// Where an interior entity is: in the archetype's `entities`, or in an
/// entity set's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Slot {
    /// The entity set's index in `entitySets`; `None` for the plain entities.
    pub set: Option<usize>,
    pub index: usize,
}

/// An interior entity's slot from its index as the world counts them: the
/// plain entities, then each (readable) entity set's.
pub fn slot(mlo: &DataBag, mut k: usize) -> Option<Slot> {
    let plain = lod::entities(mlo).len();
    if k < plain {
        return Some(Slot { set: None, index: k });
    }
    k -= plain;
    for (i, s) in mlo.bags(ENTITY_SETS).unwrap_or_default().iter().enumerate() {
        let Some(s) = s else { continue };
        let n = lod::entities(s).len();
        if k < n {
            return Some(Slot { set: Some(i), index: k });
        }
        k -= n;
    }
    None
}

/// The world's index of an interior entity's slot (the inverse of [`slot`]).
pub fn world_index(mlo: &DataBag, slot: Slot) -> u32 {
    let mut k = lod::entities(mlo).len();
    let Some(set) = slot.set else { return slot.index as u32 };
    for (i, s) in mlo.bags(ENTITY_SETS).unwrap_or_default().iter().enumerate() {
        let Some(s) = s else { continue };
        if i == set {
            return (k + slot.index) as u32;
        }
        k += lod::entities(s).len();
    }
    k as u32
}

pub fn entity(mlo: &DataBag, slot: Slot) -> Option<&DataBag> {
    let list = match slot.set {
        None => lod::entities(mlo),
        Some(s) => lod::entities(mlo.bags(ENTITY_SETS)?.get(s)?.as_ref()?),
    };
    list.get(slot.index)?.as_ref()
}

pub fn entity_mut(mlo: &mut DataBag, slot: Slot) -> Option<&mut DataBag> {
    let holder = match slot.set {
        None => mlo,
        Some(s) => mlo.bags_mut(ENTITY_SETS)?.get_mut(s)?.as_mut()?,
    };
    lod::entities_mut(holder).get_mut(slot.index)?.as_mut()
}

fn u32s_mut<'a>(bag: &'a mut DataBag, name: &str) -> Option<&'a mut Vec<u32>> {
    match bag.object_mut(name)? {
        Value::U32s(v) | Value::Hashes(v) => Some(v),
        _ => None,
    }
}

/// Calls `f` with each room's and portal's `attachedObjects`.
fn each_attached(mlo: &mut DataBag, mut f: impl FnMut(&mut Vec<u32>)) {
    for name in ["rooms", "portals"] {
        if let Some(list) = mlo.bags_mut(name) {
            for b in list.iter_mut().flatten() {
                if let Some(v) = u32s_mut(b, ATTACHED) {
                    f(v);
                }
            }
        }
    }
}

/// Removes interior entities, fixing what points at them: the rooms' and
/// portals' `attachedObjects`, the sets' `locations`, and LOD parents among
/// the plain entities.
pub fn remove(mlo: &mut DataBag, slots: &[Slot]) {
    let mut plain: Vec<u32> = slots.iter().filter(|s| s.set.is_none()).map(|s| s.index as u32).collect();
    plain.sort_unstable();
    plain.dedup();
    if !plain.is_empty() {
        let r = lod::remove(mlo, &plain);
        each_attached(mlo, |list| {
            *list = list.iter().filter_map(|&i| r.remap.get(i as usize).copied().unwrap_or(Some(i))).collect();
        });
    }
    let mut by_set: HashMap<usize, Vec<usize>> = HashMap::new();
    for s in slots {
        if let Some(set) = s.set {
            by_set.entry(set).or_default().push(s.index);
        }
    }
    for (set, mut removed) in by_set {
        removed.sort_unstable();
        removed.dedup();
        let Some(Some(s)) = mlo.bags_mut(ENTITY_SETS).and_then(|l| l.get_mut(set)) else { continue };
        for &i in removed.iter().rev() {
            let list = lod::entities_mut(s);
            if i < list.len() {
                list.remove(i);
            }
            if let Some(loc) = u32s_mut(s, LOCATIONS)
                && i < loc.len()
            {
                loc.remove(i);
            }
        }
    }
}

/// Adds a copy of an interior entity next to it: a plain entity goes in the
/// same rooms and portals, a set entity in the same room. Returns its slot.
pub fn duplicate(mlo: &mut DataBag, from: Slot, copy: DataBag) -> Option<Slot> {
    match from.set {
        None => {
            let list = lod::entities_mut(mlo);
            let index = list.len();
            list.push(Some(copy));
            each_attached(mlo, |list| {
                if list.contains(&(from.index as u32)) {
                    list.push(index as u32);
                }
            });
            Some(Slot { set: None, index })
        }
        Some(set) => {
            let s = mlo.bags_mut(ENTITY_SETS)?.get_mut(set)?.as_mut()?;
            let list = lod::entities_mut(s);
            let index = list.len();
            list.push(Some(copy));
            if let Some(loc) = u32s_mut(s, LOCATIONS) {
                let room = loc.get(from.index).copied().unwrap_or(0);
                loc.push(room);
            }
            Some(Slot { set: Some(set), index })
        }
    }
}

#[cfg(test)]
mod tests {
    use rf_formats::formats::meta::Meta;

    use super::*;

    fn mlo() -> DataBag {
        let mut m = Meta::create("CMloArchetypeDef").unwrap();
        *lod::entities_mut(&mut m) = (0..4).map(|_| Meta::create("CEntityDef")).collect();
        let mut room = Meta::create("CMloRoomDef").unwrap();
        room.set_object(ATTACHED, Some(Value::U32s(vec![0, 2, 3])));
        m.set_object("rooms", Some(Value::Bags(vec![Some(room)])));
        let mut set = Meta::create("CMloEntitySet").unwrap();
        *lod::entities_mut(&mut set) = (0..2).map(|_| Meta::create("CEntityDef")).collect();
        set.set_object(LOCATIONS, Some(Value::U32s(vec![5, 6])));
        m.set_object(ENTITY_SETS, Some(Value::Bags(vec![Some(set)])));
        m
    }

    fn attached(m: &mut DataBag) -> Vec<u32> {
        u32s_mut(m.bags_mut("rooms").unwrap()[0].as_mut().unwrap(), ATTACHED).unwrap().clone()
    }

    #[test]
    fn slots_count_plain_then_sets() {
        let m = mlo();
        assert_eq!(slot(&m, 3), Some(Slot { set: None, index: 3 }));
        assert_eq!(slot(&m, 5), Some(Slot { set: Some(0), index: 1 }));
        assert_eq!(slot(&m, 6), None);
        assert_eq!(world_index(&m, Slot { set: Some(0), index: 1 }), 5);
    }

    #[test]
    fn removing_and_copying_keep_rooms_right() {
        let mut m = mlo();
        remove(&mut m, &[Slot { set: None, index: 2 }, Slot { set: Some(0), index: 0 }]);
        assert_eq!(lod::entities(&m).len(), 3);
        assert_eq!(attached(&mut m), [0, 2]);
        let set = m.bags_mut(ENTITY_SETS).unwrap()[0].as_mut().unwrap();
        assert_eq!(u32s_mut(set, LOCATIONS).unwrap(), &[6]);

        let copy = Meta::create("CEntityDef").unwrap();
        assert_eq!(duplicate(&mut m, Slot { set: None, index: 2 }, copy.clone()), Some(Slot { set: None, index: 3 }));
        assert_eq!(attached(&mut m), [0, 2, 3]);
        assert_eq!(duplicate(&mut m, Slot { set: Some(0), index: 0 }, copy), Some(Slot { set: Some(0), index: 1 }));
        let set = m.bags_mut(ENTITY_SETS).unwrap()[0].as_mut().unwrap();
        assert_eq!(u32s_mut(set, LOCATIONS).unwrap(), &[6, 6]);
    }
}
