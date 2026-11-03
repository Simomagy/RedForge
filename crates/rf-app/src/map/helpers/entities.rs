//! "Light" and "Skeleton": the lights and the bones of the entities in view.
//!
//! In "Light" mode the C# shows no helpers: a light is picked by its reach
//! (a sphere, a spot's cone, a capsule) and outlined by it. Here the reaches
//! are drawn see-through-nothing (no colour, but their pick ids), so that
//! what's hovered and selected is outlined the same. "Skeleton" shows each
//! bone as the C#'s octahedron (`CreateSkeletonPiece`), for the entities up
//! to 20 times their radius away.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use glam::{Mat4, Quat, Vec3};
use rf_game::lights::{LightEffect, LightKind};
use rf_render::{GpuModel, HelperMesh};

use super::shapes::{Geometry, Shapes};
use super::{Collect, HelperKey, HelperPart, Mode, PartInfo};

/// A light of an archetype: its mesh and where it goes in its bone's space
/// (or the model's), its bone, how far it reaches.
struct LightMesh {
    mesh: Arc<HelperMesh>,
    local: Mat4,
    bone: u16,
    reach: f32,
    info: Arc<PartInfo>,
}

/// A bone's octahedron, in its space.
type BoneMesh = (Arc<HelperMesh>, Arc<PartInfo>);

/// What's made for a model, while it's loaded (the weak reference keeps its
/// address from being another's).
struct ForModel<T> {
    model: Weak<GpuModel>,
    parts: Vec<T>,
}

#[derive(Default)]
pub struct EntityParts {
    lights: HashMap<(u32, usize), ForModel<LightMesh>>,
    bones: HashMap<usize, ForModel<BoneMesh>>,
}

/// An entity's place in a key: its index in the ymap, and with `sub` (a
/// light's or bone's index, below 2^`bits`) the one in its interior.
fn key(mode: Mode, id: rf_world::EntityId, sub: u32, bits: u32) -> HelperKey {
    HelperKey { mode, source: id.node, group: id.entity, index: sub | id.interior_entity.wrapping_add(1) << bits }
}

fn deg(r: f32) -> String {
    format!("{:.1}°", r.to_degrees())
}

/// Where a light is and which way it points, in its bone's space: Z along
/// its direction.
fn light_frame(l: &LightEffect) -> Mat4 {
    let dir = Vec3::from(l.direction).normalize_or(Vec3::NEG_Z);
    let q = Quat::from_rotation_arc(Vec3::Z, dir);
    Mat4::from_rotation_translation(q, Vec3::from(l.position))
}

fn light_info(l: &LightEffect, owner: String) -> PartInfo {
    let kind = match l.kind {
        LightKind::Point => "Point",
        LightKind::Spot => "Spot",
        LightKind::Capsule => "Capsule",
    };
    let hours: Vec<String> = (0..24).filter(|h| (l.time_flags >> h) & 1 == 1).map(|h| h.to_string()).collect();
    let mut fields = vec![
        ("Type".into(), kind.to_owned()),
        ("Colour × intensity".into(), format!("{:.2}, {:.2}, {:.2}", l.colour[0], l.colour[1], l.colour[2])),
        ("Range".into(), format!("{}", l.range)),
        ("Falloff exponent".into(), format!("{}", l.falloff_exponent)),
    ];
    if l.kind == LightKind::Spot {
        fields.push(("Cone inner".into(), deg(l.cone_inner)));
        fields.push(("Cone outer".into(), deg(l.cone_outer)));
    }
    if l.kind == LightKind::Capsule {
        fields.push(("Extents".into(), format!("{:.3}, {:.3}, {:.3}", l.extents[0], l.extents[1], l.extents[2])));
    }
    fields.push(("Hours on".into(), if hours.len() == 24 { "all".into() } else { hours.join(" ") }));
    fields.push(("Flags".into(), format!("0x{:08X}", l.flags)));
    if l.bone_tag != 0 {
        fields.push(("Bone".into(), l.bone_tag.to_string()));
    }
    PartInfo { type_name: "Light".into(), name: kind.to_owned(), owner, fields }
}

/// A light's reach as a mesh, in its bone's space.
fn light_mesh(c: &Collect, shapes: &mut Shapes, l: &LightEffect) -> Option<(Arc<HelperMesh>, Mat4)> {
    let frame = light_frame(l);
    let range = l.range.max(0.01);
    Some(match l.kind {
        LightKind::Point => (shapes.unit_sphere(c.gpu, false), frame * Mat4::from_scale(Vec3::splat(range))),
        LightKind::Spot => {
            let mut g = Geometry::default();
            g.add_spot(l.cone_outer, [255; 4]);
            (g.upload(c.gpu, false)?, frame * Mat4::from_scale(Vec3::splat(range)))
        }
        LightKind::Capsule => {
            let mut g = Geometry::default();
            let half = Vec3::Z * l.extents[1];
            g.add_capsule(-half, half, range, [255; 4]);
            (g.upload(c.gpu, false)?, frame)
        }
    })
}

/// The C#'s bone sizes when a skeleton has none: per bone, its axis (towards
/// its children) times its length, and its width.
fn bone_sizes(model: &GpuModel) -> Vec<(Vec3, f32)> {
    let bones = &model.skeleton;
    let radius = (Vec3::from(model.max) - Vec3::from(model.min)).length() * 0.5;
    let (min_w, max_w) = (radius * 0.004, radius * 0.02);
    bones
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let (mut sum, mut count, mut far) = (Vec3::ZERO, 0.0, (Vec3::ZERO, 0.0f32));
            for c in bones.iter().filter(|c| c.parent == i as i16) {
                let p = Vec3::from(c.position);
                sum += p;
                count += 1.0;
                if p.length() > far.1 {
                    far = (p, p.length());
                }
            }
            let avg = if count > 1.0 { sum / count } else { sum };
            let c = (avg + far.0) * 0.5;
            let a = c.abs();
            let (axis, length) = if a.x >= a.y && a.x > a.z {
                (Vec3::X, c.x)
            } else if a.y >= a.x && a.y > a.z {
                (Vec3::Y, c.y)
            } else {
                (Vec3::Z, c.z)
            };
            let width = ((length.abs() + Vec3::from(b.position).length()) * 0.05).min(max_w).max(min_w);
            (axis * length, width)
        })
        .collect()
}

/// `CreateSkeletonPiece`'s octahedron for a bone of size `s` (axis times
/// length) and width `w`, in the bone's space.
fn bone_octahedron(g: &mut Geometry, s: Vec3, w: f32) {
    let sa = s.abs();
    let axis = sa.normalize_or_zero();
    let slen = if sa.x > sa.y && sa.x > sa.z {
        s.x
    } else if sa.y > sa.z {
        s.y
    } else {
        s.z
    };
    let side = (Vec3::ONE - axis) * w;
    let len = slen.abs();
    let lon = (len - w).max(w * 0.1);
    let long = side + axis * lon;
    let short = side + axis * w.min(lon);
    let forward = slen > 0.0;
    let (sizep, sizen) = if forward { (long, short) } else { (short, long) };
    g.add_octahedron(sizep, sizen, axis * if forward { w } else { -w }, [0xC0, 0xC0, 0xC0, 0xFF]);
}

impl EntityParts {
    pub fn collect(&mut self, mode: Mode, c: &Collect, shapes: &mut Shapes, out: &mut Vec<HelperPart>) {
        self.lights.retain(|_, m| m.model.strong_count() > 0);
        self.bones.retain(|_, m| m.model.strong_count() > 0);
        match mode {
            Mode::Light => self.lights(c, shapes, out),
            Mode::Skeleton => self.skeletons(c, out),
            _ => {}
        }
    }

    fn lights(&mut self, c: &Collect, shapes: &mut Shapes, out: &mut Vec<HelperPart>) {
        let data = c.game.data();
        for e in c.entities {
            let Some(effects) = data.archetypes.get(&e.archetype).and_then(|a| a.lights.clone()) else { continue };
            let made = self.lights.entry((e.archetype, Arc::as_ptr(&e.model) as usize)).or_insert_with(|| {
                let owner = c.game.hashes.name(e.archetype);
                let parts = effects
                    .iter()
                    .filter_map(|l| {
                        let (mesh, local) = light_mesh(c, shapes, l)?;
                        let info = Arc::new(light_info(l, owner.clone()));
                        Some(LightMesh { mesh, local, bone: l.bone_tag, reach: l.range + l.extents[1], info })
                    })
                    .collect();
                ForModel { model: Arc::downgrade(&e.model), parts }
            });
            for (i, l) in made.parts.iter().enumerate() {
                let bone = (l.bone != 0).then(|| e.model.bones.iter().find(|b| b.0 == l.bone).map(|b| b.1)).flatten();
                let transform = e.transform * bone.unwrap_or(Mat4::IDENTITY) * l.local;
                out.push(HelperPart {
                    key: key(Mode::Light, e.id, i as u32, 8),
                    mesh: l.mesh.clone(),
                    transform,
                    colour: [255, 255, 255, 0],
                    centre: transform.w_axis.truncate(),
                    radius: l.reach,
                    info: l.info.clone(),
                });
            }
        }
    }

    fn skeletons(&mut self, c: &Collect, out: &mut Vec<HelperPart>) {
        for e in c.entities.iter().filter(|e| !e.model.skeleton.is_empty()) {
            let (min, max) = (Vec3::from(e.model.min), Vec3::from(e.model.max));
            let scale = e.transform.x_axis.truncate().length().max(e.transform.z_axis.truncate().length());
            let radius = (max - min).length() * 0.5 * scale;
            let centre = e.transform.transform_point3((min + max) * 0.5);
            if centre.distance(c.eye) - radius > radius * 20.0 {
                continue;
            }
            let made = self.bones.entry(Arc::as_ptr(&e.model) as usize).or_insert_with(|| {
                let owner = c.game.hashes.name(e.archetype);
                let sizes = bone_sizes(&e.model);
                let parts = e
                    .model
                    .skeleton
                    .iter()
                    .zip(sizes)
                    .enumerate()
                    .map(|(i, (b, (s, w)))| {
                        let mut g = Geometry::default();
                        bone_octahedron(&mut g, s, w);
                        let info = PartInfo {
                            type_name: "Bone".into(),
                            name: if b.name.is_empty() { format!("Bone {i}") } else { b.name.clone() },
                            owner: owner.clone(),
                            fields: vec![
                                ("Index".into(), i.to_string()),
                                ("Tag".into(), b.tag.to_string()),
                                ("Parent".into(), b.parent.to_string()),
                                (
                                    "Position".into(),
                                    format!("{:.3}, {:.3}, {:.3}", b.position[0], b.position[1], b.position[2]),
                                ),
                            ],
                        };
                        (g.upload(c.gpu, false).expect("an octahedron"), Arc::new(info))
                    })
                    .collect();
                ForModel { model: Arc::downgrade(&e.model), parts }
            });
            for (i, ((mesh, info), b)) in made.parts.iter().zip(&e.model.skeleton).enumerate() {
                let transform = e.transform * b.rest;
                out.push(HelperPart {
                    key: key(Mode::Skeleton, e.id, i as u32, 12),
                    mesh: mesh.clone(),
                    transform,
                    colour: [255; 4],
                    centre: transform.w_axis.truncate(),
                    radius: radius.max(0.5),
                    info: info.clone(),
                });
            }
        }
    }
}
