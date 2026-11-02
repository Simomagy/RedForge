//! Navmeshes (ynv), from `Rsc8Navmesh.cs` and `YnvFile.cs`.
//!
//! A navmesh is a set of polygons over a grid cell of the map (or a vehicle),
//! with:
//! - vertices quantised to 16 bits inside the navmesh's box (`aabb_min` and
//!   `extents`);
//! - one index and one edge per polygon corner (an edge says which polygon is
//!   across it, possibly in an adjacent navmesh);
//! - special links (ladders, jumps, climbs...) between polygons, referenced by
//!   index from the polygons;
//! - a quadtree over the polygons, for lookups.
//!
//! The vertices, indices, edges and polygons are each stored in a split list
//! ([`NavmeshList`]) of parts of at most 16 KB. [`Navmesh::vertex_positions`],
//! [`Navmesh::indices`], [`Navmesh::edges`] and [`Navmesh::polygons`] give them
//! as flat arrays.
//!
//! The render mesh the C# builds for the editor (`InitNavmesh`) isn't ported.
//!
//! Known differences from the C#, none seen in the game files:
//! - the C# reader reuses an array already read at the same address (its
//!   `BlockPool`), even with another count; this reader doesn't;
//! - where the C# throws (XML without `Polygons`, polygons without an index
//!   list), this port carries on with empty data.

mod build;
mod xml;

use bytemuck::{Pod, Zeroable};

pub use build::NavmeshBuilder;
pub use xml::{from_xml, to_xml};

use crate::rsc8::{BlockBuf, BlockId, BlockMap, Rsc8Reader, Rsc8Writer, read_resource};
use crate::{Result, bail};

/// The resource version of ynv files.
pub const VERSION: u32 = 2;

/// Reads a loose ynv file (with its RSC8 header).
pub fn read(file: &[u8]) -> Result<Navmesh> {
    read_resource(file, |r, _| Navmesh::read(r))
}

/// Writes a navmesh as a loose ynv file, like `YnvFile.Save`.
pub fn write(nm: &Navmesh) -> Vec<u8> {
    nm.write()
}

// --- Flags ------------------------------------------------------------------

/// `Rsc8NavmeshFlags`, the navmesh's `content_flags`.
pub mod content_flags {
    pub const POLYGONS: u32 = 1;
    pub const SPECIAL_LINKS: u32 = 2;
    pub const VEHICLE: u32 = 4;
    pub const UNKNOWN_8: u32 = 8;
    pub const UNKNOWN_16: u32 = 16;
}

/// `Rsc8NavmeshEdgeFlags`, an edge's [`NavmeshEdge::flags`].
pub mod edge_flags {
    pub const ADJACENCY_DISABLED: u32 = 1;
    pub const EDGE_PROVIDES_COVER: u32 = 2;
    pub const HIGH_DROP_OVER_EDGE: u32 = 4;
    pub const EXTERNAL_EDGE: u32 = 8;
    pub const UNKNOWN_16: u32 = 16;
}

/// `Rsc8NavmeshPolyFlags`, a polygon's [`NavmeshPolygon::flags`]: the bits of
/// `flags1`, `flags2`, `flags3` and `flags4` together.
pub mod poly_flags {
    pub const SMALL: u64 = 1;
    pub const LARGE: u64 = 2;
    pub const SHELTERED: u64 = 8;
    pub const SWITCHED_OFF_FOR_PEDS: u64 = 0x40;
    pub const WATER: u64 = 0x80;
    pub const DANGER_KEEP_AWAY_1: u64 = 0x100;
    pub const DANGER_KEEP_AWAY_2: u64 = 0x200;
    pub const NEAR_CAR_NODE: u64 = 0x1000;
    pub const INTERIOR: u64 = 0x2000;
    pub const ISOLATED: u64 = 0x4000;
    /// On 2-vertex polygons (edges) along the edge of the grid cell.
    pub const ZERO_AREA_STITCH_POLY_DLC: u64 = 0x8000;
    pub const NETWORK_SPAWN_CANDIDATE: u64 = 0x10000;
    pub const ROAD: u64 = 0x20000;
    pub const LIES_ALONG_EDGE_OF_MESH: u64 = 0x80000;
    pub const MUDDY: u64 = 0x80_0000;
    pub const VEGETATION: u64 = 0x200_0000;
    pub const BOARDWALK: u64 = 0x400_0000;
    pub const PAVEMENT: u64 = 0x800_0000;
    pub const DOOR: u64 = 0x2000_0000;
    pub const STAIRS: u64 = 0x4000_0000;
    pub const STEEP_SLOPE: u64 = 0x8000_0000;
}

/// The names of `Rsc8NavmeshFlags`, sorted by value as .NET does.
pub(crate) const CONTENT_FLAG_NAMES: &[(u64, &str)] =
    &[(0, "None"), (1, "Polygons"), (2, "SpecialLinks"), (4, "Vehicle"), (8, "Unknown8"), (16, "Unknown16")];

/// The names of `Rsc8NavmeshEdgeFlags`.
pub(crate) const EDGE_FLAG_NAMES: &[(u64, &str)] = &[
    (1, "AdjacencyDisabled"),
    (2, "EdgeProvidesCover"),
    (4, "HighDropOverEdge"),
    (8, "ExternalEdge"),
    (16, "Unknown16"),
];

/// The names of `Rsc8NavmeshPolyFlags`.
pub(crate) const POLY_FLAG_NAMES: &[(u64, &str)] = &[
    (0x1, "Small"),
    (0x2, "Large"),
    (0x4, "Unk3"),
    (0x8, "Sheltered"),
    (0x10, "Unused10"),
    (0x20, "Unused20"),
    (0x40, "SwitchedOffForPeds"),
    (0x80, "Water"),
    (0x100, "DangerKeepAway1"),
    (0x200, "DangerKeepAway2"),
    (0x400, "Sheltered2"),
    (0x800, "Unk12"),
    (0x1000, "NearCarNode"),
    (0x2000, "Interior"),
    (0x4000, "Isolated"),
    (0x8000, "ZeroAreaStitchPolyDLC"),
    (0x10000, "NetworkSpawnCandidate"),
    (0x20000, "Road"),
    (0x40000, "Unk19"),
    (0x80000, "LiesAlongEdgeOfMesh"),
    (0x10_0000, "Unk21"),
    (0x20_0000, "Unk22"),
    (0x40_0000, "Unk23"),
    (0x80_0000, "Muddy"),
    (0x100_0000, "Sheltered3"),
    (0x200_0000, "Vegetation"),
    (0x400_0000, "Boardwalk"),
    (0x800_0000, "Pavement"),
    (0x1000_0000, "Unk29"),
    (0x2000_0000, "Door"),
    (0x4000_0000, "Stairs"),
    (0x8000_0000, "SteepSlope"),
    (0x1_0000_0000, "Unk33"),
    (0x2_0000_0000, "Shelter4"),
    (0x4_0000_0000, "Unk35"),
    (0x8_0000_0000, "Unk36"),
];

/// A `[Flags]` enum value as .NET's `Enum.ToString` writes it: the names of its
/// bits from the lowest, separated by `", "`, or the number when some bits have
/// no name. `names` is sorted by value.
pub(crate) fn format_flags(value: u64, names: &[(u64, &str)]) -> String {
    if value == 0 {
        return match names.first() {
            Some(&(0, n)) => n.to_owned(),
            _ => "0".to_owned(),
        };
    }
    // From the largest value down: an exact match is the name itself.
    let mut index = names.len();
    while index > 0 {
        let v = names[index - 1].0;
        if v == value {
            return names[index - 1].1.to_owned();
        }
        if v < value {
            break;
        }
        index -= 1;
    }
    let mut rest = value;
    let mut found = Vec::new();
    while index > 0 {
        let (v, n) = names[index - 1];
        if index == 1 && v == 0 {
            break;
        }
        if rest & v == v {
            rest -= v;
            found.push(n);
        }
        index -= 1;
    }
    if rest != 0 {
        return value.to_string();
    }
    found.reverse();
    found.join(", ")
}

/// .NET's `Enum.TryParse` for a `[Flags]` enum: a number, or names separated by
/// commas. `None` when it doesn't parse (or there's no text).
pub(crate) fn parse_flags(text: Option<&str>, names: &[(u64, &str)]) -> Option<u64> {
    let t = text?.trim();
    let first = t.chars().next()?;
    if first.is_ascii_digit() || first == '+' || first == '-' {
        if let Ok(v) = t.strip_prefix('+').unwrap_or(t).parse::<u64>() {
            return Some(v);
        }
        if let Ok(v) = t.parse::<i64>() {
            return Some(v as u64);
        }
    }
    let mut value = 0;
    for part in t.split(',') {
        let part = part.trim();
        value |= names.iter().find(|(_, n)| *n == part)?.0;
    }
    Some(value)
}

/// How .NET (9 and later, on x64) converts a `double` to a small integer type
/// (`(ushort)d`, `(short)d`): to an `int`, saturating (NaN is 0), then
/// truncated to the small type by the caller (`as u16`).
fn dotnet_to_int(v: f64) -> i32 {
    v as i32
}

// --- Plain-data structs -----------------------------------------------------

/// `Rsc8NavmeshVertex`: a position quantised to 16 bits in the navmesh's box.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Pod, Zeroable)]
pub struct NavmeshVertex {
    pub x: u16,
    pub y: u16,
    pub z: u16,
}

impl NavmeshVertex {
    /// `ToVector3(aabbmin, aabbsize)`.
    pub fn to_vec3(&self, aabb_min: [f32; 3], aabb_size: [f32; 3]) -> [f32; 3] {
        const MAX: f32 = u16::MAX as f32;
        let q = [self.x as f32 / MAX, self.y as f32 / MAX, self.z as f32 / MAX];
        [aabb_min[0] + aabb_size[0] * q[0], aabb_min[1] + aabb_size[1] * q[1], aabb_min[2] + aabb_size[2] * q[2]]
    }

    /// `FromVector3`: from a position in 0..1 of the box.
    pub fn from_unit(v: [f32; 3]) -> Self {
        // `(ushort)Math.Round(v * 65535)`: rounding half to even, then .NET's
        // conversion (saturating to an int, then truncated: out of the box,
        // positions wrap around).
        let q = |f: f32| dotnet_to_int(((f * u16::MAX as f32) as f64).round_ties_even()) as u16;
        Self { x: q(v[0]), y: q(v[1]), z: q(v[2]) }
    }

    /// `Rsc8NavmeshVertex.Create`: quantises a position in the box.
    pub fn new(v: [f32; 3], aabb_min: [f32; 3], aabb_size: [f32; 3]) -> Self {
        Self::from_unit([
            (v[0] - aabb_min[0]) / aabb_size[0],
            (v[1] - aabb_min[1]) / aabb_size[1],
            (v[2] - aabb_min[2]) / aabb_size[2],
        ])
    }
}

/// `Rsc8NavmeshEdge`: the edge from a polygon corner to the next one, and what's
/// across it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Pod, Zeroable)]
pub struct NavmeshEdge {
    pub data1: u32,
    pub data2: u32,
}

impl NavmeshEdge {
    /// The index in the navmesh's `adj_area_ids` of the navmesh across the edge.
    pub fn area_id1(&self) -> u32 {
        self.data1 & 0x1F
    }
    pub fn set_area_id1(&mut self, v: u32) {
        self.data1 = (self.data1 & 0xFFFF_FFE0) | (v & 0x1F);
    }
    /// The same as `area_id1`.
    pub fn area_id2(&self) -> u32 {
        self.data2 & 0x1F
    }
    pub fn set_area_id2(&mut self, v: u32) {
        self.data2 = (self.data2 & 0xFFFF_FFE0) | (v & 0x1F);
    }
    /// The polygon across the edge (0x7FFF for none).
    pub fn poly_id1(&self) -> u32 {
        (self.data1 >> 5) & 0x7FFF
    }
    pub fn set_poly_id1(&mut self, v: u32) {
        self.data1 = (self.data1 & 0xFFF0_001F) | ((v & 0x7FFF) << 5);
    }
    /// The same as `poly_id1`.
    pub fn poly_id2(&self) -> u32 {
        (self.data2 >> 5) & 0x7FFF
    }
    pub fn set_poly_id2(&mut self, v: u32) {
        self.data2 = (self.data2 & 0xFFF0_001F) | ((v & 0x7FFF) << 5);
    }
    pub fn adjacency_type(&self) -> u32 {
        (self.data1 >> 20) & 0x3
    }
    pub fn set_adjacency_type(&mut self, v: u32) {
        self.data1 = (self.data1 & 0xFFCF_FFFF) | ((v & 0x3) << 20);
    }
    /// The free space around the edge's first vertex.
    pub fn space_around_vertex(&self) -> u32 {
        (self.data1 >> 22) & 0x1F
    }
    pub fn set_space_around_vertex(&mut self, v: u32) {
        self.data1 = (self.data1 & 0xF83F_FFFF) | ((v & 0x1F) << 22);
    }
    pub fn space_beyond_edge(&self) -> u32 {
        (self.data1 >> 27) & 0x1F
    }
    pub fn set_space_beyond_edge(&mut self, v: u32) {
        self.data1 = (self.data1 & 0x07FF_FFFF) | ((v & 0x1F) << 27);
    }
    /// [`edge_flags`].
    pub fn flags(&self) -> u32 {
        self.data2 >> 20
    }
    pub fn set_flags(&mut self, v: u32) {
        self.data2 = (self.data2 & 0x000F_FFFF) | ((v & 0xFFF) << 20);
    }
}

/// `Rsc8NavmeshAABB`: a box in half metres.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Pod, Zeroable)]
pub struct NavmeshAabb {
    pub min_x: i16,
    pub max_x: i16,
    pub min_y: i16,
    pub max_y: i16,
    pub min_z: i16,
    pub max_z: i16,
}

impl NavmeshAabb {
    /// The box around `min`..`max` (rounded outwards), or an empty one if
    /// `min.x > max.x`.
    pub fn new(min: [f32; 3], max: [f32; 3]) -> Self {
        let mut b = Self::default();
        if min[0] > max[0] {
            return b;
        }
        b.set_min(min);
        b.set_max(max);
        b
    }
    pub fn min(&self) -> [f32; 3] {
        [self.min_x as f32 / 2.0, self.min_y as f32 / 2.0, self.min_z as f32 / 2.0]
    }
    pub fn max(&self) -> [f32; 3] {
        [self.max_x as f32 / 2.0, self.max_y as f32 / 2.0, self.max_z as f32 / 2.0]
    }
    pub fn set_min(&mut self, v: [f32; 3]) {
        let f = |x: f32| dotnet_to_int(((x * 2.0) as f64).floor()) as i16;
        (self.min_x, self.min_y, self.min_z) = (f(v[0]), f(v[1]), f(v[2]));
    }
    pub fn set_max(&mut self, v: [f32; 3]) {
        let f = |x: f32| dotnet_to_int(((x * 2.0) as f64).ceil()) as i16;
        (self.max_x, self.max_y, self.max_z) = (f(v[0]), f(v[1]), f(v[2]));
    }
}

/// `Rsc8NavmeshPolygon` (56 bytes). Its corners are `index_count` entries of the
/// navmesh's indices (and edges) from `index_id`; its special links are
/// `link_count` entries of `special_links_inds` from `link_id`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Pod, Zeroable)]
pub struct NavmeshPolygon {
    /// 21 bits of `flags1` (only 8 used), 11 bits of `index_count`.
    pub data1: u32,
    pub index_id: u32,
    pub u08: u64,
    pub u10: u64,
    pub cell_aabb: NavmeshAabb,
    /// 4 bits of audio properties, 12 bits of `flags2`, 1 unused, 3 bits of water
    /// depth, 12 bits of `flags3`.
    pub data2: u32,
    pub centroid_x: u8,
    pub centroid_y: u8,
    pub cover_directions: u8,
    pub immediate_mode_flags: u8,
    /// 8 bits of part ID, 6 bits of link count, 16 bits of area ID, 2 unknown.
    pub data3: u32,
    /// 8 unknown bits (slope?), 20 bits of link ID, 4 bits of `flags4`.
    pub u30: u32,
    pub u34: u32,
}

impl NavmeshPolygon {
    pub fn flags1(&self) -> u32 {
        self.data1 & 0x1F_FFFF
    }
    pub fn set_flags1(&mut self, v: u32) {
        self.data1 = (self.data1 & 0xFFE0_0000) | (v & 0x1F_FFFF);
    }
    pub fn index_count(&self) -> u32 {
        self.data1 >> 21
    }
    pub fn set_index_count(&mut self, v: u32) {
        self.data1 = (self.data1 & 0x1F_FFFF) | ((v & 0x7FF) << 21);
    }
    pub fn audio_properties(&self) -> u8 {
        (self.data2 & 0xF) as u8
    }
    pub fn set_audio_properties(&mut self, v: u8) {
        self.data2 = (self.data2 & 0xFFFF_FFF0) | (v as u32 & 0xF);
    }
    pub fn flags2(&self) -> u32 {
        (self.data2 >> 4) & 0xFFF
    }
    pub fn set_flags2(&mut self, v: u32) {
        self.data2 = (self.data2 & 0xFFFF_000F) | ((v & 0xFFF) << 4);
    }
    pub fn water_depth(&self) -> u8 {
        ((self.data2 >> 17) & 0x7) as u8
    }
    pub fn set_water_depth(&mut self, v: u8) {
        self.data2 = (self.data2 & 0xFFF1_FFFF) | ((v as u32 & 0x7) << 17);
    }
    pub fn flags3(&self) -> u32 {
        self.data2 >> 20
    }
    pub fn set_flags3(&mut self, v: u32) {
        self.data2 = (self.data2 & 0x000F_FFFF) | ((v & 0xFFF) << 20);
    }
    /// The index of the list part the polygon is in.
    pub fn part_id(&self) -> u8 {
        self.data3 as u8
    }
    pub fn set_part_id(&mut self, v: u8) {
        self.data3 = (self.data3 & 0xFFFF_FF00) | v as u32;
    }
    pub fn link_count(&self) -> u8 {
        ((self.data3 >> 8) & 0x3F) as u8
    }
    pub fn set_link_count(&mut self, v: u8) {
        self.data3 = (self.data3 & 0xFFFF_C0FF) | ((v as u32 & 0x3F) << 8);
    }
    /// Always the navmesh's area ID.
    pub fn area_id(&self) -> u16 {
        (self.data3 >> 14) as u16
    }
    pub fn set_area_id(&mut self, v: u16) {
        self.data3 = (self.data3 & 0xC000_3FFF) | ((v as u32) << 14);
    }
    /// 0 or 1, another steep slope bit?
    pub fn u2cc(&self) -> u8 {
        (self.data3 >> 30) as u8
    }
    pub fn set_u2cc(&mut self, v: u8) {
        self.data3 = (self.data3 & 0x3FFF_FFFF) | ((v as u32) << 30);
    }
    /// Lots of values, seems slope related.
    pub fn u30a(&self) -> u8 {
        self.u30 as u8
    }
    pub fn set_u30a(&mut self, v: u8) {
        self.u30 = (self.u30 & 0xFFFF_FF00) | v as u32;
    }
    /// The first index in `special_links_inds`.
    pub fn link_id(&self) -> u32 {
        (self.u30 >> 8) & 0xF_FFFF
    }
    pub fn set_link_id(&mut self, v: u32) {
        self.u30 = (self.u30 & 0xF000_00FF) | ((v & 0xF_FFFF) << 8);
    }
    pub fn flags4(&self) -> u8 {
        (self.u30 >> 28) as u8
    }
    pub fn set_flags4(&mut self, v: u8) {
        self.u30 = (self.u30 & 0x0FFF_FFFF) | ((v as u32 & 0xF) << 28);
    }
    /// All the flags ([`poly_flags`]).
    pub fn flags(&self) -> u64 {
        (self.flags1() as u64 & 0xFF)
            | ((self.flags2() as u64 & 0xFFF) << 8)
            | ((self.flags3() as u64 & 0xFFF) << 20)
            | ((self.flags4() as u64 & 0xF) << 32)
    }
    pub fn set_flags(&mut self, v: u64) {
        self.set_flags1((v & 0xFF) as u32);
        self.set_flags2(((v >> 8) & 0xFFF) as u32);
        self.set_flags3(((v >> 20) & 0xFFF) as u32);
        self.set_flags4(((v >> 32) & 0xF) as u8);
    }

    /// `GetVertex`: the position of one corner.
    pub fn vertex(&self, corner: usize, vertices: &[[f32; 3]], indices: &[u16]) -> [f32; 3] {
        let i = self.index_id as usize + corner;
        indices.get(i).and_then(|&v| vertices.get(v as usize)).copied().unwrap_or_default()
    }
}

/// `Rsc8NavmeshSpecialLink` (32 bytes): a ladder, jump, climb... from a polygon
/// to another, maybe in another navmesh.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Pod, Zeroable)]
pub struct NavmeshSpecialLink {
    pub link_type: u8,
    pub angle: u8,
    pub position1: NavmeshVertex,
    pub position2: NavmeshVertex,
    pub poly_id1: u16,
    /// The same as `poly_id1`.
    pub poly_id1a: u16,
    pub poly_id2: u16,
    /// The same as `poly_id2`.
    pub poly_id2a: u16,
    pub area_id1: u16,
    pub area_id2: u16,
    pub unused1: u16,
    pub unused2: u32,
}

/// `Rsc8NavmeshCoverPoint` (not used in RDR2).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Pod, Zeroable)]
pub struct NavmeshCoverPoint {
    pub position: NavmeshVertex,
    pub angle: u8,
    pub point_type: u8,
}

/// `Rsc8NavmeshList` (`aiSplitArray`): items in parts of at most 16 KB.
#[derive(Clone, Debug, PartialEq)]
pub struct NavmeshList<T> {
    pub vft: u64,
    pub item_count: u32,
    pub unknown_0c: u32,
    pub list_parts: Option<Vec<NavmeshListPart<T>>>,
    /// The index of each part's first item.
    pub list_offsets: Option<Vec<u32>>,
    pub list_parts_count: u32,
    pub unknown_24: u32,
    pub unknown_28: u64,
}

/// `Rsc8NavmeshListPart`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NavmeshListPart<T> {
    pub items: Option<Vec<T>>,
    pub count: u32,
    pub unknown_0c: u32,
}

impl<T> Default for NavmeshList<T> {
    fn default() -> Self {
        Self {
            vft: 0x1_4092_8878,
            item_count: 0,
            unknown_0c: 0,
            list_parts: None,
            list_offsets: None,
            list_parts_count: 0,
            unknown_24: 0,
            unknown_28: 0,
        }
    }
}

impl<T: Pod> NavmeshList<T> {
    /// `ByteCount`.
    pub fn byte_count(&self) -> u32 {
        self.item_count.wrapping_mul(size_of::<T>() as u32)
    }

    /// `GetAllItems`: the items of all the parts, `None` when there are none.
    pub fn all_items(&self) -> Option<Vec<T>> {
        let parts = self.list_parts.as_ref()?;
        let items: Vec<T> = parts.iter().filter_map(|p| p.items.as_deref()).flatten().copied().collect();
        (!items.is_empty()).then_some(items)
    }

    /// `Rebuild`: the items split into parts of at most 16 KB.
    pub fn rebuild(items: &[T]) -> Self {
        let per_part = 16384 / size_of::<T>();
        let mut parts = Vec::new();
        let mut offsets = Vec::new();
        for (i, chunk) in items.chunks(per_part).enumerate() {
            parts.push(NavmeshListPart { items: Some(chunk.to_vec()), count: 0, unknown_0c: 0 });
            offsets.push((i * per_part) as u32);
        }
        Self {
            item_count: items.len() as u32,
            list_parts_count: parts.len() as u32,
            list_parts: Some(parts),
            list_offsets: Some(offsets),
            ..Default::default()
        }
    }

    fn read(r: &mut Rsc8Reader, ptr: u64) -> Result<Option<Self>> {
        r.follow(ptr, |r| {
            let vft = r.u64()?;
            let item_count = r.u32()?;
            let unknown_0c = r.u32()?;
            let parts_ptr = r.u64()?;
            let offsets_ptr = r.u64()?;
            let list_parts_count = r.u32()?;
            let unknown_24 = r.u32()?;
            let unknown_28 = r.u64()?;
            let list_parts = r.items_at(parts_ptr, list_parts_count as usize, |r| {
                let items_ptr = r.u64()?;
                let count = r.u32()?;
                let unknown_0c = r.u32()?;
                Ok(NavmeshListPart { items: r.array_at::<T>(items_ptr, count as usize)?, count, unknown_0c })
            })?;
            let list_offsets = r.array_at::<u32>(offsets_ptr, list_parts_count as usize)?;
            Ok(Self { vft, item_count, unknown_0c, list_parts, list_offsets, list_parts_count, unknown_24, unknown_28 })
        })
    }

    fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let id = w.reserve();
        // An empty list of parts isn't written at all (a null pointer), as the C#
        // `WriteBlocks` does.
        let parts = self.list_parts.as_deref().filter(|p| !p.is_empty());
        let parts_id = parts.map(|parts| {
            let parts_id = w.reserve();
            let mut b = BlockBuf::with_capacity(parts.len() * 16);
            for p in parts {
                let items = w.array(p.items.as_deref());
                b.ptr(items);
                b.u32(p.items.as_ref().map_or(0, |i| i.len() as u32));
                b.u32(p.unknown_0c);
            }
            w.set(parts_id, b, false);
            parts_id
        });
        let offsets = w.array(self.list_offsets.as_deref());
        let mut b = BlockBuf::with_capacity(48);
        b.u64(self.vft);
        b.u32(self.item_count);
        b.u32(self.unknown_0c);
        b.ptr(parts_id);
        b.ptr(offsets);
        b.u32(self.list_parts.as_ref().map_or(0, |p| p.len() as u32));
        b.u32(self.unknown_24);
        b.u64(self.unknown_28);
        w.set(id, b, false);
        id
    }
}

/// `Rsc8NavmeshQuadTree` (96 bytes): a node of the polygon lookup tree.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NavmeshQuadTree {
    pub aabb_min: [f32; 4],
    pub aabb_max: [f32; 4],
    pub cell_aabb: NavmeshAabb,
    /// The polygons of a leaf.
    pub data: Option<NavmeshQuadTreeData>,
    /// Clockwise from +X+Y.
    pub sub_trees: [Option<Box<NavmeshQuadTree>>; 4],
    pub unknown_54: u32,
    pub unknown_58: u64,
}

/// `Rsc8NavmeshQuadTreeData` (32 bytes).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NavmeshQuadTreeData {
    pub cover_points_start_id: u32,
    pub unused_04: u32,
    pub polygon_ids: Option<Vec<u16>>,
    /// Not used in RDR2.
    pub cover_points: Option<Vec<NavmeshCoverPoint>>,
    pub polygon_ids_count: u16,
    pub cover_points_count: u16,
    pub unused_1c: u32,
}

impl NavmeshQuadTree {
    /// `ByteCount`: the size of this node and its data.
    pub fn byte_count(&self) -> u32 {
        let mut n = 96;
        if let Some(d) = &self.data {
            n += 32 + d.polygon_ids_count as u32 * 2 + d.cover_points_count as u32 * 8;
        }
        n
    }

    /// The nodes of the tree, depth first.
    pub fn nodes(&self) -> Vec<&NavmeshQuadTree> {
        let mut out = Vec::new();
        let mut stack = vec![self];
        while let Some(t) = stack.pop() {
            out.push(t);
            stack.extend(t.sub_trees.iter().flatten().map(|b| &**b));
        }
        out
    }

    fn read(r: &mut Rsc8Reader, ptr: u64, depth: usize) -> Result<Option<Box<Self>>> {
        if depth > 64 {
            bail!("navmesh quadtree deeper than 64 levels at 0x{ptr:X}");
        }
        r.follow(ptr, |r| {
            let aabb_min = r.vec4()?;
            let aabb_max = r.vec4()?;
            let cell_aabb = r.pod::<NavmeshAabb>()?;
            let data_ptr = r.u64()?;
            let subs = [r.u64()?, r.u64()?, r.u64()?, r.u64()?];
            let unknown_54 = r.u32()?;
            let unknown_58 = r.u64()?;
            let data = r.follow(data_ptr, |r| {
                let cover_points_start_id = r.u32()?;
                let unused_04 = r.u32()?;
                let ids_ptr = r.u64()?;
                let points_ptr = r.u64()?;
                let polygon_ids_count = r.u16()?;
                let cover_points_count = r.u16()?;
                let unused_1c = r.u32()?;
                Ok(NavmeshQuadTreeData {
                    cover_points_start_id,
                    unused_04,
                    polygon_ids: r.array_at(ids_ptr, polygon_ids_count as usize)?,
                    cover_points: r.array_at(points_ptr, cover_points_count as usize)?,
                    polygon_ids_count,
                    cover_points_count,
                    unused_1c,
                })
            })?;
            let mut sub_trees: [Option<Box<Self>>; 4] = Default::default();
            for (s, p) in sub_trees.iter_mut().zip(subs) {
                *s = Self::read(r, p, depth + 1)?;
            }
            Ok(Box::new(Self { aabb_min, aabb_max, cell_aabb, data, sub_trees, unknown_54, unknown_58 }))
        })
    }

    fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let id = w.reserve();
        let data = self.data.as_ref().map(|d| {
            let ids = w.array(d.polygon_ids.as_deref());
            let points = w.array(d.cover_points.as_deref());
            let mut b = BlockBuf::with_capacity(32);
            b.u32(d.cover_points_start_id);
            b.u32(d.unused_04);
            b.ptr(ids);
            b.ptr(points);
            b.u16(d.polygon_ids.as_ref().map_or(0, |v| v.len() as u16));
            b.u16(d.cover_points.as_ref().map_or(0, |v| v.len() as u16));
            b.u32(d.unused_1c);
            w.add(b, false)
        });
        let subs = self.sub_trees.each_ref().map(|s| s.as_ref().map(|t| t.write(w)));
        let mut b = BlockBuf::with_capacity(96);
        b.vec4(self.aabb_min);
        b.vec4(self.aabb_max);
        b.pod(&self.cell_aabb);
        b.ptr(data);
        for s in subs {
            b.ptr(s);
        }
        b.u32(self.unknown_54);
        b.u64(self.unknown_58);
        w.set(id, b, false);
        id
    }
}

/// `Rsc8Navmesh` (0x170 bytes), the root of a ynv file.
#[derive(Clone, Debug, PartialEq)]
pub struct Navmesh {
    pub vft: u64,
    pub block_map: Option<BlockMap>,
    /// [`content_flags`].
    pub content_flags: u32,
    pub version_a: u16,
    pub version_b: u16,
    pub unknown_018: u64,
    /// (1,0,0,0), (0,1,0,0), (0,0,1,0), (0,0,0,0).
    pub transform: [f32; 16],
    /// The size of the navmesh's box (`AABBSize`); `w` is 0.
    pub extents: [f32; 4],
    pub vertices: Option<NavmeshList<NavmeshVertex>>,
    pub unknown_078: u64,
    pub indices: Option<NavmeshList<u16>>,
    pub edges: Option<NavmeshList<NavmeshEdge>>,
    /// The number of indices (= edges).
    pub size_of_pools: u32,
    pub adj_area_ids_count: u32,
    /// The area IDs of this navmesh and its neighbours, which edges refer to by
    /// index; `adj_area_ids_count` are used.
    pub adj_area_ids: [u32; 32],
    pub polygons: Option<NavmeshList<NavmeshPolygon>>,
    pub quad_tree: Option<Box<NavmeshQuadTree>>,
    /// Also known as portals.
    pub special_links: Option<Vec<NavmeshSpecialLink>>,
    /// The special links of each polygon (`link_id`, `link_count`); also known
    /// as portal links.
    pub special_links_inds: Option<Vec<u16>>,
    pub vertices_count: u32,
    pub polygons_count: u32,
    /// The grid cell, `x + y * 100`?
    pub area_id: u32,
    pub total_bytes: u32,
    pub cover_points_count: u32,
    pub special_links_count: u32,
    pub special_links_inds_count: u32,
    pub unknown_154: u32,
    pub unknown_158: u64,
    pub build_id: u32,
    pub unknown_164: u32,
    pub unknown_168: u64,

    /// The navmesh's box, which the vertices are quantised in: from the root of
    /// the quadtree (or around the origin), not stored in the root struct.
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
}

impl Default for Navmesh {
    fn default() -> Self {
        Self {
            vft: 0,
            block_map: None,
            content_flags: 0,
            version_a: 19,
            version_b: 1,
            unknown_018: 0,
            transform: [0.0; 16],
            extents: [0.0; 4],
            vertices: None,
            unknown_078: 0,
            indices: None,
            edges: None,
            size_of_pools: 0,
            adj_area_ids_count: 0,
            adj_area_ids: [0; 32],
            polygons: None,
            quad_tree: None,
            special_links: None,
            special_links_inds: None,
            vertices_count: 0,
            polygons_count: 0,
            area_id: 0,
            total_bytes: 0,
            cover_points_count: 0,
            special_links_count: 0,
            special_links_inds_count: 0,
            unknown_154: 0,
            unknown_158: 0,
            build_id: 0,
            unknown_164: 0,
            unknown_168: 0,
            aabb_min: [0.0; 3],
            aabb_max: [0.0; 3],
        }
    }
}

impl Navmesh {
    pub fn extents3(&self) -> [f32; 3] {
        [self.extents[0], self.extents[1], self.extents[2]]
    }

    /// `GetVertexPositions`: the vertices in world (or vehicle) space.
    pub fn vertex_positions(&self) -> Option<Vec<[f32; 3]>> {
        let verts = self.vertices.as_ref()?.all_items()?;
        let (min, size) = (self.aabb_min, self.extents3());
        Some(verts.iter().map(|v| v.to_vec3(min, size)).collect())
    }
    /// `GetIndices`.
    pub fn indices(&self) -> Option<Vec<u16>> {
        self.indices.as_ref()?.all_items()
    }
    /// `GetEdges`: one per index.
    pub fn edges(&self) -> Option<Vec<NavmeshEdge>> {
        self.edges.as_ref()?.all_items()
    }
    /// `GetPolygons`.
    pub fn polygons(&self) -> Option<Vec<NavmeshPolygon>> {
        self.polygons.as_ref()?.all_items()
    }
    /// `GetCoverPoints` (not used in RDR2).
    pub fn cover_points(&self) -> Option<Vec<NavmeshCoverPoint>> {
        let tree = self.quad_tree.as_ref()?;
        let points: Vec<NavmeshCoverPoint> =
            tree.nodes().iter().filter_map(|t| t.data.as_ref()?.cover_points.as_deref()).flatten().copied().collect();
        (!points.is_empty()).then_some(points)
    }
    /// `AdjAreaIDsTrimmed`: the used adjacent area IDs, `None` for none.
    pub fn adj_area_ids_trimmed(&self) -> Option<Vec<u32>> {
        if self.adj_area_ids_count == 0 {
            return None;
        }
        let n = (self.adj_area_ids_count as usize).min(32);
        Some(self.adj_area_ids[..n].to_vec())
    }
    /// The setter of `AdjAreaIDsTrimmed`.
    pub fn set_adj_area_ids_trimmed(&mut self, v: Option<&[u32]>) {
        let Some(v) = v else { return };
        let n = v.len().min(32);
        self.adj_area_ids[..n].copy_from_slice(&v[..n]);
        self.adj_area_ids_count = n as u32;
    }
    /// `EnsureAdjAreaID`: the index of an area ID in `adj_area_ids`, added if
    /// it isn't there (0 when there are already 32).
    pub fn ensure_adj_area_id(&mut self, area_id: u32) -> u32 {
        let n = self.adj_area_ids_count;
        if let Some(i) = (0..n.min(32)).find(|&i| self.adj_area_ids[i as usize] == area_id) {
            return i;
        }
        if n >= 32 {
            return 0;
        }
        self.adj_area_ids[n as usize] = area_id;
        self.adj_area_ids_count += 1;
        n
    }

    /// Reads the root struct and everything it points to.
    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let vft = r.u64()?;
        let block_map_ptr = r.u64()?;
        let block_map = r.follow(block_map_ptr, BlockMap::read)?;
        let content_flags = r.u32()?;
        let version_a = r.u16()?;
        let version_b = r.u16()?;
        let unknown_018 = r.u64()?;
        let transform = r.floats::<16>()?;
        let extents = r.vec4()?;
        let vertices_ptr = r.u64()?;
        let unknown_078 = r.u64()?;
        let indices_ptr = r.u64()?;
        let edges_ptr = r.u64()?;
        let size_of_pools = r.u32()?;
        let adj_area_ids_count = r.u32()?;
        let mut adj_area_ids = [0u32; 32];
        for a in &mut adj_area_ids {
            *a = r.u32()?;
        }
        let polygons_ptr = r.u64()?;
        let quad_tree_ptr = r.u64()?;
        let special_links_ptr = r.u64()?;
        let special_links_inds_ptr = r.u64()?;
        let vertices_count = r.u32()?;
        let polygons_count = r.u32()?;
        let area_id = r.u32()?;
        let total_bytes = r.u32()?;
        let cover_points_count = r.u32()?;
        let special_links_count = r.u32()?;
        let special_links_inds_count = r.u32()?;
        let unknown_154 = r.u32()?;
        let unknown_158 = r.u64()?;
        let build_id = r.u32()?;
        let unknown_164 = r.u32()?;
        let unknown_168 = r.u64()?;

        let vertices = NavmeshList::read(r, vertices_ptr)?;
        let indices = NavmeshList::read(r, indices_ptr)?;
        let edges = NavmeshList::read(r, edges_ptr)?;
        let polygons = NavmeshList::read(r, polygons_ptr)?;
        let quad_tree = NavmeshQuadTree::read(r, quad_tree_ptr, 0)?;
        let special_links = r.array_at(special_links_ptr, special_links_count as usize)?;
        let special_links_inds = r.array_at(special_links_inds_ptr, special_links_inds_count as usize)?;

        let xyz = |v: [f32; 4]| [v[0], v[1], v[2]];
        let half = |s: f32| [extents[0] * s, extents[1] * s, extents[2] * s];
        let aabb_min = quad_tree.as_ref().map_or_else(|| half(-0.5), |t| xyz(t.aabb_min));
        let aabb_max = quad_tree.as_ref().map_or_else(|| half(0.5), |t| xyz(t.aabb_max));

        Ok(Self {
            vft,
            block_map,
            content_flags,
            version_a,
            version_b,
            unknown_018,
            transform,
            extents,
            vertices,
            unknown_078,
            indices,
            edges,
            size_of_pools,
            adj_area_ids_count,
            adj_area_ids,
            polygons,
            quad_tree,
            special_links,
            special_links_inds,
            vertices_count,
            polygons_count,
            area_id,
            total_bytes,
            cover_points_count,
            special_links_count,
            special_links_inds_count,
            unknown_154,
            unknown_158,
            build_id,
            unknown_164,
            unknown_168,
            aabb_min,
            aabb_max,
        })
    }

    /// Writes the loose file. The counts are recomputed from the data, as the
    /// C# `Write` does.
    pub fn write(&self) -> Vec<u8> {
        let mut w = Rsc8Writer::new();
        let root = w.reserve();
        let block_map = self.block_map.clone().unwrap_or_default().write(&mut w);
        let vertices = self.vertices.as_ref().map(|l| l.write(&mut w));
        let indices = self.indices.as_ref().map(|l| l.write(&mut w));
        let edges = self.edges.as_ref().map(|l| l.write(&mut w));
        let polygons = self.polygons.as_ref().map(|l| l.write(&mut w));
        let quad_tree = self.quad_tree.as_ref().map(|t| t.write(&mut w));
        let special_links = w.array(self.special_links.as_deref());
        let special_links_inds = w.array(self.special_links_inds.as_deref());

        let count = |n: Option<u32>| n.unwrap_or(0);
        let mut b = BlockBuf::with_capacity(0x170);
        b.u64(self.vft);
        b.ptr(Some(block_map));
        b.u32(self.content_flags);
        b.u16(self.version_a);
        b.u16(self.version_b);
        b.u64(self.unknown_018);
        b.floats(&self.transform);
        b.vec4(self.extents);
        b.ptr(vertices);
        b.u64(self.unknown_078);
        b.ptr(indices);
        b.ptr(edges);
        b.u32(count(self.indices.as_ref().map(|l| l.item_count))); // SizeOfPools
        b.u32(self.adj_area_ids_count);
        for a in self.adj_area_ids {
            b.u32(a);
        }
        b.ptr(polygons);
        b.ptr(quad_tree);
        b.ptr(special_links);
        b.ptr(special_links_inds);
        b.u32(count(self.vertices.as_ref().map(|l| l.item_count)));
        b.u32(count(self.polygons.as_ref().map(|l| l.item_count)));
        b.u32(self.area_id);
        b.u32(self.total_bytes);
        b.u32(0); // CoverPointsCount: not used in RDR2.
        b.u32(count(self.special_links.as_ref().map(|v| v.len() as u32)));
        b.u32(count(self.special_links_inds.as_ref().map(|v| v.len() as u32)));
        b.u32(self.unknown_154);
        b.u64(self.unknown_158);
        b.u32(self.build_id);
        b.u32(self.unknown_164);
        b.u64(self.unknown_168);
        w.set(root, b, false);
        w.build(VERSION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_sizes_match_the_game() {
        assert_eq!(size_of::<NavmeshVertex>(), 6);
        assert_eq!(size_of::<NavmeshEdge>(), 8);
        assert_eq!(size_of::<NavmeshAabb>(), 12);
        assert_eq!(size_of::<NavmeshPolygon>(), 56);
        assert_eq!(size_of::<NavmeshSpecialLink>(), 32);
        assert_eq!(size_of::<NavmeshCoverPoint>(), 8);
    }

    #[test]
    fn flags_format_like_dotnet() {
        assert_eq!(format_flags(0, CONTENT_FLAG_NAMES), "None");
        assert_eq!(format_flags(7, CONTENT_FLAG_NAMES), "Polygons, SpecialLinks, Vehicle");
        assert_eq!(format_flags(0, EDGE_FLAG_NAMES), "0");
        assert_eq!(format_flags(0x12, EDGE_FLAG_NAMES), "EdgeProvidesCover, Unknown16");
        assert_eq!(format_flags(0x22, EDGE_FLAG_NAMES), "34");
        assert_eq!(format_flags(0x2_0000_0201, POLY_FLAG_NAMES), "Small, DangerKeepAway2, Shelter4");
        assert_eq!(parse_flags(Some("Small, DangerKeepAway2, Shelter4"), POLY_FLAG_NAMES), Some(0x2_0000_0201));
        assert_eq!(parse_flags(Some("34"), EDGE_FLAG_NAMES), Some(34));
        assert_eq!(parse_flags(Some("0"), EDGE_FLAG_NAMES), Some(0));
        assert_eq!(parse_flags(Some("Nope"), EDGE_FLAG_NAMES), None);
        assert_eq!(parse_flags(None, EDGE_FLAG_NAMES), None);
    }

    #[test]
    fn bitfields_round_trip() {
        let mut p = NavmeshPolygon::default();
        p.set_flags(0xF_FFFF_FFFF);
        p.set_index_count(5);
        p.set_area_id(46226);
        p.set_link_count(3);
        p.set_link_id(0x12345);
        p.set_u2cc(1);
        p.set_part_id(2);
        assert_eq!(p.flags(), 0xF_FFFF_FFFF);
        assert_eq!((p.index_count(), p.area_id(), p.link_count(), p.link_id()), (5, 46226, 3, 0x12345));
        assert_eq!((p.u2cc(), p.part_id()), (1, 2));

        let mut e = NavmeshEdge::default();
        e.set_area_id1(3);
        e.set_poly_id1(0x7FFF);
        e.set_space_around_vertex(0x1F);
        e.set_flags(0x12);
        assert_eq!((e.area_id1(), e.poly_id1(), e.space_around_vertex(), e.flags()), (3, 0x7FFF, 0x1F, 0x12));
    }
}
