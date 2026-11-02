//! Clip and animation dictionaries from a real RDR2 install. They only run when
//! `REDFORGE_GAME_DIR` points at the game folder; otherwise they pass without
//! checking anything.

use std::path::{Path, PathBuf};

use rf_formats::formats::XmlContext;
use rf_formats::formats::ycd::{self, AnimDictionary, ClipDictionary, ReadCx, Sequence};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};
use rf_formats::rsc8::{VIRTUAL_BASE, read_resource};

fn open() -> Option<(Rpf8FileSystem, HashIndex)> {
    let game = PathBuf::from(std::env::var_os("REDFORGE_GAME_DIR")?);
    let strings = HashIndex::new();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt");
    strings.load_file(&path).expect("strings.txt");
    let keys = Rpf8Keys::from_game_folder(&game).expect("keys");
    let fs = Rpf8FileSystem::open(&game, keys, &strings).expect("archives");
    Some((fs, strings))
}

/// `n` files with an extension, spread over all of them.
fn sample(fs: &Rpf8FileSystem, ext: &str, n: usize) -> Vec<(String, Vec<u8>)> {
    let all: Vec<_> = fs.files().filter(|(p, _)| p.ends_with(ext)).collect();
    let step = (all.len() / n).max(1);
    all.iter()
        .step_by(step)
        .take(n)
        .map(|(p, f)| (p.to_string(), fs.extract_loose_deflate(*f).unwrap_or_else(|e| panic!("{p}: {e}"))))
        .collect()
}

/// The animations' and sequences' signatures are the game's: Fletcher-32 of the
/// bone ids, and the hash of the packed data.
fn check_signatures(path: &str, dict: &AnimDictionary) -> usize {
    let mut n = 0;
    for a in dict.entries.iter().filter_map(|e| e.anim.as_ref()) {
        assert_eq!(a.signature, a.calc_signature(), "{path}: {:?}", a.name);
        for s in a.sequences.iter().flatten().flatten() {
            assert_eq!(s.signature, Sequence::calc_signature(s.data.as_ref().unwrap()), "{path}: {:?}", a.name);
            n += 1;
        }
    }
    n
}

#[test]
fn clip_dictionaries_convert_both_ways() {
    let Some((fs, hashes)) = open() else { return };
    let cx = XmlContext { hashes: &hashes, folder: None };
    let mut sequences = 0;
    for (path, data) in sample(&fs, ".ycd", 40) {
        let d = read_resource(&data, |r, _| ClipDictionary::read(r, VIRTUAL_BASE, &mut ReadCx::new(&hashes)))
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        if let Some(a) = d.anim_dict() {
            sequences += check_signatures(&path, a);
        }

        // To XML and back: the same clips, and animations packed again with the
        // game's signatures.
        let xml = ycd::to_xml(&data, &cx).unwrap_or_else(|e| panic!("{path}: {e}"));
        let back = ycd::from_xml(&xml, &cx).unwrap_or_else(|e| panic!("{path}: {e}"));
        let d2 = read_resource(&back, |r, _| ClipDictionary::read(r, VIRTUAL_BASE, &mut ReadCx::new(&hashes)))
            .unwrap_or_else(|e| panic!("{path} written back: {e}"));
        let names = |d: &ClipDictionary| {
            let mut v: Vec<String> = d.entries.iter().filter_map(|e| e.clip.as_ref()?.name.clone()).collect();
            v.sort();
            v
        };
        assert_eq!(names(&d), names(&d2), "{path}");
        if let Some(a) = d2.anim_dict() {
            check_signatures(&path, a);
            assert_eq!(a.entries.len(), d.anim_dict().unwrap().entries.len(), "{path}");
        }
    }
    assert!(sequences > 0, "no sequences checked");
}

#[test]
fn anim_dictionaries_convert_both_ways() {
    let Some((fs, hashes)) = open() else { return };
    let cx = XmlContext { hashes: &hashes, folder: None };
    for (path, data) in sample(&fs, ".yad", 15) {
        let d = read_resource(&data, |r, _| AnimDictionary::read(r, VIRTUAL_BASE, &mut ReadCx::new(&hashes)))
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        check_signatures(&path, &d);
        let xml = ycd::yad_to_xml(&data, &cx).unwrap_or_else(|e| panic!("{path}: {e}"));
        let back = ycd::yad_from_xml(&xml, &cx).unwrap_or_else(|e| panic!("{path}: {e}"));
        let d2 = read_resource(&back, |r, _| AnimDictionary::read(r, VIRTUAL_BASE, &mut ReadCx::new(&hashes)))
            .unwrap_or_else(|e| panic!("{path} written back: {e}"));
        check_signatures(&path, &d2);
        assert_eq!(d.name, d2.name, "{path}");
        assert_eq!(d.entries.len(), d2.entries.len(), "{path}");
    }
}
