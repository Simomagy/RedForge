//! `slodindexlist.dat` and `imposterextents.dat`, from `DatFiles.cs`.
//!
//! Not ported yet.

use super::{XmlContext, todo};
use crate::Result;

pub fn slod_index_list_to_xml(_file: &[u8], _cx: &XmlContext) -> Result<String> {
    todo("slodindexlist.dat")
}

pub fn slod_index_list_from_xml(_xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    todo("slodindexlist.dat")
}

pub fn imposter_extents_to_xml(_file: &[u8], _cx: &XmlContext) -> Result<String> {
    todo("imposterextents.dat")
}

pub fn imposter_extents_from_xml(_xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    todo("imposterextents.dat")
}
