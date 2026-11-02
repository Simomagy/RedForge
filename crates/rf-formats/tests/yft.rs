//! Fragments (yft).
//!
//! `features.yft.xml` is hand-written to use what game files rarely have (a
//! drawable array, character cloth, a second verlet cloth, an environment
//! cloth drawable with a skeleton the C# drops, glass panes, groups whose
//! child groups are derived, both joint types, type connections, half floats
//! out of range, a second physics LOD with empty arrays...);
//! `features.cs.yft.xml` is what the C# RedForge reads back from the file it
//! writes from it (through `rf-oracle`). The game-file test needs
//! `REDFORGE_GAME_DIR`; the C# comparison of game files is `rf compare yft
//! --write`.

use std::path::{Path, PathBuf};

use rf_formats::formats::ybn::BoundsType;
use rf_formats::formats::yft::{self, JointType};
use rf_formats::formats::{self, XmlContext};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};
use rf_formats::rsc8::{Rsc8Reader, read_loose};

const FEATURES: &str = include_str!("data/yft/features.yft.xml");
const FEATURES_CS: &str = include_str!("data/yft/features.cs.yft.xml");

fn strings() -> HashIndex {
    let h = HashIndex::new();
    h.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt")).unwrap();
    h
}

/// The XML with what changes on every trip through XML (in the C# too) left
/// out: the bounds (whose material names are re-hashed and geometry
/// requantized and reordered on every trip, see `ybn`), and "-0" vertex
/// values (written as "0", see `ydr`).
fn normalized(xml: &str) -> String {
    let fix = |line: &str| {
        line.split('\t')
            .map(|e| e.split(' ').map(|v| if v == "-0" { "0" } else { v }).collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("\t")
    };
    let mut out = Vec::new();
    // The indent of the bounds being left out.
    let mut skipping: Option<usize> = None;
    for line in xml.lines() {
        let indent = line.len() - line.trim_start().len();
        let t = line.trim_start();
        match skipping {
            Some(i) if indent == i && t.starts_with("</Bounds>") => skipping = None,
            Some(_) => {}
            None if t.starts_with("<Bounds type=") && !t.ends_with("/>") => skipping = Some(indent),
            None if t.starts_with("<Bounds type=") => {}
            None => out.push(fix(line)),
        }
    }
    out.join("\n")
}

#[test]
fn fragment_round_trips_like_the_csharp() {
    let hashes = strings();
    let cx = XmlContext { hashes: &hashes, folder: None };
    let file = formats::from_xml("features.yft.xml", FEATURES, &cx).unwrap();
    let xml = formats::to_xml("features.yft", &file, &cx).unwrap();
    assert_eq!(xml, FEATURES_CS);
    // And again: what was read back writes and reads back the same.
    let file = formats::from_xml("features.yft.xml", &xml, &cx).unwrap();
    assert_eq!(normalized(&formats::to_xml("features.yft", &file, &cx).unwrap()), normalized(FEATURES_CS));

    let f = yft::read(&file, &cx).unwrap();
    assert_eq!(f.skeleton().unwrap().bones.as_ref().unwrap().len(), 3);
    assert_eq!(f.drawables().count(), 6);
    assert_eq!(f.drawable_array_names, Some(vec![Some("extra_part".to_owned())]));
    assert_eq!(f.bone_transforms.as_ref().unwrap().items.as_ref().unwrap().len(), 3);
    let g = f.physics_lod_group.as_ref().unwrap();
    let lod = g.physics_lod1.as_ref().unwrap();
    assert_eq!((lod.children_count, lod.groups_count, lod.root_groups_count), (2, 3, 1));
    let groups: Vec<_> = lod.groups.iter().flatten().flatten().collect();
    assert_eq!((groups[0].child_group_index, groups[0].child_group_count), (1, 2));
    assert_eq!(groups[1].child_group_index, 0xFFFF);
    assert_eq!(groups[2].value("Strength"), Some(f32::INFINITY));
    let joints = &lod.articulated_body_type.as_ref().unwrap().joints;
    assert_eq!(joints[1].as_ref().unwrap().joint_type, JointType::Dof3);
    assert_eq!(joints[1].as_ref().unwrap().unknown_b0h[1], [1e8; 4]);

    // The physics drawables share the main skeleton and the composite's
    // children: one skeleton block, one sphere block.
    let d1 = lod.drawables1.as_ref().unwrap()[1].as_ref().unwrap();
    assert!(d1.bounds_in_composite);
    assert_eq!(d1.bounds.as_ref().unwrap().bounds_type, BoundsType::Sphere);
    assert_eq!(d1.base.skeleton, f.skeleton().cloned());
    let d2 = lod.drawables2.as_ref().unwrap()[1].as_ref().unwrap();
    assert!(!d2.bounds_in_composite);
    let (h, data) = read_loose(&file).unwrap();
    let mut r = Rsc8Reader::new(&data, h.virtual_size());
    let ptr_at = |r: &mut Rsc8Reader, p: u64| r.at(p, |r| r.u64()).unwrap();
    let main = ptr_at(&mut r, 0x5000_0020);
    let group = ptr_at(&mut r, 0x5000_0090);
    let lod1 = ptr_at(&mut r, group + 0x10);
    let d1s = ptr_at(&mut r, lod1 + 0x118);
    let d1 = ptr_at(&mut r, d1s + 8);
    assert_eq!(ptr_at(&mut r, d1 + 0x18), ptr_at(&mut r, main + 0x18), "skeleton not shared");
    let comp = ptr_at(&mut r, lod1 + 0xE0);
    let kids = ptr_at(&mut r, comp + 0x70);
    assert_eq!(ptr_at(&mut r, d1 + 0xF0), ptr_at(&mut r, kids + 8), "bounds not shared");
    let archetype = ptr_at(&mut r, lod1 + 0xD0);
    assert_eq!(ptr_at(&mut r, archetype + 0x20), comp);
}

fn game() -> Option<(Rpf8FileSystem, HashIndex)> {
    let game = PathBuf::from(std::env::var_os("REDFORGE_GAME_DIR")?);
    let strings = strings();
    let keys = Rpf8Keys::from_game_folder(&game).expect("keys");
    let fs = Rpf8FileSystem::open(&game, keys, &strings).expect("archives");
    Some((fs, strings))
}

/// Game fragments (props, vehicles, peds, cloth) convert to XML and back
/// until the XML is stable (but for what [`normalized`] leaves out), which
/// takes a trip or two: as with the C#, the first trip drops environment cloth
/// drawables' skeletons, then the next their meshes' bone indices. That the
/// XML of the first trip matches the C#'s is `rf compare yft --write`.
#[test]
fn game_files_round_trip_through_xml() {
    let Some((fs, hashes)) = game() else { return };
    let tmp = std::env::temp_dir().join(format!("rf-yft-game-test-{}", std::process::id()));
    let mut files: Vec<_> = fs.files().filter(|(p, _)| p.ends_with(".yft")).collect();
    files.sort();
    let step = files.len() / 30;
    let picked = files.iter().step_by(step).chain(
        ["vehicles.rpf/", "metaped_fragments.rpf/", "p_curtain.rpf/", "p_win_"]
            .iter()
            .filter_map(|k| files.iter().find(|(p, _)| p.contains(k))),
    );
    let mut checked = 0;
    for (path, f) in picked {
        let name = path.rsplit('/').next().unwrap();
        let xml_name = format!("{name}.xml");
        let data = fs.extract_loose_deflate(*f).unwrap();
        let cx = XmlContext { hashes: &hashes, folder: Some(tmp.join(format!("{checked}"))) };
        let mut xml = formats::to_xml(name, &data, &cx).unwrap_or_else(|e| panic!("{path}: {e}"));
        let mut trips = 0;
        loop {
            let bin = formats::from_xml(&xml_name, &xml, &cx).unwrap_or_else(|e| panic!("{path}: {e}"));
            let next = formats::to_xml(name, &bin, &cx).unwrap();
            let (a, b) = (normalized(&xml), normalized(&next));
            if a == b {
                break;
            }
            trips += 1;
            if trips == 3 {
                let (la, lb) = a.lines().zip(b.lines()).find(|(x, y)| x != y).unwrap_or_default();
                panic!("{path}: not stable after 3 trips: {la} != {lb}");
            }
            xml = next;
        }
        checked += 1;
    }
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(checked >= 30, "only {checked} files checked");
}
