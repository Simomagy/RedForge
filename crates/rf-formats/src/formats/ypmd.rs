//! Pose matcher dictionaries (ypmd), from `Rsc8PoseMatcher.cs`.
//!
//! Not ported yet.

use super::{XmlContext, todo};
use crate::Result;

pub fn to_xml(_file: &[u8], _cx: &XmlContext) -> Result<String> {
    todo("ypmd")
}

pub fn from_xml(_xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    todo("ypmd")
}
