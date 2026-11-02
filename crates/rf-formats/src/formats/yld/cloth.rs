//! The cloth simulation structs characters and fragments share: controllers
//! (`Rsc8ClothController`, `Rsc8CharacterClothController`), the verlet cloth
//! (`Rsc8VerletCloth`), the bridge between simulation and drawable
//! (`Rsc8ClothBridgeSimGfx`), the morph controller and the instance tuning.

use super::util::{
    Arr, Arr32, ascii_bytes, ascii_string, field_f32, field_u16, field_u32, field_vec3, field_vec4, read_structs,
    write_structs,
};
use crate::formats::ydr::{Bone, Skeleton};
use crate::rsc8::{BlockBuf, BlockId, Rsc8Reader, Rsc8Writer};
use crate::xml::{XmlNode, XmlWriter};
use crate::{Result, bail};

/// `Rsc8ClothControllerVertex` (16 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct ClothControllerVertex {
    pub position: [f32; 3],
    pub unknown_c: u16,
    pub unknown_e: u16,
}

impl ClothControllerVertex {
    fn write_xml(&self, w: &mut XmlWriter) {
        field_vec3(w, "Position", self.position);
        field_u16(w, "UnknownC", self.unknown_c);
        field_u16(w, "UnknownE", self.unknown_e);
    }

    fn read_xml(n: XmlNode) -> Self {
        Self { position: n.vec3("Position"), unknown_c: n.u16("UnknownC", 0), unknown_e: n.u16("UnknownE", 0) }
    }
}

/// `Rsc8ClothBoneWeightsInds` (64 bytes): a vertex's 8 bone weights and indices.
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct ClothBoneWeightsInds {
    pub weights: [f32; 8],
    pub indices: [u32; 8],
}

const WEIGHT_NAMES: [&str; 8] =
    ["Weight0", "Weight1", "Weight2", "Weight3", "Weight4", "Weight5", "Weight6", "Weight7"];
const INDEX_NAMES: [&str; 8] = ["Index0", "Index1", "Index2", "Index3", "Index4", "Index5", "Index6", "Index7"];

impl ClothBoneWeightsInds {
    fn write_xml(&self, w: &mut XmlWriter) {
        for (n, v) in WEIGHT_NAMES.iter().zip(self.weights) {
            field_f32(w, n, v);
        }
        for (n, v) in INDEX_NAMES.iter().zip(self.indices) {
            field_u32(w, n, v);
        }
    }

    fn read_xml(n: XmlNode) -> Self {
        Self { weights: WEIGHT_NAMES.map(|id| n.f32(id, 0.0)), indices: INDEX_NAMES.map(|id| n.u32(id, 0)) }
    }
}

/// `Rsc8MorphControllerUnk1` (48 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct MorphControllerUnk1 {
    pub unknown0: [f32; 4],
    pub unknown1: u16,
    pub unknown2: u16,
    pub unknown3: u16,
    pub unknown4: u16,
    pub unknown5: u32,
    pub unknown6: u32,
    /// `Unknown7` to `Unknown14`.
    pub unknown7: [u16; 8],
}

const UNK1_NAMES: [&str; 8] =
    ["Unknown7", "Unknown8", "Unknown9", "Unknown10", "Unknown11", "Unknown12", "Unknown13", "Unknown14"];

impl MorphControllerUnk1 {
    fn write_xml(&self, w: &mut XmlWriter) {
        field_vec4(w, "Unknown0", self.unknown0);
        field_u16(w, "Unknown1", self.unknown1);
        field_u16(w, "Unknown2", self.unknown2);
        field_u16(w, "Unknown3", self.unknown3);
        field_u16(w, "Unknown4", self.unknown4);
        field_u32(w, "Unknown5", self.unknown5);
        field_u32(w, "Unknown6", self.unknown6);
        for (n, v) in UNK1_NAMES.iter().zip(self.unknown7) {
            field_u16(w, n, v);
        }
    }

    fn read_xml(n: XmlNode) -> Self {
        Self {
            unknown0: n.vec4("Unknown0"),
            unknown1: n.u16("Unknown1", 0),
            unknown2: n.u16("Unknown2", 0),
            unknown3: n.u16("Unknown3", 0),
            unknown4: n.u16("Unknown4", 0),
            unknown5: n.u32("Unknown5", 0),
            unknown6: n.u32("Unknown6", 0),
            unknown7: UNK1_NAMES.map(|id| n.u16(id, 0)),
        }
    }
}

/// `Rsc8VerletClothConstraint` (32 bytes): two vertices and how they hold.
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct VerletClothConstraint {
    pub unknown00: u16,
    pub unknown02: u16,
    /// `Unknown04` to `Unknown1C`.
    pub unknown04: [f32; 7],
}

const CONSTRAINT_NAMES: [&str; 7] =
    ["Unknown04", "Unknown08", "Unknown0C", "Unknown10", "Unknown14", "Unknown18", "Unknown1C"];

impl VerletClothConstraint {
    fn write_xml(&self, w: &mut XmlWriter) {
        field_u16(w, "Unknown00", self.unknown00);
        field_u16(w, "Unknown02", self.unknown02);
        for (n, v) in CONSTRAINT_NAMES.iter().zip(self.unknown04) {
            field_f32(w, n, v);
        }
    }

    fn read_xml(n: XmlNode) -> Self {
        Self {
            unknown00: n.u16("Unknown00", 0),
            unknown02: n.u16("Unknown02", 0),
            unknown04: CONSTRAINT_NAMES.map(|id| n.f32(id, 0.0)),
        }
    }
}

/// `Rsc8ClothBridgeSimGfx` (0x240 bytes): per-vertex values mapping the
/// simulation to the drawable, for each of 3 LODs. The `u64`s are zero in the
/// game's files.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClothBridgeSimGfx {
    pub vft: u64,
    pub unknown_8h: u64,
    pub unknown_10h: u64,
    pub unknown_18h: u64,
    pub unknown_20h: Arr<f32>,
    pub unknown_30h: Arr<f32>,
    pub unknown_40h: Arr<f32>,
    pub unknown_50h: u64,
    pub unknown_58h: u64,
    pub unknown_60h: Arr<f32>,
    pub unknown_70h: Arr<f32>,
    pub unknown_80h: Arr<f32>,
    pub unknown_90h: u64,
    pub unknown_98h: u64,
    pub unknown_a0h: Arr<f32>,
    pub unknown_b0h: Arr<f32>,
    pub unknown_c0h: Arr<f32>,
    pub unknown_d0h: u64,
    pub unknown_d8h: u64,
    pub unknown_e0h: Arr<f32>,
    pub unknown_f0h: Arr<f32>,
    /// Not in the XML (the C# writes `UnknownA0` in its place).
    pub unknown_100h: Arr<f32>,
    pub unknown_110h: u64,
    pub unknown_118h: u64,
    pub unknown_120h: Arr<f32>,
    pub unknown_130h: Arr<f32>,
    pub unknown_140h: Arr<f32>,
    pub unknown_150h: u64,
    pub unknown_158h: u64,
    pub unknown_160h: Arr<u16>,
    pub unknown_170h: Arr<u16>,
    pub unknown_180h: Arr<u16>,
    pub unknown_190h: u64,
    pub unknown_198h: u64,
    pub unknown_1a0h: Arr<u32>,
    pub unknown_1b0h: Arr<u32>,
    pub unknown_1c0h: Arr<u32>,
    pub unknown_1d0h: u64,
    pub unknown_1d8h: u64,
    pub unknown_1e0h: Arr<f32>,
    pub unknown_1f0h: Arr<f32>,
    pub unknown_200h: Arr<f32>,
    /// 0x210 to 0x238.
    pub unknown_210h: [u64; 6],
}

impl ClothBridgeSimGfx {
    pub const BLOCK_LENGTH: usize = 0x240;

    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        Ok(Self {
            vft: r.u64()?,
            unknown_8h: r.u64()?,
            unknown_10h: r.u64()?,
            unknown_18h: r.u64()?,
            unknown_20h: Arr::read(r)?,
            unknown_30h: Arr::read(r)?,
            unknown_40h: Arr::read(r)?,
            unknown_50h: r.u64()?,
            unknown_58h: r.u64()?,
            unknown_60h: Arr::read(r)?,
            unknown_70h: Arr::read(r)?,
            unknown_80h: Arr::read(r)?,
            unknown_90h: r.u64()?,
            unknown_98h: r.u64()?,
            unknown_a0h: Arr::read(r)?,
            unknown_b0h: Arr::read(r)?,
            unknown_c0h: Arr::read(r)?,
            unknown_d0h: r.u64()?,
            unknown_d8h: r.u64()?,
            unknown_e0h: Arr::read(r)?,
            unknown_f0h: Arr::read(r)?,
            unknown_100h: Arr::read(r)?,
            unknown_110h: r.u64()?,
            unknown_118h: r.u64()?,
            unknown_120h: Arr::read(r)?,
            unknown_130h: Arr::read(r)?,
            unknown_140h: Arr::read(r)?,
            unknown_150h: r.u64()?,
            unknown_158h: r.u64()?,
            unknown_160h: Arr::read(r)?,
            unknown_170h: Arr::read(r)?,
            unknown_180h: Arr::read(r)?,
            unknown_190h: r.u64()?,
            unknown_198h: r.u64()?,
            unknown_1a0h: Arr::read(r)?,
            unknown_1b0h: Arr::read(r)?,
            unknown_1c0h: Arr::read(r)?,
            unknown_1d0h: r.u64()?,
            unknown_1d8h: r.u64()?,
            unknown_1e0h: Arr::read(r)?,
            unknown_1f0h: Arr::read(r)?,
            unknown_200h: Arr::read(r)?,
            unknown_210h: [r.u64()?, r.u64()?, r.u64()?, r.u64()?, r.u64()?, r.u64()?],
        })
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u64(self.vft);
        b.u64(self.unknown_8h);
        b.u64(self.unknown_10h);
        b.u64(self.unknown_18h);
        self.unknown_20h.write(&mut b, w);
        self.unknown_30h.write(&mut b, w);
        self.unknown_40h.write(&mut b, w);
        b.u64(self.unknown_50h);
        b.u64(self.unknown_58h);
        self.unknown_60h.write(&mut b, w);
        self.unknown_70h.write(&mut b, w);
        self.unknown_80h.write(&mut b, w);
        b.u64(self.unknown_90h);
        b.u64(self.unknown_98h);
        self.unknown_a0h.write(&mut b, w);
        self.unknown_b0h.write(&mut b, w);
        self.unknown_c0h.write(&mut b, w);
        b.u64(self.unknown_d0h);
        b.u64(self.unknown_d8h);
        self.unknown_e0h.write(&mut b, w);
        self.unknown_f0h.write(&mut b, w);
        self.unknown_100h.write(&mut b, w);
        b.u64(self.unknown_110h);
        b.u64(self.unknown_118h);
        self.unknown_120h.write(&mut b, w);
        self.unknown_130h.write(&mut b, w);
        self.unknown_140h.write(&mut b, w);
        b.u64(self.unknown_150h);
        b.u64(self.unknown_158h);
        self.unknown_160h.write(&mut b, w);
        self.unknown_170h.write(&mut b, w);
        self.unknown_180h.write(&mut b, w);
        b.u64(self.unknown_190h);
        b.u64(self.unknown_198h);
        self.unknown_1a0h.write(&mut b, w);
        self.unknown_1b0h.write(&mut b, w);
        self.unknown_1c0h.write(&mut b, w);
        b.u64(self.unknown_1d0h);
        b.u64(self.unknown_1d8h);
        self.unknown_1e0h.write(&mut b, w);
        self.unknown_1f0h.write(&mut b, w);
        self.unknown_200h.write(&mut b, w);
        for v in self.unknown_210h {
            b.u64(v);
        }
        w.set(id, b, false);
        id
    }

    /// The XML, in the C#'s order: `UnknownA0` is written twice and
    /// `Unknown_100h` not at all.
    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.f32_array("Unknown20", self.unknown_20h.items.as_deref());
        w.f32_array("Unknown60", self.unknown_60h.items.as_deref());
        w.f32_array("UnknownA0", self.unknown_a0h.items.as_deref());
        w.f32_array("UnknownE0", self.unknown_e0h.items.as_deref());
        w.f32_array("Unknown120", self.unknown_120h.items.as_deref());
        w.u16_array("Unknown160", self.unknown_160h.items.as_deref());
        w.u32_array("Unknown1A0", self.unknown_1a0h.items.as_deref());
        w.f32_array("Unknown1E0", self.unknown_1e0h.items.as_deref());
        w.f32_array("Unknown30", self.unknown_30h.items.as_deref());
        w.f32_array("Unknown70", self.unknown_70h.items.as_deref());
        w.f32_array("UnknownB0", self.unknown_b0h.items.as_deref());
        w.f32_array("UnknownF0", self.unknown_f0h.items.as_deref());
        w.f32_array("Unknown130", self.unknown_130h.items.as_deref());
        w.u16_array("Unknown170", self.unknown_170h.items.as_deref());
        w.u32_array("Unknown1B0", self.unknown_1b0h.items.as_deref());
        w.f32_array("Unknown1F0", self.unknown_1f0h.items.as_deref());
        w.f32_array("Unknown40", self.unknown_40h.items.as_deref());
        w.f32_array("Unknown80", self.unknown_80h.items.as_deref());
        w.f32_array("UnknownC0", self.unknown_c0h.items.as_deref());
        w.f32_array("UnknownA0", self.unknown_a0h.items.as_deref());
        w.f32_array("Unknown140", self.unknown_140h.items.as_deref());
        w.u16_array("Unknown180", self.unknown_180h.items.as_deref());
        w.u32_array("Unknown1C0", self.unknown_1c0h.items.as_deref());
        w.f32_array("Unknown200", self.unknown_200h.items.as_deref());
    }

    pub fn read_xml(n: XmlNode) -> Self {
        Self {
            unknown_20h: Arr::new(n.f32_array("Unknown20")),
            unknown_60h: Arr::new(n.f32_array("Unknown60")),
            unknown_a0h: Arr::new(n.f32_array("UnknownA0")),
            unknown_e0h: Arr::new(n.f32_array("UnknownE0")),
            unknown_120h: Arr::new(n.f32_array("Unknown120")),
            unknown_160h: Arr::new(n.u16_array("Unknown160")),
            unknown_1a0h: Arr::new(n.u32_array("Unknown1A0")),
            unknown_1e0h: Arr::new(n.f32_array("Unknown1E0")),
            unknown_30h: Arr::new(n.f32_array("Unknown30")),
            unknown_70h: Arr::new(n.f32_array("Unknown70")),
            unknown_b0h: Arr::new(n.f32_array("UnknownB0")),
            unknown_f0h: Arr::new(n.f32_array("UnknownF0")),
            unknown_130h: Arr::new(n.f32_array("Unknown130")),
            unknown_170h: Arr::new(n.u16_array("Unknown170")),
            unknown_1b0h: Arr::new(n.u32_array("Unknown1B0")),
            unknown_1f0h: Arr::new(n.f32_array("Unknown1F0")),
            unknown_40h: Arr::new(n.f32_array("Unknown40")),
            unknown_80h: Arr::new(n.f32_array("Unknown80")),
            unknown_c0h: Arr::new(n.f32_array("UnknownC0")),
            unknown_140h: Arr::new(n.f32_array("Unknown140")),
            unknown_180h: Arr::new(n.u16_array("Unknown180")),
            unknown_1c0h: Arr::new(n.u32_array("Unknown1C0")),
            unknown_200h: Arr::new(n.f32_array("Unknown200")),
            ..Default::default()
        }
    }
}

/// `Rsc8MorphController` (0x240 bytes). The `u64`s not named otherwise are
/// zero in the game's files.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MorphController {
    pub vft: u64,
    pub unknown_8h: u64,
    pub unknown_10h: u64,
    pub unknown_18h: Arr<MorphControllerUnk1>,
    pub unknown_28h: Arr<MorphControllerUnk1>,
    pub unknown_38h: Arr<MorphControllerUnk1>,
    pub unknown_48h: u64,
    pub unknown_50h: u64,
    pub unknown_58h: Arr<u16>,
    pub unknown_68h: Arr<u16>,
    pub unknown_78h: Arr<u16>,
    pub unknown_88h: u64,
    pub unknown_90h: u64,
    /// A count?
    pub unknown_98h: u64,
    /// 0xA0 to 0xB8.
    pub unknown_a0h: [u64; 4],
    pub unknown_c0h: Arr<MorphControllerUnk1>,
    /// 0xD0 to 0xE8.
    pub unknown_d0h: [u64; 4],
    pub unknown_f0h: Arr<u16>,
    pub unknown_100h: Arr<u16>,
    pub unknown_110h: u64,
    pub unknown_118h: u64,
    /// A count?
    pub unknown_120h: u64,
    /// 0x128 to 0x180.
    pub unknown_128h: [u64; 12],
    pub unknown_188h: Arr<u16>,
    pub unknown_198h: u64,
    pub unknown_1a0h: u64,
    /// A count? Not in the XML.
    pub unknown_1a8h: u64,
    /// 0x1B0 to 0x238.
    pub unknown_1b0h: [u64; 18],
}

impl MorphController {
    pub const BLOCK_LENGTH: usize = 0x240;

    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let mut m = Self {
            vft: r.u64()?,
            unknown_8h: r.u64()?,
            unknown_10h: r.u64()?,
            unknown_18h: Arr::read(r)?,
            unknown_28h: Arr::read(r)?,
            unknown_38h: Arr::read(r)?,
            unknown_48h: r.u64()?,
            unknown_50h: r.u64()?,
            unknown_58h: Arr::read(r)?,
            unknown_68h: Arr::read(r)?,
            unknown_78h: Arr::read(r)?,
            unknown_88h: r.u64()?,
            unknown_90h: r.u64()?,
            unknown_98h: r.u64()?,
            ..Default::default()
        };
        for v in &mut m.unknown_a0h {
            *v = r.u64()?;
        }
        m.unknown_c0h = Arr::read(r)?;
        for v in &mut m.unknown_d0h {
            *v = r.u64()?;
        }
        m.unknown_f0h = Arr::read(r)?;
        m.unknown_100h = Arr::read(r)?;
        m.unknown_110h = r.u64()?;
        m.unknown_118h = r.u64()?;
        m.unknown_120h = r.u64()?;
        for v in &mut m.unknown_128h {
            *v = r.u64()?;
        }
        m.unknown_188h = Arr::read(r)?;
        m.unknown_198h = r.u64()?;
        m.unknown_1a0h = r.u64()?;
        m.unknown_1a8h = r.u64()?;
        for v in &mut m.unknown_1b0h {
            *v = r.u64()?;
        }
        Ok(m)
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u64(self.vft);
        b.u64(self.unknown_8h);
        b.u64(self.unknown_10h);
        self.unknown_18h.write(&mut b, w);
        self.unknown_28h.write(&mut b, w);
        self.unknown_38h.write(&mut b, w);
        b.u64(self.unknown_48h);
        b.u64(self.unknown_50h);
        self.unknown_58h.write(&mut b, w);
        self.unknown_68h.write(&mut b, w);
        self.unknown_78h.write(&mut b, w);
        b.u64(self.unknown_88h);
        b.u64(self.unknown_90h);
        b.u64(self.unknown_98h);
        self.unknown_a0h.iter().for_each(|v| b.u64(*v));
        self.unknown_c0h.write(&mut b, w);
        self.unknown_d0h.iter().for_each(|v| b.u64(*v));
        self.unknown_f0h.write(&mut b, w);
        self.unknown_100h.write(&mut b, w);
        b.u64(self.unknown_110h);
        b.u64(self.unknown_118h);
        b.u64(self.unknown_120h);
        self.unknown_128h.iter().for_each(|v| b.u64(*v));
        self.unknown_188h.write(&mut b, w);
        b.u64(self.unknown_198h);
        b.u64(self.unknown_1a0h);
        b.u64(self.unknown_1a8h);
        self.unknown_1b0h.iter().for_each(|v| b.u64(*v));
        w.set(id, b, false);
        id
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        let unk1 = |w: &mut XmlWriter, id: &str, a: &Arr<MorphControllerUnk1>| {
            write_structs(w, id, a.items.as_deref(), |w, s| s.write_xml(w))
        };
        unk1(w, "Unknown18", &self.unknown_18h);
        unk1(w, "Unknown28", &self.unknown_28h);
        unk1(w, "Unknown38", &self.unknown_38h);
        w.u16_array("Unknown58", self.unknown_58h.items.as_deref());
        w.u16_array("Unknown68", self.unknown_68h.items.as_deref());
        w.u16_array("Unknown78", self.unknown_78h.items.as_deref());
        unk1(w, "UnknownC0", &self.unknown_c0h);
        w.u16_array("UnknownF0", self.unknown_f0h.items.as_deref());
        w.u16_array("Unknown100", self.unknown_100h.items.as_deref());
        w.u16_array("Unknown188", self.unknown_188h.items.as_deref());
        w.u64("Unknown98", self.unknown_98h);
        w.u64("Unknown120", self.unknown_120h);
    }

    pub fn read_xml(n: XmlNode) -> Self {
        let unk1 = |id: &str| Arr::new(read_structs(n, id, MorphControllerUnk1::read_xml));
        Self {
            unknown_18h: unk1("Unknown18"),
            unknown_28h: unk1("Unknown28"),
            unknown_38h: unk1("Unknown38"),
            unknown_58h: Arr::new(n.u16_array("Unknown58")),
            unknown_68h: Arr::new(n.u16_array("Unknown68")),
            unknown_78h: Arr::new(n.u16_array("Unknown78")),
            unknown_c0h: unk1("UnknownC0"),
            unknown_f0h: Arr::new(n.u16_array("UnknownF0")),
            unknown_100h: Arr::new(n.u16_array("Unknown100")),
            unknown_188h: Arr::new(n.u16_array("Unknown188")),
            unknown_98h: n.u64("Unknown98", 0),
            unknown_120h: n.u64("Unknown120", 0),
            ..Default::default()
        }
    }
}

/// `Rsc8VerletCloth` (0x180 bytes): the simulated vertices and the
/// constraints between them. Fragments' environment cloth has one too.
#[derive(Clone, Debug, PartialEq)]
pub struct VerletCloth {
    pub vft: u64,
    /// 0x8 to 0x58 (a bounds pointer and bounding box were here in GTA V).
    pub unknown_8h: [u64; 11],
    pub vertices2: Arr<ClothControllerVertex>,
    pub unknown_70h: u64,
    pub unknown_78h: u64,
    /// Probably the vertex positions in the bind pose.
    pub vertices: Arr<ClothControllerVertex>,
    pub unknown_90h: u64,
    pub unknown_98h: u64,
    pub vertex_count: u16,
    /// Flags?
    pub unknown_a2h: u16,
    pub unknown_a4h: u32,
    pub unknown_a8h: f32,
    pub unknown_ach: f32,
    /// 0xB0 to 0xF8.
    pub unknown_b0h: [u64; 10],
    pub constraints2: Arr<VerletClothConstraint>,
    pub constraints: Arr<VerletClothConstraint>,
    /// 0x120 to 0x158.
    pub unknown_120h: [u64; 8],
    /// Indices?
    pub unknown_160h: Arr32<u16>,
    pub constraint_count: u32,
    /// 0 or 1.
    pub unknown_174h: u32,
    pub unknown_178h: u64,
}

impl Default for VerletCloth {
    fn default() -> Self {
        Self {
            vft: 0,
            unknown_8h: [0; 11],
            vertices2: Arr::default(),
            unknown_70h: 0,
            unknown_78h: 0,
            vertices: Arr::default(),
            unknown_90h: 0,
            unknown_98h: 0,
            vertex_count: 0,
            unknown_a2h: 0,
            unknown_a4h: 0,
            unknown_a8h: 9999.0,
            unknown_ach: 9999.0,
            unknown_b0h: [0; 10],
            constraints2: Arr::default(),
            constraints: Arr::default(),
            unknown_120h: [0; 8],
            unknown_160h: Arr32::default(),
            constraint_count: 0,
            unknown_174h: 0,
            unknown_178h: 0,
        }
    }
}

impl VerletCloth {
    pub const BLOCK_LENGTH: usize = 0x180;

    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let mut c = Self { vft: r.u64()?, ..Default::default() };
        for v in &mut c.unknown_8h {
            *v = r.u64()?;
        }
        c.vertices2 = Arr::read(r)?;
        c.unknown_70h = r.u64()?;
        c.unknown_78h = r.u64()?;
        c.vertices = Arr::read(r)?;
        c.unknown_90h = r.u64()?;
        c.unknown_98h = r.u64()?;
        c.vertex_count = r.u16()?;
        c.unknown_a2h = r.u16()?;
        c.unknown_a4h = r.u32()?;
        c.unknown_a8h = r.f32()?;
        c.unknown_ach = r.f32()?;
        for v in &mut c.unknown_b0h {
            *v = r.u64()?;
        }
        c.constraints2 = Arr::read(r)?;
        c.constraints = Arr::read(r)?;
        for v in &mut c.unknown_120h {
            *v = r.u64()?;
        }
        c.unknown_160h = Arr32::read(r)?;
        c.constraint_count = r.u32()?;
        c.unknown_174h = r.u32()?;
        c.unknown_178h = r.u64()?;
        Ok(c)
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u64(self.vft);
        self.unknown_8h.iter().for_each(|v| b.u64(*v));
        self.vertices2.write(&mut b, w);
        b.u64(self.unknown_70h);
        b.u64(self.unknown_78h);
        self.vertices.write(&mut b, w);
        b.u64(self.unknown_90h);
        b.u64(self.unknown_98h);
        b.u16(self.vertex_count);
        b.u16(self.unknown_a2h);
        b.u32(self.unknown_a4h);
        b.f32(self.unknown_a8h);
        b.f32(self.unknown_ach);
        self.unknown_b0h.iter().for_each(|v| b.u64(*v));
        self.constraints2.write(&mut b, w);
        self.constraints.write(&mut b, w);
        self.unknown_120h.iter().for_each(|v| b.u64(*v));
        self.unknown_160h.write(&mut b, w);
        b.u32(self.constraint_count);
        b.u32(self.unknown_174h);
        b.u64(self.unknown_178h);
        w.set(id, b, false);
        id
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.u16("UnknownA2", self.unknown_a2h);
        w.f32("UnknownA8", self.unknown_a8h);
        w.f32("UnknownAC", self.unknown_ach);
        w.u32("Unknown174", self.unknown_174h);
        w.u16_array("Unknown160", self.unknown_160h.items.as_deref());
        write_structs(w, "Vertices", self.vertices.items.as_deref(), |w, v| v.write_xml(w));
        write_structs(w, "Vertices2", self.vertices2.items.as_deref(), |w, v| v.write_xml(w));
        write_structs(w, "Constraints", self.constraints.items.as_deref(), |w, c| c.write_xml(w));
        write_structs(w, "Constraints2", self.constraints2.items.as_deref(), |w, c| c.write_xml(w));
    }

    /// Reads the XML; the vertex and constraint counts come from the arrays
    /// (the constraint count cut to 16 bits, as the C# does).
    pub fn read_xml(n: XmlNode) -> Self {
        let vertices = Arr::new(read_structs(n, "Vertices", ClothControllerVertex::read_xml));
        let constraints = Arr::new(read_structs(n, "Constraints", VerletClothConstraint::read_xml));
        Self {
            unknown_a2h: n.u16("UnknownA2", 0),
            unknown_a8h: n.f32("UnknownA8", 0.0),
            unknown_ach: n.f32("UnknownAC", 0.0),
            unknown_174h: n.u32("Unknown174", 0),
            unknown_160h: Arr32::new(n.u16_array("Unknown160")),
            vertex_count: vertices.items().len() as u16,
            constraint_count: constraints.items().len() as u16 as u32,
            vertices,
            vertices2: Arr::new(read_structs(n, "Vertices2", ClothControllerVertex::read_xml)),
            constraints,
            constraints2: Arr::new(read_structs(n, "Constraints2", VerletClothConstraint::read_xml)),
            ..Default::default()
        }
    }
}

/// `Rsc8ClothController` (0xA0 bytes): what simulates one cloth, with up to
/// 3 LODs of verlet cloth. Environment cloth (in fragments) has this one;
/// characters have a [`CharacterClothController`].
#[derive(Clone, Debug, PartialEq)]
pub struct ClothController {
    pub vft: u64,
    pub unknown_8h: u64,
    pub bridge_sim_gfx: Option<ClothBridgeSimGfx>,
    pub morph_controller: Option<MorphController>,
    pub verlet_cloth1: Option<VerletCloth>,
    pub verlet_cloth2: Option<VerletCloth>,
    pub verlet_cloth3: Option<VerletCloth>,
    pub unknown_38h: u64,
    pub unknown_40h: u64,
    pub unknown_48h: u64,
    /// `Type`: 3 in the game's files.
    pub cloth_type: u32,
    pub unknown_54h: u32,
    /// Up to 32 ASCII characters, in 56 bytes (what follows the name's NUL
    /// in the file isn't kept).
    pub name: String,
    pub unknown_90h: u64,
    pub unknown_98h: u32,
    pub unknown_9ch: f32,
}

impl Default for ClothController {
    fn default() -> Self {
        Self {
            vft: 0,
            unknown_8h: 0,
            bridge_sim_gfx: None,
            morph_controller: None,
            verlet_cloth1: None,
            verlet_cloth2: None,
            verlet_cloth3: None,
            unknown_38h: 0,
            unknown_40h: 0,
            unknown_48h: 0,
            cloth_type: 0,
            unknown_54h: 0,
            name: String::new(),
            unknown_90h: 0,
            unknown_98h: 0,
            unknown_9ch: 0.0,
        }
    }
}

impl ClothController {
    pub const BLOCK_LENGTH: usize = 0xA0;

    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let mut c = Self { vft: r.u64()?, unknown_8h: r.u64()?, ..Default::default() };
        c.bridge_sim_gfx = r.ptr(ClothBridgeSimGfx::read)?;
        c.morph_controller = r.ptr(MorphController::read)?;
        c.verlet_cloth1 = r.ptr(VerletCloth::read)?;
        c.verlet_cloth2 = r.ptr(VerletCloth::read)?;
        c.verlet_cloth3 = r.ptr(VerletCloth::read)?;
        c.unknown_38h = r.u64()?;
        c.unknown_40h = r.u64()?;
        c.unknown_48h = r.u64()?;
        c.cloth_type = r.u32()?;
        c.unknown_54h = r.u32()?;
        let name = r.bytes(56)?;
        // `ReadStringNullTerminated`, which fails without a NUL.
        let Some(end) = name.iter().position(|&b| b == 0) else {
            bail!("a cloth controller name without a NUL (the C# fails on it)")
        };
        c.name = ascii_string(&name[..end]);
        c.unknown_90h = r.u64()?;
        c.unknown_98h = r.u32()?;
        c.unknown_9ch = r.f32()?;
        Ok(c)
    }

    /// Writes the 0xA0 bytes into `b` (which a derived struct goes on
    /// filling), adding what they point to.
    pub fn write_fields(&self, b: &mut BlockBuf, w: &mut Rsc8Writer) {
        b.u64(self.vft);
        b.u64(self.unknown_8h);
        b.ptr(self.bridge_sim_gfx.as_ref().map(|x| x.write(w)));
        b.ptr(self.morph_controller.as_ref().map(|x| x.write(w)));
        b.ptr(self.verlet_cloth1.as_ref().map(|x| x.write(w)));
        b.ptr(self.verlet_cloth2.as_ref().map(|x| x.write(w)));
        b.ptr(self.verlet_cloth3.as_ref().map(|x| x.write(w)));
        b.u64(self.unknown_38h);
        b.u64(self.unknown_40h);
        b.u64(self.unknown_48h);
        b.u32(self.cloth_type);
        b.u32(self.unknown_54h);
        // The name, cut to 32 characters, padded with NULs to 56.
        let name: String = self.name.chars().take(32).collect();
        let mut bytes = ascii_bytes(&name);
        bytes.resize(56, 0);
        b.bytes(&bytes);
        b.u64(self.unknown_90h);
        b.u32(self.unknown_98h);
        b.f32(self.unknown_9ch);
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        self.write_fields(&mut b, w);
        w.set(id, b, false);
        id
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.string("Name", &self.name);
        w.u32("Type", self.cloth_type);
        if let Some(x) = &self.bridge_sim_gfx {
            w.node("BridgeSimGfx", |w| x.write_xml(w));
        }
        if let Some(x) = &self.morph_controller {
            w.node("MorphController", |w| x.write_xml(w));
        }
        for (id, c) in [
            ("VerletCloth1", &self.verlet_cloth1),
            ("VerletCloth2", &self.verlet_cloth2),
            ("VerletCloth3", &self.verlet_cloth3),
        ] {
            if let Some(c) = c {
                w.node(id, |w| c.write_xml(w));
            }
        }
        w.f32("Unknown9C", self.unknown_9ch);
    }

    pub fn read_xml(n: XmlNode) -> Result<Self> {
        let Some(name) = n.string("Name") else { bail!("a cloth controller without a Name (the C# fails on it)") };
        Ok(Self {
            name,
            cloth_type: n.u32("Type", 0),
            bridge_sim_gfx: n.node("BridgeSimGfx").map(ClothBridgeSimGfx::read_xml),
            morph_controller: n.node("MorphController").map(MorphController::read_xml),
            verlet_cloth1: n.node("VerletCloth1").map(VerletCloth::read_xml),
            verlet_cloth2: n.node("VerletCloth2").map(VerletCloth::read_xml),
            verlet_cloth3: n.node("VerletCloth3").map(VerletCloth::read_xml),
            unknown_9ch: n.f32("Unknown9C", 0.04),
            ..Default::default()
        })
    }
}

/// `Rsc8CharacterClothController` (0x110 bytes): a [`ClothController`] with
/// the vertices skinned to the character's bones.
#[derive(Clone, Debug, PartialEq)]
pub struct CharacterClothController {
    pub base: ClothController,
    pub unknown_a0h: Arr32<u16>,
    pub vertices: Arr<ClothControllerVertex>,
    /// 4 in the game's files.
    pub unknown_c0h: u64,
    pub unknown_c8h: u64,
    pub unknown_d0h: Arr<u32>,
    /// One per vertex.
    pub bone_weights_inds: Arr<ClothBoneWeightsInds>,
    pub unknown_f0h: u64,
    pub unknown_f8h: u32,
    /// 1 in the game's files.
    pub unknown_fch: f32,
    /// The tags of the bones `bone_weights_inds` indexes.
    pub bone_ids: Arr<u32>,
}

impl Default for CharacterClothController {
    fn default() -> Self {
        Self {
            base: ClothController::default(),
            unknown_a0h: Arr32::default(),
            vertices: Arr::default(),
            unknown_c0h: 0,
            unknown_c8h: 0,
            unknown_d0h: Arr::default(),
            bone_weights_inds: Arr::default(),
            unknown_f0h: 0,
            unknown_f8h: 0,
            unknown_fch: 0.0,
            bone_ids: Arr::default(),
        }
    }
}

impl CharacterClothController {
    pub const BLOCK_LENGTH: usize = 0x110;

    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        Ok(Self {
            base: ClothController::read(r)?,
            unknown_a0h: Arr32::read(r)?,
            vertices: Arr::read(r)?,
            unknown_c0h: r.u64()?,
            unknown_c8h: r.u64()?,
            unknown_d0h: Arr::read(r)?,
            bone_weights_inds: Arr::read(r)?,
            unknown_f0h: r.u64()?,
            unknown_f8h: r.u32()?,
            unknown_fch: r.f32()?,
            bone_ids: Arr::read(r)?,
        })
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        self.base.write_fields(&mut b, w);
        self.unknown_a0h.write(&mut b, w);
        self.vertices.write(&mut b, w);
        b.u64(self.unknown_c0h);
        b.u64(self.unknown_c8h);
        self.unknown_d0h.write(&mut b, w);
        self.bone_weights_inds.write(&mut b, w);
        b.u64(self.unknown_f0h);
        b.u32(self.unknown_f8h);
        b.f32(self.unknown_fch);
        self.bone_ids.write(&mut b, w);
        w.set(id, b, false);
        id
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        self.base.write_xml(w);
        w.u16_array("UnknownA0", self.unknown_a0h.items.as_deref());
        write_structs(w, "Vertices", self.vertices.items.as_deref(), |w, v| v.write_xml(w));
        w.u64("UnknownC0", self.unknown_c0h);
        w.u32_array("UnknownD0", self.unknown_d0h.items.as_deref());
        write_structs(w, "BoneWeightsInds", self.bone_weights_inds.items.as_deref(), |w, v| v.write_xml(w));
        w.f32("UnknownFC", self.unknown_fch);
        w.u32_array("BoneIds", self.bone_ids.items.as_deref());
    }

    /// The bone (index and bone) in `skeleton` of each of `bone_ids`, which
    /// `bone_weights_inds` indexes (the C#'s `Rsc8ClothInstance.Init`).
    pub fn bones<'a>(&self, skeleton: &'a Skeleton) -> Vec<Option<(usize, &'a Bone)>> {
        self.bone_ids.items().iter().map(|&tag| skeleton.bone_by_tag(tag as u16)).collect()
    }

    pub fn read_xml(n: XmlNode) -> Result<Self> {
        Ok(Self {
            base: ClothController::read_xml(n)?,
            unknown_a0h: Arr32::new(n.u16_array("UnknownA0")),
            vertices: Arr::new(read_structs(n, "Vertices", ClothControllerVertex::read_xml)),
            unknown_c0h: n.u64("UnknownC0", 4),
            unknown_d0h: Arr::new(n.u32_array("UnknownD0")),
            bone_weights_inds: Arr::new(read_structs(n, "BoneWeightsInds", ClothBoneWeightsInds::read_xml)),
            unknown_fch: n.f32("UnknownFC", 1.0),
            bone_ids: Arr::new(n.u32_array("BoneIds")),
            ..Default::default()
        })
    }
}

/// `Rsc8ClothInstanceTuning` (0x50 bytes), of fragments' environment cloth.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClothInstanceTuning {
    pub vft: u64,
    pub unknown_8h: u64,
    pub unknown_10h: f32,
    pub unknown_14h: f32,
    pub unknown_18h: u64,
    pub unknown_20h: f32,
    pub unknown_24h: f32,
    pub unknown_28h: f32,
    pub unknown_2ch: f32,
    pub unknown_30h: u32,
    pub unknown_34h: f32,
    pub unknown_38h: f32,
    pub unknown_3ch: f32,
    pub unknown_40h: f32,
    pub unknown_44h: f32,
    pub unknown_48h: f32,
    pub unknown_4ch: f32,
}

impl ClothInstanceTuning {
    pub const BLOCK_LENGTH: usize = 0x50;

    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        Ok(Self {
            vft: r.u64()?,
            unknown_8h: r.u64()?,
            unknown_10h: r.f32()?,
            unknown_14h: r.f32()?,
            unknown_18h: r.u64()?,
            unknown_20h: r.f32()?,
            unknown_24h: r.f32()?,
            unknown_28h: r.f32()?,
            unknown_2ch: r.f32()?,
            unknown_30h: r.u32()?,
            unknown_34h: r.f32()?,
            unknown_38h: r.f32()?,
            unknown_3ch: r.f32()?,
            unknown_40h: r.f32()?,
            unknown_44h: r.f32()?,
            unknown_48h: r.f32()?,
            unknown_4ch: r.f32()?,
        })
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u64(self.vft);
        b.u64(self.unknown_8h);
        b.f32(self.unknown_10h);
        b.f32(self.unknown_14h);
        b.u64(self.unknown_18h);
        b.f32(self.unknown_20h);
        b.f32(self.unknown_24h);
        b.f32(self.unknown_28h);
        b.f32(self.unknown_2ch);
        b.u32(self.unknown_30h);
        b.f32(self.unknown_34h);
        b.f32(self.unknown_38h);
        b.f32(self.unknown_3ch);
        b.f32(self.unknown_40h);
        b.f32(self.unknown_44h);
        b.f32(self.unknown_48h);
        b.f32(self.unknown_4ch);
        w.add(b, false)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.f32("Unknown10", self.unknown_10h);
        w.f32("Unknown14", self.unknown_14h);
        w.f32("Unknown20", self.unknown_20h);
        w.f32("Unknown24", self.unknown_24h);
        w.f32("Unknown28", self.unknown_28h);
        w.f32("Unknown2C", self.unknown_2ch);
        w.u32("Unknown30", self.unknown_30h);
        w.f32("Unknown34", self.unknown_34h);
        w.f32("Unknown38", self.unknown_38h);
        w.f32("Unknown3C", self.unknown_3ch);
        w.f32("Unknown40", self.unknown_40h);
        w.f32("Unknown44", self.unknown_44h);
        w.f32("Unknown48", self.unknown_48h);
        w.f32("Unknown4C", self.unknown_4ch);
    }

    pub fn read_xml(n: XmlNode) -> Self {
        Self {
            unknown_10h: n.f32("Unknown10", 0.0),
            unknown_14h: n.f32("Unknown14", 0.0),
            unknown_20h: n.f32("Unknown20", 0.0),
            unknown_24h: n.f32("Unknown24", 0.0),
            unknown_28h: n.f32("Unknown28", 0.0),
            unknown_2ch: n.f32("Unknown2C", 0.0),
            unknown_30h: n.u32("Unknown30", 0),
            unknown_34h: n.f32("Unknown34", 0.0),
            unknown_38h: n.f32("Unknown38", 0.0),
            unknown_3ch: n.f32("Unknown3C", 0.0),
            unknown_40h: n.f32("Unknown40", 0.0),
            unknown_44h: n.f32("Unknown44", 0.0),
            unknown_48h: n.f32("Unknown48", 0.0),
            unknown_4ch: n.f32("Unknown4C", 0.0),
            ..Default::default()
        }
    }
}
