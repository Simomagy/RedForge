//! The pause map's tiles from a real RDR2 install. These only run when
//! `REDFORGE_GAME_DIR` points at the game folder.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rf_editor::pausemap::export;
use rf_editor::pausemap::families::{FAMILIES, classify};
use rf_editor::pausemap::library::Library;
use rf_editor::pausemap::marks::Assets;
use rf_editor::pausemap::path::from_points;
use rf_editor::pausemap::project::{Common, LineObject, LineStyle, MapObject};
use rf_editor::pausemap::tile::{pack_tile, read_tile};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};

fn fs() -> Option<&'static Rpf8FileSystem> {
    static FS: OnceLock<Option<Rpf8FileSystem>> = OnceLock::new();
    FS.get_or_init(|| {
        let dir = PathBuf::from(std::env::var_os("REDFORGE_GAME_DIR")?);
        let strings = HashIndex::new();
        strings.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt")).unwrap();
        Some(Rpf8FileSystem::open(&dir, Rpf8Keys::from_game_folder(&dir).unwrap(), &strings).unwrap())
    })
    .as_ref()
}

fn library() -> Option<&'static Library> {
    static LIB: OnceLock<Option<Library>> = OnceLock::new();
    LIB.get_or_init(|| fs().map(Library::load)).as_ref()
}

#[test]
fn every_family_reads() {
    let Some(lib) = library() else { return };
    assert!(lib.failed.is_empty(), "{:?}", &lib.failed[..lib.failed.len().min(5)]);
    for f in &FAMILIES {
        let d = lib.get(f.name).unwrap_or_else(|| panic!("{}", f.name));
        let full = d.tile_count();
        let stubs = d.index.iter().filter(|&&i| i == 0).count();
        println!("{}: {full} tiles, {stubs} stubs or missing", f.name);
        assert!(full > 0 && full <= f.cells());
        assert_eq!(full + stubs, f.cells());
    }
    // The sea is stubs, and 59 tiles of update_4's patch are counted once.
    assert_eq!(lib.get("minimap_sdf_text").unwrap().tile_count(), 67);
    assert_eq!(lib.get("minimap_sdf_discovered3").unwrap().tile_count(), 497);
}

/// The vanilla tiles extracted by hand, as the web app reads them.
const VANILLA: &str = "/run/media/msstu/Duomo/RedM/3D/Minimap";
/// A RedM resource exported by Aether Cartography, known to work in game.
const EXPORT: &str = "/run/media/msstu/Duomo/RedM/3D/dusk_map_text/stream/Ytd";

#[test]
fn the_same_pixels_as_extracted_by_hand() {
    let Some(lib) = library() else { return };
    let Ok(dir) = std::fs::read_dir(VANILLA) else { return };
    let (mut checked, mut patched) = (0, 0);
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some((f, row, col)) = classify(&name) else { continue };
        // The hand extraction is of the first archive: the patched tiles are
        // update_4's.
        if fs().unwrap().find(&format!("update_4.rpf/x64/patch/textures/ui/minimap_tiles.rpf/{name}")).is_some() {
            patched += 1;
            continue;
        }
        // CodeX extracts the resource's data without its header: the
        // pixels are its last bytes.
        let data = std::fs::read(e.path()).unwrap();
        let size = u16::from_le_bytes([data[0x58], data[0x59]]) as usize;
        let d = lib.get(f.name).unwrap();
        let pixels = (size > 16).then(|| &data[data.len() - size * size..]);
        assert_eq!(d.tile(row, col), pixels, "{name}");
        checked += 1;
    }
    println!("{checked} tiles the same, {patched} patched since");
    assert!(checked > 1000);
}

#[test]
fn repacked_tiles_are_aethers() {
    let Some(lib) = library() else { return };
    let Ok(dir) = std::fs::read_dir(EXPORT) else { return };
    let (mut same, mut upgraded) = (0, 0);
    for e in dir.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some((f, row, col)) = classify(&name) else { continue };
        let theirs = std::fs::read(e.path()).unwrap();
        let t = read_tile(&theirs).unwrap();
        let pixels = t.pixels.expect("exported tiles are full");
        let d = lib.get(f.name).unwrap();
        let ours = pack_tile(&d.template, &t.name, &pixels).unwrap();
        let back = read_tile(&ours).unwrap();
        assert_eq!(back.name, f.tile_name(row, col), "{name}");
        assert_eq!(back.pixels.as_deref(), Some(&pixels[..]), "{name}");
        // The same dictionary as theirs, but for its name where theirs was a
        // stub made full. RedForge's block map has room for more pages than
        // the game's (48 bytes, as the C# RedForge writes it, 32 in the
        // game's), which RedM loads all the same.
        let dict = |file: &[u8]| read_tile(file).unwrap().dict;
        let (a, b) = (dict(&ours), dict(&theirs));
        assert_eq!(a.hashes, b.hashes, "{name}");
        let (ta, tb) = (a.textures.unwrap(), b.textures.unwrap());
        if d.tile(row, col).is_some() {
            assert_eq!(ta, tb, "{name}");
            same += 1;
        } else {
            let (ta, tb) = (ta[0].as_ref().unwrap(), tb[0].as_ref().unwrap());
            assert_eq!((ta.base.width, ta.base.format, ta.base.flags), (tb.base.width, tb.base.format, tb.base.flags));
            upgraded += 1;
        }
    }
    println!("{same} the same, {upgraded} stubs made full");
    assert!(same > 0);
}

#[test]
fn exported_tiles_carry_a_road() {
    let Some(lib) = library() else { return };
    // A road across Valentine, in map units.
    let road = MapObject::Line(LineObject {
        common: Common { id: "r".into(), ..Default::default() },
        style: LineStyle::Road,
        width: 1.5,
        spacing: 0.0,
        closed: false,
        nodes: from_points(&[[2600.0, 1600.0], [2700.0, 1600.0]], false),
    });
    let (files, plan) = export::build(lib, &[road], &Assets::default()).unwrap();
    assert_eq!(files.len(), plan.tiles.len());
    assert!(plan.skipped.is_empty());
    assert!(files.iter().any(|(n, _)| n.starts_with("minimap_sdf_discovered3_")));
    assert!(files.iter().any(|(n, _)| n.starts_with("minimap_sdf_discoveredlod3_")));
    for (name, file) in &files {
        let t = read_tile(file).unwrap();
        assert_eq!(format!("{}.ytd", t.name), *name);
        let (f, row, col) = classify(name).unwrap();
        let game = lib.get(f.name).unwrap().tile(row, col).unwrap();
        // Only ever darker than the game's: lines go in with min().
        let pixels = t.pixels.unwrap();
        assert!(pixels.iter().zip(game).all(|(a, b)| a <= b), "{name}");
        assert!(pixels.iter().zip(game).any(|(a, b)| a < b), "{name}");
    }
}

#[test]
fn the_games_fonts_read() {
    let Some(fs) = fs() else { return };
    let f = rf_editor::pausemap::gfxfont::LIBRARIES.iter().find_map(|p| fs.find(p)).unwrap();
    let data = fs.extract(f).unwrap();
    let t = std::time::Instant::now();
    let fonts = rf_editor::pausemap::gfxfont::read_library(&data).unwrap();
    println!("{} fonts in {:.2?}", fonts.len(), t.elapsed());
    let names: Vec<&str> = fonts.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(fonts.len(), 18, "{names:?}");
    for want in ["RDR Lino", "Hapna Slab Serif DemiBold", "Redemption"] {
        assert!(names.contains(&want), "{want} in {names:?}");
    }
    let lino = fonts.iter().find(|f| f.name == "RDR Lino").unwrap();
    let a = &lino.glyphs[&'A'];
    // 169 of 256 wide; the cap height about 201 of 256, above the baseline.
    assert!((a.advance - 169.0 / 256.0).abs() < 1e-9);
    let top = a.contours.iter().flatten().map(|p| p[1]).fold(f64::MAX, f64::min);
    assert!((top + 201.0 / 256.0).abs() < 0.01, "{top}");
    assert!(lino.glyphs.len() > 400);
}
