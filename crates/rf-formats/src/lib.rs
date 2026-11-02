//! RDR2 game file formats for RedForge.
//!
//! This crate knows nothing about rendering or the editor: it reads and writes
//! the game's files as plain data.

pub mod dotnet;
mod error;
pub mod formats;
pub mod hash;
pub mod rpf8;
pub mod rsc8;
pub mod xml;

pub use error::{Error, Result};
