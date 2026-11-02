//! Skeletons (`Rsc8Skeleton`, with `Rsc8SkeletonBone`) and joint limits
//! (`Rsc8JointLimits`).

use super::util::{
    ArrHeader, StructField, flags_name, node_items, parse_flags, read_array, read_struct_array, read_vec4_or,
    write_ptr_arr, write_struct_array,
};
use crate::hash::joaat;
use crate::rsc8::{BlockBuf, BlockId, Rsc8Reader, Rsc8Writer};
use crate::xml::{XmlNode, XmlWriter};
use crate::{Result, bail};

/// `Rsc8SkeletonBoneFlags`.
pub const BONE_FLAG_NAMES: &[(&str, u32)] = &[
    ("None", 0),
    ("RotX", 0x1),
    ("RotY", 0x2),
    ("RotZ", 0x4),
    ("LimitRotation", 0x8),
    ("TransX", 0x10),
    ("TransY", 0x20),
    ("TransZ", 0x40),
    ("LimitTranslation", 0x80),
    ("ScaleX", 0x100),
    ("ScaleY", 0x200),
    ("ScaleZ", 0x400),
    ("LimitScale", 0x800),
    ("Unk0", 0x1000),
    ("Unk1", 0x2000),
    ("Unk2", 0x4000),
    ("Unk3", 0x8000),
    ("Unk4", 0x10000),
    ("Unk5", 0x20000),
];

/// The `TransformUnk` a bone has when the XML doesn't say.
const DEFAULT_TRANSFORM_UNK: [f32; 4] = [0.0, 4.0, -3.0, 0.0];

/// `Rsc8SkeletonBone` (80 bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct Bone {
    /// A quaternion: x, y, z, w.
    pub rotation: [f32; 4],
    pub position: [f32; 3],
    pub unknown_1ch: u32,
    pub scale: [f32; 3],
    pub unknown_2ch: f32,
    pub name: Option<String>,
    pub properties_ptr: u64,
    /// See [`BONE_FLAG_NAMES`].
    pub flags: u32,
    pub next_sibling_index: i16,
    pub parent_index: i16,
    pub index0: i16,
    pub tag: u16,
    pub index1: i16,
    pub last_sibling_index: i16,
    /// Column 4 of the bone's bind pose matrix (not stored in the bone).
    pub transform_unk: [f32; 4],
}

impl Default for Bone {
    fn default() -> Self {
        Self {
            rotation: [0.0, 0.0, 0.0, 1.0],
            position: [0.0; 3],
            unknown_1ch: 0,
            scale: [1.0; 3],
            unknown_2ch: 1.0,
            name: None,
            properties_ptr: 0,
            flags: 0,
            next_sibling_index: 0,
            parent_index: 0,
            index0: 0,
            tag: 0,
            index1: 0,
            last_sibling_index: 0,
            transform_unk: [0.0; 4],
        }
    }
}

impl Bone {
    pub const BLOCK_LENGTH: usize = 80;

    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let rotation = r.vec4()?;
        let position = r.vec3()?;
        let unknown_1ch = r.u32()?;
        let scale = r.vec3()?;
        let unknown_2ch = r.f32()?;
        let name_ptr = r.u64()?;
        Ok(Self {
            rotation,
            position,
            unknown_1ch,
            scale,
            unknown_2ch,
            name: r.string_at(name_ptr)?,
            properties_ptr: r.u64()?,
            flags: r.u32()?,
            next_sibling_index: r.i16()?,
            parent_index: r.i16()?,
            index0: r.i16()?,
            tag: r.u16()?,
            index1: r.i16()?,
            last_sibling_index: r.i16()?,
            transform_unk: [0.0; 4],
        })
    }

    fn write(&self, b: &mut BlockBuf, w: &mut Rsc8Writer) {
        b.vec4(self.rotation);
        b.vec3(self.position);
        b.u32(self.unknown_1ch);
        b.vec3(self.scale);
        b.f32(self.unknown_2ch);
        b.ptr(w.opt_string(self.name.as_deref()));
        b.u64(self.properties_ptr);
        b.u32(self.flags);
        b.i16(self.next_sibling_index);
        b.i16(self.parent_index);
        b.i16(self.index0);
        b.u16(self.tag);
        b.i16(self.index1);
        b.i16(self.last_sibling_index);
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.string("Name", self.name.as_deref().unwrap_or(""));
        w.u16("Tag", self.tag);
        w.string("Flags", &flags_name(BONE_FLAG_NAMES, self.flags));
        w.i16("Index", self.index1);
        w.i16("ParentIndex", self.parent_index);
        w.i16("NextSiblingIndex", self.next_sibling_index);
        w.i16("LastSiblingIndex", self.last_sibling_index);
        w.vec4("Rotation", self.rotation);
        w.vec3("Position", self.position);
        w.vec3("Scale", self.scale);
        if self.transform_unk != DEFAULT_TRANSFORM_UNK {
            w.vec4("TransformUnk", self.transform_unk);
        }
    }

    pub fn read_xml(node: XmlNode) -> Self {
        let index1 = node.i16("Index", 0);
        Self {
            name: node.string("Name"),
            tag: node.u16("Tag", 0),
            flags: parse_flags(BONE_FLAG_NAMES, node.string("Flags").as_deref()).unwrap_or(0),
            index1,
            index0: index1,
            parent_index: node.i16("ParentIndex", 0),
            next_sibling_index: node.i16("NextSiblingIndex", 0),
            last_sibling_index: node.i16("LastSiblingIndex", 0),
            rotation: node.vec4("Rotation"),
            position: node.vec3("Position"),
            scale: node.vec3("Scale"),
            transform_unk: read_vec4_or(node, "TransformUnk", DEFAULT_TRANSFORM_UNK),
            ..Default::default()
        }
    }
}

/// `Rsc8SkeletonDebugBlock`: names some skeletons carry after the parent
/// indices.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkeletonDebugBlock {
    pub unknown_0h: u16,
    pub unknown_2h: u16,
    pub unknown_4h: u16,
    pub string: String,
}

/// `Rsc8SkeletonMappingBlock`: each bone's parent index, and optional debug
/// data.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkeletonMappingBlock {
    pub bones_count: u32,
    pub indices: Option<Vec<i16>>,
    pub debug_block_count: u16,
    pub debug_block_length: u16,
    pub debug_blocks: Vec<SkeletonDebugBlock>,
    pub debug_indices: Option<Vec<i16>>,
}

impl SkeletonMappingBlock {
    fn read(r: &mut Rsc8Reader, bones_count: u32) -> Result<Self> {
        let mut m = Self { bones_count, ..Default::default() };
        if bones_count == 0 {
            return Ok(m);
        }
        m.indices = read_array(r, r.pos, bones_count as usize)?;
        r.pos += bones_count as u64 * 2;
        m.debug_block_count = r.u16()?;
        if m.debug_block_count != 0 {
            m.debug_block_length = r.u16()?;
            let finish = (r.pos + m.debug_block_length as u64).wrapping_sub(4);
            while r.pos < finish {
                let mut d = SkeletonDebugBlock {
                    unknown_0h: r.u16()?,
                    unknown_2h: r.u16()?,
                    unknown_4h: r.u16()?,
                    ..Default::default()
                };
                d.string = r.string_here()?;
                m.debug_blocks.push(d);
            }
            m.debug_indices = read_array(r, r.pos, m.debug_block_count as usize)?;
        }
        Ok(m)
    }

    fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let mut b = BlockBuf::default();
        if self.bones_count != 0 {
            if let Some(i) = &self.indices {
                b.bytes(bytemuck::cast_slice(i));
            }
            b.pad_to(self.bones_count as usize * 2);
            b.u16(self.debug_block_count);
            if self.debug_block_count != 0 {
                b.u16(self.debug_block_length);
                for d in &self.debug_blocks {
                    b.u16(d.unknown_0h);
                    b.u16(d.unknown_2h);
                    b.u16(d.unknown_4h);
                    b.bytes(d.string.as_bytes());
                    b.u8(0);
                }
                if let Some(i) = &self.debug_indices {
                    b.bytes(bytemuck::cast_slice(i));
                }
            }
        }
        let len = self.bones_count as usize * 2
            + if self.debug_block_count == 0 {
                2
            } else {
                self.debug_block_length as usize + self.debug_block_count as usize * 2
            };
        b.pad_to(len);
        b.data.truncate(len);
        w.add(b, false)
    }
}

/// A 4x4 matrix as System.Numerics stores it, row by row (M11, M12, ...).
pub type Matrix4 = [f32; 16];

/// `Rsc8Skeleton` (104 bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct Skeleton {
    pub vft: u64,
    pub unknown_8h: u64,
    /// The hash map from bone tag to bone index: each bucket's chain of
    /// (tag, index); `None` for a null pointer or no buckets.
    pub bone_tags: Option<Vec<Vec<(u32, u32)>>>,
    pub bone_tags_capacity: u16,
    pub bone_tags_unknown: u32,
    pub bone_tags_count: u32,
    pub unknown_24h: u32,
    /// `None` for a null pointer.
    pub bones: Option<Vec<Bone>>,
    pub bind_pose_inv: Option<Vec<Matrix4>>,
    pub bind_pose: Option<Vec<Matrix4>>,
    pub mapping_block: Option<SkeletonMappingBlock>,
    pub unknown_48h: u64,
    pub unknown_50h: u32,
    pub unknown_54h: u32,
    pub unknown_58h: u32,
    pub unknown_5ch: u16,
    pub bones_count: u16,
    pub unknown_60h: u16,
    /// Attaches the whole skeleton to this bone of another skeleton.
    pub parent_bone_tag: u16,
    pub unknown_64h: u32,
}

impl Default for Skeleton {
    fn default() -> Self {
        Self {
            vft: 0,
            unknown_8h: 0,
            bone_tags: None,
            bone_tags_capacity: 0,
            bone_tags_unknown: 0,
            bone_tags_count: 0,
            unknown_24h: 0,
            bones: None,
            bind_pose_inv: None,
            bind_pose: None,
            mapping_block: None,
            unknown_48h: 0,
            unknown_50h: 0,
            unknown_54h: 0,
            unknown_58h: 0,
            unknown_5ch: 1,
            bones_count: 0,
            unknown_60h: 0,
            parent_bone_tag: 0,
            unknown_64h: 0,
        }
    }
}

impl Skeleton {
    pub const BLOCK_LENGTH: usize = 104;

    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let mut s = Self { vft: r.u64()?, unknown_8h: r.u64()?, ..Default::default() };
        let tags = ArrHeader::read(r)?;
        s.bone_tags_capacity = tags.capacity;
        s.bone_tags_unknown = tags.unknown;
        if let Some(ptrs) = read_array::<u64>(r, tags.ptr, tags.count as usize)? {
            let mut buckets = Vec::with_capacity(ptrs.len());
            for mut p in ptrs {
                let mut chain = Vec::new();
                while p != 0 {
                    if chain.len() > 0xFFFF {
                        bail!("a looping bone tag chain");
                    }
                    let (tag, index, next) = r.at(p, |r| Ok((r.u32()?, r.u32()?, r.u64()?)))?;
                    chain.push((tag, index));
                    p = next;
                }
                buckets.push(chain);
            }
            s.bone_tags = Some(buckets);
        }
        s.bone_tags_count = r.u32()?;
        s.unknown_24h = r.u32()?;
        let bones_ptr = r.u64()?;
        let inv_ptr = r.u64()?;
        let pose_ptr = r.u64()?;
        let mapping_ptr = r.u64()?;
        s.unknown_48h = r.u64()?;
        s.unknown_50h = r.u32()?;
        s.unknown_54h = r.u32()?;
        s.unknown_58h = r.u32()?;
        s.unknown_5ch = r.u16()?;
        s.bones_count = r.u16()?;
        s.unknown_60h = r.u16()?;
        s.parent_bone_tag = r.u16()?;
        s.unknown_64h = r.u32()?;

        let n = s.bones_count as usize;
        s.bones = r.items_at(bones_ptr, n, Bone::read)?;
        s.bind_pose_inv = read_array(r, inv_ptr, n)?;
        s.bind_pose = read_array(r, pose_ptr, n)?;
        let count = s.bones_count as u32;
        s.mapping_block = r.follow(mapping_ptr, |r| SkeletonMappingBlock::read(r, count))?;
        // BuildBonesMap: the 4th column of the bind pose.
        if let Some(bones) = &mut s.bones {
            for (i, b) in bones.iter_mut().enumerate() {
                b.transform_unk = match s.bind_pose.as_ref().and_then(|p| p.get(i)) {
                    Some(m) => [m[3], m[7], m[11], m[15]],
                    None => [0.0; 4],
                };
            }
        }
        Ok(s)
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let id = w.reserve();
        // Rsc8SkeletonBoneData: a 16-byte header with the count, then the bones.
        let bones = self.bones.as_deref().unwrap_or(&[]);
        let mut bd = BlockBuf::with_capacity(16 + bones.len() * Bone::BLOCK_LENGTH);
        bd.u64(bones.len() as u64);
        bd.u64(0);
        for bone in bones {
            bone.write(&mut bd, w);
        }
        let bd = w.add(bd, false);

        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u64(self.vft);
        b.u64(self.unknown_8h);
        let tags = self.bone_tags.as_ref().map(|buckets| {
            buckets
                .iter()
                .map(|chain| {
                    let mut next = None;
                    for &(tag, index) in chain.iter().rev() {
                        let mut t = BlockBuf::with_capacity(16);
                        t.u32(tag);
                        t.u32(index);
                        t.ptr(next);
                        next = Some(w.add(t, false));
                    }
                    next
                })
                .collect::<Vec<_>>()
        });
        write_ptr_arr(&mut b, w, tags.as_deref(), self.bone_tags_capacity, self.bone_tags_unknown);
        b.u32(self.bone_tags_count);
        b.u32(self.unknown_24h);
        b.ptr_offset(Some(bd), 0x10);
        b.ptr(w.array(self.bind_pose_inv.as_deref()));
        b.ptr(w.array(self.bind_pose.as_deref()));
        b.ptr(self.mapping_block.as_ref().map(|m| m.write(w)));
        b.u64(self.unknown_48h);
        b.u32(self.unknown_50h);
        b.u32(self.unknown_54h);
        b.u32(self.unknown_58h);
        b.u16(self.unknown_5ch);
        b.u16(self.bones_count);
        b.u16(self.unknown_60h);
        b.u16(self.parent_bone_tag);
        b.u32(self.unknown_64h);
        w.set(id, b, false);
        id
    }

    /// The bone with this tag and its index (`BonesMap`: the last one, when
    /// several have it).
    pub fn bone_by_tag(&self, tag: u16) -> Option<(usize, &Bone)> {
        self.bones.as_ref()?.iter().enumerate().rev().find(|(_, b)| b.tag == tag)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        w.u32("Unknown_24", self.unknown_24h);
        w.hash("Unknown_50", self.unknown_50h);
        w.hash("Unknown_54", self.unknown_54h);
        w.hash("Unknown_58", self.unknown_58h);
        w.u16("Unknown_60", self.unknown_60h);
        w.u16("ParentBoneTag", self.parent_bone_tag);
        w.node_array("Bones", self.bones.as_deref(), |w, b| b.write_xml(w));
    }

    /// Reads a skeleton's XML and builds what the C# builds from the bones:
    /// the parent indices, the bone tag map and the bind poses.
    pub fn read_xml(node: XmlNode) -> Result<Self> {
        let mut s = Self {
            unknown_24h: node.u32("Unknown_24", 0),
            unknown_50h: node.string("Unknown_50").map_or(0, |n| joaat(&n)),
            unknown_54h: node.string("Unknown_54").map_or(0, |n| joaat(&n)),
            unknown_58h: node.string("Unknown_58").map_or(0, |n| joaat(&n)),
            unknown_60h: node.u16("Unknown_60", 0),
            parent_bone_tag: node.u16("ParentBoneTag", 0),
            ..Default::default()
        };
        s.bones = node_items(node, "Bones", |n| Ok(Bone::read_xml(n)))?;
        let bones = s.bones.as_deref().unwrap_or(&[]);
        s.bones_count = bones.len() as u16;
        // BuildIndices
        let parents: Vec<i16> = bones.iter().map(|b| b.parent_index).collect();
        s.mapping_block = Some(SkeletonMappingBlock {
            bones_count: parents.len() as u32,
            indices: (!parents.is_empty()).then_some(parents),
            ..Default::default()
        });
        // BuildBoneTags
        let tags: Vec<(u32, u32)> = bones.iter().enumerate().map(|(i, b)| (b.tag as u32, i as u32)).collect();
        match build_bone_tags(&tags) {
            Some(buckets) => {
                s.bone_tags = Some(buckets);
                s.bone_tags_count = tags.len() as u32;
            }
            None => {
                s.bone_tags = None;
                s.bone_tags_count = 0;
            }
        }
        s.bone_tags_capacity = 0;
        // BuildBindMatrices
        let (pose, inv) = bind_matrices(bones);
        s.bind_pose = (!pose.is_empty()).then_some(pose);
        s.bind_pose_inv = (!inv.is_empty()).then_some(inv);
        Ok(s)
    }
}

/// `Rsc8DataMap.Build(tags, sortbuckets: false, reversebuckets: true)`: tags
/// in buckets by `tag % bucket count`, each bucket's chain in reverse order.
fn build_bone_tags(tags: &[(u32, u32)]) -> Option<Vec<Vec<(u32, u32)>>> {
    if tags.is_empty() || (tags.len() == 1 && tags[0].0 == 0) {
        return None;
    }
    const SIZES: &[usize] = &[
        11, 29, 53, 103, 211, 409, 647, 821, 1031, 1291, 1627, 2053, 2591, 3251, 4099, 5167, 6521, 8209, 10331, 13007,
        16411, 20663, 26017, 32771, 41299, 52021, 65537, 82571, 104033, 131101, 165161, 208067, 262147, 330287, 416147,
        524309, 660563, 832291, 1048583, 1321139, 1664543, 2097169,
    ];
    let n = SIZES.iter().copied().find(|&s| tags.len() < s).unwrap_or(2097211);
    let mut buckets = vec![Vec::new(); n];
    for &t in tags {
        buckets[(t.0 as usize) % n].push(t);
    }
    for b in &mut buckets {
        b.reverse();
    }
    Some(buckets)
}

// ----- the bind pose math (System.Numerics, row-vector matrices) -----

type Quat = [f32; 4];
type Vec3 = [f32; 3];

fn quat_mul(a: Quat, b: Quat) -> Quat {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    let cx = ay * bz - az * by;
    let cy = az * bx - ax * bz;
    let cz = ax * by - ay * bx;
    let dot = ax * bx + ay * by + az * bz;
    [ax * bw + bx * aw + cx, ay * bw + by * aw + cy, az * bw + bz * aw + cz, aw * bw - dot]
}

/// `QuaternionExtensions.Multiply`: rotates `v`.
fn quat_rotate(q: Quat, v: Vec3) -> Vec3 {
    let (axx, ayy, azz) = (q[0] * 2.0, q[1] * 2.0, q[2] * 2.0);
    let (awxx, awyy, awzz) = (q[3] * axx, q[3] * ayy, q[3] * azz);
    let (axxx, axyy, axzz) = (q[0] * axx, q[0] * ayy, q[0] * azz);
    let (ayyy, ayzz, azzz) = (q[1] * ayy, q[1] * azz, q[2] * azz);
    [
        v[0] * ((1.0 - ayyy) - azzz) + v[1] * (axyy - awzz) + v[2] * (axzz + awyy),
        v[0] * (axyy + awzz) + v[1] * ((1.0 - axxx) - azzz) + v[2] * (ayzz - awxx),
        v[0] * (axzz - awyy) + v[1] * (ayzz + awxx) + v[2] * ((1.0 - axxx) - ayyy),
    ]
}

/// `Matrix4x4Ext.Transformation`: scale, then rotation, then translation.
fn transformation(s: Vec3, q: Quat, t: Vec3) -> Matrix4 {
    let [x, y, z, w] = q;
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, wz, xz, wy, yz, wx) = (x * y, z * w, z * x, y * w, y * z, x * w);
    let r = [
        [1.0 - 2.0 * (yy + zz), 2.0 * (xy + wz), 2.0 * (xz - wy)],
        [2.0 * (xy - wz), 1.0 - 2.0 * (zz + xx), 2.0 * (yz + wx)],
        [2.0 * (xz + wy), 2.0 * (yz - wx), 1.0 - 2.0 * (yy + xx)],
    ];
    let mut m = [0.0; 16];
    for i in 0..3 {
        for j in 0..3 {
            m[i * 4 + j] = s[i] * r[i][j];
        }
    }
    m[12] = t[0];
    m[13] = t[1];
    m[14] = t[2];
    m[15] = 1.0;
    m
}

/// `Matrix4x4.Invert` (NaNs when it isn't invertible).
fn invert(m: &Matrix4) -> Matrix4 {
    let [a, b, c, d, e, f, g, h, i, j, k, l, mm, n, o, p] = *m;
    let kp_lo = k * p - l * o;
    let jp_ln = j * p - l * n;
    let jo_kn = j * o - k * n;
    let ip_lm = i * p - l * mm;
    let io_km = i * o - k * mm;
    let in_jm = i * n - j * mm;
    let a11 = f * kp_lo - g * jp_ln + h * jo_kn;
    let a12 = -(e * kp_lo - g * ip_lm + h * io_km);
    let a13 = e * jp_ln - f * ip_lm + h * in_jm;
    let a14 = -(e * jo_kn - f * io_km + g * in_jm);
    let det = a * a11 + b * a12 + c * a13 + d * a14;
    if det.abs() < f32::EPSILON {
        return [f32::NAN; 16];
    }
    let inv = 1.0 / det;
    let gp_ho = g * p - h * o;
    let fp_hn = f * p - h * n;
    let fo_gn = f * o - g * n;
    let ep_hm = e * p - h * mm;
    let eo_gm = e * o - g * mm;
    let en_fm = e * n - f * mm;
    let gl_hk = g * l - h * k;
    let fl_hj = f * l - h * j;
    let fk_gj = f * k - g * j;
    let el_hi = e * l - h * i;
    let ek_gi = e * k - g * i;
    let ej_fi = e * j - f * i;
    [
        a11 * inv,
        -(b * kp_lo - c * jp_ln + d * jo_kn) * inv,
        (b * gp_ho - c * fp_hn + d * fo_gn) * inv,
        -(b * gl_hk - c * fl_hj + d * fk_gj) * inv,
        a12 * inv,
        (a * kp_lo - c * ip_lm + d * io_km) * inv,
        -(a * gp_ho - c * ep_hm + d * eo_gm) * inv,
        (a * gl_hk - c * el_hi + d * ek_gi) * inv,
        a13 * inv,
        -(a * jp_ln - b * ip_lm + d * in_jm) * inv,
        (a * fp_hn - b * ep_hm + d * en_fm) * inv,
        -(a * fl_hj - b * el_hi + d * ej_fi) * inv,
        a14 * inv,
        (a * jo_kn - b * io_km + c * in_jm) * inv,
        -(a * fo_gn - b * eo_gm + c * en_fm) * inv,
        (a * fk_gj - b * ek_gi + c * ej_fi) * inv,
    ]
}

/// `BuildBindMatrices`: each bone's local transform (with its `transform_unk`
/// as column 4), and the inverse of its global transform.
fn bind_matrices(bones: &[Bone]) -> (Vec<Matrix4>, Vec<Matrix4>) {
    let mut mats = Vec::with_capacity(bones.len());
    let mut invs = Vec::with_capacity(bones.len());
    for bone in bones {
        let mut m = transformation(bone.scale, bone.rotation, bone.position);
        let t = bone.transform_unk;
        (m[3], m[7], m[11], m[15]) = (t[0], t[1], t[2], t[3]);
        let (mut pos, mut ori) = (bone.position, bone.rotation);
        // AssignBoneParents, with the parent indices of BuildIndices.
        let mut parent = bone.parent_index;
        let mut steps = 0;
        while parent >= 0 && (parent as usize) < bones.len() && steps <= bones.len() {
            let p = &bones[parent as usize];
            let r = quat_rotate(p.rotation, pos);
            pos = [r[0] + p.position[0], r[1] + p.position[1], r[2] + p.position[2]];
            ori = quat_mul(p.rotation, ori);
            parent = p.parent_index;
            steps += 1;
        }
        let mut mi = invert(&transformation(bone.scale, ori, pos));
        (mi[3], mi[7], mi[11], mi[15]) = (0.0, 0.0, 0.0, 0.0);
        mats.push(m);
        invs.push(mi);
    }
    (mats, invs)
}

/// `Rsc8JointLimit` (48 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct JointLimit {
    pub min: [f32; 4],
    pub max: [f32; 4],
    pub bone_tag: u16,
    pub unk0: u16,
    pub unk1: u32,
    pub unk2: u32,
    pub unk3: u32,
}

impl JointLimit {
    fn fields(&self) -> Vec<StructField<'static>> {
        vec![
            StructField::Vec4("Min", self.min),
            StructField::Vec4("Max", self.max),
            StructField::U16("BoneTag", self.bone_tag),
            StructField::U16("Unk0", self.unk0),
            StructField::U32("Unk1", self.unk1),
            StructField::U32("Unk2", self.unk2),
            StructField::U32("Unk3", self.unk3),
        ]
    }

    fn read_xml(n: XmlNode) -> Self {
        Self {
            min: n.vec4("Min"),
            max: n.vec4("Max"),
            bone_tag: n.u16("BoneTag", 0),
            unk0: n.u16("Unk0", 0),
            unk1: n.u32("Unk1", 0),
            unk2: n.u32("Unk2", 0),
            unk3: n.u32("Unk3", 0),
        }
    }
}

/// `Rsc8JointLimits` (0x60 bytes): rotation and translation limits by bone tag.
#[derive(Clone, Debug, PartialEq)]
pub struct JointLimits {
    pub unknown_00h: u64,
    pub unknown_08h: u64,
    pub unknown_10h: u64,
    pub unknown_18h: u64,
    pub rotation_limits: Option<Vec<JointLimit>>,
    pub translation_limits: Option<Vec<JointLimit>>,
    pub unknown_30h: u64,
    pub unknown_38h: u64,
    pub unknown_40h: u64,
    pub unknown_48h: u32,
    pub unknown_4ch: u16,
    pub rotation_limits_count: u16,
    pub translation_limits_count: u16,
    pub unknown_52h: u16,
    pub unknown_54h: u16,
    pub rotation_limits_count2: u16,
    pub translation_limits_count2: u16,
    pub unknown_5ah: u16,
    pub unknown_5ch: u32,
}

impl Default for JointLimits {
    fn default() -> Self {
        Self {
            unknown_00h: 0,
            unknown_08h: 0,
            unknown_10h: 1,
            unknown_18h: 0,
            rotation_limits: None,
            translation_limits: None,
            unknown_30h: 0,
            unknown_38h: 0,
            unknown_40h: 0,
            unknown_48h: 0,
            unknown_4ch: 0,
            rotation_limits_count: 0,
            translation_limits_count: 0,
            unknown_52h: 0,
            unknown_54h: 0,
            rotation_limits_count2: 0,
            translation_limits_count2: 0,
            unknown_5ah: 0,
            unknown_5ch: 0,
        }
    }
}

impl JointLimits {
    pub const BLOCK_LENGTH: usize = 0x60;

    pub fn read(r: &mut Rsc8Reader) -> Result<Self> {
        let mut j = Self {
            unknown_00h: r.u64()?,
            unknown_08h: r.u64()?,
            unknown_10h: r.u64()?,
            unknown_18h: r.u64()?,
            ..Default::default()
        };
        let rot_ptr = r.u64()?;
        let trans_ptr = r.u64()?;
        j.unknown_30h = r.u64()?;
        j.unknown_38h = r.u64()?;
        j.unknown_40h = r.u64()?;
        j.unknown_48h = r.u32()?;
        j.unknown_4ch = r.u16()?;
        j.rotation_limits_count = r.u16()?;
        j.translation_limits_count = r.u16()?;
        j.unknown_52h = r.u16()?;
        j.unknown_54h = r.u16()?;
        j.rotation_limits_count2 = r.u16()?;
        j.translation_limits_count2 = r.u16()?;
        j.unknown_5ah = r.u16()?;
        j.unknown_5ch = r.u32()?;
        j.rotation_limits = read_array(r, rot_ptr, j.rotation_limits_count as usize)?;
        j.translation_limits = read_array(r, trans_ptr, j.translation_limits_count as usize)?;
        Ok(j)
    }

    pub fn write(&self, w: &mut Rsc8Writer) -> BlockId {
        let mut b = BlockBuf::with_capacity(Self::BLOCK_LENGTH);
        b.u64(self.unknown_00h);
        b.u64(self.unknown_08h);
        b.u64(self.unknown_10h);
        b.u64(self.unknown_18h);
        b.ptr(w.array(self.rotation_limits.as_deref()));
        b.ptr(w.array(self.translation_limits.as_deref()));
        b.u64(self.unknown_30h);
        b.u64(self.unknown_38h);
        b.u64(self.unknown_40h);
        b.u32(self.unknown_48h);
        b.u16(self.unknown_4ch);
        b.u16(self.rotation_limits_count);
        b.u16(self.translation_limits_count);
        b.u16(self.unknown_52h);
        b.u16(self.unknown_54h);
        b.u16(self.rotation_limits_count2);
        b.u16(self.translation_limits_count2);
        b.u16(self.unknown_5ah);
        b.u32(self.unknown_5ch);
        w.add(b, false)
    }

    pub fn write_xml(&self, w: &mut XmlWriter) {
        write_struct_array(w, "RotationLimits", self.rotation_limits.as_deref(), JointLimit::fields);
        write_struct_array(w, "TranslationLimits", self.translation_limits.as_deref(), JointLimit::fields);
    }

    pub fn read_xml(node: XmlNode) -> Self {
        let rot = read_struct_array(node, "RotationLimits", JointLimit::read_xml);
        let trans = read_struct_array(node, "TranslationLimits", JointLimit::read_xml);
        let (rc, tc) = (rot.as_ref().map_or(0, |v| v.len() as u16), trans.as_ref().map_or(0, |v| v.len() as u16));
        Self {
            rotation_limits: rot,
            translation_limits: trans,
            rotation_limits_count: rc,
            rotation_limits_count2: rc,
            translation_limits_count: tc,
            translation_limits_count2: tc,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bone_tags_reverse_their_buckets() {
        let b = build_bone_tags(&[(1, 0), (12, 1), (23, 2), (5, 3)]).unwrap();
        assert_eq!(b.len(), 11);
        assert_eq!(b[1], vec![(23, 2), (12, 1), (1, 0)]);
        assert_eq!(b[5], vec![(5, 3)]);
        assert!(build_bone_tags(&[(0, 0)]).is_none());
    }

    #[test]
    fn bind_pose_of_a_chain() {
        let root = Bone {
            position: [1.0, 0.0, 0.0],
            parent_index: -1,
            transform_unk: [0.0, 4.0, -3.0, 0.0],
            ..Default::default()
        };
        let child = Bone { position: [0.0, 2.0, 0.0], parent_index: 0, ..Default::default() };
        let (pose, inv) = bind_matrices(&[root, child]);
        assert_eq!([pose[0][3], pose[0][7], pose[0][11], pose[0][15]], [0.0, 4.0, -3.0, 0.0]);
        assert_eq!([pose[1][12], pose[1][13], pose[1][14]], [0.0, 2.0, 0.0]);
        // The inverse global transform of the child moves (1, 2, 0) to the origin.
        assert_eq!([inv[1][12], inv[1][13], inv[1][14]], [-1.0, -2.0, 0.0]);
    }
}
