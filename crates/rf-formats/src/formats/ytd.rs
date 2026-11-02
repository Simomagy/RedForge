//! Texture dictionaries (ytd), from `Rsc8Texture.cs` and `YtdFile.cs`.

use super::{XmlContext, todo};
use crate::Result;

pub fn to_xml(_file: &[u8], _cx: &XmlContext) -> Result<String> {
    todo("ytd")
}

pub fn from_xml(_xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    todo("ytd")
}
