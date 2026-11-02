//! Drawables (`Rsc8DrawableBase`, `Rsc8Drawable`), their LODs and models, and
//! drawable dictionaries (`Rsc8DrawableDictionary`).

use super::geometry::{Geometry, Topology};
use super::sampler::SamplerDictionary;
use super::shader::ShaderGroup;
use super::skeleton::{JointLimits, Skeleton};
use super::util::{ArrHeader, node_items, node_items_opt, read_array, read_ptr_arr, write_arr, write_ptr_arr};
use crate::formats::ybn::Bounds;
use crate::hash::{HashIndex, joaat};
use crate::rsc8::{BlockBuf, BlockId, BlockMap, Rsc8Reader, Rsc8Writer};
use crate::xml::{XmlNode, XmlWriter};
use crate::{Result, bail};

/// Runs `f` inside a node, returning its result.
pub(super) fn node_with(w: &mut XmlWriter, id: &str, f: impl FnOnce(&mut XmlWriter) -> Result<()>) -> Result<()> {
    let mut res = Ok(());
    w.node(id, |w| res = f(w));
    res
}

/// `WriteNodeArray` of fallible items, keeping null items.
pub(super) fn node_array_with<T>(
    w: &mut XmlWriter,
    id: &str,
    items: Option<&[Option<T>]>,
    mut f: impl FnMut(&mut XmlWriter, &T) -> Result<()>,
) -> Result<()> {
    let mut res = Ok(());
    w.node_array_opt(id, items, |w, x| {
        if res.is_ok() {
            res = f(w, x);
        }
    });
    res
}

/// `Rsc8DrawableModel` (64 bytes): geometries drawn with one bone or skin.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawableModel {
    pub vft: u64,
    /// `None` for a null pointer or no geometries.
    pub geometries: Option<Vec<Option<Geometry>>>,
    pub geometries_capacity: u16,
    pub geometries_unknown: u32,
    /// Min and max (with a W) of the whole model, then of each geometry when
    /// there are several.
    pub geometry_bounds: Option<Vec<[f32; 8]>>,
    pub shader_mappings: Option<Vec<u16>>,
    /// Only in skinned ped components.
    pub bone_mapping: Option<Vec<u16>>,
    pub bones_count: u8,
    pub has_skin: u8,
    pub bind_unk: u8,
    pub bone_index: u8,
    /// Which views it's drawn in (main, shadows, reflections...).
    pub mask: u8,
    pub flags: u8,
    pub shader_mapping_count: u16,
    pub unknown_38h: u64,
    /// From the first geometry bounds.
    pub bounding_box: [[f32; 3]; 2],
}

impl Default for DrawableModel {
    fn default() -> Self {
        Self {
            vft: 0x0000_0001_4061_0A98,
            geometries: None,
            geometries_capacity: 0,
            geometries_unknown: 0,
            geometry_bounds: None,
            shader_mappings: None,
            bone_mapping: None,
            bones_count: 0,
            has_skin: 0,
            bind_unk: 0,
            bone_index: 0,
            mask: 0,
            flags: 0,
            shader_mapping_count: 0,
            unknown_38h: 0,
            bounding_box: [[0.0; 3]; 2],
        }
    }
}

impl DrawableModel {
    pub const BLOCK_LENGTH: usize = 64;

    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let mut m = Self { vft: r.u64()?, ..Default::default() };
        let gh = ArrHeader::read(r)?;
        m.geometries = read_ptr_arr(r, &gh, Geometry::read)?;
        m.geometries_capacity = gh.capacity;
        m.geometries_unknown = gh.unknown;
        let bounds_ptr = r.u64()?;
        let smap_ptr = r.u64()?;
        let bmap_ptr = r.u64()?;
        m.bones_count = r.u8()?;
        m.has_skin = r.u8()?;
        m.bind_unk = r.u8()?;
        m.bone_index = r.u8()?;
        m.mask = r.u8()?;
        m.flags = r.u8()?;
        m.shader_mapping_count = r.u16()?;
        m.unknown_38h = r.u64()?;
        let gc = gh.count as usize;
        m.geometry_bounds = read_array(r, bounds_ptr, if gc > 1 { gc + 1 } else { gc })?;
        m.shader_mappings = read_array(r, smap_ptr, m.shader_mapping_count as usize)?;
        m.bone_mapping = read_array(r, bmap_ptr, m.bones_count as usize)?;

        if let Some(geoms) = &mut m.geometries {
            let smap = m.shader_mappings.as_deref();
            let bdat = m.geometry_bounds.as_deref();
            for (i, g) in geoms.iter_mut().enumerate() {
                let Some(g) = g else { continue };
                g.shader_id = smap.and_then(|s| s.get(i).copied()).unwrap_or(0);
                let b = match bdat {
                    Some(b) if b.len() > 1 && i + 1 < b.len() => b[i + 1],
                    Some(b) => b[0],
                    None => [0.0; 8],
                };
                g.aabb = [[b[0], b[1], b[2], b[3]], [b[4], b[5], b[6], b[7]]];
            }
            if let Some(b) = bdat.and_then(|b| b.first()) {
                m.bounding_box = [[b[0], b[1], b[2]], [b[4], b[5], b[6]]];
            }
        }
        Ok(m)
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u64(self.vft);
        let geoms: Option<Vec<Option<BlockId>>> =
            self.geometries.as_ref().map(|v| v.iter().map(|g| g.as_ref().map(|g| g.write(w))).collect());
        write_ptr_arr(&mut b, w, geoms.as_deref(), self.geometries_capacity, self.geometries_unknown);
        b.ptr(w.array(self.geometry_bounds.as_deref()));
        b.ptr(w.array(self.shader_mappings.as_deref()));
        b.ptr(w.array(self.bone_mapping.as_deref()));
        b.u8(self.bones_count);
        b.u8(self.has_skin);
        b.u8(self.bind_unk);
        b.u8(self.bone_index);
        b.u8(self.mask);
        b.u8(self.flags);
        b.u16(self.shader_mapping_count);
        b.u64(self.unknown_38h);
        w.set(id, b, false);
        id
    }

    pub fn write_xml(&self, w: &mut XmlWriter) -> Result<()> {
        if self.mask != 0xFF {
            w.u8("Mask", self.mask);
        }
        if self.flags != 0 {
            w.u8("Flags", self.flags);
        }
        if self.has_skin == 1 {
            w.bool("HasSkin", true);
        }
        if self.bone_index != 0 {
            w.u8("BoneIndex", self.bone_index);
        }
        w.u16_array("BoneMapping", self.bone_mapping.as_deref());
        w.vec3("BoundingBoxMin", self.bounding_box[0]);
        w.vec3("BoundingBoxMax", self.bounding_box[1]);
        node_array_with(w, "Geometries", self.geometries.as_deref(), |w, g| g.write_xml(w))
    }

    pub fn read_xml(node: XmlNode) -> Result<Self> {
        let mut m = Self {
            mask: node.u8("Mask", 0xFF),
            flags: node.u8("Flags", 0),
            has_skin: node.bool("HasSkin", false) as u8,
            bone_index: node.u8("BoneIndex", 0),
            bone_mapping: node.u16_array("BoneMapping"),
            bounding_box: [node.vec3("BoundingBoxMin"), node.vec3("BoundingBoxMax")],
            ..Default::default()
        };
        let geoms = node_items(node, "Geometries", Geometry::read_xml)?;
        let gcnt = geoms.as_ref().map_or(0, |g| g.len());
        if gcnt == 0 {
            bail!("a model without geometries (the C# fails on it)");
        }
        let geoms = geoms.unwrap();
        let [min, max] = m.bounding_box;
        let mut bounds = vec![[0.0f32; 8]; if gcnt > 1 { gcnt + 1 } else { gcnt }];
        bounds[0] = [min[0], min[1], min[2], 0.0, max[0], max[1], max[2], 0.0];
        let off = if gcnt > 1 { 1 } else { 0 };
        for (i, g) in geoms.iter().enumerate() {
            let [a, b] = g.aabb;
            bounds[i + off] = [a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3]];
        }
        m.shader_mappings = Some(geoms.iter().map(|g| g.shader_id).collect());
        m.geometry_bounds = Some(bounds);
        m.shader_mapping_count = gcnt as u16;
        m.bones_count = m.bone_mapping.as_ref().map_or(0, |b| b.len() as u8);
        m.geometries_capacity = gcnt as u16;
        m.geometries = Some(geoms.into_iter().map(Some).collect());
        Ok(m)
    }

    /// The geometries that aren't null.
    pub fn geometries(&self) -> impl Iterator<Item = &Geometry> {
        self.geometries.iter().flatten().flatten()
    }
}

/// `Rsc8DrawableLod` (16 bytes): a level of detail's models.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrawableLod {
    /// `None` for a null pointer or no models.
    pub models: Option<Vec<Option<DrawableModel>>>,
    pub models_capacity: u16,
    pub models_unknown: u32,
}

impl DrawableLod {
    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let h = ArrHeader::read(r)?;
        Ok(Self {
            models: read_ptr_arr(r, &h, DrawableModel::read)?,
            models_capacity: h.capacity,
            models_unknown: h.unknown,
        })
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(16);
        let models: Option<Vec<Option<BlockId>>> =
            self.models.as_ref().map(|v| v.iter().map(|m| m.as_ref().map(|m| m.write(w))).collect());
        write_ptr_arr(&mut b, w, models.as_deref(), self.models_capacity, self.models_unknown);
        w.set(id, b, false);
        id
    }

    pub fn write_xml(&self, w: &mut XmlWriter) -> Result<()> {
        node_array_with(w, "Models", self.models.as_deref(), |w, m| m.write_xml(w))
    }

    pub fn read_xml(node: XmlNode) -> Result<Self> {
        let models = node_items_opt(node, "Models", DrawableModel::read_xml)?;
        Ok(Self { models_capacity: models.as_ref().map_or(0, |m| m.len() as u16), models, models_unknown: 0 })
    }

    /// The models that aren't null.
    pub fn models(&self) -> impl Iterator<Item = &DrawableModel> {
        self.models.iter().flatten().flatten()
    }
}

/// The XML names of the 4 LODs.
const LOD_NAMES: [&str; 4] = ["High", "Med", "Low", "Vlow"];

/// The `cable` shader, drawn as lines.
const CABLE_SHADER: u32 = 0x0160_5013;

/// `Rsc8DrawableBase` (176 bytes): what drawables, fragments' drawables and
/// samplers' drawables have in common.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawableBase {
    pub vft: u64,
    pub block_map: Option<BlockMap>,
    pub shader_group: Option<ShaderGroup>,
    pub skeleton: Option<Skeleton>,
    pub bounding_sphere_center: [f32; 3],
    pub bounding_sphere_radius: f32,
    pub bounding_box_min: [f32; 3],
    pub unknown_3ch: u32,
    pub bounding_box_max: [f32; 3],
    pub unknown_4ch: u32,
    /// High, medium, low and very low.
    pub lods: [Option<DrawableLod>; 4],
    pub lod_dists: [f32; 4],
    /// Each LOD's flags (low byte) and render mask (next byte).
    pub mask_and_flags: [u32; 4],
    pub joint_limits: Option<JointLimits>,
    pub unknown_98h: u32,
    pub num_extra_skeletons: u16,
    pub unknown_9eh: u16,
    /// `None` for a null pointer, or no extra skeletons when read.
    pub extra_skeletons: Option<Vec<Option<Skeleton>>>,
    pub name: Option<String>,
    /// From a dictionary's hashes, or the XML's `Hash` (not in the struct).
    pub name_hash: u32,
}

impl Default for DrawableBase {
    fn default() -> Self {
        Self {
            vft: 0,
            block_map: None,
            shader_group: None,
            skeleton: None,
            bounding_sphere_center: [0.0; 3],
            bounding_sphere_radius: 0.0,
            bounding_box_min: [0.0; 3],
            unknown_3ch: 0,
            bounding_box_max: [0.0; 3],
            unknown_4ch: 0,
            lods: [None, None, None, None],
            lod_dists: [0.0; 4],
            mask_and_flags: [0; 4],
            joint_limits: None,
            unknown_98h: 0,
            num_extra_skeletons: 0,
            unknown_9eh: 0,
            extra_skeletons: None,
            name: None,
            name_hash: 0,
        }
    }
}

impl DrawableBase {
    pub const BLOCK_LENGTH: usize = 176;

    /// Reads the 176 bytes at the reader's position and what they point to,
    /// then gives each geometry its shader's name and its bone, as the C#
    /// does. Texture names are added to `hashes`.
    pub fn read(r: &mut Rsc8Reader, hashes: &HashIndex) -> Result<Self> {
        let mut d = Self { vft: r.u64()?, ..Default::default() };
        d.block_map = r.ptr(BlockMap::read)?;
        d.shader_group = r.ptr(|r| ShaderGroup::read(r, hashes))?;
        d.skeleton = r.ptr(Skeleton::read)?;
        d.bounding_sphere_center = r.vec3()?;
        d.bounding_sphere_radius = r.f32()?;
        d.bounding_box_min = r.vec3()?;
        d.unknown_3ch = r.u32()?;
        d.bounding_box_max = r.vec3()?;
        d.unknown_4ch = r.u32()?;
        for lod in &mut d.lods {
            *lod = r.ptr(DrawableLod::read)?;
        }
        for v in &mut d.lod_dists {
            *v = r.f32()?;
        }
        for v in &mut d.mask_and_flags {
            *v = r.u32()?;
        }
        d.joint_limits = r.ptr(JointLimits::read)?;
        d.unknown_98h = r.u32()?;
        d.num_extra_skeletons = r.u16()?;
        d.unknown_9eh = r.u16()?;
        let extra_ptr = r.u64()?;
        let name_ptr = r.u64()?;
        d.name = r.string_at(name_ptr)?;
        if d.num_extra_skeletons > 0 {
            let n = d.num_extra_skeletons as usize;
            let Some(ptrs) = read_array::<u64>(r, extra_ptr, n)? else {
                bail!("extra skeletons with a null pointer (the C# fails on it)");
            };
            let mut v = Vec::with_capacity(n);
            for p in ptrs {
                v.push(r.follow(p, Skeleton::read)?);
            }
            d.extra_skeletons = Some(v);
        }
        d.set_skeleton()?;
        d.assign_shaders(hashes);
        Ok(d)
    }

    /// `SetSkeleton`: with a skeleton, the geometries of unskinned models whose
    /// bone index is a bone are attached to it.
    pub fn set_skeleton(&mut self) -> Result<()> {
        let Some(bones) = self.skeleton.as_ref().and_then(|s| s.bones.as_ref()) else { return Ok(()) };
        let nbones = bones.len();
        for m in self.lods.iter_mut().flatten().flat_map(|l| l.models.iter_mut().flatten().flatten()) {
            let Some(geoms) = &mut m.geometries else { continue };
            if m.has_skin == 0 && (m.bone_index as usize) < nbones {
                for g in geoms {
                    let Some(g) = g else { bail!("a null geometry in a model (the C# fails on it)") };
                    g.bone_index = m.bone_index as i32;
                }
            }
        }
        Ok(())
    }

    /// `AssignShaders`: each geometry gets its shader's name (and the `cable`
    /// shader's line topology).
    pub fn assign_shaders(&mut self, hashes: &HashIndex) {
        let Some(shaders) = self.shader_group.as_ref().and_then(|g| g.shaders.as_ref()) else { return };
        for m in self.lods.iter_mut().flatten().flat_map(|l| l.models.iter_mut().flatten().flatten()) {
            for g in m.geometries.iter_mut().flatten().flatten() {
                let Some(s) = shaders.get(g.shader_id as usize).and_then(|s| s.as_ref()) else { continue };
                g.name = Some(hashes.xml_name(s.name_hash));
                if s.name_hash == CABLE_SHADER {
                    g.topology = Topology::LineList;
                }
            }
        }
    }

    /// Writes the 176 bytes into `b` (which a derived struct goes on filling),
    /// adding what they point to.
    pub fn write_fields(&self, b: &mut BlockBuf, w: &mut Rsc8Writer) -> Result<()> {
        b.u64(self.vft);
        b.ptr(self.block_map.as_ref().map(|m| m.write(w)));
        b.ptr(self.shader_group.as_ref().map(|g| g.write(w)).transpose()?);
        b.ptr(self.skeleton.as_ref().map(|s| s.write(w)));
        b.vec3(self.bounding_sphere_center);
        b.f32(self.bounding_sphere_radius);
        b.vec3(self.bounding_box_min);
        b.u32(self.unknown_3ch);
        b.vec3(self.bounding_box_max);
        b.u32(self.unknown_4ch);
        for lod in &self.lods {
            b.ptr(lod.as_ref().map(|l| l.write(w)));
        }
        for v in self.lod_dists {
            b.f32(v);
        }
        for v in self.mask_and_flags {
            b.u32(v);
        }
        b.ptr(self.joint_limits.as_ref().map(|j| j.write(w)));
        b.u32(self.unknown_98h);
        b.u16(self.num_extra_skeletons);
        b.u16(self.unknown_9eh);
        let extra = self.extra_skeletons.as_ref().map(|v| {
            let ids: Vec<Option<BlockId>> = v.iter().map(|s| s.as_ref().map(|s| s.write(w))).collect();
            w.pointer_array(&ids)
        });
        b.ptr(extra);
        b.ptr(w.opt_string(self.name.as_deref()));
        Ok(())
    }

    /// Writes the drawable as a 176-byte block.
    pub fn write(&self, w: &mut Rsc8Writer) -> Result<BlockId> {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        self.write_fields(&mut b, w)?;
        w.set(id, b, false);
        Ok(id)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) -> Result<()> {
        if let Some(n) = &self.name {
            w.string("Name", n);
        }
        if self.name_hash != 0 {
            w.hash("Hash", self.name_hash);
        }
        if self.bounding_sphere_center != [0.0; 3] {
            w.vec3("BoundingSphereCenter", self.bounding_sphere_center);
        }
        if self.bounding_sphere_radius != 0.0 {
            w.f32("BoundingSphereRadius", self.bounding_sphere_radius);
        }
        if self.bounding_box_min != [0.0; 3] {
            w.vec3("BoundingBoxMin", self.bounding_box_min);
        }
        if self.bounding_box_max != [0.0; 3] {
            w.vec3("BoundingBoxMax", self.bounding_box_max);
        }
        for (n, v) in LOD_NAMES.iter().zip(self.lod_dists) {
            if v != 0.0 {
                w.f32(&format!("LodDist{n}"), v);
            }
        }
        for (n, v) in LOD_NAMES.iter().zip(self.mask_and_flags) {
            if v & 0xFF != 0 {
                w.u8(&format!("Flags{n}"), v as u8);
            }
        }
        if let Some(g) = &self.shader_group {
            w.node("ShaderGroup", |w| g.write_xml(w));
        }
        if let Some(s) = &self.skeleton {
            w.node("Skeleton", |w| s.write_xml(w));
        }
        if let Some(j) = &self.joint_limits {
            w.node("Joints", |w| j.write_xml(w));
        }
        for (n, lod) in LOD_NAMES.iter().zip(&self.lods) {
            if let Some(l) = lod {
                node_with(w, &format!("Lod{n}"), |w| l.write_xml(w))?;
            }
        }
        if let Some(extra) = &self.extra_skeletons {
            w.node_array_opt("ExtraSkeletons", Some(extra), |w, s| s.write_xml(w));
        }
        Ok(())
    }

    pub fn read_xml(node: XmlNode) -> Result<Self> {
        let mut d = Self {
            name: node.string("Name"),
            name_hash: node.string("Hash").map_or(0, |h| joaat(&h)),
            bounding_sphere_center: node.vec3("BoundingSphereCenter"),
            bounding_sphere_radius: node.f32("BoundingSphereRadius", 0.0),
            bounding_box_min: node.vec3("BoundingBoxMin"),
            bounding_box_max: node.vec3("BoundingBoxMax"),
            ..Default::default()
        };
        for (i, n) in LOD_NAMES.iter().enumerate() {
            d.lod_dists[i] = node.f32(&format!("LodDist{n}"), 0.0);
        }
        for (i, n) in LOD_NAMES.iter().enumerate() {
            d.mask_and_flags[i] = node.u8(&format!("Flags{n}"), 0) as u32;
        }
        d.shader_group = node.node("ShaderGroup").map(ShaderGroup::read_xml).transpose()?;
        d.skeleton = node.node("Skeleton").map(Skeleton::read_xml).transpose()?;
        d.joint_limits = node.node("Joints").map(JointLimits::read_xml);
        for (i, n) in LOD_NAMES.iter().enumerate() {
            d.lods[i] = node.node(&format!("Lod{n}")).map(DrawableLod::read_xml).transpose()?;
        }
        let extra = node_items(node, "ExtraSkeletons", Skeleton::read_xml)?;
        d.num_extra_skeletons = extra.as_ref().map_or(0, |e| e.len() as u16);
        d.extra_skeletons = extra.map(|v| v.into_iter().map(Some).collect());
        d.build_masks()?;
        Ok(d)
    }

    /// `BuildMasks`: each LOD in use gets the union of its models' masks.
    fn build_masks(&mut self) -> Result<()> {
        for i in 0..4 {
            let mut mask = 0u8;
            if self.lod_dists[i] > 0.0
                && let Some(models) = self.lods[i].as_ref().and_then(|l| l.models.as_ref())
            {
                for m in models {
                    let Some(m) = m else { bail!("a null model in a LOD (the C# fails on it)") };
                    mask |= m.mask;
                }
            }
            self.mask_and_flags[i] = (self.mask_and_flags[i] & 0xFFFF_00FF) + ((mask as u32) << 8);
        }
        Ok(())
    }

    /// The LODs' models that aren't null, from high to very low.
    pub fn models(&self) -> impl Iterator<Item = &DrawableModel> {
        self.lods.iter().flatten().flat_map(|l| l.models())
    }

    /// Each LOD's render mask.
    pub fn mask(&self, lod: usize) -> u8 {
        (self.mask_and_flags[lod] >> 8) as u8
    }

    /// Each LOD's flags.
    pub fn flags(&self, lod: usize) -> u8 {
        self.mask_and_flags[lod] as u8
    }
}

/// `Rsc8Drawable` (208 bytes): a drawable with its collision bounds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drawable {
    pub base: DrawableBase,
    pub unknown_b0h: u64,
    pub bounds: Option<Bounds>,
    pub sampler_dictionary: Option<SamplerDictionary>,
    pub unknown_c8h: u64,
}

impl Drawable {
    pub const BLOCK_LENGTH: usize = 208;

    pub fn read(r: &mut Rsc8Reader, hashes: &HashIndex) -> Result<Self> {
        let base = DrawableBase::read(r, hashes)?;
        let unknown_b0h = r.u64()?;
        let bounds_ptr = r.u64()?;
        let bounds = Bounds::read_at(r, bounds_ptr)?;
        let sampler_dictionary = r.ptr(|r| SamplerDictionary::read(r, hashes))?;
        Ok(Self { base, unknown_b0h, bounds, sampler_dictionary, unknown_c8h: r.u64()? })
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> Result<BlockId> {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        self.base.write_fields(&mut b, w)?;
        b.u64(self.unknown_b0h);
        b.ptr(self.bounds.as_ref().map(|x| x.write(w)).transpose()?);
        b.ptr(self.sampler_dictionary.as_ref().map(|s| s.write(w)).transpose()?);
        b.u64(self.unknown_c8h);
        w.set(id, b, false);
        Ok(id)
    }

    /// The drawable's XML, into the current node (the file's root, or a
    /// dictionary's item).
    pub fn write_xml(&self, w: &mut XmlWriter) -> Result<()> {
        w.u32("@version", 1);
        self.base.write_xml(w)?;
        if let Some(b) = &self.bounds {
            w.node("Bounds", |w| b.write_xml(w));
        }
        if let Some(s) = &self.sampler_dictionary {
            node_with(w, "SamplerDictionary", |w| s.write_xml(w))?;
        }
        Ok(())
    }

    pub fn read_xml(node: XmlNode) -> Result<Self> {
        let base = DrawableBase::read_xml(node)?;
        let bounds = node.node("Bounds").map(|n| Bounds::read_xml(&n)).transpose()?;
        let sampler_dictionary = node.node("SamplerDictionary").map(SamplerDictionary::read_xml).transpose()?;
        Ok(Self { base, bounds, sampler_dictionary, ..Default::default() })
    }
}

/// `Rsc8DrawableDictionary` (64 bytes): drawables sorted by name hash.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrawableDictionary {
    pub vft: u64,
    pub block_map: Option<BlockMap>,
    pub unknown_10h: u64,
    pub unknown_18h: u64,
    /// `None` for a null pointer or no hashes.
    pub hashes: Option<Vec<u32>>,
    pub hashes_capacity: u16,
    pub hashes_unknown: u32,
    /// `None` for a null pointer or no drawables.
    pub drawables: Option<Vec<Option<Drawable>>>,
    pub drawables_capacity: u16,
    pub drawables_unknown: u32,
}

impl DrawableDictionary {
    pub const BLOCK_LENGTH: usize = 64;

    /// Reads the dictionary; each drawable gets its hash as `name_hash`.
    pub fn read(r: &mut Rsc8Reader, hashes: &HashIndex) -> Result<Self> {
        let mut d = Self { vft: r.u64()?, ..Default::default() };
        d.block_map = r.ptr(BlockMap::read)?;
        d.unknown_10h = r.u64()?;
        d.unknown_18h = r.u64()?;
        let hh = ArrHeader::read(r)?;
        d.hashes = read_array(r, hh.ptr, hh.count as usize)?;
        d.hashes_capacity = hh.capacity;
        d.hashes_unknown = hh.unknown;
        let dh = ArrHeader::read(r)?;
        d.drawables = read_ptr_arr(r, &dh, |r| Drawable::read(r, hashes))?;
        d.drawables_capacity = dh.capacity;
        d.drawables_unknown = dh.unknown;
        if let (Some(h), Some(ds)) = (&d.hashes, &mut d.drawables) {
            for (h, dr) in h.iter().zip(ds.iter_mut()) {
                let Some(dr) = dr else { bail!("a null drawable in the dictionary (the C# fails on it)") };
                dr.base.name_hash = *h;
            }
        }
        Ok(d)
    }

    /// `Build`: a dictionary of these drawables, sorted by name hash.
    pub fn build(mut drawables: Vec<Drawable>) -> Self {
        drawables.sort_by_key(|d| d.base.name_hash);
        let n = drawables.len() as u16;
        Self {
            hashes: Some(drawables.iter().map(|d| d.base.name_hash).collect()),
            hashes_capacity: n,
            drawables: Some(drawables.into_iter().map(Some).collect()),
            drawables_capacity: n,
            ..Default::default()
        }
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> Result<BlockId> {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u64(self.vft);
        b.ptr(self.block_map.as_ref().map(|m| m.write(w)));
        b.u64(self.unknown_10h);
        b.u64(self.unknown_18h);
        write_arr(&mut b, w, self.hashes.as_deref(), self.hashes_capacity, self.hashes_unknown);
        let ds = match &self.drawables {
            Some(v) => Some(v.iter().map(|d| d.as_ref().map(|d| d.write(w)).transpose()).collect::<Result<Vec<_>>>()?),
            None => None,
        };
        write_ptr_arr(&mut b, w, ds.as_deref(), self.drawables_capacity, self.drawables_unknown);
        w.set(id, b, false);
        Ok(id)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) -> Result<()> {
        w.u32("@version", 1);
        node_array_with(w, "Drawables", self.drawables.as_deref(), |w, d| d.write_xml(w))
    }

    pub fn read_xml(node: XmlNode) -> Result<Self> {
        let Some(drawables) = node_items(node, "Drawables", Drawable::read_xml)? else {
            bail!("the drawable dictionary has no Drawables (the C# fails on it)");
        };
        Ok(Self::build(drawables))
    }
}
