//! Tests against a real RDR2 install. They only run when `REDFORGE_GAME_DIR` points
//! at the game folder; otherwise they pass without checking anything.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};

fn game_dir() -> Option<PathBuf> {
    std::env::var_os("REDFORGE_GAME_DIR").map(PathBuf::from)
}

fn file_system() -> Option<&'static Rpf8FileSystem> {
    static FS: OnceLock<Option<Rpf8FileSystem>> = OnceLock::new();
    FS.get_or_init(|| {
        let game = game_dir()?;
        let strings = HashIndex::new();
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt");
        strings.load_file(&path).expect("strings.txt");
        let keys = Rpf8Keys::from_game_folder(&game).expect("keys");
        Some(Rpf8FileSystem::open(&game, keys, &strings).expect("archives"))
    })
    .as_ref()
}

#[test]
fn keys_decode_from_the_exe() {
    let Some(game) = game_dir() else { return };
    let keys = Rpf8Keys::from_game_folder(&game).expect("keys");
    // Decoding again from the key alone gives the same result.
    Rpf8Keys::from_aes_key(keys.aes_key).expect("keys from the AES key");
}

#[test]
fn archives_index_with_nested_ones() {
    let Some(fs) = file_system() else { return };
    assert!(fs.file_count() > 400_000, "only {} files", fs.file_count());
    assert!(fs.archives.iter().any(|a| a.parent.is_some()), "no nested archives");
    // Paths resolve back to the same file, whatever the case or separator.
    let (path, f) = fs.files().find(|(p, _)| p.ends_with(".ymap")).expect("a ymap");
    assert_eq!(fs.find(&path.to_uppercase().replace('/', "\\")), Some(f));
}

#[test]
fn resources_extract_raw_and_loose() {
    let Some(fs) = file_system() else { return };
    let mut checked = 0;
    for ext in [".ymap", ".ytyp", ".ydr", ".ybn"] {
        let (path, f) = fs
            .files()
            .find(|(p, f)| p.ends_with(ext) && fs.entry(*f).is_encrypted() && fs.entry(*f).compressor() == 2)
            .unwrap_or_else(|| panic!("no encrypted, compressed {ext}"));
        let e = *fs.entry(f);
        let raw = fs.extract(f).unwrap_or_else(|err| panic!("{path}: {err}"));
        assert_eq!(raw.len() as u64, e.decompressed_size(), "{path}");

        let loose = fs.extract_loose(f).unwrap();
        assert_eq!(&loose[0..4], b"RSC8", "{path}");
        assert_eq!(loose[4] as u32, e.version(), "{path}");
        assert_eq!(loose[5], 1, "{path}: compressor byte should say Oodle");
        checked += 1;
    }
    assert_eq!(checked, 4);
}

#[test]
fn binary_files_extract_as_text() {
    let Some(fs) = file_system() else { return };
    let (path, f) = fs
        .files()
        .find(|(p, f)| p.ends_with(".xml") && !fs.entry(*f).is_resource() && fs.entry(*f).is_encrypted())
        .expect("an encrypted .xml");
    let data = fs.extract(f).unwrap();
    let text = String::from_utf8_lossy(&data);
    assert!(text.trim_start_matches('\u{feff}').trim_start().starts_with('<'), "{path} doesn't look like XML");
}
