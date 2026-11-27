//! Putting generated polygons in a navmesh in place of what was in a box.
//!
//! - The navmesh's polygons inside the box go; those crossing its sides are
//!   cut there, into convex pieces (left of it, right of it, below and above
//!   it between the two); the others stay as they are, with their edges.
//! - Along the box's sides every polygon gets every corner the others have
//!   there, so the pieces and the generated polygons share their vertices,
//!   and edges join where two polygons have the same two corners (by their
//!   position seen from above, a step's height apart at most). A piece of a
//!   blocked-off polygon (switched off for peds) is joined only to blocked
//!   ones, so a blocked area stays closed.
//! - Special links keep their polygons; an end on a polygon that went is put
//!   on the polygon now under it, or the link goes.
//! - Then the lists, the quadtree, the polygons' boxes and centroids and the
//!   edges' free space are made again, as the game's tools have them (see
//!   `docs/HELPER_EDITING.md`).

use std::collections::{HashMap, HashSet};

use glam::{Vec2, Vec3};
use rf_formats::formats::ynv::{
    Navmesh, NavmeshBuilder, NavmeshEdge, NavmeshPolygon, NavmeshSpecialLink, NavmeshVertex, poly_flags,
};

use super::signed_area;

/// A box seen from above, with the heights it covers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    pub min: Vec2,
    pub max: Vec2,
    pub z_min: f32,
    pub z_max: f32,
}

impl Region {
    fn holds(&self, p: Vec2) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }
}

/// A polygon to put in: its corners (counter-clockwise from above) and its
/// attributes (flags, audio...; its lists' fields are made again).
#[derive(Clone, Debug)]
pub struct NewPolygon {
    pub corners: Vec<Vec3>,
    pub attributes: NavmeshPolygon,
}

/// What happened to the navmesh's polygons.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stitched {
    /// Each old polygon's new index, if it stayed whole.
    pub kept: HashMap<u32, u32>,
    pub removed: usize,
    pub pieces: usize,
    pub added: usize,
    /// Special links put on another polygon, and those that went.
    pub links_moved: usize,
    pub links_dropped: usize,
}

const NO_POLYGON: u32 = 0x7FFF;
const NO_AREA: u32 = 0xFFFF;
/// Most corners a polygon gets (the game's have up to 10).
const MAX_CORNERS: usize = 10;
/// How far apart two corners' heights may be and still join.
const JOIN_HEIGHT: f32 = 0.75;
/// The edges' free space is in these steps (see the study in #6).
const SPACE_STEP: f32 = 0.24;

#[derive(Clone, Debug)]
enum Origin {
    /// An old polygon whole, with its old index.
    Kept(u32),
    /// A piece of an old polygon.
    Piece,
    New,
}

#[derive(Clone, Debug)]
struct Poly {
    corners: Vec<Vec3>,
    attributes: NavmeshPolygon,
    origin: Origin,
    /// A kept polygon's edges as they were.
    old_edges: Vec<NavmeshEdge>,
}

impl Poly {
    fn blocked(&self) -> bool {
        self.attributes.flags() & poly_flags::SWITCHED_OFF_FOR_PEDS != 0
    }
}

/// Clips a convex polygon to the side of a line where `inside` holds
/// (Sutherland–Hodgman); `cross` gives where an edge meets the line.
fn clip(c: &[Vec3], inside: impl Fn(Vec3) -> bool, cross: impl Fn(Vec3, Vec3) -> Vec3) -> Vec<Vec3> {
    let mut out = Vec::new();
    for i in 0..c.len() {
        let (a, b) = (c[i], c[(i + 1) % c.len()]);
        match (inside(a), inside(b)) {
            (true, true) => out.push(b),
            (true, false) => out.push(cross(a, b)),
            (false, true) => {
                out.push(cross(a, b));
                out.push(b);
            }
            (false, false) => {}
        }
    }
    out
}

/// Where the segment a-b meets x = `x` (or y = `x` with `y_axis`).
fn at(a: Vec3, b: Vec3, x: f32, y_axis: bool) -> Vec3 {
    let (ca, cb) = if y_axis { (a.y, b.y) } else { (a.x, b.x) };
    let t = if (cb - ca).abs() < 1e-9 { 0.0 } else { (x - ca) / (cb - ca) };
    let mut p = a + (b - a) * t;
    // Exactly on the line, so that every polygon has the same coordinate.
    if y_axis {
        p.y = x;
    } else {
        p.x = x;
    }
    p
}

/// The part of a convex polygon with x <= `x` (`below`) or x >= `x`, or the
/// same along y.
pub fn clip_axis(c: &[Vec3], x: f32, y_axis: bool, below: bool) -> Vec<Vec3> {
    let coord = move |p: Vec3| if y_axis { p.y } else { p.x };
    clip(c, move |p| if below { coord(p) <= x } else { coord(p) >= x }, move |a, b| at(a, b, x, y_axis))
}

/// A convex polygon cut by a grid of `size` from `origin`, into its convex
/// pieces.
pub fn split_by_grid(c: &[Vec3], origin: Vec2, size: f32) -> Vec<Vec<Vec3>> {
    let (lo, hi) = c.iter().fold((Vec2::MAX, Vec2::MIN), |(l, h), v| (l.min(v.truncate()), h.max(v.truncate())));
    let first = |o: f32, v: f32| ((v - o) / size).floor() as i64;
    let mut out = Vec::new();
    for gx in first(origin.x, lo.x)..=first(origin.x, hi.x) {
        let (x0, x1) = (origin.x + gx as f32 * size, origin.x + (gx + 1) as f32 * size);
        let column = clip_axis(&clip_axis(c, x0, false, false), x1, false, true);
        if column.len() < 3 {
            continue;
        }
        for gy in first(origin.y, lo.y)..=first(origin.y, hi.y) {
            let (y0, y1) = (origin.y + gy as f32 * size, origin.y + (gy + 1) as f32 * size);
            let cell = clip_axis(&clip_axis(&column, y0, true, false), y1, true, true);
            let cell = dedup(cell);
            if cell.len() >= 3 && signed_area(&cell) > 1e-3 {
                out.push(cell);
            }
        }
    }
    out
}

/// Corners without the ones repeated one after the other.
fn dedup(mut c: Vec<Vec3>) -> Vec<Vec3> {
    c.dedup_by(|a, b| a.truncate().distance_squared(b.truncate()) < 1e-8);
    while c.len() > 1 && c[0].truncate().distance_squared(c[c.len() - 1].truncate()) < 1e-8 {
        c.pop();
    }
    c
}

/// The pieces of a convex polygon outside a region: left of it, right of it,
/// and below and above it between the two.
fn outside_pieces(c: &[Vec3], r: &Region) -> Vec<Vec<Vec3>> {
    let left = clip_axis(c, r.min.x, false, true);
    let right = clip_axis(c, r.max.x, false, false);
    let middle = clip_axis(&clip_axis(c, r.min.x, false, false), r.max.x, false, true);
    let below = clip_axis(&middle, r.min.y, true, true);
    let above = clip_axis(&middle, r.max.y, true, false);
    [left, right, below, above].into_iter().map(dedup).filter(|p| p.len() >= 3 && signed_area(p) > 1e-3).collect()
}

/// The part of a convex polygon inside a region.
pub fn inside_part(c: &[Vec3], r: &Region) -> Vec<Vec3> {
    let p = clip_axis(&clip_axis(c, r.min.x, false, false), r.max.x, false, true);
    dedup(clip_axis(&clip_axis(&p, r.min.y, true, false), r.max.y, true, true))
}

/// A position seen from above, to the centimetre, as a key.
fn key(p: Vec3) -> (i64, i64) {
    ((p.x * 100.0).round() as i64, (p.y * 100.0).round() as i64)
}

/// Whether a corner is on one of the region's sides.
fn on_side(p: Vec3, r: &Region) -> bool {
    const E: f32 = 1e-3;
    let within_x = p.x >= r.min.x - E && p.x <= r.max.x + E;
    let within_y = p.y >= r.min.y - E && p.y <= r.max.y + E;
    ((p.x - r.min.x).abs() < E || (p.x - r.max.x).abs() < E) && within_y
        || ((p.y - r.min.y).abs() < E || (p.y - r.max.y).abs() < E) && within_x
}

/// The point-in-polygon test seen from above.
fn holds(c: &[Vec3], p: Vec2) -> bool {
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

/// How far from a polygon a point counts as on it (a link's end at a ledge,
/// with the navmesh now a little back from it).
pub const NEAR: f32 = 1.0;

/// How far a point is from a polygon seen from above (0 inside it).
pub fn distance_to(c: &[Vec3], p: Vec2) -> f32 {
    if holds(c, p) {
        return 0.0;
    }
    (0..c.len())
        .map(|i| {
            let (a, b) = (c[i].truncate(), c[(i + 1) % c.len()].truncate());
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
            (a + ab * t - p).length()
        })
        .fold(f32::MAX, f32::min)
}

/// The polygon under a point, or the nearest within [`NEAR`], nearest its
/// height (2 m at most).
pub fn nearest_polygon<'a>(polys: impl Iterator<Item = &'a [Vec3]>, p: Vec3) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for (i, c) in polys.enumerate() {
        if c.len() < 3 {
            continue;
        }
        let d2 = distance_to(c, p.truncate());
        if d2 > NEAR {
            continue;
        }
        let z = c.iter().map(|v| v.z).sum::<f32>() / c.len() as f32;
        let dz = (z - p.z).abs();
        let d = d2 * 4.0 + dz;
        if dz < 2.0 && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| i)
}

/// The polygon of `polys` under a point (see [`nearest_polygon`]).
fn polygon_under(polys: &[Poly], p: Vec3) -> Option<usize> {
    nearest_polygon(polys.iter().map(|q| q.corners.as_slice()), p)
}

/// `centroid_x` and `centroid_y` as the game has them: where the corners'
/// middle is in the polygon's box (in half metres), in 256ths.
fn centroid(p: &NavmeshPolygon, corners: &[Vec3]) -> (u8, u8) {
    let c = corners.iter().sum::<Vec3>() / corners.len().max(1) as f32;
    let (min, max) = (p.cell_aabb.min(), p.cell_aabb.max());
    let f = |v: f32, lo: f32, hi: f32| (((v - lo) / (hi - lo).max(1e-6)) * 256.0).floor().clamp(0.0, 255.0) as u8;
    (f(c.x, min[0], max[0]), f(c.y, min[1], max[1]))
}

/// Puts `added` in `nm` in place of what's in `r`. The region must keep
/// clear of the navmesh's sides (its polygons there join the next cell).
pub fn replace_region(nm: &Navmesh, r: &Region, added: Vec<NewPolygon>) -> Result<(Navmesh, Stitched), String> {
    let (Some(verts), Some(inds), Some(polys), Some(edges)) =
        (nm.vertex_positions(), nm.indices(), nm.polygons(), nm.edges())
    else {
        return Err("the navmesh has no polygons".into());
    };
    let n_areas = (nm.adj_area_ids_count as usize).min(32);
    let own = nm.adj_area_ids[..n_areas].iter().position(|&a| a == nm.area_id).map(|i| i as u32);
    let mut report = Stitched::default();

    // The old polygons: kept whole, cut, or gone.
    let mut out: Vec<Poly> = Vec::new();
    let mut gone: HashSet<u32> = HashSet::new();
    for (i, p) in polys.iter().enumerate() {
        let i = i as u32;
        let n = p.index_count() as usize;
        let corners: Vec<Vec3> = (0..n).map(|k| Vec3::from(p.vertex(k, &verts, &inds))).collect();
        let old_edges: Vec<NavmeshEdge> =
            (0..n).map(|k| edges.get(p.index_id as usize + k).copied().unwrap_or_default()).collect();
        let (zlo, zhi) = corners.iter().fold((f32::MAX, f32::MIN), |(l, h), c| (l.min(c.z), h.max(c.z)));
        let in_heights = zhi >= r.z_min && zlo <= r.z_max;
        let inside = corners.iter().filter(|c| r.holds(c.truncate())).count();
        let touches = n >= 3
            && in_heights
            && (inside > 0 || inside_part(&corners, r).len() >= 3 || corners.iter().any(|c| on_side(*c, r)));
        if n < 3 || !touches {
            if n < 3 && in_heights && inside > 0 {
                return Err("the box reaches the navmesh's side: keep it inside the cell".into());
            }
            out.push(Poly { corners, attributes: *p, origin: Origin::Kept(i), old_edges });
            continue;
        }
        gone.insert(i);
        if inside == n {
            report.removed += 1;
            continue;
        }
        for piece in outside_pieces(&corners, r) {
            report.pieces += 1;
            out.push(Poly { corners: piece, attributes: *p, origin: Origin::Piece, old_edges: Vec::new() });
        }
    }
    for a in added {
        report.added += 1;
        out.push(Poly { corners: a.corners, attributes: a.attributes, origin: Origin::New, old_edges: Vec::new() });
    }

    // Along the region's sides, every polygon gets every corner there. The
    // old pieces' heights win: the navmesh around stays as it was.
    let mut side_points: HashMap<(i64, i64), Vec3> = HashMap::new();
    for poly in out.iter().filter(|p| !matches!(p.origin, Origin::Kept(_))) {
        for c in &poly.corners {
            if on_side(*c, r) {
                let old = matches!(poly.origin, Origin::Piece);
                side_points
                    .entry(key(*c))
                    .and_modify(|e| {
                        if old {
                            *e = *c
                        }
                    })
                    .or_insert(*c);
            }
        }
    }
    let points: Vec<Vec3> = side_points.values().copied().collect();
    for poly in out.iter_mut().filter(|p| !matches!(p.origin, Origin::Kept(_))) {
        let c = &poly.corners;
        let mut with = Vec::with_capacity(c.len());
        for i in 0..c.len() {
            let (a, b) = (c[i], c[(i + 1) % c.len()]);
            with.push(side_points.get(&key(a)).copied().unwrap_or(a));
            if !(on_side(a, r) && on_side(b, r)) {
                continue;
            }
            // The points strictly between a and b on their line, in order.
            let ab = b.truncate() - a.truncate();
            let len2 = ab.length_squared();
            if len2 < 1e-8 {
                continue;
            }
            let mut between: Vec<(f32, Vec3)> = points
                .iter()
                .filter_map(|&p| {
                    let t = (p.truncate() - a.truncate()).dot(ab) / len2;
                    let off = (a.truncate() + ab * t - p.truncate()).length();
                    (t > 1e-4 && t < 1.0 - 1e-4 && off < 1e-3).then_some((t, p))
                })
                .collect();
            between.sort_by(|x, y| x.0.total_cmp(&y.0));
            with.extend(between.into_iter().map(|(_, p)| p));
        }
        poly.corners = dedup(with);
    }
    // Too many corners for the game: halved, along a diagonal.
    let mut i = 0;
    while i < out.len() {
        if out[i].corners.len() > MAX_CORNERS && !matches!(out[i].origin, Origin::Kept(_)) {
            let c = out[i].corners.clone();
            let half = c.len() / 2;
            let first: Vec<Vec3> = c[..=half].to_vec();
            let mut second: Vec<Vec3> = c[half..].to_vec();
            second.push(c[0]);
            out[i].corners = first;
            let mut other = out[i].clone();
            other.corners = second;
            out.push(other);
            continue;
        }
        i += 1;
    }

    // The new indices of the old polygons kept whole.
    for (n, p) in out.iter().enumerate() {
        if let Origin::Kept(o) = p.origin {
            report.kept.insert(o, n as u32);
        }
    }

    // Edges: a kept polygon's to another kept one stay; the rest join where
    // two polygons have the same corners (reversed), seen from above.
    // By each edge's corners seen from above: the polygons and corners.
    type EdgeKey = ((i64, i64), (i64, i64));
    let mut by_edge: HashMap<EdgeKey, Vec<(usize, usize)>> = HashMap::new();
    for (pi, p) in out.iter().enumerate() {
        let n = p.corners.len();
        for k in 0..n {
            by_edge.entry((key(p.corners[k]), key(p.corners[(k + 1) % n]))).or_default().push((pi, k));
        }
    }
    let mut neighbours: Vec<Vec<Option<usize>>> = out.iter().map(|p| vec![None; p.corners.len()]).collect();
    for (pi, p) in out.iter().enumerate() {
        let n = p.corners.len();
        #[allow(clippy::needless_range_loop)]
        for k in 0..n {
            if let Origin::Kept(_) = p.origin {
                let e = p.old_edges[k];
                let across = (Some(e.area_id1()) == own && e.poly_id1() != NO_POLYGON).then(|| e.poly_id1());
                match across {
                    Some(q) if !gone.contains(&q) => {
                        neighbours[pi][k] = report.kept.get(&q).map(|&x| x as usize);
                        continue;
                    }
                    // Cut before (blocked, a wall...): stays so.
                    None => continue,
                    Some(_) => {}
                }
            }
            let (a, b) = (p.corners[k], p.corners[(k + 1) % n]);
            let found = by_edge.get(&(key(b), key(a))).and_then(|list| {
                list.iter().copied().find(|&(qi, qk)| {
                    let q = &out[qi];
                    let (qa, qb) = (q.corners[qk], q.corners[(qk + 1) % q.corners.len()]);
                    qi != pi
                        && (qa.z - b.z).abs() <= JOIN_HEIGHT
                        && (qb.z - a.z).abs() <= JOIN_HEIGHT
                        && q.blocked() == p.blocked()
                })
            });
            neighbours[pi][k] = found.map(|(qi, _)| qi);
        }
    }

    // Only joins both polygons have stay: a kept polygon's edge closed on
    // purpose (a wall, a blocked area) stays closed.
    for pi in 0..out.len() {
        for k in 0..neighbours[pi].len() {
            if let Some(q) = neighbours[pi][k]
                && !neighbours[q].contains(&Some(pi))
            {
                neighbours[pi][k] = None;
            }
        }
    }

    // Special links: their polygons now, or the ones under their ends.
    let (min, size) = (nm.aabb_min, nm.extents3());
    let old_links = nm.special_links.clone().unwrap_or_default();
    let old_inds = nm.special_links_inds.clone().unwrap_or_default();
    let mut listed: HashSet<u16> = HashSet::new();
    for p in &polys {
        let s = p.link_id() as usize;
        listed.extend(old_inds.get(s..s + p.link_count() as usize).unwrap_or_default());
    }
    let mut links: Vec<NavmeshSpecialLink> = Vec::new();
    let mut new_index: HashMap<u16, u16> = HashMap::new();
    let mut lists: Vec<Vec<u16>> = vec![Vec::new(); out.len()];
    let place = |poly: u16, pos: NavmeshVertex, report: &mut Stitched| -> Option<u16> {
        if !gone.contains(&(poly as u32)) {
            return report.kept.get(&(poly as u32)).map(|&x| x as u16);
        }
        let p = Vec3::from(pos.to_vec3(min, size));
        let found = polygon_under(&out, p).map(|x| x as u16);
        if found.is_some() {
            report.links_moved += 1;
        }
        found
    };
    for (li, l) in old_links.iter().enumerate() {
        let mut l = *l;
        let mut ok = true;
        if l.area_id1 as u32 == nm.area_id {
            match place(l.poly_id1, l.position1, &mut report) {
                Some(p) => (l.poly_id1, l.poly_id1a) = (p, p),
                None => ok = false,
            }
        }
        if ok && l.area_id2 as u32 == nm.area_id {
            match place(l.poly_id2, l.position2, &mut report) {
                Some(p) => (l.poly_id2, l.poly_id2a) = (p, p),
                None => ok = false,
            }
        }
        if !ok {
            report.links_dropped += 1;
            continue;
        }
        let ni = links.len() as u16;
        new_index.insert(li as u16, ni);
        if listed.contains(&(li as u16)) {
            for (area, poly) in [(l.area_id1, l.poly_id1), (l.area_id2, l.poly_id2)] {
                if area as u32 == nm.area_id && !lists[poly as usize].contains(&ni) {
                    lists[poly as usize].push(ni);
                }
            }
        }
        links.push(l);
    }
    // A kept polygon keeps its own list's order.
    for (pi, p) in out.iter().enumerate() {
        if let Origin::Kept(o) = p.origin {
            let old = &polys[o as usize];
            let s = old.link_id() as usize;
            let mut own_list: Vec<u16> = old_inds
                .get(s..s + old.link_count() as usize)
                .unwrap_or_default()
                .iter()
                .filter_map(|l| new_index.get(l).copied())
                .collect();
            for l in &lists[pi] {
                if !own_list.contains(l) {
                    own_list.push(*l);
                }
            }
            lists[pi] = own_list;
        }
    }

    // The navmesh's box: the cell's, high enough for everything.
    let mut result = nm.clone();
    let (zlo, zhi) =
        out.iter().flat_map(|p| p.corners.iter()).fold((f32::MAX, f32::MIN), |(l, h), c| (l.min(c.z), h.max(c.z)));
    if zlo < result.aabb_min[2] || zhi > result.aabb_max[2] {
        result.aabb_min[2] = result.aabb_min[2].min(zlo);
        result.aabb_max[2] = result.aabb_max[2].max(zhi);
        result.extents[2] = result.aabb_max[2] - result.aabb_min[2];
        // The links' ends here are quantised in the box.
        for l in &mut links {
            for (area, pos) in [(l.area_id1, &mut l.position1), (l.area_id2, &mut l.position2)] {
                if area as u32 == nm.area_id {
                    let w = pos.to_vec3(min, size);
                    *pos = NavmeshVertex::new(w, result.aabb_min, result.extents3());
                }
            }
        }
    }
    let none = result.ensure_adj_area_id(NO_AREA);
    let own = own.unwrap_or_else(|| result.ensure_adj_area_id(nm.area_id));

    // The free space around each corner, for the polygons that changed and
    // those near them.
    let open: Vec<(Vec2, Vec2)> = out
        .iter()
        .enumerate()
        .flat_map(|(pi, p)| {
            let n = p.corners.len();
            let nb = &neighbours[pi];
            (0..n)
                .filter(move |&k| nb[k].is_none())
                .map(move |k| (p.corners[k].truncate(), p.corners[(k + 1) % n].truncate()))
        })
        .collect();
    let near = |p: Vec2| p.x > r.min.x - 8.0 && p.x < r.max.x + 8.0 && p.y > r.min.y - 8.0 && p.y < r.max.y + 8.0;
    let space = |p: Vec2| -> u32 {
        let d = open
            .iter()
            .filter(|(a, b)| a.distance(p) < 12.0 || b.distance(p) < 12.0)
            .map(|&(a, b)| {
                let ab = b - a;
                let t = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
                (a + ab * t - p).length()
            })
            .fold(f32::MAX, f32::min);
        if d == f32::MAX { 31 } else { (d / SPACE_STEP).round().min(31.0) as u32 }
    };

    let mut builder = NavmeshBuilder::new();
    let mut built: Vec<NavmeshPolygon> = Vec::with_capacity(out.len());
    for (pi, p) in out.iter().enumerate() {
        let n = p.corners.len();
        let edges: Vec<NavmeshEdge> = (0..n)
            .map(|k| {
                let mut e = match p.origin {
                    Origin::Kept(_) => p.old_edges[k],
                    _ => NavmeshEdge::default(),
                };
                match neighbours[pi][k] {
                    Some(q) => {
                        e.set_area_id1(own);
                        e.set_area_id2(own);
                        e.set_poly_id1(q as u32);
                        e.set_poly_id2(q as u32);
                    }
                    None => {
                        e.set_area_id1(none);
                        e.set_area_id2(none);
                        e.set_poly_id1(NO_POLYGON);
                        e.set_poly_id2(NO_POLYGON);
                    }
                }
                let changed = !matches!(p.origin, Origin::Kept(_)) || near(p.corners[k].truncate());
                if changed && n >= 3 {
                    e.set_space_around_vertex(space(p.corners[k].truncate()));
                }
                e
            })
            .collect();
        let mut poly = p.attributes;
        poly.set_area_id(nm.area_id as u16);
        let corners: Vec<[f32; 3]> = p.corners.iter().map(|c| c.to_array()).collect();
        let links = (!lists[pi].is_empty()).then_some(lists[pi].as_slice());
        builder.build_polygon(&mut poly, Some(&corners), &edges, links);
        if !matches!(p.origin, Origin::Kept(_)) {
            (poly.centroid_x, poly.centroid_y) = centroid(&poly, &p.corners);
        }
        built.push(poly);
    }
    result.special_links = (!links.is_empty()).then_some(links);
    result.special_links_count = result.special_links.as_ref().map_or(0, |l| l.len() as u32);
    result.special_links_inds = Some(std::mem::take(&mut builder.links_inds));
    result.rebuild_lists(&builder.vertices, &builder.indices, &builder.edges, Some(&built));
    result.rebuild_quad_tree(&built, &builder.vertices, &builder.indices);
    result.update_total_bytes();
    Ok((result, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f32, y: f32, s: f32, z: f32) -> Vec<Vec3> {
        vec![Vec3::new(x, y, z), Vec3::new(x + s, y, z), Vec3::new(x + s, y + s, z), Vec3::new(x, y + s, z)]
    }

    #[test]
    fn clipping_and_grids() {
        let c = square(0.0, 0.0, 10.0, 0.0);
        assert_eq!(clip_axis(&c, 4.0, false, true).len(), 4);
        assert!((signed_area(&clip_axis(&c, 4.0, false, true)) - 40.0).abs() < 1e-3);
        let cells = split_by_grid(&c, Vec2::ZERO, 4.0);
        assert_eq!(cells.len(), 9);
        let total: f32 = cells.iter().map(|p| signed_area(p)).sum();
        assert!((total - 100.0).abs() < 1e-3);
        let r = Region { min: Vec2::new(2.0, 2.0), max: Vec2::new(5.0, 5.0), z_min: -1.0, z_max: 1.0 };
        let pieces = outside_pieces(&c, &r);
        assert_eq!(pieces.len(), 4);
        let total: f32 = pieces.iter().map(|p| signed_area(p)).sum();
        assert!((total - 91.0).abs() < 1e-3);
        assert!((signed_area(&inside_part(&c, &r)) - 9.0).abs() < 1e-3);
    }

    /// A 12 m by 12 m navmesh of nine 4 m squares, each joined to the next.
    fn grid_navmesh() -> Navmesh {
        let mut nm = Navmesh { area_id: 5, ..Default::default() };
        nm.set_adj_area_ids_trimmed(Some(&[5, NO_AREA]));
        nm.aabb_min = [0.0, 0.0, -1.0];
        nm.aabb_max = [12.0, 12.0, 1.0];
        nm.extents = [12.0, 12.0, 2.0, 0.0];
        let mut b = NavmeshBuilder::new();
        let mut built = Vec::new();
        for j in 0..3 {
            for i in 0..3 {
                let c: Vec<[f32; 3]> =
                    square(i as f32 * 4.0, j as f32 * 4.0, 4.0, 0.0).iter().map(|v| v.to_array()).collect();
                let idx = |i: i32, j: i32| (0..3).contains(&i) && (0..3).contains(&j);
                let e = |di: i32, dj: i32| {
                    let mut e = NavmeshEdge::default();
                    if idx(i + di, j + dj) {
                        e.set_area_id1(0);
                        e.set_poly_id1(((j + dj) * 3 + i + di) as u32);
                    } else {
                        e.set_area_id1(1);
                        e.set_poly_id1(NO_POLYGON);
                    }
                    e.set_area_id2(e.area_id1());
                    e.set_poly_id2(e.poly_id1());
                    e
                };
                // Bottom, right, top, left.
                let edges = [e(0, -1), e(1, 0), e(0, 1), e(-1, 0)];
                let mut p = NavmeshPolygon::default();
                b.build_polygon(&mut p, Some(&c), &edges, None);
                built.push(p);
            }
        }
        nm.rebuild_lists(&b.vertices, &b.indices, &b.edges, Some(&built));
        nm.rebuild_quad_tree(&built, &b.vertices, &b.indices);
        nm
    }

    fn joined(nm: &Navmesh) -> usize {
        let (p, e) = (nm.polygons().unwrap(), nm.edges().unwrap());
        p.iter()
            .map(|p| {
                (0..p.index_count() as usize).filter(|k| e[p.index_id as usize + k].poly_id1() != NO_POLYGON).count()
            })
            .sum()
    }

    #[test]
    fn a_region_is_replaced_and_joined() {
        let nm = grid_navmesh();
        // The middle square's box, a little bigger: it goes, its neighbours
        // are cut, and two new squares fill it.
        let r = Region { min: Vec2::new(3.0, 3.0), max: Vec2::new(9.0, 9.0), z_min: -1.0, z_max: 1.0 };
        let added = vec![NewPolygon {
            corners: square(3.0, 3.0, 6.0, 0.0)[..].to_vec(),
            attributes: NavmeshPolygon::default(),
        }];
        let added = added.into_iter().flat_map(|a| {
            split_by_grid(&a.corners, Vec2::ZERO, 4.0)
                .into_iter()
                .map(move |c| NewPolygon { corners: c, attributes: a.attributes })
        });
        let (out, report) = replace_region(&nm, &r, added.collect()).unwrap();
        assert_eq!(report.removed, 1);
        // The eight around are each cut into the part outside.
        assert_eq!(report.kept.len(), 0);
        assert!(report.pieces >= 8);
        // Cut by the 4 m grid: 3 by 3.
        assert_eq!(report.added, 9);
        // Every polygon is joined to something, and the whole is connected.
        let (p, e) = (out.polygons().unwrap(), out.edges().unwrap());
        let mut seen = vec![false; p.len()];
        let mut stack = vec![0usize];
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            for k in 0..p[i].index_count() as usize {
                let q = e[p[i].index_id as usize + k].poly_id1();
                if q != NO_POLYGON {
                    stack.push(q as usize);
                }
            }
        }
        assert!(seen.iter().all(|s| *s), "not connected: {seen:?}");
        // Joins go both ways.
        for (i, poly) in p.iter().enumerate() {
            for k in 0..poly.index_count() as usize {
                let q = e[poly.index_id as usize + k].poly_id1();
                if q != NO_POLYGON {
                    let back = &p[q as usize];
                    assert!(
                        (0..back.index_count() as usize).any(|j| e[back.index_id as usize + j].poly_id1() == i as u32)
                    );
                }
            }
        }
        assert!(joined(&out) > joined(&nm) / 2);
        // The area is the same.
        let v = out.vertex_positions().unwrap();
        let idx = out.indices().unwrap();
        let area: f32 = p
            .iter()
            .map(|q| {
                signed_area(
                    &(0..q.index_count() as usize).map(|k| Vec3::from(q.vertex(k, &v, &idx))).collect::<Vec<_>>(),
                )
            })
            .sum();
        assert!((area - 144.0).abs() < 0.1, "{area}");
    }

    #[test]
    fn a_region_outside_keeps_everything() {
        let nm = grid_navmesh();
        let r = Region { min: Vec2::new(20.0, 20.0), max: Vec2::new(30.0, 30.0), z_min: -1.0, z_max: 1.0 };
        let (out, report) = replace_region(&nm, &r, Vec::new()).unwrap();
        assert_eq!((report.kept.len(), report.removed, report.pieces), (9, 0, 0));
        assert_eq!(
            out.edges().map(|e| e.iter().map(|x| x.poly_id1()).collect::<Vec<_>>()),
            nm.edges().map(|e| e.iter().map(|x| x.poly_id1()).collect())
        );
    }
}
