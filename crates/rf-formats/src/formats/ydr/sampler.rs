//! Sampler dictionaries (`Rsc8SamplerDictionary`, `Rsc8DrawableSampler`):
//! triangles of a drawable to sample positions from, with their own drawable.

use super::drawable::{DrawableBase, node_array_with, node_with};
use super::util::{
    ArrHeader, StructField, node_items, read_array, read_ptr_arr, read_struct_array, write_arr, write_ptr_arr,
    write_struct_array,
};
use crate::hash::{HashIndex, joaat};
use crate::rsc8::{BlockBuf, BlockId, BlockMap, Rsc8Reader, Rsc8Writer};
use crate::xml::{XmlNode, XmlWriter};
use crate::{Result, bail};

/// `Rsc8SamplerTri` (`rage::rmcDrawableSampler::Tri`, 112 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct SamplerTri {
    pub point1: [f32; 4],
    pub point2: [f32; 4],
    pub point3: [f32; 4],
    pub square1: [f32; 4],
    pub square2: [f32; 4],
    pub square3: [f32; 4],
    pub length_square: [f32; 4],
}

const TRI_FIELDS: [&str; 7] = ["Point1", "Point2", "Point3", "Square1", "Square2", "Square3", "LengthSquare"];

impl SamplerTri {
    fn values(&self) -> [[f32; 4]; 7] {
        [self.point1, self.point2, self.point3, self.square1, self.square2, self.square3, self.length_square]
    }

    fn fields(&self) -> Vec<StructField<'static>> {
        TRI_FIELDS.iter().zip(self.values()).map(|(n, v)| StructField::Vec4(n, v)).collect()
    }

    fn read_xml(n: XmlNode) -> Self {
        let v = TRI_FIELDS.map(|f| n.vec4(f));
        Self {
            point1: v[0],
            point2: v[1],
            point3: v[2],
            square1: v[3],
            square2: v[4],
            square3: v[5],
            length_square: v[6],
        }
    }
}

/// A `Rsc8Arr` of plain data as read: the items, capacity and last field.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Arr<T> {
    /// `None` for a null pointer or no items.
    pub items: Option<Vec<T>>,
    pub capacity: u16,
    pub unknown: u32,
}

impl<T: bytemuck::Pod> Arr<T> {
    fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let h = ArrHeader::read(r)?;
        Ok(Self { items: read_array(r, h.ptr, h.count as usize)?, capacity: h.capacity, unknown: h.unknown })
    }

    fn new(items: Option<Vec<T>>) -> Self {
        Self { capacity: items.as_ref().map_or(0, |i| i.len() as u16), items, unknown: 0 }
    }

    fn write(&self, b: &mut BlockBuf, w: &mut Rsc8Writer) {
        write_arr(b, w, self.items.as_deref(), self.capacity, self.unknown);
    }
}

/// `Rsc8DrawableSampler` (160 bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct DrawableSampler {
    pub unknown_0h: u64,
    pub unknown_8h: u64,
    pub unknown_10h: u64,
    pub triangles: Arr<SamplerTri>,
    pub floats1: Arr<f32>,
    pub floats2: Arr<f32>,
    pub ints: Arr<u32>,
    pub drawable: Option<Box<DrawableBase>>,
    pub unknown_60h: u64,
    pub runtime_data: u64,
    pub bounding_box_min: [f32; 3],
    pub unknown_7ch: f32,
    pub bounding_box_max: [f32; 3],
    pub unknown_8ch: f32,
    pub unknown_90h: u8,
    pub unknown_91h: u8,
    pub unknown_92h: u8,
    pub unknown_93h: u8,
    pub unknown_94h: u32,
    pub unknown_98h: u32,
    pub unknown_9ch: f32,
    /// From the dictionary's hashes.
    pub name_hash: u32,
}

impl Default for DrawableSampler {
    fn default() -> Self {
        Self {
            unknown_0h: 0,
            unknown_8h: 0,
            unknown_10h: 1,
            triangles: Arr::default(),
            floats1: Arr::default(),
            floats2: Arr::default(),
            ints: Arr::default(),
            drawable: None,
            unknown_60h: 0,
            runtime_data: 0,
            bounding_box_min: [0.0; 3],
            unknown_7ch: 0.0,
            bounding_box_max: [0.0; 3],
            unknown_8ch: 0.0,
            unknown_90h: 0,
            unknown_91h: 0,
            unknown_92h: 0,
            unknown_93h: 0,
            unknown_94h: 0,
            unknown_98h: 0,
            unknown_9ch: 0.0,
            name_hash: 0,
        }
    }
}

impl DrawableSampler {
    fn read(r: &mut Rsc8Reader, hashes: &HashIndex) -> Result<Self> {
        Ok(Self {
            unknown_0h: r.u64()?,
            unknown_8h: r.u64()?,
            unknown_10h: r.u64()?,
            triangles: Arr::read(r)?,
            floats1: Arr::read(r)?,
            floats2: Arr::read(r)?,
            ints: Arr::read(r)?,
            drawable: r.ptr(|r| DrawableBase::read(r, hashes))?.map(Box::new),
            unknown_60h: r.u64()?,
            runtime_data: r.u64()?,
            bounding_box_min: r.vec3()?,
            unknown_7ch: r.f32()?,
            bounding_box_max: r.vec3()?,
            unknown_8ch: r.f32()?,
            unknown_90h: r.u8()?,
            unknown_91h: r.u8()?,
            unknown_92h: r.u8()?,
            unknown_93h: r.u8()?,
            unknown_94h: r.u32()?,
            unknown_98h: r.u32()?,
            unknown_9ch: r.f32()?,
            name_hash: 0,
        })
    }

    fn write(&self, w: &mut Rsc8Writer) -> Result<BlockId> {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(160);
        b.u64(self.unknown_0h);
        b.u64(self.unknown_8h);
        b.u64(self.unknown_10h);
        self.triangles.write(&mut b, w);
        self.floats1.write(&mut b, w);
        self.floats2.write(&mut b, w);
        self.ints.write(&mut b, w);
        b.ptr(self.drawable.as_ref().map(|d| d.write(w)).transpose()?);
        b.u64(self.unknown_60h);
        b.u64(self.runtime_data);
        b.vec3(self.bounding_box_min);
        b.f32(self.unknown_7ch);
        b.vec3(self.bounding_box_max);
        b.f32(self.unknown_8ch);
        b.u8(self.unknown_90h);
        b.u8(self.unknown_91h);
        b.u8(self.unknown_92h);
        b.u8(self.unknown_93h);
        b.u32(self.unknown_94h);
        b.u32(self.unknown_98h);
        b.f32(self.unknown_9ch);
        w.set(id, b, false);
        Ok(id)
    }

    fn write_xml(&self, w: &mut XmlWriter) -> Result<()> {
        w.hash("Name", self.name_hash);
        w.vec3("BoundingBoxMin", self.bounding_box_min);
        w.vec3("BoundingBoxMax", self.bounding_box_max);
        w.u8("Unknown_90", self.unknown_90h);
        w.u8("Unknown_91", self.unknown_91h);
        w.u8("Unknown_92", self.unknown_92h);
        write_struct_array(w, "TrianglesData", self.triangles.items.as_deref(), SamplerTri::fields);
        w.f32_array("FloatsArray1", self.floats1.items.as_deref());
        w.f32_array("FloatsArray2", self.floats2.items.as_deref());
        w.u32_array("IntsArray", self.ints.items.as_deref());
        if let Some(d) = &self.drawable {
            node_with(w, "Drawable", |w| d.write_xml(w))?;
        }
        Ok(())
    }

    fn read_xml(node: XmlNode) -> Result<Self> {
        Ok(Self {
            name_hash: node.string("Name").map_or(0, |n| joaat(&n)),
            bounding_box_min: node.vec3("BoundingBoxMin"),
            bounding_box_max: node.vec3("BoundingBoxMax"),
            unknown_90h: node.u8("Unknown_90", 0),
            unknown_91h: node.u8("Unknown_91", 0),
            unknown_92h: node.u8("Unknown_92", 0),
            triangles: Arr::new(read_struct_array(node, "TrianglesData", SamplerTri::read_xml)),
            floats1: Arr::new(node.f32_array("FloatsArray1")),
            floats2: Arr::new(node.f32_array("FloatsArray2")),
            ints: Arr::new(node.u32_array("IntsArray")),
            drawable: node.node("Drawable").map(DrawableBase::read_xml).transpose()?.map(Box::new),
            ..Default::default()
        })
    }
}

/// `Rsc8SamplerDictionary` (64 bytes): samplers sorted by name hash.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SamplerDictionary {
    pub vft: u64,
    pub block_map: Option<BlockMap>,
    pub unknown_10h: u64,
    pub unknown_18h: u64,
    pub hashes: Arr<u32>,
    pub samplers: Option<Vec<Option<DrawableSampler>>>,
    pub samplers_capacity: u16,
    pub samplers_unknown: u32,
}

impl SamplerDictionary {
    pub fn read(r: &mut Rsc8Reader, hashes: &HashIndex) -> Result<Self> {
        let mut d = Self { vft: r.u64()?, ..Default::default() };
        d.block_map = r.ptr(BlockMap::read)?;
        d.unknown_10h = r.u64()?;
        d.unknown_18h = r.u64()?;
        d.hashes = Arr::read(r)?;
        let sh = ArrHeader::read(r)?;
        d.samplers = read_ptr_arr(r, &sh, |r| DrawableSampler::read(r, hashes))?;
        d.samplers_capacity = sh.capacity;
        d.samplers_unknown = sh.unknown;
        if let (Some(h), Some(s)) = (&d.hashes.items, &mut d.samplers) {
            for (h, s) in h.iter().zip(s.iter_mut()) {
                let Some(s) = s else { bail!("a null sampler in the dictionary (the C# fails on it)") };
                s.name_hash = *h;
            }
        }
        Ok(d)
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> Result<BlockId> {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(64);
        b.u64(self.vft);
        b.ptr(self.block_map.as_ref().map(|m| m.write(w)));
        b.u64(self.unknown_10h);
        b.u64(self.unknown_18h);
        self.hashes.write(&mut b, w);
        let ss = match &self.samplers {
            Some(v) => Some(v.iter().map(|s| s.as_ref().map(|s| s.write(w)).transpose()).collect::<Result<Vec<_>>>()?),
            None => None,
        };
        write_ptr_arr(&mut b, w, ss.as_deref(), self.samplers_capacity, self.samplers_unknown);
        w.set(id, b, false);
        Ok(id)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) -> Result<()> {
        node_array_with(w, "Samplers", self.samplers.as_deref(), |w, s| s.write_xml(w))
    }

    /// `Build`: the samplers sorted by name hash.
    pub fn read_xml(node: XmlNode) -> Result<Self> {
        let Some(mut samplers) = node_items(node, "Samplers", DrawableSampler::read_xml)? else {
            bail!("a sampler dictionary without Samplers (the C# fails on it)");
        };
        samplers.sort_by_key(|s| s.name_hash);
        let n = samplers.len() as u16;
        Ok(Self {
            hashes: Arr::new(Some(samplers.iter().map(|s| s.name_hash).collect())),
            samplers: Some(samplers.into_iter().map(Some).collect()),
            samplers_capacity: n,
            ..Default::default()
        })
    }
}
