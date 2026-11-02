//! The game's file formats, and their conversion to and from XML the way the C#
//! RedForge (and CodeX) does it, so the XML stays compatible with Sollumz.
//!
//! Each format module has its plain-data structs plus two functions used by the
//! dispatch below:
//! - `to_xml(file, cx)`: a loose file (RSC8 header for resources) to XML;
//! - `from_xml(xml, cx)`: XML back to a loose file.

pub mod dat;
pub mod databag;
pub mod fxdb;
pub mod meta;
pub mod pso;
pub mod rbf;
pub mod ybn;
pub mod ycd;
pub mod ydr;
pub mod yed;
pub mod yfd;
pub mod yft;
pub mod yld;
pub mod yldb;
pub mod ynv;
pub mod ypmd;
pub mod yrecords;
pub mod ytd;

use std::path::{Path, PathBuf};

use crate::hash::HashIndex;
use crate::rsc8::is_loose_resource;
use crate::{Error, Result};

/// What converting to or from XML needs besides the data.
pub struct XmlContext<'a> {
    pub hashes: &'a HashIndex,
    /// The folder for the files that go next to the XML (textures as DDS),
    /// for the formats that have them.
    pub folder: Option<PathBuf>,
}

/// The kinds of file that convert to XML.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Ydr,
    Yft,
    Ydd,
    Ytd,
    Ybn,
    Ycd,
    Yld,
    Yed,
    Yldb,
    Yad,
    Yvr,
    Ywr,
    Yfd,
    Ypmd,
    Ynv,
    /// Resource metadata: ymap, ytyp, ymt.
    Meta,
    Pso,
    Rbf,
    Fxdb,
    SlodIndexList,
    ImposterExtents,
}

impl Kind {
    /// The root element of the XML.
    pub fn root(self) -> Option<&'static str> {
        Some(match self {
            Kind::Ydr => "RDR2Drawable",
            Kind::Yft => "RDR2Fragment",
            Kind::Ydd => "RDR2DrawableDictionary",
            Kind::Ytd => "RDR2TextureDictionary",
            Kind::Ybn => "RDR2Bounds",
            Kind::Ycd => "RDR2ClipDictionary",
            Kind::Yld => "RDR2ClothDictionary",
            Kind::Yed => "RDR2ExpressionDictionary",
            Kind::Yldb => "RDR2LanguageDatabase",
            Kind::Yad => "RDR2AnimDictionary",
            Kind::Yvr => "RDR2VehicleRecords",
            Kind::Ywr => "RDR2WaypointRecords",
            Kind::Yfd => "RDR2FrameFilterDictionary",
            Kind::Ypmd => "RDR2PoseMatcherDictionary",
            Kind::Ynv => "RDR2Navmesh",
            Kind::Fxdb => "RDR2EffectsDatabase",
            Kind::SlodIndexList => "RDR2SlodIndexList",
            Kind::ImposterExtents => "RDR2ImposterExtents",
            // The root comes from the data.
            Kind::Meta | Kind::Pso | Kind::Rbf => return None,
        })
    }

    /// Which kind a file is, as RedForge's `ConvertToXml` decides it: resources
    /// by extension, other files by name or magic.
    pub fn of_file(name: &str, data: &[u8]) -> Option<Kind> {
        let lower = name.to_lowercase();
        let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        if is_loose_resource(data) {
            return Some(match ext {
                "ydr" => Kind::Ydr,
                "yft" => Kind::Yft,
                "ydd" => Kind::Ydd,
                "ytd" => Kind::Ytd,
                "ybn" => Kind::Ybn,
                "ycd" => Kind::Ycd,
                "yld" => Kind::Yld,
                "yed" => Kind::Yed,
                "yldb" => Kind::Yldb,
                "yad" => Kind::Yad,
                "yvr" => Kind::Yvr,
                "ywr" => Kind::Ywr,
                "yfd" => Kind::Yfd,
                "ypmd" => Kind::Ypmd,
                "ynv" => Kind::Ynv,
                "ypt" => return None,
                _ => Kind::Meta,
            });
        }
        if lower.ends_with("slodindexlist.dat") {
            return Some(Kind::SlodIndexList);
        }
        if lower.ends_with("imposterextents.dat") {
            return Some(Kind::ImposterExtents);
        }
        match data.get(0..4).map(|m| u32::from_le_bytes(m.try_into().unwrap())) {
            Some(0x3046_4252) => Some(Kind::Rbf),
            Some(0x4E49_5350) => Some(Kind::Pso),
            Some(m) if fxdb::is_magic(m) => Some(Kind::Fxdb),
            _ => None,
        }
    }

    /// Which kind an XML file converts back to, by its name (`foo.ydr.xml`).
    pub fn of_xml_name(name: &str) -> Option<Kind> {
        let n = name.to_lowercase();
        let ends = |s: &str| n.ends_with(s);
        Some(if ends(".pso.xml") {
            Kind::Pso
        } else if ends(".rsc.xml") {
            Kind::Meta
        } else if ends(".rbf.xml") {
            Kind::Rbf
        } else if ends(".ydr.xml") {
            Kind::Ydr
        } else if ends(".yft.xml") {
            Kind::Yft
        } else if ends(".ydd.xml") {
            Kind::Ydd
        } else if ends(".ytd.xml") {
            Kind::Ytd
        } else if ends(".ybn.xml") {
            Kind::Ybn
        } else if ends(".ycd.xml") {
            Kind::Ycd
        } else if ends(".yld.xml") {
            Kind::Yld
        } else if ends(".yed.xml") {
            Kind::Yed
        } else if ends(".yldb.xml") {
            Kind::Yldb
        } else if ends(".yad.xml") {
            Kind::Yad
        } else if ends(".yvr.xml") {
            Kind::Yvr
        } else if ends(".ywr.xml") {
            Kind::Ywr
        } else if ends(".yfd.xml") {
            Kind::Yfd
        } else if ends(".ypmd.xml") {
            Kind::Ypmd
        } else if ends(".ynv.xml") {
            Kind::Ynv
        } else if ends("slodindexlist.dat.xml") {
            Kind::SlodIndexList
        } else if ends("imposterextents.dat.xml") {
            Kind::ImposterExtents
        } else {
            return None;
        })
    }

    /// The XML file name for a game file of this kind.
    pub fn xml_file_name(self, name: &str) -> String {
        match self {
            Kind::Meta => format!("{name}.rsc.xml"),
            Kind::Pso => format!("{name}.pso.xml"),
            Kind::Rbf => format!("{name}.rbf.xml"),
            _ => format!("{name}.xml"),
        }
    }

    /// Whether the XML has files next to it, in a folder named after the file.
    pub fn has_folder(self) -> bool {
        matches!(self, Kind::Ydr | Kind::Yft | Kind::Ydd | Kind::Ytd | Kind::Fxdb)
    }
}

/// The folder for a file's DDS textures and other externals: next to the XML,
/// named after the game file without its extension (`foo.ydr` → `foo/`).
pub fn externals_folder(xml_dir: &Path, file_name: &str) -> PathBuf {
    let stem = Path::new(file_name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    xml_dir.join(stem)
}

/// Converts a game file to XML. `data` is the file as on disk: a loose resource
/// with its RSC8 header, or a binary file.
pub fn to_xml(name: &str, data: &[u8], cx: &XmlContext) -> Result<String> {
    let lower = name.to_lowercase();
    if lower.ends_with(".xml") || lower.ends_with(".meta") {
        return Ok(String::from_utf8_lossy(data).into_owned());
    }
    let Some(kind) = Kind::of_file(name, data) else {
        return Err(Error::Unsupported(format!("{name}: no XML conversion for this file")));
    };
    match kind {
        Kind::Ytd => ytd::to_xml(data, cx),
        Kind::Ydr => ydr::to_xml(data, cx),
        Kind::Ydd => ydr::ydd_to_xml(data, cx),
        Kind::Yft => yft::to_xml(data, cx),
        Kind::Ybn => ybn::to_xml(data, cx),
        Kind::Ycd => ycd::to_xml(data, cx),
        Kind::Yad => ycd::yad_to_xml(data, cx),
        Kind::Yld => yld::to_xml(data, cx),
        Kind::Yed => yed::to_xml(data, cx),
        Kind::Yldb => yldb::to_xml(data, cx),
        Kind::Yvr => yrecords::yvr_to_xml(data, cx),
        Kind::Ywr => yrecords::ywr_to_xml(data, cx),
        Kind::Yfd => yfd::to_xml(data, cx),
        Kind::Ypmd => ypmd::to_xml(data, cx),
        Kind::Ynv => ynv::to_xml(data, cx),
        Kind::Meta => meta::to_xml(data, cx),
        Kind::Pso => pso::to_xml(data, cx),
        Kind::Rbf => rbf::to_xml(data, cx),
        Kind::Fxdb => fxdb::to_xml(data, cx),
        Kind::SlodIndexList => dat::slod_index_list_to_xml(data, cx),
        Kind::ImposterExtents => dat::imposter_extents_to_xml(data, cx),
    }
}

/// Converts XML back to a game file, choosing the format by the XML's file name
/// (`foo.ydr.xml`, `foo.ymap.rsc.xml`, ...).
pub fn from_xml(xml_name: &str, xml: &str, cx: &XmlContext) -> Result<Vec<u8>> {
    let Some(kind) = Kind::of_xml_name(xml_name) else {
        return Err(Error::Unsupported(format!("{xml_name}: unknown XML file type")));
    };
    match kind {
        Kind::Ytd => ytd::from_xml(xml, cx),
        Kind::Ydr => ydr::from_xml(xml, cx),
        Kind::Ydd => ydr::ydd_from_xml(xml, cx),
        Kind::Yft => yft::from_xml(xml, cx),
        Kind::Ybn => ybn::from_xml(xml, cx),
        Kind::Ycd => ycd::from_xml(xml, cx),
        Kind::Yad => ycd::yad_from_xml(xml, cx),
        Kind::Yld => yld::from_xml(xml, cx),
        Kind::Yed => yed::from_xml(xml, cx),
        Kind::Yldb => yldb::from_xml(xml, cx),
        Kind::Yvr => yrecords::yvr_from_xml(xml, cx),
        Kind::Ywr => yrecords::ywr_from_xml(xml, cx),
        Kind::Yfd => yfd::from_xml(xml, cx),
        Kind::Ypmd => ypmd::from_xml(xml, cx),
        Kind::Ynv => ynv::from_xml(xml, cx),
        Kind::Meta => meta::from_xml(xml, cx),
        Kind::Pso => pso::from_xml(xml, cx),
        Kind::Rbf => rbf::from_xml(xml, cx),
        Kind::Fxdb => Err(Error::Unsupported("fxdb can only be converted to XML".into())),
        Kind::SlodIndexList => dat::slod_index_list_from_xml(xml, cx),
        Kind::ImposterExtents => dat::imposter_extents_from_xml(xml, cx),
    }
}

/// For format modules that aren't ported yet.
pub(crate) fn todo<T>(what: &str) -> Result<T> {
    Err(Error::Unsupported(format!("{what} isn't ported to Rust yet")))
}
