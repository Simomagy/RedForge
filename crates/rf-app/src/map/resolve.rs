//! Which files make an archetype's model (`RDR2MapFileCache.GetPiece`).
//!
//! Stands in for `rf_world::model_files` until the world's port lands.

use rf_game::{AssetType, GameData};
use rf_world::{ModelFiles, ModelKind};

pub fn model_files(data: &GameData, archetype: u32) -> Option<ModelFiles> {
    let a = data.archetypes.get(&archetype)?;
    let (kind, ext, pack) = match a.asset_type {
        AssetType::DRAWABLE => (ModelKind::Ydr, "ydr", a.asset_name),
        AssetType::DRAWABLE_DICTIONARY => (ModelKind::Ydd, "ydd", a.drawable_dictionary),
        AssetType::FRAGMENT => (ModelKind::Yft, "yft", a.asset_name),
        _ => return None,
    };
    let file = data.stream_entry(ext, pack)?;
    let txds = data.txds(a.asset_type, a.asset_name, a.texture_dictionary);
    Some(ModelFiles { kind, file, entry: a.asset_name, txds })
}
