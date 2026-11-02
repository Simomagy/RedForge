//! Models in the game's files, found the way the C# finds them for Blender.

use std::sync::Arc;

use rf_formats::formats::{XmlContext, ydr};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{FileRef, Rpf8FileSystem};
use rf_game::{GameData, name_hash};
use rf_world::{ModelKind, model_files};

use crate::{ModelData, ModelSource};

impl ModelSource {
    /// The model of an archetype in the game's files (the ydr, the ydd's
    /// entry or the yft that `rf_world::model_files` finds), read with its
    /// texture dictionaries by [`read_game_model`]. `None` for an archetype
    /// without a model.
    pub fn of_archetype(
        fs: &Arc<Rpf8FileSystem>,
        data: &Arc<GameData>,
        hashes: &Arc<HashIndex>,
        archetype: u32,
    ) -> Option<ModelSource> {
        let files = model_files(data, archetype)?;
        let path = fs.path(files.file);
        let file = file_name(&path).to_owned();
        // Sollumz names a ydd's drawables by their hash, as the XML writes it.
        // (The C# sent the drawable's own name, `<dictionary>.#dd` for all of
        // them, which matches none: the add-on hid every drawable.)
        let piece = match files.kind {
            ModelKind::Ydd => hashes.xml_name(files.entry),
            ModelKind::Ydr | ModelKind::Yft => stem(&file).to_owned(),
        };
        let (fs, data, hashes, name) = (fs.clone(), data.clone(), hashes.clone(), file.clone());
        let load = Box::new(move || read_game_model(&fs, &data, &hashes, &name, files.file));
        Some(ModelSource { file, piece, load })
    }
}

/// Reads a model file of the game, and its texture dictionaries by
/// [`game_texture_dictionaries`].
pub fn read_game_model(
    fs: &Rpf8FileSystem,
    data: &GameData,
    hashes: &HashIndex,
    file_name: &str,
    file: FileRef,
) -> Result<ModelData, String> {
    let model = fs.extract_loose(file).map_err(|e| format!("Unable to load file: {e}"))?;
    let texture_dictionaries = game_texture_dictionaries(fs, data, hashes, file_name, &model);
    Ok(ModelData { data: model, texture_dictionaries })
}

/// The game's texture dictionaries for a model (`GetDependencyList`), best
/// first, as loose files: for each model in the file (a ydd's drawables, else
/// the file itself), those of the archetype named like it, or else like the
/// file. Also for a model that isn't the game's (a project's), by its name.
pub fn game_texture_dictionaries(
    fs: &Rpf8FileSystem,
    data: &GameData,
    hashes: &HashIndex,
    file_name: &str,
    model: &[u8],
) -> Vec<Vec<u8>> {
    let file_hash = name_hash(stem(file_name));
    let pieces = match pieces(file_name, model, hashes) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("{file_name}: {e}");
            Vec::new()
        }
    };
    let mut txds: Vec<FileRef> = Vec::new();
    for piece in pieces {
        for txd in data.txds_for_piece(piece, file_hash) {
            if !txds.contains(&txd) {
                txds.push(txd);
            }
        }
    }
    txds.into_iter()
        .filter_map(|txd| match fs.extract_loose(txd) {
            Ok(d) => Some(d),
            Err(e) => {
                log::warn!("{}: {e}", fs.path(txd));
                None
            }
        })
        .collect()
}

/// The name hashes of the models in a model file (the C#'s `Pieces` keys): a
/// ydd's entries, else the file's name.
fn pieces(file_name: &str, model: &[u8], hashes: &HashIndex) -> Result<Vec<u32>, String> {
    if !file_name.to_lowercase().ends_with(".ydd") {
        return Ok(vec![name_hash(stem(file_name))]);
    }
    let dict = ydr::read_dictionary(model, &XmlContext { hashes, folder: None }).map_err(|e| e.to_string())?;
    Ok(match (dict.hashes, dict.drawables) {
        (Some(h), Some(d)) => (0..d.len()).map(|i| h.get(i).copied().unwrap_or(0)).collect(),
        _ => Vec::new(),
    })
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn stem(path: &str) -> &str {
    let name = file_name(path);
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}
