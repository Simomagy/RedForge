//! Static collision bounds (ybn).
//!
//! `all_types.ybn.xml` has every bound type; `all_types.cs.ybn.xml` is what the C#
//! RedForge reads back from the file it writes from it (through `rf-oracle`):
//! writing has to give a file that reads back the same, BVH polygon order,
//! quantized vertices, Plane-as-Box and re-hashed material names included.

use std::path::{Path, PathBuf};

use rf_formats::formats::ybn::{self, BoundsShape, BoundsType};
use rf_formats::formats::{self, XmlContext};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};

const INPUT: &str = include_str!("data/ybn/all_types.ybn.xml");
const EXPECTED: &str = include_str!("data/ybn/all_types.cs.ybn.xml");

#[test]
fn every_bound_type_round_trips_like_the_csharp() {
    let hashes = HashIndex::new();
    let cx = XmlContext { hashes: &hashes, folder: None };
    let file = formats::from_xml("all_types.ybn.xml", INPUT, &cx).unwrap();
    let xml = formats::to_xml("all_types.ybn", &file, &cx).unwrap();
    assert_eq!(xml, EXPECTED);

    let root = ybn::read(&file).unwrap();
    let BoundsShape::Composite(c) = &root.shape else { panic!("root isn't a composite") };
    let kids = c.children.as_ref().unwrap();
    assert_eq!(kids.len(), 10);
    assert!(kids[3].is_none());
    // A plane is written as a box, as the C#'s Rsc8BoundPlane constructor says.
    assert_eq!(kids[5].as_ref().unwrap().bounds_type, BoundsType::Box);
    // 10 children and NoBVH: no BVH.
    assert!(c.bvh.is_none());
    let BoundsShape::GeometryBvh(g, part) = &kids[0].as_ref().unwrap().shape else { panic!("not a BVH") };
    assert!(part.bvh.is_some());
    assert_eq!(g.polygons.len(), 30);
}

fn game_file_system() -> Option<Rpf8FileSystem> {
    let game = PathBuf::from(std::env::var_os("REDFORGE_GAME_DIR")?);
    let strings = HashIndex::new();
    strings.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt")).unwrap();
    let keys = Rpf8Keys::from_game_folder(&game).unwrap();
    Some(Rpf8FileSystem::open(&game, keys, &strings).unwrap())
}

/// Game ybns convert to XML and back, and the file written reads back with the
/// same bounds (the C# comparison is `rf compare ybn --write`).
#[test]
fn game_ybns_convert_both_ways() {
    let Some(fs) = game_file_system() else { return };
    let hashes = HashIndex::new();
    let cx = XmlContext { hashes: &hashes, folder: None };
    let mut files: Vec<_> = fs.files().filter(|(p, _)| p.ends_with(".ybn")).map(|(p, f)| (p.to_owned(), f)).collect();
    files.sort();
    let step = (files.len() / 40).max(1);
    for (path, f) in files.iter().step_by(step) {
        let data = fs.extract_loose(*f).unwrap();
        let original = ybn::read(&data).unwrap_or_else(|e| panic!("{path}: {e}"));
        let xml = formats::to_xml("x.ybn", &data, &cx).unwrap_or_else(|e| panic!("{path}: {e}"));
        let written = formats::from_xml("x.ybn.xml", &xml, &cx).unwrap_or_else(|e| panic!("{path}: {e}"));
        let back = ybn::read(&written).unwrap_or_else(|e| panic!("{path}: {e}"));
        let (BoundsShape::Composite(a), BoundsShape::Composite(b)) = (&original.shape, &back.shape) else {
            panic!("{path}: not a composite")
        };
        let (a, b) = (a.children.as_ref().unwrap(), b.children.as_ref().unwrap());
        assert_eq!(a.len(), b.len(), "{path}");
        for (x, y) in a.iter().zip(b) {
            let (x, y) = (x.as_ref().unwrap(), y.as_ref().unwrap());
            assert_eq!((x.bounds_type, x.box_min, x.box_max), (y.bounds_type, y.box_min, y.box_max), "{path}");
            if let (BoundsShape::GeometryBvh(gx, _), BoundsShape::GeometryBvh(gy, _)) = (&x.shape, &y.shape) {
                assert_eq!(gx.polygons.len(), gy.polygons.len(), "{path}");
                assert_eq!(gx.vertices.as_ref().map(Vec::len), gy.vertices.as_ref().map(Vec::len), "{path}");
            }
        }
    }
}
