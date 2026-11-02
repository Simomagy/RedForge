//! Resource metadata (ymap, ytyp, ymt) against a real RDR2 install. Skipped
//! (passing without checking anything) when `REDFORGE_GAME_DIR` isn't set.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rf_formats::formats::databag::Value;
use rf_formats::formats::meta::{self, Meta};
use rf_formats::formats::{Kind, XmlContext};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{FileRef, Rpf8FileSystem, Rpf8Keys};

fn game_dir() -> Option<PathBuf> {
    std::env::var_os("REDFORGE_GAME_DIR").map(PathBuf::from)
}

fn setup() -> Option<&'static (Rpf8FileSystem, HashIndex)> {
    static FS: OnceLock<Option<(Rpf8FileSystem, HashIndex)>> = OnceLock::new();
    FS.get_or_init(|| {
        let game = game_dir()?;
        let strings = HashIndex::new();
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt");
        strings.load_file(&path).expect("strings.txt");
        let keys = Rpf8Keys::from_game_folder(&game).expect("keys");
        let fs = Rpf8FileSystem::open(&game, keys, &strings).expect("archives");
        Some((fs, strings))
    })
    .as_ref()
}

/// `n` resource files with this extension (and path part), spread over all of them.
fn sample(fs: &Rpf8FileSystem, ext: &str, part: &str, n: usize) -> Vec<(String, FileRef)> {
    let mut all: Vec<(String, FileRef)> = fs
        .files()
        .filter(|(p, f)| p.ends_with(ext) && p.contains(part) && fs.entry(*f).is_resource())
        .map(|(p, f)| (p.to_owned(), f))
        .collect();
    all.sort_by(|a, b| a.0.cmp(&b.0));
    let step = (all.len() / n).max(1);
    all.into_iter().step_by(step).take(n).collect()
}

/// Game file → XML → game file → XML gives the same XML.
#[test]
fn meta_files_round_trip_through_xml() {
    let Some((fs, hashes)) = setup() else { return };
    let cx = XmlContext { hashes, folder: None };
    let mut checked = 0;
    for (ext, part, n) in [
        (".ymap", "", 40),
        (".ytyp", "", 30),
        (".ymt", "coverregion", 10),
        (".ymt", "/scenario/", 10),
        (".ymt", "propgroups", 10),
    ] {
        for (path, f) in sample(fs, ext, part, n) {
            let file = fs.extract_loose(f).unwrap_or_else(|e| panic!("{path}: {e}"));
            assert_eq!(Kind::of_file(&path, &file), Some(Kind::Meta), "{path}");
            let xml = meta::to_xml(&file, &cx).unwrap_or_else(|e| panic!("{path}: {e}"));
            let back = meta::from_xml(&xml, &cx).unwrap_or_else(|e| panic!("{path}: {e}"));
            let xml2 = meta::to_xml(&back, &cx).unwrap_or_else(|e| panic!("{path} (written back): {e}"));
            assert!(xml == xml2, "{path}: the XML changed after a round trip");
            checked += 1;
        }
    }
    assert!(checked >= 90, "only {checked} files");
}

/// A ymap's entities can be read and edited through the data bags.
#[test]
fn ymap_entities_edit() {
    let Some((fs, hashes)) = setup() else { return };
    let (path, f) = sample(fs, ".ymap", "_strm_", 50)
        .into_iter()
        .find(|(_, f)| {
            let file = fs.extract_loose(*f).unwrap();
            Meta::read(&file).ok().and_then(|m| m.root).and_then(|r| r.bags("entities").map(|e| e.len())).unwrap_or(0)
                > 2
        })
        .expect("a ymap with entities");
    let mut map = Meta::read(&fs.extract_loose(f).unwrap()).unwrap();
    let root = map.root.as_mut().unwrap();
    assert_eq!(hashes.xml_name(root.class_name()), "CMapData", "{path}");
    let entities = root.bags_mut("entities").unwrap();
    let count = entities.len();
    let first = entities[0].as_mut().unwrap();
    let archetype = first.hash("archetypeName");
    assert_ne!(archetype, 0, "{path}");
    let [x, y, z] = first.vec3("position");
    first.set_vec3("position", [x + 10.0, y, z]);
    first.set_f32("lodDist", 123.5);

    // A new entity, copied from the first, at the end.
    let mut added = first.clone();
    added.set_hash("archetypeName", hashes.ensure("p_bench09x"));
    entities.push(Some(added));

    let saved = Meta::read(&map.save().unwrap()).unwrap();
    let root = saved.root.as_ref().unwrap();
    let entities = root.bags("entities").unwrap();
    assert_eq!(entities.len(), count + 1);
    let first = entities[0].as_ref().unwrap();
    assert_eq!(first.hash("archetypeName"), archetype);
    assert_eq!(first.vec3("position"), [x + 10.0, y, z]);
    assert_eq!(first.f32("lodDist"), 123.5);
    let last = entities[count].as_ref().unwrap();
    assert_eq!(hashes.xml_name(last.hash("archetypeName")), "p_bench09x");
    assert!(matches!(root.object("entities"), Some(Value::Bags(_))));
}
