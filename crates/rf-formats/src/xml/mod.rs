//! XML in the format RedForge reads and writes.

pub mod number;
pub mod reader;
pub mod writer;

pub use reader::{XmlDocument, XmlError, XmlNode, parse_hash};
pub use writer::XmlWriter;
