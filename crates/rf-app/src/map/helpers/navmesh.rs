//! "Navmesh": the navigation meshes (ynv) around the camera, streamed as the
//! C#'s `RDR2Map` does: the files of the navmesh grid's cells within
//! [`RANGE`] of the camera, read on worker threads and uploaded a few per
//! frame.
//!
//! Each navmesh is shown as `Rsc8Navmesh.InitNavmesh` builds it: its polygons
//! as triangle fans and its two-corner polygons as lines, each with its own
//! normal and colour (`DisplayColour`), in meshes of at most [`MAX_VERTICES`]
//! vertices (the C#'s `ShapeBuilder`). Each mesh is a part: the navmesh and
//! its `PartChildren`.
//!
//! A navmesh open in the editor is drawn from its document, and again when
//! it changes. Polygons are what's hovered and selected: the GPU pick finds the
//! mesh and the point under the mouse, [`Navmesh::polygon_at`] the polygon
//! there. The selected polygons and the hovered one are drawn over their
//! navmesh, outlined.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glam::{Mat4, Vec2, Vec3};
use rf_formats::formats::ynv::{self, NavmeshEdge, NavmeshPolygon, content_flags, poly_flags};
use rf_formats::hash::joaat;
use rf_formats::rpf8::FileRef;
use rf_render::HelperVertex;

use super::shapes::Shapes;
use super::{Collect, HelperKey, HelperPart, Mode, PartInfo};
use crate::game::Game;

/// The groups of a navmesh's helper keys (`source` is the navmesh's name hash).
pub mod group {
    /// One of its meshes; `index` is the mesh's.
    pub const MESH: u32 = 0;
    /// A polygon, what's hovered and selected; `index` is the polygon's.
    pub const POLYGON: u32 = 1;
    /// Its selected polygons, drawn over it.
    pub const SELECTED: u32 = 2;
    /// Its hovered polygon, drawn over it.
    pub const HOVERED: u32 = 3;
    /// Its edges, open edges and special links, drawn over it.
    pub const LINES: u32 = 4;
    /// Lines from its selected polygons to those across their edges.
    pub const NEIGHBOURS: u32 = 5;
}

/// What's drawn over the navmeshes, as bits (`Tools::navmesh_overlays`).
pub mod overlay {
    /// Every polygon's outline.
    pub const EDGES: u8 = 1;
    /// The edges with nothing across (walls, the navmesh's edge, blocked
    /// areas), and the two-corner polygons that join the next cell.
    pub const OPEN_EDGES: u8 = 2;
    /// From the selected polygons to those across their edges.
    pub const NEIGHBOURS: u8 = 4;
    /// Ladders, jumps, climbs and drops.
    pub const LINKS: u8 = 8;
    /// The ones made with the navmesh's meshes (the rest follow the selection).
    pub const WITH_MESHES: u8 = EDGES | OPEN_EDGES | LINKS;
}

const EDGE_COLOUR: [u8; 4] = [235, 235, 235, 255];
const OPEN_EDGE_COLOUR: [u8; 4] = [255, 60, 40, 255];
const STITCH_COLOUR: [u8; 4] = [40, 220, 255, 255];
const LINK_COLOUR: [u8; 4] = [255, 215, 0, 255];
const NEIGHBOUR_COLOUR: [u8; 4] = [80, 255, 120, 255];

/// The key of a navmesh's polygon.
pub fn polygon_key(navmesh: u32, index: u32) -> HelperKey {
    HelperKey { mode: Mode::Navmesh, source: navmesh, group: group::POLYGON, index }
}

/// How high over the navmesh the selection and the hover are drawn.
const OVERLAY_LIFT: f32 = 0.04;

/// Polygons with the flag "colour by flag" shows, and the others.
const HIGHLIGHTED: [u8; 4] = [255, 205, 40, 255];
const NOT_HIGHLIGHTED: [u8; 4] = [45, 45, 50, 255];
/// Polygons switched off for peds (blocked ones are).
const SWITCHED_OFF: [u8; 4] = [225, 30, 200, 255];

/// `StreamNavmeshesRange`.
const RANGE: f32 = 200.0;

/// `RDR2Map.NavGrid`: cells of 150 m from (-15900, -15900), 215 by 215.
const GRID_CELL: f32 = 150.0;
const GRID_MIN: f32 = -15900.0;
const GRID_CELLS: i32 = 215;

/// `ShapeBuilder.MaxVerticesPerMesh`.
const MAX_VERTICES: usize = 0xFF00;

/// How long a navmesh out of range is kept.
const KEEP: Duration = Duration::from_secs(20);

/// The frame time given to uploading loaded navmeshes.
const UPLOAD_BUDGET: Duration = Duration::from_millis(4);

const WORKERS: usize = 2;

/// `RDR2MapGrid.GetCellRange` of the box [`RANGE`] around `eye`: the cells
/// it touches, none when it's off the grid.
fn cells(eye: Vec3) -> Vec<(i32, i32)> {
    let (min, max) = (eye.truncate() - Vec2::splat(RANGE), eye.truncate() + Vec2::splat(RANGE));
    let inv = 1.0 / GRID_CELL;
    let lo = (min - Vec2::splat(GRID_MIN)) * inv;
    let hi = (max - Vec2::splat(GRID_MIN)) * inv;
    let (min_x, min_y, max_x, max_y) = (lo.x as i32, lo.y as i32, hi.x as i32, hi.y as i32);
    if max_x < 0 || max_y < 0 || min_x >= GRID_CELLS || min_y >= GRID_CELLS {
        return Vec::new();
    }
    (min_x..=max_x).flat_map(|x| (min_y..=max_y).map(move |y| (x, y))).collect()
}

/// The name of a grid cell's navmesh file (without its extension).
fn file_name((x, y): (i32, i32)) -> String {
    format!("navmesh[{}][{}]", x * 3, y * 3)
}

/// A polygon's colour: with "colour by flag", whether it has the flag; else
/// switched off for peds, or as the C# shows it.
fn colour(p: &NavmeshPolygon, highlight: Option<u64>) -> [u8; 4] {
    match highlight {
        Some(flag) if p.flags() & flag == flag => HIGHLIGHTED,
        Some(_) => NOT_HIGHLIGHTED,
        None if p.flags() & poly_flags::SWITCHED_OFF_FOR_PEDS != 0 => SWITCHED_OFF,
        None => display_colour(p),
    }
}

/// `Rsc8NavmeshPolygon.DisplayColour`.
fn display_colour(p: &NavmeshPolygon) -> [u8; 4] {
    use poly_flags::*;
    let f = p.flags();
    let has = |bit: u64| f & bit != 0;
    let (mut r, mut g, mut b) = (0.05f32, 0.05f32, 0.05f32);
    if has(DANGER_KEEP_AWAY_1) {
        r += 0.25;
    }
    if has(DANGER_KEEP_AWAY_2) {
        r += 0.25;
    }
    if has(PAVEMENT) {
        g += 0.25;
    }
    if has(DOOR) {
        r += 0.5;
        b += 0.5;
    }
    if has(STAIRS) {
        r += 0.5;
        g += 0.25;
    }
    if has(WATER) {
        b += 0.15 * (8 - p.water_depth() as i32) as f32;
    }
    if has(MUDDY) {
        r += 0.2;
        g += 0.1;
    }
    if has(INTERIOR) {
        b += 0.1;
    }
    if has(ROAD) {
        b += 0.1;
    }
    // `Colour.ToByte`: truncated, then clamped.
    let byte = |v: f32| ((v * 255.0) as i32).clamp(0, 255) as u8;
    [byte(r), byte(g), byte(b), 255]
}

/// `Rsc8NavmeshPolygon.ComputeNormal`: from its first two corners and the
/// average of the others; up when it has fewer than three or they meet.
fn normal(p: &NavmeshPolygon, vertices: &[[f32; 3]], indices: &[u16]) -> Vec3 {
    let count = p.index_count() as usize;
    if count < 3 {
        return Vec3::Z;
    }
    let corner = |i: usize| Vec3::from(p.vertex(i, vertices, indices));
    let (v0, v1) = (corner(0), corner(1));
    let mut v2 = (2..count).map(corner).sum::<Vec3>();
    if count > 3 {
        v2 *= 1.0 / (count - 2) as f32;
    }
    if v0 == v1 || v0 == v2 || v1 == v2 {
        return Vec3::Z;
    }
    let (d1, d2) = ((v1 - v0).normalize(), (v2 - v0).normalize());
    d1.cross(d2).normalize_or(Vec3::Z)
}

/// One of a navmesh's meshes.
#[derive(Default)]
struct Chunk {
    lines: bool,
    vertices: Vec<HelperVertex>,
    indices: Vec<u32>,
    /// How many of the navmesh's polygons it has.
    polygons: usize,
    min: Vec3,
    max: Vec3,
}

impl Chunk {
    fn add(&mut self, p: Vec3, n: Vec3, colour: [u8; 4]) {
        if self.vertices.is_empty() {
            (self.min, self.max) = (p, p);
        }
        (self.min, self.max) = (self.min.min(p), self.max.max(p));
        self.indices.push(self.vertices.len() as u32);
        self.vertices.push(HelperVertex::new(p, n, colour));
    }
}

/// `ShapeBuilder` as the navmesh uses it: no shared vertices, and a new mesh
/// before one would reach [`MAX_VERTICES`].
struct Builder {
    current: Chunk,
    done: Vec<Chunk>,
}

impl Builder {
    fn new(lines: bool) -> Self {
        Self { current: Chunk { lines, ..Default::default() }, done: Vec::new() }
    }

    /// `EnsureSpaceForPolygon`, and counts the polygon in.
    fn polygon(&mut self, vertex_count: usize) {
        if self.current.vertices.len() + vertex_count >= MAX_VERTICES && !self.current.vertices.is_empty() {
            let lines = self.current.lines;
            self.done.push(std::mem::replace(&mut self.current, Chunk { lines, ..Default::default() }));
        }
        self.current.polygons += 1;
    }

    /// `EndBuild`.
    fn finish(mut self) -> Vec<Chunk> {
        if !self.current.vertices.is_empty() {
            self.done.push(self.current);
        }
        self.done
    }
}

/// `InitNavmesh`: the triangle meshes, then the line meshes.
fn meshes(vertices: &[[f32; 3]], indices: &[u16], polygons: &[NavmeshPolygon], highlight: Option<u64>) -> Vec<Chunk> {
    let mut tris = Builder::new(false);
    let mut lines = Builder::new(true);
    for poly in polygons {
        let count = poly.index_count() as usize;
        let colour = colour(poly, highlight);
        let n = normal(poly, vertices, indices);
        let corner = |i: usize| Vec3::from(poly.vertex(i, vertices, indices));
        match count {
            0 | 1 => {}
            2 => {
                lines.polygon(2);
                lines.current.add(corner(0), Vec3::ZERO, colour);
                lines.current.add(corner(1), Vec3::ZERO, colour);
            }
            _ => {
                tris.polygon((count - 2) * 3);
                let p0 = corner(0);
                for t in 0..count - 2 {
                    tris.current.add(p0, n, colour);
                    tris.current.add(corner(t + 1), n, colour);
                    tris.current.add(corner(t + 2), n, colour);
                }
            }
        }
    }
    let mut all = tris.finish();
    all.extend(lines.finish());
    all
}

/// `Rsc8NavmeshFlags` as .NET names them.
fn content_flags_name(flags: u32) -> String {
    use content_flags::*;
    const NAMES: [(u32, &str); 5] = [
        (POLYGONS, "Polygons"),
        (SPECIAL_LINKS, "SpecialLinks"),
        (VEHICLE, "Vehicle"),
        (UNKNOWN_8, "Unknown8"),
        (UNKNOWN_16, "Unknown16"),
    ];
    let known = NAMES.iter().fold(0, |m, (bit, _)| m | bit);
    match flags {
        0 => "None".into(),
        f if f & !known != 0 => f.to_string(),
        f => NAMES.iter().filter(|(bit, _)| f & bit != 0).map(|(_, n)| *n).collect::<Vec<_>>().join(", "),
    }
}

fn v3(v: [f32; 3]) -> String {
    format!("{:.3}, {:.3}, {:.3}", v[0], v[1], v[2])
}

/// The navmesh's fields, for Properties.
fn fields(nm: &ynv::Navmesh) -> Vec<(String, String)> {
    let adjacent = nm.adj_area_ids_trimmed().unwrap_or_default();
    let adjacent: Vec<String> = adjacent.iter().map(u32::to_string).collect();
    vec![
        ("AreaID".into(), nm.area_id.to_string()),
        ("BuildID".into(), nm.build_id.to_string()),
        ("ContentFlags".into(), content_flags_name(nm.content_flags)),
        ("VerticesCount".into(), nm.vertices_count.to_string()),
        ("PolygonsCount".into(), nm.polygons_count.to_string()),
        ("SpecialLinksCount".into(), nm.special_links_count.to_string()),
        ("AdjAreaIDs".into(), adjacent.join(", ")),
        ("AABBMin".into(), v3(nm.aabb_min)),
        ("AABBMax".into(), v3(nm.aabb_max)),
    ]
}

/// What Properties shows of a polygon.
fn polygon_fields(flat: &Flat, index: u32) -> Vec<(String, String)> {
    let Some(p) = flat.polygons.get(index as usize) else { return Vec::new() };
    let corners: Vec<Vec3> = flat.corners(p).collect();
    let centre = corners.iter().sum::<Vec3>() / corners.len().max(1) as f32;
    vec![
        ("Polygon".into(), index.to_string()),
        ("Corners".into(), corners.len().to_string()),
        ("Centre".into(), v3(centre.to_array())),
        ("Flags".into(), ynv::format_poly_flags(p.flags())),
    ]
}

/// A navmesh's polygons on the CPU: for finding the one under the mouse, and
/// drawing the selection.
pub struct Flat {
    vertices: Vec<[f32; 3]>,
    indices: Vec<u16>,
    polygons: Vec<NavmeshPolygon>,
    /// One per corner, as the indices.
    edges: Vec<NavmeshEdge>,
    /// The navmesh's own index in its adjacent areas: edges to polygons of
    /// the same navmesh have it.
    own: Option<u32>,
    /// The special links' ends in this navmesh: an end in another one is
    /// quantised in that one's box, so it isn't known here.
    links: Vec<(Option<Vec3>, Option<Vec3>)>,
    /// Each polygon's middle (its corners' average).
    centres: Vec<Vec3>,
}

impl Flat {
    fn new(nm: &ynv::Navmesh) -> Option<Self> {
        let n = (nm.adj_area_ids_count as usize).min(32);
        let own = nm.adj_area_ids[..n].iter().position(|&a| a == nm.area_id).map(|i| i as u32);
        let (min, size) = (nm.aabb_min, nm.extents3());
        let area = nm.area_id;
        let end = |v: ynv::NavmeshVertex, a: u16| (a as u32 == area).then(|| Vec3::from(v.to_vec3(min, size)));
        // The links the polygons list: the ones in use (each pair's other
        // half, the way back, is listed by none).
        let polygons = nm.polygons()?;
        let inds = nm.special_links_inds.as_deref().unwrap_or_default();
        let mut listed: Vec<u16> = polygons
            .iter()
            .flat_map(|p| {
                let start = p.link_id() as usize;
                inds.get(start..start + p.link_count() as usize).unwrap_or_default().iter().copied()
            })
            .collect();
        listed.sort_unstable();
        listed.dedup();
        let all = nm.special_links.as_deref().unwrap_or_default();
        let links = listed.iter().filter_map(|&i| all.get(i as usize));
        let links = links.map(|l| (end(l.position1, l.area_id1), end(l.position2, l.area_id2)));
        let mut flat = Self {
            vertices: nm.vertex_positions()?,
            indices: nm.indices()?,
            polygons,
            edges: nm.edges().unwrap_or_default(),
            own,
            links: links.collect(),
            centres: Vec::new(),
        };
        flat.centres =
            flat.polygons.iter().map(|p| flat.corners(p).sum::<Vec3>() / p.index_count().max(1) as f32).collect();
        Some(flat)
    }

    /// The polygon of the same navmesh across a corner's edge.
    fn across(&self, corner: usize) -> Option<u32> {
        let e = self.edges.get(corner)?;
        (Some(e.area_id1()) == self.own && e.poly_id1() != 0x7FFF && (e.poly_id1() as usize) < self.polygons.len())
            .then(|| e.poly_id1())
    }

    /// The line meshes of `overlays` ([`overlay::WITH_MESHES`] ones).
    fn lines(&self, overlays: u8) -> Vec<Chunk> {
        let mut b = Builder::new(true);
        let mut line = |a: Vec3, z: Vec3, colour: [u8; 4]| {
            b.polygon(2);
            b.current.add(a, Vec3::ZERO, colour);
            b.current.add(z, Vec3::ZERO, colour);
        };
        let lift = |v: Vec3, h: f32| v + Vec3::Z * h;
        for p in &self.polygons {
            let c: Vec<Vec3> = self.corners(p).collect();
            if c.len() == 2 {
                if overlays & overlay::OPEN_EDGES != 0 {
                    line(lift(c[0], 0.06), lift(c[1], 0.06), STITCH_COLOUR);
                }
                continue;
            }
            for (i, &a) in c.iter().enumerate() {
                let z = c[(i + 1) % c.len()];
                let open = self.across(p.index_id as usize + i).is_none();
                if open && overlays & overlay::OPEN_EDGES != 0 {
                    line(lift(a, 0.05), lift(z, 0.05), OPEN_EDGE_COLOUR);
                } else if overlays & overlay::EDGES != 0 {
                    line(lift(a, 0.03), lift(z, 0.03), EDGE_COLOUR);
                }
            }
        }
        if overlays & overlay::LINKS != 0 {
            for &ends in &self.links {
                match ends {
                    (Some(a), Some(z)) => {
                        line(lift(a, 0.15), lift(z, 0.15), LINK_COLOUR);
                        line(a, lift(a, 0.3), LINK_COLOUR);
                        line(z, lift(z, 0.3), LINK_COLOUR);
                    }
                    // Into or from the next cell: a post on this side.
                    (Some(a), None) | (None, Some(a)) => line(a, lift(a, 1.0), LINK_COLOUR),
                    (None, None) => {}
                }
            }
        }
        b.finish()
    }

    /// Lines from the polygons' middles to their neighbours'.
    fn neighbour_lines(&self, polygons: &[u32]) -> (Vec<HelperVertex>, Vec<u32>) {
        let (mut vertices, mut indices) = (Vec::new(), Vec::new());
        let lift = Vec3::Z * (OVERLAY_LIFT + 0.05);
        for &i in polygons {
            let Some(p) = self.polygons.get(i as usize) else { continue };
            for k in p.index_id as usize..(p.index_id + p.index_count()) as usize {
                let Some(q) = self.across(k) else { continue };
                for v in [self.centres[i as usize], self.centres[q as usize]] {
                    indices.push(vertices.len() as u32);
                    vertices.push(HelperVertex::new(v + lift, Vec3::ZERO, NEIGHBOUR_COLOUR));
                }
            }
        }
        (vertices, indices)
    }

    /// The polygons whose middle the camera sees inside a box of the view (in
    /// normalised device coordinates), within `range` of the eye.
    fn polygons_in(&self, view: &rf_render::CameraView, min: Vec2, max: Vec2, range: f32) -> Vec<u32> {
        let inside = |v: Vec2| v.x >= min.x && v.x <= max.x && v.y >= min.y && v.y <= max.y;
        (0..self.polygons.len())
            .filter(|&i| self.polygons[i].index_count() >= 3)
            .filter(|&i| self.centres[i].distance(view.eye) <= range)
            .filter(|&i| view.project(self.centres[i]).is_some_and(inside))
            .map(|i| i as u32)
            .collect()
    }

    fn corners<'a>(&'a self, p: &'a NavmeshPolygon) -> impl Iterator<Item = Vec3> + 'a {
        (0..p.index_count() as usize).map(move |i| Vec3::from(p.vertex(i, &self.vertices, &self.indices)))
    }

    /// The polygon under a point of its surface: the one whose triangles
    /// have the point inside (seen from above) and pass nearest it.
    pub fn polygon_at(&self, point: Vec3) -> Option<u32> {
        const MARGIN: f32 = 0.5;
        let mut best: Option<(f32, u32)> = None;
        for (i, p) in self.polygons.iter().enumerate() {
            if p.index_count() < 3 {
                continue;
            }
            let (min, max) = (Vec3::from(p.cell_aabb.min()), Vec3::from(p.cell_aabb.max()));
            if point.x < min.x - 0.1 || point.x > max.x + 0.1 || point.y < min.y - 0.1 || point.y > max.y + 0.1 {
                continue;
            }
            if point.z < min.z - MARGIN || point.z > max.z + MARGIN {
                continue;
            }
            let c: Vec<Vec3> = self.corners(p).collect();
            for t in 1..c.len() - 1 {
                if let Some(z) = height_in_triangle(point, c[0], c[t], c[t + 1]) {
                    let d = (z - point.z).abs();
                    if d < MARGIN && best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, i as u32));
                    }
                }
            }
        }
        best.map(|(_, i)| i)
    }

    /// A mesh of some polygons, a little over them.
    fn overlay(&self, polygons: &[u32]) -> (Vec<HelperVertex>, Vec<u32>) {
        let (mut vertices, mut indices) = (Vec::new(), Vec::new());
        let lift = Vec3::Z * OVERLAY_LIFT;
        for &i in polygons {
            let Some(p) = self.polygons.get(i as usize).filter(|p| p.index_count() >= 3) else { continue };
            let c: Vec<Vec3> = self.corners(p).collect();
            let n = normal(p, &self.vertices, &self.indices);
            for t in 1..c.len() - 1 {
                for v in [c[0], c[t], c[t + 1]] {
                    indices.push(vertices.len() as u32);
                    vertices.push(HelperVertex::new(v + lift, n, [255; 4]));
                }
            }
        }
        (vertices, indices)
    }
}

/// The height of the triangle `a b c` over `p` (seen from above), if `p` is in
/// it.
fn height_in_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let (v0, v1, v2) = ((b - a).truncate(), (c - a).truncate(), (p - a).truncate());
    let d = v0.perp_dot(v1);
    if d.abs() < 1e-9 {
        return None;
    }
    let (u, v) = (v2.perp_dot(v1) / d, v0.perp_dot(v2) / d);
    const E: f32 = -1e-4;
    (u >= E && v >= E && u + v <= 1.0 - E).then_some(a.z + u * (b.z - a.z) + v * (c.z - a.z))
}

/// What a navmesh is drawn from: the editor's document (its data's address),
/// or its file (0); the flag "colour by flag" shows; and the overlays made
/// with its meshes.
type Built = (usize, Option<u64>, u8);

/// Where a navmesh to draw is.
enum Data {
    File(FileRef),
    Doc(Arc<ynv::Navmesh>),
}

/// A navmesh to read and make meshes of.
struct Job {
    version: u64,
    hash: u32,
    data: Data,
    built: Built,
    owner: String,
}

/// A navmesh read: its meshes, each with what Properties shows of it.
struct Loaded {
    version: u64,
    hash: u32,
    built: Built,
    meshes: Meshes,
    flat: Option<Arc<Flat>>,
}

/// A navmesh's meshes, each with what Properties shows of it and its key's
/// group.
type Meshes = Vec<(Chunk, PartInfo, u32)>;

/// A navmesh's meshes, and its polygons on the CPU.
fn load(game: &Game, job: &Job) -> (Meshes, Option<Arc<Flat>>) {
    let nm = match &job.data {
        Data::Doc(nm) => nm.clone(),
        Data::File(file) => {
            let read = game.fs.extract_loose(*file).map_err(rf_formats::Error::from).and_then(|d| ynv::read(&d));
            match read {
                Ok(nm) => Arc::new(nm),
                Err(e) => {
                    log::warn!("{}: {e}", game.path(*file));
                    return (Vec::new(), None);
                }
            }
        }
    };
    let Some(flat) = Flat::new(&nm) else { return (Vec::new(), None) };
    let navmesh = fields(&nm);
    let (mut triangles, mut lines) = (0, 0);
    let mut meshes: Meshes = meshes(&flat.vertices, &flat.indices, &flat.polygons, job.built.1)
        .into_iter()
        .map(|chunk| {
            // The meshes after the first as `ShapeBuilder` names them, the
            // lines' apart.
            let n = if chunk.lines { &mut lines } else { &mut triangles };
            let name = match (chunk.lines, *n) {
                (false, 0) => String::new(),
                (false, i) => format!("Extra{i}"),
                (true, 0) => "Lines".into(),
                (true, i) => format!("Lines Extra{i}"),
            };
            *n += 1;
            let (primitives, count) = match chunk.lines {
                true => ("Lines", chunk.indices.len() / 2),
                false => ("Triangles", chunk.indices.len() / 3),
            };
            let mut fields = navmesh.clone();
            fields.push(("Polygons".into(), chunk.polygons.to_string()));
            fields.push((primitives.into(), count.to_string()));
            let info = PartInfo { type_name: "Navmesh".into(), name, owner: job.owner.clone(), fields };
            (chunk, info, group::MESH)
        })
        .collect();
    for chunk in flat.lines(job.built.2) {
        let info = PartInfo { type_name: "Navmesh lines".into(), owner: job.owner.clone(), ..Default::default() };
        meshes.push((chunk, info, group::LINES));
    }
    (meshes, Some(Arc::new(flat)))
}

/// The workers reading navmesh files.
struct Loader {
    game: Arc<Game>,
    jobs: Sender<Job>,
    loaded: Receiver<Loaded>,
}

impl Loader {
    fn new(game: Arc<Game>) -> Self {
        let (jobs, queue) = channel::<Job>();
        let (done, loaded) = channel();
        let queue = Arc::new(Mutex::new(queue));
        for _ in 0..WORKERS {
            let (queue, done, game) = (queue.clone(), done.clone(), game.clone());
            let work = move || {
                loop {
                    // The queue goes with the loader: the workers stop.
                    let Ok(job) = queue.lock().unwrap().recv() else { return };
                    let (meshes, flat) = load(&game, &job);
                    let loaded = Loaded { version: job.version, hash: job.hash, built: job.built, meshes, flat };
                    if done.send(loaded).is_err() {
                        return;
                    }
                }
            };
            std::thread::Builder::new().name("navmesh".into()).spawn(work).expect("a navmesh worker");
        }
        Self { game, jobs, loaded }
    }
}

enum State {
    Loading,
    Ready(Vec<HelperPart>),
    /// No file for the cell, or it didn't read.
    Missing,
}

struct Entry {
    state: State,
    flat: Option<Arc<Flat>>,
    /// The file's name, for labels.
    owner: String,
    /// What the meshes shown are of, and what's being made.
    built: Option<Built>,
    pending: Option<Built>,
    /// The selection drawn over it: the polygons' signature, and the parts.
    selected: Option<(u64, Vec<HelperPart>)>,
    last_used: Instant,
}

/// The navmeshes loaded, by file name hash.
#[derive(Default)]
pub struct Navmesh {
    loader: Option<Loader>,
    /// The game data's version the files came from.
    version: u64,
    files: HashMap<u32, Entry>,
    /// The hovered polygon drawn over its navmesh.
    hovered: Option<(HelperKey, HelperPart)>,
}

impl Navmesh {
    pub fn collect(&mut self, c: &Collect, _shapes: &mut Shapes, out: &mut Vec<HelperPart>) {
        if self.loader.as_ref().is_none_or(|l| !Arc::ptr_eq(&l.game, c.game)) {
            self.loader = Some(Loader::new(c.game.clone()));
            self.files.clear();
        }
        let version = c.game.data_version();
        if version != self.version {
            // Other game data (the project's files changed): every navmesh again.
            self.version = version;
            self.files.clear();
        }
        let Some(loader) = &self.loader else { return };

        let start = Instant::now();
        while start.elapsed() < UPLOAD_BUDGET {
            let Ok(loaded) = loader.loaded.try_recv() else { break };
            if loaded.version != self.version {
                continue;
            }
            let Some(entry) = self.files.get_mut(&loaded.hash) else { continue };
            if entry.pending == Some(loaded.built) {
                entry.pending = None;
            }
            entry.built = Some(loaded.built);
            entry.flat = loaded.flat;
            entry.selected = None;
            entry.state = if loaded.meshes.is_empty() {
                State::Missing
            } else {
                State::Ready(parts(c, loaded.hash, loaded.meshes))
            };
        }

        let now = Instant::now();
        let data = c.game.data();
        for cell in cells(c.eye) {
            let name = file_name(cell);
            let hash = joaat(&name);
            let doc = c
                .navmesh_preview
                .and_then(|m| m.navmesh(hash).cloned())
                .or_else(|| c.editor.navmesh(hash).map(|d| d.root.clone()));
            let doc_at = doc.as_ref().map_or(0, |d| Arc::as_ptr(d) as usize);
            let want: Built = (doc_at, c.highlight, c.overlays & overlay::WITH_MESHES);
            let entry = self.files.entry(hash).or_insert_with(|| Entry {
                state: State::Loading,
                flat: None,
                owner: format!("{name}.ynv"),
                built: None,
                pending: None,
                selected: None,
                last_used: now,
            });
            entry.last_used = now;
            if entry.built != Some(want) && entry.pending != Some(want) {
                let data = match doc {
                    Some(nm) => Some(Data::Doc(nm)),
                    None => data.stream_entry("ynv", hash).map(Data::File),
                };
                match data {
                    Some(data) => {
                        if let Data::File(f) = &data {
                            entry.owner = c.game.fs.name(*f);
                        }
                        let job = Job { version, hash, data, built: want, owner: entry.owner.clone() };
                        let _ = loader.jobs.send(job);
                        entry.pending = Some(want);
                    }
                    None => {
                        entry.state = State::Missing;
                        entry.built = Some(want);
                    }
                }
            }
            if let State::Ready(parts) = &entry.state {
                out.extend(parts.iter().cloned());
            }

            // The selected polygons over it.
            let Some(flat) = &entry.flat else { continue };
            let mut selected: Vec<u32> = c
                .selected
                .iter()
                .filter(|k| k.mode == Mode::Navmesh && k.group == group::POLYGON && k.source == hash)
                .map(|k| k.index)
                .collect();
            if selected.is_empty() {
                entry.selected = None;
                continue;
            }
            selected.sort_unstable();
            let neighbours = c.overlays & overlay::NEIGHBOURS != 0;
            let mut h = std::collections::hash_map::DefaultHasher::new();
            selected.hash(&mut h);
            (Arc::as_ptr(flat) as usize).hash(&mut h);
            neighbours.hash(&mut h);
            let sig = h.finish();
            if entry.selected.as_ref().is_none_or(|(s, _)| *s != sig) {
                let key = HelperKey { mode: Mode::Navmesh, source: hash, group: group::SELECTED, index: 0 };
                let info = PartInfo { type_name: "Polygons".into(), owner: entry.owner.clone(), ..Default::default() };
                let mut parts: Vec<HelperPart> = overlay(c, flat, &selected, key, info).into_iter().collect();
                if neighbours {
                    let (v, i) = flat.neighbour_lines(&selected);
                    let key = HelperKey { group: group::NEIGHBOURS, ..key };
                    let info =
                        PartInfo { type_name: "Neighbours".into(), owner: entry.owner.clone(), ..Default::default() };
                    parts.extend(lines_part(c, &v, &i, key, info));
                }
                entry.selected = Some((sig, parts));
            }
            if let Some((_, parts)) = &entry.selected {
                out.extend(parts.iter().cloned());
            }
        }
        self.files.retain(|_, e| e.pending.is_some() || now.duration_since(e.last_used) < KEEP);

        // The hovered polygon over its navmesh.
        let hover = c.hover.filter(|k| k.mode == Mode::Navmesh && k.group == group::POLYGON);
        match hover {
            Some(k) if self.hovered.as_ref().is_some_and(|(h, _)| *h == k) => {}
            Some(k) => {
                self.hovered = self.files.get(&k.source).and_then(|e| {
                    let key = HelperKey { group: group::HOVERED, ..k };
                    let info = PartInfo { type_name: "Polygon".into(), owner: e.owner.clone(), ..Default::default() };
                    overlay(c, e.flat.as_ref()?, &[k.index], key, info).map(|p| (k, p))
                });
            }
            None => self.hovered = None,
        }
        if let Some((_, p)) = &self.hovered {
            out.push(p.clone());
        }
    }

    /// The polygon of a navmesh under a point of its surface.
    pub fn polygon_at(&self, navmesh: u32, point: Vec3) -> Option<HelperKey> {
        let flat = self.files.get(&navmesh)?.flat.as_ref()?;
        flat.polygon_at(point).map(|i| polygon_key(navmesh, i))
    }

    /// The polygons whose middle the camera sees in a box of the view (in
    /// normalised device coordinates), in the navmeshes loaded around it.
    pub fn polygons_in(&self, view: &rf_render::CameraView, min: Vec2, max: Vec2) -> Vec<HelperKey> {
        let mut out = Vec::new();
        for (&hash, e) in &self.files {
            let Some(flat) = &e.flat else { continue };
            out.extend(flat.polygons_in(view, min, max, RANGE).into_iter().map(|i| polygon_key(hash, i)));
        }
        out
    }

    /// The hover label and Properties' fields of a polygon's key.
    pub fn polygon_info(&self, key: HelperKey) -> Option<PartInfo> {
        let e = self.files.get(&key.source)?;
        let fields = polygon_fields(e.flat.as_ref()?, key.index);
        Some(PartInfo { type_name: "Polygon".into(), name: key.index.to_string(), owner: e.owner.clone(), fields })
    }
}

/// A part drawing some polygons over their navmesh.
fn overlay(c: &Collect, flat: &Flat, polygons: &[u32], key: HelperKey, info: PartInfo) -> Option<HelperPart> {
    let (vertices, indices) = flat.overlay(polygons);
    if vertices.is_empty() {
        return None;
    }
    let (min, max) = vertices.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), v| {
        (lo.min(Vec3::from(v.position)), hi.max(Vec3::from(v.position)))
    });
    let mesh = c.gpu.renderer.create_helper_mesh(&c.gpu.ctx, &vertices, &indices, false);
    let colour = if key.group == group::HOVERED { [255, 255, 255, 255] } else { [255, 150, 40, 255] };
    Some(HelperPart {
        key,
        mesh: Arc::new(mesh),
        transform: Mat4::IDENTITY,
        colour,
        centre: (min + max) * 0.5,
        radius: (max - min).length() * 0.5,
        info: Arc::new(info),
    })
}

/// A part of lines.
fn lines_part(
    c: &Collect,
    vertices: &[HelperVertex],
    indices: &[u32],
    key: HelperKey,
    info: PartInfo,
) -> Option<HelperPart> {
    if vertices.is_empty() {
        return None;
    }
    let (min, max) = vertices.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), v| {
        (lo.min(Vec3::from(v.position)), hi.max(Vec3::from(v.position)))
    });
    let mesh = c.gpu.renderer.create_helper_mesh(&c.gpu.ctx, vertices, indices, true);
    Some(HelperPart {
        key,
        mesh: Arc::new(mesh),
        transform: Mat4::IDENTITY,
        colour: [255; 4],
        centre: (min + max) * 0.5,
        radius: (max - min).length() * 0.5,
        info: Arc::new(info),
    })
}

/// A navmesh's meshes on the GPU, as parts.
fn parts(c: &Collect, hash: u32, meshes: Meshes) -> Vec<HelperPart> {
    meshes
        .into_iter()
        .enumerate()
        .map(|(i, (chunk, info, group))| {
            let gpu = c.gpu;
            let mesh = gpu.renderer.create_helper_mesh(&gpu.ctx, &chunk.vertices, &chunk.indices, chunk.lines);
            HelperPart {
                key: HelperKey { mode: Mode::Navmesh, source: hash, group, index: i as u32 },
                mesh: Arc::new(mesh),
                transform: Mat4::IDENTITY,
                colour: [255; 4],
                centre: (chunk.min + chunk.max) * 0.5,
                radius: (chunk.max - chunk.min).length() * 0.5,
                info: Arc::new(info),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_around_valentine() {
        let cells = cells(Vec3::new(-300.0, 640.0, 122.0));
        assert_eq!(cells.len(), 16);
        assert_eq!(cells.first(), Some(&(102, 108)));
        assert_eq!(cells.last(), Some(&(105, 111)));
        assert_eq!(file_name(cells[0]), "navmesh[306][324]");
        assert!(super::cells(Vec3::new(20000.0, 0.0, 0.0)).is_empty());
    }

    fn polygon(index_id: u32, count: u32, flags: u64) -> NavmeshPolygon {
        let mut p = NavmeshPolygon { index_id, ..Default::default() };
        p.set_index_count(count);
        p.set_flags(flags);
        p
    }

    #[test]
    fn colours() {
        assert_eq!(display_colour(&polygon(0, 3, 0)), [12, 12, 12, 255]);
        assert_eq!(display_colour(&polygon(0, 3, poly_flags::DOOR)), [140, 12, 140, 255]);
        let mut water = polygon(0, 3, poly_flags::WATER);
        assert_eq!(display_colour(&water)[2], 255);
        water.set_water_depth(7);
        assert_eq!(display_colour(&water)[2], 51);
    }

    #[test]
    fn polygons_to_meshes() {
        let vertices = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
        let indices = [0, 1, 2, 3, 0, 1];
        let polys = [polygon(0, 4, 0), polygon(4, 2, 0)];
        let m = meshes(&vertices, &indices, &polys, None);
        assert_eq!(m.len(), 2);
        assert!(!m[0].lines && m[1].lines);
        assert_eq!((m[0].vertices.len(), m[0].polygons), (6, 1));
        assert_eq!(m[1].indices, [0, 1]);
        assert_eq!(Vec3::from(m[0].vertices[0].normal), Vec3::Z);
        assert_eq!((m[0].min, m[0].max), (Vec3::ZERO, Vec3::new(1.0, 1.0, 0.0)));

        // A new mesh before one reaches the limit.
        let polys = vec![polygon(0, 3, 0); MAX_VERTICES / 3];
        let m = meshes(&vertices, &indices, &polys, None);
        assert_eq!(m.iter().map(|c| c.polygons).collect::<Vec<_>>(), [MAX_VERTICES / 3 - 1, 1]);
    }

    #[test]
    fn polygon_under_a_point() {
        // Two squares side by side, the second a metre higher.
        let vertices = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 1.0],
            [2.0, 0.0, 1.0],
            [2.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
        ];
        let indices = vec![0, 1, 2, 3, 4, 5, 6, 7];
        let mut polygons = vec![polygon(0, 4, 0), polygon(4, 4, 0)];
        polygons[0].cell_aabb = ynv::NavmeshAabb::new([0.0; 3], [1.0, 1.0, 0.0]);
        polygons[1].cell_aabb = ynv::NavmeshAabb::new([1.0, 0.0, 1.0], [2.0, 1.0, 1.0]);
        // Their shared edge (the first's right, the second's left) joins them.
        let edge = |poly: u32| {
            let mut e = NavmeshEdge::default();
            e.set_area_id1(if poly == 0x7FFF { 1 } else { 0 });
            e.set_poly_id1(poly);
            e
        };
        let mut edges = vec![edge(0x7FFF); 8];
        edges[1] = edge(1);
        edges[7] = edge(0);
        let mut flat =
            Flat { vertices, indices, polygons, edges, own: Some(0), links: Vec::new(), centres: Vec::new() };
        flat.centres = flat.polygons.iter().map(|p| flat.corners(p).sum::<Vec3>() / 4.0).collect();
        assert_eq!(flat.polygon_at(Vec3::new(0.3, 0.7, 0.0)), Some(0));
        assert_eq!(flat.polygon_at(Vec3::new(1.5, 0.2, 1.0)), Some(1));
        // Over the first, but far above it.
        assert_eq!(flat.polygon_at(Vec3::new(0.5, 0.5, 3.0)), None);
        assert_eq!(flat.polygon_at(Vec3::new(5.0, 5.0, 0.0)), None);
        let (v, i) = flat.overlay(&[1]);
        assert_eq!((v.len(), i.len()), (6, 6));
        assert!((v[0].position[2] - (1.0 + OVERLAY_LIFT)).abs() < 1e-6);

        // Eight corners' edges: the one between the squares is an edge of each
        // (drawn twice), the other six are open. Edges alone draws them all.
        let count = |chunks: Vec<Chunk>| chunks.iter().map(|c| c.vertices.len() / 2).sum::<usize>();
        assert_eq!(count(flat.lines(overlay::EDGES)), 8);
        assert_eq!(count(flat.lines(overlay::OPEN_EDGES)), 6);
        assert_eq!(count(flat.lines(overlay::EDGES | overlay::OPEN_EDGES)), 8);
        assert_eq!(flat.neighbour_lines(&[0]).0.len(), 2);

        // Seen from above, a box around the first square's middle.
        let camera = rf_render::FlyCamera { position: Vec3::new(1.0, 0.5, 10.0), yaw: 0.0, pitch: -1.55, fov_y: 1.0 };
        let view = camera.camera_view(1.0);
        let c0 = view.project(flat.centres[0]).unwrap();
        let d = Vec2::splat(0.01);
        assert_eq!(flat.polygons_in(&view, c0 - d, c0 + d, 100.0), [0]);
        assert_eq!(flat.polygons_in(&view, Vec2::splat(-1.0), Vec2::splat(1.0), 100.0), [0, 1]);
        assert!(flat.polygons_in(&view, Vec2::splat(-1.0), Vec2::splat(1.0), 5.0).is_empty());
    }

    #[test]
    fn colours_by_flag() {
        let p = polygon(0, 3, poly_flags::PAVEMENT | poly_flags::SWITCHED_OFF_FOR_PEDS);
        assert_eq!(colour(&p, None), SWITCHED_OFF);
        assert_eq!(colour(&p, Some(poly_flags::PAVEMENT)), HIGHLIGHTED);
        assert_eq!(colour(&p, Some(poly_flags::ROAD)), NOT_HIGHLIGHTED);
    }
}
