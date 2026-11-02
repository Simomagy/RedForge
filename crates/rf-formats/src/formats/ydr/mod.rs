//! Drawables (ydr) and drawable dictionaries (ydd), from `Rsc8Drawable.cs`,
//! `YdrFile.cs` and `YddFile.cs`, with the XML of the engine classes they
//! derive from (`Mesh.cs` for geometries and their vertices, `VertexLayout.cs`).
//!
//! # Structs
//!
//! - [`Drawable`] (a ydr's root, a ydd's items) is a [`DrawableBase`] plus its
//!   collision [`Bounds`](crate::formats::ybn::Bounds) and an optional
//!   [`SamplerDictionary`]. Fragments' drawables derive from [`DrawableBase`]
//!   too: [`DrawableBase::write_fields`] writes its 176 bytes into a block a
//!   derived struct goes on filling.
//! - [`DrawableBase`] has a [`ShaderGroup`] (the [`Shader`]s and an embedded
//!   [`TextureDictionary`](crate::formats::ytd::TextureDictionary)), a
//!   [`Skeleton`] of [`Bone`]s, [`JointLimits`], and 4 [`DrawableLod`]s of
//!   [`DrawableModel`]s, each of [`Geometry`]s.
//! - A [`Geometry`] has its vertex and index buffers and the engine
//!   [`VertexLayout`] of its vertex data (non-interleaved as stored in files:
//!   all the positions, then all the normals...); [`Geometry::vertex_value`]
//!   decodes one element. Its `name` (the shader's), `bone_index` and
//!   `topology` are the engine mesh's, set when the drawable is read
//!   ([`DrawableBase::set_skeleton`], [`DrawableBase::assign_shaders`]).
//! - Shader texture parameters point to a [`ShaderTexture`]: one of the
//!   group's own textures, or a texture by name.
//! - [`Skeleton::bone_by_tag`] finds a bone as the C#'s `BonesMap` does (for
//!   cloth, animations).
//!
//! Every struct has `read` (at the reader's position), `write` (adds its block,
//! returns its id), `write_xml` (into the current node) and `read_xml`. What
//! the C# derives when reading XML is derived too: shader parameter data and
//! sizes, models' bounds and shader mappings, LOD masks, the skeleton's parent
//! indices, bone tag map and bind poses (the bind pose math only matters for
//! the game: the XML only has their 4th column).
//!
//! # C# behaviour kept
//!
//! - Names in the XML (`Hash`, shader and parameter names, skeleton hashes)
//!   are read back by hashing the text as it is (`new JenkHash(string)`), not
//!   by parsing `0x...` names; collision names hash back to the same value.
//! - Vertex data is read and written through the engine's formats (`Dec3N`,
//!   halves...), including `Vector4ToShort4N`/`Vector2ToShort2N` taking every
//!   component's sign from X.
//! - A shader parameter's `ParamInfo.Type` setter also clears bit 2.
//! - Parameters naming a texture of the group's dictionary (by lowercased
//!   name) point to it in the file.
//! - A model with one geometry stores that geometry's bounds as its own, so a
//!   model's `BoundingBox` from XML is lost then.
//! - The first trip through XML can change some values, then they're stable:
//!   a `Dec3N` component stored as "-0" is written back as 0, and collision
//!   material names are re-hashed on every trip (see `ybn`).
//!
//! # Known issues
//!
//! - The C# fails to write shaders with no constant buffers from XML (no
//!   `BufferSizes`, a `NullReferenceException`; about 2% of game ydrs, all
//!   `out_*` map pieces); this port writes them, and the C# reads them back
//!   as expected.
//! - The C#'s `List.Sort` isn't stable: drawables (or samplers) with equal
//!   name hashes may come out in another order.
//! - Arrays the C#'s block pool shares (two pointers to one array, read with
//!   different counts) are read with each count here.
//! - Writing a drawable read from a file (not from XML) writes each of a
//!   shader's 4 copies of an external texture as its own block.
//! - `Rsc8DrawablePtfx` (particle drawables) isn't used by ydr/ydd and isn't
//!   ported; the C# can't write it either.

mod drawable;
mod geometry;
mod sampler;
mod shader;
mod skeleton;
pub(crate) mod util;
mod vertex;

pub use drawable::{Drawable, DrawableBase, DrawableDictionary, DrawableLod, DrawableModel};
pub use geometry::{Geometry, IndexBuffer, Topology, VertexBuffer, VertexFormat, file_format};
pub use sampler::{Arr, DrawableSampler, SamplerDictionary, SamplerTri};
pub use shader::{
    Shader, ShaderGroup, ShaderParam, ShaderParamData, ShaderParamInfo, ShaderParamType, ShaderParams, ShaderTexture,
};
pub use skeleton::{
    BONE_FLAG_NAMES, Bone, JointLimit, JointLimits, Matrix4, Skeleton, SkeletonDebugBlock, SkeletonMappingBlock,
};
pub use vertex::{VertexLayout, f32_to_half, format_size, half_to_f32, semantic, vertex_format};

use super::XmlContext;
use crate::rsc8::{BlockMap, Rsc8Writer, read_resource};
use crate::xml::{XmlDocument, XmlWriter};
use crate::{Result, bail};

/// The resource version of ydr and ydd files.
pub const VERSION: u32 = 162;

/// Reads a ydr (a loose resource).
pub fn read(file: &[u8], cx: &XmlContext) -> Result<Drawable> {
    read_resource(file, |r, _| Drawable::read(r, cx.hashes))
}

/// Builds a ydr, as `YdrFile.Save` does (with a block map).
pub fn write(drawable: &Drawable) -> Result<Vec<u8>> {
    let mut w = Rsc8Writer::new();
    if drawable.base.block_map.is_some() {
        drawable.write(&mut w)?;
    } else {
        let mut d = drawable.clone();
        d.base.block_map = Some(BlockMap::default());
        d.write(&mut w)?;
    }
    Ok(w.build(VERSION))
}

/// Reads a ydd (a loose resource).
pub fn read_dictionary(file: &[u8], cx: &XmlContext) -> Result<DrawableDictionary> {
    read_resource(file, |r, _| DrawableDictionary::read(r, cx.hashes))
}

/// Builds a ydd, as `YddFile.Save` does (with a block map).
pub fn write_dictionary(dict: &DrawableDictionary) -> Result<Vec<u8>> {
    let mut w = Rsc8Writer::new();
    if dict.block_map.is_some() {
        dict.write(&mut w)?;
    } else {
        let mut d = dict.clone();
        d.block_map = Some(BlockMap::default());
        d.write(&mut w)?;
    }
    Ok(w.build(VERSION))
}

/// Writes a document with `f`, failing on its errors and on the DDS files
/// that couldn't be made (the C# fails then).
fn document(cx: &XmlContext, root: &str, f: impl FnOnce(&mut XmlWriter) -> Result<()>) -> Result<String> {
    let mut res = Ok(());
    let mut errors = Vec::new();
    let xml = XmlWriter::document(cx.hashes, root, cx.folder.clone(), |w| {
        res = f(w);
        errors = std::mem::take(&mut w.external_errors);
    });
    res?;
    if !errors.is_empty() {
        bail!("{}", errors.join("; "));
    }
    Ok(xml)
}

pub fn to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let d = read(file, cx)?;
    document(cx, "RDR2Drawable", |w| d.write_xml(w))
}

pub fn from_xml(xml: &str, cx: &XmlContext) -> Result<Vec<u8>> {
    let doc = XmlDocument::new(xml);
    let d = doc.with_root(cx.folder.as_deref(), Drawable::read_xml)??;
    write(&d)
}

pub fn ydd_to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let d = read_dictionary(file, cx)?;
    document(cx, "RDR2DrawableDictionary", |w| d.write_xml(w))
}

pub fn ydd_from_xml(xml: &str, cx: &XmlContext) -> Result<Vec<u8>> {
    let doc = XmlDocument::new(xml);
    let d = doc.with_root(cx.folder.as_deref(), DrawableDictionary::read_xml)??;
    write_dictionary(&d)
}
