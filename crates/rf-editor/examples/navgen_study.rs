//! Issue #6: how the game's navmeshes relate to their collisions. Generates
//! navmesh from the static collisions of a few places with different settings
//! and compares it with the game's (`REDFORGE_GAME_DIR` must be the game).
//!
//! `cargo run --release -p rf-editor --example navgen_study [grid|sizes|edges|flags]`

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use glam::{Vec2, Vec3};
use rf_editor::navgen::collision::{self, Triangle, WorldBox};
use rf_editor::navgen::{self, Settings, signed_area};
use rf_formats::formats::ynv::{self, poly_flags};
use rf_formats::hash::{HashIndex, joaat};
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};
use rf_game::GameData;

const GRID_CELL: f32 = 150.0;
const GRID_MIN: f32 = -15900.0;

struct Game {
    fs: Rpf8FileSystem,
    data: Arc<GameData>,
}

fn game() -> Game {
    let dir = PathBuf::from(std::env::var_os("REDFORGE_GAME_DIR").expect("REDFORGE_GAME_DIR"));
    let strings = HashIndex::new();
    strings.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt")).unwrap();
    let fs = Rpf8FileSystem::open(&dir, Rpf8Keys::from_game_folder(&dir).unwrap(), &strings).unwrap();
    let data = Arc::new(GameData::load(&fs, &strings));
    Game { fs, data }
}

/// A game navmesh's polygons in 2D, with their heights and flags.
struct GamePoly {
    corners: Vec<Vec3>,
    flags: u64,
}

fn game_polygons(g: &Game, area: WorldBox) -> (Vec<GamePoly>, Vec<(ynv::Navmesh, u32)>) {
    let (lo, hi) = (
        (area.0.truncate() - Vec2::splat(GRID_MIN)) / GRID_CELL,
        (area.1.truncate() - Vec2::splat(GRID_MIN)) / GRID_CELL,
    );
    let mut out = Vec::new();
    let mut meshes = Vec::new();
    for x in lo.x as i32..=hi.x as i32 {
        for y in lo.y as i32..=hi.y as i32 {
            let name = format!("navmesh[{}][{}]", x * 3, y * 3);
            let Some(file) = g.data.stream_entry("ynv", joaat(&name)) else { continue };
            let nm = ynv::read(&g.fs.extract_loose(file).unwrap()).unwrap();
            let (v, i, p) = (nm.vertex_positions().unwrap(), nm.indices().unwrap(), nm.polygons().unwrap());
            for poly in &p {
                let n = poly.index_count() as usize;
                if n < 3 {
                    continue;
                }
                let corners: Vec<Vec3> = (0..n).map(|k| Vec3::from(poly.vertex(k, &v, &i))).collect();
                out.push(GamePoly { corners, flags: poly.flags() });
            }
            meshes.push((nm, joaat(&name)));
        }
    }
    (out, meshes)
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

/// Polygons by 2 m buckets, for point lookups.
struct Buckets {
    min: Vec2,
    w: usize,
    cells: Vec<Vec<usize>>,
}

impl Buckets {
    const SIZE: f32 = 2.0;
    fn new(polys: &[&[Vec3]], area: WorldBox) -> Self {
        let min = area.0.truncate();
        let w = ((area.1.x - area.0.x) / Self::SIZE).ceil() as usize + 1;
        let h = ((area.1.y - area.0.y) / Self::SIZE).ceil() as usize + 1;
        let mut cells = vec![Vec::new(); w * h];
        for (i, c) in polys.iter().enumerate() {
            let (lo, hi) =
                c.iter().fold((Vec2::MAX, Vec2::MIN), |(l, h), v| (l.min(v.truncate()), h.max(v.truncate())));
            let (lo, hi) = (((lo - min) / Self::SIZE).floor(), ((hi - min) / Self::SIZE).floor());
            for y in lo.y.max(0.0) as usize..=(hi.y.max(0.0) as usize).min(h - 1) {
                for x in lo.x.max(0.0) as usize..=(hi.x.max(0.0) as usize).min(w - 1) {
                    cells[y * w + x].push(i);
                }
            }
        }
        Self { min, w, cells }
    }
    fn at(&self, p: Vec2) -> &[usize] {
        let q = ((p - self.min) / Self::SIZE).floor();
        if q.x < 0.0 || q.y < 0.0 {
            return &[];
        }
        self.cells.get(q.y as usize * self.w + q.x as usize).map_or(&[], |v| v.as_slice())
    }
}

/// Where both, only the game, or only the generated navmesh are walkable, on
/// a 0.5 m grid inside the area (2 m in from its sides).
fn compare(game: &[&[Vec3]], made: &[&[Vec3]], area: WorldBox) -> (usize, usize, usize) {
    let (gb, mb) = (Buckets::new(game, area), Buckets::new(made, area));
    let (mut both, mut only_game, mut only_made) = (0, 0, 0);
    let mut y = area.0.y + 2.0;
    while y < area.1.y - 2.0 {
        let mut x = area.0.x + 2.0;
        while x < area.1.x - 2.0 {
            let p = Vec2::new(x, y);
            let g = gb.at(p).iter().any(|&i| inside(game[i], p));
            let m = mb.at(p).iter().any(|&i| inside(made[i], p));
            match (g, m) {
                (true, true) => both += 1,
                (true, false) => only_game += 1,
                (false, true) => only_made += 1,
                _ => {}
            }
            x += 0.5;
        }
        y += 0.5;
    }
    (both, only_game, only_made)
}

struct Site {
    name: String,
    area: WorldBox,
    triangles: Vec<Triangle>,
    game: Vec<GamePoly>,
    meshes: Vec<(ynv::Navmesh, u32)>,
}

fn sites(g: &Game) -> Vec<Site> {
    // Towns and the country around them: 80 m boxes.
    let places: [(&str, f32, f32); 8] = [
        ("Valentine street", -290.0, 790.0),
        ("Valentine fields", -150.0, 1000.0),
        ("Rhodes", 1290.0, -1300.0),
        ("Rhodes country", 1500.0, -1100.0),
        ("Strawberry", -1790.0, -400.0),
        ("Big Valley slope", -1900.0, -700.0),
        ("Blackwater", -800.0, -1290.0),
        ("Saint Denis", 2650.0, -1250.0),
    ];
    let mut out = Vec::new();
    for (name, x, y) in places {
        let half = 40.0;
        let flat = (Vec3::new(x - half, y - half, -1000.0), Vec3::new(x + half, y + half, 2000.0));
        let (all, meshes) = game_polygons(g, flat);
        // The game's polygons in the box, outdoors.
        let game: Vec<GamePoly> = all
            .into_iter()
            .filter(|p| {
                p.corners.iter().any(|c| c.x >= flat.0.x && c.x <= flat.1.x && c.y >= flat.0.y && c.y <= flat.1.y)
            })
            .filter(|p| p.flags & poly_flags::INTERIOR == 0)
            .collect();
        if game.is_empty() {
            eprintln!("{name}: no navmesh");
            continue;
        }
        let (zmin, zmax) =
            game.iter().flat_map(|p| p.corners.iter()).fold((f32::MAX, f32::MIN), |(l, h), c| (l.min(c.z), h.max(c.z)));
        let area = (Vec3::new(x - half, y - half, zmin - 5.0), Vec3::new(x + half, y + half, zmax + 10.0));
        let mut triangles = Vec::new();
        for f in collision::files_in(&g.data, area, &[]) {
            collision::read_triangles(&g.fs, f, area, &mut triangles);
        }
        out.push(Site { name: name.into(), area, triangles, game, meshes });
    }
    out
}

fn run(site: &Site, s: &Settings) -> (f32, f32, f32, usize, f32) {
    let made = navgen::generate(&site.triangles, site.area, s, &|_| 1).unwrap();
    let game: Vec<&[Vec3]> = site.game.iter().map(|p| p.corners.as_slice()).collect();
    let mine: Vec<&[Vec3]> = made.iter().map(|p| p.corners.as_slice()).collect();
    let (both, g, m) = compare(&game, &mine, site.area);
    let iou = both as f32 / (both + g + m).max(1) as f32;
    let recall = both as f32 / (both + g).max(1) as f32;
    let precision = both as f32 / (both + m).max(1) as f32;
    let mean_area = made.iter().map(|p| signed_area(&p.corners)).sum::<f32>() / made.len().max(1) as f32;
    (iou, recall, precision, made.len(), mean_area)
}

fn main() {
    let what = std::env::args().nth(1).unwrap_or_else(|| "grid".into());
    let g = game();
    let sites = sites(&g);
    for s in &sites {
        let up = s.triangles.iter().filter(|t| t.normal().z > 0.5).count();
        let down = s.triangles.iter().filter(|t| t.normal().z < -0.5).count();
        let area: f32 = s.game.iter().map(|p| signed_area(&p.corners).abs()).sum::<f32>() / s.game.len() as f32;
        let ccw = s.game.iter().filter(|p| signed_area(&p.corners) > 0.0).count();
        println!(
            "{}: {} triangles ({up} facing up, {down} down), game: {} polygons, mean {area:.1} m², {ccw} counter-clockwise",
            s.name,
            s.triangles.len(),
            s.game.len()
        );
    }
    match what.as_str() {
        "grid" => {
            println!("radius climb slope | IoU recall precision per site, then the mean");
            for radius in [0.2, 0.3, 0.4, 0.5] {
                for climb in [0.35, 0.55, 0.75] {
                    for slope in [40.0, 50.0, 60.0] {
                        let s =
                            Settings { agent_radius: radius, max_climb: climb, max_slope: slope, ..Default::default() };
                        let r: Vec<(f32, f32, f32, usize, f32)> = sites.iter().map(|site| run(site, &s)).collect();
                        let mean = r.iter().map(|x| x.0).sum::<f32>() / r.len() as f32;
                        let cells: Vec<String> =
                            r.iter().map(|x| format!("{:.2}/{:.2}/{:.2}", x.0, x.1, x.2)).collect();
                        println!("{radius:.2} {climb:.2} {slope:.0} | {} | mean IoU {mean:.3}", cells.join(" "));
                    }
                }
            }
        }
        "sizes" => {
            for edge in [0.0, 6.0, 12.0, 24.0] {
                for err in [0.3, 1.0, 1.3, 2.0] {
                    let s = Settings { max_edge_len: edge, max_error: err, ..Default::default() };
                    let r: Vec<(f32, f32, f32, usize, f32)> = sites.iter().map(|site| run(site, &s)).collect();
                    let cells: Vec<String> =
                        r.iter().map(|x| format!("{} polys {:.1} m² IoU {:.2}", x.3, x.4, x.0)).collect();
                    println!("edge {edge} error {err} | {}", cells.join(" | "));
                }
            }
        }
        "fine" => {
            for radius in [0.2, 0.25, 0.3, 0.35, 0.4] {
                let s = Settings { agent_radius: radius, cell_size: 0.1, cell_height: 0.05, ..Default::default() };
                let r: Vec<(f32, f32, f32, usize, f32)> = sites.iter().map(|site| run(site, &s)).collect();
                let mean = r.iter().map(|x| x.0).sum::<f32>() / r.len() as f32;
                let cells: Vec<String> = r.iter().map(|x| format!("{:.2}/{:.2}/{:.2}", x.0, x.1, x.2)).collect();
                println!("radius {radius:.2} (cells 0.1) | {} | mean IoU {mean:.3}", cells.join(" "));
            }
        }
        "edges" => edges(&sites),
        "flags" => flags(&g, &sites),
        _ => eprintln!("grid, sizes, edges or flags"),
    }
}

/// What the edges' "space around vertex" and "space beyond edge" hold:
/// their values against how far the vertex is from the nearest open edge.
fn edges(sites: &[Site]) {
    let mut around: HashMap<u32, (usize, f32)> = HashMap::new();
    let mut beyond: HashMap<u32, usize> = HashMap::new();
    let mut adjacency: HashMap<u32, usize> = HashMap::new();
    for site in sites {
        for (nm, _) in &site.meshes {
            let (v, i, p, e) =
                (nm.vertex_positions().unwrap(), nm.indices().unwrap(), nm.polygons().unwrap(), nm.edges().unwrap());
            // Open edges' segments, for distances.
            let mut open: Vec<(Vec2, Vec2)> = Vec::new();
            for poly in &p {
                let n = poly.index_count() as usize;
                for k in 0..n {
                    let edge = e[poly.index_id as usize + k];
                    if edge.poly_id1() == 0x7FFF {
                        let a = Vec3::from(poly.vertex(k, &v, &i)).truncate();
                        let b = Vec3::from(poly.vertex((k + 1) % n, &v, &i)).truncate();
                        open.push((a, b));
                    }
                }
            }
            for poly in p.iter().filter(|p| p.index_count() >= 3) {
                let n = poly.index_count() as usize;
                for k in 0..n {
                    let edge = e[poly.index_id as usize + k];
                    let a = Vec3::from(poly.vertex(k, &v, &i));
                    if a.x < site.area.0.x || a.x > site.area.1.x || a.y < site.area.0.y || a.y > site.area.1.y {
                        continue;
                    }
                    let d = open
                        .iter()
                        .filter(|(s, t)| (s - a.truncate()).length() < 20.0 || (t - a.truncate()).length() < 20.0)
                        .map(|&(s, t)| segment_distance(a.truncate(), s, t))
                        .fold(20.0f32, f32::min);
                    let x = around.entry(edge.space_around_vertex()).or_default();
                    x.0 += 1;
                    x.1 += d;
                    *beyond.entry(edge.space_beyond_edge()).or_default() += 1;
                    *adjacency.entry(edge.adjacency_type()).or_default() += 1;
                }
            }
        }
    }
    let mut a: Vec<_> = around.into_iter().collect();
    a.sort_by_key(|x| x.0);
    println!("space around vertex: count, mean distance to an open edge (m)");
    for (k, (n, d)) in a {
        println!("  {k:2}: {n:6} {:.2}", d / n as f32);
    }
    let mut b: Vec<_> = beyond.into_iter().collect();
    b.sort();
    println!("space beyond edge: {b:?}");
    let mut c: Vec<_> = adjacency.into_iter().collect();
    c.sort();
    println!("adjacency type: {c:?}");
}

fn segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
    (a + ab * t - p).length()
}

/// Which polygon flags go with which collision materials: each game
/// polygon's middle, straight down onto the nearest collision triangle.
fn flags(g: &Game, sites: &[Site]) {
    let materials = rf_game::materials::Materials::load(&g.fs);
    let names: Vec<(u64, &str)> = ynv::POLY_FLAG_NAMES.to_vec();
    let mut table: HashMap<String, (usize, HashMap<&str, usize>)> = HashMap::new();
    let mut stairs_material = (0, 0);
    for site in sites {
        for p in &site.game {
            let n = p.corners.len() as f32;
            let c = p.corners.iter().sum::<Vec3>() / n;
            let mut best: Option<(f32, &Triangle)> = None;
            for t in &site.triangles {
                if t.normal().z < 0.2 {
                    continue;
                }
                if let Some(z) = height_at(t, c.truncate())
                    && (z - c.z).abs() < 1.0
                    && best.is_none_or(|(bz, _)| (z - c.z).abs() < (bz - c.z).abs())
                {
                    best = Some((z, t));
                }
            }
            let Some((_, t)) = best else { continue };
            let m = materials.name(t.material.data1.hash());
            let e = table.entry(m).or_default();
            e.0 += 1;
            for (bit, name) in &names {
                if p.flags & bit != 0 {
                    *e.1.entry(name).or_default() += 1;
                }
            }
            if p.flags & poly_flags::STAIRS != 0 {
                stairs_material.0 += 1;
                if t.material.data2.flags() & collision::FLAG_STAIRS != 0 {
                    stairs_material.1 += 1;
                }
            }
        }
    }
    let mut rows: Vec<_> = table.into_iter().collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.1.0));
    for (m, (n, f)) in rows.iter().take(30) {
        let mut f: Vec<_> = f
            .iter()
            .filter(|(_, c)| **c * 10 >= *n)
            .map(|(k, c)| format!("{k} {:.0}%", *c as f32 * 100.0 / *n as f32))
            .collect();
        f.sort();
        println!("{m:32} {n:6} | {}", f.join(", "));
    }
    println!("stairs polygons on a FLAG_STAIRS material: {} of {}", stairs_material.1, stairs_material.0);
}

fn height_at(t: &Triangle, at: Vec2) -> Option<f32> {
    let [a, b, c] = t.v;
    let (v0, v1, v2) = ((b - a).truncate(), (c - a).truncate(), at - a.truncate());
    let d = v0.perp_dot(v1);
    if d.abs() < 1e-9 {
        return None;
    }
    let (u, v) = (v2.perp_dot(v1) / d, v0.perp_dot(v2) / d);
    (u >= 0.0 && v >= 0.0 && u + v <= 1.0).then_some(a.z + u * (b.z - a.z) + v * (c.z - a.z))
}
