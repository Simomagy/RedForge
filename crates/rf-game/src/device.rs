//! The game's file devices (`Rpf8DataFileDevice`): `common:/`, `platform:/`,
//! `update:/`, one per DLC... Each maps file hashes to the files of the archives
//! mounted on it, and can redirect to paths on other devices first.

use std::collections::HashMap;

use rf_formats::hash::joaat;
use rf_formats::rpf8::{FileRef, Rpf8FileSystem};

/// What a device path leads to: a file, or a whole archive (the C#'s directory
/// entry for an archive's root).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MapEntry {
    File(FileRef),
    /// An index into [`Rpf8FileSystem::archives`].
    Archive(u32),
}

impl MapEntry {
    pub fn file(self) -> Option<FileRef> {
        match self {
            MapEntry::File(f) => Some(f),
            MapEntry::Archive(_) => None,
        }
    }
}

/// `Rpf8DataFileDevice`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Device {
    /// File hash → file. Archives are keyed by the hash of their name plus `.rpf`.
    pub main_map: HashMap<u32, MapEntry>,
    /// Paths on other devices looked in first, the last added first
    /// (`update_common:/common/`...).
    pub extra_map: Vec<String>,
}

/// All the devices, by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Devices {
    pub devices: HashMap<String, Device>,
}

/// Lookups that go deeper than this through extra mappings are given up (the C#
/// would recurse until the stack overflows).
const MAX_DEPTH: usize = 32;

/// `SplitPathString`: `common:/data/x.meta` → (`common`, `data/x.meta`), both
/// lowercased; a path without a device is on the device named "" and keeps its case.
pub fn split_path(path: &str) -> (String, String) {
    match path.find(":/") {
        Some(i) => (path[..i].to_lowercase(), path[i + 2..].to_lowercase()),
        None => (String::new(), path.to_owned()),
    }
}

impl Devices {
    /// `GetDevice`: the device, created when missing.
    pub fn get_mut(&mut self, name: &str) -> &mut Device {
        self.devices.entry(name.to_owned()).or_default()
    }

    /// `Rpf8DataFileMgr.MapLookup`: the file or archive a device path leads to.
    pub fn lookup(&self, fs: &Rpf8FileSystem, path: &str) -> Option<MapEntry> {
        self.lookup_depth(fs, path, 0)
    }

    fn lookup_depth(&self, fs: &Rpf8FileSystem, path: &str, depth: usize) -> Option<MapEntry> {
        if depth > MAX_DEPTH {
            log::warn!("{path}: too many redirections");
            return None;
        }
        let (device, file) = split_path(path);
        let d = self.devices.get(&device)?;
        // `Rpf8DataFileDevice.MapLookup`.
        for extra in d.extra_map.iter().rev() {
            if let Some(e) = self.lookup_depth(fs, &format!("{extra}{file}"), depth + 1) {
                return Some(e);
            }
        }
        let Some(idx) = file.find(".rpf") else {
            return d.main_map.get(&joaat(&file)).copied();
        };
        let rpf = *d.main_map.get(&joaat(&file[..idx + 4]))?;
        if file.len() < idx + 5 {
            return Some(rpf);
        }
        let hash = joaat(&file[idx + 5..]);
        let MapEntry::Archive(a) = rpf else { return None };
        // Only the files at the archive's root, as the C# looks in the root
        // directory entry.
        let archive = &fs.archives[a as usize];
        archive
            .entries
            .iter()
            .enumerate()
            .find(|(i, e)| e.name_hash == hash && !archive.names[*i].contains('/'))
            .map(|(i, _)| MapEntry::File(FileRef { archive: a, entry: i as u32 }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_paths() {
        assert_eq!(split_path("Common:/Data/X.meta"), ("common".into(), "data/x.meta".into()));
        assert_eq!(split_path("Data/X.meta"), ("".into(), "Data/X.meta".into()));
    }
}
