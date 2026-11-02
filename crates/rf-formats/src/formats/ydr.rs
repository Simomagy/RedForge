//! Drawables (ydr) and drawable dictionaries (ydd), from `Rsc8Drawable.cs`.
//!
//! Not ported yet.

use super::{XmlContext, todo};
use crate::Result;

pub fn to_xml(_file: &[u8], _cx: &XmlContext) -> Result<String> {
    todo("ydr")
}

pub fn from_xml(_xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    todo("ydr")
}

pub fn ydd_to_xml(_file: &[u8], _cx: &XmlContext) -> Result<String> {
    todo("ydd")
}

pub fn ydd_from_xml(_xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    todo("ydd")
}
