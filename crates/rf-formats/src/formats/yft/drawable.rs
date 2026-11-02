//! The fragment's drawables (`Rsc8FragDrawable`), environment cloth
//! (`Rsc8EnvironmentCloth`), bone transforms (`Rsc8FragBoneTransforms`) and
//! glass panes (`Rsc8FragPaneModelInfo`).

use super::ReadCx;
use crate::formats::ybn::Bounds;
use crate::formats::ydr::util::{ArrHeader, read_array, read_ptr_arr, write_ptr_arr};
use crate::formats::ydr::{DrawableBase, VertexFormat, VertexLayout};
use crate::formats::yld::util::node_with;
use crate::formats::yld::{Arr, ClothController, ClothInstanceTuning};
use crate::rsc8::{BlockBuf, BlockId, Rsc8Reader, Rsc8Writer};
use crate::xml::{XmlNode, XmlWriter};
use crate::{Result, bail};

/// `Rsc8FragDrawable` (0x150 bytes): a [`DrawableBase`] placed in the
/// fragment, with its bounds.
#[derive(Clone, Debug, PartialEq)]
pub struct FragDrawable {
    pub base: DrawableBase,
    pub frag_matrix: [f32; 16],
    pub bounds: Option<Bounds>,
    /// Null pointers in the game's files.
    pub extra_bounds: Option<Vec<Option<Bounds>>>,
    pub extra_bounds_capacity: u16,
    pub extra_bounds_unknown: u32,
    /// Read when `extra_matrices_count` isn't 0.
    pub extra_matrices: Option<Vec<[f32; 16]>>,
    pub extra_matrices_count: u16,
    pub unknown_112h: u16,
    pub unknown_114h: u32,
    pub locator_pointer: u64,
    pub unknown_120h: u64,
    pub unknown_128h: u64,
    pub frag_name: Option<String>,
    /// Empty in the game's files.
    pub frag_animations: Arr<u64>,
    pub unknown_148h: u64,
    /// Not in the file: whether the XML has the drawable's skeleton (the C#'s
    /// `FragSkeletonExport`). The fragment's own drawables (`drawable`,
    /// `drawable_array`, `drawable_cloth`) always do; another drawable does
    /// when it is one of them in the file (an environment cloth's drawable is
    /// usually one of `drawable_cloth`).
    pub frag_skeleton_export: bool,
    /// Not in the file: the bounds are a child of a physics LOD's composite
    /// bound, which has them in the XML (the C#'s `Bounds.Parent`). Physics
    /// LODs write such drawables of `drawables1` pointing to the composite's
    /// child of the same index.
    pub bounds_in_composite: bool,
}

impl Default for FragDrawable {
    fn default() -> Self {
        Self {
            base: DrawableBase::default(),
            frag_matrix: [0.0; 16],
            bounds: None,
            extra_bounds: None,
            extra_bounds_capacity: 0,
            extra_bounds_unknown: 0,
            extra_matrices: None,
            extra_matrices_count: 0,
            unknown_112h: 1,
            unknown_114h: 0,
            locator_pointer: 0,
            unknown_120h: 0,
            unknown_128h: 0,
            frag_name: None,
            frag_animations: Arr::default(),
            unknown_148h: 0,
            frag_skeleton_export: false,
            bounds_in_composite: false,
        }
    }
}

impl FragDrawable {
    pub const BLOCK_LENGTH: usize = 0x150;

    pub fn read(r: &mut Rsc8Reader, cx: &ReadCx) -> Result<Self> {
        let pos = r.pos;
        let mut d = Self { base: DrawableBase::read(r, cx.hashes)?, frag_matrix: r.floats()?, ..Default::default() };
        let bounds_ptr = r.u64()?;
        d.bounds = Bounds::read_at(r, bounds_ptr)?;
        let eb = ArrHeader::read(r)?;
        d.extra_bounds = read_ptr_arr(r, &eb, Bounds::read)?;
        d.extra_bounds_capacity = eb.capacity;
        d.extra_bounds_unknown = eb.unknown;
        let matrices_ptr = r.u64()?;
        d.extra_matrices_count = r.u16()?;
        d.unknown_112h = r.u16()?;
        d.unknown_114h = r.u32()?;
        d.locator_pointer = r.u64()?;
        d.unknown_120h = r.u64()?;
        d.unknown_128h = r.u64()?;
        let name_ptr = r.u64()?;
        d.frag_name = r.string_at(name_ptr)?;
        d.frag_animations = Arr::read(r)?;
        d.unknown_148h = r.u64()?;
        if d.extra_matrices_count > 0 {
            d.extra_matrices = read_array(r, matrices_ptr, d.extra_matrices_count as usize)?;
        }
        d.frag_skeleton_export = cx.roots.contains(&pos);
        d.bounds_in_composite = bounds_ptr != 0 && cx.composite_children.contains(&bounds_ptr);
        Ok(d)
    }

    /// Writes the drawable. `skeleton` is a skeleton block to point to instead
    /// of writing the drawable's own (see
    /// [`DrawableBase::write_fields_with`]); `bounds`, a bounds block to point
    /// to instead of writing the drawable's own.
    pub fn write(
        &self,
        w: &mut Rsc8Writer,
        skeleton: Option<BlockId>,
        bounds: Option<Option<BlockId>>,
    ) -> Result<BlockId> {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        self.base.write_fields_with(&mut b, w, skeleton)?;
        b.floats(&self.frag_matrix);
        let bounds = match bounds {
            Some(x) => x,
            None => self.bounds.as_ref().map(|x| x.write(w)).transpose()?,
        };
        b.ptr(bounds);
        let extra = match &self.extra_bounds {
            Some(v) => Some(v.iter().map(|x| x.as_ref().map(|x| x.write(w)).transpose()).collect::<Result<Vec<_>>>()?),
            None => None,
        };
        write_ptr_arr(&mut b, w, extra.as_deref(), self.extra_bounds_capacity, self.extra_bounds_unknown);
        b.ptr(w.array(self.extra_matrices.as_deref()));
        b.u16(self.extra_matrices_count);
        b.u16(self.unknown_112h);
        b.u32(self.unknown_114h);
        b.u64(self.locator_pointer);
        b.u64(self.unknown_120h);
        b.u64(self.unknown_128h);
        b.ptr(w.opt_string(self.frag_name.as_deref()));
        self.frag_animations.write(&mut b, w);
        b.u64(self.unknown_148h);
        w.set(id, b, false);
        Ok(id)
    }

    /// The XML, into the current node; the skeleton only with `skeleton`.
    pub fn write_xml(&self, w: &mut XmlWriter, skeleton: bool) -> Result<()> {
        w.string("FragName", self.frag_name.as_deref().unwrap_or(""));
        w.matrix4x4("FragMatrix", &self.frag_matrix);
        w.matrix4x4_array("ExtraMatrices", self.extra_matrices.as_deref());
        self.base.write_xml_with(w, skeleton || self.frag_skeleton_export)?;
        if let Some(b) = self.bounds.as_ref().filter(|_| !self.bounds_in_composite) {
            w.node("Bounds", |w| b.write_xml(w));
        }
        Ok(())
    }

    pub fn read_xml(n: XmlNode) -> Result<Self> {
        let extra_matrices = n.matrix_array::<16>("ExtraMatrices");
        let count = extra_matrices.as_ref().map_or(0, |m| m.len()) as u16;
        Ok(Self {
            frag_name: n.string("FragName").filter(|s| !s.is_empty()),
            frag_matrix: n.matrix("FragMatrix").unwrap_or([0.0; 16]),
            extra_matrices,
            extra_matrices_count: count,
            base: DrawableBase::read_xml(n)?,
            bounds: n.node("Bounds").map(|b| Bounds::read_xml(&b)).transpose()?,
            extra_bounds: (count != 0).then(|| vec![None; count as usize]),
            extra_bounds_capacity: count,
            ..Default::default()
        })
    }
}

/// `Rsc8EnvironmentCloth` (0x80 bytes): cloth in the world (flags, curtains),
/// with its drawable and controller.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EnvironmentCloth {
    pub vft: u64,
    pub unknown_8h: u64,
    pub instance_tuning: Option<ClothInstanceTuning>,
    pub drawable: Option<FragDrawable>,
    pub unknown_20h: u64,
    pub controller: Option<ClothController>,
    /// 0x30 to 0x58.
    pub unknown_30h: [u64; 6],
    /// Unused?
    pub unknown_data: Arr<u32>,
    pub unknown_70h: u64,
    /// 0, 0x10, 0x2000 or 0x2010.
    pub unknown_78h: u32,
    pub unknown_7ch: u32,
}

impl EnvironmentCloth {
    pub const BLOCK_LENGTH: usize = 0x80;

    pub fn read(r: &mut Rsc8Reader, cx: &ReadCx) -> Result<Self> {
        let mut c = Self { vft: r.u64()?, unknown_8h: r.u64()?, ..Default::default() };
        c.instance_tuning = r.ptr(ClothInstanceTuning::read)?;
        c.drawable = r.ptr(|r| FragDrawable::read(r, cx))?;
        c.unknown_20h = r.u64()?;
        c.controller = r.ptr(ClothController::read)?;
        for v in &mut c.unknown_30h {
            *v = r.u64()?;
        }
        c.unknown_data = Arr::read(r)?;
        c.unknown_70h = r.u64()?;
        c.unknown_78h = r.u32()?;
        c.unknown_7ch = r.u32()?;
        Ok(c)
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> Result<BlockId> {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u64(self.vft);
        b.u64(self.unknown_8h);
        b.ptr(self.instance_tuning.as_ref().map(|t| t.write(w)));
        b.ptr(self.drawable.as_ref().map(|d| d.write(w, None, None)).transpose()?);
        b.u64(self.unknown_20h);
        b.ptr(self.controller.as_ref().map(|c| c.write(w)));
        self.unknown_30h.iter().for_each(|v| b.u64(*v));
        self.unknown_data.write(&mut b, w);
        b.u64(self.unknown_70h);
        b.u32(self.unknown_78h);
        b.u32(self.unknown_7ch);
        w.set(id, b, false);
        Ok(id)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) -> Result<()> {
        if let Some(t) = &self.instance_tuning {
            w.node("InstanceTuning", |w| t.write_xml(w));
        }
        if let Some(d) = &self.drawable {
            node_with(w, "Drawable", |w| d.write_xml(w, false))?;
        }
        if let Some(c) = &self.controller {
            w.node("Controller", |w| c.write_xml(w));
        }
        w.u32("Unknown78", self.unknown_78h);
        Ok(())
    }

    pub fn read_xml(n: XmlNode) -> Result<Self> {
        Ok(Self {
            instance_tuning: n.node("InstanceTuning").map(ClothInstanceTuning::read_xml),
            drawable: n.node("Drawable").map(FragDrawable::read_xml).transpose()?,
            controller: n.node("Controller").map(ClothController::read_xml).transpose()?,
            unknown_78h: n.u32("Unknown78", 0),
            ..Default::default()
        })
    }
}

/// `Rsc8FragBoneTransforms` (0x20 bytes and the matrices): a matrix per bone
/// of the fragment's skeleton.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FragBoneTransforms {
    pub unknown_00h: u64,
    pub unknown_08h: u64,
    /// The low byte of the count.
    pub item_count1: u8,
    pub item_count2: u8,
    pub unknown_12h: u16,
    pub unknown_14h: u32,
    pub unknown_18h: u64,
    /// 3x4 matrices, as many as the fragment's drawable has bones when read
    /// (the counts are bytes).
    pub items: Option<Vec<[f32; 12]>>,
}

impl FragBoneTransforms {
    /// As `new Rsc8FragBoneTransforms(items)`.
    pub fn new(items: Option<Vec<[f32; 12]>>) -> Self {
        let n = items.as_ref().map_or(0, |i| i.len()) as u8;
        Self { item_count1: n, item_count2: n, items, ..Default::default() }
    }

    /// Reads the header and `bone_count` matrices after it.
    pub fn read(r: &mut Rsc8Reader, bone_count: usize) -> Result<Self> {
        let mut t = Self {
            unknown_00h: r.u64()?,
            unknown_08h: r.u64()?,
            item_count1: r.u8()?,
            item_count2: r.u8()?,
            unknown_12h: r.u16()?,
            unknown_14h: r.u32()?,
            unknown_18h: r.u64()?,
            items: None,
        };
        t.items = read_array(r, r.pos, bone_count)?;
        Ok(t)
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let items = self.items.as_deref().unwrap_or(&[]);
        let mut b = BlockBuf::with_capacity(0x20 + items.len() * 0x30);
        b.u64(self.unknown_00h);
        b.u64(self.unknown_08h);
        b.u8(self.item_count1);
        b.u8(self.item_count2);
        b.u16(self.unknown_12h);
        b.u32(self.unknown_14h);
        b.u64(self.unknown_18h);
        items.iter().for_each(|m| b.floats(m));
        w.add(b, false)
    }
}

/// `Rsc8FragPaneModelInfo` (0x1A0 bytes): a breakable glass pane.
#[derive(Clone, Debug, PartialEq)]
pub struct FragPaneModelInfo {
    pub projection: [f32; 16],
    /// Stored in the struct, not pointed to.
    pub vertex_format: VertexFormat,
    pub unknown_17ch: u32,
    pub unknown_180h: f32,
    pub unknown_184h: u16,
    pub unknown_186h: u8,
    pub frag_index: u8,
    pub thickness: f32,
    pub tangent: [f32; 3],
    pub unknown_198h: f32,
    pub unknown_19ch: u32,
}

impl FragPaneModelInfo {
    pub const BLOCK_LENGTH: usize = 0x1A0;

    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        Ok(Self {
            projection: r.floats()?,
            vertex_format: VertexFormat::read(r)?,
            unknown_17ch: r.u32()?,
            unknown_180h: r.f32()?,
            unknown_184h: r.u16()?,
            unknown_186h: r.u8()?,
            frag_index: r.u8()?,
            thickness: r.f32()?,
            tangent: r.vec3()?,
            unknown_198h: r.f32()?,
            unknown_19ch: r.u32()?,
        })
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.floats(&self.projection);
        let f = &self.vertex_format;
        f.offsets.iter().for_each(|o| b.u32(*o));
        b.bytes(&f.sizes);
        b.bytes(&f.types);
        b.u32(f.data);
        b.u32(self.unknown_17ch);
        b.f32(self.unknown_180h);
        b.u16(self.unknown_184h);
        b.u8(self.unknown_186h);
        b.u8(self.frag_index);
        b.f32(self.thickness);
        b.vec3(self.tangent);
        b.f32(self.unknown_198h);
        b.u32(self.unknown_19ch);
        w.add(b, false)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.matrix4x4("Projection", &self.projection);
        let l = self.vertex_format.engine_layout();
        w.node("VertexLayout", |w| {
            w.string("Semantics", &l.semantics_string());
            w.string("Formats", &l.formats_string());
            if l.non_interleaved() {
                w.bool("NonInterleaved", true);
            }
        });
        w.i32("VertexCount", self.vertex_format.vertex_count() as i32);
        let cs = self.vertex_format.colour_semantic_index();
        if cs != 0 {
            w.u32("ColourSemantic", cs);
        }
        w.f32("Unknown180", self.unknown_180h);
        w.u16("Unknown184", self.unknown_184h);
        w.u8("FragIndex", self.frag_index);
        w.f32("Thickness", self.thickness);
        w.vec3("Tangent", self.tangent);
        w.f32("Unknown198", self.unknown_198h);
    }

    pub fn read_xml(n: XmlNode) -> Result<Self> {
        let Some(l) = n.node("VertexLayout") else { bail!("a glass pane without a VertexLayout (the C# fails on it)") };
        let (Some(s), Some(f)) = (l.string("Semantics"), l.string("Formats")) else {
            bail!("a vertex layout without Semantics or Formats (the C# fails on it)");
        };
        let mut layout = VertexLayout::from_strings(&s, &f);
        if l.bool("NonInterleaved", false) {
            layout.set_non_interleaved(true);
        }
        let count = n.i32("VertexCount", 0);
        let cs = n.u32("ColourSemantic", 0);
        Ok(Self {
            projection: n.matrix("Projection").unwrap_or([0.0; 16]),
            vertex_format: VertexFormat::new(&layout, count as u32 as usize, cs),
            unknown_17ch: 0,
            unknown_180h: n.f32("Unknown180", 0.0),
            unknown_184h: n.u16("Unknown184", 0),
            unknown_186h: 0,
            frag_index: n.u8("FragIndex", 0),
            thickness: n.f32("Thickness", 0.0),
            tangent: n.vec3("Tangent"),
            unknown_198h: n.f32("Unknown198", 0.0),
            unknown_19ch: 0,
        })
    }
}
