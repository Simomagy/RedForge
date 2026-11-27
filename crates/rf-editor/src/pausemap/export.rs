//! The edited tiles, for a RedM resource's `stream`: each tile a mark
//! touches, drawn again, detail and zoomed-out levels alike.

use std::collections::BTreeMap;

use rayon::prelude::*;

use super::compose::{can_fill_stub, compose_tile, tiles_in};
use super::doc::renderable;
use super::families::{FAMILIES, Family};
use super::library::Library;
use super::marks::{Assets, Mark, mark_for};
use super::project::MapObject;
use super::tile::pack_tile;

/// The families an export writes (the coast's isn't drawn on).
pub const TARGETS: [&str; 6] = [
    "minimap_sdf_text",
    "minimap_sdf_textlod3",
    "minimap_sdf_discovered3",
    "minimap_sdf_discoveredlod3",
    "minimap_sdf_undiscovered3",
    "minimap_sdf_undiscoveredlod",
];

/// Files by name: `minimap_sdf_text_3_4.ytd`, its bytes.
pub type TileFiles = Vec<(String, Vec<u8>)>;

pub struct PlannedTile {
    pub family: &'static Family,
    pub row: u32,
    pub col: u32,
    /// Indices into the plan's marks.
    pub marks: Vec<usize>,
}

#[derive(Default)]
pub struct ExportPlan {
    pub marks: Vec<Mark>,
    pub tiles: Vec<PlannedTile>,
    /// Tiles a mark touches where the game ships an empty stub of a family
    /// that can't be filled: nothing drawn there shows in game.
    pub skipped: Vec<String>,
}

pub fn plan(lib: &Library, objects: &[MapObject], assets: &Assets) -> ExportPlan {
    let shown = renderable(objects);
    let mut plan = ExportPlan::default();
    for name in TARGETS {
        let (Some(data), Some(f)) = (lib.get(name), FAMILIES.iter().find(|f| f.name == name)) else { continue };
        let mut by_tile: BTreeMap<(u32, u32), Vec<usize>> = BTreeMap::new();
        for o in &shown {
            let Some(m) = mark_for(f, o, assets) else { continue };
            let tiles = tiles_in(f, &m.bounds());
            if tiles.is_empty() {
                continue;
            }
            let i = plan.marks.len();
            plan.marks.push(m);
            for t in tiles {
                by_tile.entry(t).or_default().push(i);
            }
        }
        for ((row, col), marks) in by_tile {
            if data.tile(row, col).is_some() || can_fill_stub(f) {
                plan.tiles.push(PlannedTile { family: f, row, col, marks });
            } else if marks.iter().any(|&m| !plan.marks[m].erases()) {
                // An eraser over empty sea takes nothing away: only marks
                // drawn there are lost.
                plan.skipped.push(f.tile_name(row, col));
            }
        }
    }
    plan
}

/// The tiles' files: `minimap_sdf_..._row_col.ytd`, and the plan.
pub fn build(lib: &Library, objects: &[MapObject], assets: &Assets) -> Result<(TileFiles, ExportPlan), String> {
    let plan = plan(lib, objects, assets);
    let files = plan
        .tiles
        .par_iter()
        .map(|t| {
            let data = lib.get(t.family.name).unwrap();
            let pixels = compose_tile(data, t.row, t.col, t.marks.iter().map(|&i| &plan.marks[i])).unwrap();
            let name = t.family.tile_name(t.row, t.col);
            let file = pack_tile(&data.template, &name, &pixels).map_err(|e| format!("{name}: {e}"))?;
            Ok((format!("{name}.ytd"), file))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok((files, plan))
}
