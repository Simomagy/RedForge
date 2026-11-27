//! The editor's side of making navmesh: a box of a navmesh cell made again
//! from the collision under it, as one undo step.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Mat4, Vec2, Vec3};
use rf_formats::formats::ynv::{Navmesh, NavmeshPolygon, NavmeshVertex, poly_flags};
use rf_formats::hash::joaat;

use super::collision::{self, FLAG_STAIRS, Triangle};
use super::stitch::{self, NewPolygon, Region, Stitched};
use super::{Settings, signed_area};
use crate::{DocKey, Editor, Root};

/// The navmesh grid: cells of 150 m from (-15900, -15900).
const GRID_CELL: f32 = 150.0;
const GRID_MIN: f32 = -15900.0;
/// The game's navmeshes cut open ground into squares this big (3.97 m in
/// Valentine's).
const GRID_SPLIT: f32 = 4.0;
/// How far a region keeps from its cell's sides.
const CELL_MARGIN: f32 = 1.5;
/// Polygons smaller than this are `Small` (as every one of the game's is).
const SMALL_AREA: f32 = 2.0;
/// Steeper polygons are `SteepSlope` (the game's have a median of 47°).
const STEEP: f32 = 45.0;
/// The Recast area of stairs (others are 1), so that they stay polygons of
/// their own.
const STAIRS_AREA: u8 = 2;

/// The flags a generated polygon takes from the game's polygon under it:
/// those about what's there, which collision doesn't say.
const INHERITED: u64 = poly_flags::PAVEMENT
    | poly_flags::ROAD
    | poly_flags::BOARDWALK
    | poly_flags::MUDDY
    | poly_flags::VEGETATION
    | poly_flags::WATER
    | poly_flags::SHELTERED
    | 0x400 // Sheltered2
    | 0x100_0000 // Sheltered3
    | 0x2_0000_0000 // Shelter4
    | poly_flags::DANGER_KEEP_AWAY_1
    | poly_flags::DANGER_KEEP_AWAY_2
    | poly_flags::NETWORK_SPAWN_CANDIDATE
    | poly_flags::NEAR_CAR_NODE
    | poly_flags::ISOLATED
    | poly_flags::STAIRS;

/// What making navmesh in a box did.
#[derive(Clone, Debug, Default)]
pub struct Generated {
    /// The navmesh's file name.
    pub navmesh: String,
    /// Collision triangles under the box, and polygons made from them.
    pub triangles: usize,
    pub stitched: Stitched,
    /// Special links of the next cells into this one, put on its new
    /// polygons; and those that found none there (left as they were).
    pub neighbour_links: usize,
    pub neighbour_links_lost: usize,
}

/// The navmesh cell a point is in, and its file name.
pub fn cell_of(p: Vec2) -> ((i32, i32), String) {
    let c = ((p - Vec2::splat(GRID_MIN)) / GRID_CELL).floor();
    let (x, y) = (c.x as i32, c.y as i32);
    ((x, y), format!("navmesh[{}][{}]", x * 3, y * 3))
}

/// A navmesh's polygons on the CPU.
struct Flat {
    corners: Vec<Vec<Vec3>>,
    polygons: Vec<NavmeshPolygon>,
}

impl Flat {
    fn new(nm: &Navmesh) -> Self {
        let (v, i, p) = (
            nm.vertex_positions().unwrap_or_default(),
            nm.indices().unwrap_or_default(),
            nm.polygons().unwrap_or_default(),
        );
        let corners = p
            .iter()
            .map(|q| (0..q.index_count() as usize).map(|k| Vec3::from(q.vertex(k, &v, &i))).collect())
            .collect();
        Self { corners, polygons: p }
    }

    /// The polygon under a point, nearest its height (2 m at most).
    fn under(&self, p: Vec3) -> Option<usize> {
        let mut best: Option<(f32, usize)> = None;
        for (i, c) in self.corners.iter().enumerate() {
            if c.len() < 3 || !inside(c, p.truncate()) {
                continue;
            }
            let z = c.iter().map(|v| v.z).sum::<f32>() / c.len() as f32;
            let d = (z - p.z).abs();
            if d < 2.0 && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, i));
            }
        }
        best.map(|(_, i)| i)
    }

    /// The polygon under a point, or the nearest within a metre.
    fn nearest(&self, p: Vec3) -> Option<usize> {
        stitch::nearest_polygon(self.corners.iter().map(|c| c.as_slice()), p)
    }
}

fn inside(c: &[Vec3], p: Vec2) -> bool {
    let n = c.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (c[i], c[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
    }
    inside
}

/// Walkable triangles by 2 m squares, for the ground's height.
struct Ground<'a> {
    triangles: &'a [Triangle],
    min: Vec2,
    w: usize,
    cells: Vec<Vec<u32>>,
}

impl<'a> Ground<'a> {
    const SIZE: f32 = 2.0;

    fn new(triangles: &'a [Triangle], min: Vec2, max: Vec2, slope_cos: f32) -> Self {
        let w = ((max.x - min.x) / Self::SIZE).ceil() as usize + 1;
        let h = ((max.y - min.y) / Self::SIZE).ceil() as usize + 1;
        let mut cells = vec![Vec::new(); w * h];
        for (i, t) in triangles.iter().enumerate() {
            if t.normal().z < slope_cos * 0.5 {
                continue;
            }
            let (lo, hi) =
                t.v.iter().fold((Vec2::MAX, Vec2::MIN), |(l, h), v| (l.min(v.truncate()), h.max(v.truncate())));
            let (lo, hi) = (((lo - min) / Self::SIZE).floor(), ((hi - min) / Self::SIZE).floor());
            for y in (lo.y.max(0.0) as usize)..=(hi.y.max(0.0) as usize).min(h - 1) {
                for x in (lo.x.max(0.0) as usize)..=(hi.x.max(0.0) as usize).min(w - 1) {
                    cells[y * w + x].push(i as u32);
                }
            }
        }
        Self { triangles, min, w, cells }
    }

    fn near(&self, p: Vec2) -> impl Iterator<Item = &Triangle> {
        let q = ((p - self.min) / Self::SIZE).floor();
        let cell = (q.x >= 0.0 && q.y >= 0.0).then(|| self.cells.get(q.y as usize * self.w + q.x as usize)).flatten();
        cell.into_iter().flatten().map(|&i| &self.triangles[i as usize])
    }

    /// The ground nearest `p`'s height under or over it (within `reach`), and
    /// the triangle it's on.
    fn at(&self, p: Vec3, reach: f32) -> Option<(f32, &Triangle)> {
        let mut best: Option<(f32, &Triangle)> = None;
        for t in self.near(p.truncate()) {
            if let Some(z) = height_in(t, p.truncate())
                && (z - p.z).abs() <= reach
                && best.is_none_or(|(b, _)| (z - p.z).abs() < (b - p.z).abs())
            {
                best = Some((z, t));
            }
        }
        best
    }
}

fn height_in(t: &Triangle, at: Vec2) -> Option<f32> {
    let [a, b, c] = t.v;
    let (v0, v1, v2) = ((b - a).truncate(), (c - a).truncate(), at - a.truncate());
    let d = v0.perp_dot(v1);
    if d.abs() < 1e-9 {
        return None;
    }
    let (u, v) = (v2.perp_dot(v1) / d, v0.perp_dot(v2) / d);
    const E: f32 = -1e-4;
    (u >= E && v >= E && u + v <= 1.0 - E).then_some(a.z + u * (b.z - a.z) + v * (c.z - a.z))
}

/// A polygon's slope from the horizontal, in degrees.
fn slope(c: &[Vec3]) -> f32 {
    // Newell's normal, from the first corner (far from the origin, sums of
    // coordinates are imprecise).
    let Some(&o) = c.first() else { return 0.0 };
    let mut n = Vec3::ZERO;
    for i in 0..c.len() {
        let (a, b) = (c[i] - o, c[(i + 1) % c.len()] - o);
        n += Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
    }
    let n = n.normalize_or(Vec3::Z);
    n.z.abs().clamp(0.0, 1.0).acos().to_degrees()
}

/// Whether an archetype opens (doors and gates): its collision isn't a wall.
fn opens(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("door") || n.contains("gate")
}

impl Editor {
    /// The collision triangles in a box: the world's static bounds, the
    /// interiors' and the entities' models' (doors and gates left out).
    pub fn collision_in(&self, area: collision::WorldBox) -> Vec<Triangle> {
        let data = self.data.clone();
        let interiors: Vec<(u32, Mat4)> = self
            .world
            .map_helpers()
            .iter()
            .flat_map(|(_, h)| h.interiors.iter().map(|i| (i.archetype, Mat4::from_cols_array(&i.transform))))
            .collect();
        let mut out = Vec::new();
        let files = collision::files_in(&data, area, &interiors);
        log::debug!(
            "navgen: {} interiors around ({:?}), {} ybns",
            interiors.len(),
            interiors
                .iter()
                .map(|(a, _)| (self.hashes.name(*a), data.interiors.get(a).map(|i| i.bounds.clone())))
                .collect::<Vec<_>>(),
            files.len()
        );
        for f in files {
            let before = out.len();
            collision::read_triangles(&self.fs, f, area, &mut out);
            log::debug!("navgen: {} {} triangles", self.fs.path(f.file), out.len() - before);
        }
        let mut cache = HashMap::new();
        for e in self.world.visible() {
            let place = Mat4::from_cols_array(&e.transform);
            let p = place.w_axis.truncate();
            let near = p.x > area.0.x - 50.0 && p.x < area.1.x + 50.0 && p.y > area.0.y - 50.0 && p.y < area.1.y + 50.0;
            if !near || opens(&self.hashes.name(e.archetype)) {
                continue;
            }
            if let Some(b) = collision::model_bounds(&self.fs, &self.hashes, &data, e.archetype, &mut cache) {
                let before = out.len();
                collision::triangles(&b, place, area, &mut out);
                if out.len() > before {
                    log::debug!("navgen: {} {} triangles", self.hashes.name(e.archetype), out.len() - before);
                }
            }
        }
        out
    }

    /// Makes a region of a navmesh cell again from the collision under it
    /// (see [`stitch`]): one undo step, with the next cells' links into it.
    pub fn generate_navmesh(&mut self, region: Region, s: &Settings) -> Result<Generated, String> {
        let centre = (region.min + region.max) * 0.5;
        let (cell, file_name) = cell_of(centre);
        let name = joaat(&file_name);
        self.open_navmesh(name).map_err(|e| format!("{file_name}: {e}"))?;
        let old = self.navmeshes[&name].root.clone();
        let (cmin, cmax) = (Vec2::new(old.aabb_min[0], old.aabb_min[1]), Vec2::new(old.aabb_max[0], old.aabb_max[1]));
        if region.min.x < cmin.x + CELL_MARGIN
            || region.min.y < cmin.y + CELL_MARGIN
            || region.max.x > cmax.x - CELL_MARGIN
            || region.max.y > cmax.y - CELL_MARGIN
        {
            return Err(format!(
                "keep the box inside one navmesh cell, {CELL_MARGIN} m from its sides: {file_name} is {:.0}, {:.0} to {:.0}, {:.0}",
                cmin.x, cmin.y, cmax.x, cmax.y
            ));
        }

        // The collision around the box, wide enough for the edges inside it.
        let pad = s.agent_radius + 4.0 * s.cell_size;
        let area = (
            Vec3::new(region.min.x - pad, region.min.y - pad, region.z_min),
            Vec3::new(region.max.x + pad, region.max.y + pad, region.z_max),
        );
        let triangles = self.collision_in(area);
        let made = super::generate(&triangles, area, s, &|t| {
            if t.material.data2.flags() & FLAG_STAIRS != 0 { STAIRS_AREA } else { 1 }
        })?;

        // Inside the box, cut by the game's 4 m grid, on the ground.
        let slope_cos = s.max_slope.to_radians().cos();
        let ground = Ground::new(&triangles, area.0.truncate(), area.1.truncate(), slope_cos);
        let reach = s.max_climb + 2.0 * s.cell_height;
        let before = Flat::new(&old);
        let interiors = self.interior_rooms();
        let mut added = Vec::new();
        for p in &made {
            let part = stitch::inside_part(&p.corners, &region);
            if part.len() < 3 {
                continue;
            }
            for mut corners in stitch::split_by_grid(&part, cmin, GRID_SPLIT) {
                for c in &mut corners {
                    if let Some((z, _)) = ground.at(*c, reach) {
                        c.z = z;
                    }
                }
                let attributes = attributes(&corners, p.area, &before, &ground, &interiors);
                added.push(NewPolygon { corners, attributes });
            }
        }
        let (mut nm, stitched) = stitch::replace_region(&old, &region, added)?;
        nm.build_id = old.build_id;

        // The next cells' links into this one.
        let mut roots = vec![(DocKey::Navmesh(name), Root::Navmesh(Arc::new(nm.clone())))];
        let (mut moved, mut lost) = (0, 0);
        let after = Flat::new(&nm);
        for dx in -1..=1 {
            for dy in -1..=1 {
                if (dx, dy) == (0, 0) {
                    continue;
                }
                let other_name = format!("navmesh[{}][{}]", (cell.0 + dx) * 3, (cell.1 + dy) * 3);
                let other = joaat(&other_name);
                if self.data.stream_entry("ynv", other).is_none() && !self.navmeshes.contains_key(&other) {
                    continue;
                }
                if self.open_navmesh(other).is_err() {
                    continue;
                }
                let mut o = (*self.navmeshes[&other].root).clone();
                let Some(links) = o.special_links.as_mut() else { continue };
                let mut changed = false;
                for l in links.iter_mut() {
                    for end in [0, 1] {
                        let (area_id, poly, pos) = if end == 0 {
                            (l.area_id1, &mut l.poly_id1, &mut l.position1)
                        } else {
                            (l.area_id2, &mut l.poly_id2, &mut l.position2)
                        };
                        if area_id as u32 != old.area_id {
                            continue;
                        }
                        let world = Vec3::from(pos.to_vec3(old.aabb_min, old.extents3()));
                        let now = match stitched.kept.get(&(*poly as u32)) {
                            Some(&k) => Some(k as u16),
                            None => after.nearest(world).map(|i| i as u16),
                        };
                        match now {
                            Some(p) => {
                                if p != *poly {
                                    moved += 1;
                                }
                                *poly = p;
                            }
                            None => lost += 1,
                        }
                        *pos = NavmeshVertex::new(world.to_array(), nm.aabb_min, nm.extents3());
                        changed = true;
                    }
                }
                for l in links.iter_mut() {
                    (l.poly_id1a, l.poly_id2a) = (l.poly_id1, l.poly_id2);
                }
                if changed {
                    roots.push((DocKey::Navmesh(other), Root::Navmesh(Arc::new(o))));
                }
            }
        }
        self.commit_roots("Make navmesh", roots);
        Ok(Generated {
            navmesh: file_name,
            triangles: triangles.len(),
            stitched,
            neighbour_links: moved,
            neighbour_links_lost: lost,
        })
    }

    /// The rooms of the interiors placed around (their boxes in the world).
    fn interior_rooms(&self) -> Vec<(Vec3, Vec3)> {
        let mut out = Vec::new();
        for (_, h) in self.world.map_helpers() {
            for i in &h.interiors {
                let m = Mat4::from_cols_array(&i.transform);
                for r in &i.rooms {
                    if r.name.eq_ignore_ascii_case("limbo") {
                        continue;
                    }
                    let (rmin, rmax) = (Vec3::from(r.min), Vec3::from(r.max));
                    let (lo, hi) = (0..8).fold((Vec3::MAX, Vec3::MIN), |(lo, hi), k| {
                        let c = Vec3::new(
                            if k & 1 != 0 { rmax.x } else { rmin.x },
                            if k & 2 != 0 { rmax.y } else { rmin.y },
                            if k & 4 != 0 { rmax.z } else { rmin.z },
                        );
                        let p = m.transform_point3(c);
                        (lo.min(p), hi.max(p))
                    });
                    out.push((lo, hi));
                }
            }
        }
        out
    }
}

/// A generated polygon's attributes: what the game's polygon under it had
/// that collision doesn't say, and what its shape and collision do.
fn attributes(corners: &[Vec3], area: u8, before: &Flat, ground: &Ground, rooms: &[(Vec3, Vec3)]) -> NavmeshPolygon {
    let centre = corners.iter().sum::<Vec3>() / corners.len() as f32;
    let mut p = NavmeshPolygon::default();
    let mut flags = 0u64;
    if let Some(i) = before.under(centre) {
        let old = &before.polygons[i];
        flags = old.flags() & INHERITED;
        p.set_audio_properties(old.audio_properties());
        p.set_water_depth(old.water_depth());
        p.set_u30a(old.u30a());
    }
    let stairs =
        area == STAIRS_AREA || ground.at(centre, 1.0).is_some_and(|(_, t)| t.material.data2.flags() & FLAG_STAIRS != 0);
    if stairs {
        flags |= poly_flags::STAIRS;
    }
    if signed_area(corners) < SMALL_AREA {
        flags |= poly_flags::SMALL;
    }
    if flags & poly_flags::STAIRS == 0 && slope(corners) > STEEP {
        flags |= poly_flags::STEEP_SLOPE;
    }
    if rooms.iter().any(|(lo, hi)| centre.cmpge(*lo).all() && centre.cmple(*hi).all()) {
        flags |= poly_flags::INTERIOR;
        flags &= !(poly_flags::PAVEMENT | poly_flags::ROAD | poly_flags::VEGETATION);
    }
    p.set_flags(flags);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells() {
        assert_eq!(cell_of(Vec2::new(-280.0, 790.0)), ((104, 111), "navmesh[312][333]".into()));
        assert_eq!(cell_of(Vec2::new(-300.5, 790.0)).1, "navmesh[309][333]");
    }

    #[test]
    fn slopes() {
        let flat = [Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0)];
        assert!(slope(&flat) < 0.01);
        let steep = [Vec3::ZERO, Vec3::new(1.0, 0.0, 1.0), Vec3::new(1.0, 1.0, 1.0), Vec3::new(0.0, 1.0, 0.0)];
        assert!((slope(&steep) - 45.0).abs() < 0.01);
        assert!(opens("p_door_val_bank02x") && opens("P_GATE01X") && !opens("p_fencewood01x"));
    }
}
