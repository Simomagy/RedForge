//! The tile families of the pause map (`minimap_tiles.rpf`), and where each
//! one sits in map space: the stitched main text atlas, one unit per text
//! pixel (5102 x 4082).

/// The map layer a family draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    /// Lettering.
    Text,
    /// Roads, railroads, outlines (the "discovered" families).
    Lines,
    /// The coast.
    Water,
    /// The terrain's shading and buildings' fills (the "undiscovered" families).
    Relief,
}

impl Layer {
    pub const ALL: [Layer; 4] = [Layer::Text, Layer::Lines, Layer::Water, Layer::Relief];
}

/// A family of tiles: a grid of square single-channel (L8) textures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Family {
    pub name: &'static str,
    pub layer: Layer,
    /// A zoomed-out level.
    pub lod: bool,
    pub cols: u32,
    pub rows: u32,
    /// Pixels a side.
    pub tile: u32,
}

pub const FAMILIES: [Family; 7] = [
    Family { name: "minimap_sdf_text", layer: Layer::Text, lod: false, cols: 10, rows: 8, tile: 512 },
    Family { name: "minimap_sdf_textlod3", layer: Layer::Text, lod: true, cols: 5, rows: 5, tile: 512 },
    Family { name: "minimap_sdf_discovered3", layer: Layer::Lines, lod: false, cols: 33, rows: 29, tile: 256 },
    Family { name: "minimap_sdf_discoveredlod3", layer: Layer::Lines, lod: true, cols: 5, rows: 5, tile: 512 },
    Family { name: "minimap_sdf_undiscovered3", layer: Layer::Relief, lod: false, cols: 33, rows: 29, tile: 256 },
    Family { name: "minimap_sdf_undiscoveredlod", layer: Layer::Relief, lod: true, cols: 4, rows: 4, tile: 512 },
    Family { name: "minimap_sdf_waterline", layer: Layer::Water, lod: false, cols: 8, rows: 8, tile: 256 },
];

/// The family by its name.
pub fn family(name: &str) -> Option<&'static Family> {
    FAMILIES.iter().find(|f| f.name == name)
}

/// The detail family of a layer.
pub fn detail(layer: Layer) -> &'static Family {
    FAMILIES.iter().find(|f| f.layer == layer && !f.lod).unwrap()
}

/// Neighbouring tiles share their last and first two rows and columns, bit
/// for bit.
pub const OVERLAP: u32 = 2;

impl Family {
    pub fn stride(&self) -> u32 {
        self.tile - OVERLAP
    }
    pub fn atlas_width(&self) -> u32 {
        (self.cols - 1) * self.stride() + self.tile
    }
    pub fn atlas_height(&self) -> u32 {
        (self.rows - 1) * self.stride() + self.tile
    }
    pub fn cells(&self) -> usize {
        (self.cols * self.rows) as usize
    }
    /// The tile's file name, without `.ytd` (and its texture's name).
    pub fn tile_name(&self, row: u32, col: u32) -> String {
        format!("{}_{row}_{col}", self.name)
    }
}

const TEXT: Family = FAMILIES[0];
/// Map space's width: the text atlas's (5102).
pub const WORLD_W: f64 = ((TEXT.cols - 1) * (TEXT.tile - OVERLAP) + TEXT.tile) as f64;
/// Map space's height (4082).
pub const WORLD_H: f64 = ((TEXT.rows - 1) * (TEXT.tile - OVERLAP) + TEXT.tile) as f64;

/// Map space to a family's atlas pixels: `atlas = (map + o) * s`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    pub sx: f64,
    pub sy: f64,
    pub ox: f64,
    pub oy: f64,
}

impl Fit {
    pub fn apply(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        [(x + self.ox) * self.sx, (y + self.oy) * self.sy]
    }
    /// Atlas pixels per map unit, both ways at once.
    pub fn unit(&self) -> f64 {
        (self.sx * self.sy).sqrt()
    }
}

/// The full-resolution line and relief atlases cover other bounds than the
/// text atlas: this rectangle (in map units) was matched on coastlines (the
/// waterline family's water edges land on the coast strokes of
/// discovered3; 1% either way is clearly worse).
const DETAIL_LINE_RECT: [f64; 4] = [-1.0, 0.0, 4660.0, 4139.0];

pub fn fit_of(f: &Family) -> Fit {
    let (w, h) = (f.atlas_width() as f64, f.atlas_height() as f64);
    if !f.lod && matches!(f.layer, Layer::Lines | Layer::Relief) {
        let [x, y, rw, rh] = DETAIL_LINE_RECT;
        return Fit { sx: w / rw, sy: h / rh, ox: -x, oy: -y };
    }
    // Every other family spans the text atlas exactly.
    Fit { sx: w / WORLD_W, sy: h / WORLD_H, ox: 0.0, oy: 0.0 }
}

/// The grid cell of a family under a map point (the lower one where two
/// overlap).
pub fn tile_at(f: &Family, x: f64, y: f64) -> Option<(u32, u32)> {
    let [ax, ay] = fit_of(f).apply([x, y]);
    let (col, row) = ((ax / f.stride() as f64).floor(), (ay / f.stride() as f64).floor());
    (col >= 0.0 && row >= 0.0 && col < f.cols as f64 && row < f.rows as f64).then_some((row as u32, col as u32))
}

/// A tile's family, row and column from its file name
/// (`minimap_sdf_text_3_4.ytd`).
pub fn classify(file_name: &str) -> Option<(&'static Family, u32, u32)> {
    let lower = file_name.to_ascii_lowercase();
    let stem = lower.strip_suffix(".ytd")?;
    let (rest, col) = stem.rsplit_once('_')?;
    let (name, row) = rest.rsplit_once('_')?;
    let (row, col) = (row.parse::<u32>().ok()?, col.parse::<u32>().ok()?);
    let f = family(name)?;
    (row < f.rows && col < f.cols).then_some((f, row, col))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_names() {
        assert_eq!((WORLD_W, WORLD_H), (5102.0, 4082.0));
        // The web app's rectangle, rounded as it rounds it.
        assert_eq!(DETAIL_LINE_RECT[2], (WORLD_W * 0.91335).round());
        assert_eq!(DETAIL_LINE_RECT[3], (WORLD_H * 1.01385).round());
        let (f, row, col) = classify("MINIMAP_SDF_DISCOVERED3_28_32.ytd").unwrap();
        assert_eq!((f.name, row, col), ("minimap_sdf_discovered3", 28, 32));
        assert!(classify("minimap_sdf_discovered3_29_0.ytd").is_none());
        assert!(classify("minimap_guarma_sdf_text_0_0.ytd").is_none());
        assert_eq!(tile_at(&FAMILIES[0], 509.0, 0.0), Some((0, 0)));
        assert_eq!(tile_at(&FAMILIES[0], 510.0, 1021.0), Some((2, 1)));
        assert_eq!(tile_at(&FAMILIES[0], -1.0, 0.0), None);
    }
}
