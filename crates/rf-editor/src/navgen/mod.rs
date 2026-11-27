//! Making navmesh from collision (Recast, through the pure-Rust `rerecast`).
//!
//! [`collision`] gathers the triangles of a box of the world; [`generate`]
//! voxelises them and makes convex polygons where an agent of [`Settings`]
//! can stand and walk, each with the polygon across each of its edges.
//!
//! Recast is Y-up and RDR2 Z-up: a point `(x, y, z)` goes to Recast as
//! `(x, z, y)`. That mirrors the world, so triangles go with two corners
//! swapped to keep facing the same way, and polygons come back wound the
//! other way (they're put back counter-clockwise seen from above).

pub mod collision;
mod editor;
pub mod stitch;

pub use editor::{Generated, MadeNavmesh, cell_of};

use glam::Vec3;
use glam033 as g;
use rerecast::{AreaType, HeightfieldBuilder, PolygonNavmesh, TriMesh};

use collision::{Triangle, WorldBox};

/// What the navmesh is made for, and how finely.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// How far from walls the navmesh stays (metres).
    pub agent_radius: f32,
    /// The headroom it needs (metres).
    pub agent_height: f32,
    /// The highest step it walks up (metres).
    pub max_climb: f32,
    /// The steepest slope it walks on (degrees).
    pub max_slope: f32,
    /// The voxels' size across and up (metres).
    pub cell_size: f32,
    pub cell_height: f32,
    /// The longest edge along a wall (metres; 0 for no limit).
    pub max_edge_len: f32,
    /// How far an outline may stray from the voxels' (metres).
    pub max_error: f32,
    /// Corners a polygon may have.
    pub max_corners: u16,
    /// Islands smaller than this go (square metres), and regions smaller than
    /// the merge size are merged into their neighbours.
    pub min_region_area: f32,
    pub merge_region_area: f32,
}

/// The settings closest to the game's navmeshes (issue #6: 8 places, towns and
/// country, compared on a 0.5 m grid): 91% of the walkable ground the same.
/// A radius of 0.25-0.3 m fits best; the step (0.35-0.55 m) and the slope
/// (50-60°) hardly matter; an outline error of 0.3 gives the game's polygon
/// sizes in towns (the 4 m grid cut gives them in the open).
impl Default for Settings {
    fn default() -> Self {
        Self {
            agent_radius: 0.3,
            agent_height: 1.8,
            max_climb: 0.5,
            max_slope: 50.0,
            cell_size: 0.15,
            cell_height: 0.1,
            max_edge_len: 12.0,
            max_error: 0.3,
            max_corners: 6,
            min_region_area: 1.0,
            merge_region_area: 9.0,
        }
    }
}

/// A polygon made: its corners counter-clockwise seen from above, and for
/// each edge (from a corner to the next) the polygon across it, if any.
#[derive(Clone, Debug, PartialEq)]
pub struct Polygon {
    pub corners: Vec<Vec3>,
    pub neighbours: Vec<Option<u32>>,
    /// The Recast area its triangles had (see [`generate`]'s `area_of`).
    pub area: u8,
}

/// Where Recast has a point of the world.
fn to_recast(v: Vec3) -> g::Vec3A {
    g::Vec3A::new(v.x, v.z, v.y)
}

/// The area a triangle gives what's made on it: 0 for none (too steep, or
/// facing down), else 1 to 255.
pub type AreaOf<'a> = &'a dyn Fn(&Triangle) -> u8;

/// Makes polygons from triangles, over the box `area` (its Z range is where
/// to look for ground). `area_of` gives each walkable triangle its area: the
/// polygons don't cross from one area to another (0 is never walkable).
pub fn generate(triangles: &[Triangle], area: WorldBox, s: &Settings, area_of: AreaOf) -> Result<Vec<Polygon>, String> {
    let slope_cos = s.max_slope.to_radians().cos();
    let mut mesh = TriMesh::default();
    for t in triangles {
        let n = t.normal();
        let base = mesh.vertices.len() as u32;
        mesh.vertices.extend(t.v.iter().map(|&v| to_recast(v)));
        // Two corners swapped: the mirrored triangle faces the same way.
        mesh.indices.push(g::UVec3::new(base, base + 2, base + 1));
        let walkable = n.z >= slope_cos;
        mesh.area_types.push(if walkable { AreaType(area_of(t)) } else { AreaType::NOT_WALKABLE });
    }
    if mesh.indices.is_empty() {
        return Ok(Vec::new());
    }

    let cs = s.cell_size;
    let ch = s.cell_height;
    let walkable_radius = (s.agent_radius / cs).ceil() as u16;
    let walkable_height = (s.agent_height / ch).ceil() as u16;
    let walkable_climb = (s.max_climb / ch).floor() as u16;
    let border = walkable_radius + 3;
    // Recast's box: the area's, one border wider so that the edges inside the
    // area are made as if it went on.
    let pad = border as f32 * cs;
    let aabb = rerecast::Aabb3d {
        min: g::Vec3::new(area.0.x - pad, area.0.z, area.0.y - pad),
        max: g::Vec3::new(area.1.x + pad, area.1.z, area.1.y + pad),
    };
    let cells = |len: f32| (len / cs + 0.5) as u32;
    if cells(aabb.max.x - aabb.min.x) > u16::MAX as u32 || cells(aabb.max.z - aabb.min.z) > u16::MAX as u32 {
        return Err("the box is too big for this cell size".into());
    }

    let mut hf = HeightfieldBuilder { aabb, cell_size: cs, cell_height: ch }.build().map_err(|e| format!("{e:?}"))?;
    hf.rasterize_triangles(&mesh, walkable_climb).map_err(|e| format!("{e:?}"))?;
    hf.filter_low_hanging_walkable_obstacles(walkable_climb);
    hf.filter_ledge_spans(walkable_height, walkable_climb);
    hf.filter_walkable_low_height_spans(walkable_height);
    let mut chf = hf.into_compact(walkable_height, walkable_climb).map_err(|e| format!("{e:?}"))?;
    chf.erode_walkable_area(walkable_radius);
    chf.build_distance_field();
    let region = |m2: f32| ((m2 / (cs * cs)).sqrt().ceil() as u16).max(1);
    let (min_region, merge_region) = (region(s.min_region_area), region(s.merge_region_area));
    chf.build_regions(border, min_region * min_region, merge_region * merge_region).map_err(|e| format!("{e:?}"))?;
    let max_edge = if s.max_edge_len > 0.0 { (s.max_edge_len / cs) as u16 } else { 0 };
    let contours = chf.build_contours(s.max_error, max_edge, rerecast::BuildContoursFlags::default());
    let poly: PolygonNavmesh = contours.into_polygon_mesh(s.max_corners).map_err(|e| format!("{e:?}"))?;
    Ok(polygons(&poly))
}

/// The polygons of Recast's mesh, in the world, counter-clockwise from above.
fn polygons(m: &PolygonNavmesh) -> Vec<Polygon> {
    let nvp = m.max_vertices_per_polygon as usize;
    let world = |i: u16| {
        let v = m.vertices[i as usize];
        let x = m.aabb.min.x + v.x as f32 * m.cell_size;
        let up = m.aabb.min.y + v.y as f32 * m.cell_height;
        let y = m.aabb.min.z + v.z as f32 * m.cell_size;
        Vec3::new(x, y, up)
    };
    let count = m.polygons.len() / nvp.max(1);
    let mut out = Vec::with_capacity(count);
    for p in 0..count {
        let base = p * nvp;
        let ids: Vec<u16> =
            m.polygons[base..base + nvp].iter().copied().take_while(|&i| i != PolygonNavmesh::NO_INDEX).collect();
        let across: Vec<Option<u32>> = m.polygon_neighbors[base..base + ids.len()]
            .iter()
            .map(|&n| (n & 0x8000 == 0 && n != PolygonNavmesh::NO_CONNECTION).then_some(n as u32))
            .collect();
        let mut corners: Vec<Vec3> = ids.iter().map(|&i| world(i)).collect();
        let mut neighbours = across;
        // Mirrored: clockwise from above. Reversed, the edge from corner i to
        // i+1 is the one that went from i+1 to i.
        corners.reverse();
        let n = neighbours.len();
        neighbours = (0..n).map(|i| neighbours[(2 * n - 2 - i) % n]).collect();
        out.push(Polygon { corners, neighbours, area: *m.areas[p] });
    }
    out
}

/// The signed area of a polygon seen from above (positive counter-clockwise).
/// From its first corner and in f64: the world's coordinates (thousands of
/// metres) multiplied in f32 are metres off.
pub fn signed_area(c: &[Vec3]) -> f32 {
    let Some(&o) = c.first() else { return 0.0 };
    let n = c.len();
    let mut s = 0.0f64;
    for i in 0..n {
        let (a, b) = (c[i] - o, c[(i + 1) % n] - o);
        s += a.x as f64 * b.y as f64 - b.x as f64 * a.y as f64;
    }
    (s * 0.5) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_formats::formats::ybn::BoundMaterial;

    fn quad(min: Vec3, max: Vec3, z: f32) -> [Triangle; 2] {
        let m = BoundMaterial::default();
        let (a, b, c, d) = (
            Vec3::new(min.x, min.y, z),
            Vec3::new(max.x, min.y, z),
            Vec3::new(max.x, max.y, z),
            Vec3::new(min.x, max.y, z),
        );
        [Triangle { v: [a, b, c], material: m }, Triangle { v: [a, c, d], material: m }]
    }

    #[test]
    fn a_floor_makes_polygons_inside_it() {
        let floor = quad(Vec3::new(0.0, 0.0, 0.0), Vec3::new(10.0, 6.0, 0.0), 0.0);
        let area = (Vec3::new(-1.0, -1.0, -1.0), Vec3::new(11.0, 7.0, 3.0));
        let polys = generate(&floor, area, &Settings::default(), &|_| 1).unwrap();
        assert!(!polys.is_empty());
        let s = Settings::default();
        for p in &polys {
            assert!(signed_area(&p.corners) > 0.0, "{p:?}");
            assert_eq!(p.corners.len(), p.neighbours.len());
            for c in &p.corners {
                // On the floor, and the agent's radius in from its edges.
                assert!(c.z.abs() < 0.2, "{c}");
                assert!(c.x > s.agent_radius - 0.2 && c.x < 10.0 - s.agent_radius + 0.2, "{c}");
                assert!(c.y > s.agent_radius - 0.2 && c.y < 6.0 - s.agent_radius + 0.2, "{c}");
            }
        }
        let total: f32 = polys.iter().map(|p| signed_area(&p.corners)).sum();
        let inner = (10.0 - 2.0 * s.agent_radius) * (6.0 - 2.0 * s.agent_radius);
        assert!((total - inner).abs() < inner * 0.1, "{total} vs {inner}");
        // Neighbours point at each other.
        for (i, p) in polys.iter().enumerate() {
            for q in p.neighbours.iter().flatten() {
                assert!(polys[*q as usize].neighbours.contains(&Some(i as u32)));
            }
        }
    }

    #[test]
    fn areas_far_from_the_origin() {
        // A 15 cm square at Tumbleweed: 0.0225 m², not metres off.
        let o = Vec3::new(-5619.05, -3045.1, 1.0);
        let sq = [o, o + Vec3::X * 0.15, o + Vec3::new(0.15, 0.15, 0.0), o + Vec3::Y * 0.15];
        assert!((signed_area(&sq) - 0.0225).abs() < 1e-3, "{}", signed_area(&sq));
    }

    #[test]
    fn a_floor_facing_down_or_too_steep_makes_none() {
        let mut floor = quad(Vec3::ZERO, Vec3::new(10.0, 6.0, 0.0), 0.0);
        for t in &mut floor {
            t.v.swap(1, 2);
        }
        let area = (Vec3::new(-1.0, -1.0, -1.0), Vec3::new(11.0, 7.0, 3.0));
        assert!(generate(&floor, area, &Settings::default(), &|_| 1).unwrap().is_empty());
    }
}
