//! Renders a model from the game to a PNG, to see what the renderer does
//! without the app:
//!
//! ```sh
//! REDFORGE_GAME_DIR=... cargo run --release -p rf-render --example snapshot -- <path or file name> [out.png] [view] [--game-data]
//! ```
//!
//! `view` is `lit` (default), `albedo`, `normals`, `material` or `colour`.
//!
//! Textures come from the texture dictionaries named like the model. With
//! `--game-data`, they come first from the ones the game uses for it (its
//! archetype's dictionary, its GTXD parents and their HD versions, found by
//! `rf_game`), then from those.

use std::path::Path;

use anyhow::{Context, Result, bail};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};
use rf_render::assets::{AssetIndex, DictionaryTextures, read_model, related_dictionaries};
use rf_render::{DebugView, GpuContext, GpuModel, OrbitCamera, Renderer, ViewSettings};

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let all: Vec<String> = std::env::args().skip(1).collect();
    let game_data = all.iter().any(|a| a == "--game-data");
    let args: Vec<&str> = all.iter().filter(|a| !a.starts_with("--")).map(String::as_str).collect();
    let Some(&wanted) = args.first() else {
        bail!("usage: snapshot <path or file name> [out.png] [view] [--game-data]")
    };
    let out = args.get(1).copied().unwrap_or("snapshot.png");
    let debug = match args.get(2).copied() {
        Some("albedo") => DebugView::Albedo,
        Some("normals") => DebugView::Normals,
        Some("material") => DebugView::Material,
        Some("colour") => DebugView::VertexColour,
        _ => DebugView::Lit,
    };

    let game = std::env::var("REDFORGE_GAME_DIR").context("set REDFORGE_GAME_DIR")?;
    let strings = HashIndex::new();
    strings.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt"))?;
    let fs = Rpf8FileSystem::open(Path::new(&game), Rpf8Keys::from_game_folder(Path::new(&game))?, &strings)?;
    let index = AssetIndex::new(&fs);
    let file = fs
        .find(wanted)
        .or_else(|| index.by_name(wanted).first().copied())
        .with_context(|| format!("{wanted} not found"))?;
    let path = fs.path(file);
    let model = read_model(&path, &fs.extract_loose(file)?, &strings)?;
    let Some((name, drawable)) = model.drawables.first() else { bail!("{path} has no drawables") };

    let ctx = pollster::block_on(GpuContext::headless())?;
    let renderer = Renderer::new(&ctx);
    let mut textures = if game_data {
        let gd = rf_game::GameData::load(&fs, &strings);
        // The drawable's archetype, or the file's; then the dictionaries named
        // like the model, for models without one.
        let mut files = gd.txds_for_piece(rf_game::name_hash(name), fs.entry(file).name_hash);
        for f in related_dictionaries(&fs, &index, &path) {
            if !files.contains(&f) {
                files.push(f);
            }
        }
        for f in &files {
            println!("  {}", fs.path(*f));
        }
        DictionaryTextures::from_files(ctx.clone(), &fs, &strings, &files)
    } else {
        DictionaryTextures::for_model(ctx.clone(), &fs, &index, &strings, &path)
    };
    let gpu = GpuModel::from_drawable(&ctx, &renderer, name, drawable, &strings, &mut textures);
    println!("{path}: {} draws, {} texture dictionaries", gpu.draw_count(), textures.dictionaries().len());
    if !gpu.missing_textures.is_empty() {
        println!("missing textures ({}): {}", gpu.missing_textures.len(), gpu.missing_textures.join(", "));
    }

    let mut camera = OrbitCamera::default();
    camera.frame(gpu.min.into(), gpu.max.into());
    let settings = ViewSettings { debug, ..Default::default() };
    let targets = renderer.create_targets(&ctx, 1280, 800);
    renderer.render(&ctx, &targets, &camera, &settings, &[(&gpu, glam::Mat4::IDENTITY)]);
    let pixels = renderer.read_output(&ctx, &targets);
    image::save_buffer(out, &pixels, targets.width, targets.height, image::ColorType::Rgba8)?;
    println!("saved {out}");
    Ok(())
}
