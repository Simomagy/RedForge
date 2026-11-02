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
