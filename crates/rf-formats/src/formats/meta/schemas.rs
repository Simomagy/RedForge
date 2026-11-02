//! The schemas of every RDR2 resource meta and PSO class and enum
//! (`Rpf8Schemas.LoadSchemaFile` / `EnsureSchemas`).
//!
//! They come from `data/rdr2/schemas.txt`, a copy of RedForge's
//! `RedForge.Games.RDR2.schemas.txt` (made by `Rpf8Schemas.BuildSchemaFile` from
//! the game's files). The file is embedded in the library with `include_str!`, so
//! nothing has to be found at run time, and parsed once, on first use.
//!
//! The format: items separated by empty lines, each a header line then one line
//! per entry or value.
//! - `RSCC <name hex> <size> <key> <unknown>`, entries `<name hex> <offset> <type> <unknown> <ref index> <type name hex>`
//! - `RSCE <name hex> <key>`, values `<name hex> <value>`
//! - `PSOC <name hex> <size>`, entries `<name hex> <offset> <type> <unknown> <ref key hex>`
//! - `PSOE <name hex>`, values `<name hex> <value>`

use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

use super::{RscClassEntry, RscDataType, rsc_class, rsc_enum};
use crate::formats::databag::{EnumValue, Schema};

static TEXT: &str = include_str!("../../../../../data/rdr2/schemas.txt");

/// An `RSCC` item.
#[derive(Clone, Debug, PartialEq)]
pub struct RscClassDef {
    pub name: u32,
    pub size: u32,
    pub key: u32,
    pub unknown: u32,
    /// Malformed entry lines are left as zeroed entries, as the C# does.
    pub entries: Vec<RscClassEntry>,
}

/// An `RSCE` item.
#[derive(Clone, Debug, PartialEq)]
pub struct RscEnumDef {
    pub name: u32,
    pub key: u32,
    pub values: Vec<EnumValue>,
}

/// One entry of a `PSOC` item. `data_type` is the C# `PsoDataType` name.
#[derive(Clone, Debug, PartialEq)]
pub struct PsoEntryDef {
    pub name: u32,
    pub offset: u16,
    pub data_type: String,
    pub unknown_5h: u8,
    pub reference_key: u32,
}

/// A `PSOC` item. Malformed entry lines are `None` (null in the C#).
#[derive(Clone, Debug, PartialEq)]
pub struct PsoClassDef {
    pub name: u32,
    pub size: u32,
    pub entries: Vec<Option<PsoEntryDef>>,
}

/// A `PSOE` item. Malformed value lines are `None`.
#[derive(Clone, Debug, PartialEq)]
pub struct PsoEnumDef {
    pub name: u32,
    pub values: Vec<Option<EnumValue>>,
}

/// The parsed schemas file: the first item of each kind and name, in file order
/// (as the C#'s dictionaries keep them).
#[derive(Clone, Debug, Default)]
pub struct SchemaFile {
    pub rsc_classes: Vec<RscClassDef>,
    pub rsc_enums: Vec<RscEnumDef>,
    pub pso_classes: Vec<PsoClassDef>,
    pub pso_enums: Vec<PsoEnumDef>,
}

fn hex(s: &str) -> Option<u32> {
    u32::from_str_radix(s, 16).ok()
}

impl SchemaFile {
    pub fn parse(text: &str) -> Self {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut file = SchemaFile::default();
        let mut seen: [HashSet<u32>; 4] = Default::default();
        let mut item: Vec<&str> = Vec::new();
        for line in text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).chain(std::iter::once("")) {
            if !line.is_empty() {
                item.push(line);
                continue;
            }
            if !item.is_empty() {
                file.add_item(&item, &mut seen);
                item.clear();
            }
        }
        file
    }

    fn add_item(&mut self, item: &[&str], seen: &mut [HashSet<u32>; 4]) {
        let ids: Vec<&str> = item[0].split(' ').collect();
        if ids.len() <= 1 {
            return;
        }
        let Some(name) = hex(ids[1]) else { return };
        let lines = &item[1..];
        match ids[0] {
            "RSCC" if ids.len() == 5 => {
                let (Ok(size), Ok(key), Ok(unknown)) = (ids[2].parse(), ids[3].parse(), ids[4].parse()) else { return };
                let entries = lines
                    .iter()
                    .map(|l| {
                        let e: Vec<&str> = l.split(' ').collect();
                        if e.len() != 6 {
                            return RscClassEntry::default();
                        }
                        RscClassEntry {
                            name: hex(e[0]).unwrap_or(0),
                            offset: e[1].parse().unwrap_or(0),
                            data_type: RscDataType::parse(e[2]).unwrap_or_default(),
                            unknown_9h: e[3].parse().unwrap_or(0),
                            reference_type_index: e[4].parse().unwrap_or(0),
                            type_name: hex(e[5]).unwrap_or(0),
                        }
                    })
                    .collect();
                if seen[0].insert(name) {
                    self.rsc_classes.push(RscClassDef { name, size, key, unknown, entries });
                }
            }
            "RSCE" if ids.len() == 3 => {
                let Ok(key) = ids[2].parse() else { return };
                let values = lines
                    .iter()
                    .filter_map(|l| {
                        let v: Vec<&str> = l.split(' ').collect();
                        (v.len() == 2)
                            .then(|| EnumValue { name: hex(v[0]).unwrap_or(0), value: v[1].parse().unwrap_or(0) })
                    })
                    .collect();
                if seen[1].insert(name) {
                    self.rsc_enums.push(RscEnumDef { name, key, values });
                }
            }
            "PSOC" if ids.len() == 3 => {
                let Ok(size) = ids[2].parse() else { return };
                let entries = lines
                    .iter()
                    .map(|l| {
                        let e: Vec<&str> = l.split(' ').collect();
                        (e.len() == 5).then(|| PsoEntryDef {
                            name: hex(e[0]).unwrap_or(0),
                            offset: e[1].parse().unwrap_or(0),
                            data_type: e[2].to_owned(),
                            unknown_5h: e[3].parse().unwrap_or(0),
                            reference_key: hex(e[4]).unwrap_or(0),
                        })
                    })
                    .collect();
                if seen[2].insert(name) {
                    self.pso_classes.push(PsoClassDef { name, size, entries });
                }
            }
            "PSOE" if ids.len() == 2 => {
                let values = lines
                    .iter()
                    .map(|l| {
                        let v: Vec<&str> = l.split(' ').collect();
                        (v.len() == 2)
                            .then(|| EnumValue { name: hex(v[0]).unwrap_or(0), value: v[1].parse().unwrap_or(0) })
                    })
                    .collect();
                if seen[3].insert(name) {
                    self.pso_enums.push(PsoEnumDef { name, values });
                }
            }
            _ => {}
        }
    }
}

/// The schemas file, parsed on first use.
pub fn schema_file() -> &'static SchemaFile {
    static FILE: OnceLock<SchemaFile> = OnceLock::new();
    FILE.get_or_init(|| SchemaFile::parse(TEXT))
}

/// Every RDR2 resource meta class and enum (`Rpf8Schemas.RscSchema`): what XML is
/// read with, and new files are made from.
pub fn rsc_schema() -> &'static Schema {
    static SCHEMA: OnceLock<Schema> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let f = schema_file();
        let classes =
            f.rsc_classes.iter().filter_map(|c| match rsc_class(c.name, c.key, c.unknown, c.size, c.entries.clone()) {
                Ok(cls) => Some(Arc::new(cls)),
                Err(e) => {
                    log::warn!("schemas.txt: class 0x{:08X}: {e}", c.name);
                    None
                }
            });
        let enums = f.rsc_enums.iter().map(|e| Arc::new(rsc_enum(e.name, e.key, e.values.clone())));
        Schema::new(classes, enums)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_kinds() {
        let f = schema_file();
        assert!(f.rsc_classes.len() > 50, "{}", f.rsc_classes.len());
        assert!(!f.rsc_enums.is_empty());
        assert!(f.pso_classes.len() > 1000);
        assert!(!f.pso_enums.is_empty());
        let s = SchemaFile::parse(
            "\u{feff}RSCC 0000000A 8 1 2\n0000000B 4 Float 0 0 0\nbad\n\nRSCE 0000000C 5\n0000000D 3\n",
        );
        assert_eq!(s.rsc_classes[0].entries.len(), 2);
        assert_eq!(s.rsc_classes[0].entries[0].data_type, RscDataType::FLOAT);
        assert_eq!(s.rsc_classes[0].entries[1], RscClassEntry::default());
        assert_eq!(s.rsc_enums[0].values, vec![EnumValue { name: 0xD, value: 3 }]);
    }
}
