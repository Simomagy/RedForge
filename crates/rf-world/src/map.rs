//! A ymap's entities, as `RDR2MapData.InitEntities` makes them: their
//! transforms, LOD distances and LOD tree within the map, and the entities of
//! the interiors (MLOs) placed in it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rf_formats::formats::databag::{DataBag, Value};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{FileRef, Rpf8FileSystem};
use rf_game::{Archetype, GameData};

use crate::math::{self, Aabb, Quat, Vec3};

/// `"entities"`, of a `CMapData`, a `CMloArchetypeDef` and a `CMloEntitySet`.
const ENTITIES: u32 = 0xCCAB_9307;
/// `CMloInstanceDef`'s class name hash.
const MLO_INSTANCE_DEF: u32 = 0x09CC_28BE;
/// `RDR2EntityFlags.LodInParentMap`.
const LOD_IN_PARENT_MAP: u32 = 8;

/// The fields of a `CEntityDef` (or `CMloInstanceDef`) the streaming uses.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct EntityDef {
    pub archetype: u32,
    pub flags: u32,
    pub position: Vec3,
    /// As stored: the inverse of the entity's orientation, except for an MLO
    /// instance.
    pub rotation: Quat,
    pub scale_xy: f32,
    pub scale_z: f32,
    pub parent_index: i32,
    pub lod_dist: f32,
    pub child_lod_dist: f32,
    pub lod_level: u32,
    pub num_children: u32,
    pub tint: u32,
    /// For an MLO instance: `defaultEntitySets`.
    pub default_entity_sets: Option<Vec<u32>>,
    pub is_mlo: bool,
}

impl EntityDef {
    pub fn read(b: &DataBag) -> Self {
        let is_mlo = b.class_name() == MLO_INSTANCE_DEF;
        EntityDef {
            archetype: b.hash(0xA023_A02C),     // "archetypeName"
            flags: b.u32(0x67D2_6872),          // "flags"
            position: b.vec3(0x0116_6164),      // "position"
            rotation: b.vec4(0x77DD_4010),      // "rotation"
            scale_xy: b.f32(0x9CA3_2637),       // "scaleXY"
            scale_z: b.f32(0x10FB_7C42),        // "scaleZ"
            parent_index: b.i32(0xD892_31BD),   // "parentIndex"
            lod_dist: b.f32(0xBF74_DFA7),       // "lodDist"
            child_lod_dist: b.f32(0xCA97_4BCD), // "childLodDist"
            lod_level: b.u32(0x6C8F_1715),      // "lodLevel"
            num_children: b.u32(0xA687_AC89),   // "numChildren"
            tint: b.u32(0x3C85_2527),           // "tintValue"
            default_entity_sets: is_mlo.then(|| b.hashes(0x53DF_8649).map(<[u32]>::to_vec)).flatten(),
            is_mlo,
        }
    }

    fn scale(&self) -> Vec3 {
        [self.scale_xy, self.scale_xy, self.scale_z]
    }
}

/// `RDR2Entity.GetEngineLodLevel`: 0 orphan HD, 1 HD, 2 LOD, 3-6 SLOD1-4.
pub(crate) fn engine_lod_level(lod_type: u32) -> u8 {
    match lod_type {
        0 => 1, // LODTYPES_DEPTH_HD
        1 => 2, // LOD
        2 => 3, // SLOD1
        3 => 4, // SLOD2
        4 => 5, // SLOD3
        6 => 6, // SLOD4
        _ => 0, // ORPHANHD and anything else
    }
}

/// What `RecurseAddStreamEntity` needs of an entity, in world space.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entity {
    pub archetype: u32,
    /// Model to world, column-major.
    pub transform: [f32; 16],
    pub position: Vec3,
    /// The archetype's box through the transform (the C# uses the loaded
    /// model's, which is the same box); empty without an archetype.
    pub bounds: Aabb,
    /// `LodDistMax`: the entity's `lodDist`, or its archetype's when negative.
    pub lod_dist: f32,
    /// `LodDistMin`: `childLodDist`.
    pub child_lod_dist: f32,
    pub num_children: u32,
    pub parent_index: i32,
    pub parent_in_parent_map: bool,
    pub lod_level: u8,
    pub tint: u32,
    /// A time archetype's `timeFlags`.
    pub time_flags: Option<u32>,
    /// For an MLO instance: its index in [`MapData::interiors`].
    pub interior: Option<u32>,
    /// False for an entity that couldn't be read (kept so indices match the file).
    pub valid: bool,
}

impl Entity {
    /// A placeholder for an unreadable entity: never in range.
    fn missing() -> Self {
        Entity {
            archetype: 0,
            transform: math::transformation([1.0; 3], math::IDENTITY, [0.0; 3]),
            position: [0.0; 3],
            bounds: Aabb::EMPTY,
            lod_dist: f32::NEG_INFINITY,
            child_lod_dist: 0.0,
            num_children: 0,
            parent_index: -1,
            parent_in_parent_map: false,
            lod_level: 0,
            tint: 0,
            time_flags: None,
            interior: None,
            valid: false,
        }
    }

    /// `new RDR2Entity(def)` then `LoadArchetype`, with its world orientation
    /// and position: `Orientation` is the inverse of the stored rotation for a
    /// plain entity, the rotation itself for an MLO instance.
    fn new(def: &EntityDef, orientation: Quat, position: Vec3, archetypes: &HashMap<u32, Archetype>) -> Self {
        let scale = def.scale();
        let transform = math::transformation(scale, orientation, position);
        let arch = archetypes.get(&def.archetype);
        let mut lod_dist = def.lod_dist;
        if lod_dist < 0.0
            && let Some(a) = arch
        {
            lod_dist = a.lod_dist;
        }
        Entity {
            archetype: def.archetype,
            transform,
            position,
            bounds: arch
                .map_or(Aabb::EMPTY, |a| Aabb::new(a.bb_min, a.bb_max).transformed(scale, orientation, &transform)),
            lod_dist,
            child_lod_dist: def.child_lod_dist,
            num_children: def.num_children,
            parent_index: def.parent_index,
            parent_in_parent_map: def.flags & LOD_IN_PARENT_MAP != 0,
            lod_level: engine_lod_level(def.lod_level),
            tint: def.tint,
            time_flags: arch.filter(|a| a.class == Archetype::TIME_CLASS).map(|a| a.time_flags),
            interior: None,
            valid: true,
        }
    }

    /// A ymap entity.
    fn placed(def: &EntityDef, archetypes: &HashMap<u32, Archetype>) -> Self {
        let orientation = if def.is_mlo { def.rotation } else { math::quat_inverse(def.rotation) };
        Self::new(def, orientation, def.position, archetypes)
    }

    /// `new RDR2Entity(def, mloParent, ...)`: an entity of an interior, placed
    /// by the interior instance's position and stored rotation.
    fn in_interior(def: &EntityDef, mlo: &EntityDef, archetypes: &HashMap<u32, Archetype>) -> Self {
        let position = math::add(mlo.position, math::quat_rotate(mlo.rotation, def.position));
        let orientation = math::quat_normalize(math::quat_mul(mlo.rotation, math::quat_inverse(def.rotation)));
        Self::new(def, orientation, position, archetypes)
    }

    /// `IsActive(hour)`.
    pub fn is_active(&self, hour: i32) -> bool {
        match self.time_flags {
            Some(f) if (0..=23).contains(&hour) => (f >> hour) & 1 == 1,
            _ => true,
        }
    }

    /// `StreamingDistance`: from the camera to the entity's position, 0 inside
    /// its bounds.
    pub fn distance(&self, camera: Vec3) -> f32 {
        if self.bounds.contains(camera) { 0.0 } else { math::distance(self.position, camera) }
    }
}

/// A `CMloArchetypeDef`'s entities and entity sets, as stored (in the
/// interior's space). Shared by every instance of the interior.
#[derive(Debug, Default)]
pub(crate) struct InteriorDef {
    pub entities: Vec<Option<EntityDef>>,
    pub sets: Vec<(u32, Vec<Option<EntityDef>>)>,
}

impl InteriorDef {
    pub fn read(arch: &DataBag) -> Self {
        let defs = |b: &DataBag| -> Vec<Option<EntityDef>> {
            b.bags(ENTITIES).unwrap_or_default().iter().map(|e| e.as_ref().map(EntityDef::read)).collect()
        };
        let sets = arch
            .bags(0x45BC_B930) // "entitySets"
            .unwrap_or_default()
            .iter()
            .flatten()
            .map(|s| (s.hash(0xE6E1_7F14), defs(s))) // "name"
            .collect();
        InteriorDef { entities: defs(arch), sets }
    }
}

/// An interior placed in a map: its entities in world space, the plain ones
/// then each entity set's.
#[derive(Debug, Default)]
pub(crate) struct Interior {
    pub entities: Vec<Entity>,
    /// Per entity set: its name, its entities' range, and whether it's one of
    /// the instance's `defaultEntitySets` (the only ones the C# enables).
    pub sets: Vec<EntitySet>,
    /// How many plain entities there are (they come first).
    pub plain: u32,
    /// The instance it's placed by, to place it again when it changes.
    pub instance: EntityDef,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EntitySet {
    pub name: u32,
    pub start: u32,
    pub end: u32,
    pub enabled: bool,
}

impl Interior {
    fn new(def: &InteriorDef, mlo: &EntityDef, archetypes: &HashMap<u32, Archetype>) -> Self {
        let place = |d: &Option<EntityDef>| match d {
            Some(d) => Entity::in_interior(d, mlo, archetypes),
            None => Entity::missing(),
        };
        let mut entities: Vec<Entity> = def.entities.iter().map(place).collect();
        let plain = entities.len() as u32;
        let defaults = mlo.default_entity_sets.as_deref().unwrap_or_default();
        let mut sets = Vec::new();
        for (name, list) in &def.sets {
            let start = entities.len() as u32;
            entities.extend(list.iter().map(place));
            // Each default enables the first set of its name.
            let enabled = defaults.contains(name) && !sets.iter().any(|s: &EntitySet| s.name == *name && s.enabled);
            sets.push(EntitySet { name: *name, start, end: entities.len() as u32, enabled });
        }
        Interior { entities, sets, plain, instance: mlo.clone() }
    }

    /// Placed again from a changed definition (keeping which sets are on).
    pub fn rebuild(&mut self, def: &InteriorDef, archetypes: &HashMap<u32, Archetype>) {
        let enabled: Vec<bool> = self.sets.iter().map(|s| s.enabled).collect();
        *self = Interior::new(def, &self.instance, archetypes);
        for (s, on) in self.sets.iter_mut().zip(enabled) {
            s.enabled = on;
        }
    }

    /// The entities shown: the plain ones and those of the enabled sets.
    pub fn shown(&self) -> impl Iterator<Item = u32> + '_ {
        let sets = self.sets.iter().filter(|s| s.enabled).flat_map(|s| s.start..s.end);
        (0..self.plain).chain(sets)
    }
}

/// Interior definitions by archetype, shared by the loading threads.
pub(crate) type InteriorCache = Mutex<HashMap<u32, Option<Arc<InteriorDef>>>>;

/// What a loading thread needs.
pub(crate) struct Sources {
    pub fs: Arc<Rpf8FileSystem>,
    pub hashes: Arc<HashIndex>,
    pub data: Arc<GameData>,
    pub interiors: InteriorCache,
}

impl Sources {
    /// An MLO archetype's definition, read from its ytyp once.
    pub(crate) fn interior(&self, archetype: u32) -> Option<Arc<InteriorDef>> {
        if let Some(i) = self.interiors.lock().unwrap().get(&archetype) {
            return i.clone();
        }
        let def = self
            .data
            .archetypes
            .get(&archetype)
            .filter(|a| a.class == Archetype::MLO_CLASS)
            .and_then(|_| self.data.archetype_bag(&self.fs, &self.hashes, archetype))
            .map(|bag| Arc::new(InteriorDef::read(&bag)));
        self.interiors.lock().unwrap().insert(archetype, def.clone());
        def
    }
}

/// `RDR2MapData`: a loaded ymap's entities.
#[derive(Debug, Default)]
pub(crate) struct MapData {
    /// `CMapData.flags`: bit 0 is "scripted".
    pub flags: u32,
    /// Every item of the ymap's `entities`, in order.
    pub entities: Vec<Entity>,
    /// `RootEntities`: the entities with no parent in the map (none, or one in
    /// the parent map).
    pub roots: Vec<u32>,
    /// The LOD children within the map, per entity: `children[child_start[i]..child_start[i + 1]]`.
    child_start: Vec<u32>,
    children: Vec<u32>,
    pub interiors: Vec<Interior>,
}

impl MapData {
    /// Reads a ymap and builds its entities.
    pub fn load(src: &Sources, file: FileRef) -> Option<MapData> {
        let doc = rf_game::load_doc(&src.fs, &src.hashes, file)?;
        Some(Self::build(&doc.bag, &src.data.archetypes, &|a| src.interior(a)))
    }

    /// `InitEntities`, from a `CMapData`.
    pub fn build(
        bag: &DataBag,
        archetypes: &HashMap<u32, Archetype>,
        interior: &dyn Fn(u32) -> Option<Arc<InteriorDef>>,
    ) -> MapData {
        let mut map = MapData { flags: bag.u32(0x67D2_6872), ..Default::default() }; // "flags"
        let defs = match bag.object(ENTITIES) {
            Some(Value::Bags(b)) => b.as_slice(),
            _ => &[],
        };
        for def in defs {
            let Some(def) = def.as_ref().map(EntityDef::read) else {
                map.entities.push(Entity::missing());
                continue;
            };
            let mut e = Entity::placed(&def, archetypes);
            if def.is_mlo
                && let Some(i) = interior(def.archetype)
            {
                e.interior = Some(map.interiors.len() as u32);
                map.interiors.push(Interior::new(&i, &def, archetypes));
            }
            map.entities.push(e);
        }
        let n = map.entities.len();
        // The parents within the map; `LevelLod.Add` makes the others roots.
        let parent = |e: &Entity| {
            (e.valid && !e.parent_in_parent_map && e.parent_index >= 0 && (e.parent_index as usize) < n)
                .then_some(e.parent_index as usize)
        };
        let mut counts = vec![0u32; n + 1];
        for (i, e) in map.entities.iter().enumerate() {
            match parent(e) {
                Some(p) => counts[p + 1] += 1,
                None if e.valid && (e.parent_in_parent_map || e.parent_index < 0) => map.roots.push(i as u32),
                None => {}
            }
        }
        for i in 0..n {
            counts[i + 1] += counts[i];
        }
        let mut next = counts.clone();
        map.children = vec![0; counts[n] as usize];
        for (i, e) in map.entities.iter().enumerate() {
            if let Some(p) = parent(e) {
                map.children[next[p] as usize] = i as u32;
                next[p] += 1;
            }
        }
        map.child_start = counts;
        map
    }

    /// The entity's LOD children in this map.
    pub fn children(&self, entity: u32) -> &[u32] {
        let i = entity as usize;
        match (self.child_start.get(i), self.child_start.get(i + 1)) {
            (Some(&a), Some(&b)) => &self.children[a as usize..b as usize],
            _ => &[],
        }
    }

    pub fn is_scripted(&self) -> bool {
        self.flags & 1 != 0
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use rf_formats::formats::meta::Meta;

    use super::*;

    /// A `CEntityDef` (or `CMloInstanceDef`) bag.
    pub fn entity_bag(
        class: &str,
        archetype: &str,
        pos: Vec3,
        parent: i32,
        flags: u32,
        lod: (f32, f32, u32),
    ) -> DataBag {
        let mut b = Meta::create(class).unwrap();
        b.set_hash("archetypeName", rf_formats::hash::joaat(archetype));
        b.set_vec3("position", pos);
        b.set_vec4("rotation", [0.0, 0.0, 0.0, 1.0]);
        b.set_f32("scaleXY", 1.0);
        b.set_f32("scaleZ", 1.0);
        b.set_i32("parentIndex", parent);
        b.set_u32("flags", flags);
        b.set_f32("lodDist", lod.0);
        b.set_f32("childLodDist", lod.1);
        b.set_u32("lodLevel", lod.2);
        b
    }

    pub fn map_bag(entities: Vec<DataBag>) -> DataBag {
        let mut m = Meta::create("CMapData").unwrap();
        m.set_object("entities", Some(Value::Bags(entities.into_iter().map(Some).collect())));
        m
    }

    #[test]
    fn lod_tree_within_a_map() {
        // An SLOD (root), two LODs under it, an HD under the second LOD, an
        // orphan HD, and an HD whose parent is in the parent map.
        let m = map_bag(vec![
            entity_bag("CEntityDef", "slod", [0.0; 3], -1, 0, (1000.0, 300.0, 2)),
            entity_bag("CEntityDef", "lod_a", [10.0, 0.0, 0.0], 0, 0, (300.0, 100.0, 1)),
            entity_bag("CEntityDef", "lod_b", [20.0, 0.0, 0.0], 0, 0, (300.0, 100.0, 1)),
            entity_bag("CEntityDef", "hd", [21.0, 0.0, 0.0], 2, 0, (100.0, 0.0, 0)),
            entity_bag("CEntityDef", "orphan", [0.0; 3], -1, 0, (-1.0, 0.0, 5)),
            entity_bag("CEntityDef", "hd_out", [0.0; 3], 7, LOD_IN_PARENT_MAP, (100.0, 0.0, 0)),
        ]);
        let mut archetypes = HashMap::new();
        let mut orphan = sample_archetype("orphan");
        orphan.lod_dist = 55.0;
        archetypes.insert(orphan.name, orphan);
        let map = MapData::build(&m, &archetypes, &|_| None);
        assert_eq!(map.roots, [0, 4, 5]);
        assert_eq!(map.children(0), [1, 2]);
        assert_eq!(map.children(2), [3]);
        assert!(map.children(3).is_empty() && map.children(99).is_empty());
        let lods: Vec<u8> = map.entities.iter().map(|e| e.lod_level).collect();
        assert_eq!(lods, [3, 2, 2, 1, 0, 1]);
        // A negative lodDist is the archetype's; the archetype gives the bounds.
        assert_eq!(map.entities[4].lod_dist, 55.0);
        assert_eq!(map.entities[4].bounds, Aabb::new([-1.0; 3], [1.0; 3]));
        assert_eq!(map.entities[4].distance([0.5, 0.0, 0.0]), 0.0);
        assert_eq!(map.entities[3].distance([21.0, 3.0, 4.0]), 5.0);
        assert!(map.entities[5].parent_in_parent_map);
    }

    pub fn sample_archetype(name: &str) -> Archetype {
        Archetype {
            name: rf_formats::hash::joaat(name),
            class: rf_formats::hash::joaat("CBaseArchetypeDef"),
            lod_dist: 100.0,
            flags: 0,
            special_attribute: 0,
            bb_min: [-1.0; 3],
            bb_max: [1.0; 3],
            bs_centre: [0.0; 3],
            bs_radius: 1.0,
            hd_texture_dist: 0.0,
            texture_dictionary: 0,
            clip_dictionary: 0,
            drawable_dictionary: 0,
            physics_dictionary: 0,
            asset_type: rf_game::AssetType::DRAWABLE,
            asset_name: rf_formats::hash::joaat(name),
            time_flags: 0,
            ytyp: FileRef { archive: 0, entry: 0 },
            index: 0,
        }
    }

    #[test]
    fn interior_entities_follow_the_instance() {
        // An interior instance at (100, 0, 0) turned 90° about Z (stored as is
        // for an MLO), with an entity at (1, 0, 0) inside and an entity set.
        let h = (45f32.to_radians()).sin();
        let turn = [0.0, 0.0, h, h];
        let mut mlo = entity_bag("CMloInstanceDef", "int", [100.0, 0.0, 0.0], -1, 0, (200.0, 0.0, 5));
        mlo.set_vec4("rotation", turn);
        mlo.set_object("defaultEntitySets", Some(Value::Hashes(vec![rf_formats::hash::joaat("set_b")])));
        let inner = entity_bag("CEntityDef", "chair", [1.0, 0.0, 0.0], -1, 0, (50.0, 0.0, 5));
        let def = InteriorDef {
            entities: vec![Some(EntityDef::read(&inner)), None],
            sets: vec![
                (rf_formats::hash::joaat("set_a"), vec![Some(EntityDef::read(&inner))]),
                (rf_formats::hash::joaat("set_b"), vec![Some(EntityDef::read(&inner)); 2]),
            ],
        };
        let def = Arc::new(def);
        let map = MapData::build(&map_bag(vec![mlo]), &HashMap::new(), &|_| Some(def.clone()));
        assert_eq!(map.entities[0].interior, Some(0));
        let int = &map.interiors[0];
        assert_eq!(int.entities.len(), 5);
        assert_eq!(int.shown().collect::<Vec<_>>(), [0, 1, 3, 4]);
        let chair = &int.entities[0];
        assert!(math::distance(chair.position, [100.0, 1.0, 0.0]) < 1e-5);
        // Its X axis turns with the interior.
        let x_axis = &chair.transform[0..3];
        assert!(x_axis[0].abs() < 1e-5 && (x_axis[1] - 1.0).abs() < 1e-5);
        assert!(!int.entities[1].valid);
    }

    #[test]
    fn time_archetypes() {
        let mut a = sample_archetype("lamp");
        a.class = Archetype::TIME_CLASS;
        a.time_flags = 0b1111 << 20; // 20:00 to 23:59
        let mut archetypes = HashMap::new();
        archetypes.insert(a.name, a);
        let m = map_bag(vec![entity_bag("CEntityDef", "lamp", [0.0; 3], -1, 0, (10.0, 0.0, 5))]);
        let e = &MapData::build(&m, &archetypes, &|_| None).entities[0];
        assert!(e.is_active(21) && !e.is_active(12) && e.is_active(-1) && e.is_active(24));
    }
}
