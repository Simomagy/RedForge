//! Round trips of the small formats (yldb, yvr, ywr, yfd, ypmd and the two
//! `.dat` lists) through XML on real game files. They only run when
//! `REDFORGE_GAME_DIR` points at the game folder.
//!
//! These check the Rust against itself; `rf compare` checks it against the C#.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rf_formats::formats::{self, Kind, XmlContext};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};

struct Game {
    fs: Rpf8FileSystem,
    strings: HashIndex,
}

fn game() -> Option<&'static Game> {
    static GAME: OnceLock<Option<Game>> = OnceLock::new();
    GAME.get_or_init(|| {
        let dir = PathBuf::from(std::env::var_os("REDFORGE_GAME_DIR")?);
        let strings = HashIndex::new();
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt");
        strings.load_file(&path).expect("strings.txt");
        let keys = Rpf8Keys::from_game_folder(&dir).expect("keys");
        let fs = Rpf8FileSystem::open(&dir, keys, &strings).expect("archives");
        Some(Game { fs, strings })
    })
    .as_ref()
}

/// Up to `limit` files whose path ends with `suffix`, spread over all of them:
/// (name, loose file).
fn files(g: &Game, suffix: &str, limit: usize) -> Vec<(String, Vec<u8>)> {
    let all: Vec<_> = g.fs.files().filter(|(p, _)| p.ends_with(suffix)).map(|(p, f)| (p.to_owned(), f)).collect();
    assert!(!all.is_empty(), "no {suffix} files");
    let step = all.len().div_ceil(limit);
    all.iter()
        .step_by(step)
        .map(|(p, f)| {
            let name = p.rsplit('/').next().unwrap().to_owned();
            (name, g.fs.extract_loose_deflate(*f).unwrap_or_else(|e| panic!("{p}: {e}")))
        })
        .collect()
}

/// XML → file → XML, for each file; `expect` turns the first XML into what
/// the second should be (for the C# quirks that change data on the way).
fn round_trip(suffix: &str, limit: usize, expect: impl Fn(&str) -> Option<String>) -> usize {
    let Some(g) = game() else { return 0 };
    let cx = XmlContext { hashes: &g.strings, folder: None };
    let mut checked = 0;
    for (name, data) in files(g, suffix, limit) {
        let kind = Kind::of_file(&name, &data).unwrap_or_else(|| panic!("{name}: unknown kind"));
        let xml = formats::to_xml(&name, &data, &cx).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(xml.starts_with("<?xml"), "{name}");
        let Some(want) = expect(&xml) else { continue };
        let file = formats::from_xml(&kind.xml_file_name(&name), &xml, &cx).unwrap_or_else(|e| panic!("{name}: {e}"));
        let back = formats::to_xml(&name, &file, &cx).unwrap_or_else(|e| panic!("{name} written back: {e}"));
        assert!(back == want, "{name}: the XML changed through a round trip");
        checked += 1;
    }
    checked
}

fn same(xml: &str) -> Option<String> {
    Some(xml.to_owned())
}

#[test]
fn vehicle_and_waypoint_records() {
    round_trip(".yvr", 1000, same);
    round_trip(".ywr", 200, same);
}

#[test]
fn frame_filters() {
    round_trip(".yfd", 1000, same);
}

#[test]
fn language_databases() {
    // String lengths count UTF-16 units (a C# quirk), which cuts non-ASCII text
    // short once written; ASCII-only files come back unchanged.
    let n = round_trip(".yldb", 200, |xml| xml.is_ascii().then(|| xml.to_owned()));
    if game().is_some() {
        assert!(n > 0, "no ASCII-only yldb checked");
    }
}

#[test]
fn pose_matchers() {
    // The C# writes points with spaces and parses them with commas, so they
    // come back as zeros.
    round_trip(".ypmd", 1000, |xml| {
        let mut out = String::new();
        let mut in_points = false;
        for line in xml.lines() {
            if line.trim_start().starts_with("</Points>") {
                in_points = false;
            }
            out.push_str(if in_points { "0 0 0" } else { line });
            out.push('\n');
            if line.trim_start() == "<Points>" {
                in_points = true;
            }
        }
        Some(out)
    });
}

#[test]
fn dat_lists_rewrite_identically() {
    let Some(g) = game() else { return };
    let cx = XmlContext { hashes: &g.strings, folder: None };
    for suffix in ["slodindexlist.dat", "imposterextents.dat"] {
        for (name, data) in files(g, suffix, 10) {
            let xml = formats::to_xml(&name, &data, &cx).unwrap();
            let file = formats::from_xml(&format!("{name}.xml"), &xml, &cx).unwrap();
            assert!(file == data, "{name}: not rewritten identically");
        }
    }
}
