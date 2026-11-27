//! Draws a part of the pause map, the game's tiles with a project's marks,
//! coloured as the map pane colours it, into a PNG:
//!
//! `cargo run --release -p rf-editor --example pausemap_preview -- <map-project.json> x0,y0,x1,y1 <scale> <out.png>`
//!
//! (map units; scale is pixels per map unit). Needs `REDFORGE_GAME_DIR`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rf_editor::pausemap::compose::{compose_tile, tiles_in};
use rf_editor::pausemap::doc::renderable;
use rf_editor::pausemap::families::{Layer, detail, fit_of};
use rf_editor::pausemap::linework::Bounds;
use rf_editor::pausemap::{Assets, Library, Mark, mark_for, project_file};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};

/// A layer's tiles over the rectangle, composed, and its family.
type Composed = (HashMap<(u32, u32), Vec<u8>>, &'static rf_editor::pausemap::Family);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, map, rect, scale, out] = &args[..] else {
        eprintln!("usage: pausemap_preview <map-project.json> x0,y0,x1,y1 <scale> <out.png>");
        std::process::exit(2);
    };
    let r: Vec<f64> = rect.split(',').map(|v| v.parse().unwrap()).collect();
    let scale: f64 = scale.parse().unwrap();
    let dir = PathBuf::from(std::env::var_os("REDFORGE_GAME_DIR").expect("REDFORGE_GAME_DIR"));
    let strings = HashIndex::new();
    strings.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt")).unwrap();
    let fs = Rpf8FileSystem::open(&dir, Rpf8Keys::from_game_folder(&dir).unwrap(), &strings).unwrap();
    let lib = Library::load(&fs);
    let mut assets = Assets::default();
    let data = project_file::open(&std::fs::read_to_string(map).unwrap(), &assets.fonts.names()).unwrap().data;
    for (id, b) in &data.pictures {
        let _ = assets.pictures.insert(id, b.to_vec());
    }
    let shown = renderable(&data.objects);

    // Each layer's tiles over the rectangle, composed.
    let mut layers: HashMap<Layer, Composed> = HashMap::new();
    for layer in Layer::ALL {
        let f = detail(layer);
        let Some(fd) = lib.get(f.name) else { continue };
        let fit = fit_of(f);
        let [ax0, ay0] = fit.apply([r[0], r[1]]);
        let [ax1, ay1] = fit.apply([r[2], r[3]]);
        let marks: Vec<Mark> = shown.iter().filter_map(|o| mark_for(f, o, &assets)).collect();
        let mut tiles = HashMap::new();
        for (row, col) in tiles_in(f, &Bounds { x0: ax0, y0: ay0, x1: ax1, y1: ay1 }) {
            let st = f.stride() as f64;
            let tb = Bounds {
                x0: col as f64 * st,
                y0: row as f64 * st,
                x1: col as f64 * st + f.tile as f64,
                y1: row as f64 * st + f.tile as f64,
            };
            let here: Vec<&Mark> = marks
                .iter()
                .filter(|m| {
                    let b = m.bounds();
                    b.x1 >= tb.x0 && b.x0 <= tb.x1 && b.y1 >= tb.y0 && b.y0 <= tb.y1
                })
                .collect();
            let t = compose_tile(fd, row, col, here)
                .unwrap_or_else(|| vec![fd.stub_value[(row * f.cols + col) as usize]; (f.tile * f.tile) as usize]);
            tiles.insert((row, col), t);
        }
        layers.insert(layer, (tiles, f));
    }
    let sample = |layer: Layer, x: f64, y: f64| -> f64 {
        let Some((tiles, f)) = layers.get(&layer) else { return 255.0 };
        let [px, py] = fit_of(f).apply([x, y]);
        let st = f.stride() as f64;
        let col = ((px - 1.0) / st).floor().clamp(0.0, f.cols as f64 - 1.0) as u32;
        let row = ((py - 1.0) / st).floor().clamp(0.0, f.rows as f64 - 1.0) as u32;
        let Some(t) = tiles.get(&(row, col)) else { return 255.0 };
        let (ix, iy) = ((px - col as f64 * st) as i64, (py - row as f64 * st) as i64);
        if ix < 0 || iy < 0 || ix >= f.tile as i64 || iy >= f.tile as i64 {
            return 255.0;
        }
        t[(iy as usize) * f.tile as usize + ix as usize] as f64
    };
    let (w, h) = (((r[2] - r[0]) * scale) as u32, ((r[3] - r[1]) * scale) as u32);
    let ink = |v: f64, cut: f64, width: f64| ((cut - v) / width + 0.5).clamp(0.0, 1.0);
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        let (mx, my) = (r[0] + (x as f64 + 0.5) / scale, r[1] + (y as f64 + 0.5) / scale);
        let (text, lines, water, relief) = (
            sample(Layer::Text, mx, my),
            sample(Layer::Lines, mx, my),
            sample(Layer::Water, mx, my),
            sample(Layer::Relief, mx, my),
        );
        let paper = [0.827, 0.753, 0.627];
        let ink_c = [0.243, 0.216, 0.188];
        let wet = [0.612, 0.592, 0.522];
        let water_fill = ink(water, 124.0, 3.0);
        let mut a: f64 = ink(text, 68.0, 4.0).max(ink(lines, 64.0, 4.0));
        a = a.max(ink(relief, 58.0, 4.0) * 0.4);
        let mut c = [0.0; 3];
        for i in 0..3 {
            let base = paper[i] + (wet[i] - paper[i]) * water_fill * 0.92;
            c[i] = base + (ink_c[i] - base) * a;
        }
        image::Rgb(c.map(|v| (v * 255.0) as u8))
    });
    img.save(out).unwrap();
    println!("{out}: {w} x {h}");
}
