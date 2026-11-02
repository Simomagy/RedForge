//! Clip dictionaries (ycd) and animation dictionaries (yad), from `Rsc8Animation.cs`.
//!
//! Not ported yet.

use super::{XmlContext, todo};
use crate::Result;

pub fn to_xml(_file: &[u8], _cx: &XmlContext) -> Result<String> {
    todo("ycd")
}

pub fn from_xml(_xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    todo("ycd")
}

pub fn yad_to_xml(_file: &[u8], _cx: &XmlContext) -> Result<String> {
    todo("yad")
}

pub fn yad_from_xml(_xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    todo("yad")
}
