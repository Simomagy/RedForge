//! PSO and RBF metadata against a real RDR2 install. Skipped (passing without
//! checking anything) when `REDFORGE_GAME_DIR` isn't set.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rf_formats::formats::{Kind, XmlContext, pso, rbf};
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

/// Up to `n` non-resource files with this extension and path part, spread over
/// all of them.
fn sample(fs: &Rpf8FileSystem, ext: &str, part: &str, n: usize) -> Vec<(String, FileRef)> {
    let mut all: Vec<(String, FileRef)> = fs
        .files()
        .filter(|(p, f)| p.ends_with(ext) && p.contains(part) && !fs.entry(*f).is_resource())
        .map(|(p, f)| (p.to_owned(), f))
        .collect();
    all.sort_by(|a, b| a.0.cmp(&b.0));
    let step = (all.len() / n).max(1);
    all.into_iter().step_by(step).take(n).collect()
}

/// PSO files: game file → XML → game file → XML → game file → XML. Writing uses
/// the schemas of `schemas.txt`, which can have more or fewer fields than a file's
/// own, so the XML is only stable from the second time.
#[test]
fn pso_files_round_trip_through_xml() {
    let Some((fs, hashes)) = setup() else { return };
    let cx = XmlContext { hashes, folder: None };
    let mut checked = 0;
    for (ext, part, n) in [
        (".ymt", "/conversations", 10),
        (".ymt", "metapeds", 10),
        (".ymt", "ambientscanner", 5),
        (".ymt", "cliffpoints", 5),
        (".ymt", "materialmap", 5),
        (".ymt", "/camera/", 5),
        (".ymt", "/tune/", 10),
        (".ymt", "road_nodes", 3),
        (".yas", "", 15),
        (".ymf", "", 1),
        (".ych", "", 1),
    ] {
        for (path, f) in sample(fs, ext, part, n) {
            let file = fs.extract(f).unwrap_or_else(|e| panic!("{path}: {e}"));
            if Kind::of_file(&path, &file) != Some(Kind::Pso) {
                continue;
            }
            let xml = pso::to_xml(&file, &cx).unwrap_or_else(|e| panic!("{path}: {e}"));
            let Ok(back) = pso::from_xml(&xml, &cx) else {
                // Root classes missing from schemas.txt can't be written (the C#
                // writes an empty file).
                continue;
            };
            let xml2 = pso::to_xml(&back, &cx).unwrap_or_else(|e| panic!("{path} (written back): {e}"));
            let back2 = pso::from_xml(&xml2, &cx).unwrap_or_else(|e| panic!("{path} (second write): {e}"));
            let xml3 = pso::to_xml(&back2, &cx).unwrap_or_else(|e| panic!("{path} (written back twice): {e}"));
            assert!(xml2 == xml3, "{path}: the XML changed after a round trip");
            assert_eq!(back, back2, "{path}: the file changed after a round trip");
            checked += 1;
        }
    }
    assert!(checked >= 50, "only {checked} files");
}

/// RBF files read, write back, and convert to XML and back.
#[test]
fn rbf_files_round_trip_through_xml() {
    let Some((fs, hashes)) = setup() else { return };
    let cx = XmlContext { hashes, folder: None };
    let mut checked = 0;
    for (path, f) in sample(fs, ".ymt", "/data/", 400) {
        let file = fs.extract(f).unwrap_or_else(|e| panic!("{path}: {e}"));
        if Kind::of_file(&path, &file) != Some(Kind::Rbf) {
            continue;
        }
        // The C#'s Save doesn't reproduce the game's files byte for byte (it keys
        // the descriptors by name only, and writes 0 for the header's unknowns),
        // but what it writes reads back the same.
        let tree = rbf::read(&file).unwrap_or_else(|e| panic!("{path}: {e}"));
        let again = rbf::read(&rbf::write(&tree).unwrap()).unwrap_or_else(|e| panic!("{path} (written back): {e}"));
        assert!(again == tree, "{path}: the tree changed when written back");
        let xml = rbf::to_xml(&file, &cx).unwrap_or_else(|e| panic!("{path}: {e}"));
        // Content without a name comes out as `<>`, which the C# can't read
        // back either.
        let Ok(back) = rbf::from_xml(&xml, &cx) else { continue };
        // Not stable in general, as in the C#: `<Item key="0" value="10" />` is
        // read back as a float, which an array then drops.
        rbf::to_xml(&back, &cx).unwrap_or_else(|e| panic!("{path} (written back): {e}"));
        checked += 1;
    }
    assert!(checked >= 30, "only {checked} files");
}

/// Converts many PSO and RBF files and counts what fails, and the kinds of map
/// items seen. Run by hand: `cargo test --release --test pso_rbf -- --ignored`.
#[test]
#[ignore]
fn sweep() {
    let Some((fs, hashes)) = setup() else { return };
    let cx = XmlContext { hashes, folder: None };
    let n: usize = std::env::var("SWEEP_N").ok().and_then(|s| s.parse().ok()).unwrap_or(3000);
    let mut tally: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut add = |k: String, p: &str| {
        let e = tally.entry(k).or_insert((0, p.to_owned()));
        e.0 += 1;
    };
    for ext in [".ymt", ".yas", ".ymf", ".ych"] {
        for (path, f) in sample(fs, ext, "", n) {
            let Ok(file) = fs.extract(f) else { continue };
            let kind = Kind::of_file(&path, &file);
            let r = match kind {
                Some(Kind::Pso) => pso::to_xml(&file, &cx).and_then(|x| pso::from_xml(&x, &cx).map(|b| (x, b))),
                Some(Kind::Rbf) => rbf::to_xml(&file, &cx).and_then(|x| rbf::from_xml(&x, &cx).map(|b| (x, b))),
                _ => continue,
            };
            match r {
                Ok((xml, _)) => {
                    add(format!("{kind:?} ok"), &path);
                    for l in xml.lines() {
                        if let Some(i) = l.find(" key=\"") {
                            let t = l[..i].rsplit_once("type=\"").map(|(_, t)| t.trim_end_matches('"'));
                            add(format!("map item type {}", t.unwrap_or("-")), &path);
                        }
                    }
                }
                Err(e) => {
                    let e = e.to_string();
                    add(format!("{kind:?} error: {}", &e[..e.len().min(60)]), &path);
                }
            }
        }
    }
    for (k, (n, p)) in tally {
        println!("{n:6} {k}  e.g. {p}");
    }
}
