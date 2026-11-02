//! Bound materials and the flag enums of bounds, with their XML forms.

use bytemuck::{Pod, Zeroable};

use crate::xml::{XmlNode, XmlWriter};

/// `Rsc8BoundFlags`, by bit. Bits 36 and up have no name.
pub const BOUND_FLAG_NAMES: [&str; 36] = [
    "CF_VOID_TYPE_BIT",
    "CF_MAP_TYPE_WEAPON",
    "CF_MAP_TYPE_MOVER",
    "CF_MAP_TYPE_HORSE",
    "CF_COVER_TYPE",
    "CF_MAP_TYPE_VEHICLE",
    "CF_VEHICLE_NON_BVH_TYPE",
    "CF_VEHICLE_BVH_TYPE",
    "CF_BOX_VEHICLE_TYPE",
    "CF_PED_TYPE",
    "CF_RAGDOLL_TYPE",
    "CF_HORSE_TYPE",
    "CF_HORSE_RAGDOLL_TYPE",
    "CF_OBJECT_TYPE",
    "CF_ENVCLOTH_OBJECT_TYPE",
    "CF_PLANT_TYPE",
    "CF_PROJECTILE_TYPE",
    "CF_EXPLOSION_TYPE",
    "CF_PICKUP_TYPE",
    "CF_FOLIAGE_TYPE",
    "CF_FORKLIFT_FORKS_TYPE",
    "CF_WEAPON_TEST",
    "CF_CAMERA_TEST",
    "CF_AI_TEST",
    "CF_SCRIPT_TEST",
    "CF_WHEEL_TEST",
    "CF_GLASS_TYPE",
    "CF_RIVER_TYPE",
    "CF_SMOKE_TYPE",
    "CF_UNSMASHED_TYPE",
    "CF_STAIR_SLOPE_TYPE",
    "CF_DEEP_SURFACE_TYPE",
    "CF_NO_HORSE_WALKABLE_TYPE",
    "CF_MAP_TYPE_AI_MOVER",
    "CF_HORSE_AVOIDANCE",
    "CF_MAP_TYPE_CAMERA",
];

/// `Rsc8BoundMaterialFlags`, by bit (all 16 are named).
pub const MATERIAL_FLAG_NAMES: [&str; 16] = [
    "FLAG_STAIRS",
    "FLAG_NOT_CLIMBABLE",
    "FLAG_SEE_THROUGH",
    "FLAG_SHOOT_THROUGH",
    "FLAG_NOT_COVER",
    "FLAG_WALKABLE_PATH",
    "FLAG_NO_CAM_COLLISION",
    "FLAG_SHOOT_THROUGH_FX",
    "FLAG_NO_DECAL",
    "FLAG_NO_NAVMESH",
    "FLAG_NO_RAGDOLL",
    "FLAG_VEHICLE_WHEEL",
    "FLAG_NO_PTFX",
    "FLAG_TOO_STEEP_FOR_PLAYER",
    "FLAG_NO_NETWORK_SPAWN",
    "FLAG_NO_CAM_COLLISION_ALLOW_CLIPPING",
];

/// .NET's `ToString()` of a `[Flags]` enum whose names are single bits (and
/// `NONE` = 0): the names of the set bits from lowest to highest, joined by
/// ", ", or the number if a set bit has no name.
pub fn flags_to_string(v: u64, names: &[&str]) -> String {
    if v == 0 {
        return "NONE".into();
    }
    if names.len() < 64 && v >> names.len() != 0 {
        return v.to_string();
    }
    let mut out = String::new();
    for (bit, name) in names.iter().enumerate() {
        if v & (1 << bit) != 0 {
            if !out.is_empty() {
                out.push_str(", ");
            }
            out.push_str(name);
        }
    }
    out
}

/// .NET's `Enum.TryParse<T>(string)` for such an enum: a number, or names
/// separated by commas (OR'ed). `None` where .NET fails.
pub fn parse_flags(s: &str, names: &[&str], bits: u32) -> Option<u64> {
    let t = s.trim();
    let first = t.chars().next()?;
    if first.is_ascii_digit() || first == '-' || first == '+' {
        // Parsed as the (unsigned) underlying type: "-0" is fine, "-1" isn't.
        let v: i128 = t.parse().ok()?;
        let max = if bits >= 64 { u64::MAX as i128 } else { (1i128 << bits) - 1 };
        return (0..=max).contains(&v).then_some(v as u64);
    }
    let mut v = 0u64;
    for part in t.split(',') {
        let p = part.trim();
        if p == "NONE" {
            continue;
        }
        let bit = names.iter().position(|n| *n == p)?;
        v |= 1 << bit;
    }
    Some(v)
}

/// RedForge's `Rsc8BoundsMaterialTypes.GetMaterialHash`, the 16-bit hash of a
/// material name.
pub fn material_hash(name: &str) -> u16 {
    if name.is_empty() {
        return 12667;
    }
    let mut hash: u32 = 0;
    for c in name.encode_utf16() {
        hash = (c as u8 as u32).wrapping_add(hash.wrapping_mul(16));
        let top = hash & 0xF000_0000;
        if top != 0 {
            hash ^= top ^ (top >> 24);
        }
    }
    (hash % 0x7FED + 18) as u16
}

/// `Rsc8BoundMaterial1`: the material hash, procedural id, room id and ped
/// density, packed in a u32.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Pod, Zeroable)]
pub struct BoundMaterial1(pub u32);

impl BoundMaterial1 {
    pub fn hash(self) -> u16 {
        self.0 as u16
    }
    pub fn procedural_id(self) -> u8 {
        (self.0 >> 16) as u8
    }
    pub fn room_id(self) -> u8 {
        ((self.0 >> 24) & 0x1F) as u8
    }
    pub fn ped_density(self) -> u8 {
        ((self.0 >> 29) & 0x7) as u8
    }
    pub fn set_hash(&mut self, v: u16) {
        self.0 = (self.0 & 0xFFFF_0000) | v as u32;
    }
    pub fn set_procedural_id(&mut self, v: u8) {
        self.0 = (self.0 & 0xFF00_FFFF) | ((v as u32) << 16);
    }
    pub fn set_room_id(&mut self, v: u8) {
        self.0 = (self.0 & 0xE0FF_FFFF) | (((v as u32) & 0x1F) << 24);
    }
    pub fn set_ped_density(&mut self, v: u8) {
        self.0 = (self.0 & 0x1FFF_FFFF) | (((v as u32) & 0x7) << 29);
    }

    /// The material's name in XML. The C# shows names from the game's
    /// `materials.meta`, but only once it has loaded the game; the converter
    /// (like the oracle) never does, so the name is the hash in decimal.
    pub fn name(self) -> String {
        self.hash().to_string()
    }

    pub fn write_xml(self, w: &mut XmlWriter, prefix: &str) {
        if self.hash() != 0 {
            w.string(&format!("{prefix}Name"), &self.name());
        }
        if self.procedural_id() != 0 {
            w.u8(&format!("{prefix}ProcID"), self.procedural_id());
        }
        if self.room_id() != 0 {
            w.u8(&format!("{prefix}RoomID"), self.room_id());
        }
        if self.ped_density() != 0 {
            w.u8(&format!("{prefix}PedDens"), self.ped_density());
        }
    }

    pub fn read_xml(n: &XmlNode, prefix: &str) -> Self {
        let mut m = Self(0);
        // The C# hashes whatever name it reads, even the decimal hash it wrote.
        match n.string(&format!("{prefix}Name")) {
            Some(s) if !s.is_empty() => m.set_hash(material_hash(&s)),
            _ => m.set_hash(0),
        }
        m.set_procedural_id(n.u8(&format!("{prefix}ProcID"), 0));
        m.set_room_id(n.u8(&format!("{prefix}RoomID"), 0));
        m.set_ped_density(n.u8(&format!("{prefix}PedDens"), 0));
        m
    }
}

/// `Rsc8BoundMaterial2`: the material flags and an unknown u16.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Pod, Zeroable)]
pub struct BoundMaterial2(pub u32);

impl BoundMaterial2 {
    pub fn flags(self) -> u16 {
        self.0 as u16
    }
    pub fn unk(self) -> u16 {
        (self.0 >> 16) as u16
    }

    pub fn write_xml(self, w: &mut XmlWriter, prefix: &str) {
        if self.flags() != 0 {
            w.string(&format!("{prefix}Flags"), &flags_to_string(self.flags() as u64, &MATERIAL_FLAG_NAMES));
        }
        if self.unk() != 0 {
            w.u16(&format!("{prefix}Unk"), self.unk());
        }
    }

    pub fn read_xml(n: &XmlNode, prefix: &str) -> Self {
        let flags =
            n.string(&format!("{prefix}Flags")).and_then(|s| parse_flags(&s, &MATERIAL_FLAG_NAMES, 16)).unwrap_or(0)
                as u32;
        let unk = n.u16(&format!("{prefix}Unk"), 0) as u32;
        Self(flags | (unk << 16))
    }
}

/// `Rsc8BoundMaterial`, the per-polygon materials of geometries.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Pod, Zeroable)]
pub struct BoundMaterial {
    pub data1: BoundMaterial1,
    pub data2: BoundMaterial2,
}

impl BoundMaterial {
    pub fn write_xml(&self, w: &mut XmlWriter) {
        self.data1.write_xml(w, "");
        self.data2.write_xml(w, "");
    }
    pub fn read_xml(n: &XmlNode) -> Self {
        Self { data1: BoundMaterial1::read_xml(n, ""), data2: BoundMaterial2::read_xml(n, "") }
    }
}

/// `Rsc8BoundCompositeChildrenFlags`: what a composite's child is, and what it
/// collides with (`Rsc8BoundFlags`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Pod, Zeroable)]
pub struct CompositeFlags {
    pub type_flags: u64,
    pub include_flags: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_like_dotnet() {
        assert_eq!(flags_to_string(0b11, &BOUND_FLAG_NAMES), "CF_VOID_TYPE_BIT, CF_MAP_TYPE_WEAPON");
        assert_eq!(flags_to_string(1 << 40, &BOUND_FLAG_NAMES), "1099511627776");
        assert_eq!(parse_flags("CF_VOID_TYPE_BIT, CF_MAP_TYPE_WEAPON", &BOUND_FLAG_NAMES, 64), Some(3));
        assert_eq!(parse_flags("1099511627776", &BOUND_FLAG_NAMES, 64), Some(1 << 40));
        assert_eq!(parse_flags("NOPE", &BOUND_FLAG_NAMES, 64), None);
        assert_eq!(parse_flags("70000", &MATERIAL_FLAG_NAMES, 16), None);
    }

    #[test]
    fn material_hash_of_names() {
        // A decimal hash written by the C# hashes to something else.
        assert_ne!(material_hash("2804"), 2804);
        assert_eq!(material_hash(""), 12667);
    }
}
