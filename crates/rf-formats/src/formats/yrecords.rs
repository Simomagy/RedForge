//! Vehicle records (yvr) and waypoint records (ywr), from `Rsc8Records.cs`.
//!
//! Not ported yet.

use super::{XmlContext, todo};
use crate::Result;

pub fn yvr_to_xml(_file: &[u8], _cx: &XmlContext) -> Result<String> {
    todo("yvr")
}

pub fn yvr_from_xml(_xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    todo("yvr")
}

pub fn ywr_to_xml(_file: &[u8], _cx: &XmlContext) -> Result<String> {
    todo("ywr")
}

pub fn ywr_from_xml(_xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    todo("ywr")
}
