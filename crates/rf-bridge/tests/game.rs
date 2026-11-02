//! Exporting the game's models for Blender and converting them back. Only
//! runs when `REDFORGE_GAME_DIR` points at the game folder.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rf_bridge::{ModelSource, convert_xml, export_model};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};
use rf_game::GameData;
use rf_world::{ModelKind, model_files};

fn open() -> Option<(Arc<Rpf8FileSystem>, Arc<GameData>, Arc<HashIndex>)> {
    let dir = PathBuf::from(std::env::var_os("REDFORGE_GAME_DIR")?);
    let strings = HashIndex::new();
    strings.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt")).unwrap();
    let fs = Rpf8FileSystem::open(&dir, Rpf8Keys::from_game_folder(&dir).unwrap(), &strings).unwrap();
    let data = GameData::load(&fs, &strings);
    Some((Arc::new(fs), Arc::new(data), Arc::new(strings)))
}

#[test]
fn game_models_export_and_come_back() {
    let Some((fs, data, hashes)) = open() else { return };
    // Some archetypes of each kind of model, spread over all of them.
    let mut archetypes: Vec<(u32, ModelKind, usize)> = data
        .archetypes
        .keys()
        .filter_map(|&a| model_files(&data, a).map(|m| (a, m.kind, m.txds.len())))
        .filter(|(_, _, txds)| *txds > 0)
        .collect();
    archetypes.sort_by_key(|(a, _, _)| *a);
    let mut picked = Vec::new();
    for kind in [ModelKind::Ydr, ModelKind::Ydd, ModelKind::Yft] {
        let of_kind: Vec<u32> = archetypes.iter().filter(|(_, k, _)| *k == kind).map(|(a, _, _)| *a).collect();
        picked.extend(of_kind.iter().step_by(of_kind.len() / 4).take(4).map(|a| (*a, kind)));
    }

    let dir = std::env::temp_dir().join(format!("rf-bridge-game-test-{}", std::process::id()));
    for (archetype, kind) in picked {
        let name = hashes.name(archetype);
        let source = ModelSource::of_archetype(&fs, &data, &hashes, archetype).unwrap();
        let folder = dir.join(&source.file);
        let model = (source.load)().unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(!model.texture_dictionaries.is_empty(), "{name}: no texture dictionaries");
        let xml = export_model(&source.file, &model, &hashes, &folder).unwrap_or_else(|e| panic!("{name}: {e}"));
        let text = std::fs::read_to_string(&xml).unwrap();
        let stem = source.file.rsplit_once('.').unwrap().0;
        let textures = std::fs::read_dir(folder.join(stem)).map(|d| d.count()).unwrap_or(0);
        assert!(textures > 0, "{name}: no textures exported");
        if kind == ModelKind::Ydd {
            assert!(text.contains(&format!("<Hash>{}</Hash>", source.piece)), "{name}: piece {}", source.piece);
        } else {
            assert_eq!(source.piece, stem);
        }
        // What Sollumz exports is a model like this one: it converts back.
        let back = convert_xml(&xml, &hashes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(back.starts_with(b"RSC8"), "{name}");
        println!("{name}: {} ({kind:?}), {textures} textures", source.file);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
