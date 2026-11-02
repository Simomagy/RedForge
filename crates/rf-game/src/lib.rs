//! RDR2's game data layer: what the game reads at startup to know where its
//! files are. A port of the C# RedForge's `Rpf8DataFileMgr` (in
//! `Rpf8FileManager.cs`).
//!
//! [`GameData::load`] mounts the archives on their devices (`common:/`,
//! `platform:/`, `update:/`, the DLCs'), reads the DLC list and each pack's setup
//! and content files, enables the data files of the level and of the startup
//! change set groups, and from the enabled archives builds:
//! - the **stream entries**: the file the game streams for each name hash, per
//!   extension (later archives override earlier ones);
//! - the **archetypes** of every streamed ytyp;
//! - from the `_manifest.ymf` of each archive and the `gtxd.ymt` files: the
//!   **GTXD hierarchy** (a texture dictionary's parent), the **HD texture
//!   bindings** (`+hi` and `+hidr` dictionaries), map data groups, asset owners
//!   and interiors;
//! - from the `_cache.ych` files: the bounds, map data and interior proxy caches.
//!
//! The texture dictionaries a model needs come from these: an archetype names
//! its `textureDictionary`, which has parents in the GTXD hierarchy, and HD
//! versions of either ([`GameData::txds_for_archetype`],
//! [`GameData::txds_for_model`]).
//!
//! ```ignore
//! let fs = Rpf8FileSystem::open(game_dir, keys, &strings)?;
//! let gd = GameData::load(&fs, &strings);
//! for ytd in gd.txds_for_model("val_01_aucbarns_barn2.ydr") {
//!     println!("{}", fs.path(ytd)); // ...+hidr.ytd, val_01_+hi.ytd, val_01_.ytd, valentine+hi.ytd...
//! }
//! ```
//!
//! Loading reads about 6000 metadata files (70 MB) in parallel: about a second
//! (two on one thread), less than opening the archives takes. So unlike the C#
//! there is no startup cache. [`csharp`] reads the C#'s cache, to check this
//! port against it: everything in it matches.
//!
//! Known issues and differences from the C#, none of which matter for the game
//! as installed:
//! - Where the C# would throw (a missing setup field, a null dictionary key),
//!   the item is skipped instead; where it would recurse forever (device
//!   redirections that loop, a GTXD parent loop), the lookup gives up.
//! - An archive whose name the strings don't know is mounted under the hash of
//!   `0x<HASH>.rpf` instead of the C#'s made-up collision name; nothing can look
//!   up either. The names the C# adds to its string index on the way (DLC
//!   folders, `<name>.rpf`) aren't added.
//! - Data bags are kept as plain structs of the fields the C# reads
//!   ([`Archetype`], [`MapDataGroup`]...), not whole bags;
//!   [`GameData::archetype_bag`] reads an archetype's bag from its ytyp.
//! - Init's own state (packs, change sets, data files, overlay infos, levels) is
//!   not kept, except the enabled data files and archives.
//! - The project-file hooks (`AddStreamEntry`, `RemoveStreamEntry`) aren't
//!   ported yet: they belong with the projects.

pub mod content;
pub mod csharp;
mod device;
mod files;
mod init;
mod manifest;
pub mod timecycle;

use std::collections::HashMap;

use indexmap::IndexMap;
use rf_formats::formats::databag::DataBag;
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{FileRef, Rpf8FileSystem};

pub use device::{Device, Devices, MapEntry, split_path};
pub use files::{Doc, ext_id, file_ext, file_hash, load_doc, name_hash};
pub use init::sanitize;
pub use manifest::{
    Archetype, AssetOwnerEntry, AssetType, BoundsCacheEntry, CacheFile, Interior, InteriorProxy, Manifest,
    MapDataCacheEntry, MapDataGroup,
};

/// The number of stream entry tables (`Rpf8FileEntry.BaseExts` and `ExtraExts`).
pub const STREAM_TABLES: usize = 14 + 24;

/// `GetStreamEntriesIndex`: the stream entry table of an `Rpf8FileExt` id (ids
/// 64 and up follow the 14 base ones). Ids past the tables have none.
pub fn stream_index(ext_id: u8) -> usize {
    if ext_id >= 64 { ext_id as usize - 64 + 14 } else { ext_id as usize }
}

/// Everything Init builds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameData {
    /// The devices by name, for [`GameData::map_lookup`].
    pub devices: Devices,
    /// DLC name → its extra paths from the title update (`ExtraMap`).
    pub extra_map: HashMap<String, Vec<String>>,
    /// The enabled archives, in the order they were enabled (indices into
    /// [`Rpf8FileSystem::archives`]; one can be enabled twice).
    pub enabled_rpfs: Vec<u32>,
    /// Data file type → the enabled files of that type (`EnabledDataFiles`).
    pub enabled_data_files: IndexMap<String, Vec<FileRef>>,
    /// Per [`stream_index`]: name hash → file. In the C#'s order: where a hash
    /// first appeared, with the file that last replaced it.
    pub stream_entries: Vec<IndexMap<u32, FileRef>>,
    /// Name hash → archetype (`AllArchetypes`).
    pub archetypes: HashMap<u32, Archetype>,
    /// Texture dictionary → its parent.
    pub gtxd_hierarchy: HashMap<u32, u32>,
    /// HD texture dictionaries by asset type (`AT_TXD`, `AT_DRB`, `AT_DWD`,
    /// `AT_FRG`): asset → its HD texture dictionary.
    pub hd_txd_bindings: [HashMap<u32, u32>; 4],
    pub asset_owner_map: HashMap<u32, AssetOwnerEntry>,
    /// Map data groups by name (`MapDataGroupManifest`).
    pub map_data_groups: HashMap<u32, MapDataGroup>,
    /// Interiors by archetype name (`InteriorManifest`).
    pub interiors: HashMap<u32, Interior>,
    /// Bounds file → its interior.
    pub interior_bounds_lookup: HashMap<u32, u32>,
    /// Static bounds by ybn name (`BoundsCache`).
    pub bounds_cache: HashMap<u32, BoundsCacheEntry>,
    /// Map data by ymap name.
    pub map_data_cache: HashMap<u32, MapDataCacheEntry>,
    /// Interior proxies by ymap name.
    pub interior_proxy_cache: HashMap<u32, InteriorProxy>,
    /// The files read, in the C#'s order (the keys of its `AllYtypsData`,
    /// `AllYchsData` and `AllYmfsData`).
    pub ytyp_files: Vec<FileRef>,
    pub ych_files: Vec<FileRef>,
    pub ymf_files: Vec<FileRef>,
}

impl GameData {
    /// Runs the game's startup (`Rpf8DataFileMgr.Init`) on the archives: about a
    /// second, the files being read in parallel.
    pub fn load(fs: &Rpf8FileSystem, hashes: &HashIndex) -> GameData {
        init::load(fs, hashes)
    }

    /// The game's data with a project's files ([mounted](Rpf8FileSystem::mount)):
    /// each replaces the game's stream entry of its name and type (the C#'s
    /// `AddStreamEntry` for project files), and a ytyp's archetypes are added
    /// (replacing the game's of the same name), so new props can be placed.
    pub fn with_project(&self, fs: &Rpf8FileSystem, hashes: &HashIndex, files: &[FileRef]) -> GameData {
        let mut gd = self.clone();
        for &f in files {
            let e = fs.entry(f);
            if let Some(table) = gd.stream_entries.get_mut(stream_index(e.ext_id)) {
                table.insert(e.name_hash, f);
            }
            if e.ext_id == init::YTYP
                && let Some(doc) = load_doc(fs, hashes, f)
            {
                for a in manifest::ytyp_archetypes(&doc, f) {
                    gd.archetypes.insert(a.name, a);
                }
                gd.ytyp_files.push(f);
            }
        }
        gd
    }

    // -----------------------------------------------------------------------
    // Stream entries

    /// `TryGetStreamEntry`: the file streamed for a name hash with an extension
    /// (`"ytd"`, `"ydr"`...).
    pub fn stream_entry(&self, ext: &str, hash: u32) -> Option<FileRef> {
        self.stream_entry_by_id(ext_id(ext)?, hash)
    }

    /// The same, by `Rpf8FileExt` id.
    pub fn stream_entry_by_id(&self, ext_id: u8, hash: u32) -> Option<FileRef> {
        self.stream_entries.get(stream_index(ext_id))?.get(&hash).copied()
    }

    /// The stream entries of an extension, in order.
    pub fn stream_entries(&self, ext: &str) -> impl Iterator<Item = (u32, FileRef)> + '_ {
        ext_id(ext)
            .and_then(|id| self.stream_entries.get(stream_index(id)))
            .into_iter()
            .flat_map(|t| t.iter().map(|(h, f)| (*h, *f)))
    }

    // -----------------------------------------------------------------------
    // Texture dictionaries

    /// `GetTxds`: the hashes of an asset's texture dictionaries, best first: the
    /// asset's HD dictionary, then the HD version and the dictionary itself for
    /// the texture dictionary and each of its GTXD parents.
    pub fn txd_hashes(&self, asset_type: AssetType, asset_name: u32, texture_dictionary: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let hd_txd = &self.hd_txd_bindings[0];
        let hd_asset = match asset_type {
            AssetType::DRAWABLE => Some(&self.hd_txd_bindings[1]),
            AssetType::DRAWABLE_DICTIONARY => Some(&self.hd_txd_bindings[2]),
            AssetType::FRAGMENT => Some(&self.hd_txd_bindings[3]),
            _ => None,
        };
        if let Some(h) = hd_asset.and_then(|m| m.get(&asset_name)) {
            out.push(*h);
        }
        let mut txd = texture_dictionary;
        if txd == 0 {
            return out;
        }
        if let Some(h) = hd_txd.get(&txd) {
            out.push(*h);
        }
        out.push(txd);
        // The parents; a loop would hang the C#.
        let mut seen = vec![txd];
        while let Some(&parent) = self.gtxd_hierarchy.get(&txd) {
            if txd == 0 || seen.contains(&parent) {
                break;
            }
            if let Some(h) = hd_txd.get(&parent) {
                out.push(*h);
            }
            out.push(parent);
            seen.push(parent);
            txd = parent;
        }
        out
    }

    /// [`GameData::txd_hashes`] as the ytd files streamed for them.
    pub fn txds(&self, asset_type: AssetType, asset_name: u32, texture_dictionary: u32) -> Vec<FileRef> {
        let mut out = Vec::new();
        for h in self.txd_hashes(asset_type, asset_name, texture_dictionary) {
            if let Some(f) = self.stream_entry_by_id(YTD, h)
                && !out.contains(&f)
            {
                out.push(f);
            }
        }
        out
    }

    /// `GetArchetypeTxds`: the texture dictionaries of an archetype, best first.
    /// `None` when there's no such archetype.
    pub fn txds_for_archetype(&self, archetype: u32) -> Option<Vec<FileRef>> {
        let a = self.archetypes.get(&archetype)?;
        Some(self.txds(a.asset_type, a.asset_name, a.texture_dictionary))
    }

    /// `LoadDependencies` for one piece of a model (a ydr or yft's drawable, or a
    /// drawable of a ydd): the dictionaries of the archetype named like the
    /// piece, or else of the one named like the file.
    pub fn txds_for_piece(&self, piece: u32, file_hash: u32) -> Vec<FileRef> {
        self.txds_for_archetype(piece).or_else(|| self.txds_for_archetype(file_hash)).unwrap_or_default()
    }

    /// The texture dictionaries of a ydr or yft, by its file name (or path):
    /// [`GameData::txds_for_piece`] with the hash of its name. A name like
    /// `0x1234ABCD.ydr` stands for that hash. For a ydd, ask per drawable.
    pub fn txds_for_model(&self, file_name: &str) -> Vec<FileRef> {
        let h = name_hash(files::stem(file_name));
        self.txds_for_piece(h, h)
    }

    // -----------------------------------------------------------------------
    // Devices and archetype bags

    /// `MapLookup`: what a device path (`common:/data/dlclist.xml`) leads to.
    pub fn map_lookup(&self, fs: &Rpf8FileSystem, path: &str) -> Option<MapEntry> {
        self.devices.lookup(fs, path)
    }

    /// `MetaLoad`: a metadata file by device path.
    pub fn meta_load(&self, fs: &Rpf8FileSystem, hashes: &HashIndex, path: &str) -> Option<Doc> {
        load_doc(fs, hashes, self.map_lookup(fs, path)?.file()?)
    }

    /// An archetype's whole definition, read from its ytyp.
    pub fn archetype_bag(&self, fs: &Rpf8FileSystem, hashes: &HashIndex, archetype: u32) -> Option<DataBag> {
        let a = self.archetypes.get(&archetype)?;
        let doc = load_doc(fs, hashes, a.ytyp)?;
        doc.bag.bags(0x018A_3B1B)?.get(a.index as usize)?.clone() // "archetypes"
    }

    // -----------------------------------------------------------------------
    // Applying what the files hold

    /// `EnableYmfFile`'s loads, in the C#'s order.
    fn apply_manifest(&mut self, m: &Manifest) {
        self.gtxd_hierarchy.extend(m.txd_relationships.iter().copied());
        for &(t, target, hd) in &m.hd_txd_bindings {
            if let Some(b) = usize::try_from(t).ok().and_then(|t| self.hd_txd_bindings.get_mut(t)) {
                b.insert(target, hd);
            }
        }
        for g in &m.map_data_groups {
            // A group with bounds isn't replaced by one without.
            let keep = self.map_data_groups.get(&g.name).is_some_and(|ex| g.bounds.is_none() && ex.bounds.is_some());
            if !keep {
                self.map_data_groups.insert(g.name, g.clone());
            }
        }
        self.asset_owner_map.extend(m.asset_owners.iter().copied());
        for i in &m.interiors {
            for &b in i.bounds.iter().flatten() {
                self.interior_bounds_lookup.insert(b, i.name);
            }
            self.interiors.insert(i.name, i.clone());
        }
    }

    /// `LoadCaches`.
    fn apply_cache(&mut self, c: &CacheFile) {
        self.bounds_cache.extend(c.bounds.iter().map(|b| (b.hash_key, *b)));
        self.map_data_cache.extend(c.map_data.iter().map(|m| (m.name_hash, *m)));
        self.interior_proxy_cache.extend(c.interior_proxies.iter().map(|p| (p.map_data, p.clone())));
    }
}

const YTD: u8 = 5;

#[cfg(test)]
mod tests {
    use rf_formats::hash::joaat;

    use super::*;

    fn f(entry: u32) -> FileRef {
        FileRef { archive: 0, entry }
    }

    /// An archetype `prop` with texture dictionary `child`, whose parents are
    /// `parent` then `grandparent`, with HD dictionaries for the drawable and two
    /// of the dictionaries, and every dictionary streamed.
    fn sample() -> GameData {
        let h = joaat;
        let mut gd = GameData { stream_entries: vec![IndexMap::new(); STREAM_TABLES], ..Default::default() };
        gd.archetypes.insert(
            h("prop"),
            Archetype {
                name: h("prop"),
                class: h("CBaseArchetypeDef"),
                lod_dist: 100.0,
                flags: 0,
                special_attribute: 0,
                bb_min: [0.0; 3],
                bb_max: [0.0; 3],
                bs_centre: [0.0; 3],
                bs_radius: 0.0,
                hd_texture_dist: 0.0,
                texture_dictionary: h("child"),
                clip_dictionary: 0,
                drawable_dictionary: 0,
                physics_dictionary: 0,
                asset_type: AssetType::DRAWABLE,
                asset_name: h("prop"),
                time_flags: 0,
                ytyp: f(0),
                index: 0,
            },
        );
        gd.gtxd_hierarchy.extend([(h("child"), h("parent")), (h("parent"), h("grandparent"))]);
        gd.hd_txd_bindings[0].extend([(h("child"), h("child+hi")), (h("grandparent"), h("grandparent+hi"))]);
        gd.hd_txd_bindings[1].insert(h("prop"), h("prop+hidr"));
        let ytds = &mut gd.stream_entries[stream_index(YTD)];
        for (i, n) in ["prop+hidr", "child+hi", "child", "parent", "grandparent+hi", "grandparent"].iter().enumerate() {
            ytds.insert(h(n), f(i as u32));
        }
        gd
    }

    #[test]
    fn texture_dictionaries_best_first() {
        let gd = sample();
        let names = ["prop+hidr", "child+hi", "child", "parent", "grandparent+hi", "grandparent"];
        let want: Vec<u32> = names.iter().map(|n| joaat(n)).collect();
        assert_eq!(gd.txd_hashes(AssetType::DRAWABLE, joaat("prop"), joaat("child")), want);
        // A fragment doesn't use the drawable HD bindings.
        assert_eq!(gd.txd_hashes(AssetType::FRAGMENT, joaat("prop"), joaat("child")), want[1..]);
        assert_eq!(gd.txds_for_archetype(joaat("prop")), Some((0..6).map(f).collect()));
        assert_eq!(gd.txds_for_archetype(joaat("nothing")), None);
        assert_eq!(gd.txds_for_model("levels_0.rpf/x/Prop.ydr"), (0..6).map(f).collect::<Vec<_>>());
        // A piece without an archetype falls back to the file's.
        assert_eq!(gd.txds_for_piece(joaat("other"), joaat("prop")).len(), 6);
        assert!(gd.txds_for_piece(joaat("other"), joaat("none")).is_empty());
        assert_eq!(gd.stream_entry("ytd", joaat("parent")), Some(f(3)));
        assert_eq!(gd.stream_entry("ydr", joaat("parent")), None);
        assert_eq!(gd.stream_entries("ytd").count(), 6);
    }

    #[test]
    fn gtxd_loops_end() {
        let mut gd = sample();
        gd.gtxd_hierarchy.insert(joaat("grandparent"), joaat("child"));
        assert_eq!(gd.txd_hashes(AssetType::ASSETLESS, 0, joaat("child")).len(), 5);
    }

    #[test]
    fn stream_tables() {
        assert_eq!(stream_index(5), 5);
        assert_eq!(stream_index(64), 14);
        assert_eq!(stream_index(87), STREAM_TABLES - 1);
        assert_eq!(stream_index(ext_id("ytyp").unwrap()), 24);
    }
}
