//! Navmeshes (ynv) against a real RDR2 install. They only run when
//! `REDFORGE_GAME_DIR` points at the game folder; otherwise they pass without
//! checking anything.

use std::path::{Path, PathBuf};

use rf_formats::formats::{XmlContext, ynv};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};

fn open() -> Option<(Rpf8FileSystem, HashIndex)> {
    let game = PathBuf::from(std::env::var_os("REDFORGE_GAME_DIR")?);
    let strings = HashIndex::new();
    strings.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt")).expect("strings.txt");
    let keys = Rpf8Keys::from_game_folder(&game).expect("keys");
    let fs = Rpf8FileSystem::open(&game, keys, &strings).expect("archives");
    Some((fs, strings))
}

/// Game file → XML → game file → XML gives the same XML (as it does with the
/// C#), for navmeshes of all kinds: map cells big and small, flat ones, vehicle
/// ones, with and without special links.
#[test]
fn navmeshes_round_trip_through_xml() {
    let Some((fs, strings)) = open() else { return };
    let cx = XmlContext { hashes: &strings, folder: None };
    let all: Vec<(String, _)> =
        fs.files().filter(|(p, _)| p.ends_with(".ynv")).map(|(p, f)| (p.to_owned(), f)).collect();
    assert!(all.len() > 7000, "only {} ynv files", all.len());
    let special = ["/navmesh[351][369].ynv", "/navmesh[327][207].ynv", "/horseboat.ynv", "/privatepassenger01x.ynv"];
    let picked = all
        .iter()
        .enumerate()
        .filter(|(i, (p, _))| i % 400 == 0 || special.iter().any(|s| p.ends_with(s)))
        .map(|(_, f)| f);

    let (mut vehicles, mut with_links, mut flat) = (0, 0, 0);
    for (path, f) in picked {
        let file = fs.extract_loose(*f).unwrap_or_else(|e| panic!("{path}: {e}"));
        let nm = ynv::read(&file).unwrap_or_else(|e| panic!("{path}: {e}"));
        vehicles += (nm.content_flags & ynv::content_flags::VEHICLE != 0) as usize;
        with_links += nm.special_links.as_ref().is_some_and(|l| !l.is_empty()) as usize;
        flat += (nm.extents[2] == 0.0) as usize;

        let xml = ynv::to_xml(&file, &cx).unwrap_or_else(|e| panic!("{path}: {e}"));
        let written = ynv::from_xml(&xml, &cx).unwrap_or_else(|e| panic!("{path}: {e}"));
        let again = ynv::read(&written).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(ynv::to_xml(&written, &cx).unwrap(), xml, "{path}");

        // The rebuilt navmesh has the same data, and a quadtree over it.
        assert_eq!(again.polygons_count, nm.polygons_count, "{path}");
        assert_eq!(again.vertex_positions().map(|v| v.len()), nm.vertex_positions().map(|v| v.len()), "{path}");
        assert_eq!(again.special_links, nm.special_links, "{path}");
        let tree = again.quad_tree.as_ref().expect("a quadtree");
        assert_eq!(tree.aabb_min[..3], nm.aabb_min, "{path}");
    }
    assert!(vehicles > 0 && with_links > 0 && flat > 0, "{vehicles} vehicles, {with_links} with links, {flat} flat");
}

/// A navmesh changed in place (polygon flags, an edge) and written reads back
/// with those changes and nothing else.
#[test]
fn navmeshes_change_in_place() {
    let Some((fs, _)) = open() else { return };
    let f = fs.find("levels_1.rpf/levels/rdr3/navmeshes3.rpf/navmesh[312][333].ynv").expect("Valentine's navmesh");
    let nm = ynv::read(&fs.extract_loose(f).unwrap()).unwrap();
    let mut edited = nm.clone();
    let polys = edited.polygons.as_mut().unwrap();
    let p = polys.get_mut(1234).unwrap();
    p.set_flags(p.flags() | ynv::poly_flags::SWITCHED_OFF_FOR_PEDS);
    let mut e = *edited.edges.as_ref().unwrap().all_items().unwrap().get(77).unwrap();
    e.set_poly_id1(0x7FFF);
    e.set_poly_id2(0x7FFF);
    *edited.edges.as_mut().unwrap().get_mut(77).unwrap() = e;

    let again = ynv::read(&ynv::write(&edited)).unwrap();
    let (before, after) = (nm.polygons().unwrap(), again.polygons().unwrap());
    assert_eq!(before.len(), after.len());
    for (i, (b, a)) in before.iter().zip(&after).enumerate() {
        if i == 1234 {
            assert_eq!(a.flags(), b.flags() | ynv::poly_flags::SWITCHED_OFF_FOR_PEDS);
            assert_eq!(a.data1 & !0x1F_FFFF, b.data1 & !0x1F_FFFF);
        } else {
            assert_eq!(a, b, "polygon {i}");
        }
    }
    let (before, after) = (nm.edges().unwrap(), again.edges().unwrap());
    for (i, (b, a)) in before.iter().zip(&after).enumerate() {
        if i == 77 {
            assert_eq!((a.poly_id1(), a.poly_id2(), a.area_id1()), (0x7FFF, 0x7FFF, b.area_id1()));
        } else {
            assert_eq!(a, b, "edge {i}");
        }
    }
    assert_eq!(again.vertex_positions(), nm.vertex_positions());
    assert_eq!(again.quad_tree, nm.quad_tree);
}
