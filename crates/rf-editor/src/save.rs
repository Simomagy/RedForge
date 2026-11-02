//! What saving a ymap works out again: its extents and flags. The extents as
//! the C# RedForge's `RDR2MapData.PreSaveUpdate` does (`UpdateExtents`), the
//! flags as the game's own ymaps have them (the C#'s `UpdateFlags` was a guess
//! from GTA V).
//!
//! - **Entities' extents**: each entity's archetype box (`bbMin`, `bbMax`)
//!   through its transform, each prop and grass batch's `BatchAABB`, and the
//!   boxes of the occluders, car generators and time cycle modifiers.
//! - **Streaming extents**: the same boxes, an entity's grown by its `lodDist`
//!   (its archetype's when negative), a batch's by its instances' distance (or
//!   1.1 × its bounding radius if more), a car generator's by twice its
//!   `perpendicularLength`.
//! - **Flags**: the scripted flag and the critical content flag are kept, the
//!   rest set again from what the map holds:
//!
//!   | Content flag | When | From |
//!   |---|---|---|
//!   | 1 HD | an HD or orphan HD entity (interior instances included) | C#, game for interiors |
//!   | 2 LOD | a LOD or SLOD1 entity (also sets `flags` 2) | C#, game for SLOD1 |
//!   | 4 SLOD2+ | an SLOD2, SLOD3 or SLOD4 entity (also sets `flags` 2) | C#, game without 16 |
//!   | 8 interior | an interior (MLO) instance | C# |
//!   | 16 block info | `block.name` is set | game |
//!   | 32 occluders | box occluders or occlude models | C# |
//!   | 64 physics | `physicsDictionaries`, or prop batches | C#, game for props |
//!   | 128 LOD lights | `LODLightsSOA` has lights | C# |
//!   | 256 distant lights | `DistantLODLightsSOA` has lights (also sets `flags` 2) | C# |
//!   | 1024 batches | prop or grass batches | C# |
//!   | `0x1000` | an entity with flag `0x200` | game |
//!
//!   The C# set 16 for SLOD1 and 4 and 16 for SLOD2+, set nothing more for an
//!   interior instance or prop batches, and cleared `0x1000`. Each "game" rule
//!   holds for every one of the game's ymaps it applies to; SLOD1 always comes
//!   with LODs there, so whether it sets 2 itself can't be told (it's a LOD
//!   level, so it does here).
//!
//! # Differences from the C#
//!
//! - The flags, above.
//! - Box occluders: the C# reads GTA V's fields (`iCenterX`, `iWidth`...),
//!   which RDR2 doesn't have, so any ymap with one reached the world's origin.
//!   RDR2's `BoxOccluder` is `CenterXYZ_CosZ` and `SizeXYZ_SinZ`: its centre,
//!   whole size, and the cosine and sine of its angle about Z.
//! - A negative `lodDist` without an archetype counts as 0 (the C# shrinks the
//!   streaming box by it).
//! - Arrays count when they have items: an empty `physicsDictionaries` (as XML
//!   may give) doesn't set the physics flag.
//!
//! # Compared with the game
//!
//! Worked out again for all 13,595 of the game's ymaps (`tests/save.rs` checks
//! a sample):
//!
//! - Entities' extents are exact for 89%. The others: turned models, whose
//!   box the game takes tighter (from the model, it seems); entities whose
//!   archetype isn't in the game data (`des_` objects, terrain SLODs); every
//!   interior, as RDR2's `CMloArchetypeDef`s have no box, so an interior is a
//!   point at its position (the game's extents hold its rooms); and the LOD
//!   light ymaps, whose lights aren't counted, as in the C#.
//! - Streaming extents are exact for 6% and hold the game's for 63% more. The
//!   game grows each entity's *position* by its `lodDist` (exact for 69% of
//!   the ymaps of plain entities), not its box; batch ymaps get a round margin
//!   (mostly 300 to 750 m) and interiors a cube of 25 to 300 m around them.
//! - Flags match for 98% (13,291; the C#'s rules: 74%). The others have
//!   `flags` 2 (LOD) with nothing LOD in them: 279 empty ymaps and 25 of box
//!   occluders only.

use std::collections::HashMap;

use glam::{Mat3, Quat, Vec3, Vec4, Vec4Swizzles};
use rf_formats::formats::databag::{DataBag, Value};
use rf_game::Archetype;

/// `CMloInstanceDef`'s class name hash.
const MLO_INSTANCE_DEF: u32 = 0x09CC_28BE;

/// `CMapData.flags`: scripted, and LOD (the map holds LODs, SLODs or distant lights).
const FLAG_SCRIPTED: u32 = 1;
const FLAG_LOD: u32 = 2;

/// `CMapData.contentFlags`.
const CF_HD: u32 = 1;
const CF_LOD: u32 = 2;
const CF_SLOD2: u32 = 4;
const CF_INTERIOR: u32 = 8;
const CF_BLOCK_INFO: u32 = 16;
const CF_OCCLUDER: u32 = 32;
const CF_PHYSICS: u32 = 64;
const CF_LOD_LIGHTS: u32 = 128;
const CF_DISTANT_LIGHTS: u32 = 256;
const CF_CRITICAL: u32 = 512;
const CF_BATCHES: u32 = 1024;
/// Set when an entity has flag [`ENTITY_FLAG_0X200`]; its meaning is unknown.
const CF_0X1000: u32 = 0x1000;
/// An entity flag (unnamed; "Unused10" in the C#'s GTA V list).
const ENTITY_FLAG_0X200: u32 = 0x200;

/// Works out a `CMapData`'s extents and flags again from its contents, as the
/// C# RedForge does before saving a ymap.
pub fn update_map(map: &mut DataBag, archetypes: &HashMap<u32, Archetype>) {
    let (entities, streaming) = extents(map, archetypes);
    map.set_vec3("entitiesExtentsMin", entities.min.into());
    map.set_vec3("entitiesExtentsMax", entities.max.into());
    map.set_vec3("streamingExtentsMin", streaming.min.into());
    map.set_vec3("streamingExtentsMax", streaming.max.into());
    let (flags, content_flags) = flags(map);
    map.set_u32("flags", flags);
    map.set_u32("contentFlags", content_flags);
}

/// A box grown to take in others; zero when nothing was added.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Bounds {
    min: Vec3,
    max: Vec3,
}

impl Bounds {
    const EMPTY: Bounds = Bounds { min: Vec3::MAX, max: Vec3::MIN };

    fn add(&mut self, min: Vec3, max: Vec3) {
        self.min = self.min.min(min);
        self.max = self.max.max(max);
    }

    /// Each side on its own: the C# zeroes a side left at its starting value.
    fn or_zero(self) -> Bounds {
        Bounds {
            min: if self.min == Vec3::MAX { Vec3::ZERO } else { self.min },
            max: if self.max == Vec3::MIN { Vec3::ZERO } else { self.max },
        }
    }
}

/// `UpdateExtents`: the entities' extents (what the map holds) and its
/// streaming extents (where it must be loaded: each entity's box grown by its
/// `lodDist`).
fn extents(map: &DataBag, archetypes: &HashMap<u32, Archetype>) -> (Bounds, Bounds) {
    let mut ents = Bounds::EMPTY;
    let mut strm = Bounds::EMPTY;
    let mut add = |min: Vec3, max: Vec3, reach: Vec3| {
        ents.add(min, max);
        strm.add(min - reach, max + reach);
    };

    for e in items(map, "entities") {
        let arch = archetypes.get(&e.hash("archetypeName"));
        let rotation = Quat::from_array(e.vec4("rotation"));
        // A ymap entity's rotation is the inverse of its orientation, an
        // interior instance's is the orientation itself.
        let orientation = if e.class_name() == MLO_INSTANCE_DEF { rotation } else { inverse(rotation) };
        let scale = Vec3::new(e.f32("scaleXY"), e.f32("scaleXY"), e.f32("scaleZ"));
        let (bmin, bmax) = arch.map_or((Vec3::ZERO, Vec3::ZERO), |a| (a.bb_min.into(), a.bb_max.into()));
        let (min, max) = transform_box(bmin, bmax, Vec3::from(e.vec3("position")), orientation, scale);
        let mut lod_dist = e.f32("lodDist");
        if lod_dist < 0.0
            && let Some(a) = arch
        {
            lod_dist = a.lod_dist;
        }
        // Still negative without an archetype: the C# would shrink the box.
        add(min, max, Vec3::splat(lod_dist.max(0.0)));
    }

    for b in batches(map) {
        let aabb = b.bag("BatchAABB");
        let min = aabb.map_or(Vec3::ZERO, |a| Vec4::from(a.vec4("min")).xyz());
        let max = aabb.map_or(Vec3::ZERO, |a| Vec4::from(a.vec4("max")).xyz());
        // `RDR2EntityBatch.SetArchetype` (only for a known archetype): the
        // instances' distance, or the batch's bounding sphere radius × 1.1 if
        // greater; else the batch's own `lodDist`.
        let lod_dist = if archetypes.contains_key(&b.hash("archetypeName")) {
            b.f32("InstanceLodDist").max((max - min).length() * 0.5 * 1.1)
        } else {
            b.f32("lodDist")
        };
        add(min, max, Vec3::splat(lod_dist.max(0.0)));
    }

    for o in items(map, "boxOccluders") {
        let (centre, size) = (Vec4::from(o.vec4("CenterXYZ_CosZ")), Vec4::from(o.vec4("SizeXYZ_SinZ")));
        let half = box_occluder_half_size(size.xyz(), centre.w, size.w);
        add(centre.xyz() - half, centre.xyz() + half, Vec3::ZERO);
    }

    for o in items(map, "occludeModels") {
        add(o.vec3("bmin").into(), o.vec3("bmax").into(), Vec3::ZERO);
    }

    // The game's ymaps have none (RDR2's schema has no `CCarGen`).
    for c in items(map, "carGenerators") {
        let pos = Vec3::from(c.vec3("position"));
        let len = Vec3::splat(c.f32("perpendicularLength"));
        ents.add(pos - len, pos + len);
        strm.add(pos - len * 2.0, pos + len * 2.0);
    }

    for t in items(map, "timeCycleModifiers") {
        let (min, max) = (Vec3::from(t.vec3("minExtents")), Vec3::from(t.vec3("maxExtents")));
        ents.add(min, max);
        strm.add(min, max);
    }

    (ents.or_zero(), strm.or_zero())
}

/// The world-space half size of a box occluder: its size (the whole box, as
/// the game stores it) turned by its angle about Z.
fn box_occluder_half_size(size: Vec3, cos_z: f32, sin_z: f32) -> Vec3 {
    let (c, s) = (cos_z.abs(), sin_z.abs());
    Vec3::new(c * size.x + s * size.y, s * size.x + c * size.y, size.z) * 0.5
}

/// `UpdateFlags`, with the game's rules: what the map holds, from its entities
/// and other contents. The scripted flag and the critical content flag are
/// kept.
fn flags(map: &DataBag) -> (u32, u32) {
    let mut flags = map.u32("flags") & FLAG_SCRIPTED;
    let mut cf = map.u32("contentFlags") & CF_CRITICAL;
    for e in items(map, "entities") {
        if e.class_name() == MLO_INSTANCE_DEF {
            cf |= CF_INTERIOR;
        }
        if e.u32("flags") & ENTITY_FLAG_0X200 != 0 {
            cf |= CF_0X1000;
        }
        match e.u32("lodLevel") {
            0 | 5 => cf |= CF_HD, // HD, orphan HD
            1 | 2 => {
                // LOD, SLOD1
                cf |= CF_LOD;
                flags |= FLAG_LOD;
            }
            3 | 4 | 6 => {
                // SLOD2, SLOD3, SLOD4
                cf |= CF_SLOD2;
                flags |= FLAG_LOD;
            }
            _ => {}
        }
    }
    if map.bag("block").and_then(|b| b.string("name")).is_some_and(|n| !n.is_empty()) {
        cf |= CF_BLOCK_INFO;
    }
    if batches(map).next().is_some() {
        cf |= CF_BATCHES;
    }
    let props = map.bag("instancedData").is_some_and(|d| items(d, "PropInstanceList").next().is_some());
    if props || has_items(map.object("physicsDictionaries")) {
        cf |= CF_PHYSICS;
    }
    if has_items(map.bag("LODLightsSOA").and_then(|l| l.object("direction"))) {
        cf |= CF_LOD_LIGHTS;
    }
    if has_items(map.bag("DistantLODLightsSOA").and_then(|l| l.object("position"))) {
        flags |= FLAG_LOD;
        cf |= CF_DISTANT_LIGHTS;
    }
    if has_items(map.object("boxOccluders")) || has_items(map.object("occludeModels")) {
        cf |= CF_OCCLUDER;
    }
    (flags, cf)
}

/// The readable items of an array of objects.
fn items<'a>(b: &'a DataBag, name: &str) -> impl Iterator<Item = &'a DataBag> + use<'a> {
    b.bags(name).into_iter().flatten().flatten()
}

/// The prop and grass instance batches (`instancedData`).
fn batches(map: &DataBag) -> impl Iterator<Item = &DataBag> {
    let data = map.bag("instancedData");
    let list = move |name| data.into_iter().flat_map(move |d| items(d, name));
    list("PropInstanceList").chain(list("GrassInstanceList"))
}

/// Whether a value is there and, for an array, has items. The C# only asks
/// for the value; its readers leave out empty arrays, but XML may not.
fn has_items(v: Option<&Value>) -> bool {
    match v {
        None => false,
        Some(Value::Bags(a)) => !a.is_empty(),
        Some(Value::Hashes(a)) => !a.is_empty(),
        Some(Value::Vec3s(a)) => !a.is_empty(),
        Some(Value::Vec4s(a)) => !a.is_empty(),
        Some(_) => true,
    }
}

/// `Quaternion.Inverse`: the conjugate over the squared length (glam's
/// assumes a unit quaternion).
fn inverse(q: Quat) -> Quat {
    let inv = 1.0 / q.length_squared();
    Quat::from_xyzw(-q.x * inv, -q.y * inv, -q.z * inv, q.w * inv)
}

/// `BoundingBox.Transform(position, orientation, scale)`: the box's centre
/// through the transform, its half size through the transform's absolute
/// values.
fn transform_box(min: Vec3, max: Vec3, position: Vec3, orientation: Quat, scale: Vec3) -> (Vec3, Vec3) {
    let m = Mat3::from_quat(orientation) * Mat3::from_diagonal(scale);
    let centre = m * ((max + min) * 0.5) + position;
    let half = (m.abs() * ((max - min) * 0.5)).abs();
    (centre - half, centre + half)
}

#[cfg(test)]
mod tests {
    use rf_formats::formats::meta::Meta;

    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-4)
    }

    fn archetype(name: &str, bb_min: [f32; 3], bb_max: [f32; 3], lod_dist: f32) -> Archetype {
        Archetype {
            name: rf_formats::hash::joaat(name),
            class: rf_formats::hash::joaat("CBaseArchetypeDef"),
            lod_dist,
            flags: 0,
            special_attribute: 0,
            bb_min,
            bb_max,
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
            ytyp: rf_formats::rpf8::FileRef { archive: 0, entry: 0 },
            index: 0,
        }
    }

    fn entity(class: &str, archetype: &str, pos: [f32; 3], rotation: [f32; 4], lod: (f32, u32)) -> DataBag {
        let mut b = Meta::create(class).unwrap();
        b.set_hash("archetypeName", rf_formats::hash::joaat(archetype));
        b.set_vec3("position", pos);
        b.set_vec4("rotation", rotation);
        b.set_f32("scaleXY", 1.0);
        b.set_f32("scaleZ", 1.0);
        b.set_i32("parentIndex", -1);
        b.set_f32("lodDist", lod.0);
        b.set_u32("lodLevel", lod.1);
        b
    }

    fn map(field: &str, items: Vec<DataBag>) -> DataBag {
        let mut m = Meta::create("CMapData").unwrap();
        m.set_object(field, Some(Value::Bags(items.into_iter().map(Some).collect())));
        m
    }

    fn read(m: &DataBag) -> [[f32; 3]; 4] {
        ["entitiesExtentsMin", "entitiesExtentsMax", "streamingExtentsMin", "streamingExtentsMax"].map(|f| m.vec3(f))
    }

    /// A rotation of `deg` degrees about Z.
    fn about_z(deg: f32) -> [f32; 4] {
        let h = deg.to_radians() * 0.5;
        [0.0, 0.0, h.sin(), h.cos()]
    }

    #[test]
    fn entities_and_their_lod_distance() {
        let mut archetypes = HashMap::new();
        for a in
            [archetype("long", [-2.0, -1.0, 0.0], [2.0, 1.0, 3.0], 50.0), archetype("cube", [-1.0; 3], [1.0; 3], 20.0)]
        {
            archetypes.insert(a.name, a);
        }
        // A long box turned 90° (stored as the inverse), using its archetype's
        // lodDist; a cube with its own; an interior instance turned 90° (stored
        // as is) whose archetype isn't known: a point.
        let mut m = map(
            "entities",
            vec![
                entity("CEntityDef", "long", [100.0, 0.0, 0.0], about_z(-90.0), (-1.0, 0)),
                entity("CEntityDef", "cube", [0.0, 0.0, 10.0], [0.0, 0.0, 0.0, 1.0], (5.0, 1)),
                entity("CMloInstanceDef", "int", [0.0, -30.0, 0.0], about_z(90.0), (-1.0, 0)),
            ],
        );
        update_map(&mut m, &archetypes);
        let [emin, emax, smin, smax] = read(&m);
        assert!(close(emin, [-1.0, -30.0, 0.0]) && close(emax, [101.0, 2.0, 11.0]), "{emin:?} {emax:?}");
        assert!(close(smin, [-6.0, -52.0, -50.0]) && close(smax, [151.0, 52.0, 53.0]), "{smin:?} {smax:?}");
        assert_eq!((m.u32("flags"), m.u32("contentFlags")), (FLAG_LOD, CF_HD | CF_LOD | CF_INTERIOR));

        // Scaled and turned about X: the box's height lies along Y.
        let mut e = entity("CEntityDef", "long", [0.0; 3], [-(0.5f32.sqrt()), 0.0, 0.0, 0.5f32.sqrt()], (0.0, 0));
        e.set_f32("scaleXY", 2.0);
        e.set_f32("scaleZ", 0.5);
        let mut m = map("entities", vec![e]);
        update_map(&mut m, &archetypes);
        let [emin, emax, ..] = read(&m);
        assert!(close(emin, [-4.0, -1.5, -2.0]) && close(emax, [4.0, 0.0, 2.0]), "{emin:?} {emax:?}");
    }

    #[test]
    fn box_occluders() {
        // 10 × 2 × 4 at (100, 50, 20), unturned, then turned 90° and 45°.
        let occluder = |cos: f32, sin: f32| {
            let mut b = Meta::create("BoxOccluder").unwrap();
            b.set_vec4("CenterXYZ_CosZ", [100.0, 50.0, 20.0, cos]);
            b.set_vec4("SizeXYZ_SinZ", [10.0, 2.0, 4.0, sin]);
            b
        };
        let extents = |cos, sin| {
            let mut m = map("boxOccluders", vec![occluder(cos, sin)]);
            update_map(&mut m, &HashMap::new());
            assert_eq!(m.u32("contentFlags"), CF_OCCLUDER);
            let r = read(&m);
            assert_eq!((r[0], r[1]), (r[2], r[3]), "streaming extents are the same");
            (r[0], r[1])
        };
        assert_eq!(extents(1.0, 0.0), ([95.0, 49.0, 18.0], [105.0, 51.0, 22.0]));
        let (min, max) = extents(0.0, 1.0);
        assert!(close(min, [99.0, 45.0, 18.0]) && close(max, [101.0, 55.0, 22.0]));
        let h = 0.5f32.sqrt();
        let (min, max) = extents(h, -h);
        let r = 6.0 * h;
        assert!(close(min, [100.0 - r, 50.0 - r, 18.0]) && close(max, [100.0 + r, 50.0 + r, 22.0]));
    }

    #[test]
    fn batches_occlude_models_and_time_cycle_modifiers() {
        let mut batch = Meta::create("rage__fwGrassInstanceListDef").unwrap();
        let aabb_class = batch.class.field("BatchAABB").unwrap().type_name;
        let mut aabb = rf_formats::formats::meta::rsc_schema().create(aabb_class).unwrap();
        aabb.set_vec4("min", [0.0, 0.0, 0.0, 0.0]);
        aabb.set_vec4("max", [30.0, 40.0, 0.0, 0.0]);
        batch.set_bag("BatchAABB", Some(aabb));
        batch.set_hash("archetypeName", rf_formats::hash::joaat("grass"));
        batch.set_f32("InstanceLodDist", 20.0);
        batch.set_f32("lodDist", 7.0);
        let mut data = Meta::create("rage__fwInstancedMapData").unwrap();
        data.set_object("GrassInstanceList", Some(Value::Bags(vec![Some(batch)])));
        let mut m = Meta::create("CMapData").unwrap();
        m.set_bag("instancedData", Some(data));
        m.set_u32("flags", 3);
        m.set_u32("contentFlags", 0xFFFF);

        // Without the archetype: the batch's lodDist.
        let mut a = m.clone();
        update_map(&mut a, &HashMap::new());
        let [emin, emax, smin, smax] = read(&a);
        assert_eq!((emin, emax), ([0.0; 3], [30.0, 40.0, 0.0]));
        assert_eq!((smin, smax), ([-7.0; 3], [37.0, 47.0, 7.0]));
        // Only the scripted and critical flags stay.
        assert_eq!((a.u32("flags"), a.u32("contentFlags")), (FLAG_SCRIPTED, CF_CRITICAL | CF_BATCHES));

        // With it: the instances' distance, or 1.1 × the radius (25) if more.
        let mut archetypes = HashMap::new();
        let grass = archetype("grass", [0.0; 3], [1.0; 3], 100.0);
        archetypes.insert(grass.name, grass);
        let mut a = m.clone();
        update_map(&mut a, &archetypes);
        let [_, _, smin, smax] = read(&a);
        assert!(close(smin, [-27.5; 3]) && close(smax, [57.5, 67.5, 27.5]), "{smin:?} {smax:?}");

        let mut model = Meta::create("OccludeModel").unwrap();
        model.set_vec3("bmin", [-100.0, 0.0, 0.0]);
        model.set_vec3("bmax", [-90.0, 1.0, 1.0]);
        let mut tc = Meta::create("CTimeCycleModifier").unwrap();
        tc.set_vec3("minExtents", [0.0, 0.0, -50.0]);
        tc.set_vec3("maxExtents", [1.0, 1.0, 1.0]);
        m.set_object("occludeModels", Some(Value::Bags(vec![Some(model)])));
        m.set_object("timeCycleModifiers", Some(Value::Bags(vec![Some(tc)])));
        update_map(&mut m, &HashMap::new());
        let [emin, emax, smin, smax] = read(&m);
        assert_eq!((emin, emax), ([-100.0, 0.0, -50.0], [30.0, 40.0, 1.0]));
        assert_eq!((smin, smax), ([-100.0, -7.0, -50.0], [37.0, 47.0, 7.0]));
        assert_eq!(m.u32("contentFlags"), CF_CRITICAL | CF_BATCHES | CF_OCCLUDER);
    }

    #[test]
    fn empty_map_and_flags() {
        let mut m = Meta::create("CMapData").unwrap();
        m.set_vec3("entitiesExtentsMin", [1.0; 3]);
        m.set_u32("flags", 2);
        m.set_u32("contentFlags", 2);
        m.set_object("physicsDictionaries", Some(Value::Hashes(vec![])));
        update_map(&mut m, &HashMap::new());
        assert_eq!(read(&m), [[0.0; 3]; 4]);
        assert_eq!((m.u32("flags"), m.u32("contentFlags")), (0, 0));

        let lods =
            [(0, CF_HD, 0), (5, CF_HD, 0), (1, CF_LOD, FLAG_LOD), (2, CF_LOD, FLAG_LOD), (6, CF_SLOD2, FLAG_LOD)];
        for (level, cf, flags) in lods {
            let mut m = map("entities", vec![entity("CEntityDef", "x", [0.0; 3], [0.0, 0.0, 0.0, 1.0], (1.0, level))]);
            m.set_object("physicsDictionaries", Some(Value::Hashes(vec![1])));
            update_map(&mut m, &HashMap::new());
            assert_eq!((m.u32("flags"), m.u32("contentFlags")), (flags, cf | CF_PHYSICS), "lodLevel {level}");
        }
    }

    #[test]
    fn flags_from_the_game() {
        let flags = |mut m: DataBag| {
            update_map(&mut m, &HashMap::new());
            (m.u32("flags"), m.u32("contentFlags"))
        };
        // An interior instance is also HD.
        let int = entity("CMloInstanceDef", "int", [0.0; 3], [0.0, 0.0, 0.0, 1.0], (25.0, 0));
        assert_eq!(flags(map("entities", vec![int])), (0, CF_INTERIOR | CF_HD));

        // An entity with flag 0x200.
        let mut e = entity("CEntityDef", "x", [0.0; 3], [0.0, 0.0, 0.0, 1.0], (1.0, 0));
        e.set_u32("flags", ENTITY_FLAG_0X200 | 1);
        assert_eq!(flags(map("entities", vec![e])), (0, CF_HD | CF_0X1000));

        // Block info, with a name.
        let mut m = Meta::create("CMapData").unwrap();
        let mut block = Meta::create("CBlockDesc").unwrap();
        block.set_string("name", "");
        m.set_bag("block", Some(block.clone()));
        assert_eq!(flags(m.clone()), (0, 0));
        block.set_string("name", "val_01");
        m.set_bag("block", Some(block));
        assert_eq!(flags(m), (0, CF_BLOCK_INFO));

        // Prop batches use physics, grass batches don't.
        for (list, cf) in [("PropInstanceList", CF_BATCHES | CF_PHYSICS), ("GrassInstanceList", CF_BATCHES)] {
            let class =
                if list.starts_with('P') { "rage__fwPropInstanceListDef" } else { "rage__fwGrassInstanceListDef" };
            let mut data = Meta::create("rage__fwInstancedMapData").unwrap();
            data.set_object(list, Some(Value::Bags(vec![Some(Meta::create(class).unwrap())])));
            let mut m = Meta::create("CMapData").unwrap();
            m.set_bag("instancedData", Some(data));
            assert_eq!(flags(m), (0, cf), "{list}");
        }
    }
}
