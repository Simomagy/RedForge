//! A tile's file: a texture dictionary with one square L8 texture, named
//! like the file. Where a family has nothing, the game ships a 4 x 4 stub of
//! one value.

use rf_formats::formats::ytd::TextureDictionary;
use rf_formats::hash::{HashIndex, joaat};
use rf_formats::rsc8::{BlockMap, Rsc8Writer, read_resource};

/// Textures this small are stubs.
const STUB_SIZE: u16 = 16;

pub struct Tile {
    pub name: String,
    /// Pixels a side.
    pub size: u32,
    /// `None` for a stub.
    pub pixels: Option<Vec<u8>>,
    /// A stub's value (255 for a full tile).
    pub stub_value: u8,
    /// The file's dictionary, to make new tiles of the family from.
    pub dict: TextureDictionary,
}

/// Reads a tile from its file (with its `RSC8` header).
pub fn read_tile(loose: &[u8]) -> Result<Tile, String> {
    let dict = read_resource(loose, |r, _| TextureDictionary::read(r, &HashIndex::new())).map_err(|e| e.to_string())?;
    let t = match dict.textures.as_deref() {
        Some([Some(t)]) => &t.base,
        _ => return Err("not a single-texture dictionary".into()),
    };
    if t.width != t.height {
        return Err(format!("not square: {} x {}", t.width, t.height));
    }
    let name = t.name.clone().unwrap_or_default();
    let data = t.data.as_deref().unwrap_or_default();
    let size = t.width as u32;
    if t.width <= STUB_SIZE {
        let stub_value = data.first().copied().unwrap_or(255);
        return Ok(Tile { name, size, pixels: None, stub_value, dict });
    }
    let n = (size * size) as usize;
    if data.len() < n {
        return Err(format!("{} bytes of pixels, {n} expected", data.len()));
    }
    let pixels = Some(data[..n].to_vec());
    Ok(Tile { name, size, pixels, stub_value: 255, dict })
}

/// A tile's file made from a full-size tile of the same family: its name and
/// pixels changed, the rest as the game has it.
pub fn pack_tile(template: &TextureDictionary, name: &str, pixels: &[u8]) -> Result<Vec<u8>, String> {
    let mut dict = template.clone();
    let Some([Some(t)]) = dict.textures.as_deref_mut() else { return Err("not a single-texture dictionary".into()) };
    let size = t.base.width as usize;
    if pixels.len() != size * size {
        return Err(format!("{} pixels for a {size} x {size} tile", pixels.len()));
    }
    if t.base.mip_levels != 1 {
        return Err(format!("the template has {} mips", t.base.mip_levels));
    }
    t.base.name = Some(name.to_owned());
    t.base.data = Some(pixels.to_vec());
    dict.hashes = Some(vec![joaat(&name.to_lowercase())]);
    dict.block_map.get_or_insert_with(BlockMap::default);
    let mut w = Rsc8Writer::new();
    dict.write(&mut w);
    Ok(w.build(2))
}
