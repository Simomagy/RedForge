//! The helpers the streamed ymaps hold ([`rf_world::helpers::MapHelpers`]):
//! "Occlusion" (box occluders, occlude models), "Time Cycle Modifier" (boxes)
//! and "Interior Instance" (each interior's rooms, portals and time cycle
//! modifier spheres), coloured as the C#'s parts.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use glam::{Mat4, Quat, Vec3};
use rf_world::helpers::{InteriorHelpers, MapHelpers, TimeCycleParams};

use super::shapes::{Geometry, Shapes};
use super::{Collect, HelperKey, HelperPart, Mode, PartInfo, rgba};

/// The parts made of each ymap's helpers, by mode, made again when its
/// helpers change.
#[derive(Default)]
pub struct MapParts {
    maps: HashMap<u32, Built>,
}

struct Built {
    from: Arc<MapHelpers>,
    parts: HashMap<Mode, Vec<HelperPart>>,
}

impl MapParts {
    pub fn collect(&mut self, mode: Mode, c: &Collect, shapes: &mut Shapes, out: &mut Vec<HelperPart>) {
        let maps = c.world.map_helpers();
        let names: HashSet<u32> = maps.iter().map(|m| m.0).collect();
        self.maps.retain(|n, _| names.contains(n));
        for (name, helpers) in maps {
            let built = self.maps.entry(name).or_insert_with(|| Built { from: helpers.clone(), parts: HashMap::new() });
            if !Arc::ptr_eq(&built.from, &helpers) {
                *built = Built { from: helpers.clone(), parts: HashMap::new() };
            }
            let parts = built.parts.entry(mode).or_insert_with(|| {
                let owner = c.world.map_file_name(name).unwrap_or_default().to_owned();
                let mut list = Vec::new();
                let mut m = Make { c, shapes, owner, source: name, mode, out: &mut list };
                match mode {
                    Mode::Occlusion => m.occlusion(&helpers),
                    Mode::TimeCycleModifier => m.time_cycle_modifiers(&helpers),
                    Mode::InteriorInstance => {
                        for (i, interior) in helpers.interiors.iter().enumerate() {
                            m.interior(i as u32, interior);
                        }
                    }
                    _ => {}
                }
                list
            });
            out.extend(parts.iter().cloned());
        }
    }
}

/// Making one ymap's parts for a mode.
struct Make<'a, 'b> {
    c: &'a Collect<'b>,
    shapes: &'a mut Shapes,
    owner: String,
    source: u32,
    mode: Mode,
    out: &'a mut Vec<HelperPart>,
}

fn v3(v: [f32; 3]) -> String {
    format!("{:.3}, {:.3}, {:.3}", v[0], v[1], v[2])
}

/// The world sphere around a unit box or sphere through `m`.
fn bounds_of(m: Mat4, radius: f32) -> (Vec3, f32) {
    let scale = m.x_axis.truncate().length().max(m.y_axis.truncate().length()).max(m.z_axis.truncate().length());
    (m.w_axis.truncate(), radius * scale)
}

impl Make<'_, '_> {
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        group: u32,
        index: u32,
        mesh: Arc<rf_render::HelperMesh>,
        transform: Mat4,
        colour: [u8; 4],
        (centre, radius): (Vec3, f32),
        info: PartInfo,
    ) {
        let key = HelperKey { mode: self.mode, source: self.source, group, index };
        let info = Arc::new(PartInfo { owner: self.owner.clone(), ..info });
        self.out.push(HelperPart { key, mesh, transform, colour, centre, radius, info });
    }

    fn tcm_fields(&self, p: &TimeCycleParams) -> Vec<(String, String)> {
        vec![
            ("percentage".into(), format!("{}", p.percentage)),
            ("range".into(), format!("{}", p.range)),
            ("startHour".into(), p.start_hour.to_string()),
            ("endHour".into(), p.end_hour.to_string()),
        ]
    }

    /// `RDR2BoxOccluder` and `RDR2OccludeModel`.
    fn occlusion(&mut self, h: &MapHelpers) {
        let gpu = self.c.gpu;
        for (i, b) in h.box_occluders.iter().enumerate() {
            let m = Mat4::from_scale_rotation_translation(
                Vec3::from(b.size),
                Quat::from_rotation_z(b.angle),
                Vec3::from(b.centre),
            );
            let mesh = self.shapes.unit_box(gpu, false);
            let info = PartInfo {
                type_name: "Box Occluder".into(),
                name: format!("Box Occluder {i}"),
                fields: vec![
                    ("Centre".into(), v3(b.centre)),
                    ("Size".into(), v3(b.size)),
                    ("Angle".into(), format!("{:.2}°", b.angle.to_degrees())),
                ],
                ..Default::default()
            };
            self.push(0, i as u32, mesh, m, rgba(0.75, 0.75, 0.75, 1.0), bounds_of(m, 0.87), info);
        }
        for (i, o) in h.occlude_models.iter().enumerate() {
            let mut g = Geometry::default();
            let colour = [0xA0, 0xA0, 0xA0, 0xFF];
            for t in o.indices.as_chunks::<3>().0 {
                let p = |k: u8| o.vertices.get(k as usize).copied().map(Vec3::from);
                if let (Some(a), Some(b), Some(c)) = (p(t[0]), p(t[1]), p(t[2])) {
                    g.add_triangle(a, b, c, colour);
                }
            }
            let Some(mesh) = g.upload(gpu, false) else { continue };
            let (min, max) = (Vec3::from(o.min), Vec3::from(o.max));
            let info = PartInfo {
                type_name: "Occlude Model".into(),
                name: format!("Occlude Model {i}"),
                fields: vec![
                    ("Triangles".into(), (o.indices.len() / 3).to_string()),
                    ("Vertices".into(), o.vertices.len().to_string()),
                    ("flags".into(), o.flags.to_string()),
                    ("bmin".into(), v3(o.min)),
                    ("bmax".into(), v3(o.max)),
                ],
                ..Default::default()
            };
            let bounds = ((min + max) * 0.5, (max - min).length() * 0.5);
            self.push(1, i as u32, mesh, Mat4::IDENTITY, [255; 4], bounds, info);
        }
    }

    /// `RDR2TimeCycleModifier`: boxes turned about their centres.
    fn time_cycle_modifiers(&mut self, h: &MapHelpers) {
        for (i, t) in h.time_cycle_modifiers.iter().enumerate() {
            let (min, max) = (Vec3::from(t.min), Vec3::from(t.max));
            let q = Quat::from_array(t.orientation).normalize();
            let m = Mat4::from_scale_rotation_translation(max - min, q, (min + max) * 0.5);
            let mesh = self.shapes.unit_box(self.c.gpu, true);
            let mut fields = vec![("minExtents".into(), v3(t.min)), ("maxExtents".into(), v3(t.max))];
            fields.extend(self.tcm_fields(&t.params));
            let info = PartInfo {
                type_name: "Time Cycle Modifier".into(),
                name: self.c.game.hashes.name(t.params.name),
                fields,
                ..Default::default()
            };
            self.push(0, i as u32, mesh, m, rgba(0.75, 0.75, 0.75, 0.75), bounds_of(m, 0.87), info);
        }
    }

    /// `RDR2MloRoom`, `RDR2MloPortal` and `RDR2MloTimeCycleModifier`, through
    /// the interior's transform.
    fn interior(&mut self, n: u32, h: &InteriorHelpers) {
        let gpu = self.c.gpu;
        let place = Mat4::from_cols_array(&h.transform);
        let interior = self.c.game.hashes.name(h.archetype);
        for (i, r) in h.rooms.iter().enumerate() {
            let (min, max) = (Vec3::from(r.min), Vec3::from(r.max));
            let m = place * Mat4::from_scale_rotation_translation(max - min, Quat::IDENTITY, (min + max) * 0.5);
            let info = PartInfo {
                type_name: "Room".into(),
                name: r.name.clone(),
                fields: vec![
                    ("Interior".into(), interior.clone()),
                    ("bbMin".into(), v3(r.min)),
                    ("bbMax".into(), v3(r.max)),
                ],
                ..Default::default()
            };
            let mesh = self.shapes.unit_box(gpu, true);
            self.push(n * 3, i as u32, mesh, m, rgba(0.75, 0.75, 0.75, 0.2), bounds_of(m, 0.87), info);
        }
        for (i, p) in h.portals.iter().enumerate() {
            let c = p.corners.map(Vec3::from);
            let (dz, dx) = (c[1] - c[0], c[3] - c[0]);
            let (nz, nx) = (dz.normalize_or(Vec3::Z), dx.normalize_or(Vec3::X));
            let ny = nz.cross(nx).normalize_or(Vec3::Y);
            let centre = (c[0] + c[1] + c[2] + c[3]) * 0.25;
            let local = Mat4::from_cols(
                (nx * dx.length()).extend(0.0),
                (ny * 0.001).extend(0.0),
                (nz * dz.length()).extend(0.0),
                centre.extend(1.0),
            );
            let m = place * local;
            let info = PartInfo {
                type_name: "Portal".into(),
                name: format!("From Room {} to Room {}", p.from, p.to),
                fields: vec![
                    ("Interior".into(), interior.clone()),
                    ("roomFrom".into(), p.from.to_string()),
                    ("roomTo".into(), p.to.to_string()),
                ],
                ..Default::default()
            };
            let mesh = self.shapes.unit_box(gpu, true);
            self.push(n * 3 + 1, i as u32, mesh, m, rgba(0.1, 0.1, 1.0, 0.75), bounds_of(m, 0.87), info);
        }
        for (i, t) in h.time_cycle_modifiers.iter().enumerate() {
            let m = place
                * Mat4::from_scale_rotation_translation(Vec3::splat(t.radius), Quat::IDENTITY, Vec3::from(t.centre));
            let mut fields = vec![("Interior".into(), interior.clone()), ("sphere".into(), v3(t.centre))];
            fields.push(("radius".into(), format!("{}", t.radius)));
            fields.extend(self.tcm_fields(&t.params));
            let info = PartInfo {
                type_name: "Time Cycle Modifier".into(),
                name: self.c.game.hashes.name(t.params.name),
                fields,
                ..Default::default()
            };
            let mesh = self.shapes.unit_sphere(gpu, true);
            self.push(n * 3 + 2, i as u32, mesh, m, rgba(1.0, 0.5, 0.1, 0.5), bounds_of(m, 1.0), info);
        }
    }
}
