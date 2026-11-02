//! The game data of a real RDR2 install. These only run when `REDFORGE_GAME_DIR`
//! points at the game folder.
//!
//! `matches_the_csharp_startup_cache` also needs the C# RedForge's startup cache
//! written from the same install: `REDFORGE_CSHARP_CACHE`, or by default
//! `RedForge-App/RedForge.Games.RDR2.startup.dat` next to this repository.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rf_formats::hash::{HashIndex, joaat};
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};
use rf_game::{AssetType, GameData, MapEntry, csharp};

struct Game {
    fs: Rpf8FileSystem,
    strings: HashIndex,
    gd: GameData,
}

fn game() -> Option<&'static Game> {
    static GAME: OnceLock<Option<Game>> = OnceLock::new();
    GAME.get_or_init(|| {
        let dir = PathBuf::from(std::env::var_os("REDFORGE_GAME_DIR")?);
        let strings = HashIndex::new();
        strings.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt")).unwrap();
        let fs = Rpf8FileSystem::open(&dir, Rpf8Keys::from_game_folder(&dir).unwrap(), &strings).unwrap();
        let gd = GameData::load(&fs, &strings);
        Some(Game { fs, strings, gd })
    })
    .as_ref()
}

#[test]
fn loads_the_game() {
    let Some(g) = game() else { return };
    let (fs, gd) = (&g.fs, &g.gd);
    assert!(gd.enabled_rpfs.len() > 4000, "{} archives enabled", gd.enabled_rpfs.len());
    assert!(gd.archetypes.len() > 100_000, "{} archetypes", gd.archetypes.len());
    assert!(gd.stream_entries("ytd").count() > 50_000);
    assert!(gd.gtxd_hierarchy.len() > 5000 && gd.hd_txd_bindings[1].len() > 5000);
    assert!(!gd.bounds_cache.is_empty() && !gd.map_data_cache.is_empty() && !gd.asset_owner_map.is_empty());
    assert!(gd.enabled_data_files.get("SCENARIO_POINTS_FILE_RESOURCED").is_some_and(|f| !f.is_empty()));

    // Devices: the title update's file wins over the base game's.
    let f = gd.map_lookup(fs, "common:/data/dlclist.xml").and_then(MapEntry::file).expect("dlclist.xml");
    assert_eq!(fs.path(f), "update_1.rpf/common/data/dlclist.xml");
    assert!(matches!(gd.map_lookup(fs, "dlcpacks:/patchpack004/dlc.rpf"), Some(MapEntry::Archive(_))));

    // A streamed texture dictionary.
    let f = gd.stream_entry("ytd", joaat("valentine")).expect("valentine.ytd");
    assert!(fs.path(f).ends_with("/gtxd_valentine.rpf/valentine.ytd"), "{}", fs.path(f));
}

#[test]
fn finds_a_models_texture_dictionaries() {
    let Some(g) = game() else { return };
    let (fs, gd) = (&g.fs, &g.gd);
    let a = &gd.archetypes[&joaat("val_01_aucbarns_barn2")];
    assert_eq!(a.asset_type, AssetType::DRAWABLE);
    assert_eq!(a.texture_dictionary, joaat("val_01_"));
    let paths: Vec<String> = gd.txds_for_model("val_01_aucbarns_barn2.ydr").into_iter().map(|f| fs.path(f)).collect();
    let folder = "levels_1.rpf/levels/rdr3/area/jklm_07_10/val_01_.rpf";
    let gtxd = "levels_2.rpf/levels/rdr3/gtxd/gtxd_valentine.rpf";
    assert_eq!(
        paths,
        [
            format!("{folder}/val_01_aucbarns_barn2+hidr.ytd"),
            format!("{folder}/val_01_+hi.ytd"),
            format!("{folder}/val_01_.ytd"),
            format!("{gtxd}/valentine+hi.ytd"),
            format!("{gtxd}/valentine.ytd"),
        ]
    );

    // The whole archetype, from its ytyp.
    let bag = gd.archetype_bag(fs, &g.strings, joaat("val_01_aucbarns_barn2")).expect("archetype bag");
    assert_eq!(bag.hash("name"), joaat("val_01_aucbarns_barn2"));
    assert_eq!(bag.hash("textureDictionary"), joaat("val_01_"));
}

#[test]
fn matches_the_csharp_startup_cache() {
    let Some(g) = game() else { return };
    let path = std::env::var_os("REDFORGE_CSHARP_CACHE").map(PathBuf::from).unwrap_or_else(|| {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../RedForge-App/RedForge.Games.RDR2.startup.dat")
    });
    let Ok(file) = std::fs::read(&path) else {
        eprintln!("no C# startup cache at {}: skipped", path.display());
        return;
    };
    let cs = csharp::read(&file).expect("C# startup cache");
    let c = csharp::compare(&g.fs, &g.gd, &cs);
    for l in &c.lines {
        eprintln!("{l}");
    }
    assert!(c.differences.is_empty(), "{:#?}", c.differences);
}
