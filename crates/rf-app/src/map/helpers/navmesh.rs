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

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glam::{Mat4, Vec2, Vec3};
use rf_formats::formats::ynv::{self, NavmeshPolygon, content_flags, poly_flags};
use rf_formats::hash::joaat;
use rf_formats::rpf8::FileRef;
use rf_render::HelperVertex;

use super::shapes::Shapes;
use super::{Collect, HelperKey, HelperPart, Mode, PartInfo};
use crate::game::Game;

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
fn meshes(vertices: &[[f32; 3]], indices: &[u16], polygons: &[NavmeshPolygon]) -> Vec<Chunk> {
    let mut tris = Builder::new(false);
    let mut lines = Builder::new(true);
    for poly in polygons {
        let count = poly.index_count() as usize;
        let colour = display_colour(poly);
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

/// A navmesh file to read.
struct Job {
    version: u64,
    hash: u32,
    file: FileRef,
}

/// A navmesh file read: its meshes, each with what Properties shows of it.
struct Loaded {
    version: u64,
    hash: u32,
    meshes: Vec<(Chunk, PartInfo)>,
}

fn load(game: &Game, job: &Job) -> Vec<(Chunk, PartInfo)> {
    let owner = game.fs.name(job.file);
    let read = game.fs.extract_loose(job.file).map_err(rf_formats::Error::from).and_then(|d| ynv::read(&d));
    let nm = match read {
        Ok(nm) => nm,
        Err(e) => {
            log::warn!("{}: {e}", game.path(job.file));
            return Vec::new();
        }
    };
    let (Some(vertices), Some(indices), Some(polygons)) = (nm.vertex_positions(), nm.indices(), nm.polygons()) else {
        return Vec::new();
    };
    let navmesh = fields(&nm);
    let (mut triangles, mut lines) = (0, 0);
    meshes(&vertices, &indices, &polygons)
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
            let info = PartInfo { type_name: "Navmesh".into(), name, owner: owner.clone(), fields };
            (chunk, info)
        })
        .collect()
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
                    let meshes = load(&game, &job);
                    if done.send(Loaded { version: job.version, hash: job.hash, meshes }).is_err() {
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
    last_used: Instant,
}

/// The navmeshes loaded, by file name hash.
#[derive(Default)]
pub struct Navmesh {
    loader: Option<Loader>,
    /// The game data's version the files came from.
    version: u64,
    files: HashMap<u32, Entry>,
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
            entry.state = if loaded.meshes.is_empty() {
                State::Missing
            } else {
                State::Ready(parts(c, loaded.hash, loaded.meshes))
            };
        }

        let now = Instant::now();
        let data = c.game.data();
        for cell in cells(c.eye) {
            let hash = joaat(&file_name(cell));
            let entry = self.files.entry(hash).or_insert_with(|| {
                let state = match data.stream_entry("ynv", hash) {
                    Some(file) => {
                        let _ = loader.jobs.send(Job { version, hash, file });
                        State::Loading
                    }
                    None => State::Missing,
                };
                Entry { state, last_used: now }
            });
            entry.last_used = now;
            if let State::Ready(parts) = &entry.state {
                out.extend(parts.iter().cloned());
            }
        }
        self.files.retain(|_, e| matches!(e.state, State::Loading) || now.duration_since(e.last_used) < KEEP);
    }
}

/// A navmesh's meshes on the GPU, as parts.
fn parts(c: &Collect, hash: u32, meshes: Vec<(Chunk, PartInfo)>) -> Vec<HelperPart> {
    meshes
        .into_iter()
        .enumerate()
        .map(|(i, (chunk, info))| {
            let gpu = c.gpu;
            let mesh = gpu.renderer.create_helper_mesh(&gpu.ctx, &chunk.vertices, &chunk.indices, chunk.lines);
            HelperPart {
                key: HelperKey { mode: Mode::Navmesh, source: hash, group: 0, index: i as u32 },
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
        let m = meshes(&vertices, &indices, &polys);
        assert_eq!(m.len(), 2);
        assert!(!m[0].lines && m[1].lines);
        assert_eq!((m[0].vertices.len(), m[0].polygons), (6, 1));
        assert_eq!(m[1].indices, [0, 1]);
        assert_eq!(Vec3::from(m[0].vertices[0].normal), Vec3::Z);
        assert_eq!((m[0].min, m[0].max), (Vec3::ZERO, Vec3::new(1.0, 1.0, 0.0)));

        // A new mesh before one reaches the limit.
        let polys = vec![polygon(0, 3, 0); MAX_VERTICES / 3];
        let m = meshes(&vertices, &indices, &polys);
        assert_eq!(m.iter().map(|c| c.polygons).collect::<Vec<_>>(), [MAX_VERTICES / 3 - 1, 1]);
    }
}
