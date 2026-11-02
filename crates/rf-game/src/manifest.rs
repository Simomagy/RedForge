//! What Init reads from the files it enables: archetypes from ytyps, the
//! `_manifest.ymf` of each archive (texture dictionary parents, HD texture
//! bindings, map data groups, asset owners, interiors), the `_cache.ych` files
//! (bounds, map data and interior proxy caches) and `gtxd.ymt`.
//!
//! The C# keeps the data bags; these are plain structs with the fields the C#
//! reads from them. [`crate::GameData::archetype_bag`] gives an archetype's whole
//! bag when needed.

use rf_formats::formats::databag::{DataBag, Schema};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::FileRef;

use crate::files::{Doc, bags, string, string_hash};

/// An archetype's `assetType` (`RDR2ArchetypeAssetType`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct AssetType(pub i32);

impl AssetType {
    pub const UNINITIALIZED: Self = Self(0);
    pub const FRAGMENT: Self = Self(1);
    pub const DRAWABLE: Self = Self(2);
    pub const DRAWABLE_DICTIONARY: Self = Self(3);
    pub const ASSETLESS: Self = Self(4);
}

/// An archetype definition from a ytyp (`CBaseArchetypeDef`, `CTimeArchetypeDef`,
/// `CMloArchetypeDef`...): its common fields, and where its whole bag is.
#[derive(Clone, Debug, PartialEq)]
pub struct Archetype {
    pub name: u32,
    /// The class name hash (`CBaseArchetypeDef`...).
    pub class: u32,
    pub lod_dist: f32,
    pub flags: u32,
    pub special_attribute: u32,
    pub bb_min: [f32; 3],
    pub bb_max: [f32; 3],
    pub bs_centre: [f32; 3],
    pub bs_radius: f32,
    pub hd_texture_dist: f32,
    pub texture_dictionary: u32,
    pub clip_dictionary: u32,
    pub drawable_dictionary: u32,
    pub physics_dictionary: u32,
    pub asset_type: AssetType,
    pub asset_name: u32,
    /// The ytyp it's from, and its index in the ytyp's `archetypes`.
    pub ytyp: FileRef,
    pub index: u32,
}

impl Archetype {
    fn read(b: &DataBag, ytyp: FileRef, index: u32) -> Self {
        Self {
            name: b.hash("name"),
            class: b.class_name(),
            lod_dist: b.f32("lodDist"),
            flags: b.u32("flags"),
            special_attribute: b.u32("specialAttribute"),
            bb_min: b.vec3("bbMin"),
            bb_max: b.vec3("bbMax"),
            bs_centre: b.vec3("bsCentre"),
            bs_radius: b.f32("bsRadius"),
            hd_texture_dist: b.f32("hdTextureDist"),
            texture_dictionary: b.hash("textureDictionary"),
            clip_dictionary: b.hash("clipDictionary"),
            drawable_dictionary: b.hash("drawableDictionary"),
            physics_dictionary: b.hash("physicsDictionary"),
            asset_type: AssetType(b.i32("assetType")),
            asset_name: b.hash("assetName"),
            ytyp,
            index,
        }
    }
}

/// `LoadArchetypes`: a ytyp's archetypes, in file order.
pub fn ytyp_archetypes(doc: &Doc, ytyp: FileRef) -> Vec<Archetype> {
    let mut out = Vec::new();
    // "archetypes"; the index counts items that couldn't be read too.
    for (i, a) in doc.bag.bags(0x018A_3B1B).into_iter().flatten().enumerate() {
        if let Some(a) = a {
            out.push(Archetype::read(a, ytyp, i as u32));
        }
    }
    out
}

/// `CAssetOwnerEntry`, from a manifest's `assetOwnerMap`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AssetOwnerEntry {
    pub owner: u32,
    pub owner_hd: u32,
    pub index1: u16,
    pub index2: u16,
}

/// A manifest's `CMapDataGroup` (`RDR2MapDataGroup`'s fields).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapDataGroup {
    pub name: u32,
    /// `None` when the group has no bounds array.
    pub bounds: Option<Vec<u32>>,
    pub flags: u32,
    pub weather_types: Option<Vec<u32>>,
    pub hours_on_off: u32,
}

/// A manifest's interior: its archetype name and its bounds files.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Interior {
    pub name: u32,
    pub bounds: Option<Vec<u32>>,
}

/// What a `_manifest.ymf` holds, in file order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Manifest {
    /// (child, parent) texture dictionaries.
    pub txd_relationships: Vec<(u32, u32)>,
    /// (asset type, target asset, HD texture dictionary).
    pub hd_txd_bindings: Vec<(i32, u32, u32)>,
    pub map_data_groups: Vec<MapDataGroup>,
    pub asset_owners: Vec<(u32, AssetOwnerEntry)>,
    pub interiors: Vec<Interior>,
}

/// `LoadTxdRelationships`: (child, parent) hashes of the lowercase names.
pub fn txd_relationships(bag: &DataBag, schema: Option<&Schema>, hashes: &HashIndex) -> Vec<(u32, u32)> {
    bags(bag, 0x069A_DF93) // "txdRelationships"
        .filter_map(|r| {
            let parent = string_hash(r, 0xA99C_4651, schema, hashes)?; // "parent"
            let child = string_hash(r, 0x033E_C6B5, schema, hashes)?; // "child"
            Some((child, parent))
        })
        .collect()
}

impl Manifest {
    /// `EnableYmfFile`'s loads.
    pub fn read(doc: &Doc, hashes: &HashIndex) -> Self {
        let (b, schema) = (&doc.bag, doc.schema.as_ref());
        let hd_txd_bindings = bags(b, 0xF78A_FB23) // "HDTxdBindingArray"
            .filter_map(|h| {
                let asset_type = h.i32(0x0690_04B2); // "assetType"
                let target = string_hash(h, 0x277D_CAE0, schema, hashes)?; // "targetAsset"
                let hd = string_hash(h, 0x64DD_3030, schema, hashes)?; // "HDTxd"
                Some((asset_type, target, hd))
            })
            .collect();
        let map_data_groups = bags(b, 0xB52C_AE23) // "MapDataGroups"
            .map(|g| MapDataGroup {
                name: g.hash(0xACEC_22BE),                          // "Name"
                bounds: g.hashes(0xC496_E4A8).map(<[u32]>::to_vec), // "Bounds"
                flags: g.u32(0x4B62_1302),                          // "Flags"
                weather_types: g.hashes(0x977C_7282).map(<[u32]>::to_vec),
                hours_on_off: g.u32(0xF9CA_C411),
            })
            .collect();
        let asset_owners = bags(b, 0x7F09_1F5A) // "assetOwnerMap"
            .filter_map(|kv| {
                let key = kv.hash(0x6098_A50E); // "Key"
                let item = kv.bag(0x063F_A3F2)?; // "Item"
                (key != 0).then(|| {
                    (
                        key,
                        AssetOwnerEntry {
                            owner: item.hash(0x8C06_0170),    // "owner"
                            owner_hd: item.hash(0xB328_AAEC), // "ownerHd"
                            index1: item.u16(0x9F3B_F7B7),    // "index1"
                            index2: item.u16(0x2FD1_18D3),    // "index2"
                        },
                    )
                })
            })
            .collect();
        let interiors = bags(b, 0x3876_7A8F) // "Interiors"
            .map(|i| Interior { name: i.hash(0xACEC_22BE), bounds: i.hashes(0xC496_E4A8).map(<[u32]>::to_vec) })
            .collect();
        Self {
            txd_relationships: txd_relationships(b, schema, hashes),
            hd_txd_bindings,
            map_data_groups,
            asset_owners,
            interiors,
        }
    }
}

/// A `_cache.ych` static bounds entry (`RDR2BoundsStoreItem`'s fields).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BoundsCacheEntry {
    pub hash_key: u32,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub asset_type: u8,
}

/// A `_cache.ych` map data entry (what `RDR2MapNode` reads).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MapDataCacheEntry {
    pub name_hash: u32,
    pub parent_name_hash: u32,
    pub content_flags: u32,
    pub streaming_min: [f32; 3],
    pub streaming_max: [f32; 3],
    pub physics_min: [f32; 3],
    pub physics_max: [f32; 3],
    pub dynamic_streaming: bool,
    pub is_parent: bool,
}

/// A `_cache.ych` interior proxy entry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InteriorProxy {
    /// The interior's ytyp/ybn name.
    pub name: u32,
    /// The ymap.
    pub map_data: u32,
    pub interior_name: Option<String>,
}

/// What a `_cache.ych` holds, in file order (`LoadCaches`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CacheFile {
    pub bounds: Vec<BoundsCacheEntry>,
    pub map_data: Vec<MapDataCacheEntry>,
    pub interior_proxies: Vec<InteriorProxy>,
}

impl CacheFile {
    pub fn read(doc: &Doc, hashes: &HashIndex) -> Self {
        let (b, schema) = (&doc.bag, doc.schema.as_ref());
        // "cacheEntries" of each cache.
        let entries = |cache: u32| b.bag(cache).into_iter().flat_map(|c| bags(c, 0x16CB_4432));
        let xyz = |e: &DataBag, h: [u32; 3]| h.map(|h| e.f32(h));
        Self {
            bounds: entries(0xBBE9_E016) // "staticBoundsCache"
                .map(|e| BoundsCacheEntry {
                    hash_key: e.hash(0xA6CE_B34F),
                    min: xyz(e, [0x245D_6D2B, 0xF405_8C7C, 0x0210_2891]),
                    max: xyz(e, [0xB76E_C073, 0xCA34_65FE, 0x9BD7_0944]),
                    asset_type: e.u8(0x0690_04B2),
                })
                .collect(),
            map_data: entries(0xBFBF_83D9) // "mapDataCache"
                .map(|e| MapDataCacheEntry {
                    name_hash: e.hash(0x97CF_ED26),
                    parent_name_hash: e.hash(0xADF2_C538),
                    content_flags: e.u32(0x6A67_5035),
                    streaming_min: xyz(e, [0x4954_ED3C, 0x3306_C0A0, 0xE4D1_A437]),
                    streaming_max: xyz(e, [0xA758_BF75, 0x8992_83E9, 0x79DD_E47C]),
                    physics_min: xyz(e, [0x4A6B_5A4B, 0x2E2C_21CD, 0x1FFE_8572]),
                    physics_max: xyz(e, [0x78C1_CB28, 0x6F0C_B7BE, 0x5D46_1431]),
                    dynamic_streaming: e.bool(0xA095_9197),
                    is_parent: e.bool(0x8845_63DD),
                })
                .collect(),
            interior_proxies: entries(0xF919_9DAE) // "interiorProxyCache"
                .map(|e| InteriorProxy {
                    name: e.hash(0x4E29_2F18),
                    map_data: e.hash(0x314F_87F8),
                    interior_name: string(e, 0x2EFA_0869, schema, hashes),
                })
                .collect(),
        }
    }
}
