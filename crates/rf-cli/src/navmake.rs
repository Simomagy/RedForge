//! `rf navmesh-make`: a box of a navmesh cell made again from the collision
//! there (the game's, and a project's or a resource's on top), the navmeshes
//! that changed written out, ready for a resource's `stream`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use glam::{Vec2, Vec3};
use rf_editor::Editor;
use rf_editor::navgen::Settings;
use rf_editor::navgen::stitch::Region;
use rf_formats::hash::{HashIndex, joaat};
use rf_formats::rpf8::Rpf8FileSystem;
use rf_game::GameData;

pub struct Options {
    /// An `.rfproj`, or a folder of game files (a resource's `stream`).
    pub project: Option<PathBuf>,
    /// The box seen from above: min x, min y, max x, max y.
    pub area: [f32; 4],
    /// The heights to look for ground in (default: around the navmesh's
    /// polygons in the box).
    pub heights: Option<[f32; 2]>,
    pub out: PathBuf,
    pub settings: Settings,
}

/// "a,b,c,d" as four numbers.
pub fn four(s: &str) -> std::result::Result<[f32; 4], String> {
    let v: Vec<f32> =
        s.split(',').map(|x| x.trim().parse::<f32>().map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
    v.try_into().map_err(|_| "four numbers: min x, min y, max x, max y".to_string())
}

/// "a,b" as two numbers.
pub fn two(s: &str) -> std::result::Result<[f32; 2], String> {
    let v: Vec<f32> =
        s.split(',').map(|x| x.trim().parse::<f32>().map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
    v.try_into().map_err(|_| "two numbers: lowest, highest".to_string())
}

pub fn run(fs: Rpf8FileSystem, strings: HashIndex, o: &Options) -> Result<()> {
    let (fs, strings) = (Arc::new(fs), Arc::new(strings));
    let t = Instant::now();
    let data = Arc::new(GameData::load(&fs, &strings));
    log::info!("game data in {:.2?}", t.elapsed());
    let mut ed = Editor::new(fs, strings, data);
    if let Some(p) = &o.project {
        let problems = if p.is_dir() {
            ed.add_project_folder(p)
        } else {
            ed.open_project(p).map_err(anyhow::Error::msg).with_context(|| p.display().to_string())?
        };
        for e in problems {
            log::warn!("{e}");
        }
    }

    // The world around the box, for the interiors and the objects there.
    let [x0, y0, x1, y1] = o.area;
    let centre = Vec3::new((x0 + x1) * 0.5, (y0 + y1) * 0.5, 0.0);
    let start = Instant::now();
    loop {
        ed.world.update(centre.to_array(), 12.0);
        if ed.world.stats().nodes_loading == 0 {
            ed.world.update(centre.to_array(), 12.0);
            break;
        }
        if start.elapsed() > Duration::from_secs(90) {
            log::warn!("the world around the box is still loading: going on");
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    let (min, max) = (Vec2::new(x0.min(x1), y0.min(y1)), Vec2::new(x0.max(x1), y0.max(y1)));
    let [z_min, z_max] = match o.heights {
        Some(h) => h,
        None => heights_in(&mut ed, min, max).context("no navmesh polygons in the box: give --heights")?,
    };
    let region = Region { min, max, z_min, z_max };
    let t = Instant::now();
    let g = ed.generate_navmesh(region, &o.settings).map_err(anyhow::Error::msg)?;
    let s = &g.stitched;
    println!(
        "{}: {} polygons made from {} collision triangles in {:.2?}; {} removed, {} cut to fit, {} border polygons made again; special links: {} moved, {} lost; the next cells': {} moved, {} lost",
        g.navmeshes.join(", "),
        s.added,
        g.triangles,
        t.elapsed(),
        s.removed,
        s.pieces,
        s.borders,
        s.links_moved,
        s.links_dropped,
        g.neighbour_links,
        g.neighbour_links_lost
    );

    if !g.cells_without_navmesh.is_empty() {
        println!("no navmesh in {}: nothing made there", g.cells_without_navmesh.join(", "));
    }
    std::fs::create_dir_all(&o.out).with_context(|| o.out.display().to_string())?;
    let changed: Vec<(u32, String)> =
        ed.navmeshes().filter(|d| d.is_dirty()).map(|d| (d.name, d.file_name.clone())).collect();
    if changed.is_empty() {
        bail!("nothing changed");
    }
    for (name, file) in changed {
        let path: PathBuf = o.out.join(format!("{file}.ynv"));
        ed.save_navmesh(name, &path).map_err(anyhow::Error::msg)?;
        println!("  {}", display(&path));
    }
    Ok(())
}

/// The heights of the navmesh polygons in a box: from 3 m under the lowest
/// to 12 m over the highest.
fn heights_in(ed: &mut Editor, min: Vec2, max: Vec2) -> Option<[f32; 2]> {
    let (_, file) = rf_editor::navgen::cell_of((min + max) * 0.5);
    let doc = ed.open_navmesh(joaat(&file)).ok()?;
    let v = doc.root.vertex_positions()?;
    let (lo, hi) = v
        .iter()
        .filter(|p| p[0] >= min.x && p[0] <= max.x && p[1] >= min.y && p[1] <= max.y)
        .fold((f32::MAX, f32::MIN), |(l, h), p| (l.min(p[2]), h.max(p[2])));
    (lo <= hi).then_some([lo - 3.0, hi + 12.0])
}

fn display(p: &Path) -> String {
    p.display().to_string()
}
