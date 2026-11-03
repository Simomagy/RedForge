//! "Scenario": the scenario points of the scenario point regions around the
//! camera (`RDR2ScenarioPointRegion`), each a box of a metre turned about Z
//! the way it faces.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use glam::{Mat4, Quat, Vec3, Vec4};
use rf_formats::formats::databag::{DataBag, Schema};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::FileRef;

use super::fields;
use super::shapes::Shapes;
use super::{Collect, HelperKey, HelperPart, Mode, PartInfo};
use crate::game::Game;

/// `ScenarioPointsStreamingRange`: how near the camera a region's box is
/// for its points to show.
const STREAMING_RANGE: f32 = 1000.0;
/// How far a region's points are kept, so they aren't read again when the
/// camera goes back and forth.
const KEPT_RANGE: f32 = STREAMING_RANGE * 1.5;

/// A region of the scenario point manifests (`RegionDefs`): its ymt and
/// box.
#[derive(Clone, Debug, PartialEq)]
struct RegionDef {
    file: FileRef,
    /// The ymt's name, and its hash for the parts' keys.
    name: String,
    hash: u32,
    min: Vec3,
    max: Vec3,
}

impl RegionDef {
    /// `BoundingBox.BoxDistance` as meant: how far `p` is outside the box
    /// along the axis it's farthest out on (negative inside). The C#'s gives
    /// the Y axis' only, showing the regions of a band across the map.
    fn distance(&self, p: Vec3) -> f32 {
        (self.min - p).max(p - self.max).max_element()
    }
}

/// A scenario point (`MyPoints`): where it is and which way it faces
/// (`vPositionAndDirection`), and what Properties shows of it.
struct Point {
    position_and_direction: Vec4,
    info: Arc<PartInfo>,
}

enum Loaded {
    Regions(Vec<RegionDef>),
    Points(usize, Vec<Point>),
}

/// The thread reading the manifests, then the regions asked for.
struct Worker {
    requests: Sender<usize>,
    loaded: Receiver<Loaded>,
}

impl Worker {
    fn new(game: Arc<Game>) -> Self {
        let (requests, asked) = channel::<usize>();
        let (tx, loaded) = channel();
        std::thread::spawn(move || {
            let regions = region_defs(&game);
            if tx.send(Loaded::Regions(regions.clone())).is_err() {
                return;
            }
            for i in asked {
                let r = &regions[i];
                let points = rf_game::load_doc(&game.fs, &game.hashes, r.file)
                    .map(|doc| points(&doc.bag, doc.schema.as_ref(), &game.hashes, &r.name))
                    .unwrap_or_default();
                if tx.send(Loaded::Points(i, points)).is_err() {
                    return;
                }
            }
        });
        Self { requests, loaded }
    }
}

/// The regions, and the parts of those near the camera.
#[derive(Default)]
pub struct Scenario {
    /// The game data's version the regions are from.
    version: Option<u64>,
    worker: Option<Worker>,
    regions: Vec<RegionDef>,
    /// Region → its parts (`None` while it's read).
    near: HashMap<usize, Option<Vec<HelperPart>>>,
}

impl Scenario {
    pub fn collect(&mut self, c: &Collect, shapes: &mut Shapes, out: &mut Vec<HelperPart>) {
        let version = c.game.data_version();
        if self.version != Some(version) {
            *self =
                Scenario { version: Some(version), worker: Some(Worker::new(c.game.clone())), ..Default::default() };
        }
        let Some(worker) = &self.worker else { return };
        while let Ok(loaded) = worker.loaded.try_recv() {
            match loaded {
                Loaded::Regions(r) => self.regions = r,
                Loaded::Points(i, points) => {
                    if let (Some(slot), Some(r)) = (self.near.get_mut(&i), self.regions.get(i)) {
                        *slot = Some(parts(c, shapes, r, points));
                    }
                }
            }
        }
        for (i, r) in self.regions.iter().enumerate() {
            let d = r.distance(c.eye);
            if d <= STREAMING_RANGE && !self.near.contains_key(&i) {
                self.near.insert(i, None);
                let _ = worker.requests.send(i);
            } else if d > KEPT_RANGE {
                self.near.remove(&i);
            }
        }
        for (&i, parts) in &self.near {
            if let Some(parts) = parts
                && self.regions[i].distance(c.eye) <= STREAMING_RANGE
            {
                out.extend(parts.iter().cloned());
            }
        }
    }
}

const REGION_DEFS: u32 = 0xDD51_866B; // "RegionDefs"
const NAME: u32 = 0xACEC_22BE; // "Name"
const AABB: u32 = 0x6359_09D7; // "AABB"
const MIN: u32 = 0xFE30_E6F3; // "min"
const MAX: u32 = 0x6060_68C4; // "max"
const X: u32 = 0x9303_A5E5; // "x"
const Y: u32 = 0x8095_0108; // "y"
const Z: u32 = 0xB660_6C9E; // "z"
const POINTS: u32 = 0x29E2_1837; // "Points"
const MY_POINTS: u32 = 0x45C8_B3D0; // "MyPoints"
const POSITION_AND_DIRECTION: u32 = 0x0047_7CED; // "vPositionAndDirection"

/// `RDR2Map`'s scenario point regions: the `RegionDefs` of the enabled
/// scenario point manifests (made and generated ones) whose ymt is found.
fn region_defs(game: &Game) -> Vec<RegionDef> {
    let data = game.data();
    let mut out = Vec::new();
    for kind in ["SCENARIO_POINTS_FILE_RESOURCED", "GENERATED_SCENARIO_POINTS_FILE_RESOURCED"] {
        for &f in data.enabled_data_files.get(kind).into_iter().flatten() {
            let Some(doc) = rf_game::load_doc(&game.fs, &game.hashes, f) else { continue };
            for def in doc.bag.bags(REGION_DEFS).into_iter().flatten().flatten() {
                let Some((name, min, max)) = region_box(def) else { continue };
                let Some(file) = data.map_lookup(&game.fs, name).and_then(|e| e.file()) else { continue };
                let name = file_name(game, file);
                let hash = rf_game::file_hash(&name);
                out.push(RegionDef { file, name, hash, min, max });
            }
        }
    }
    log::info!("{} scenario point regions", out.len());
    out
}

/// A file's name, without the folders it's in in its archive
/// (`Rpf8FileEntry.Name`).
pub(super) fn file_name(game: &Game, f: FileRef) -> String {
    let name = game.fs.name(f);
    name.rsplit('/').next().unwrap_or_default().to_owned()
}

/// A region definition's `Name` and `AABB` (its corners as `x`, `y`, `z`
/// structs, or as vectors).
fn region_box(def: &DataBag) -> Option<(&str, Vec3, Vec3)> {
    let name = def.string(NAME).filter(|n| !n.is_empty())?;
    let aabb = def.bag(AABB)?;
    let corner = |field| match aabb.bag(field) {
        Some(b) => Vec3::new(b.f32(X), b.f32(Y), b.f32(Z)),
        None => Vec4::from(aabb.vec4(field)).truncate(),
    };
    Some((name, corner(MIN), corner(MAX)))
}

/// A region's points: its `Points`' `MyPoints`, each with all its fields.
fn points(region: &DataBag, schema: Option<&Schema>, hashes: &HashIndex, owner: &str) -> Vec<Point> {
    let Some(points) = region.bag(POINTS) else { return Vec::new() };
    let my_points = points.bags(MY_POINTS).into_iter().flatten().flatten();
    my_points
        .enumerate()
        .map(|(i, p)| Point {
            position_and_direction: Vec4::from(p.vec4(POSITION_AND_DIRECTION)),
            info: Arc::new(PartInfo {
                type_name: "Scenario Point".into(),
                name: format!("Point {i}"),
                owner: owner.to_owned(),
                fields: fields::all(p, schema, hashes),
            }),
        })
        .collect()
}

/// A point's box: a metre, turned about Z by minus its direction.
fn point_transform(p: Vec4) -> Mat4 {
    Mat4::from_rotation_translation(Quat::from_rotation_z(-p.w), p.truncate())
}

fn parts(c: &Collect, shapes: &mut Shapes, r: &RegionDef, points: Vec<Point>) -> Vec<HelperPart> {
    let mesh = shapes.unit_box(c.gpu, false);
    let part = |(i, p): (usize, Point)| HelperPart {
        key: HelperKey { mode: Mode::Scenario, source: r.hash, group: 0, index: i as u32 },
        mesh: mesh.clone(),
        transform: point_transform(p.position_and_direction),
        colour: [255; 4],
        centre: p.position_and_direction.truncate(),
        radius: 0.87,
        info: p.info,
    };
    points.into_iter().enumerate().map(part).collect()
}

#[cfg(test)]
mod tests {
    use rf_formats::formats::databag::Value;
    use rf_formats::formats::meta::Meta;

    use super::*;

    #[test]
    fn distance_to_a_region() {
        let r = RegionDef {
            file: FileRef { archive: 0, entry: 0 },
            name: String::new(),
            hash: 0,
            min: Vec3::new(0.0, 0.0, 0.0),
            max: Vec3::new(10.0, 20.0, 5.0),
        };
        assert_eq!(r.distance(Vec3::new(5.0, 10.0, 2.0)), -2.0);
        assert_eq!(r.distance(Vec3::new(15.0, 10.0, 2.0)), 5.0);
        assert_eq!(r.distance(Vec3::new(-1.0, 1030.0, 2.0)), 1010.0);
    }

    #[test]
    fn points_face_their_direction() {
        let m = point_transform(Vec4::new(1.0, 2.0, 3.0, std::f32::consts::FRAC_PI_2));
        assert!(m.transform_point3(Vec3::ZERO).abs_diff_eq(Vec3::new(1.0, 2.0, 3.0), 1e-6));
        // Turned by minus the direction: X goes to -Y.
        assert!(m.transform_vector3(Vec3::X).abs_diff_eq(-Vec3::Y, 1e-6));
    }

    #[test]
    fn reads_a_region() {
        let mut point = Meta::create("CScenarioPoint").expect("CScenarioPoint");
        point.set_vec4("vPositionAndDirection", [1.0, 2.0, 3.0, 0.5]);
        point.set_u8("iProbability", 7);
        let mut container = Meta::create("CScenarioPointContainer").expect("CScenarioPointContainer");
        container.set_object("MyPoints", Some(Value::Bags(vec![Some(point.clone()), Some(point)])));
        let mut region = Meta::create("CScenarioPointRegion").expect("CScenarioPointRegion");
        region.set_bag("Points", Some(container));
        let hashes = HashIndex::new();
        hashes.load_str("iProbability\nvPositionAndDirection");
        let points = points(&region, None, &hashes, "region.ymt");
        assert_eq!(points.len(), 2);
        assert_eq!(points[1].position_and_direction, Vec4::new(1.0, 2.0, 3.0, 0.5));
        let info = &points[1].info;
        assert_eq!((info.name.as_str(), info.owner.as_str()), ("Point 1", "region.ymt"));
        assert!(info.fields.contains(&("iProbability".into(), "7".into())), "{:?}", info.fields);
    }
}
