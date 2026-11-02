//! `Rpf8DataFileMgr.Init`: mounts the archives on their devices, reads the DLC
//! list, the packs' setup and content files and the level's data files, enables
//! the data files of the startup change set groups, and from the enabled
//! archives fills the stream entries and loads the manifests, caches, GTXD
//! files and ytyps.
//!
//! The order of everything is the C#'s, as later files override earlier ones.
//! The only change: the manifests, caches, GTXD files and ytyps are read in
//! parallel after the rest, then applied in the order the C# loads them.

use std::collections::HashMap;
use std::time::Instant;

use indexmap::IndexMap;
use rayon::prelude::*;
use rf_formats::dotnet::dotnet_sort;
use rf_formats::hash::{HashIndex, joaat};
use rf_formats::rpf8::{FileRef, Rpf8FileSystem};

use crate::content::{Content, DataFile, DataPack, LevelData, OverlayInfo};
use crate::device::{Devices, MapEntry, split_path};
use crate::files::{Doc, bags, extension, load_doc, string};
use crate::manifest::{CacheFile, Manifest, txd_relationships, ytyp_archetypes};
use crate::{GameData, STREAM_TABLES, stream_index};

const MANIFEST_HASH: u32 = 0xAD68_0BC3; // "_manifest"
const CACHE_HASH: u32 = 0xD50E_BAA1; // "_cache"
const GTXD_HASH: u32 = 0x8CE3_2B76; // "gtxd"

/// The change set groups Init activates, in order.
const STARTUP_GROUPS: [&str; 12] = [
    "GROUP_STARTUP",
    "GROUP_EARLY_ON",
    "INIT_LEVEL_WITH_MAP_UNLOADED_PRE",
    "INIT_LEVEL_WITH_MAP_UNLOADED",
    "GROUP_UPDATE_SCRIPT",
    "GROUP_UPDATE_TEXT",
    "GROUP_UPDATE_STREAMING",
    "GROUP_UPDATE_WEAPON_PATCH",
    "GROUP_MAP",
    "GROUP_MAP_SP",
    "INIT_SESSION",
    "GROUP_ON_DEMAND",
];

/// A file the C# loads while enabling an archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Load {
    Manifest(FileRef),
    Cache(FileRef),
    Gtxd(FileRef),
}

struct Builder<'a> {
    fs: &'a Rpf8FileSystem,
    hashes: &'a HashIndex,
    /// Lowercase archive path → archive (`ArchiveDict`).
    archive_by_path: HashMap<String, u32>,
    /// The archive an `.rpf` entry opens as.
    child_archive: HashMap<FileRef, u32>,
    devices: Devices,
    /// DLC name → extra paths (`ExtraMap`).
    extra_map: HashMap<String, Vec<String>>,
    content: Content,
    enabled_data_files: IndexMap<String, Vec<FileRef>>,
    enabled_rpfs: Vec<u32>,
    stream_entries: Vec<IndexMap<u32, FileRef>>,
    loads: Vec<Load>,
}

/// Runs Init and builds the game data.
pub fn load(fs: &Rpf8FileSystem, hashes: &HashIndex) -> GameData {
    let t = Instant::now();
    let mut b = Builder::new(fs, hashes);
    b.init();
    log::info!(
        "game data: {} archives enabled, {} files to load ({:.2?})",
        b.enabled_rpfs.len(),
        b.loads.len(),
        t.elapsed()
    );
    b.finish()
}

impl<'a> Builder<'a> {
    fn new(fs: &'a Rpf8FileSystem, hashes: &'a HashIndex) -> Self {
        let mut archive_by_path = HashMap::with_capacity(fs.archives.len());
        let mut child_archive = HashMap::new();
        for (i, a) in fs.archives.iter().enumerate() {
            archive_by_path.insert(a.path.clone(), i as u32);
            if let Some((pa, pe)) = a.parent {
                child_archive.insert(FileRef { archive: pa as u32, entry: pe as u32 }, i as u32);
            }
        }
        Self {
            fs,
            hashes,
            archive_by_path,
            child_archive,
            devices: Devices::default(),
            extra_map: HashMap::new(),
            content: Content::default(),
            enabled_data_files: IndexMap::new(),
            enabled_rpfs: Vec::new(),
            stream_entries: vec![IndexMap::new(); STREAM_TABLES],
            loads: Vec::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Devices

    /// `Rpf8DataFileDevice.MountRpfEntries`: every file of an archive, by hash.
    fn mount_rpf_entries(&mut self, device: &str, path: &str) {
        if path.is_empty() {
            return;
        }
        let Some(&ai) = self.archive_by_path.get(&path.replace('\\', "/").to_lowercase()) else { return };
        let archive = &self.fs.archives[ai as usize];
        let mut entries = Vec::with_capacity(archive.entries.len());
        for (ei, e) in archive.entries.iter().enumerate() {
            let f = FileRef { archive: ai, entry: ei as u32 };
            let mut hash = e.name_hash;
            let mut entry = MapEntry::File(f);
            if archive.names[ei].to_lowercase().ends_with(".rpf") {
                if let Some(&c) = self.child_archive.get(&f) {
                    entry = MapEntry::Archive(c);
                }
                // The C# keys archives by their name with ".rpf"; a name the
                // strings don't know gives a key nothing looks up.
                let name = self.hashes.get(hash).unwrap_or_else(|| format!("0x{hash:08X}"));
                hash = joaat(&format!("{name}.rpf"));
            }
            entries.push((hash, entry));
        }
        self.devices.get_mut(device).main_map.extend(entries);
    }

    /// `ReMountEntries`: the archive a path leads to, or else the path as the
    /// device's first redirection.
    fn remount_entries(&mut self, device: &str, path: &str) {
        match self.devices.lookup(self.fs, path) {
            Some(MapEntry::Archive(a)) => {
                let apath = self.fs.archives[a as usize].path.clone();
                self.mount_rpf_entries(device, &apath);
            }
            _ => self.devices.get_mut(device).extra_map.insert(0, format!("{path}/")),
        }
    }

    fn map_lookup(&self, path: &str) -> Option<MapEntry> {
        self.devices.lookup(self.fs, path)
    }

    /// `MetaLoad`.
    fn meta_load(&self, path: &str) -> Option<Doc> {
        let f = self.map_lookup(path)?.file()?;
        load_doc(self.fs, self.hashes, f)
    }

    /// `MetaDlcLoad`: from the DLC's extra paths, last first, then its own path.
    fn meta_dlc_load(&self, dlc_name: &str, dlc_path: &str, name: &str) -> Option<Doc> {
        if let Some(extra) = self.extra_map.get(dlc_name) {
            for path in extra.iter().rev() {
                if let Some(doc) = self.meta_load(&format!("{path}{name}")) {
                    return Some(doc);
                }
            }
        }
        self.meta_load(&format!("{dlc_path}{name}"))
    }

    fn string(&self, doc: &Doc, b: &rf_formats::formats::databag::DataBag, field: u32) -> Option<String> {
        string(b, field, doc.schema.as_ref(), self.hashes)
    }

    // -----------------------------------------------------------------------
    // Init

    fn init(&mut self) {
        let fs = self.fs;
        let Some(manifest) =
            fs.find("appdata0_update.rpf/mountmanifest_tu.xml").and_then(|f| load_doc(fs, self.hashes, f))
        else {
            log::warn!("game data: no mount manifest");
            return;
        };
        if manifest.bag.bags(0x7019_9276).is_none() {
            return; // "Mounts"
        }
        for m in bags(&manifest.bag, 0x7019_9276) {
            let is_crc = m.bool(0x4091_AE43); // "IsCrcMount"
            let path = self.string(&manifest, m, 0x5B18_3DDC); // "Path"
            let mount_point = self.string(&manifest, m, 0xDC79_CE51); // "MountPoint"
            if let (Some(path), Some(mount_point)) = (path, mount_point) {
                self.mount_rpf_entries(&mount_point, &path);
                if is_crc {
                    self.mount_rpf_entries(&format!("{mount_point}crc"), &path);
                }
            }
        }

        // The DLC archives on the dlcpacks device.
        let mut dlcpacks = Vec::new();
        for (i, a) in fs.archives.iter().enumerate() {
            if a.parent.is_none()
                && let Some(rest) = a.path.strip_prefix("x64/dlcpacks/")
            {
                dlcpacks.push((joaat(rest), MapEntry::Archive(i as u32)));
            }
        }
        self.devices.get_mut("dlcpacks").main_map.extend(dlcpacks);

        for (device, path) in [
            ("common", "update_common:/common/"),
            ("commoncrc", "update_common:/common/"),
            ("platform", "update_platform:/x64/"),
            ("platformcrc", "update_platform:/x64/"),
        ] {
            self.devices.get_mut(device).extra_map.push(path.to_owned());
        }

        // The title update's patches (SExtraTitleUpdateData).
        let Some(extratu) = self.meta_load("common:/data/extratitleupdatedata.meta") else {
            log::warn!("game data: no extratitleupdatedata.meta");
            return;
        };
        if extratu.bag.bags(0x7019_9276).is_none() {
            return; // "Mounts"
        }
        for m in bags(&extratu.bag, 0x7019_9276) {
            let Some(dlc_name) = self.string(&extratu, m, 0x8B56_6EA2) else { continue }; // "dlcName"
            for mapping in bags(m, 0x972E_E0A3) {
                // "mappings"
                let device_type = self.string(&extratu, mapping, 0xF525_A1CB); // "DeviceType"
                let Some(path) = self.string(&extratu, mapping, 0xA63E_D6CB) else { continue }; // "PathToMap"
                match device_type.as_deref() {
                    Some("DM_DLCROOT") => self.extra_map.entry(dlc_name.to_lowercase()).or_default().push(path),
                    Some("DM_COMMON") => self.devices.get_mut("common").extra_map.push(path),
                    Some("DM_PLATFORM") => {
                        self.devices.get_mut("platform").extra_map.push(path.replace("%PLATFORM%", "x64"))
                    }
                    _ => {}
                }
            }
        }

        // The DLC list (SMandatoryPacksData).
        let Some(dlclist) = self.meta_load("common:/data/dlclist.xml") else {
            log::warn!("game data: no dlclist.xml");
            return;
        };
        if dlclist.bag.bags(0x8BC9_0C94).is_none() {
            return; // "Packs"
        }
        let mut packs = Vec::new();
        for p in bags(&dlclist.bag, 0x8BC9_0C94) {
            let Some(path) = self.string(&dlclist, p, 0x5B18_3DDC) else { continue }; // "Path"
            let ptype = self.string(&dlclist, p, 0xEF36_65C9); // "Type"
            let noslash = path.strip_suffix('/').unwrap_or(&path);
            let dlc_path = format!("{noslash}/");
            let mut dlc_name = "base".to_owned();
            if ptype.as_deref() != Some("PT_BASEPKG")
                && let Some(i) = noslash.rfind('/')
            {
                let n = &noslash[..i];
                dlc_name = n[n.rfind('/').map_or(0, |j| j + 1)..].to_lowercase();
            }
            // setup2.xml, then the content file it names.
            let Some(setup) = self.meta_dlc_load(&dlc_name, &dlc_path, "setup2.xml") else { continue };
            let mut pack = DataPack::from_setup(&dlc_name, &dlc_path, &setup.bag, setup.schema.as_ref(), self.hashes);
            let Some(dat_file) = pack.dat_file.clone() else { continue };
            let Some(content) = self.meta_dlc_load(&dlc_name, &dlc_path, &dat_file) else { continue };
            self.load_dat_file_content(&mut pack, &content);
            packs.push(pack);
        }
        // By order, then minor order. Several packs have the same, so the order
        // of .NET's unstable sort matters.
        let mut order: Vec<usize> = (0..packs.len()).collect();
        dotnet_sort(&mut order, &|&a, &b| {
            let (a, b) = (&packs[a], &packs[b]);
            a.order.cmp(&b.order).then(a.minor_order.cmp(&b.minor_order)) as i32
        });
        let mut packs: Vec<Option<DataPack>> = packs.into_iter().map(Some).collect();
        self.content.mandatory_packs = order.into_iter().filter_map(|i| packs[i].take()).collect();

        // The level (platform:/data/levels) and its data files.
        let Some(levels) = self.meta_load("platform:/data/levels") else {
            log::warn!("game data: no levels file");
            return;
        };
        if levels.bag.bags(0x61EF_32B0).is_none() {
            return; // "aLevelsData"
        }
        for l in bags(&levels.bag, 0x61EF_32B0) {
            let level = LevelData::read(l, levels.schema.as_ref(), self.hashes);
            self.content.levels.insert(level.title.clone().unwrap_or_default(), level);
        }
        let Some(level) = self.content.levels.get("MO_JIM_L11").cloned() else {
            log::warn!("game data: no MO_JIM_L11 level");
            return;
        };
        let Some(level_meta) =
            level.filename.as_ref().and_then(|f| self.meta_load(&format!("{}.meta", f.replace('\\', "/"))))
        else {
            log::warn!("game data: no level meta file");
            return;
        };
        let level_name = level.friendly_name.clone().unwrap_or_default();
        let mut base = DataPack { name: level_name.clone(), ..Default::default() };
        self.load_dat_file_content(&mut base, &level_meta);
        for (k, v) in &base.data_files {
            self.content.data_files.insert(k.clone(), v.clone());
        }
        self.content.base_pack = Some(base);

        // Mount each DLC and gather its change sets and data files.
        let packs = std::mem::take(&mut self.content.mandatory_packs);
        for pack in &packs {
            let Some(device) = pack.device_name.as_ref().map(|d| d.to_lowercase()) else { continue };
            let path = pack.path.strip_suffix('/').unwrap_or(&pack.path);
            if let Some(extra) = pack.name_hash.as_ref().and_then(|n| self.extra_map.get(n)).cloned() {
                self.devices.get_mut(&device).extra_map.extend(extra);
            }
            self.remount_entries(&device, path);

            for (group_name, set_names) in &pack.content_change_set_groups {
                let group = self.content.change_set_groups.entry(group_name.clone()).or_default();
                for name in set_names {
                    if let Some(&cs) = pack.content_change_sets.get(name) {
                        group.change_sets.push(cs);
                        self.content.change_sets_by_name.insert(name.clone(), cs);
                    }
                }
            }
            for (k, v) in &pack.data_files {
                self.content.data_files.insert(k.clone(), v.clone());
            }
        }
        self.content.mandatory_packs = packs;

        // Enable the level's data files, then the defaults (platform:/data/default).
        let base_files: Vec<DataFile> =
            self.content.base_pack.iter().flat_map(|p| p.data_files.values().cloned()).collect();
        for df in &base_files {
            self.enable_data_file(df, false);
        }
        if let Some(default) = self.meta_load("platform:/data/default") {
            let mut pack = DataPack { name: level_name, ..Default::default() };
            self.load_dat_file_content(&mut pack, &default);
            for (k, v) in &pack.data_files {
                self.content.data_files.insert(sanitize(k), v.clone());
                self.enable_data_file(v, false);
            }
        }

        // The startup change set groups.
        for group_name in STARTUP_GROUPS {
            let Some(group) = self.content.change_set_groups.get(group_name) else { continue };
            for cs in group.all_change_sets() {
                let cs = self.content.change_sets[cs].clone();
                for file in &cs.files_to_enable {
                    self.enable_data_file_named(file, false);
                }
                for files in &cs.map_files_to_enable {
                    for file in files {
                        self.enable_data_file_named(file, true);
                    }
                }
            }
        }
    }

    /// `LoadDatFileContent`: the included files' data files, then the file's own
    /// data files and change sets.
    fn load_dat_file_content(&mut self, pack: &mut DataPack, content: &Doc) {
        for inc in bags(&content.bag, 0xC38F_6DB2) {
            // "includedXmlFiles"
            let Some(xi) = inc.bag(0x671E_D6FB) else { continue }; // "xi:include"
            let Some(href) = self.string(content, xi, 0x8E92_6F4B) else { continue }; // "href"
            let Some(file) = self.meta_load(&href.to_lowercase()) else { continue };
            for df in bags(&file.bag, 0xF83A_C9E3) {
                // "dataFiles"
                let df = DataFile::read(df, file.schema.as_ref(), self.hashes);
                if let Some(name) = df.filename.clone().filter(|n| !n.is_empty()) {
                    pack.data_files.insert(name, df);
                }
            }
        }
        self.content.read_dat_file(pack, &content.bag, content.schema.as_ref(), self.hashes);
    }

    fn enable_data_file_named(&mut self, name: &str, map_change_set: bool) {
        if let Some(df) = self.content.data_files.get(name).cloned() {
            self.enable_data_file(&df, map_change_set);
        }
    }

    /// `EnableDataFile`.
    fn enable_data_file(&mut self, df: &DataFile, _map_change_set: bool) {
        let Some(file_type) = df.file_type.as_deref() else { return };
        match file_type {
            "OVERLAY_INFO_FILE" => self.enable_overlay_info_file(df),
            "EXTRA_FOLDER_MOUNT_DATA" => self.enable_extra_folder_mount_file(df),
            "RPF_FILE" => self.enable_rpf_file(df),
            t if ENABLED_FILE_TYPES.contains(&t) => self.enable_file(df),
            _ => {}
        }
    }

    fn enable_overlay_info_file(&mut self, df: &DataFile) {
        let Some(file) = df.filename.as_ref().and_then(|f| self.meta_load(f)) else { return };
        for oi in bags(&file.bag, 0x8B26_83C8) {
            // "overlayInfos"
            let mut oi = OverlayInfo::read(oi, file.schema.as_ref(), self.hashes);
            let Some(group) = oi.change_set_group_to_execute_with.as_ref() else { continue };
            let Some(name_id) = oi.name_id.clone() else { continue };
            oi.change_set_id = oi.change_set.as_ref().and_then(|c| self.content.change_sets_by_name.get(c)).copied();
            let Some(csg) = self.content.change_set_groups.get_mut(group) else { continue };
            if csg.overlay_infos.get(&name_id).is_some_and(|ex| oi.version < ex.version) {
                continue;
            }
            csg.overlay_infos.insert(name_id, oi);
        }
    }

    fn enable_extra_folder_mount_file(&mut self, df: &DataFile) {
        let Some(file) = df.filename.as_ref().and_then(|f| self.meta_load(f)) else { return };
        for fm in bags(&file.bag, 0x0E1E_4F10) {
            // "FolderMounts"
            let Some(path) = self.string(&file, fm, 0xAD8A_7AB5) else { continue }; // "path"
            let mount_as = self.string(&file, fm, 0x0D4B_DA35).unwrap_or_default(); // "mountAs"
            let (device, _) = split_path(&mount_as);
            let mut path = sanitize(&path);
            if !path.ends_with('/') {
                path.push('/');
            }
            self.devices.get_mut(&device).extra_map.push(path);
        }
    }

    /// `EnableRpfFile`: every file of the archive becomes a stream entry; its
    /// manifest, cache and GTXD file get loaded.
    fn enable_rpf_file(&mut self, df: &DataFile) {
        let Some(name) = df.filename.as_ref() else { return };
        let Some(MapEntry::Archive(ai)) = self.map_lookup(&sanitize(name)) else { return };
        self.enabled_rpfs.push(ai);
        let archive = &self.fs.archives[ai as usize];
        for (ei, e) in archive.entries.iter().enumerate() {
            let f = FileRef { archive: ai, entry: ei as u32 };
            if let Some(table) = self.stream_entries.get_mut(stream_index(e.ext_id)) {
                table.insert(e.name_hash, f);
            }
            match e.name_hash {
                MANIFEST_HASH => self.loads.push(Load::Manifest(f)),
                CACHE_HASH => self.loads.push(Load::Cache(f)),
                GTXD_HASH => self.loads.push(Load::Gtxd(f)),
                _ => {}
            }
        }
    }

    /// `EnableFile`: remembered by file type, for whoever reads them later.
    fn enable_file(&mut self, df: &DataFile) {
        let Some(name) = df.filename.as_ref() else { return };
        let name = sanitize(name);
        let mut f = self.map_lookup(&name).and_then(MapEntry::file);
        if f.is_none() && extension(&name).is_empty() {
            f = self.map_lookup(&format!("{name}.meta")).and_then(MapEntry::file);
        }
        let Some(f) = f else { return };
        let ft = df.file_type.clone().unwrap_or_default();
        self.enabled_data_files.entry(ft).or_default().push(f);
    }

    // -----------------------------------------------------------------------
    // The files Init loads

    fn finish(self) -> GameData {
        let (fs, hashes) = (self.fs, self.hashes);
        let mut gd = GameData {
            devices: self.devices,
            extra_map: self.extra_map,
            enabled_rpfs: self.enabled_rpfs,
            enabled_data_files: self.enabled_data_files,
            stream_entries: self.stream_entries,
            ..Default::default()
        };

        // Manifests, caches and GTXD files: read each once, apply in order.
        let t = Instant::now();
        let mut unique: Vec<Load> = self.loads.clone();
        unique.sort_by_key(|l| match *l {
            Load::Manifest(f) | Load::Cache(f) | Load::Gtxd(f) => f,
        });
        unique.dedup();
        enum Parsed {
            Manifest(Manifest),
            Cache(CacheFile),
            Gtxd(Vec<(u32, u32)>),
        }
        let parsed: HashMap<Load, Parsed> = unique
            .into_par_iter()
            .filter_map(|l| {
                let p = match l {
                    Load::Manifest(f) => Parsed::Manifest(Manifest::read(&load_doc(fs, hashes, f)?, hashes)),
                    Load::Cache(f) => Parsed::Cache(CacheFile::read(&load_doc(fs, hashes, f)?, hashes)),
                    Load::Gtxd(f) => {
                        let doc = load_doc(fs, hashes, f)?;
                        Parsed::Gtxd(txd_relationships(&doc.bag, doc.schema.as_ref(), hashes))
                    }
                };
                Some((l, p))
            })
            .collect();
        for l in &self.loads {
            match (l, parsed.get(l)) {
                (Load::Manifest(f), Some(Parsed::Manifest(m))) => {
                    gd.apply_manifest(m);
                    push_unique(&mut gd.ymf_files, *f);
                }
                (Load::Cache(f), Some(Parsed::Cache(c))) => {
                    gd.apply_cache(c);
                    push_unique(&mut gd.ych_files, *f);
                }
                (Load::Gtxd(_), Some(Parsed::Gtxd(r))) => gd.gtxd_hierarchy.extend(r.iter().copied()),
                _ => {}
            }
        }
        log::info!(
            "game data: {} manifests, {} caches read ({:.2?})",
            gd.ymf_files.len(),
            gd.ych_files.len(),
            t.elapsed()
        );

        // Every ytyp stream entry's archetypes (`EnableYtypFile`), in order.
        let t = Instant::now();
        let ytyps: Vec<FileRef> = gd.stream_entries[stream_index(YTYP)].values().copied().collect();
        let archetypes: Vec<_> =
            ytyps.par_iter().map(|&f| load_doc(fs, hashes, f).map(|d| ytyp_archetypes(&d, f))).collect();
        for (f, list) in ytyps.iter().zip(archetypes) {
            gd.ytyp_files.push(*f);
            for a in list.into_iter().flatten() {
                gd.archetypes.insert(a.name, a);
            }
        }
        log::info!("game data: {} archetypes from {} ytyps ({:.2?})", gd.archetypes.len(), ytyps.len(), t.elapsed());
        gd
    }
}

const YTYP: u8 = 74;

fn push_unique(list: &mut Vec<FileRef>, f: FileRef) {
    if !list.contains(&f) {
        list.push(f);
    }
}

/// `SanitizeFilePath`.
pub fn sanitize(path: &str) -> String {
    path.replace("%PLATFORM%", "x64").replace('\\', "/").to_lowercase()
}

/// The data file types `EnableDataFile` remembers (`EnableFile`). The made-up
/// names are enum values whose names the strings don't know, as the C# spells them.
const ENABLED_FILE_TYPES: &[&str] = &[
    "PERMANENT_ITYP_FILE",
    "DLC_ITYP_REQUEST",
    "LOADING_SCREEN_DATA_FILE",
    "WHEEL_INFO_FILE",
    "WHEEL_SLOT_FILE",
    "PED_METADATA_FILE",
    "METAPED_SETTINGS",
    "METAPED_ASSETS",
    "METAPED_SHOP_ITEMS",
    "VFXVEHICLEINFO_FILE",
    "VEHICLE_AMV_FILE",
    "VEHICLE_FOG_VOLUME_FILE",
    "VEHICLE_LAYOUTS_FILE",
    "VEHICLE_METADATA_FILE",
    "VEHICLE_VARIATION_FILE",
    "CARCOLS_FILE",
    "CARMODCOLS_FILE",
    "TRANSPORT_ANIMATIONS_FILE",
    "MOTIONDBG_FOLDER",
    "AUDIO_WAVEPACK",
    "AUDIO_GAMEDATA",
    "AUDIO_SOUNDDATA",
    "AUDIO_SYNTHDATA",
    "AUDIO_CURVEDATA",
    "AUDIO_SPEECHDATA",
    "AUDIO_DYNAMIXDATA",
    "AUDIO_BUFFERED_GAMEDATA",
    "AUDIO_BUFFERED_SOUNDDATA",
    "MEMGRPS_FOLDER",
    "RESIDENT_TXD",
    "PROJ_DECAL_TXD_OVERRIDE_FILE",
    "EXPRESSION_SETS_FILE",
    "CLIP_SETS_FILE",
    "MOTION_SETS_FILE",
    "CLIP_DICTIONARY_BUDGET_FLAGS_FILE",
    "MOTION_DICTIONARY_BUDGET_FLAGS_FILE",
    "DLC_WEAPON_PICKUPS",
    "WEAPON_TXD_CHAIN_FILE",
    "WEAPON_METADATA_FILE",
    "PED_BRAWLING_REFERENCE_FILE",
    "PED_BRAWLING_STYLE_FILE",
    "AI_KNOWLEDGE_FILE",
    "WEAPONCOMPONENTSINFO_FILE",
    "WEAPON_ANIMATIONS_FILE",
    "WEAPON_HOLDING_ANIMATIONS_FILE",
    "WEAPONINFO_FILE",
    "WEAPONINFO_MP_PATCH_FILE",
    "VFXWEAPONINFO_FILE",
    "SATCHEL_ITEMS_FILE",
    "COVER_WEAPON_FILE",
    "SWATCHES_TXD",
    "HANDLING_FILE",
    "AIM_UPPER_BODY_DATA_FILE",
    "ONLINE_SERVICES_CONFIG",
    "VFX_SETTINGS_FILE",
    "VFXPEDINFO_FILE",
    "VFXMATERIALINFO_FILE",
    "VFXINTERIORINFO_FILE",
    "VFXREGIONINFO_FILE",
    "VFXVOLUMEPLACEMENTINFO_FILE",
    "VFXLIGHTNINGINFO_FILE",
    "PTFXASSETINFO_FILE",
    "VFXENTITYINFO_FILE",
    "VFXEXPLOSIONINFO_FILE",
    "VFXLIQUIDINFO_FILE",
    "VFXSCRIPTINFO_FILE",
    "VFXCUTSCENEINFO_FILE",
    "VFXPROJECTILEINFO_FILE",
    "VFXBLOODINFO_FILE",
    "VFXWHEELINFO_FILE",
    "VFXCOLLISIONINFO_FILE",
    "VFXFIREINFO_FILE",
    "VFXWATERINFO_FILE",
    "VFXVEGETATIONINFO_FILE",
    "DECALINFO_FILE",
    "WINDINFO_FILE",
    "LIGHTEMITTERINFO_FILE",
    "DAMAGEMODELINFO_FILE",
    "VFXANIMINFO_FILE",
    "VFXGROUPINFO_FILE",
    "EAGLEEYEINFO_FILE",
    "DECALOVERRIDEINFO_FILE",
    "INTERACTIVEFIREINFO_FILE",
    "PROC_META_FILE",
    "AMBIENT_SCANNER_ENUM_FILE",
    "DOOR_TUNING_FILE",
    "VEHICLE_BONE_COLLISION_FLAG_FILE",
    "PED_REASONER_FILE",
    "AI_MARKUP_FILE",
    "SCENARIO_PERS_PROP_FILE",
    "AMV_LOW_RES_GRID_FILE",
    "AMV_HIGH_RES_GRID_FILE",
    "AMV_GROUP_DEFINITIONS_FILE",
    "SCENARIO_POINTS_FILE_RESOURCED",
    "GENERATED_SCENARIO_POINTS_FILE_RESOURCED",
    "PROP_SCENARIOS_FILE",
    "PROP_SET_SCENARIOS_FILE",
    "CUSTOM_OBJECT_COVER_POINTS_SETS_FILE",
    "AMBIENT_SCANNER_FILE",
    "AMBIENT_SCANNER_IPL_FILE",
    "TIMECYCLE_FILE",
    "TIMECYCLEMOD_FILE",
    "MISSION_METADATA_FILE",
    "TIME_FILE",
    "TIME_WEATHER_FILE",
    "WEATHER_FILE",
    "WATER_FILE",
    "WATERFOG_TGS_INFO_FILE",
    "MISTMAP_TGS_INFO_FILE",
    "FALLEN_SNOW_TGS_INFO_FILE",
    "UNDERWATERHEIGHT_TGS_INFO_FILE",
    "PATH_ZONES_FILE",
    "POPGRP_FILE",
    "POPSCHED_FILE",
    "POPZONE_FILE",
    "TRAINCONFIGS_FILE",
    "TRAINTRACK_FILE",
    "TROLLEY_CABLE_TRACK_FILE",
    "WORLD_HEIGHTMAP_FILE",
    "WORLD_WATERHEIGHT_FILE",
    "PROP_SET_TUNING_FILE",
    "PERSCHARS_INFO_FILE",
    "VEHICLE_POPULATION_FILE",
    "CUSTOM_MODEL_BOUNDS_FILE",
    "OBJ_COVER_TUNING_FILE",
    "BAR_BRAWL_FILE",
    "FLEE_FILE",
    "FLUID_VESSELS_FILE",
    "ENVIRONMENTAL_WATER_DISTURBERS_FILE",
    "DYNAMIC_SCENARIOS_FILE",
    "GRIDSTREAMER_INFO_FILE",
    "SLOWNESS_ZONES_FILE",
    "SCENIC_VIEWS_FILE",
    "ZONED_ASSET_FILE",
    "CONVEX_HULL_MODIFIER_VOLUMES_FILE",
    "IDLE_PREFERENCE_FILE",
    "FLOCK_VARIATIONS_FILE",
    "EMOTE_INFO_FILE",
    "mJZFaAA_0x66B2F671",
    "MAPZONES_FILE",
    "MARKERSINFO_FILE",
    "CHECKPOINTSINFO_FILE",
    "CONSTELLATIONS_FILE",
    "IeUClzA_0x578A23BD",
    "GvLeJDA_0x886D3FF0",
    "BENDABLEPLANTS_FILE",
    "STREAMING_REQUEST_LISTS_FILE",
    "PED_PERSONALITY_FILE",
    "VEHICLE_CAMERA_OFFSETS_FILE",
    "FRONTEND_MENU_FILE",
    "LEADERBOARD_DATA_FILE",
    "STATS_GROUP_ITEM_SETS_FILE",
    "XtRTsXA_0x793734AD",
    "QEFNWJA_0x4F04BED0",
    "RADIO_FILE",
    "DEFAULT_TASK_DATA_FILE",
    "MOTION_TASK_DATA_FILE",
    "MOUNT_TUNE_FILE",
    "PED_BOUNDS_FILE",
    "DOOR_INTERACTION_DATA_FILE",
    "PED_DUELING_FILE",
    "GRAVITY_WELL_DEFS_FILE",
    "LASSO_DATA_FILE",
    "SCENE_TERRAIN_ANALYZER_DATA_FILE",
    "CARRYING_DATA_FILE",
    "LOOT_CONFIG_FILE",
    "LOOT_TABLE_CONDITIONS_FILE",
    "LOOT_TABLES_FILE",
    "LOOT_TABLES_FILE_MP",
    "LOOT_SOUNDS_FILE",
    "LOOT_VFX_FILE",
    "JHDwOpA_0xD371F2CD",
    "QsjpoUA_0xB463F2DB",
    "KDPJvGA_0xC98BD115",
    "DEFAULT_CARRIABLES_DATA_FILE",
    "DEFAULT_LOOT_DATA_FILE",
    "LOOT_ITEMS_DATA_FILE",
    "SCENARIO_PROP_DATA_FILE",
    "PED_HEALTH_FILE",
    "PED_NAV_CAPABILITES_FILE",
    "PED_COMPONENT_SETS_FILE",
    "PED_PERCEPTION_FILE",
    "PED_IK_SETTINGS_FILE",
    "GESTURE_CONDITIONS_FILE",
    "PED_GESTURE_SETTINGS_FILE",
    "PED_TASK_DATA_FILE",
    "PED_SPECIAL_ABILITIES_FILE",
    "DISPATCH_DATA_FILE",
    "DEFORMABLE_OBJECTS_FILE",
    "TUNABLE_OBJECTS_FILE",
    "LOADOUTS_FILE",
    "FIRINGPATTERNS_FILE",
    "MOTIVATIONS_FILE",
    "AMBIENT_PED_MODEL_SET_FILE",
    "AMBIENT_PROP_MODEL_SET_FILE",
    "AMBIENT_VEHICLE_MODEL_SET_FILE",
    "LADDER_METADATA_FILE",
    "CAMERA_METADATA_FILE",
    "sKFYRAA_0x1D4D6564",
    "qZFKMRA_0x0E3041B8",
    "LIGHTING_DATA_MANAGER_FILE",
    "EXPLOSION_INFO_FILE",
    "ZFPUDSA_0xDDA81DAD",
    "quZqkBA_0xC870D0FC",
    "NmALxSA_0x2436E878",
    "sXzdUIA_0x440A9E62",
    "uLgcvQA_0x03988D65",
    "xyWwSCA_0x34221B50",
    "ACTION_TABLE_DEFINITIONS",
    "gqHUDYA_0xE4D87F60",
    "ACTION_TABLE_RESULTS",
    "ACTION_TABLE_IMPULSES",
    "ACTION_TABLE_INTERRELATIONS",
    "hxlDZKA_0x15C553A2",
    "qjlHqAA_0x2DE38B84",
    "ZtJAmIA_0xD8FAB2EC",
    "rlxDETA_0xA0CB6547",
    "ACTION_TABLE_HOMINGS",
    "asUYUIA_0x316182C5",
    "ACTION_TABLE_DAMAGES",
    "ACTION_TABLE_STRIKE_BONES",
    "ACTION_TABLE_RUMBLES",
    "ACTION_TABLE_BRANCHES",
    "ExHVMjA_0x44F9EB8D",
    "zjJcKBA_0x8BDDFCBD",
    "CLIMB_TUNE_FILE",
    "RELATIONSHIPS_FILE",
    "ZwEiSUA_0x74083861",
    "PERSISTENCE_CONSTANTS_FILE",
    "MODEL_AGING_FILE",
    "TASK_LOCO_FILE",
    "SPEED_VOLUMES_FILE",
    "OBJECT_EXPLOSION_INFO_FILE",
    "COMPANION_BEHAVIOR_ANALYSIS_FILE",
    "COMPANION_PERSONALITY_FILE",
    "QqfDQJA_0x1FE7656B",
    "REFERENCED_CONDITIONS_FILE",
    "wVLAyAA_0xCE439F21",
    "UI_TRACKING_FILE",
    "PTFXSHADERINFO_FILE",
    "PED_SPEED_DATA_FILE",
    "NM_ART_FILE",
    "YcLgmEA_0x48AC3E2D",
    "DAMAGE_DATA_FILE",
    "LOOT_ITEM_MATRIX_FILE",
    "STzosJA_0x27CBA7D2",
    "zGzOABA_0x14FA2DDE",
    "HKcrrlA_0xCC253FA0",
    "UNFveDA_0x69BA843D",
    "nOQZtKA_0xAAD88FA6",
    "MFrVjCA_0xA119C847",
    "RBJuaGA_0xC007E0AF",
    "CKPdQYA_0x22139B6D",
    "otnYlaA_0xCB0391E6",
    "iSWMGQA_0x590824C4",
    "TREE_INFO_FILE",
    "STRAFE_ANIMATION_FILE",
    "YjTyUJA_0xC8A8522B",
    "DDQUpTA_0x2B277A51",
    "kwYHkHA_0x05752057",
    "PED_DAMAGE_INFO_FILE",
    "SPECIES_FILE",
    "SPECIES_RELATIONS_FILE",
    "ANIMAL_TUNING_FILE",
    "FLOCK_TUNING_FILE",
    "PED_ATTRIBUTE_FILE",
    "MODEL_INDICES_FILE",
    "PTFX_SHADERINFO_FILE",
    "PTFX_QUALITYSETTINGS_FILE",
    "YrJyREA_0xB68C9646",
    "lOVfXAA_0xB55C7C9C",
    "zONlgAA_0x5AD6321C",
    "DhJqKNA_0x24162D27",
    "COVER_HELPERS_FILE",
    "kxppTDA_0xDD11711C",
    "ITEM_INTERACTION_INFO_FILE",
    "FISHING_INTERACTION_DATA_FILE",
    "lDxqnEA_0x186204FC",
    "HLkmBZA_0xA3A87FB6",
    "COVER_STREAMING_FILE",
    "zMVtRAA_0x6E38CA3F",
    "IihTYAA_0x26D3B9F0",
    "COMPENDIUM",
    "COMPENDIUM_MP",
    "COLLECTABLES_SP",
    "COLLECTABLES_MP",
    "JkXyUAB_0xB2B575A3",
    "CoNCAJA_0xF53A8C0B",
    "EjOdrAA_0x0EC47CA6",
    "KiLsxXA_0xFB65A5C9",
    "WaWSqOA_0xA5A3502E",
    "TSHzeBA_0x5438022C",
    "mjWGtOA_0x37493E72",
];
