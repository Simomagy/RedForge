//! The collision a navmesh is made from: every static collision triangle in a
//! box of the world, with its material.
//!
//! - The game's static bounds (the streamed ybns that aren't an interior's:
//!   terrain, buildings, the areas' combined collisions), by their boxes in
//!   the bounds cache.
//! - The bounds of the interiors placed in the box (each interior's ybns,
//!   where its instance puts them): the project's own interiors are placed by
//!   its ymaps the same way.
//! - A composite's children only when peds or horses collide with them
//!   (`CF_MAP_TYPE_MOVER`, `CF_MAP_TYPE_HORSE`): the ones only for weapons or
//!   the camera are left out, and triangles whose material says
//!   `FLAG_NO_NAVMESH` too.
//! - Spheres, capsules and cylinders are obstacles (posts, barrels, trunks):
//!   they come as the box around them, which is all a navmesh needs of them.
//!
//! Every triangle faces outwards: the game's are as stored, a primitive's box
//! is wound so.

use glam::{Mat4, Vec3};
use rf_formats::formats::ybn::{self, BoundMaterial, Bounds, BoundsShape, BoundsType, PolygonKind};
use rf_formats::rpf8::{FileRef, Rpf8FileSystem};
use rf_game::GameData;

/// `CF_MAP_TYPE_MOVER` and `CF_MAP_TYPE_HORSE` in a composite child's type
/// flags.
const MOVER: u64 = 1 << 2;
const HORSE: u64 = 1 << 3;

/// `FLAG_NO_NAVMESH` in a material's flags.
pub const FLAG_NO_NAVMESH: u16 = 1 << 9;
/// `FLAG_STAIRS`.
pub const FLAG_STAIRS: u16 = 1 << 0;

/// A collision triangle in the world, with the material of what it's of.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Triangle {
    pub v: [Vec3; 3],
    pub material: BoundMaterial,
}

impl Triangle {
    /// Its normal, unit length (up for a floor).
    pub fn normal(&self) -> Vec3 {
        (self.v[1] - self.v[0]).cross(self.v[2] - self.v[0]).normalize_or_zero()
    }
}

/// A box of the world (min, max).
pub type WorldBox = (Vec3, Vec3);

fn overlaps(a: WorldBox, b: WorldBox) -> bool {
    a.0.x <= b.1.x && a.1.x >= b.0.x && a.0.y <= b.1.y && a.1.y >= b.0.y && a.0.z <= b.1.z && a.1.z >= b.0.z
}

/// The world box around a box through a transform.
fn transformed_box(m: &Mat4, min: Vec3, max: Vec3) -> WorldBox {
    (0..8).fold((Vec3::MAX, Vec3::MIN), |(lo, hi), i| {
        let corner = Vec3::new(
            if i & 1 != 0 { max.x } else { min.x },
            if i & 2 != 0 { max.y } else { min.y },
            if i & 4 != 0 { max.z } else { min.z },
        );
        let p = m.transform_point3(corner);
        (lo.min(p), hi.max(p))
    })
}

/// A ybn and where it's placed (`None` for the world's own space).
#[derive(Clone, Copy, Debug)]
pub struct Placed {
    pub file: FileRef,
    pub place: Option<Mat4>,
}

/// The ybns with collision in `area`: the static store's, and those of the
/// interiors placed there (`interiors`: each placed one's archetype and
/// transform).
pub fn files_in(data: &GameData, area: WorldBox, interiors: &[(u32, Mat4)]) -> Vec<Placed> {
    // Boxes as wide as the map are a whole map's, not what's in the area.
    const MAP_WIDE: f32 = 3000.0;
    let mut out: Vec<Placed> = data
        .stream_entries("ybn")
        .filter(|(hash, _)| !data.interior_bounds_lookup.contains_key(hash))
        .filter(|(hash, _)| {
            data.bounds_cache.get(hash).is_some_and(|b| {
                let (min, max) = (Vec3::from(b.min), Vec3::from(b.max));
                (max.x - min.x).min(max.y - min.y) < MAP_WIDE && overlaps((min, max), area)
            })
        })
        .map(|(_, file)| Placed { file, place: None })
        .collect();
    for (archetype, place) in interiors {
        // The game's interiors list their bounds; a custom one's collision is
        // the ybn of its name (the archetype's asset).
        let own = [*archetype];
        let files = match data.interiors.get(archetype).and_then(|i| i.bounds.as_ref()) {
            Some(f) => f.as_slice(),
            None => &own,
        };
        for &hash in files {
            // A custom interior's isn't in the bounds cache: near its place.
            let near = match data.bounds_cache.get(&hash) {
                Some(b) => overlaps(transformed_box(place, b.min.into(), b.max.into()), area),
                None => {
                    let p = place.w_axis.truncate();
                    overlaps((p - Vec3::splat(100.0), p + Vec3::splat(100.0)), area)
                }
            };
            if near && let Some(file) = data.stream_entry("ybn", hash) {
                out.push(Placed { file, place: Some(*place) });
            }
        }
    }
    out
}

/// A child's transform in its composite (the game stores junk in the last
/// row).
fn child_transform(m: &[f32; 16]) -> Mat4 {
    let mut m = *m;
    (m[3], m[7], m[11], m[15]) = (0.0, 0.0, 0.0, 1.0);
    Mat4::from_cols_array(&m)
}

/// Whether peds or horses collide with a composite's child.
fn walked_on(b: &Bounds, in_composite: bool) -> bool {
    !in_composite || b.composite_flags1.type_flags & (MOVER | HORSE) != 0
}

/// The triangles of a ybn's bounds in `area`, placed.
pub fn triangles(root: &Bounds, place: Mat4, area: WorldBox, out: &mut Vec<Triangle>) {
    add(root, place, false, area, out);
}

/// Reads a placed ybn's triangles in `area`.
pub fn read_triangles(fs: &Rpf8FileSystem, p: Placed, area: WorldBox, out: &mut Vec<Triangle>) {
    match fs.extract_loose(p.file).map_err(rf_formats::Error::from).and_then(|d| ybn::read(&d)) {
        Ok(root) => triangles(&root, p.place.unwrap_or(Mat4::IDENTITY), area, out),
        Err(e) => log::warn!("{}: {e}", fs.path(p.file)),
    }
}

/// The collision of an archetype's model (a drawable's own, a fragment's
/// first physics LOD's or else its drawable's, a dictionary entry's), read
/// once per call through `cache`.
pub fn model_bounds(
    fs: &Rpf8FileSystem,
    hashes: &rf_formats::hash::HashIndex,
    data: &GameData,
    archetype: u32,
    cache: &mut std::collections::HashMap<u32, Option<std::sync::Arc<Bounds>>>,
) -> Option<std::sync::Arc<Bounds>> {
    use rf_formats::formats::{XmlContext, ydr, yft};
    use rf_world::ModelKind;
    if let Some(b) = cache.get(&archetype) {
        return b.clone();
    }
    let read = || -> Option<Bounds> {
        let mf = rf_world::model_files(data, archetype)?;
        let cx = XmlContext { hashes, folder: None };
        let bytes = fs.extract_loose(mf.file).ok()?;
        match mf.kind {
            ModelKind::Ydr => ydr::read(&bytes, &cx).ok()?.bounds,
            ModelKind::Yft => {
                let f = yft::read(&bytes, &cx).ok()?;
                let lod = f.physics_lod_group.and_then(|g| g.physics_lod1).and_then(|l| l.bounds);
                lod.or(f.drawable.and_then(|d| d.bounds))
            }
            ModelKind::Ydd => {
                let dict = ydr::read_dictionary(&bytes, &cx).ok()?;
                let hashes = dict.hashes.unwrap_or_default();
                let drawables = dict.drawables.unwrap_or_default();
                hashes.into_iter().zip(drawables).find(|(h, _)| *h == mf.entry).and_then(|(_, d)| d?.bounds)
            }
        }
    };
    let b = read().map(std::sync::Arc::new);
    cache.insert(archetype, b.clone());
    b
}

fn add(b: &Bounds, parent: Mat4, in_composite: bool, area: WorldBox, out: &mut Vec<Triangle>) {
    if !walked_on(b, in_composite) {
        return;
    }
    let place = parent * child_transform(&b.transform);
    if !overlaps(transformed_box(&place, b.box_min.into(), b.box_max.into()), area) {
        return;
    }
    let material = BoundMaterial { data1: b.material, data2: b.material2 };
    match &b.shape {
        BoundsShape::Composite(c) => {
            for child in c.children.iter().flatten().flatten() {
                add(child, place, true, area, out);
            }
        }
        BoundsShape::Geometry(g) | BoundsShape::GeometryBvh(g, _) => {
            let vertices = g.vertices.as_deref().unwrap_or_default();
            let materials = g.materials.as_deref().unwrap_or_default();
            let offset = Vec3::from(g.center_geom);
            let v = |i: i64| {
                let p = usize::try_from(i).ok().and_then(|i| vertices.get(i))?;
                Some(place.transform_point3(Vec3::from(*p) + offset))
            };
            for p in &g.polygons {
                let material = usize::try_from(p.material_index).ok().and_then(|i| materials.get(i)).copied();
                let material = material.unwrap_or(material_of_bound(b));
                if material.data2.flags() & FLAG_NO_NAVMESH != 0 {
                    continue;
                }
                match p.kind {
                    PolygonKind::Triangle(t) => {
                        if let [Some(a), Some(b), Some(c)] =
                            [t.vert_index1, t.vert_index2, t.vert_index3].map(|i| v(i as i64))
                        {
                            out.push(Triangle { v: [a, b, c], material });
                        }
                    }
                    PolygonKind::Box(bx) => {
                        let corners = [bx.vert_index1, bx.vert_index2, bx.vert_index3, bx.vert_index4];
                        if let [Some(a), Some(b), Some(c), Some(d)] = corners.map(|i| v(i as i64)) {
                            box_triangles(poly_box(a, b, c, d), material, out);
                        }
                    }
                    PolygonKind::Sphere(s) => {
                        if let Some(c) = v(s.vert_index as i64) {
                            let r = Vec3::splat(s.sphere_radius);
                            aabb_triangles(c - r, c + r, material, out);
                        }
                    }
                    PolygonKind::Capsule(s) | PolygonKind::Cylinder(s) => {
                        if let (Some(a), Some(c)) = (v(s.vert_index1 as i64), v(s.vert_index2 as i64)) {
                            let r = Vec3::splat(s.capsule_radius);
                            aabb_triangles(a.min(c) - r, a.max(c) + r, material, out);
                        }
                    }
                    PolygonKind::Other(_) => {}
                }
            }
        }
        // A box, sphere, capsule or cylinder on its own: its box.
        _ if b.bounds_type != BoundsType::Disc => {
            if material.data2.flags() & FLAG_NO_NAVMESH != 0 {
                return;
            }
            let (min, max) = (Vec3::from(b.box_min), Vec3::from(b.box_max));
            box_triangles(
                place * Mat4::from_translation((min + max) * 0.5) * Mat4::from_scale(max - min),
                material,
                out,
            );
        }
        _ => {}
    }
}

fn material_of_bound(b: &Bounds) -> BoundMaterial {
    BoundMaterial { data1: b.material, data2: b.material2 }
}

/// The box a geometry's box polygon makes of its four corners (the C#'s
/// `GetBoxMatrix`): its axes' half sizes and its centre.
pub fn poly_box(v1: Vec3, v2: Vec3, v3: Vec3, v4: Vec3) -> Mat4 {
    let x = (v1 + v2) - (v3 + v4);
    let y = (v1 + v3) - (v2 + v4);
    let z = (v1 + v4) - (v2 + v3);
    let t = (v1 + v2 + v3 + v4) * 0.25;
    let dx = x.normalize_or(Vec3::X);
    let dy = y.normalize_or(Vec3::Y);
    let dz = dx.cross(dy).normalize_or(Vec3::Z);
    Mat4::from_cols(
        (dx * x.length() * 0.5).extend(0.0),
        (dy * y.length() * 0.5).extend(0.0),
        (dz * z.length() * 0.5).extend(0.0),
        t.extend(1.0),
    )
}

/// The 12 triangles of the unit box (-0.5 to 0.5) through a transform, each
/// facing out of the box.
fn box_triangles(m: Mat4, material: BoundMaterial, out: &mut Vec<Triangle>) {
    let c = |i: usize| {
        m.transform_point3(Vec3::new((i & 1) as f32 - 0.5, ((i >> 1) & 1) as f32 - 0.5, ((i >> 2) & 1) as f32 - 0.5))
    };
    let centre = m.transform_point3(Vec3::ZERO);
    for [a, b, cc, d] in [[0, 1, 3, 2], [4, 5, 7, 6], [0, 1, 5, 4], [2, 3, 7, 6], [0, 2, 6, 4], [1, 3, 7, 5]] {
        for v in [[c(a), c(b), c(cc)], [c(a), c(cc), c(d)]] {
            let t = Triangle { v, material };
            let middle = (v[0] + v[1] + v[2]) / 3.0;
            let v = if t.normal().dot(middle - centre) < 0.0 { [v[0], v[2], v[1]] } else { v };
            out.push(Triangle { v, material });
        }
    }
}

/// The triangles of an axis-aligned box.
fn aabb_triangles(min: Vec3, max: Vec3, material: BoundMaterial, out: &mut Vec<Triangle>) {
    box_triangles(Mat4::from_translation((min + max) * 0.5) * Mat4::from_scale(max - min), material, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_faces_out() {
        let mut out = Vec::new();
        aabb_triangles(Vec3::ZERO, Vec3::new(2.0, 2.0, 1.0), BoundMaterial::default(), &mut out);
        assert_eq!(out.len(), 12);
        let centre = Vec3::new(1.0, 1.0, 0.5);
        for t in &out {
            let middle = (t.v[0] + t.v[1] + t.v[2]) / 3.0;
            assert!(t.normal().dot(middle - centre) > 0.0, "{t:?}");
        }
        // Two face up, on top.
        let up: Vec<&Triangle> = out.iter().filter(|t| t.normal().z > 0.99).collect();
        assert_eq!(up.len(), 2);
        assert!(up.iter().all(|t| t.v.iter().all(|v| v.z == 1.0)));
    }
}
