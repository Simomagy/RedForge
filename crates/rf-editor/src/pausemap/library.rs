//! The game's tiles, read from `minimap_tiles.rpf`: one buffer per family.

use std::collections::HashMap;

use rayon::prelude::*;
use rf_formats::formats::ytd::TextureDictionary;
use rf_formats::rpf8::Rpf8FileSystem;

use super::families::{FAMILIES, Family, classify};
use super::tile::read_tile;

/// A family's tiles as the game has them.
pub struct FamilyData {
    pub family: &'static Family,
    /// The full-size tiles back to back.
    pub tiles: Vec<u8>,
    /// Per grid cell (`row * cols + col`): the tile's slot in `tiles` plus
    /// one, 0 for a stub or a missing tile.
    pub index: Vec<u16>,
    /// Per grid cell: a stub's value (255 where there's no tile at all).
    pub stub_value: Vec<u8>,
    /// A full-size tile's dictionary, to make new tiles from.
    pub template: TextureDictionary,
}

impl FamilyData {
    /// A cell's pixels; `None` for a stub.
    pub fn tile(&self, row: u32, col: u32) -> Option<&[u8]> {
        let slot = self.index[(row * self.family.cols + col) as usize] as usize;
        let px = (self.family.tile * self.family.tile) as usize;
        (slot > 0).then(|| &self.tiles[(slot - 1) * px..slot * px])
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len() / (self.family.tile * self.family.tile) as usize
    }
}

/// How late an archive path's update is: `update_4.rpf/...` is 5, the
/// first archives 0.
fn update_rank(path: &str) -> u32 {
    path.strip_prefix("update_")
        .and_then(|r| r.split('.').next())
        .and_then(|n| n.parse::<u32>().ok())
        .map_or(0, |n| n + 1)
}

/// The families read, by name.
#[derive(Default)]
pub struct Library {
    pub families: HashMap<&'static str, FamilyData>,
    /// Tiles that couldn't be read: path, why.
    pub failed: Vec<(String, String)>,
}

impl Library {
    pub fn get(&self, name: &str) -> Option<&FamilyData> {
        self.families.get(name)
    }

    /// Every family's tiles in the game's archives.
    pub fn load(fs: &Rpf8FileSystem) -> Library {
        // The update archives' patches (update_4.rpf has 59 of the line and
        // relief tiles again) win over the first ones.
        let mut newest: HashMap<String, (u32, String, rf_formats::rpf8::FileRef)> = HashMap::new();
        for (p, f) in fs.files() {
            let lower = p.to_ascii_lowercase();
            if !lower.contains("minimap_tiles.rpf/") {
                continue;
            }
            let name = lower.rsplit('/').next().unwrap_or_default().to_owned();
            let rank = update_rank(&lower);
            if newest.get(&name).is_none_or(|(r, _, _)| rank > *r) {
                newest.insert(name, (rank, p.to_owned(), f));
            }
        }
        let found: Vec<(String, rf_formats::rpf8::FileRef)> = newest.into_values().map(|(_, p, f)| (p, f)).collect();
        let read: Vec<_> = found
            .par_iter()
            .filter_map(|(path, f)| {
                let name = path.rsplit('/').next().unwrap_or(path);
                let (family, row, col) = classify(name)?;
                let tile = fs.extract_loose(*f).map_err(|e| e.to_string()).and_then(|d| read_tile(&d));
                Some((path.clone(), family, row, col, tile))
            })
            .collect();

        let mut lib = Library::default();
        let mut by_family: HashMap<&'static str, Vec<_>> = HashMap::new();
        for (path, family, row, col, tile) in read {
            match tile {
                Ok(t) if t.pixels.is_some() && t.size != family.tile => {
                    lib.failed.push((path, format!("{} pixels a side, {} expected", t.size, family.tile)))
                }
                Ok(t) => by_family.entry(family.name).or_default().push((row, col, t)),
                Err(e) => lib.failed.push((path, e)),
            }
        }
        for family in &FAMILIES {
            let Some(mut tiles) = by_family.remove(family.name) else { continue };
            tiles.sort_by_key(|(row, col, _)| (*row, *col));
            let Some(template) = tiles.iter().find(|(_, _, t)| t.pixels.is_some()).map(|(_, _, t)| t.dict.clone())
            else {
                continue;
            };
            let px = (family.tile * family.tile) as usize;
            let full = tiles.iter().filter(|(_, _, t)| t.pixels.is_some()).count();
            let mut data = FamilyData {
                family,
                tiles: Vec::with_capacity(full * px),
                index: vec![0; family.cells()],
                stub_value: vec![255; family.cells()],
                template,
            };
            for (row, col, t) in tiles {
                let cell = (row * family.cols + col) as usize;
                match t.pixels {
                    Some(p) => {
                        data.tiles.extend_from_slice(&p);
                        data.index[cell] = (data.tiles.len() / px) as u16;
                    }
                    None => data.stub_value[cell] = t.stub_value,
                }
            }
            lib.families.insert(family.name, data);
        }
        lib
    }
}
