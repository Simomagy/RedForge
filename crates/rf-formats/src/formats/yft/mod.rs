//! Fragments (yft), from `Rsc8Fragment.cs` and `YftFile.cs`: breakable or
//! articulated models (props, buildings' parts, vehicles, wagons, weapons,
//! peds) with their physics, and environment cloth (flags, curtains).
//!
//! # Structs
//!
//! - [`Fragment`] (the root) has a main [`FragDrawable`] (a
//!   [`DrawableBase`](crate::formats::ydr::DrawableBase) with a matrix and
//!   bounds), more drawables (`drawable_array`, `drawable_cloth`),
//!   [`EnvironmentCloth`] (a drawable and a
//!   [`ClothController`](crate::formats::yld::ClothController) with its
//!   [`VerletCloth`](crate::formats::yld::VerletCloth)), character cloth
//!   ([`CharacterCloth`], as in yld),
//!   [`FragBoneTransforms`], glass panes ([`FragPaneModelInfo`]) and the
//!   physics: a [`FragPhysLodGroup`] of up to 3 [`FragPhysLod`]s.
//! - A [`FragPhysLod`] has children: each a child of its composite
//!   [`Bounds`](crate::formats::ybn::Bounds), a drawable of `drawables1` (and
//!   a damaged one of `drawables2`), masses and inertia; [`FragPhysGroup`]s of
//!   them attached to bones; a [`FragPhysArchetype`] and an articulated body
//!   ([`FragPhysArticulatedBodyType`] of [`FragPhysJoint`]s).
//! - [`Fragment::drawables`] lists every drawable, for loading the fragment
//!   as a model; the main drawable's skeleton is the fragment's.
//!
//! Every struct has `read` (at the reader's position), `write` (adds its
//! block, returns its id), `write_xml` (into the current node) and `read_xml`.
//!
//! # C# behaviour kept
//!
//! - Blocks the C# shares are shared when written: the physics drawables
//!   point to the main drawable's skeleton block, `drawables1`'s drawables
//!   to their LOD's composite children, and the archetype to its LOD's
//!   bounds. From XML, the physics drawables get the main drawable's
//!   skeleton, and `drawables1`'s the composite's children as bounds.
//! - In XML, only the fragment's own drawables (and drawables that are one of
//!   them in the file) have a skeleton, and a drawable's bounds are left out
//!   when they're a composite's child (see [`FragDrawable`]'s
//!   `frag_skeleton_export` and `bounds_in_composite`).
//! - Bone transforms are read as many as the main drawable has bones, and
//!   always written from XML (empty if the XML has none).
//! - Counts come from the arrays when read from XML (`children_count` from
//!   `ChildrenUnkFloats`), groups get their child group index and count, the
//!   articulated body its joint count plus 1 and 22-value arrays.
//! - Group values are half floats; group and controller names ASCII.
//! - Empty raw pointer arrays (`Drawables1`, `Groups`, `Joints` with a count
//!   of 0) are `<X />` in XML and written as a pointer to nothing.
//! - So the first trip through XML drops the skeleton of an environment
//!   cloth's drawable (in the file it is one of `drawable_cloth`, from XML
//!   its own copy), and the next its meshes' bone indices; then it's stable.
//!   Group values out of the half-float range become infinite.
//!
//! # Known issues
//!
//! - Written files aren't byte-identical to the C#'s: blocks of equal size
//!   are laid out in another order (the C#'s sort isn't stable). On 600 game
//!   yfts spread over all of them, and on vehicles, peds, cloth, glass and
//!   drawable arrays, the XML is the C#'s and the files written from it read
//!   back the same with the C#; all 10024 game yfts read.
//! - No game yft has character cloth; it was checked with hand-written XML.
//! - The C# reads the whole file through one block pool, so a block pointed
//!   to twice is one object. This port reads each pointer on its own and
//!   only works out the two sharings that change the XML (above). Other
//!   sharings (an environment cloth's drawable that is one of
//!   `drawable_cloth`...) are written as copies when a fragment read from a
//!   file is written without going through XML.
//! - The C# fails where a raw pointer array has a count and a null pointer,
//!   on joints of unknown type, and on glass panes or physics groups without
//!   their vertex layout or name in XML; so does this port.
//! - A drawable's bounds count as a composite's child only for the physics
//!   LODs' composites (the C# also counts other composites' children; none
//!   was seen holding a drawable's bounds).
//! - Bone transforms written from XML without matrices make the C# read as
//!   many matrices as the skeleton has bones from what follows the block;
//!   what it reads depends on the block layout.

mod drawable;
mod physics;
mod util;

pub use drawable::{EnvironmentCloth, FragBoneTransforms, FragDrawable, FragPaneModelInfo};
pub use physics::{
    FragPhysArchetype, FragPhysArticulatedBodyType, FragPhysGroup, FragPhysJoint, FragPhysLod, FragPhysLodGroup,
    FragTypeConnection, GROUP_VALUE_NAMES, JointType,
};

use std::collections::HashSet;

use super::XmlContext;
use crate::formats::ydr::util::{ArrHeader, node_items_opt, read_array, read_ptr_arr, write_ptr_arr};
use crate::formats::yld::CharacterCloth;
use crate::formats::yld::util::{node_array_with, node_with};
use crate::hash::HashIndex;
use crate::rsc8::{BlockBuf, BlockId, BlockMap, Rsc8Reader, Rsc8Writer, read_resource};
use crate::xml::{XmlDocument, XmlNode, XmlWriter};
use crate::{Result, bail};
use util::{read_raw_ptr_arr, write_raw_ptr_arr};

/// The resource version of yft files.
pub const VERSION: u32 = 186;

/// What reading needs besides the data: the hash index, and what the C#'s
/// block pool shares that changes the XML.
pub struct ReadCx<'a> {
    pub hashes: &'a HashIndex,
    /// Where the fragment's own drawables are.
    pub roots: HashSet<u64>,
    /// Where the physics LODs' composites' children are.
    pub composite_children: HashSet<u64>,
}

impl<'a> ReadCx<'a> {
    /// Finds the fragment's own drawables and the physics composites'
    /// children, for the fragment at `root`.
    pub fn scan(r: &mut Rsc8Reader, root: u64, hashes: &'a HashIndex) -> Result<Self> {
        let mut cx = Self { hashes, roots: HashSet::new(), composite_children: HashSet::new() };
        let at64 = |r: &mut Rsc8Reader, p: u64| r.at(p, |r| r.u64());
        let mut pointers = |r: &mut Rsc8Reader, ptr: u64, n: usize| -> Result<()> {
            if let Some(p) = read_array::<u64>(r, ptr, n)? {
                cx.roots.extend(p.into_iter().filter(|&p| p != 0));
            }
            Ok(())
        };
        let array = at64(r, root + 0x28)?;
        let array_count = r.at(root + 0x38, |r| r.u32())? as usize;
        pointers(r, array, array_count)?;
        let cloth = at64(r, root + 0x98)?;
        let cloth_count = r.at(root + 0xA0, |r| r.u16())? as usize;
        pointers(r, cloth, cloth_count)?;
        let main = at64(r, root + 0x20)?;
        if main != 0 {
            cx.roots.insert(main);
        }
        let group = at64(r, root + 0x90)?;
        if group != 0 {
            for off in [0x10, 0x18, 0x20] {
                let lod = at64(r, group + off)?;
                if lod != 0 {
                    let bounds = at64(r, lod + 0xE0)?;
                    composite_children(r, bounds, &mut cx.composite_children, 0)?;
                }
            }
        }
        Ok(cx)
    }
}

/// Adds the children of the composite bound at `ptr` (and theirs).
fn composite_children(r: &mut Rsc8Reader, ptr: u64, set: &mut HashSet<u64>, depth: usize) -> Result<()> {
    const COMPOSITE: u8 = 6;
    if ptr == 0 || depth > 8 || r.at(ptr + 0x10, |r| r.u8())? != COMPOSITE {
        return Ok(());
    }
    let kids = r.at(ptr + 0x70, |r| r.u64())?;
    let n = r.at(ptr + 0x98, |r| r.u16())? as usize;
    for k in read_array::<u64>(r, kids, n)?.unwrap_or_default() {
        if k != 0 && set.insert(k) {
            composite_children(r, k, set, depth + 1)?;
        }
    }
    Ok(())
}

/// `Rsc8Fragment` (0xC0 bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct Fragment {
    pub vft: u64,
    pub block_map: Option<BlockMap>,
    pub bounding_sphere_center: [f32; 3],
    pub bounding_sphere_radius: f32,
    /// The main drawable, with the fragment's skeleton.
    pub drawable: Option<FragDrawable>,
    /// Read when `drawable_array_count` isn't 0.
    pub drawable_array: Option<Vec<Option<FragDrawable>>>,
    /// One per `drawable_array` item; read when `drawable_array_count` isn't 0.
    pub drawable_array_names: Option<Vec<Option<String>>>,
    pub drawable_array_count: u32,
    /// -1 without a drawable array, 0 with one.
    pub drawable_array_flag: i32,
    pub name: Option<String>,
    pub environment_cloth: Option<Vec<Option<EnvironmentCloth>>>,
    pub environment_cloth_capacity: u16,
    pub environment_cloth_unknown: u32,
    pub character_cloth: Option<Vec<Option<CharacterCloth>>>,
    pub character_cloth_capacity: u16,
    pub character_cloth_unknown: u32,
    pub unknown_68h: u64,
    pub bone_transforms: Option<FragBoneTransforms>,
    pub unknown_78h: u8,
    /// The NaturalMotion asset (peds), 255 for none.
    pub nm_asset_id: u8,
    pub pane_model_infos_count: u16,
    pub break_and_damage_flags: u32,
    pub unknown_80h: u32,
    pub unknown_84h: f32,
    /// Read when `pane_model_infos_count` isn't 0.
    pub pane_model_infos: Option<Vec<Option<FragPaneModelInfo>>>,
    pub physics_lod_group: Option<FragPhysLodGroup>,
    /// The environment cloth's drawables.
    pub drawable_cloth: Option<Vec<Option<FragDrawable>>>,
    pub drawable_cloth_capacity: u16,
    pub drawable_cloth_unknown: u32,
    pub unknown_a8h: u64,
    pub vehicle_glass_windows_pointer: u64,
    pub unknown_b8h: u64,
}

impl Default for Fragment {
    fn default() -> Self {
        Self {
            vft: 0,
            block_map: None,
            bounding_sphere_center: [0.0; 3],
            bounding_sphere_radius: 0.0,
            drawable: None,
            drawable_array: None,
            drawable_array_names: None,
            drawable_array_count: 0,
            drawable_array_flag: 0,
            name: None,
            environment_cloth: None,
            environment_cloth_capacity: 0,
            environment_cloth_unknown: 0,
            character_cloth: None,
            character_cloth_capacity: 0,
            character_cloth_unknown: 0,
            unknown_68h: 0,
            bone_transforms: None,
            unknown_78h: 0,
            nm_asset_id: 0,
            pane_model_infos_count: 0,
            break_and_damage_flags: 0,
            unknown_80h: 0xFFFF_FFFF,
            unknown_84h: 0.0,
            pane_model_infos: None,
            physics_lod_group: None,
            drawable_cloth: None,
            drawable_cloth_capacity: 0,
            drawable_cloth_unknown: 0,
            unknown_a8h: 0,
            vehicle_glass_windows_pointer: 0,
            unknown_b8h: 0,
        }
    }
}

/// Writes items that may fail into blocks.
fn write_items<T>(
    w: &mut Rsc8Writer,
    items: &Option<Vec<Option<T>>>,
    mut f: impl FnMut(&T, &mut Rsc8Writer) -> Result<BlockId>,
) -> Result<Option<Vec<Option<BlockId>>>> {
    let Some(items) = items else { return Ok(None) };
    let ids = items.iter().map(|x| x.as_ref().map(|x| f(x, w)).transpose()).collect::<Result<Vec<_>>>()?;
    Ok(Some(ids))
}

impl Fragment {
    pub const BLOCK_LENGTH: usize = 0xC0;

    /// Reads the fragment at the reader's position. Texture names are added to
    /// `hashes`.
    pub fn read(r: &mut Rsc8Reader, hashes: &HashIndex) -> Result<Self> {
        let root = r.pos;
        let cx = ReadCx::scan(r, root, hashes)?;
        let mut f = Self { vft: r.u64()?, ..Default::default() };
        f.block_map = r.ptr(BlockMap::read)?;
        f.bounding_sphere_center = r.vec3()?;
        f.bounding_sphere_radius = r.f32()?;
        f.drawable = r.ptr(|r| FragDrawable::read(r, &cx))?;
        let array_ptr = r.u64()?;
        let names_ptr = r.u64()?;
        f.drawable_array_count = r.u32()?;
        f.drawable_array_flag = r.i32()?;
        let name_ptr = r.u64()?;
        f.name = r.string_at(name_ptr)?;
        let h = ArrHeader::read(r)?;
        f.environment_cloth = read_ptr_arr(r, &h, |r| EnvironmentCloth::read(r, &cx))?;
        f.environment_cloth_capacity = h.capacity;
        f.environment_cloth_unknown = h.unknown;
        let h = ArrHeader::read(r)?;
        f.character_cloth = read_ptr_arr(r, &h, CharacterCloth::read)?;
        f.character_cloth_capacity = h.capacity;
        f.character_cloth_unknown = h.unknown;
        f.unknown_68h = r.u64()?;
        let bones = f.skeleton().and_then(|s| s.bones.as_ref()).map_or(0, |b| b.len());
        f.bone_transforms = r.ptr(|r| FragBoneTransforms::read(r, bones))?;
        f.unknown_78h = r.u8()?;
        f.nm_asset_id = r.u8()?;
        f.pane_model_infos_count = r.u16()?;
        f.break_and_damage_flags = r.u32()?;
        f.unknown_80h = r.u32()?;
        f.unknown_84h = r.f32()?;
        let panes_ptr = r.u64()?;
        f.physics_lod_group = r.ptr(|r| FragPhysLodGroup::read(r, &cx))?;
        let h = ArrHeader::read(r)?;
        f.drawable_cloth = read_ptr_arr(r, &h, |r| FragDrawable::read(r, &cx))?;
        f.drawable_cloth_capacity = h.capacity;
        f.drawable_cloth_unknown = h.unknown;
        f.unknown_a8h = r.u64()?;
        f.vehicle_glass_windows_pointer = r.u64()?;
        f.unknown_b8h = r.u64()?;

        if f.pane_model_infos_count != 0 {
            let n = f.pane_model_infos_count as usize;
            f.pane_model_infos = read_raw_ptr_arr(r, panes_ptr, n, FragPaneModelInfo::read)?;
        }
        if f.drawable_array_count != 0 {
            let n = f.drawable_array_count as usize;
            f.drawable_array = read_raw_ptr_arr(r, array_ptr, n, |r| FragDrawable::read(r, &cx))?;
            f.drawable_array_names = read_raw_ptr_arr(r, names_ptr, n, |r| r.string_here())?;
        }
        Ok(f)
    }

    /// The fragment's skeleton: its main drawable's.
    pub fn skeleton(&self) -> Option<&crate::formats::ydr::Skeleton> {
        self.drawable.as_ref().and_then(|d| d.base.skeleton.as_ref())
    }

    /// Every drawable that isn't null: the main one, the drawable array, the
    /// cloth drawables, then each physics LOD's.
    pub fn drawables(&self) -> impl Iterator<Item = &FragDrawable> {
        let lods = self.physics_lod_group.iter().flat_map(|g| g.lods()).flatten();
        let phys = lods.flat_map(|l| l.drawables1.iter().chain(l.drawables2.iter()).flatten().flatten());
        self.drawable
            .iter()
            .chain(self.drawable_array.iter().flatten().flatten())
            .chain(self.drawable_cloth.iter().flatten().flatten())
            .chain(phys)
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> Result<BlockId> {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u64(self.vft);
        b.ptr(self.block_map.as_ref().map(|m| m.write(w)));
        b.vec3(self.bounding_sphere_center);
        b.f32(self.bounding_sphere_radius);
        // The skeleton first, for the physics drawables to share.
        let skeleton = self.skeleton();
        let skel_id = skeleton.map(|s| s.write(w));
        b.ptr(self.drawable.as_ref().map(|d| d.write(w, skel_id, None)).transpose()?);
        let array = write_items(w, &self.drawable_array, |d, w| d.write(w, None, None))?;
        b.ptr(write_raw_ptr_arr(w, array.as_deref()));
        let names: Option<Vec<Option<BlockId>>> =
            self.drawable_array_names.as_ref().map(|n| n.iter().map(|s| s.as_deref().map(|s| w.string(s))).collect());
        b.ptr(write_raw_ptr_arr(w, names.as_deref()));
        b.u32(self.drawable_array_count);
        b.i32(self.drawable_array_flag);
        b.ptr(w.opt_string(self.name.as_deref()));
        let env = write_items(w, &self.environment_cloth, |c, w| c.write(w))?;
        write_ptr_arr(&mut b, w, env.as_deref(), self.environment_cloth_capacity, self.environment_cloth_unknown);
        let chars = write_items(w, &self.character_cloth, |c, w| c.write(w))?;
        write_ptr_arr(&mut b, w, chars.as_deref(), self.character_cloth_capacity, self.character_cloth_unknown);
        b.u64(self.unknown_68h);
        b.ptr(self.bone_transforms.as_ref().map(|t| t.write(w)));
        b.u8(self.unknown_78h);
        b.u8(self.nm_asset_id);
        b.u16(self.pane_model_infos_count);
        b.u32(self.break_and_damage_flags);
        b.u32(self.unknown_80h);
        b.f32(self.unknown_84h);
        let panes = write_items(w, &self.pane_model_infos, |p, w| Ok(p.write(w)))?;
        b.ptr(write_raw_ptr_arr(w, panes.as_deref()));
        b.ptr(self.physics_lod_group.as_ref().map(|g| g.write(w, (skeleton, skel_id))).transpose()?);
        let cloth = write_items(w, &self.drawable_cloth, |d, w| d.write(w, None, None))?;
        write_ptr_arr(&mut b, w, cloth.as_deref(), self.drawable_cloth_capacity, self.drawable_cloth_unknown);
        b.u64(self.unknown_a8h);
        b.u64(self.vehicle_glass_windows_pointer);
        b.u64(self.unknown_b8h);
        w.set(id, b, false);
        Ok(id)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) -> Result<()> {
        w.u32("@version", 0);
        w.string("Name", self.name.as_deref().unwrap_or(""));
        w.vec3("BoundingSphereCenter", self.bounding_sphere_center);
        w.f32("BoundingSphereRadius", self.bounding_sphere_radius);
        w.u8("NmAssetID", self.nm_asset_id);
        w.u32("BreakAndDamageFlags", self.break_and_damage_flags);
        w.f32("Unknown_84h", self.unknown_84h);
        if let Some(d) = &self.drawable {
            node_with(w, "Drawable", |w| d.write_xml(w, true))?;
        }
        if let Some(names) = &self.drawable_array_names {
            let Some(names) = names.iter().cloned().collect::<Option<Vec<String>>>() else {
                bail!("a null drawable array name (the C# fails on it)");
            };
            w.string_array("DrawableArrayNames", Some(&names));
        }
        node_array_with(w, "DrawableArray", self.drawable_array.as_deref(), |w, d| d.write_xml(w, true))?;
        node_array_with(w, "EnvironmentCloth", self.environment_cloth.as_deref(), |w, c| c.write_xml(w))?;
        node_array_with(w, "CharacterCloth", self.character_cloth.as_deref(), |w, c| {
            c.write_xml(w);
            Ok(())
        })?;
        w.matrix3x4_array("BoneTransforms", self.bone_transforms.as_ref().and_then(|t| t.items.as_deref()));
        node_array_with(w, "PaneModelInfos", self.pane_model_infos.as_deref(), |w, p| {
            p.write_xml(w);
            Ok(())
        })?;
        if let Some(g) = &self.physics_lod_group {
            node_with(w, "PhysicsLODGroup", |w| g.write_xml(w))?;
        }
        node_array_with(w, "DrawableCloth", self.drawable_cloth.as_deref(), |w, d| d.write_xml(w, true))?;
        Ok(())
    }

    /// Reads the XML and derives what the C# derives: the counts, and the
    /// physics drawables get the main drawable's skeleton.
    pub fn read_xml(n: XmlNode) -> Result<Self> {
        let roots = |v: Option<Vec<Option<FragDrawable>>>| {
            v.map(|v| {
                v.into_iter().map(|d| d.map(|d| FragDrawable { frag_skeleton_export: true, ..d })).collect::<Vec<_>>()
            })
        };
        let mut f = Self {
            name: n.string("Name"),
            bounding_sphere_center: n.vec3("BoundingSphereCenter"),
            bounding_sphere_radius: n.f32("BoundingSphereRadius", 0.0),
            nm_asset_id: n.u8("NmAssetID", 0),
            break_and_damage_flags: n.u32("BreakAndDamageFlags", 0),
            unknown_84h: n.f32("Unknown_84h", 0.0),
            drawable: n
                .node("Drawable")
                .map(FragDrawable::read_xml)
                .transpose()?
                .map(|d| FragDrawable { frag_skeleton_export: true, ..d }),
            drawable_array_names: n.string_array("DrawableArrayNames").map(|v| v.into_iter().map(Some).collect()),
            drawable_array: roots(node_items_opt(n, "DrawableArray", FragDrawable::read_xml)?),
            environment_cloth: node_items_opt(n, "EnvironmentCloth", EnvironmentCloth::read_xml)?,
            character_cloth: node_items_opt(n, "CharacterCloth", CharacterCloth::read_xml)?,
            bone_transforms: Some(FragBoneTransforms::new(n.matrix_array::<12>("BoneTransforms"))),
            pane_model_infos: node_items_opt(n, "PaneModelInfos", FragPaneModelInfo::read_xml)?,
            physics_lod_group: n.node("PhysicsLODGroup").map(FragPhysLodGroup::read_xml).transpose()?,
            drawable_cloth: roots(node_items_opt(n, "DrawableCloth", FragDrawable::read_xml)?),
            ..Default::default()
        };
        f.drawable_array_count = f.drawable_array.as_ref().map_or(0, |d| d.len() as u32);
        f.drawable_array_flag = if f.drawable_array_count == 0 { -1 } else { 0 };
        f.environment_cloth_capacity = f.environment_cloth.as_ref().map_or(0, |c| c.len() as u16);
        f.character_cloth_capacity = f.character_cloth.as_ref().map_or(0, |c| c.len() as u16);
        f.drawable_cloth_capacity = f.drawable_cloth.as_ref().map_or(0, |c| c.len() as u16);
        f.pane_model_infos_count = f.pane_model_infos.as_ref().map_or(0, |p| p.len() as u16);
        let skeleton = f.skeleton().cloned();
        if let Some(g) = &mut f.physics_lod_group {
            for lod in g.lods_mut().into_iter().flatten() {
                for d in lod.drawables_mut() {
                    d.base.skeleton = skeleton.clone();
                }
            }
        }
        Ok(f)
    }
}

/// Reads a yft (a loose resource).
pub fn read(file: &[u8], cx: &XmlContext) -> Result<Fragment> {
    read_resource(file, |r, _| Fragment::read(r, cx.hashes))
}

/// Builds a yft, as `YftFile.Save` does (with a block map).
pub fn write(fragment: &Fragment) -> Result<Vec<u8>> {
    let mut w = Rsc8Writer::new();
    if fragment.block_map.is_some() {
        fragment.write(&mut w)?;
    } else {
        let mut f = fragment.clone();
        f.block_map = Some(BlockMap::default());
        f.write(&mut w)?;
    }
    Ok(w.build(VERSION))
}

pub fn to_xml(file: &[u8], cx: &XmlContext) -> Result<String> {
    let f = read(file, cx)?;
    let mut res = Ok(());
    let mut errors = Vec::new();
    let xml = XmlWriter::document(cx.hashes, "RDR2Fragment", cx.folder.clone(), |w| {
        res = f.write_xml(w);
        errors = std::mem::take(&mut w.external_errors);
    });
    res?;
    if !errors.is_empty() {
        bail!("{}", errors.join("; "));
    }
    Ok(xml)
}

pub fn from_xml(xml: &str, cx: &XmlContext) -> Result<Vec<u8>> {
    let doc = XmlDocument::new(xml);
    let f = doc.with_root(cx.folder.as_deref(), Fragment::read_xml)??;
    write(&f)
}
