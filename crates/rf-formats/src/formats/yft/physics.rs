//! The fragment's physics: `Rsc8FragPhysLODGroup`, `Rsc8FragPhysLOD` and what
//! they point to (archetype, groups, articulated body and joints, type
//! connections).

use super::ReadCx;
use super::drawable::FragDrawable;
use super::util::{read_raw_ptr_arr, write_raw_ptr_arr};
use crate::formats::ybn::{BOUND_FLAG_NAMES, Bounds, BoundsShape, flags_to_string, parse_flags};
use crate::formats::ydr::util::{node_items, node_items_opt, read_array};
use crate::formats::ydr::{Skeleton, f32_to_half, half_to_f32};
use crate::formats::yld::Arr;
use crate::formats::yld::util::{
    ascii_bytes, ascii_string, field_f32, field_u16, field_u64, node_array_with, read_structs, write_structs,
};
use crate::rsc8::{BlockBuf, BlockId, Rsc8Reader, Rsc8Writer};
use crate::xml::{XmlNode, XmlWriter};
use crate::{Result, bail};

/// `Rsc8FragTypeConnection` (24 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct FragTypeConnection {
    pub unknown00: u64,
    pub unknown08: u64,
    pub unknown10: u16,
    pub unknown12: u16,
    pub unknown14: f32,
}

impl FragTypeConnection {
    fn write_xml(&self, w: &mut XmlWriter) {
        field_u64(w, "Unknown00", self.unknown00);
        field_u64(w, "Unknown08", self.unknown08);
        field_u16(w, "Unknown10", self.unknown10);
        field_u16(w, "Unknown12", self.unknown12);
        field_f32(w, "Unknown14", self.unknown14);
    }

    fn read_xml(n: XmlNode) -> Self {
        Self {
            unknown00: n.u64("Unknown00", 0),
            unknown08: n.u64("Unknown08", 0),
            unknown10: n.u16("Unknown10", 0),
            unknown12: n.u16("Unknown12", 0),
            unknown14: n.f32("Unknown14", 0.0),
        }
    }
}

/// `Rsc8FragPhysArchetype` (0xD0 bytes): the physical properties of a LOD.
/// Its bounds are its physics LOD's (they are the same in the game's files,
/// and the C# makes them so from XML).
#[derive(Clone, Debug, PartialEq)]
pub struct FragPhysArchetype {
    pub vft: u64,
    pub unknown_08h: u64,
    pub archetype_type: u64,
    pub name: Option<String>,
    /// `Rsc8BoundFlags`.
    pub type_flags: u64,
    pub include_flags: u64,
    pub property_flags: u16,
    pub num_refs: u16,
    pub unknown_3ch: u32,
    /// In pounds.
    pub mass: [f32; 4],
    pub mass_inv: [f32; 4],
    pub gravity_factor: f32,
    pub max_speed: f32,
    pub max_ang_speed: f32,
    pub buoyancy_factor: f32,
    pub linear_c: [f32; 4],
    pub linear_v: [f32; 4],
    pub linear_v2: [f32; 4],
    pub angular_c: [f32; 4],
    pub angular_v: [f32; 4],
    pub angular_v2: [f32; 4],
}

impl Default for FragPhysArchetype {
    fn default() -> Self {
        Self {
            vft: 0,
            unknown_08h: 0,
            archetype_type: 2,
            name: None,
            type_flags: 0,
            include_flags: u64::MAX,
            property_flags: 0,
            num_refs: 1,
            unknown_3ch: 0,
            mass: [0.0; 4],
            mass_inv: [0.0; 4],
            gravity_factor: 0.0,
            max_speed: 0.0,
            max_ang_speed: 0.0,
            buoyancy_factor: 0.0,
            linear_c: [0.0; 4],
            linear_v: [0.0; 4],
            linear_v2: [0.0; 4],
            angular_c: [0.0; 4],
            angular_v: [0.0; 4],
            angular_v2: [0.0; 4],
        }
    }
}

impl FragPhysArchetype {
    pub const BLOCK_LENGTH: usize = 0xD0;

    /// Reads the archetype (its bounds pointer is skipped: see the struct).
    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let mut a = Self { vft: r.u64()?, unknown_08h: r.u64()?, archetype_type: r.u64()?, ..Default::default() };
        let name_ptr = r.u64()?;
        a.name = r.string_at(name_ptr)?;
        let _bounds = r.u64()?;
        a.type_flags = r.u64()?;
        a.include_flags = r.u64()?;
        a.property_flags = r.u16()?;
        a.num_refs = r.u16()?;
        a.unknown_3ch = r.u32()?;
        a.mass = r.vec4()?;
        a.mass_inv = r.vec4()?;
        a.gravity_factor = r.f32()?;
        a.max_speed = r.f32()?;
        a.max_ang_speed = r.f32()?;
        a.buoyancy_factor = r.f32()?;
        a.linear_c = r.vec4()?;
        a.linear_v = r.vec4()?;
        a.linear_v2 = r.vec4()?;
        a.angular_c = r.vec4()?;
        a.angular_v = r.vec4()?;
        a.angular_v2 = r.vec4()?;
        Ok(a)
    }

    /// Writes the archetype, pointing to `bounds` (its LOD's).
    pub fn write(&self, w: &mut Rsc8Writer, bounds: Option<BlockId>) -> BlockId {
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u64(self.vft);
        b.u64(self.unknown_08h);
        b.u64(self.archetype_type);
        b.ptr(w.opt_string(self.name.as_deref()));
        b.ptr(bounds);
        b.u64(self.type_flags);
        b.u64(self.include_flags);
        b.u16(self.property_flags);
        b.u16(self.num_refs);
        b.u32(self.unknown_3ch);
        b.vec4(self.mass);
        b.vec4(self.mass_inv);
        b.f32(self.gravity_factor);
        b.f32(self.max_speed);
        b.f32(self.max_ang_speed);
        b.f32(self.buoyancy_factor);
        for v in [self.linear_c, self.linear_v, self.linear_v2, self.angular_c, self.angular_v, self.angular_v2] {
            b.vec4(v);
        }
        w.add(b, false)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.string("Name", self.name.as_deref().unwrap_or(""));
        w.string("TypeFlags", &flags_to_string(self.type_flags, &BOUND_FLAG_NAMES));
        w.vec4("Mass", self.mass);
        w.vec4("MassInv", self.mass_inv);
        w.f32("GravityFactor", self.gravity_factor);
        w.f32("MaxSpeed", self.max_speed);
        w.f32("MaxAngSpeed", self.max_ang_speed);
        w.f32("BuoyancyFactor", self.buoyancy_factor);
        w.vec4("LinearC", self.linear_c);
        w.vec4("LinearV", self.linear_v);
        w.vec4("LinearV2", self.linear_v2);
        w.vec4("AngularC", self.angular_c);
        w.vec4("AngularV", self.angular_v);
        w.vec4("AngularV2", self.angular_v2);
    }

    pub fn read_xml(n: XmlNode) -> Self {
        Self {
            name: n.string("Name"),
            type_flags: n.string("TypeFlags").and_then(|s| parse_flags(&s, &BOUND_FLAG_NAMES, 64)).unwrap_or(0),
            mass: n.vec4("Mass"),
            mass_inv: n.vec4("MassInv"),
            gravity_factor: n.f32("GravityFactor", 0.0),
            max_speed: n.f32("MaxSpeed", 0.0),
            max_ang_speed: n.f32("MaxAngSpeed", 0.0),
            buoyancy_factor: n.f32("BuoyancyFactor", 0.0),
            linear_c: n.vec4("LinearC"),
            linear_v: n.vec4("LinearV"),
            linear_v2: n.vec4("LinearV2"),
            angular_c: n.vec4("AngularC"),
            angular_v: n.vec4("AngularV"),
            angular_v2: n.vec4("AngularV2"),
            ..Default::default()
        }
    }
}

/// The XML names of a [`FragPhysGroup`]'s half-float `values`, in order.
pub const GROUP_VALUE_NAMES: [&str; 30] = [
    "Unknown10",
    "Strength",
    "ForceTransmissionScaleUp",
    "ForceTransmissionScaleDown",
    "JointStiffness",
    "MinSoftAngle1",
    "MaxSoftAngle1",
    "MaxSoftAngle2",
    "MaxSoftAngle3",
    "RotationSpeed",
    "RotationStrength",
    "RestoringStrength",
    "RestoringMaxTorque",
    "Unknown2A",
    "Unknown2C",
    "Unknown2E",
    "Unknown30",
    "MinDamageForce",
    "DamageHealth",
    "WeaponHealth",
    "WeaponScale",
    "VehicleScale",
    "PedScale",
    "RagdollScale",
    "ExplosionScale",
    "ObjectScale",
    "PedInvMassScale",
    "MeleeScale",
    "Unknown48",
    "Unknown4A",
];

/// `Rsc8FragPhysGroup` (0x90 bytes): a breakable group of the fragment's
/// children, attached to a bone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FragPhysGroup {
    pub unknown_00h: u8,
    pub unknown_01h: u8,
    pub unknown_02h: u16,
    /// The first group with this one as parent (`0xFFFF` for none); derived
    /// from the groups' parents when read from XML.
    pub child_group_index: u16,
    pub parent_index: u16,
    pub child_index: u16,
    pub child_count: u16,
    /// How many groups have this one as parent; derived when read from XML.
    pub child_group_count: u16,
    /// The bone tag.
    pub bone_id: u16,
    /// Half floats (`Strength`, `DamageHealth`...), named by
    /// [`GROUP_VALUE_NAMES`]; see [`value`](Self::value).
    pub values: [u16; 30],
    pub unknown_4ch: u16,
    /// Increments with each group?
    pub unknown_4eh: u8,
    /// Up to 56 ASCII characters.
    pub name: String,
    pub unknown_87h: u8,
    pub unknown_88h: u64,
}

impl FragPhysGroup {
    pub const BLOCK_LENGTH: usize = 0x90;

    /// The value of this XML name (`"Strength"`...).
    pub fn value(&self, name: &str) -> Option<f32> {
        GROUP_VALUE_NAMES.iter().position(|n| *n == name).map(|i| half_to_f32(self.values[i]))
    }

    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let mut g = Self {
            unknown_00h: r.u8()?,
            unknown_01h: r.u8()?,
            unknown_02h: r.u16()?,
            child_group_index: r.u16()?,
            parent_index: r.u16()?,
            child_index: r.u16()?,
            child_count: r.u16()?,
            child_group_count: r.u16()?,
            bone_id: r.u16()?,
            ..Default::default()
        };
        for v in &mut g.values {
            *v = r.u16()?;
        }
        g.unknown_4ch = r.u16()?;
        g.unknown_4eh = r.u8()?;
        // ReadStringFixedLengthTrimmed: all 56 bytes, NULs trimmed at both ends.
        g.name = ascii_string(r.bytes(56)?).trim_matches('\0').to_owned();
        g.unknown_87h = r.u8()?;
        g.unknown_88h = r.u64()?;
        Ok(g)
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u8(self.unknown_00h);
        b.u8(self.unknown_01h);
        b.u16(self.unknown_02h);
        b.u16(self.child_group_index);
        b.u16(self.parent_index);
        b.u16(self.child_index);
        b.u16(self.child_count);
        b.u16(self.child_group_count);
        b.u16(self.bone_id);
        self.values.iter().for_each(|v| b.u16(*v));
        b.u16(self.unknown_4ch);
        b.u8(self.unknown_4eh);
        let mut name = ascii_bytes(&self.name);
        name.resize(56, 0);
        b.bytes(&name);
        b.u8(self.unknown_87h);
        b.u64(self.unknown_88h);
        w.add(b, false)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.string("Name", &self.name);
        w.u8("Unknown00", self.unknown_00h);
        w.u8("Unknown01", self.unknown_01h);
        w.u16("ParentIndex", self.parent_index);
        w.u16("ChildIndex", self.child_index);
        w.u16("ChildCount", self.child_count);
        w.u16("BoneId", self.bone_id);
        for (n, v) in GROUP_VALUE_NAMES.iter().zip(self.values) {
            w.f32(n, half_to_f32(v));
        }
        w.u16("Unknown4C", self.unknown_4ch);
        w.u8("Unknown4E", self.unknown_4eh);
    }

    pub fn read_xml(n: XmlNode) -> Result<Self> {
        let Some(name) = n.string("Name") else { bail!("a physics group without a Name (the C# fails on it)") };
        Ok(Self {
            name,
            unknown_00h: n.u8("Unknown00", 0),
            unknown_01h: n.u8("Unknown01", 0),
            parent_index: n.u16("ParentIndex", 0),
            child_index: n.u16("ChildIndex", 0),
            child_count: n.u16("ChildCount", 0),
            bone_id: n.u16("BoneId", 0),
            values: GROUP_VALUE_NAMES.map(|id| f32_to_half(n.f32(id, 0.0))),
            unknown_4ch: n.u16("Unknown4C", 0),
            unknown_4eh: n.u8("Unknown4E", 0),
            ..Default::default()
        })
    }
}

/// `Rsc8FragPhysJoint.JointType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JointType {
    /// One degree of freedom (0xB0 bytes).
    Dof1,
    /// Three degrees of freedom (0xF0 bytes).
    Dof3,
}

impl JointType {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Dof1),
            1 => Some(Self::Dof3),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Dof1 => "DOF1",
            Self::Dof3 => "DOF3",
        }
    }

    /// `Enum.TryParse<JointType>`: a name or a number.
    fn parse(s: &str) -> Option<Self> {
        let t = s.trim();
        match t {
            "DOF1" => Some(Self::Dof1),
            "DOF3" => Some(Self::Dof3),
            _ => t.parse::<i64>().ok().and_then(|v| u8::try_from(v).ok()).and_then(Self::from_u8),
        }
    }
}

const JOINT_VECTOR_NAMES: [&str; 9] = [
    "Unknown20",
    "Unknown30",
    "Unknown40",
    "Unknown50",
    "Unknown60",
    "Unknown70",
    "Unknown80",
    "Unknown90",
    "UnknownA0",
];

/// `Rsc8FragPhysJoint1Dof`/`Rsc8FragPhysJoint3Dof`: a joint of the
/// articulated body.
#[derive(Clone, Debug, PartialEq)]
pub struct FragPhysJoint {
    pub vft: u64,
    pub unknown_08h: u64,
    pub unknown_10h: f32,
    pub unknown_14h: u8,
    pub joint_type: JointType,
    pub unknown_16h: u16,
    pub frag_index1: u32,
    pub frag_index2: u32,
    /// `Unknown_20h` to `Unknown_A0h`.
    pub unknown_20h: [[f32; 4]; 9],
    /// 3 DOF joints only: `Unknown_B0h` to `Unknown_E0h` (not in the XML).
    pub unknown_b0h: [[f32; 4]; 4],
}

impl FragPhysJoint {
    /// A joint as the C# creates one of that type.
    pub fn new(joint_type: JointType) -> Self {
        Self {
            vft: 0,
            unknown_08h: 0,
            unknown_10h: 0.825,
            unknown_14h: 0,
            joint_type,
            unknown_16h: 0,
            frag_index1: 0,
            frag_index2: 0,
            unknown_20h: [[0.0; 4]; 9],
            unknown_b0h: [[0.0; 4], [1e8; 4], [-1e8; 4], [0.0; 4]],
        }
    }

    pub fn block_length(&self) -> usize {
        match self.joint_type {
            JointType::Dof1 => 0xB0,
            JointType::Dof3 => 0xF0,
        }
    }

    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let start = r.pos;
        let t = r.at(start + 21, |r| r.u8())?;
        let Some(t) = JointType::from_u8(t) else { bail!("a joint of unknown type {t} (the C# fails on it)") };
        let mut j = Self::new(t);
        j.vft = r.u64()?;
        j.unknown_08h = r.u64()?;
        j.unknown_10h = r.f32()?;
        j.unknown_14h = r.u8()?;
        let _type = r.u8()?;
        j.unknown_16h = r.u16()?;
        j.frag_index1 = r.u32()?;
        j.frag_index2 = r.u32()?;
        for v in &mut j.unknown_20h {
            *v = r.vec4()?;
        }
        if t == JointType::Dof3 {
            for v in &mut j.unknown_b0h {
                *v = r.vec4()?;
            }
        }
        Ok(j)
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let mut b = BlockBuf::with_capacity(self.block_length());
        b.u64(self.vft);
        b.u64(self.unknown_08h);
        b.f32(self.unknown_10h);
        b.u8(self.unknown_14h);
        b.u8(self.joint_type as u8);
        b.u16(self.unknown_16h);
        b.u32(self.frag_index1);
        b.u32(self.frag_index2);
        self.unknown_20h.iter().for_each(|v| b.vec4(*v));
        if self.joint_type == JointType::Dof3 {
            self.unknown_b0h.iter().for_each(|v| b.vec4(*v));
        }
        w.add(b, false)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.string("@type", self.joint_type.name());
        w.u32("FragIndex1", self.frag_index1);
        w.u32("FragIndex2", self.frag_index2);
        for (n, v) in JOINT_VECTOR_NAMES.iter().zip(self.unknown_20h) {
            w.vec4(n, v);
        }
    }

    pub fn read_xml(n: XmlNode) -> Result<Self> {
        let ty = n.string("@type");
        let Some(t) = ty.as_deref().and_then(JointType::parse) else {
            bail!("a joint of unknown type {ty:?} (the C# fails on it)")
        };
        let mut j = Self::new(t);
        j.frag_index1 = n.u32("FragIndex1", 0);
        j.frag_index2 = n.u32("FragIndex2", 0);
        j.unknown_20h = JOINT_VECTOR_NAMES.map(|id| n.vec4(id));
        Ok(j)
    }
}

/// `Rsc8FragPhysArticulatedBodyType` (0xB0 bytes): the joints between the
/// fragment's parts.
#[derive(Clone, Debug, PartialEq)]
pub struct FragPhysArticulatedBodyType {
    pub vft: u64,
    pub unknown_08h: u64,
    /// 22 values.
    pub item_indices: Option<Vec<u32>>,
    pub unknown_68h: u32,
    pub unknown_6ch: f32,
    pub unknown_70h: u64,
    /// `None` items are null pointers.
    pub joints: Vec<Option<FragPhysJoint>>,
    /// `unknown_count` of them (not in the XML).
    pub unknown_vectors: Option<Vec<[f32; 4]>>,
    /// The joints' count plus 1 (so when written).
    pub unknown_count: u8,
    pub joints_count: u8,
    /// 22 values: the joints' types?
    pub item_flags: Option<Vec<u8>>,
    pub unknown_a0h: u64,
    pub unknown_a8h: u64,
}

impl Default for FragPhysArticulatedBodyType {
    fn default() -> Self {
        Self {
            vft: 0,
            unknown_08h: 0,
            item_indices: None,
            unknown_68h: 0,
            unknown_6ch: 1.0,
            unknown_70h: 0,
            joints: Vec::new(),
            unknown_vectors: None,
            unknown_count: 0,
            joints_count: 0,
            item_flags: None,
            unknown_a0h: 0,
            unknown_a8h: 0,
        }
    }
}

impl FragPhysArticulatedBodyType {
    pub const BLOCK_LENGTH: usize = 0xB0;

    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let mut a = Self { vft: r.u64()?, unknown_08h: r.u64()?, ..Default::default() };
        a.item_indices = Some((0..22).map(|_| r.u32()).collect::<Result<_>>()?);
        a.unknown_68h = r.u32()?;
        a.unknown_6ch = r.f32()?;
        a.unknown_70h = r.u64()?;
        let joints_ptr = r.u64()?;
        let vectors_ptr = r.u64()?;
        a.unknown_count = r.u8()?;
        a.joints_count = r.u8()?;
        a.item_flags = Some(r.bytes(22)?.to_vec());
        a.unknown_a0h = r.u64()?;
        a.unknown_a8h = r.u64()?;
        let joints = read_raw_ptr_arr(r, joints_ptr, a.joints_count as usize, FragPhysJoint::read)?;
        a.joints = joints.unwrap_or_default();
        a.unknown_vectors = read_array(r, vectors_ptr, a.unknown_count as usize)?;
        Ok(a)
    }

    /// Writes it as the C# does: the counts come from the joints, and the
    /// two 22-value arrays are resized to 22.
    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let id = w.reserve();
        let joints_count = self.joints.len() as u8;
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u64(self.vft);
        b.u64(self.unknown_08h);
        let mut indices = self.item_indices.clone().unwrap_or_default();
        indices.resize(22, 0);
        indices.iter().for_each(|v| b.u32(*v));
        b.u32(self.unknown_68h);
        b.f32(self.unknown_6ch);
        b.u64(self.unknown_70h);
        let joints: Vec<Option<BlockId>> = self.joints.iter().map(|j| j.as_ref().map(|j| j.write(w))).collect();
        b.ptr(write_raw_ptr_arr(w, Some(&joints)));
        b.ptr(w.array(self.unknown_vectors.as_deref()));
        b.u8(joints_count.wrapping_add(1));
        b.u8(joints_count);
        let mut flags = self.item_flags.clone().unwrap_or_default();
        flags.resize(22, 0);
        b.bytes(&flags);
        b.u64(self.unknown_a0h);
        b.u64(self.unknown_a8h);
        w.set(id, b, false);
        id
    }

    pub fn write_xml(&self, w: &mut XmlWriter) -> Result<()> {
        w.u32_array("ItemIndices", self.item_indices.as_deref());
        w.byte_array("ItemFlags", self.item_flags.as_deref());
        node_array_with(w, "Joints", Some(&self.joints), |w, j| {
            j.write_xml(w);
            Ok(())
        })
    }

    pub fn read_xml(n: XmlNode) -> Result<Self> {
        Ok(Self {
            item_indices: n.u32_array("ItemIndices"),
            item_flags: n.byte_array("ItemFlags"),
            joints: node_items_opt(n, "Joints", FragPhysJoint::read_xml)?.unwrap_or_default(),
            ..Default::default()
        })
    }
}

/// `Rsc8FragPhysLOD` (0x160 bytes): a level of detail of the fragment's
/// physics: its children (a bound and drawables each), the groups they form,
/// their masses and the articulated body joining them.
#[derive(Clone, Debug, PartialEq)]
pub struct FragPhysLod {
    pub vft: u64,
    pub unknown_08h: u64,
    pub unknown_10h: u32,
    pub smallest_ang_inertia: f32,
    pub largest_ang_inertia: f32,
    pub min_move_force: f32,
    pub articulated_body_type: Option<FragPhysArticulatedBodyType>,
    /// One per child; `None` for a null pointer or no children.
    pub children_unk_floats: Option<Vec<f32>>,
    pub position_offset: [f32; 4],
    pub original_root_cg_offset: [f32; 4],
    pub unbroken_cg_offset: [f32; 4],
    pub damping_linear_c: [f32; 4],
    pub damping_linear_v: [f32; 4],
    pub damping_linear_v2: [f32; 4],
    pub damping_angular_c: [f32; 4],
    pub damping_angular_v: [f32; 4],
    pub damping_angular_v2: [f32; 4],
    pub unknown_c0h: u64,
    /// `None` items are null pointers.
    pub groups: Option<Vec<Option<FragPhysGroup>>>,
    pub archetype: Option<FragPhysArchetype>,
    pub frag_type_connections: Option<Arr<FragTypeConnection>>,
    /// Usually a composite with a child per child of the LOD.
    pub bounds: Option<Bounds>,
    pub children_inertia_tensors: Option<Vec<[f32; 4]>>,
    pub children_unk_vecs: Option<Vec<[f32; 4]>>,
    pub runtime_type_extension: u64,
    pub unknown_data1: Option<Vec<u16>>,
    pub unknown_data2: Option<Vec<u16>>,
    pub children_group_indices: Option<Vec<u16>>,
    /// The children's drawables; each one's bounds are the child of `bounds`
    /// with its index.
    pub drawables1: Option<Vec<Option<FragDrawable>>>,
    /// The children's damaged drawables, with their own bounds.
    pub drawables2: Option<Vec<Option<FragDrawable>>>,
    pub children_pristine_mass: Option<Vec<f32>>,
    pub children_damaged_mass: Option<Vec<f32>>,
    pub unknown_data1_count: u32,
    pub unknown_data2_count: u32,
    pub groups_count: u16,
    pub root_groups_count: u16,
    pub unknown_144h: u16,
    pub children_count: u16,
    pub unknown_148h: u64,
    pub unknown_150h: u64,
    pub unknown_158h: u64,
}

impl Default for FragPhysLod {
    fn default() -> Self {
        Self {
            vft: 0,
            unknown_08h: 0,
            unknown_10h: 0,
            smallest_ang_inertia: 0.0,
            largest_ang_inertia: 0.0,
            min_move_force: 0.0,
            articulated_body_type: None,
            children_unk_floats: None,
            position_offset: [0.0; 4],
            original_root_cg_offset: [0.0; 4],
            unbroken_cg_offset: [0.0; 4],
            damping_linear_c: [0.0; 4],
            damping_linear_v: [0.0; 4],
            damping_linear_v2: [0.0; 4],
            damping_angular_c: [0.0; 4],
            damping_angular_v: [0.0; 4],
            damping_angular_v2: [0.0; 4],
            unknown_c0h: 0,
            groups: None,
            archetype: None,
            frag_type_connections: None,
            bounds: None,
            children_inertia_tensors: None,
            children_unk_vecs: None,
            runtime_type_extension: 0,
            unknown_data1: None,
            unknown_data2: None,
            children_group_indices: None,
            drawables1: None,
            drawables2: None,
            children_pristine_mass: None,
            children_damaged_mass: None,
            unknown_data1_count: 0,
            unknown_data2_count: 0,
            groups_count: 0,
            root_groups_count: 0,
            unknown_144h: 1,
            children_count: 0,
            unknown_148h: 0,
            unknown_150h: 0,
            unknown_158h: 0,
        }
    }
}

impl FragPhysLod {
    pub const BLOCK_LENGTH: usize = 0x160;

    pub fn read(r: &mut Rsc8Reader, cx: &ReadCx) -> Result<Self> {
        let mut l = Self {
            vft: r.u64()?,
            unknown_08h: r.u64()?,
            unknown_10h: r.u32()?,
            smallest_ang_inertia: r.f32()?,
            largest_ang_inertia: r.f32()?,
            min_move_force: r.f32()?,
            ..Default::default()
        };
        l.articulated_body_type = r.ptr(FragPhysArticulatedBodyType::read)?;
        let unk_floats_ptr = r.u64()?;
        l.position_offset = r.vec4()?;
        l.original_root_cg_offset = r.vec4()?;
        l.unbroken_cg_offset = r.vec4()?;
        l.damping_linear_c = r.vec4()?;
        l.damping_linear_v = r.vec4()?;
        l.damping_linear_v2 = r.vec4()?;
        l.damping_angular_c = r.vec4()?;
        l.damping_angular_v = r.vec4()?;
        l.damping_angular_v2 = r.vec4()?;
        l.unknown_c0h = r.u64()?;
        let groups_ptr = r.u64()?;
        l.archetype = r.ptr(FragPhysArchetype::read)?;
        l.frag_type_connections = r.ptr(Arr::read)?;
        let bounds_ptr = r.u64()?;
        l.bounds = Bounds::read_at(r, bounds_ptr)?;
        let tensors_ptr = r.u64()?;
        let vecs_ptr = r.u64()?;
        l.runtime_type_extension = r.u64()?;
        let data1_ptr = r.u64()?;
        let data2_ptr = r.u64()?;
        let group_indices_ptr = r.u64()?;
        let drawables1_ptr = r.u64()?;
        let drawables2_ptr = r.u64()?;
        let pristine_ptr = r.u64()?;
        let damaged_ptr = r.u64()?;
        l.unknown_data1_count = r.u32()?;
        l.unknown_data2_count = r.u32()?;
        l.groups_count = r.u16()?;
        l.root_groups_count = r.u16()?;
        l.unknown_144h = r.u16()?;
        l.children_count = r.u16()?;
        l.unknown_148h = r.u64()?;
        l.unknown_150h = r.u64()?;
        l.unknown_158h = r.u64()?;

        let n = l.children_count as usize;
        l.children_unk_floats = read_array(r, unk_floats_ptr, n)?;
        l.groups = read_raw_ptr_arr(r, groups_ptr, l.groups_count as usize, FragPhysGroup::read)?;
        l.children_inertia_tensors = read_array(r, tensors_ptr, n)?;
        l.children_unk_vecs = read_array(r, vecs_ptr, n)?;
        l.unknown_data1 = read_array(r, data1_ptr, l.unknown_data1_count as usize)?;
        l.unknown_data2 = read_array(r, data2_ptr, l.unknown_data2_count as usize)?;
        l.children_group_indices = read_array(r, group_indices_ptr, n)?;
        l.drawables1 = read_raw_ptr_arr(r, drawables1_ptr, n, |r| FragDrawable::read(r, cx))?;
        l.drawables2 = read_raw_ptr_arr(r, drawables2_ptr, n, |r| FragDrawable::read(r, cx))?;
        l.children_pristine_mass = read_array(r, pristine_ptr, n)?;
        l.children_damaged_mass = read_array(r, damaged_ptr, n)?;
        Ok(l)
    }

    /// Writes the LOD. Its composite's children are written once, and the
    /// drawables of `drawables1` whose bounds are in it point to them, as
    /// does the archetype to the bounds; drawables whose skeleton is
    /// `skeleton.0` point to the block `skeleton.1`.
    pub fn write(&self, w: &mut Rsc8Writer, skeleton: (Option<&Skeleton>, Option<BlockId>)) -> Result<BlockId> {
        let id = w.reserve();
        let (bounds, children) = match &self.bounds {
            Some(b) => {
                let (id, kids) = b.write_with_children(w)?;
                (Some(id), kids)
            }
            None => (None, Vec::new()),
        };
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u64(self.vft);
        b.u64(self.unknown_08h);
        b.u32(self.unknown_10h);
        b.f32(self.smallest_ang_inertia);
        b.f32(self.largest_ang_inertia);
        b.f32(self.min_move_force);
        b.ptr(self.articulated_body_type.as_ref().map(|a| a.write(w)));
        b.ptr(w.array(self.children_unk_floats.as_deref()));
        for v in [
            self.position_offset,
            self.original_root_cg_offset,
            self.unbroken_cg_offset,
            self.damping_linear_c,
            self.damping_linear_v,
            self.damping_linear_v2,
            self.damping_angular_c,
            self.damping_angular_v,
            self.damping_angular_v2,
        ] {
            b.vec4(v);
        }
        b.u64(self.unknown_c0h);
        let groups: Option<Vec<Option<BlockId>>> =
            self.groups.as_ref().map(|g| g.iter().map(|g| g.as_ref().map(|g| g.write(w))).collect());
        b.ptr(write_raw_ptr_arr(w, groups.as_deref()));
        b.ptr(self.archetype.as_ref().map(|a| a.write(w, bounds)));
        b.ptr(self.frag_type_connections.as_ref().map(|c| {
            let mut cb = BlockBuf::with_capacity(16);
            c.write(&mut cb, w);
            w.add(cb, false)
        }));
        b.ptr(bounds);
        b.ptr(w.array(self.children_inertia_tensors.as_deref()));
        b.ptr(w.array(self.children_unk_vecs.as_deref()));
        b.u64(self.runtime_type_extension);
        b.ptr(w.array(self.unknown_data1.as_deref()));
        b.ptr(w.array(self.unknown_data2.as_deref()));
        b.ptr(w.array(self.children_group_indices.as_deref()));
        let write_drawables = |w: &mut Rsc8Writer, list: &Option<Vec<Option<FragDrawable>>>, share_bounds: bool| {
            let Some(list) = list else { return Ok(None) };
            let mut ids = Vec::with_capacity(list.len());
            for (i, d) in list.iter().enumerate() {
                let Some(d) = d else {
                    ids.push(None);
                    continue;
                };
                let in_composite = share_bounds && d.bounds_in_composite && i < children.len();
                let bounds = in_composite.then(|| children[i]);
                let skel = skeleton.1.filter(|_| d.base.skeleton.is_some() && d.base.skeleton.as_ref() == skeleton.0);
                ids.push(Some(d.write(w, skel, bounds)?));
            }
            Ok::<_, crate::Error>(write_raw_ptr_arr(w, Some(&ids)))
        };
        let d1 = write_drawables(w, &self.drawables1, true)?;
        let d2 = write_drawables(w, &self.drawables2, false)?;
        b.ptr(d1);
        b.ptr(d2);
        b.ptr(w.array(self.children_pristine_mass.as_deref()));
        b.ptr(w.array(self.children_damaged_mass.as_deref()));
        b.u32(self.unknown_data1_count);
        b.u32(self.unknown_data2_count);
        b.u16(self.groups_count);
        b.u16(self.root_groups_count);
        b.u16(self.unknown_144h);
        b.u16(self.children_count);
        b.u64(self.unknown_148h);
        b.u64(self.unknown_150h);
        b.u64(self.unknown_158h);
        w.set(id, b, false);
        Ok(id)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) -> Result<()> {
        w.f32("MinMoveForce", self.min_move_force);
        w.vec4("OriginalRootCgOffset", self.original_root_cg_offset);
        if let Some(a) = &self.archetype {
            w.node("Archetype", |w| a.write_xml(w));
        }
        node_array_with(w, "Groups", self.groups.as_deref(), |w, g| {
            g.write_xml(w);
            Ok(())
        })?;
        w.u16_array("UnknownData1", self.unknown_data1.as_deref());
        w.u16_array("UnknownData2", self.unknown_data2.as_deref());
        w.f32_array("ChildrenUnkFloats", self.children_unk_floats.as_deref());
        w.f32_array("ChildrenPristineMass", self.children_pristine_mass.as_deref());
        w.f32_array("ChildrenDamagedMass", self.children_damaged_mass.as_deref());
        w.u16_array("ChildrenGroupIndices", self.children_group_indices.as_deref());
        w.vec4_array("ChildrenUnkVecs", self.children_unk_vecs.as_deref());
        w.vec4_array("ChildrenInertiaTensors", self.children_inertia_tensors.as_deref());
        node_array_with(w, "Drawables1", self.drawables1.as_deref(), |w, d| d.write_xml(w, false))?;
        node_array_with(w, "Drawables2", self.drawables2.as_deref(), |w, d| d.write_xml(w, false))?;
        if let Some(b) = &self.bounds {
            w.node("Bounds", |w| b.write_xml(w));
        }
        if let Some(a) = &self.articulated_body_type {
            let mut res = Ok(());
            w.node("ArticulatedBodyType", |w| res = a.write_xml(w));
            res?;
        }
        write_structs(
            w,
            "FragTypeConnections",
            self.frag_type_connections.as_ref().and_then(|c| c.items.as_deref()),
            |w, c| c.write_xml(w),
        );
        Ok(())
    }

    /// Reads the XML and derives what the C# derives: the counts, the groups'
    /// child group index and count, and the drawables of `drawables1` get
    /// the composite's children as bounds.
    pub fn read_xml(n: XmlNode) -> Result<Self> {
        let mut l = Self {
            min_move_force: n.f32("MinMoveForce", 0.0),
            original_root_cg_offset: n.vec4("OriginalRootCgOffset"),
            archetype: n.node("Archetype").map(FragPhysArchetype::read_xml),
            groups: node_items(n, "Groups", FragPhysGroup::read_xml)?.map(|g| g.into_iter().map(Some).collect()),
            unknown_data1: n.u16_array("UnknownData1"),
            unknown_data2: n.u16_array("UnknownData2"),
            children_unk_floats: n.f32_array("ChildrenUnkFloats"),
            children_pristine_mass: n.f32_array("ChildrenPristineMass"),
            children_damaged_mass: n.f32_array("ChildrenDamagedMass"),
            children_group_indices: n.u16_array("ChildrenGroupIndices"),
            children_unk_vecs: n.vec4_array("ChildrenUnkVecs"),
            children_inertia_tensors: n.vec4_array("ChildrenInertiaTensors"),
            drawables1: node_items_opt(n, "Drawables1", FragDrawable::read_xml)?,
            drawables2: node_items_opt(n, "Drawables2", FragDrawable::read_xml)?,
            bounds: n.node("Bounds").map(|b| Bounds::read_xml(&b)).transpose()?,
            articulated_body_type: n
                .node("ArticulatedBodyType")
                .map(FragPhysArticulatedBodyType::read_xml)
                .transpose()?,
            frag_type_connections: read_structs(n, "FragTypeConnections", FragTypeConnection::read_xml)
                .map(|c| Arr::new(Some(c))),
            ..Default::default()
        };
        l.unknown_data1_count = l.unknown_data1.as_ref().map_or(0, |d| d.len() as u32);
        l.unknown_data2_count = l.unknown_data2.as_ref().map_or(0, |d| d.len() as u32);
        l.groups_count = l.groups.as_ref().map_or(0, |g| g.len() as u16);
        l.children_count = l.children_unk_floats.as_ref().map_or(0, |f| f.len() as u16);
        if let (Some(BoundsShape::Composite(c)), Some(drawables)) =
            (l.bounds.as_ref().map(|b| &b.shape), l.drawables1.as_mut())
        {
            let kids = c.children.as_deref().unwrap_or(&[]);
            for (d, b) in drawables.iter_mut().zip(kids) {
                if let Some(d) = d {
                    d.bounds = b.clone();
                    d.bounds_in_composite = true;
                }
            }
        }
        if let Some(groups) = &mut l.groups {
            let parents: Vec<u16> = groups.iter().map(|g| g.as_ref().map_or(0, |g| g.parent_index)).collect();
            l.root_groups_count = parents.iter().filter(|&&p| p == 0xFFFF).count() as u16;
            for (i, g) in groups.iter_mut().enumerate() {
                let Some(g) = g else { continue };
                let kids: Vec<usize> = (0..parents.len()).filter(|&j| parents[j] as usize == i).collect();
                g.child_group_count = kids.len() as u16;
                g.child_group_index = kids.first().map_or(0xFFFF, |&j| j as u16);
            }
        }
        Ok(l)
    }

    /// The physics drawables that aren't null, of both lists.
    pub fn drawables_mut(&mut self) -> impl Iterator<Item = &mut FragDrawable> {
        self.drawables1.iter_mut().chain(self.drawables2.iter_mut()).flatten().flatten()
    }
}

/// `Rsc8FragPhysLODGroup` (0x30 bytes): up to 3 physics LODs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FragPhysLodGroup {
    pub vft: u64,
    pub unknown_08h: u64,
    pub physics_lod1: Option<FragPhysLod>,
    pub physics_lod2: Option<FragPhysLod>,
    pub physics_lod3: Option<FragPhysLod>,
    pub unknown_28h: u64,
}

const LOD_NAMES: [&str; 3] = ["PhysicsLOD1", "PhysicsLOD2", "PhysicsLOD3"];

impl FragPhysLodGroup {
    pub const BLOCK_LENGTH: usize = 0x30;

    pub fn read(r: &mut Rsc8Reader, cx: &ReadCx) -> Result<Self> {
        Ok(Self {
            vft: r.u64()?,
            unknown_08h: r.u64()?,
            physics_lod1: r.ptr(|r| FragPhysLod::read(r, cx))?,
            physics_lod2: r.ptr(|r| FragPhysLod::read(r, cx))?,
            physics_lod3: r.ptr(|r| FragPhysLod::read(r, cx))?,
            unknown_28h: r.u64()?,
        })
    }

    pub fn lods(&self) -> [&Option<FragPhysLod>; 3] {
        [&self.physics_lod1, &self.physics_lod2, &self.physics_lod3]
    }

    pub fn lods_mut(&mut self) -> [&mut Option<FragPhysLod>; 3] {
        [&mut self.physics_lod1, &mut self.physics_lod2, &mut self.physics_lod3]
    }

    pub fn write(&self, w: &mut Rsc8Writer, skeleton: (Option<&Skeleton>, Option<BlockId>)) -> Result<BlockId> {
        let id = w.reserve();
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u64(self.vft);
        b.u64(self.unknown_08h);
        for lod in self.lods() {
            b.ptr(lod.as_ref().map(|l| l.write(w, skeleton)).transpose()?);
        }
        b.u64(self.unknown_28h);
        w.set(id, b, false);
        Ok(id)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) -> Result<()> {
        for (n, lod) in LOD_NAMES.iter().zip(self.lods()) {
            if let Some(l) = lod {
                let mut res = Ok(());
                w.node(n, |w| res = l.write_xml(w));
                res?;
            }
        }
        Ok(())
    }

    pub fn read_xml(n: XmlNode) -> Result<Self> {
        let lod = |id: &str| n.node(id).map(FragPhysLod::read_xml).transpose();
        Ok(Self {
            physics_lod1: lod(LOD_NAMES[0])?,
            physics_lod2: lod(LOD_NAMES[1])?,
            physics_lod3: lod(LOD_NAMES[2])?,
            ..Default::default()
        })
    }
}
