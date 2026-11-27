//! Keeps the GPU's tiles the game's plus the marks: an object that changed
//! (or came, or went) has the tiles it was on and is on drawn again.

use std::collections::{BTreeSet, HashMap};

use rf_editor::Editor;
use rf_editor::pausemap::compose::{compose_tile, tiles_in};
use rf_editor::pausemap::doc::renderable;
use rf_editor::pausemap::families::{Layer, detail};
use rf_editor::pausemap::{MapObject, Mark, mark_for};

use super::gpu::MapGpu;

/// The layers what's drawn changes (the coast isn't drawn on).
const DRAWN: [Layer; 3] = [Layer::Text, Layer::Lines, Layer::Relief];

struct Entry {
    obj: MapObject,
    mark: Option<Mark>,
    tiles: Vec<(u32, u32)>,
}

#[derive(Default)]
pub struct TileSync {
    /// Per layer, per object.
    cache: HashMap<Layer, HashMap<String, Entry>>,
    /// The fonts' and pictures' count when last drawn: new ones redraw all.
    assets: (usize, usize),
}

impl TileSync {
    /// Brings the GPU's tiles up to the objects. Returns how many tiles were
    /// drawn again.
    pub fn sync(&mut self, gpu: &mut MapGpu, queue: &eframe::wgpu::Queue, ed: &Editor, objects: &[MapObject]) -> usize {
        let lib = ed.pause_tiles();
        let assets = &ed.pause_assets;
        let now = (assets.fonts.all().len(), assets.pictures.images.len());
        let force = std::mem::replace(&mut self.assets, now) != now;
        let shown = renderable(objects);
        let mut drawn = 0;
        for layer in DRAWN {
            let f = detail(layer);
            let Some(data) = lib.get(f.name) else { continue };
            let cache = self.cache.entry(layer).or_default();
            let mut dirty: BTreeSet<(u32, u32)> = BTreeSet::new();
            let mut seen = std::collections::HashSet::new();
            for o in &shown {
                seen.insert(o.id().to_owned());
                if !force && cache.get(o.id()).is_some_and(|e| e.obj == **o) {
                    continue;
                }
                if let Some(prev) = cache.get(o.id()) {
                    dirty.extend(prev.tiles.iter().copied());
                }
                let mark = mark_for(f, o, assets);
                let tiles = mark.as_ref().map(|m| tiles_in(f, &m.bounds())).unwrap_or_default();
                dirty.extend(tiles.iter().copied());
                cache.insert(o.id().to_owned(), Entry { obj: (*o).clone(), mark, tiles });
            }
            cache.retain(|id, e| {
                let keep = seen.contains(id);
                if !keep {
                    dirty.extend(e.tiles.iter().copied());
                }
                keep
            });
            // In the list's order: the marks merge with min, so it doesn't
            // matter, but erasers go first.
            let order: Vec<&str> = shown.iter().map(|o| o.id()).collect();
            for (row, col) in dirty {
                let marks: Vec<&Mark> = order
                    .iter()
                    .filter_map(|id| cache.get(*id))
                    .filter(|e| e.tiles.contains(&(row, col)))
                    .filter_map(|e| e.mark.as_ref())
                    .collect();
                if let Some(pixels) = compose_tile(data, row, col, marks) {
                    gpu.update_tile(queue, layer, row, col, &pixels);
                    drawn += 1;
                }
            }
        }
        drawn
    }
}
