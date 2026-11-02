//! Shader effects databases (fxdb), from `FxdbFile.cs`. Read and XML export only.
//!
//! Not ported yet.

use super::{XmlContext, todo};
use crate::Result;

/// The `SGDB` and `SGD1` magics.
pub fn is_magic(m: u32) -> bool {
    m == u32::from_le_bytes(*b"SGDB") || m == u32::from_le_bytes(*b"SGD1")
}

pub fn to_xml(_file: &[u8], _cx: &XmlContext) -> Result<String> {
    todo("fxdb")
}
