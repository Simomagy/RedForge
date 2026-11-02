//! Texture dictionaries from the game: to XML (with DDS files), back to a ytd,
//! and to XML again gives the same XML and DDS files. Only runs when
//! `REDFORGE_GAME_DIR` points at the game folder.

use std::path::{Path, PathBuf};

use rf_formats::formats::{self, XmlContext};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};

fn open() -> Option<(Rpf8FileSystem, HashIndex)> {
    let game = PathBuf::from(std::env::var_os("REDFORGE_GAME_DIR")?);
    let strings = HashIndex::new();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt");
    strings.load_file(&path).expect("strings.txt");
    let keys = Rpf8Keys::from_game_folder(&game).expect("keys");
    let fs = Rpf8FileSystem::open(&game, keys, &strings).expect("archives");
    Some((fs, strings))
}

fn dir_files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap()))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[test]
fn ytd_round_trips_through_xml() {
    let Some((fs, hashes)) = open() else { return };
    let mut files: Vec<_> = fs.files().filter(|(p, _)| p.ends_with(".ytd")).collect();
    files.sort();
    // Some files spread over all of them, and some with volumes, cube maps,
    // arrays and uncompressed or rare formats (RGBA32F, RGBA16F, RGB10A2,
    // B5G5R5A1, BC6H, BC7 sRGB, 12 mips, flags 0x18228002).
    let step = files.len() / 40;
    let special = [
        "/x64/textures/graphics.ytd",
        "/lutpostfx.ytd",
        "/skydome.ytd",
        "/mp001_s_mp_catalogue01x.ytd",
        "/footage_tu002_ffx_ondemand.ytd",
        "/footage_tu002_ffx_resident.ytd",
        "/amb_lights.ytd",
        "/amv_zone_0.ytd",
        "/deferred_probes_256x256.rpf/2669690291.ytd",
    ];
    let picked = files.iter().step_by(step).chain(files.iter().filter(|(p, _)| special.iter().any(|s| p.ends_with(s))));

    let tmp = std::env::temp_dir().join(format!("rf-ytd-game-test-{}", std::process::id()));
    let (mut checked, mut specials) = (0, 0);
    for (path, f) in picked {
        let name = path.rsplit('/').next().unwrap();
        let data = fs.extract_loose_deflate(*f).unwrap();
        let (a, b) = (tmp.join(format!("{checked}a")), tmp.join(format!("{checked}b")));
        let xml = formats::to_xml(name, &data, &XmlContext { hashes: &hashes, folder: Some(a.clone()) })
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        if !xml.contains("<Textures") {
            // An empty dictionary: the C# can't read that XML back either.
            continue;
        }
        let xml_name = format!("{name}.xml");
        let bin = formats::from_xml(&xml_name, &xml, &XmlContext { hashes: &hashes, folder: Some(a.clone()) })
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        let again = formats::to_xml(name, &bin, &XmlContext { hashes: &hashes, folder: Some(b.clone()) }).unwrap();
        assert_eq!(xml, again, "{path}");
        assert_eq!(dir_files(&a), dir_files(&b), "{path}: DDS files differ");
        checked += 1;
        specials += special.iter().any(|s| path.ends_with(s)) as usize;
    }
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(checked >= 30, "only {checked} files checked");
    assert!(specials >= special.len(), "only {specials} of the special files found");
}
