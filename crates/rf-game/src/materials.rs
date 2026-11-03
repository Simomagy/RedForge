//! The game's collision materials (`materials.meta`), as the C# RedForge's
//! `Rsc8BoundsMaterialTypes` reads them: by the 16-bit hash bounds store
//! ([`rf_formats::formats::ybn::material_hash`] of the name), each with
//! RedForge's colour for its VFX group (`CreateMaterialColour`), which the
//! "Physics" mode shows collisions in.

use std::collections::HashMap;

use rf_formats::formats::ybn::material_hash;
use rf_formats::rpf8::Rpf8FileSystem;
use rf_formats::xml::reader::XmlDocument;

/// Where the materials are, patched first (the C# only reads the patched one).
const PATHS: [&str; 2] =
    ["update_1.rpf/common/data/materials/materials.meta", "common_0.rpf/data/materials/materials.meta"];

/// `Rsc8BoundsMaterialData`: a material's `materials.meta` item.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Material {
    pub name: String,
    pub filter: String,
    pub vfx_group: String,
    pub friction: f32,
    pub elasticity: f32,
    pub density: f32,
    pub flags: String,
    /// Straight RGBA, from the VFX group.
    pub colour: [u8; 4],
}

/// The materials by hash (`MaterialsByHash`).
#[derive(Clone, Debug, Default)]
pub struct Materials {
    by_hash: HashMap<u16, Material>,
}

/// The colour of a material that isn't known (`Rsc8BoundMaterial1.Colour`).
pub const UNKNOWN_COLOUR: [u8; 4] = [255, 0, 0, 255];

impl Materials {
    /// The game's materials; none when the file isn't there.
    pub fn load(fs: &Rpf8FileSystem) -> Self {
        let text = PATHS.iter().find_map(|p| fs.extract(fs.find(p)?).ok());
        match text {
            Some(t) => Self::read(&String::from_utf8_lossy(&t)),
            None => {
                log::warn!("materials.meta not found: collisions show in red");
                Self::default()
            }
        }
    }

    /// `AddMaterialsDat`: the `materials/Item`s of the file.
    pub fn read(text: &str) -> Self {
        let doc = XmlDocument::new(text);
        let list = doc.with_root(None, |root| {
            let Some(items) = root.child("materials") else { return Vec::new() };
            items
                .children("Item")
                .map(|item| {
                    let text = |id: &str| item.string(id).unwrap_or_default();
                    let vfx_group = text("vfxGroup");
                    Material {
                        name: text("name"),
                        filter: text("filter"),
                        friction: item.f32("friction", 0.0),
                        elasticity: item.f32("elasticity", 0.0),
                        density: item.f32("density", 0.0),
                        flags: text("flags"),
                        colour: vfx_group_colour(&vfx_group),
                        vfx_group,
                    }
                })
                .collect()
        });
        let list = list.unwrap_or_else(|e| {
            log::warn!("materials.meta: {e}");
            Vec::new()
        });
        Self { by_hash: list.into_iter().map(|m| (material_hash(&m.name), m)).collect() }
    }

    pub fn len(&self) -> usize {
        self.by_hash.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty()
    }

    /// `GetMaterial(hash)`.
    pub fn get(&self, hash: u16) -> Option<&Material> {
        self.by_hash.get(&hash)
    }

    /// `Rsc8BoundMaterial1.Colour`: the material's colour, red if it's unknown.
    pub fn colour(&self, hash: u16) -> [u8; 4] {
        self.get(hash).map_or(UNKNOWN_COLOUR, |m| m.colour)
    }

    /// `Rsc8BoundMaterial1.Name`: the material's name, or the hash.
    pub fn name(&self, hash: u16) -> String {
        self.get(hash).map_or_else(|| hash.to_string(), |m| m.name.clone())
    }
}

/// `CreateMaterialColour`: RedForge's colour for a VFX group, opaque.
pub fn vfx_group_colour(group: &str) -> [u8; 4] {
    let [r, g, b] = match group {
        "VFXGROUP_VOID" => [0, 0, 0],
        "VFXGROUP_CONCRETE" => [145, 145, 145],
        "VFXGROUP_TARMAC" => [90, 90, 90],
        "VFXGROUP_STONE" => [185, 185, 185],
        "VFXGROUP_STONE_DIRT" => [180, 175, 165],
        "VFXGROUP_PAVING" => [200, 165, 130],
        "VFXGROUP_BRICK" => [195, 95, 30],
        "VFXGROUP_SANDSTONE" => [215, 195, 150],
        "VFXGROUP_SANDSTONE_BRITTLE" => [205, 180, 120],
        "VFXGROUP_GRAVEL" | "VFXGROUP_GRAVEL_LARGE" => [235, 235, 235],
        "VFXGROUP_GRAVEL_MUDDY" => [235, 215, 215],
        "VFXGROUP_GRAVEL_DEEP" => [235, 225, 225],
        "VFXGROUP_GRAVEL_MUDDY_DEEP" => [225, 225, 225],
        "VFXGROUP_GRAVEL_DIRT" => [245, 235, 235],
        "VFXGROUP_GRAVEL_GRASS" => [235, 255, 235],
        "VFXGROUP_MUD_HARD" => [115, 105, 85],
        "VFXGROUP_MUD_SOFT" | "VFXGROUP_MUD_DEEP" => [105, 95, 75],
        "VFXGROUP_MUD_UNDERWATER" => [75, 65, 50],
        "VFXGROUP_MARSH" => [105, 155, 75],
        "VFXGROUP_MARSH_DEEP" => [95, 145, 65],
        "VFXGROUP_CLAY" => [160, 160, 160],
        "VFXGROUP_DIRT_DRY" => [175, 160, 140],
        "VFXGROUP_DIRT_DRY_DEEP" => [165, 150, 130],
        "VFXGROUP_DIRT_GRASS" => [165, 180, 130],
        "VFXGROUP_SOIL" => [105, 115, 75],
        "VFXGROUP_SOIL_DEEP" => [95, 105, 65],
        "VFXGROUP_SAND_COMPACT" => [250, 240, 220],
        "VFXGROUP_SAND_WET" => [190, 185, 165],
        "VFXGROUP_SAND_DRY_DEEP" => [120, 110, 95],
        "VFXGROUP_SAND_WET_DEEP" => [110, 100, 85],
        "VFXGROUP_QUICKSAND" => [130, 120, 105],
        "VFXGROUP_TAR_PIT" => [80, 80, 80],
        "VFXGROUP_ICE" => [200, 250, 255],
        "VFXGROUP_SNOW_LOOSE" | "VFXGROUP_SNOW_COMPACT" => [255, 255, 255],
        "VFXGROUP_SLUSH" => [235, 255, 255],
        "VFXGROUP_MANURE" => [110, 70, 70],
        "VFXGROUP_MOSS" => [110, 190, 110],
        "VFXGROUP_ASH" => [110, 110, 110],
        "VFXGROUP_COAL" => [30, 30, 30],
        "VFXGROUP_WOOD"
        | "VFXGROUP_WOOD_POLISHED"
        | "VFXGROUP_WOOD_ROTTEN"
        | "VFXGROUP_WOOD_BURNING"
        | "VFXGROUP_WOOD_BURNT"
        | "VFXGROUP_WOOD_SOLID_SMALL"
        | "VFXGROUP_CAR_WOOD" => [155, 130, 95],
        "VFXGROUP_WOOD_DUSTY" => [165, 145, 110],
        "VFXGROUP_WOOD_SPLINTER" => [170, 150, 125],
        "VFXGROUP_METAL" | "VFXGROUP_METAL_THIN" | "VFXGROUP_METAL_SOLID_SMALL" => [155, 180, 190],
        "VFXGROUP_CERAMIC" => [220, 210, 195],
        "VFXGROUP_MARBLE" => [195, 155, 145],
        "VFXGROUP_LAMINATE" => [240, 230, 185],
        "VFXGROUP_CARPET_FABRIC" | "VFXGROUP_CARPET_FABRIC_DUSTY" => [250, 100, 100],
        "VFXGROUP_LINOLEUM" => [205, 150, 80],
        "VFXGROUP_PLASTER_BRITTLE" => [225, 225, 225],
        "VFXGROUP_RUBBER" => [70, 70, 70],
        "VFXGROUP_PLASTIC" | "VFXGROUP_PLASTIC_HOLLOW" => [255, 250, 210],
        "VFXGROUP_CARDBOARD" => [120, 115, 95],
        "VFXGROUP_PAPER" => [230, 225, 220],
        "VFXGROUP_MATTRESS_FOAM" => [230, 235, 240],
        "VFXGROUP_FEATHERS" => [230, 230, 230],
        "VFXGROUP_LEATHER" => [150, 100, 85],
        "VFXGROUP_WICKER" => [180, 130, 105],
        "VFXGROUP_TERRACOTTA" => [220, 180, 105],
        "VFXGROUP_HEMP_SACK" => [180, 140, 115],
        "VFXGROUP_GRASS" | "VFXGROUP_GRASS_SHORT" => [130, 205, 75],
        "VFXGROUP_GRASS_DEAD" => [190, 205, 150],
        "VFXGROUP_GRASS_LUSH" => [130, 235, 75],
        "VFXGROUP_CROP_HUSK" | "VFXGROUP_HAY" => [240, 205, 125],
        "VFXGROUP_BUSHES" | "VFXGROUP_BUSHES_SPARSE" | "VFXGROUP_BUSHES_DENSE" => [85, 160, 30],
        "VFXGROUP_TREE_BARK" | "VFXGROUP_TREE_PINE" => [115, 100, 70],
        "VFXGROUP_TREE_PINE_SNOW" => [145, 130, 100],
        "VFXGROUP_FLOWERS" => [200, 180, 190],
        "VFXGROUP_TWIGS" => [90, 100, 50],
        "VFXGROUP_LEAVES"
        | "VFXGROUP_LEAVES_TOBACCO"
        | "VFXGROUP_LEAVES_MAPLE"
        | "VFXGROUP_LEAVES_OAK"
        | "VFXGROUP_LEAVES_FICUS" => [70, 100, 50],
        "VFXGROUP_PINE_NEEDLES" => [90, 100, 60],
        "VFXGROUP_CACTUS" => [110, 195, 55],
        "VFXGROUP_CLOTH" => [230, 235, 235],
        "VFXGROUP_GLASS" | "VFXGROUP_GLASS_SHOOT_THROUGH" | "VFXGROUP_GLASS_BULLETPROOF" => [205, 240, 255],
        "VFXGROUP_CAR_GLASS" => [210, 245, 245],
        "VFXGROUP_CAR_METAL" | "VFXGROUP_CAR_PLASTIC" => [255, 255, 255],
        "VFXGROUP_PUDDLE" | "VFXGROUP_LIQUID_WATER" => [55, 145, 230],
        "VFXGROUP_LIQUID_BLOOD" => [205, 5, 5],
        "VFXGROUP_LIQUID_OIL" => [80, 65, 65],
        "VFXGROUP_LIQUID_PETROL" => [70, 100, 120],
        "VFXGROUP_GENERIC" => [255, 0, 255],
        "VFXGROUP_FRESH_MEAT" | "VFXGROUP_PED_HEAD" | "VFXGROUP_PED_CAPSULE" | "VFXGROUP_PED_TORSO" => [255, 55, 20],
        "VFXGROUP_DRIED_MEAT" | "VFXGROUP_PED_LIMB" | "VFXGROUP_PED_FOOT" => [185, 100, 85],
        "VFXGROUP_PELT_FUR" => [185, 130, 95],
        "VFXGROUP_BONE" => [225, 220, 200],
        _ => [0xCC, 0xCC, 0xCC],
    };
    [r, g, b, 255]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_items() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<CMaterialMgr>
  <materials>
    <Item>
      <name>WOOD_SOLID_MEDIUM</name>
      <filter>WOOD</filter>
      <vfxGroup>VFXGROUP_WOOD</vfxGroup>
      <friction value="0.6"/>
    </Item>
    <Item>
      <name>MAKEBELIEVE</name>
      <vfxGroup>VFXGROUP_NOPE</vfxGroup>
    </Item>
  </materials>
</CMaterialMgr>"#;
        let m = Materials::read(xml);
        assert_eq!(m.len(), 2);
        let wood = m.get(material_hash("WOOD_SOLID_MEDIUM")).unwrap();
        assert_eq!((wood.filter.as_str(), wood.friction, wood.colour), ("WOOD", 0.6, [155, 130, 95, 255]));
        assert_eq!(m.colour(material_hash("MAKEBELIEVE")), [0xCC, 0xCC, 0xCC, 255]);
        assert_eq!(m.colour(1), UNKNOWN_COLOUR);
        assert_eq!(m.name(1), "1");
    }
}
