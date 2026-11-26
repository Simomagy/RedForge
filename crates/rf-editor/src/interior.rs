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

/// Adds a new plain entity at `position` (in the interior's space), attached
/// to the room it's in: the first room but limbo (room 0) whose box holds the
/// position, else the first room after limbo, else limbo. Returns its slot.
pub fn add(mlo: &mut DataBag, entity: DataBag, position: [f32; 3]) -> Slot {
    let list = lod::entities_mut(mlo);
    let index = list.len();
    list.push(Some(entity));
    let inside = room_at(mlo, position);
    if let Some(rooms) = mlo.bags_mut("rooms") {
        let room = inside.unwrap_or(if rooms.len() > 1 { 1 } else { 0 });
        if let Some(Some(r)) = rooms.get_mut(room) {
            match u32s_mut(r, ATTACHED) {
                Some(v) => v.push(index as u32),
                None => r.set_object(ATTACHED, Some(Value::U32s(vec![index as u32]))),
            }
        }
    }
    Slot { set: None, index }
}

fn u32s<'a>(bag: &'a DataBag, name: &str) -> &'a [u32] {
    match bag.object(name) {
        Some(Value::U32s(v) | Value::Hashes(v)) => v,
        _ => &[],
    }
}

/// A set entity's location that is a portal's, not a room's.
const PORTAL_LOCATION: u32 = 1 << 31;

/// The interior's rooms by name, in their order: the first is limbo, the
/// outside.
pub fn room_names(mlo: &DataBag) -> Vec<String> {
    let rooms = mlo.bags("rooms").unwrap_or_default().iter().enumerate();
    rooms
        .map(|(i, r)| r.as_ref().and_then(|r| r.string("name")).map_or_else(|| format!("room {i}"), str::to_owned))
        .collect()
}

/// The room an interior entity is in: the first that lists it, or the one its
/// set gives it. `None` for one in no room (a door or window in its portal
/// only, or one nothing lists).
pub fn room_of(mlo: &DataBag, slot: Slot) -> Option<usize> {
    let rooms = mlo.bags("rooms").unwrap_or_default();
    match slot.set {
        None => rooms.iter().position(|r| r.as_ref().is_some_and(|r| u32s(r, ATTACHED).contains(&(slot.index as u32)))),
        Some(set) => {
            let s = mlo.bags(ENTITY_SETS)?.get(set)?.as_ref()?;
            let room = *u32s(s, LOCATIONS).get(slot.index)?;
            (room & PORTAL_LOCATION == 0 && (room as usize) < rooms.len()).then_some(room as usize)
        }
    }
}

/// Whether an interior entity is attached to a portal: a door or a window,
/// drawn with what's on either side of it.
pub fn in_portal(mlo: &DataBag, slot: Slot) -> bool {
    match slot.set {
        None => mlo
            .bags("portals")
            .unwrap_or_default()
            .iter()
            .flatten()
            .any(|p| u32s(p, ATTACHED).contains(&(slot.index as u32))),
        Some(set) => mlo
            .bags(ENTITY_SETS)
            .and_then(|l| l.get(set))
            .and_then(Option::as_ref)
            .and_then(|s| u32s(s, LOCATIONS).get(slot.index))
            .is_some_and(|l| l & PORTAL_LOCATION != 0),
    }
}

fn room_holds(room: &DataBag, position: [f32; 3]) -> bool {
    let (lo, hi) = (room.vec3("bbMin"), room.vec3("bbMax"));
    (0..3).all(|i| lo[i] <= position[i] && position[i] <= hi[i])
}

/// The room a position (in the interior's space) is in: the first but limbo
/// whose box holds it.
pub fn room_at(mlo: &DataBag, position: [f32; 3]) -> Option<usize> {
    let mut rooms = mlo.bags("rooms").unwrap_or_default().iter().enumerate().skip(1);
    rooms.find(|(_, r)| r.as_ref().is_some_and(|r| room_holds(r, position))).map(|(i, _)| i)
}

/// Puts an interior entity in a room, and in that one only: it's taken out of
/// the other rooms and of the portals. Returns whether anything changed.
pub fn set_room(mlo: &mut DataBag, slot: Slot, room: usize) -> bool {
    if mlo.bags("rooms").is_none_or(|r| r.get(room).is_none_or(Option::is_none)) {
        return false;
    }
    match slot.set {
        None => {
            if slot.index >= lod::entities(mlo).len() || (room_of(mlo, slot) == Some(room) && !in_portal(mlo, slot)) {
                return false;
            }
            let index = slot.index as u32;
            each_attached(mlo, |list| list.retain(|&i| i != index));
            if let Some(Some(r)) = mlo.bags_mut("rooms").and_then(|l| l.get_mut(room)) {
                match u32s_mut(r, ATTACHED) {
                    Some(v) => v.push(index),
                    None => r.set_object(ATTACHED, Some(Value::U32s(vec![index]))),
                }
            }
            true
        }
        Some(set) => {
            let Some(Some(s)) = mlo.bags_mut(ENTITY_SETS).and_then(|l| l.get_mut(set)) else { return false };
            match u32s_mut(s, LOCATIONS).and_then(|l| l.get_mut(slot.index)) {
                Some(location) if *location != room as u32 => {
                    *location = room as u32;
                    true
                }
                _ => false,
            }
        }
    }
}

/// The room an interior entity goes to when it's moved to `position` (in the
/// interior's space): the one that holds it now, when that isn't the room it's
/// in. `None` when it stays where it is: still inside its room's box, or
/// outside every room; and for what isn't in a room by where it stands: the
/// shell and whatever else is in limbo, a door or window in its portal.
pub fn room_after_move(mlo: &DataBag, slot: Slot, position: [f32; 3]) -> Option<usize> {
    if in_portal(mlo, slot) {
        return None;
    }
    let current = room_of(mlo, slot);
    if current == Some(0) {
        return None;
    }
    let rooms = mlo.bags("rooms").unwrap_or_default();
    if current.and_then(|r| rooms[r].as_ref()).is_some_and(|r| room_holds(r, position)) {
        return None;
    }
    room_at(mlo, position).filter(|r| Some(*r) != current)
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

/// What an interior's lighting lacks next to the game's own interiors: its
/// rooms with no time cycle or no reflection probe, and how many of its
/// entities have none of the flags nearly all of the game's interior entities
/// have.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lighting {
    /// Every room of the game's interiors but the first (the whole of it,
    /// "limbo") names a time cycle: without one the room is lit as outdoors.
    pub no_time_cycle: Vec<String>,
    /// Nearly every interior of the game has a probe in its rooms: what its
    /// metals and glass reflect.
    pub no_probe: Vec<String>,
    /// Entities with neither of the two flags (`0x180000`) that 95 in 100 of
    /// the game's interior entities have. What the flags do isn't known.
    pub unflagged: usize,
    pub entities: usize,
}

/// The flags nearly all of the game's interior entities have.
pub const USUAL_ENTITY_FLAGS: u32 = 0x0018_0000;

impl Lighting {
    pub fn is_complete(&self) -> bool {
        self.no_time_cycle.is_empty() && self.no_probe.is_empty() && self.unflagged == 0
    }
}

/// See [`Lighting`].
pub fn lighting(mlo: &DataBag) -> Lighting {
    let mut l = Lighting::default();
    for (i, room) in mlo.bags("rooms").unwrap_or_default().iter().flatten().enumerate().skip(1) {
        let name = || room.string("name").map_or_else(|| format!("room {i}"), str::to_owned);
        if room.hash("timecycleName") == 0 {
            l.no_time_cycle.push(name());
        }
        if room.bags("reflectionProbes").is_none_or(|p| p.is_empty()) {
            l.no_probe.push(name());
        }
    }
    let sets = mlo.bags(ENTITY_SETS).unwrap_or_default().iter().flatten();
    for def in lod::entities(mlo).iter().chain(sets.flat_map(lod::entities)).flatten() {
        l.entities += 1;
        l.unflagged += usize::from(def.u32("flags") & USUAL_ENTITY_FLAGS == 0);
    }
    l
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
    fn a_new_entity_goes_in_the_room_it_is_in() {
        let room = |lo: [f32; 3], hi: [f32; 3]| {
            let mut r = Meta::create("CMloRoomDef").unwrap();
            r.set_vec3("bbMin", lo);
            r.set_vec3("bbMax", hi);
            Some(r)
        };
        let mut m = Meta::create("CMloArchetypeDef").unwrap();
        // Limbo holds everything; then a hall and a kitchen.
        let rooms = vec![room([-9.0; 3], [9.0; 3]), room([0.0; 3], [4.0; 3]), room([4.0, 0.0, 0.0], [8.0, 4.0, 4.0])];
        m.set_object("rooms", Some(Value::Bags(rooms)));
        let attached = |m: &mut DataBag, room: usize| {
            u32s_mut(m.bags_mut("rooms").unwrap()[room].as_mut().unwrap(), ATTACHED).cloned().unwrap_or_default()
        };
        let entity = || Meta::create("CEntityDef").unwrap();
        assert_eq!(add(&mut m, entity(), [6.0, 1.0, 1.0]), Slot { set: None, index: 0 });
        assert_eq!(add(&mut m, entity(), [1.0, 1.0, 1.0]).index, 1);
        // Outside every room: the first one after limbo.
        assert_eq!(add(&mut m, entity(), [-5.0, 0.0, 0.0]).index, 2);
        assert_eq!((attached(&mut m, 0), attached(&mut m, 1), attached(&mut m, 2)), (vec![], vec![1, 2], vec![0]));
        assert_eq!(lod::entities(&m).len(), 3);
    }

    /// Limbo, a hall and a kitchen next to it, with four entities: the shell
    /// in limbo, a chair in the hall, a door in a portal, a lamp nothing lists.
    fn house() -> DataBag {
        let room = |name: &str, lo: [f32; 3], hi: [f32; 3], attached: &[u32]| {
            let mut r = Meta::create("CMloRoomDef").unwrap();
            r.set_string("name", name);
            r.set_vec3("bbMin", lo);
            r.set_vec3("bbMax", hi);
            r.set_object(ATTACHED, Some(Value::U32s(attached.to_vec())));
            Some(r)
        };
        let mut m = Meta::create("CMloArchetypeDef").unwrap();
        *lod::entities_mut(&mut m) = (0..4).map(|_| Meta::create("CEntityDef")).collect();
        m.set_object(
            "rooms",
            Some(Value::Bags(vec![
                room("limbo", [-9.0; 3], [9.0; 3], &[0]),
                room("hall", [0.0; 3], [4.0; 3], &[1]),
                room("kitchen", [4.0, 0.0, 0.0], [8.0, 4.0, 4.0], &[]),
            ])),
        );
        let mut portal = Meta::create("CMloPortalDef").unwrap();
        portal.set_object(ATTACHED, Some(Value::U32s(vec![2])));
        m.set_object("portals", Some(Value::Bags(vec![Some(portal)])));
        m
    }

    fn plain(index: usize) -> Slot {
        Slot { set: None, index }
    }

    #[test]
    fn an_entity_is_put_in_one_room() {
        let mut m = house();
        assert_eq!(room_names(&m), ["limbo", "hall", "kitchen"]);
        assert_eq!((room_of(&m, plain(1)), room_of(&m, plain(2)), room_of(&m, plain(3))), (Some(1), None, None));
        assert!(in_portal(&m, plain(2)) && !in_portal(&m, plain(1)));
        // The chair to the kitchen: out of the hall.
        assert!(set_room(&mut m, plain(1), 2));
        assert_eq!(room_of(&m, plain(1)), Some(2));
        assert!(!set_room(&mut m, plain(1), 2), "it's there already");
        // The door to the hall: out of its portal.
        assert!(set_room(&mut m, plain(2), 1));
        assert_eq!((room_of(&m, plain(2)), in_portal(&m, plain(2))), (Some(1), false));
        // No such room, no such entity.
        assert!(!set_room(&mut m, plain(1), 7) && !set_room(&mut m, plain(9), 1));
        // A set's entity: its location.
        let mut m = mlo();
        m.set_object("rooms", house().object("rooms").cloned());
        let in_set = Slot { set: Some(0), index: 1 };
        assert_eq!(room_of(&m, in_set), None, "6 isn't a room of it");
        assert!(set_room(&mut m, in_set, 2));
        assert_eq!(room_of(&m, in_set), Some(2));
    }

    #[test]
    fn a_moved_entity_follows_its_room() {
        let m = house();
        // The chair: still in the hall, then in the kitchen, then outside every room.
        assert_eq!(room_after_move(&m, plain(1), [1.0, 1.0, 1.0]), None);
        assert_eq!(room_after_move(&m, plain(1), [6.0, 1.0, 1.0]), Some(2));
        assert_eq!(room_after_move(&m, plain(1), [-5.0, 1.0, 1.0]), None);
        // On the wall between the two (both boxes hold it): it stays in its room.
        assert_eq!(room_after_move(&m, plain(1), [4.0, 1.0, 1.0]), None);
        // The shell stays in limbo and the door in its portal, wherever they're moved.
        assert_eq!(room_after_move(&m, plain(0), [6.0, 1.0, 1.0]), None);
        assert_eq!(room_after_move(&m, plain(2), [6.0, 1.0, 1.0]), None);
        // The lamp nothing lists gets the room it's moved into.
        assert_eq!(room_after_move(&m, plain(3), [6.0, 1.0, 1.0]), Some(2));
        assert_eq!(room_at(&m, [-5.0, 1.0, 1.0]), None);
    }

    #[test]
    fn lighting_lists_what_rooms_and_entities_lack() {
        let room = |name: &str, time_cycle: u32, probe: bool| {
            let mut r = Meta::create("CMloRoomDef").unwrap();
            r.set_string("name", name);
            r.set_hash("timecycleName", time_cycle);
            if probe {
                r.set_object("reflectionProbes", Some(Value::Bags(vec![Meta::create("CReflectionProbeData")])));
            }
            Some(r)
        };
        let mut m = mlo();
        // Limbo isn't looked at; the hall has everything, the kitchen nothing.
        m.set_object(
            "rooms",
            Some(Value::Bags(vec![room("limbo", 0, false), room("hall", 7, true), room("kitchen", 0, false)])),
        );
        lod::entities_mut(&mut m)[1].as_mut().unwrap().set_u32("flags", USUAL_ENTITY_FLAGS | 32);
        let l = lighting(&m);
        assert_eq!(l.no_time_cycle, ["kitchen"]);
        assert_eq!(l.no_probe, ["kitchen"]);
        // Four plain entities and two in a set, one of them flagged.
        assert_eq!((l.unflagged, l.entities), (5, 6));
        assert!(!l.is_complete());
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
