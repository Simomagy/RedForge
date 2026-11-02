//! Metadata files as data bags (`Rpf8FileManager.LoadDataBagPack`), and the
//! `DataBag2` getters the data file manager uses, with the C#'s semantics.

use rf_formats::formats::databag::{DataBag, Schema, Value, ValueType};
use rf_formats::formats::meta::Meta;
use rf_formats::formats::pso::PsoFile;
use rf_formats::formats::rbf;
use rf_formats::hash::{HashIndex, joaat};
use rf_formats::rpf8::{FileRef, Rpf8FileSystem};

/// A metadata file's root bag, with the schema its enum names come from.
pub struct Doc {
    pub bag: DataBag,
    /// The file's own schema for PSO and resource metadata; `None` for RBF and XML.
    pub schema: Option<Schema>,
}

const MAGIC_XML: u32 = 0x6D78_3F3C; // "<?xm"
const MAGIC_BOM_XML: u32 = 0x3CBF_BBEF; // BOM and "<"
const MAGIC_RBF: u32 = 0x3046_4252; // "RBF0"
const MAGIC_PSO: u32 = 0x4E49_5350; // "PSIN"

/// `LoadDataBagPack`: a resource as resource metadata; a binary file as XML, RBF
/// or PSO by its magic (or as XML by its `.xml`/`.meta` extension). `None` when
/// the file can't be read or isn't metadata.
pub fn load_doc(fs: &Rpf8FileSystem, hashes: &HashIndex, f: FileRef) -> Option<Doc> {
    let e = fs.entry(f);
    let result = if e.is_resource() {
        fs.extract_loose(f).map_err(|e| e.to_string()).and_then(|d| {
            Meta::read(&d).map(|m| m.root.map(|bag| Doc { bag, schema: Some(m.schema) })).map_err(|e| e.to_string())
        })
    } else {
        let name = &fs.archive(f).names[f.entry as usize];
        fs.extract(f).map_err(|e| e.to_string()).and_then(|d| binary_doc(&d, name, hashes))
    };
    match result {
        Ok(doc) => doc,
        Err(err) => {
            log::warn!("{}: {err}", fs.path(f));
            None
        }
    }
}

fn binary_doc(data: &[u8], name: &str, hashes: &HashIndex) -> Result<Option<Doc>, String> {
    let Some(magic) = data.get(0..4).map(|m| u32::from_le_bytes(m.try_into().unwrap())) else { return Ok(None) };
    let xml = |data: &[u8]| {
        let text = String::from_utf8_lossy(data);
        DataBag::from_xml(&text, None, hashes)
            .map(|b| b.map(|bag| Doc { bag, schema: None }))
            .map_err(|e| e.to_string())
    };
    match magic {
        MAGIC_XML | MAGIC_BOM_XML => xml(data),
        MAGIC_RBF => {
            let mut root = rbf::read(data).map_err(|e| e.to_string())?;
            Ok(rbf::data_bag(&mut root, hashes).map(|bag| Doc { bag, schema: None }))
        }
        MAGIC_PSO => {
            let pso = PsoFile::read(data, hashes).map_err(|e| e.to_string())?;
            Ok(pso.root().map_err(|e| e.to_string())?.map(|(bag, schema)| Doc { bag, schema: Some(schema) }))
        }
        _ => {
            let lower = name.to_lowercase();
            if lower.ends_with(".xml") || lower.ends_with(".meta") { xml(data) } else { Ok(None) }
        }
    }
}

/// `GetString`: a string field, or an enum field's value names (from the file's
/// schema, or made-up names for unknown hashes as the C# does).
pub fn string(bag: &DataBag, field: u32, schema: Option<&Schema>, hashes: &HashIndex) -> Option<String> {
    bag.enum_string(field, schema, hashes)
}

/// `GetBags`, without the items that couldn't be read.
pub fn bags(bag: &DataBag, field: u32) -> impl Iterator<Item = &DataBag> {
    bag.bags(field).into_iter().flatten().flatten()
}

/// `GetArray<string>`.
pub fn strings(bag: &DataBag, field: u32) -> Option<&[String]> {
    match bag.object(field) {
        Some(Value::Strings(s)) => Some(s),
        _ => None,
    }
}

/// `JenkHash.GenHash(GetString(f).ToLowerInvariant())`, as the C# hashes names
/// read as strings. A hash field gives its value (where the C# would throw on the
/// missing string).
pub fn string_hash(bag: &DataBag, field: u32, schema: Option<&Schema>, hashes: &HashIndex) -> Option<u32> {
    match string(bag, field, schema, hashes) {
        Some(s) => Some(joaat(&s.to_lowercase())),
        None => bag.class.field(field).filter(|f| f.data_type == ValueType::Hash).map(|_| bag.hash(field)),
    }
}

/// `GetRpf8FileHash`: the hash a file is streamed by, the joaat of its lowercase
/// name without folder or extension.
pub fn file_hash(file_name: &str) -> u32 {
    joaat(&stem(file_name).to_lowercase())
}

/// The hash of a name, or for names the strings don't know (`0x1234ABCD`, as
/// [`Rpf8FileSystem`] paths show them) the hash they stand for.
pub fn name_hash(name: &str) -> u32 {
    if let Some(hex) = name.strip_prefix("0x").or_else(|| name.strip_prefix("0X"))
        && let Ok(h) = u32::from_str_radix(hex, 16)
    {
        return h;
    }
    joaat(&name.to_lowercase())
}

/// `GetRpf8FileExt`: the `Rpf8FileExt` id of a file name's extension, 0 (`rpf`)
/// when it isn't one.
pub fn file_ext(file_name: &str) -> u8 {
    let ext = extension(file_name).to_lowercase();
    ext_id(&ext).unwrap_or(0)
}

/// The `Rpf8FileExt` id of an extension (`"ytd"` → 5).
pub fn ext_id(ext: &str) -> Option<u8> {
    let ext = ext.strip_prefix('.').unwrap_or(ext);
    (0..=u8::MAX).find(|&id| rf_formats::rpf8::ext_name(id).is_some_and(|n| n.eq_ignore_ascii_case(ext)))
}

/// `Path.GetFileName`, with `/`, `\` and `:` as separators.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\', ':']).next().unwrap_or(path)
}

/// `Path.GetFileNameWithoutExtension`.
pub fn stem(path: &str) -> &str {
    let name = file_name(path);
    name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name)
}

/// `Path.GetExtension`, without the dot.
pub fn extension(path: &str) -> &str {
    file_name(path).rsplit_once('.').map(|(_, e)| e).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names() {
        assert_eq!(file_hash("levels_1.rpf/x/val_01_.rpf/Val_01_Barn.ydr"), joaat("val_01_barn"));
        assert_eq!(file_hash("foo"), joaat("foo"));
        assert_eq!(file_ext("a/b.YTD"), 5);
        assert_eq!(file_ext("a/b.ymap"), 73);
        assert_eq!(file_ext("a/b.xml"), 0);
        assert_eq!(file_ext("common:/data/levels"), 0);
        assert_eq!(ext_id(".ytyp"), Some(74));
        assert_eq!(name_hash("0x1234ABCD"), 0x1234_ABCD);
        assert_eq!(name_hash("Prop"), joaat("prop"));
        assert_eq!(extension("common:/data/x.meta"), "meta");
        assert_eq!(extension("platform:/levels/rdr3/sp_manifest"), "");
    }
}
