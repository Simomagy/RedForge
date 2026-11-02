//! Cloth dictionaries (yld).
//!
//! `clothes.yld.xml` is hand-written to use what game files rarely have (an
//! unknown cloth name, a long controller name, every bridge array, morph
//! controller structs, a third verlet cloth LOD, default struct fields...);
//! `clothes.cs.yld.xml` is what the C# RedForge reads back from the file it
//! writes from it (through `rf-oracle`). The game-file test needs
//! `REDFORGE_GAME_DIR`; the C# comparison of game files is `rf compare yld
//! --write`.

use std::path::{Path, PathBuf};

use rf_formats::formats::ybn::BoundsShape;
use rf_formats::formats::ydr::{Bone, Skeleton};
use rf_formats::formats::{self, XmlContext, yld};
use rf_formats::hash::{HashIndex, joaat};
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};

const CLOTHES: &str = include_str!("data/yld/clothes.yld.xml");
const CLOTHES_CS: &str = include_str!("data/yld/clothes.cs.yld.xml");

fn strings() -> HashIndex {
    let h = HashIndex::new();
    h.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt")).unwrap();
    h
}

#[test]
fn clothes_round_trip_like_the_csharp() {
    let hashes = strings();
    let cx = XmlContext { hashes: &hashes, folder: None };
    let file = formats::from_xml("clothes.yld.xml", CLOTHES, &cx).unwrap();
    let xml = formats::to_xml("clothes.yld", &file, &cx).unwrap();
    assert_eq!(xml, CLOTHES_CS);
    // And again: what was read back writes and reads back the same.
    let file = formats::from_xml("clothes.yld.xml", &xml, &cx).unwrap();
    assert_eq!(formats::to_xml("clothes.yld", &file, &cx).unwrap(), CLOTHES_CS);

    let d = yld::read(&file).unwrap();
    assert!(d.hashes.items().is_sorted());
    let c = d.get(joaat("zz_not_a_known_cloth_name")).unwrap();
    let cc = c.controller.as_ref().unwrap();
    assert_eq!(cc.base.name, "CLOTHSIM_A_NAME_LONGER_THAN_THIR");
    let v1 = cc.base.verlet_cloth1.as_ref().unwrap();
    assert_eq!((v1.vertex_count, v1.constraint_count), (3, 2));
    assert!(cc.base.verlet_cloth2.is_none());
    assert_eq!(cc.base.verlet_cloth3.as_ref().unwrap().unknown_a8h, 60.0);
    // Lost through XML, as with the C#.
    assert!(cc.base.bridge_sim_gfx.as_ref().unwrap().unknown_100h.items.is_none());
    let m = cc.base.morph_controller.as_ref().unwrap();
    assert_eq!(m.unknown_18h.items()[1].unknown7[7], 14);

    // The capsules and the bones they follow.
    let bone = |tag| Bone { tag, ..Default::default() };
    let skel = Skeleton { bones: Some(vec![bone(1), bone(4089), bone(18545)]), ..Default::default() };
    let caps = c.capsule_bones(&skel);
    assert_eq!(caps.len(), 2);
    assert_eq!(caps[0].1.map(|(i, _)| i), Some(2));
    assert_eq!(caps[1].1.map(|(i, b)| (i, b.tag)), Some((1, 4089)));
    let bones = cc.bones(&skel);
    assert_eq!(bones.len(), 2);
    assert!(bones.iter().all(Option::is_none));
    let Some(BoundsShape::Composite(comp)) = c.bounds.as_ref().map(|b| &b.shape) else { panic!("not a composite") };
    assert_eq!(comp.children.as_ref().unwrap().len(), 2);
}

fn game() -> Option<(Rpf8FileSystem, HashIndex)> {
    let game = PathBuf::from(std::env::var_os("REDFORGE_GAME_DIR")?);
    let strings = strings();
    let keys = Rpf8Keys::from_game_folder(&game).expect("keys");
    let fs = Rpf8FileSystem::open(&game, keys, &strings).expect("archives");
    Some((fs, strings))
}

/// Game cloth dictionaries convert to XML, back, and to the same XML again.
#[test]
fn game_files_round_trip_through_xml() {
    let Some((fs, hashes)) = game() else { return };
    let cx = XmlContext { hashes: &hashes, folder: None };
    let mut files: Vec<_> = fs.files().filter(|(p, _)| p.ends_with(".yld")).collect();
    files.sort();
    let step = (files.len() / 40).max(1);
    let mut checked = 0;
    for (path, f) in files.iter().step_by(step) {
        let data = fs.extract_loose_deflate(*f).unwrap();
        let xml = formats::to_xml("x.yld", &data, &cx).unwrap_or_else(|e| panic!("{path}: {e}"));
        let bin = formats::from_xml("x.yld.xml", &xml, &cx).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(formats::to_xml("x.yld", &bin, &cx).unwrap(), xml, "{path}");
        let d = yld::read(&data).unwrap();
        assert!(d.clothes().all(|c| c.controller.is_some()), "{path}");
        checked += 1;
    }
    assert!(checked >= 30, "only {checked} files checked");
}
