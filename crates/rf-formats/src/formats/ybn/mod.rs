//! Collision bounds: static collision files (ybn), from `Rsc8Bounds.cs` and
//! `YbnFile.cs`. Drawables and fragments embed the same bounds.
//!
//! # Using bounds from another format
//!
//! - [`Bounds::read_at`]`(r, ptr)` reads the bound a pointer points to, with its
//!   children (`ReadBlock(Rsc8Bounds.Create)`); [`Bounds::read`] reads the one
//!   at the reader's position.
//! - [`Bounds::write`]`(w)` adds it to a writer and returns its block. It
//!   recomputes what the C# recomputes on saving (quantized vertices, octants,
//!   edges, BVHs, a composite's matrix and flag arrays), so it can fail where
//!   the C# throws (a geometry without vertices or materials).
//! - [`Bounds::write_xml`]`(w)` writes the `type`/`version` attributes and the
//!   fields into the current node, e.g. inside `w.node("Bounds", ..)`;
//!   [`Bounds::read_xml`]`(node)` reads one back from such a node.
//!
//! A bound is the common fields of [`Bounds`] and a [`BoundsShape`] with the
//! data of its type. A composite's children carry their placement in their
//! own `transform` and `composite_flags1`/`2`.
//!
//! The C# never loads the game's material names when converting, so material
//! names in XML are the 16-bit material hashes in decimal; reading XML hashes
//! the name again, as the C# does (see [`material_hash`]).
//!
//! # Known issues
//!
//! - Because of the above, material hashes change on every trip through XML,
//!   exactly as with the C# converter. Writing real names needs the game's
//!   `materials.meta`.
//! - An array shorter than its count (say fewer vertex colours than vertices
//!   in hand-written XML) is read past its end, by the C# too; what is there
//!   depends on the block layout, which isn't byte-identical to the C#'s.
//! - The game's ybns only hold composites of BVH geometries. Other types were
//!   checked against the C# with drawables' bounds and hand-written XML, not
//!   with fragments' (yft) bounds.

mod bounds;
mod bvh;
mod geometry;
mod material;
mod math;
mod reader;

pub use bounds::{
    BoundCapsule, BoundComposite, BoundCylinder, BoundDisc, BoundTaperedCapsule, Bounds, BoundsShape, BoundsType,
};
pub use bvh::{Bvh, BvhNode};
pub use geometry::{
    BoundBvhPart, BoundGeometry, BoundPolygon, PolyBox, PolyCapsule, PolySphere, PolyTriangle, PolygonKind,
};
pub use material::{
    BOUND_FLAG_NAMES, BoundMaterial, BoundMaterial1, BoundMaterial2, CompositeFlags, MATERIAL_FLAG_NAMES, material_hash,
};
pub use math::{IDENTITY, Mat4, Vec3};

use super::XmlContext;
use crate::Result;
use crate::rsc8::{self, BlockMap, Rsc8Writer};
use crate::xml::{XmlDocument, XmlWriter};

/// The resource version of ybn files.
pub const VERSION: u32 = 47;

/// Reads a ybn (a loose resource) into its root bound.
pub fn read(file: &[u8]) -> Result<Bounds> {
    rsc8::read_resource(file, |r, _| Bounds::read(r))
}

/// Builds a ybn from its root bound, as `YbnFile.Save` does.
pub fn write(bounds: &Bounds) -> Result<Vec<u8>> {
    let mut w = Rsc8Writer::new();
    if bounds.block_map.is_some() {
        bounds.write(&mut w)?;
    } else {
        let mut b = bounds.clone();
        b.block_map = Some(BlockMap::default());
        b.write(&mut w)?;
    }
    Ok(w.build(VERSION))
}

pub fn to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let bounds = read(file)?;
    Ok(XmlWriter::document(cx.hashes, "RDR2Bounds", None, |w| bounds.write_xml(w)))
}

pub fn from_xml(xml: &str, _cx: &XmlContext) -> Result<Vec<u8>> {
    let doc = XmlDocument::new(xml);
    let bounds = doc.with_root(None, |root| Bounds::read_xml(&root))??;
    write(&bounds)
}
