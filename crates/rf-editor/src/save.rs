//! What saving a ymap works out again: its extents and flags.

use std::collections::HashMap;

use rf_formats::formats::databag::DataBag;

/// Works out a `CMapData`'s extents and flags again (being ported).
pub fn update_map(_map: &mut DataBag, _archetypes: &HashMap<u32, rf_game::Archetype>) {}
