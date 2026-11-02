//! What the setup and content files say: packs (`Rpf8DataPack`), their data
//! files (`Rpf8DataFile`), content change sets and their groups, overlay infos
//! and levels. Init reads these to know which archives and files to enable.

use indexmap::IndexMap;
use rf_formats::formats::databag::{DataBag, Schema};
use rf_formats::hash::HashIndex;

use crate::files::{bags, string, strings};

/// `Rpf8DataFile`: one item of a content file's `dataFiles`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DataFile {
    pub filename: Option<String>,
    pub file_type: Option<String>,
    pub contents: Option<String>,
}

impl DataFile {
    pub fn read(b: &DataBag, schema: Option<&Schema>, hashes: &HashIndex) -> Self {
        Self {
            filename: string(b, 0x40CA_983A, schema, hashes),  // "filename"
            file_type: string(b, 0x1492_81CE, schema, hashes), // "fileType"
            contents: string(b, 0x2E85_D584, schema, hashes),  // "contents"
        }
    }
}

/// `Rpf8ContentChangeSet`, with what Init uses.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContentChangeSet {
    pub name: Option<String>,
    pub files_to_enable: Vec<String>,
    /// The `filesToEnable` of each `mapChangeSetData` item.
    pub map_files_to_enable: Vec<Vec<String>>,
}

impl ContentChangeSet {
    fn read(b: &DataBag, schema: Option<&Schema>, hashes: &HashIndex) -> Self {
        let files = |b: &DataBag| strings(b, 0x9017_AFD6).map(<[String]>::to_vec).unwrap_or_default(); // "filesToEnable"
        Self {
            name: string(b, 0xD7B2_838B, schema, hashes), // "changeSetName"
            files_to_enable: files(b),
            map_files_to_enable: bags(b, 0x3096_5039).map(files).collect(), // "mapChangeSetData"
        }
    }
}

/// `Rpf8OverlayInfo`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OverlayInfo {
    pub name_id: Option<String>,
    pub change_set: Option<String>,
    pub version: i32,
    pub change_set_group_to_execute_with: Option<String>,
    /// The change set it names, in [`Content::change_sets`].
    pub change_set_id: Option<usize>,
}

impl OverlayInfo {
    pub fn read(b: &DataBag, schema: Option<&Schema>, hashes: &HashIndex) -> Self {
        Self {
            name_id: string(b, 0x0A05_516F, schema, hashes),    // "nameId"
            change_set: string(b, 0x33EF_BD82, schema, hashes), // "changeSet"
            version: b.i32(0x68C2_7E33),                        // "version"
            change_set_group_to_execute_with: string(b, 0xDA77_24AA, schema, hashes),
            change_set_id: None,
        }
    }
}

/// `Rpf8ContentChangeSetGroup`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContentChangeSetGroup {
    /// Indices into [`Content::change_sets`].
    pub change_sets: Vec<usize>,
    pub overlay_infos: IndexMap<String, OverlayInfo>,
}

impl ContentChangeSetGroup {
    /// `GetAllChangeSets`: the group's change sets, then those of its overlay
    /// infos that aren't already there.
    pub fn all_change_sets(&self) -> Vec<usize> {
        let mut list = self.change_sets.clone();
        for oi in self.overlay_infos.values() {
            if let Some(cs) = oi.change_set_id
                && !list.contains(&cs)
            {
                list.push(cs);
            }
        }
        list
    }
}

/// `Rpf8DataPack`: the base pack (`rdr3.meta`) or a DLC.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DataPack {
    pub name: String,
    /// The path in `dlclist.xml`, with a trailing `/`.
    pub path: String,
    // From setup2.xml.
    pub device_name: Option<String>,
    pub dat_file: Option<String>,
    pub name_hash: Option<String>,
    pub order: i32,
    pub minor_order: i32,
    /// Group name → change set names.
    pub content_change_set_groups: IndexMap<String, Vec<String>>,
    // From the content file.
    pub data_files: IndexMap<String, DataFile>,
    /// Change set name → index into [`Content::change_sets`].
    pub content_change_sets: IndexMap<String, usize>,
}

impl DataPack {
    /// `new Rpf8DataPack(name, path, setupData)`.
    pub fn from_setup(name: &str, path: &str, setup: &DataBag, schema: Option<&Schema>, hashes: &HashIndex) -> Self {
        let mut groups = IndexMap::new();
        for g in bags(setup, 0x14D2_E343) {
            // "contentChangeSetGroups"
            let name = string(g, 0xDF14_4583, schema, hashes); // "NameHash"
            let sets = strings(g, 0xD34A_932D); // "ContentChangeSets"
            if let (Some(name), Some(sets)) = (name, sets)
                && !name.is_empty()
            {
                groups.insert(name, sets.to_vec());
            }
        }
        Self {
            name: name.to_owned(),
            path: path.to_owned(),
            device_name: string(setup, 0x6A9E_7997, schema, hashes), // "deviceName"
            dat_file: string(setup, 0x2270_F8B2, schema, hashes),    // "datFile"
            name_hash: string(setup, 0x97CF_ED26, schema, hashes),   // "nameHash"
            order: setup.i32(0x458F_864B),                           // "order"
            minor_order: setup.i32(0x9DC6_F0C0),                     // "minorOrder"
            content_change_set_groups: groups,
            ..Default::default()
        }
    }
}

/// `Rpf8LevelData`, with what Init uses.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LevelData {
    pub friendly_name: Option<String>,
    pub title: Option<String>,
    pub filename: Option<String>,
}

impl LevelData {
    pub fn read(b: &DataBag, schema: Option<&Schema>, hashes: &HashIndex) -> Self {
        Self {
            friendly_name: string(b, 0x7BCC_E6D3, schema, hashes), // "cFriendlyName"
            title: string(b, 0x31B6_9372, schema, hashes),         // "cTitle"
            filename: string(b, 0xE01F_1D17, schema, hashes),      // "cFilename"
        }
    }
}

/// Init's view of the content: packs, change sets, data files.
#[derive(Clone, Debug, Default)]
pub struct Content {
    pub base_pack: Option<DataPack>,
    pub mandatory_packs: Vec<DataPack>,
    pub levels: IndexMap<String, LevelData>,
    /// Every change set read, referenced by index.
    pub change_sets: Vec<ContentChangeSet>,
    /// `ContentChangeSets`: name → index.
    pub change_sets_by_name: IndexMap<String, usize>,
    pub change_set_groups: IndexMap<String, ContentChangeSetGroup>,
    /// `DataFiles`: file name → data file.
    pub data_files: IndexMap<String, DataFile>,
}

impl Content {
    /// `LoadDatFileContent`'s own data files and change sets (the included
    /// files' data files are added by the caller, which can load them).
    pub fn read_dat_file(
        &mut self,
        pack: &mut DataPack,
        content: &DataBag,
        schema: Option<&Schema>,
        hashes: &HashIndex,
    ) {
        for df in bags(content, 0xF83A_C9E3) {
            // "dataFiles"
            let df = DataFile::read(df, schema, hashes);
            if let Some(name) = df.filename.clone().filter(|n| !n.is_empty()) {
                pack.data_files.insert(name, df);
            }
        }
        for cs in bags(content, 0x24A3_82B4) {
            // "contentChangeSets"
            let cs = ContentChangeSet::read(cs, schema, hashes);
            if let Some(name) = cs.name.clone().filter(|n| !n.is_empty()) {
                self.change_sets.push(cs);
                pack.content_change_sets.insert(name, self.change_sets.len() - 1);
            }
        }
    }
}
