//! `Rsc8Bounds` and its subclasses: every bound type, composites included.

use super::bvh::{self, BuildItem, Bvh};
use super::geometry::{BoundBvhPart, BoundGeometry};
use super::material::{BOUND_FLAG_NAMES, BoundMaterial1, BoundMaterial2, CompositeFlags, flags_to_string, parse_flags};
use super::math::{self, IDENTITY, Mat4, Vec3};
use super::reader::{BoundsReader, ChildInfo, Tag};
use crate::rsc8::{BlockBuf, BlockId, BlockMap, Rsc8Reader, Rsc8Writer};
use crate::xml::{XmlNode, XmlWriter};
use crate::{Error, Result, bail};

/// `Rsc8BoundsType`, the bound's type byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum BoundsType {
    Sphere = 0,
    Capsule = 1,
    TaperedCapsule = 2,
    Box = 3,
    Geometry = 4,
    GeometryBvh = 5,
    Composite = 6,
    Triangle = 7,
    Disc = 8,
    Cylinder = 9,
    Plane = 10,
    MorphGeometry = 11,
    Ring = 12,
    Cloth = 13,
}

const TYPE_NAMES: [&str; 14] = [
    "Sphere",
    "Capsule",
    "TaperedCapsule",
    "Box",
    "Geometry",
    "GeometryBVH",
    "Composite",
    "Triangle",
    "Disc",
    "Cylinder",
    "Plane",
    "MorphGeometry",
    "Ring",
    "Cloth",
];

impl BoundsType {
    pub fn from_u8(v: u8) -> Option<Self> {
        use BoundsType::*;
        Some(match v {
            0 => Sphere,
            1 => Capsule,
            2 => TaperedCapsule,
            3 => Box,
            4 => Geometry,
            5 => GeometryBvh,
            6 => Composite,
            7 => Triangle,
            8 => Disc,
            9 => Cylinder,
            10 => Plane,
            11 => MorphGeometry,
            12 => Ring,
            13 => Cloth,
            _ => return None,
        })
    }

    /// The C# enum name, as in the XML's `type` attribute.
    pub fn name(self) -> &'static str {
        TYPE_NAMES[self as usize]
    }

    /// `Enum.TryParse<Rsc8BoundsType>`: a name, a number, or names separated by
    /// commas (OR'ed); `None` also for `None` (255) and unnamed values, for
    /// which the C# creates no bound.
    pub fn parse(s: &str) -> Option<Self> {
        let t = s.trim();
        let first = t.chars().next()?;
        let v: u32 = if first.is_ascii_digit() || first == '-' || first == '+' {
            let v: i64 = t.parse().ok()?;
            u8::try_from(v).ok()? as u32
        } else {
            let mut v = 0u32;
            for part in t.split(',') {
                let p = part.trim();
                v |= if p == "None" { 255 } else { TYPE_NAMES.iter().position(|n| *n == p)? as u32 };
            }
            v
        };
        Self::from_u8(v as u8)
    }
}

/// `Rsc8BoundCapsule`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundCapsule {
    pub half_height: f32,
    pub unknown_74h: u32,
    pub unknown_78h: u32,
    pub unknown_7ch: u32,
}

/// `Rsc8BoundTaperedCapsule`.
#[derive(Clone, Debug, PartialEq)]
pub struct BoundTaperedCapsule {
    pub half_height: f32,
    pub unknown_74h: f32,
    pub unknown_78h: f32,
    pub unknown_7ch: f32,
    pub unknown_80h: f32,
    pub unknown_84h: u32,
    pub unknown_88h: u32,
    pub unknown_8ch: u32,
}

impl Default for BoundTaperedCapsule {
    fn default() -> Self {
        Self {
            half_height: 0.0,
            unknown_74h: 0.0,
            unknown_78h: 0.0,
            unknown_7ch: 0.0,
            unknown_80h: 0.0,
            unknown_84h: 0,
            unknown_88h: 12667,
            unknown_8ch: 0,
        }
    }
}

/// `Rsc8BoundDisc`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundDisc {
    pub unknown_70h: u32,
    pub unknown_74h: u32,
    pub unknown_78h: u32,
    pub unknown_7ch: u32,
}

/// `Rsc8BoundCylinder`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundCylinder {
    pub half_height: f32,
    pub unknown_74h: u32,
    pub unknown_78h: u32,
    pub unknown_7ch: u32,
}

/// `Rsc8BoundComposite`: child bounds, each placed by its own `transform`.
///
/// The matrix and flag arrays are kept as read; writing builds them again from
/// the children's `transform` and `composite_flags1`/`2`, as the C# does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundComposite {
    /// `None` items are null pointers.
    pub children: Option<Vec<Option<Bounds>>>,
    pub current_matrices: Option<Vec<Mat4>>,
    pub last_matrices: Option<Vec<Mat4>>,
    pub type_and_include_flags: Option<Vec<CompositeFlags>>,
    pub owned_type_and_include_flags: Option<Vec<CompositeFlags>>,
    pub max_num_bounds: u16,
    pub num_bounds: u16,
    pub unknown_a4h: u32,
    pub bvh: Option<Bvh>,
    /// From XML: write no BVH even with 6 or more children.
    pub no_bvh: bool,
}

/// The data of each bound type beyond the common fields.
#[derive(Clone, Debug, PartialEq)]
pub enum BoundsShape {
    /// Sphere, box, triangle, plane, morph geometry, ring and cloth: nothing more.
    Basic,
    Capsule(BoundCapsule),
    TaperedCapsule(BoundTaperedCapsule),
    Disc(BoundDisc),
    Cylinder(BoundCylinder),
    Geometry(Box<BoundGeometry>),
    GeometryBvh(Box<BoundGeometry>, BoundBvhPart),
    Composite(BoundComposite),
}

impl BoundsShape {
    /// The shape the C# creates for a type.
    pub fn for_type(t: BoundsType) -> Self {
        use BoundsType as T;
        match t {
            T::Capsule => Self::Capsule(Default::default()),
            T::TaperedCapsule => Self::TaperedCapsule(Default::default()),
            T::Disc => Self::Disc(Default::default()),
            T::Cylinder => Self::Cylinder(Default::default()),
            T::Geometry => Self::Geometry(Default::default()),
            T::GeometryBvh => Self::GeometryBvh(Default::default(), Default::default()),
            T::Composite => Self::Composite(Default::default()),
            _ => Self::Basic,
        }
    }

    /// The size of the bound's block.
    pub fn block_length(&self) -> usize {
        match self {
            Self::Basic => 112,
            Self::Capsule(_) | Self::Disc(_) | Self::Cylinder(_) => 128,
            Self::TaperedCapsule(_) => 144,
            Self::Geometry(_) => 304,
            Self::GeometryBvh(..) => 336,
            Self::Composite(_) => 176,
        }
    }
}

/// `Rsc8Bounds`: a collision bound (`phBound`).
#[derive(Clone, Debug, PartialEq)]
pub struct Bounds {
    pub vft: u64,
    pub block_map: Option<BlockMap>,
    pub bounds_type: BoundsType,
    pub unknown_11h: u8,
    pub unknown_12h: u16,
    pub sphere_radius: f32,
    pub unknown_18h: u32,
    pub unknown_1ch: u32,
    pub box_max: Vec3,
    pub margin: f32,
    pub box_min: Vec3,
    pub unknown_3ch: u32,
    pub box_center: Vec3,
    pub material: BoundMaterial1,
    pub sphere_center: Vec3,
    pub material2: BoundMaterial2,
    pub inertia: Vec3,
    pub mass: f32,
    /// Where a composite puts this child (identity otherwise).
    pub transform: Mat4,
    /// This child's type and include flags in its composite.
    pub composite_flags1: CompositeFlags,
    /// This child's "owned" type and include flags in its composite.
    pub composite_flags2: CompositeFlags,
    pub shape: BoundsShape,
}

impl Bounds {
    /// A bound as the C# constructs one of that type. A `Plane` gets the type
    /// `Box`, as `Rsc8BoundPlane`'s constructor sets.
    pub fn new(t: BoundsType) -> Self {
        Self {
            vft: 0,
            block_map: None,
            bounds_type: if t == BoundsType::Plane { BoundsType::Box } else { t },
            unknown_11h: 0,
            unknown_12h: 0,
            sphere_radius: 0.0,
            unknown_18h: 0,
            unknown_1ch: 0,
            box_max: [0.0; 3],
            margin: 0.0,
            box_min: [0.0; 3],
            unknown_3ch: 1,
            box_center: [0.0; 3],
            material: BoundMaterial1(0),
            sphere_center: [0.0; 3],
            material2: BoundMaterial2(0),
            inertia: [0.0; 3],
            mass: 0.0,
            transform: IDENTITY,
            composite_flags1: CompositeFlags::default(),
            composite_flags2: CompositeFlags::default(),
            shape: BoundsShape::for_type(t),
        }
    }

    // ----- binary -----

    /// Reads the bound at `ptr` (`None` for a null pointer), with its children.
    pub fn read_at(r: &mut Rsc8Reader, ptr: u64) -> Result<Option<Bounds>> {
        if ptr == 0 {
            return Ok(None);
        }
        let mut br = BoundsReader::new(r);
        br.load(ptr)?;
        br.assemble(ptr).map(Some)
    }

    /// Reads the bound at the reader's position, with its children.
    pub fn read(r: &mut Rsc8Reader) -> Result<Bounds> {
        let ptr = r.pos;
        let b = Self::read_at(r, ptr)?;
        b.ok_or_else(|| Error::Format("null bounds".into()))
    }

    /// Reads one block at the current position (children only by pointer).
    pub(crate) fn read_block(br: &mut BoundsReader) -> Result<(Bounds, Option<Vec<u64>>)> {
        let start = br.r.pos;
        let vft = br.r.u64()?;
        let block_map_ptr = br.r.u64()?;
        let type_byte = br.r.u8()?;
        let Some(t) = BoundsType::from_u8(type_byte) else { bail!("unknown bounds type {type_byte} at 0x{start:X}") };
        let mut b = Bounds::new(t);
        b.bounds_type = t;
        b.vft = vft;
        // Read in place, as `Rsc8Ptr.Read` does.
        b.block_map = br.block_map(block_map_ptr)?;
        let r = &mut *br.r;
        b.unknown_11h = r.u8()?;
        b.unknown_12h = r.u16()?;
        b.sphere_radius = r.f32()?;
        b.unknown_18h = r.u32()?;
        b.unknown_1ch = r.u32()?;
        b.box_max = r.vec3()?;
        b.margin = r.f32()?;
        b.box_min = r.vec3()?;
        b.unknown_3ch = r.u32()?;
        b.box_center = r.vec3()?;
        b.material = BoundMaterial1(r.u32()?);
        b.sphere_center = r.vec3()?;
        b.material2 = BoundMaterial2(r.u32()?);
        b.inertia = r.vec3()?;
        b.mass = r.f32()?;

        let mut children = None;
        match &mut b.shape {
            BoundsShape::Basic => {}
            BoundsShape::Capsule(c) => {
                c.half_height = r.f32()?;
                c.unknown_74h = r.u32()?;
                c.unknown_78h = r.u32()?;
                c.unknown_7ch = r.u32()?;
            }
            BoundsShape::TaperedCapsule(c) => {
                c.half_height = r.f32()?;
                c.unknown_74h = r.f32()?;
                c.unknown_78h = r.f32()?;
                c.unknown_7ch = r.f32()?;
                c.unknown_80h = r.f32()?;
                c.unknown_84h = r.u32()?;
                c.unknown_88h = r.u32()?;
                c.unknown_8ch = r.u32()?;
            }
            BoundsShape::Disc(c) => {
                c.unknown_70h = r.u32()?;
                c.unknown_74h = r.u32()?;
                c.unknown_78h = r.u32()?;
                c.unknown_7ch = r.u32()?;
            }
            BoundsShape::Cylinder(c) => {
                c.half_height = r.f32()?;
                c.unknown_74h = r.u32()?;
                c.unknown_78h = r.u32()?;
                c.unknown_7ch = r.u32()?;
            }
            BoundsShape::Geometry(g) => **g = BoundGeometry::read(br)?,
            BoundsShape::GeometryBvh(g, part) => {
                **g = BoundGeometry::read(br)?;
                *part = BoundBvhPart::read(br)?;
            }
            BoundsShape::Composite(c) => children = Some(read_composite(br, c)?),
        }
        Ok((b, children))
    }

    /// Adds the bound and everything it points to; the bound's block is
    /// reserved first, so a root bound comes first in the file.
    ///
    /// Like the C#, this recomputes what is derived: a geometry's quantized
    /// vertices, shrunk vertices, octants, edges and BVH (which reorders a BVH
    /// geometry's polygons), and a composite's matrices, flags and BVH.
    pub fn write(&self, w: &mut Rsc8Writer) -> Result<BlockId> {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(self.shape.block_length());
        self.write_base(w, &mut b);
        match &self.shape {
            BoundsShape::Basic => {}
            BoundsShape::Capsule(c) => {
                b.f32(c.half_height);
                b.u32(c.unknown_74h);
                b.u32(c.unknown_78h);
                b.u32(c.unknown_7ch);
            }
            BoundsShape::TaperedCapsule(c) => {
                b.f32(c.half_height);
                b.f32(c.unknown_74h);
                b.f32(c.unknown_78h);
                b.f32(c.unknown_7ch);
                b.f32(c.unknown_80h);
                b.u32(c.unknown_84h);
                b.u32(c.unknown_88h);
                b.u32(c.unknown_8ch);
            }
            BoundsShape::Disc(c) => {
                b.u32(c.unknown_70h);
                b.u32(c.unknown_74h);
                b.u32(c.unknown_78h);
                b.u32(c.unknown_7ch);
            }
            BoundsShape::Cylinder(c) => {
                b.f32(c.half_height);
                b.u32(c.unknown_74h);
                b.u32(c.unknown_78h);
                b.u32(c.unknown_7ch);
            }
            BoundsShape::Geometry(g) => g.write(self, None, w, &mut b)?,
            BoundsShape::GeometryBvh(g, part) => g.write(self, Some(part), w, &mut b)?,
            BoundsShape::Composite(c) => self.write_composite(c, w, &mut b)?,
        }
        b.pad_to(self.shape.block_length());
        w.set(id, b, false);
        Ok(id)
    }

    fn write_base(&self, w: &mut Rsc8Writer, b: &mut BlockBuf) {
        b.u64(self.vft);
        let bm = self.block_map.as_ref().map(|m| m.write(w));
        b.ptr(bm);
        b.u8(self.bounds_type as u8);
        b.u8(self.unknown_11h);
        b.u16(self.unknown_12h);
        b.f32(self.sphere_radius);
        b.u32(self.unknown_18h);
        b.u32(self.unknown_1ch);
        b.vec3(self.box_max);
        b.f32(self.margin);
        b.vec3(self.box_min);
        b.u32(self.unknown_3ch);
        b.vec3(self.box_center);
        b.u32(self.material.0);
        b.vec3(self.sphere_center);
        b.u32(self.material2.0);
        b.vec3(self.inertia);
        b.f32(self.mass);
    }

    fn write_composite(&self, c: &BoundComposite, w: &mut Rsc8Writer, b: &mut BlockBuf) -> Result<()> {
        // UpdateStuffBeforeSaving
        let kids = c.children.as_deref();
        let count = kids.map_or(0, |k| k.len()) as u16;

        // UpdateChildrenFlags: the owned flags share the first array unless
        // some are set.
        let (flags1, flags2) = match kids {
            None => (None, None),
            Some(k) => {
                let f1: Vec<CompositeFlags> =
                    k.iter().map(|c| c.as_ref().map(|c| c.composite_flags1).unwrap_or_default()).collect();
                let f2: Vec<CompositeFlags> =
                    k.iter().map(|c| c.as_ref().map(|c| c.composite_flags2).unwrap_or_default()).collect();
                let used = |f: &[CompositeFlags]| f.iter().any(|f| f.type_flags != 0 || f.include_flags != 0);
                let a = used(&f1).then(|| w.array(Some(&f1[..]))).flatten();
                let o = if used(&f2) { w.array(Some(&f2[..])) } else { a };
                (a, o)
            }
        };
        // UpdateChildrenTransformations: the last matrices are the current ones.
        let matrices = kids.map(|k| {
            let m: Vec<Mat4> = k.iter().map(|c| c.as_ref().map_or(IDENTITY, |c| c.transform)).collect();
            w.array(Some(&m[..])).unwrap()
        });
        let bvh = self.composite_bvh(c).map(|bvh| bvh.write(w));

        let ptrs = match kids {
            None => None,
            Some(k) => {
                let mut ids = Vec::with_capacity(k.len());
                for child in k {
                    ids.push(match child {
                        Some(child) => Some(child.write(w)?),
                        None => None,
                    });
                }
                Some(w.pointer_array(&ids))
            }
        };
        b.ptr(ptrs);
        b.ptr(matrices);
        b.ptr(matrices);
        b.ptr(flags1);
        b.ptr(flags2);
        b.u16(count);
        b.u16(count);
        b.u32(c.unknown_a4h);
        b.ptr(bvh);
        Ok(())
    }

    /// `Rsc8BoundComposite.BuildBVH`: composites with 6 or more children get a
    /// BVH of their children's transformed boxes, within the composite's box.
    fn composite_bvh(&self, c: &BoundComposite) -> Option<Bvh> {
        if c.no_bvh {
            return None;
        }
        let kids = c.children.as_deref()?;
        if kids.len() <= 5 {
            return None;
        }
        let mut items: Vec<Option<BuildItem>> = kids
            .iter()
            .enumerate()
            .map(|(i, child)| {
                child.as_ref().map(|child| {
                    let (min, max) = transform_box(child.box_min, child.box_max, &child.transform);
                    BuildItem { min, max, index: i as i32 }
                })
            })
            .collect();
        Some(bvh::build(&mut items, 1, Some((self.box_min, self.box_max))).0)
    }

    // ----- XML -----

    /// Writes the bound into the current XML node: its `type` and `version`
    /// attributes, then its fields (`Rsc8Bounds.Write(MetaNodeWriter)`).
    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.string("@type", self.bounds_type.name());
        w.u32("@version", 2);
        if self.box_min != [0.0; 3] {
            w.vec3("BoxMin", self.box_min);
        }
        if self.box_max != [0.0; 3] {
            w.vec3("BoxMax", self.box_max);
        }
        if self.box_center != [0.0; 3] {
            w.vec3("BoxCenter", self.box_center);
        }
        if self.sphere_center != [0.0; 3] {
            w.vec3("SphereCenter", self.sphere_center);
        }
        if self.sphere_radius != 0.0 {
            w.f32("SphereRadius", self.sphere_radius);
        }
        if self.margin != 0.0 {
            w.f32("Margin", self.margin);
        }
        if self.mass != 0.0 {
            w.f32("Mass", self.mass);
        }
        if self.inertia != [0.0; 3] {
            w.vec3("Inertia", self.inertia);
        }
        if self.unknown_11h != 0 {
            w.u8("Unknown_11h", self.unknown_11h);
        }
        if self.unknown_3ch != 1 {
            w.u32("Unknown_3Ch", self.unknown_3ch);
        }
        self.material.write_xml(w, "Material");
        self.material2.write_xml(w, "Material");
        if self.transform != IDENTITY {
            w.matrix4x4("Transform", &self.transform);
        }
        let flag = |w: &mut XmlWriter, id: &str, v: u64| {
            if v != 0 {
                w.string(id, &flags_to_string(v, &BOUND_FLAG_NAMES));
            }
        };
        flag(w, "TypeFlags", self.composite_flags1.type_flags);
        flag(w, "IncludeFlags", self.composite_flags1.include_flags);
        flag(w, "OwnedTypeFlags", self.composite_flags2.type_flags);
        flag(w, "OwnedIncludeFlags", self.composite_flags2.include_flags);

        match &self.shape {
            BoundsShape::Basic | BoundsShape::Disc(_) => {}
            BoundsShape::Capsule(c) => {
                if c.half_height != 0.0 {
                    w.f32("HalfHeight", c.half_height);
                }
            }
            BoundsShape::Cylinder(c) => {
                if c.half_height != 0.0 {
                    w.f32("HalfHeight", c.half_height);
                }
            }
            BoundsShape::TaperedCapsule(c) => {
                for (id, v) in [
                    ("HalfHeight", c.half_height),
                    ("Unknown74", c.unknown_74h),
                    ("Unknown78", c.unknown_78h),
                    ("Unknown7C", c.unknown_7ch),
                    ("Unknown80", c.unknown_80h),
                ] {
                    if v != 0.0 {
                        w.f32(id, v);
                    }
                }
            }
            BoundsShape::Geometry(g) => g.write_xml(w),
            BoundsShape::GeometryBvh(g, part) => {
                g.write_xml(w);
                if part.unknown_142h != 0 {
                    w.u16("Unknown_142h", part.unknown_142h);
                }
            }
            BoundsShape::Composite(c) => {
                w.node_array_opt("Bounds", c.children.as_deref(), |w, b| b.write_xml(w));
                if c.children.as_ref().map_or(0, |k| k.len()) >= 6 && c.bvh.is_none() {
                    w.bool("NoBVH", true);
                }
            }
        }
    }

    /// Reads a bound from an XML node with a `type` attribute
    /// (`Rsc8Bounds.Create(type)` then `Read(MetaNodeReader)`).
    pub fn read_xml(n: &XmlNode) -> Result<Bounds> {
        let ty = n.string("@type").unwrap_or_default();
        let Some(t) = BoundsType::parse(&ty) else { bail!("unknown bounds type \"{ty}\"") };
        let mut b = Bounds::new(t);
        b.box_min = n.vec3("BoxMin");
        b.box_max = n.vec3("BoxMax");
        b.box_center = n.vec3("BoxCenter");
        b.sphere_center = n.vec3("SphereCenter");
        b.sphere_radius = n.f32("SphereRadius", 0.0);
        b.margin = n.f32("Margin", 0.0);
        b.mass = n.f32("Mass", 0.0);
        b.inertia = n.vec3("Inertia");
        b.unknown_11h = n.u8("Unknown_11h", 0);
        b.unknown_3ch = n.u32("Unknown_3Ch", 1);
        b.material = BoundMaterial1::read_xml(n, "Material");
        b.material2 = BoundMaterial2::read_xml(n, "Material");
        b.transform = n.matrix::<16>("Transform").unwrap_or(IDENTITY);
        let flag = |id: &str| n.string(id).and_then(|s| parse_flags(&s, &BOUND_FLAG_NAMES, 64)).unwrap_or(0);
        b.composite_flags1 = CompositeFlags { type_flags: flag("TypeFlags"), include_flags: flag("IncludeFlags") };
        b.composite_flags2 =
            CompositeFlags { type_flags: flag("OwnedTypeFlags"), include_flags: flag("OwnedIncludeFlags") };

        match &mut b.shape {
            BoundsShape::Basic | BoundsShape::Disc(_) => {}
            BoundsShape::Capsule(c) => c.half_height = n.f32("HalfHeight", 0.0),
            BoundsShape::Cylinder(c) => c.half_height = n.f32("HalfHeight", 0.0),
            BoundsShape::TaperedCapsule(c) => {
                c.half_height = n.f32("HalfHeight", 0.0);
                c.unknown_74h = n.f32("Unknown74", 0.0);
                c.unknown_78h = n.f32("Unknown78", 0.0);
                c.unknown_7ch = n.f32("Unknown7C", 0.0);
                c.unknown_80h = n.f32("Unknown80", 0.0);
            }
            BoundsShape::Geometry(g) => **g = BoundGeometry::read_xml(n),
            BoundsShape::GeometryBvh(g, part) => {
                **g = BoundGeometry::read_xml(n);
                part.unknown_142h = n.u16("Unknown_142h", 0);
            }
            BoundsShape::Composite(c) => {
                c.children = match n.node_array("Bounds") {
                    None => None,
                    Some(items) => {
                        let mut kids = Vec::with_capacity(items.len());
                        for item in items {
                            kids.push(match item {
                                Some(item) => Some(Bounds::read_xml(&item)?),
                                None => None,
                            });
                        }
                        Some(kids)
                    }
                };
                c.no_bvh = n.bool("NoBVH", false);
            }
        }
        Ok(b)
    }
}

/// `BoundingBox.Transform(matrix)`.
fn transform_box(min: Vec3, max: Vec3, m: &Mat4) -> (Vec3, Vec3) {
    let abs: Mat4 = m.map(f32::abs);
    let centre = math::scale(math::add(max, min), 0.5);
    let extent = math::scale(math::sub(max, min), 0.5);
    let c = math::transform_coordinate(centre, m);
    let e = math::abs(math::transform_normal(extent, &abs));
    (math::sub(c, e), math::add(c, e))
}

/// `Rsc8BoundComposite.Read`: the header, then the children and arrays, then
/// what each child gets from them.
fn read_composite(br: &mut BoundsReader, c: &mut BoundComposite) -> Result<Vec<u64>> {
    let bounds_ptr = br.r.u64()?;
    let current_ptr = br.r.u64()?;
    let last_ptr = br.r.u64()?;
    let flags_ptr = br.r.u64()?;
    let owned_ptr = br.r.u64()?;
    c.max_num_bounds = br.r.u16()?;
    c.num_bounds = br.r.u16()?;
    c.unknown_a4h = br.r.u32()?;
    let bvh_ptr = br.r.u64()?;
    c.bvh = br.block(Tag::Bvh, bvh_ptr, Bvh::read)?;

    let n = c.max_num_bounds as usize;
    let pointers: Option<Vec<u64>> = br.array(Tag::U64, bounds_ptr, n)?;
    let mut children = Vec::with_capacity(n);
    for i in 0..n {
        let Some(p) = pointers.as_ref().and_then(|p| p.get(i).copied()) else {
            bail!("composite has {n} children but no pointer for child {i}")
        };
        br.load(p)?;
        children.push(p);
    }
    c.current_matrices = br.array(Tag::Matrix, current_ptr, n)?;
    c.last_matrices = br.array(Tag::Matrix, last_ptr, n)?;
    let (flags1, id1) = br.array_id(Tag::Flags, flags_ptr, n)?;
    let (flags2, id2) = br.array_id(Tag::Flags, owned_ptr, n)?;
    c.type_and_include_flags = flags1;
    c.owned_type_and_include_flags = flags2;

    // The owned flags count only if they're another array than the first.
    let owned = if id1 != id2 { c.owned_type_and_include_flags.as_deref() } else { None };
    let transforms = c.current_matrices.as_deref().or(c.last_matrices.as_deref());
    for (i, &p) in children.iter().enumerate() {
        if p == 0 {
            continue;
        }
        let mut transform = transforms.and_then(|t| t.get(i).copied()).unwrap_or(IDENTITY);
        transform[15] = 1.0;
        let flags1 = c.type_and_include_flags.as_deref().and_then(|f| f.get(i).copied()).unwrap_or_default();
        let flags2 = owned.and_then(|f| f.get(i).copied()).unwrap_or_default();
        br.set_info(p, ChildInfo { transform, flags1, flags2 });
    }
    Ok(children)
}
