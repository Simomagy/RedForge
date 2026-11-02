//! Resource metadata (ymap, ytyp, ymt), from `Rsc8Meta.cs` and `DataBag.cs`.
//!
//! Not ported yet.

use super::{XmlContext, todo};
use crate::Result;

pub fn to_xml(_file: &[u8], _cx: &XmlContext) -> Result<String> {
    todo("resource meta")
}

pub fn from_xml(_xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    todo("resource meta")
}
