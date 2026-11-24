//! Editing a ymap's occluders: its box occluders (`boxOccluders`) and occlude
//! models (`occludeModels`), the game's own included. A changed ymap is a
//! [`MapDoc`](crate::MapDoc) like any other: it goes into the project, and
//! exported it replaces the game's of the same name.
//!
//! - A **box** is `CenterXYZ_CosZ` and `SizeXYZ_SinZ`: its centre, its whole
//!   size, and the cosine and sine of its angle about Z. As a
//!   [`Placement`] it's at its centre, turned about Z, scaled by its size.
//! - A **model** is world-space vertices (`verts`: `numVertsInBytes / 12`
//!   `f32` triples, then `u8` indices) and their box (`bmin`, `bmax`). As a
//!   placement it's at its box's centre, not turned, at scale 1; a change of
//!   placement is written into its vertices.

use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};
use rf_formats::formats::databag::{DataBag, Value};

use crate::{DocKey, Editor, Placement};

const BOXES: &str = "boxOccluders";
const MODELS: &str = "occludeModels";

/// The thinnest a box gets.
const MIN_SIZE: f32 = 0.01;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OccluderKind {
    Box,
    Model,
}

impl OccluderKind {
    fn field(self) -> &'static str {
        match self {
            OccluderKind::Box => BOXES,
            OccluderKind::Model => MODELS,
        }
    }
}

/// An occluder: its ymap's name hash, and its index among the ymap's boxes or
/// models. Deleting moves the indices after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OccluderId {
    pub map: u32,
    pub kind: OccluderKind,
    pub index: u32,
}

fn item(map: &DataBag, kind: OccluderKind, index: u32) -> Option<&DataBag> {
    map.bags(kind.field())?.get(index as usize)?.as_ref()
}

fn item_mut(map: &mut DataBag, kind: OccluderKind, index: u32) -> Option<&mut DataBag> {
    map.bags_mut(kind.field())?.get_mut(index as usize)?.as_mut()
}

/// How many boxes or models a `CMapData` has.
pub fn count(map: &DataBag, kind: OccluderKind) -> usize {
    map.bags(kind.field()).map_or(0, <[_]>::len)
}

/// Where an occluder of a `CMapData` is.
pub fn placement(map: &DataBag, kind: OccluderKind, index: u32) -> Option<Placement> {
    let o = item(map, kind, index)?;
    Some(match kind {
        OccluderKind::Box => {
            let (c, s) = (o.vec4("CenterXYZ_CosZ"), o.vec4("SizeXYZ_SinZ"));
            Placement {
                position: Vec3::new(c[0], c[1], c[2]),
                orientation: Quat::from_rotation_z(s[3].atan2(c[3])),
                scale: Vec3::new(s[0], s[1], s[2]),
            }
        }
        OccluderKind::Model => {
            let (min, max) = (Vec3::from(o.vec3("bmin")), Vec3::from(o.vec3("bmax")));
            Placement { position: (min + max) * 0.5, ..Default::default() }
        }
    })
}

/// How far a placement is turned about Z.
pub fn yaw(p: &Placement) -> f32 {
    let x = p.orientation * Vec3::X;
    x.y.atan2(x.x)
}

/// Puts an occluder that is at `from` at `to`. A box takes `to` as it is (its
/// turn about Z only); a model's vertices go through the change from `from`
/// to `to`, and its box is made again.
pub fn set(map: &mut DataBag, kind: OccluderKind, index: u32, from: &Placement, to: &Placement) -> bool {
    let Some(o) = item_mut(map, kind, index) else { return false };
    match kind {
        OccluderKind::Box => {
            let (sin, cos) = yaw(to).sin_cos();
            let size = to.scale.abs().max(Vec3::splat(MIN_SIZE));
            o.set_vec4("CenterXYZ_CosZ", [to.position.x, to.position.y, to.position.z, cos]);
            o.set_vec4("SizeXYZ_SinZ", [size.x, size.y, size.z, sin]);
            true
        }
        OccluderKind::Model => {
            let change = to.matrix() * from.matrix().inverse();
            if !change.is_finite() {
                return false;
            }
            let vert_bytes = o.u16("numVertsInBytes") as usize;
            let Some(Value::Bytes(data)) = o.object_mut("verts") else { return false };
            let n = vert_bytes.min(data.len());
            let (mut min, mut max) = (Vec3::MAX, Vec3::MIN);
            for v in data[..n].as_chunks_mut::<12>().0 {
                let at = |i: usize| f32::from_le_bytes([v[i * 4], v[i * 4 + 1], v[i * 4 + 2], v[i * 4 + 3]]);
                let p = change.transform_point3(Vec3::new(at(0), at(1), at(2)));
                for (i, c) in p.to_array().into_iter().enumerate() {
                    v[i * 4..i * 4 + 4].copy_from_slice(&c.to_le_bytes());
                }
                min = min.min(p);
                max = max.max(p);
            }
            if min.x > max.x {
                return false;
            }
            o.set_vec3("bmin", min.into());
            o.set_vec3("bmax", max.into());
            true
        }
    }
}

/// Removes occluders of a kind by index; returns how many went.
pub fn remove(map: &mut DataBag, kind: OccluderKind, indices: &[u32]) -> usize {
    let Some(list) = map.bags_mut(kind.field()) else { return 0 };
    let before = list.len();
    let mut i = 0u32;
    list.retain(|_| {
        i += 1;
        !indices.contains(&(i - 1))
    });
    before - list.len()
}

impl Editor {
    /// Opens a ymap so its occluders can be read and changed (the game's
    /// file, or what it already is in the editor).
    pub fn open_map(&mut self, name: u32) -> bool {
        self.open(name).is_ok()
    }

    /// Where an occluder is; its ymap must be [open](Self::open_map).
    pub fn occluder_placement(&self, id: OccluderId) -> Option<Placement> {
        placement(&self.map(id.map)?.root, id.kind, id.index)
    }

    /// How many boxes or models an open ymap has.
    pub fn occluder_count(&self, map: u32, kind: OccluderKind) -> usize {
        self.map(map).map_or(0, |m| count(&m.root, kind))
    }

    /// Puts occluders elsewhere (each from where it is, to where it goes):
    /// one step. Returns whether anything changed.
    pub fn set_occluders(&mut self, label: &str, items: &[(OccluderId, Placement, Placement)]) -> bool {
        let mut roots: HashMap<DocKey, DataBag> = HashMap::new();
        for (id, from, to) in items {
            if from == to || self.open(id.map).is_err() {
                continue;
            }
            let key = DocKey::Map(id.map);
            let Some(base) = self.root(key) else { continue };
            let mut root = roots.remove(&key).unwrap_or_else(|| (**base).clone());
            if set(&mut root, id.kind, id.index, from, to) {
                roots.insert(key, root);
            }
        }
        if roots.is_empty() {
            return false;
        }
        let selection = self.selection.clone();
        self.commit(label, roots.into_iter().collect(), selection);
        true
    }

    /// Removes occluders from their ymaps: one step. Returns how many went.
    pub fn delete_occluders(&mut self, ids: &[OccluderId]) -> Result<usize, String> {
        let mut by_list: HashMap<(u32, OccluderKind), Vec<u32>> = HashMap::new();
        for id in ids {
            by_list.entry((id.map, id.kind)).or_default().push(id.index);
        }
        let mut roots: HashMap<DocKey, DataBag> = HashMap::new();
        let mut gone = 0;
        for ((map, kind), indices) in by_list {
            self.open(map)?;
            let key = DocKey::Map(map);
            let base = self.root(key).ok_or("map not open")?;
            let mut root = roots.remove(&key).unwrap_or_else(|| (**base).clone());
            let n = remove(&mut root, kind, &indices);
            gone += n;
            if n > 0 {
                roots.insert(key, root);
            }
        }
        if gone > 0 {
            let selection = self.selection.clone();
            self.commit("Delete occluders", roots.into_iter().collect(), selection);
        }
        Ok(gone)
    }
}

/// The change that takes what's at `from` to `to`, for showing an occluder
/// where a drag has it.
pub fn change(from: &Placement, to: &Placement) -> Mat4 {
    to.matrix() * from.matrix().inverse()
}

#[cfg(test)]
mod tests {
    use rf_formats::formats::meta::Meta;

    use super::*;

    fn map() -> DataBag {
        let mut b = Meta::create("BoxOccluder").unwrap();
        let (sin, cos) = 0.5f32.sin_cos();
        b.set_vec4("CenterXYZ_CosZ", [10.0, 20.0, 30.0, cos]);
        b.set_vec4("SizeXYZ_SinZ", [4.0, 2.0, 6.0, sin]);
        let mut m = Meta::create("OccludeModel").unwrap();
        let verts = [[0.0f32, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 4.0, 0.0]];
        let mut data: Vec<u8> = verts.iter().flatten().flat_map(|f| f.to_le_bytes()).collect();
        data.extend([0u8, 1, 2]);
        m.set_u16("numVertsInBytes", 36);
        m.set_u32("dataSize", 39);
        m.set_object("verts", Some(Value::Bytes(data)));
        m.set_vec3("bmin", [0.0, 0.0, 0.0]);
        m.set_vec3("bmax", [2.0, 4.0, 0.0]);
        let mut root = Meta::create("CMapData").unwrap();
        root.set_object(BOXES, Some(Value::Bags(vec![Some(b.clone()), Some(b)])));
        root.set_object(MODELS, Some(Value::Bags(vec![Some(m)])));
        root
    }

    #[test]
    fn a_box_reads_and_writes_its_angle() {
        let mut root = map();
        let p = placement(&root, OccluderKind::Box, 0).unwrap();
        assert!((yaw(&p) - 0.5).abs() < 1e-6);
        assert_eq!((p.position, p.scale), (Vec3::new(10.0, 20.0, 30.0), Vec3::new(4.0, 2.0, 6.0)));
        let to = Placement {
            position: Vec3::new(1.0, 2.0, 3.0),
            orientation: Quat::from_rotation_z(-1.2),
            scale: Vec3::new(5.0, -1.0, 0.0),
        };
        assert!(set(&mut root, OccluderKind::Box, 0, &p, &to));
        let back = placement(&root, OccluderKind::Box, 0).unwrap();
        assert!((yaw(&back) + 1.2).abs() < 1e-6);
        assert_eq!((back.position, back.scale), (to.position, Vec3::new(5.0, 1.0, MIN_SIZE)));
        // The other box is as it was.
        assert_eq!(placement(&root, OccluderKind::Box, 1).unwrap().position, p.position);
    }

    #[test]
    fn a_model_moves_its_vertices_and_its_box() {
        let mut root = map();
        let from = placement(&root, OccluderKind::Model, 0).unwrap();
        assert_eq!(from.position, Vec3::new(1.0, 2.0, 0.0));
        // A quarter turn about its centre, then 10 m up.
        let to = Placement {
            position: from.position + Vec3::Z * 10.0,
            orientation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            ..from
        };
        assert!(set(&mut root, OccluderKind::Model, 0, &from, &to));
        let o = item(&root, OccluderKind::Model, 0).unwrap();
        let (min, max) = (Vec3::from(o.vec3("bmin")), Vec3::from(o.vec3("bmax")));
        assert!((min - Vec3::new(-1.0, 1.0, 10.0)).length() < 1e-4, "{min}");
        assert!((max - Vec3::new(3.0, 3.0, 10.0)).length() < 1e-4, "{max}");
        let Some(Value::Bytes(data)) = o.object("verts") else { panic!() };
        assert_eq!((data.len(), &data[36..]), (39, &[0u8, 1, 2][..]));
    }

    #[test]
    fn removing_keeps_the_others() {
        let mut root = map();
        assert_eq!(remove(&mut root, OccluderKind::Box, &[0, 7]), 1);
        assert_eq!((count(&root, OccluderKind::Box), count(&root, OccluderKind::Model)), (1, 1));
        assert_eq!(remove(&mut root, OccluderKind::Model, &[0]), 1);
        assert_eq!(count(&root, OccluderKind::Model), 0);
    }
}
