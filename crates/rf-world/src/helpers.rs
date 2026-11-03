//! What a ymap holds besides its entities that the editor shows in its
//! selection modes, as the C#'s `RDR2MapData` and `RDR2MloInstance` read it:
//! box occluders and occlude models ("Occlusion"), time cycle modifier boxes
//! ("Time Cycle Modifier"), and its interiors' rooms, portals and time cycle
//! modifier spheres ("Interior Instance").

use std::collections::HashMap;

use rf_formats::formats::databag::{DataBag, Value};
use rf_game::Archetype;

use crate::map::{Entity, EntityDef};
use crate::math::{self, Aabb, Quat, Vec3};

/// `RDR2BoxOccluder`: a box turned about Z.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxOccluder {
    pub centre: Vec3,
    pub size: Vec3,
    /// About Z, radians: `atan2(CosZ, SinZ)`, as the C# reads it.
    pub angle: f32,
}

/// `RDR2OccludeModel`: a mesh of triangles.
#[derive(Clone, Debug, PartialEq)]
pub struct OccludeModel {
    pub min: Vec3,
    pub max: Vec3,
    pub flags: u32,
    pub vertices: Vec<Vec3>,
    /// Three vertex indices a triangle.
    pub indices: Vec<u8>,
}

/// A time cycle modifier's numbers, shown as the C#'s fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimeCycleParams {
    pub name: u32,
    pub percentage: f32,
    pub range: f32,
    pub start_hour: u32,
    pub end_hour: u32,
}

/// `RDR2TimeCycleModifier`: a box, turned about its centre.
#[derive(Clone, Debug, PartialEq)]
pub struct TimeCycleBox {
    pub min: Vec3,
    pub max: Vec3,
    pub orientation: Quat,
    pub params: TimeCycleParams,
}

/// `RDR2MloRoom`, in the interior's space.
#[derive(Clone, Debug, PartialEq)]
pub struct Room {
    pub name: String,
    pub min: Vec3,
    pub max: Vec3,
}

/// `RDR2MloPortal`: its four corners, in the interior's space.
#[derive(Clone, Debug, PartialEq)]
pub struct Portal {
    pub from: u32,
    pub to: u32,
    pub corners: [Vec3; 4],
}

/// `RDR2MloTimeCycleModifier`: a sphere, in the interior's space.
#[derive(Clone, Debug, PartialEq)]
pub struct TimeCycleSphere {
    pub centre: Vec3,
    pub radius: f32,
    pub params: TimeCycleParams,
}

/// An interior placed in the ymap: what its archetype has, and where.
#[derive(Clone, Debug, PartialEq)]
pub struct InteriorHelpers {
    /// The instance's index in the ymap's entities.
    pub entity: u32,
    pub archetype: u32,
    /// Interior to world, column-major.
    pub transform: [f32; 16],
    pub rooms: Vec<Room>,
    pub portals: Vec<Portal>,
    pub time_cycle_modifiers: Vec<TimeCycleSphere>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapHelpers {
    pub box_occluders: Vec<BoxOccluder>,
    pub occlude_models: Vec<OccludeModel>,
    pub time_cycle_modifiers: Vec<TimeCycleBox>,
    pub interiors: Vec<InteriorHelpers>,
}

const NAME: u32 = 0xE6E1_7F14;

fn bags(bag: &DataBag, name: u32) -> impl Iterator<Item = &DataBag> {
    bag.bags(name).unwrap_or_default().iter().flatten()
}

fn params(b: &DataBag) -> TimeCycleParams {
    TimeCycleParams {
        name: b.hash(NAME),
        percentage: b.f32(0xAFB0_2915), // "percentage"
        range: b.f32(0x13D1_4FA4),      // "range"
        start_hour: b.u32(0x2543_DC07), // "startHour"
        end_hour: b.u32(0x16AF_8FB2),   // "endHour"
    }
}

/// A rotation matrix's quaternion (its upper 3x3, by columns).
fn matrix_quat(m: &[f32; 16]) -> Quat {
    let (m00, m11, m22) = (m[0], m[5], m[10]);
    let trace = m00 + m11 + m22;
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [(m[6] - m[9]) / s, (m[8] - m[2]) / s, (m[1] - m[4]) / s, 0.25 * s]
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        [0.25 * s, (m[4] + m[1]) / s, (m[8] + m[2]) / s, (m[6] - m[9]) / s]
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        [(m[4] + m[1]) / s, 0.25 * s, (m[9] + m[6]) / s, (m[8] - m[2]) / s]
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        [(m[8] + m[2]) / s, (m[9] + m[6]) / s, 0.25 * s, (m[1] - m[4]) / s]
    };
    math::quat_normalize(q)
}

impl MapHelpers {
    /// `InitOccluders` and `InitTimeCycleModifiers`, from a `CMapData`.
    pub(crate) fn read(bag: &DataBag) -> Self {
        let box_occluders = bags(bag, 0xED70_C614) // "boxOccluders"
            .map(|b| {
                let centre_cos = b.vec4(0xCC22_90C3); // "CenterXYZ_CosZ"
                let size_sin = b.vec4(0x614B_958A); // "SizeXYZ_SinZ"
                BoxOccluder {
                    centre: [centre_cos[0], centre_cos[1], centre_cos[2]],
                    size: [size_sin[0], size_sin[1], size_sin[2]],
                    angle: centre_cos[3].atan2(size_sin[3]),
                }
            })
            .collect();
        let occlude_models = bags(bag, 0x7F19_98DD) // "occludeModels"
            .map(|b| {
                let data = match b.object(0x072E_A9EF) {
                    // "verts"
                    Some(Value::Bytes(v)) => v.as_slice(),
                    _ => &[],
                };
                let vert_bytes = (b.u16(0x32E6_AB8B) as usize).min(data.len()); // "numVertsInBytes"
                let size = (b.u32(0x9199_755B) as usize).min(data.len()); // "dataSize"
                let vertices = data[..vert_bytes]
                    .as_chunks::<12>()
                    .0
                    .iter()
                    .map(|c| {
                        std::array::from_fn(|i| {
                            f32::from_le_bytes([c[i * 4], c[i * 4 + 1], c[i * 4 + 2], c[i * 4 + 3]])
                        })
                    })
                    .collect();
                let mut indices = data[vert_bytes..size.max(vert_bytes)].to_vec();
                indices.truncate(indices.len() / 3 * 3);
                OccludeModel {
                    min: b.vec3(0x78E9_A1BF), // "bmin"
                    max: b.vec3(0xBFF5_EFEB), // "bmax"
                    flags: b.u32(0x67D2_6872),
                    vertices,
                    indices,
                }
            })
            .collect();
        let time_cycle_modifiers = bags(bag, 0xAF9C_3BD9) // "timeCycleModifiers"
            .map(|b| TimeCycleBox {
                min: b.vec3(0x672D_4771),                            // "minExtents"
                max: b.vec3(0x9847_4238),                            // "maxExtents"
                orientation: matrix_quat(&b.matrix4x4(0xA093_0B32)), // "transform"
                params: params(b),
            })
            .collect();
        MapHelpers { box_occluders, occlude_models, time_cycle_modifiers, interiors: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.box_occluders.is_empty()
            && self.occlude_models.is_empty()
            && self.time_cycle_modifiers.is_empty()
            && self.interiors.is_empty()
    }
}

/// What an MLO archetype has for its helpers, in the interior's space.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct InteriorParts {
    /// Each room's name, box and attached objects.
    rooms: Vec<(String, Vec3, Vec3, Vec<u32>)>,
    portals: Vec<Portal>,
    time_cycle_modifiers: Vec<TimeCycleSphere>,
}

impl InteriorParts {
    pub fn read(arch: &DataBag) -> Self {
        let rooms = bags(arch, 0xCD20_D7A8) // "rooms"
            .map(|r| {
                let attached = match r.object(0x8E05_312C) {
                    // "attachedObjects"
                    Some(Value::U32s(v)) => v.clone(),
                    _ => Vec::new(),
                };
                (r.string(NAME).unwrap_or_default().to_owned(), r.vec3(0xD9EF_8236), r.vec3(0xE78A_A618), attached)
            })
            .collect();
        let portals = bags(arch, 0x89F7_E992) // "portals"
            .map(|p| {
                let corners = match p.object(0x4F42_AEB3) {
                    // "corners"
                    Some(Value::Vec4s(c)) if c.len() >= 4 => std::array::from_fn(|i| [c[i][0], c[i][1], c[i][2]]),
                    _ => [[0.0; 3]; 4],
                };
                Portal { from: p.u32(0xF470_D2FD), to: p.u32(0x9B64_9621), corners } // "roomFrom", "roomTo"
            })
            .collect();
        let time_cycle_modifiers = bags(arch, 0xAF9C_3BD9) // "timeCycleModifiers"
            .map(|t| {
                let s = t.vec4(0x38DA_06A2); // "sphere"
                TimeCycleSphere { centre: [s[0], s[1], s[2]], radius: s[3], params: params(t) }
            })
            .collect();
        InteriorParts { rooms, portals, time_cycle_modifiers }
    }

    /// The helpers of an instance (entity `index` of its ymap, placed by
    /// `mlo`; `entities` are the interior's, the plain ones then each set's):
    /// a room's box (but limbo's, room 0) is made again from its objects'
    /// boxes, as the C# does, since the game's are off.
    pub fn place(
        &self,
        index: u32,
        mlo: &Entity,
        entities: &[&EntityDef],
        archetypes: &HashMap<u32, Archetype>,
    ) -> InteriorHelpers {
        let rooms = self
            .rooms
            .iter()
            .enumerate()
            .map(|(i, (name, min, max, attached))| {
                let mut b = Aabb::EMPTY;
                if i != 0 {
                    for e in attached.iter().filter_map(|&o| entities.get(o as usize)) {
                        let Some(a) = archetypes.get(&e.archetype) else { continue };
                        let q = math::quat_inverse(e.rotation);
                        let s = [e.scale_xy, e.scale_xy, e.scale_z];
                        let m = math::transformation(s, q, e.position);
                        let eb = Aabb::new(a.bb_min, a.bb_max).transformed(s, q, &m);
                        b.extend(eb.min);
                        b.extend(eb.max);
                    }
                }
                let found = b.max[0] > b.min[0];
                let (min, max) = if found { (b.min, b.max) } else { (*min, *max) };
                Room { name: name.clone(), min, max }
            })
            .collect();
        InteriorHelpers {
            entity: index,
            archetype: mlo.archetype,
            transform: mlo.transform,
            rooms,
            portals: self.portals.clone(),
            time_cycle_modifiers: self.time_cycle_modifiers.clone(),
        }
    }
}
