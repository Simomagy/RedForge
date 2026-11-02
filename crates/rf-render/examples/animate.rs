//! Renders a ped (or animal) playing a clip of a clip dictionary to PNGs, at a
//! few times of the clip, to see the skinning and animations without the app:
//!
//! ```sh
//! REDFORGE_GAME_DIR=... cargo run --release -p rf-render --example animate -- \
//!     <ycd path or file name> <ped> [clip name or index] [out prefix] [times...]
//! ```
//!
//! Times are in seconds (default: 0, a third and two thirds of the clip); `rest`
//! shows the rest pose. The ped is put together as the animation viewer does
//! (`rf_game::peds`).

use std::path::Path;

use anyhow::{Context, Result, bail};
use rf_formats::formats::ycd;
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};
use rf_render::assets::AssetIndex;
use rf_render::{GpuContext, GpuModel, GpuTexture, ModelOptions, NoTextures, OrbitCamera, Pose, PosedModel, Renderer};

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [wanted, ped_name, rest @ ..] = &args[..] else {
        bail!("usage: animate <ycd> <ped> [clip] [out prefix] [times...]")
    };
    let clip_arg = rest.first().map(String::as_str).unwrap_or("0");
    let out = rest.get(1).map(String::as_str).unwrap_or("animate");

    let game = std::env::var("REDFORGE_GAME_DIR").context("set REDFORGE_GAME_DIR")?;
    let strings = HashIndex::new();
    strings.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt"))?;
    let fs = Rpf8FileSystem::open(Path::new(&game), Rpf8Keys::from_game_folder(Path::new(&game))?, &strings)?;
    let index = AssetIndex::new(&fs);
    let gd = rf_game::GameData::load(&fs, &strings);

    let file = fs
        .find(wanted)
        .or_else(|| index.by_name(wanted).first().copied())
        .with_context(|| format!("{wanted} not found"))?;
    let dict = ycd::read(&fs.extract_loose(file)?, &strings)?;
    let clips = dict.clips();
    let clip = match clip_arg.parse::<usize>() {
        Ok(i) => clips.get(i).copied(),
        Err(_) => clips.iter().copied().find(|c| c.name.as_deref().is_some_and(|n| n.contains(clip_arg))),
    }
    .with_context(|| format!("no clip {clip_arg}; there are {}", clips.len()))?;
    let anims = dict.anims();
    let duration = clip.duration();
    println!("{}: clip {:?}, {duration:.2} s", fs.path(file), clip.name);

    let peds = rf_game::peds::Peds::load(&fs, &strings, &gd);
    let def = peds.definition(&fs, &strings, &gd, ped_name).with_context(|| format!("no ped {ped_name}"))?;
    let ped = rf_game::peds::load_ped(&fs, &strings, &gd, &def)?;
    println!("{}: {} parts, missing {:?}", ped.name, ped.parts.len(), ped.missing);

    let ctx = pollster::block_on(GpuContext::headless())?;
    let renderer = Renderer::new(&ctx);
    let mut pose = Pose::default();
    if let Some(s) = &ped.skeleton {
        pose.attach(s);
    }
    let mut models = Vec::new();
    for part in &ped.parts {
        pose.attach_drawable(&part.drawable);
        let upload = |t: &Option<rf_formats::formats::ytd::TextureBase>| {
            t.as_ref().and_then(|t| GpuTexture::upload(&ctx, t).ok()).map(std::sync::Arc::new)
        };
        let options = ModelOptions {
            skin: true,
            overrides: [&part.textures[0], &part.textures[1], &part.textures[2]].map(upload),
            palette: upload(&part.palette).map(|p| (p, part.tints)),
        };
        models.push(GpuModel::from_drawable_with(
            &ctx,
            &renderer,
            &part.name,
            &part.drawable,
            &strings,
            &mut NoTextures,
            &options,
        ));
    }
    // In the rest pose, skinning should leave the vertices where they are: the
    // bones the models' rigs use whose matrices aren't the identity.
    let identity = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
    let used: std::collections::BTreeSet<u16> = models.iter().flat_map(|m| m.rigs.iter().flatten().copied()).collect();
    let mut off = Vec::new();
    for (&tag, rows) in used.iter().zip(pose.skin_rows(&used.iter().copied().collect::<Vec<_>>())) {
        let d = (0..12).map(|i| (rows[i / 4][i % 4] - identity[i / 4][i % 4]).abs()).fold(0.0f32, f32::max);
        if d > 1e-3 {
            let name = pose.bone_index(tag).and_then(|i| pose.bones()[i].name.clone());
            off.push(format!("{tag} {name:?} {d:.3}"));
        }
    }
    println!("{} bones, {} used; not identity in the rest pose: {off:?}", pose.bones().len(), used.len());

    let mut camera = OrbitCamera::default();
    let (mut min, mut max) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
    for m in &models {
        min = min.min(m.min.into());
        max = max.max(m.max.into());
    }
    camera.frame(min, max);
    if let Some(yaw) = std::env::var("YAW").ok().and_then(|y| y.parse::<f32>().ok()) {
        camera.yaw = yaw.to_radians();
    }
    let settings = rf_render::ViewSettings::default();
    let targets = renderer.create_targets(&ctx, 960, 960);
    let posed: Vec<PosedModel> = models.iter().map(|m| PosedModel { model: m, lod: m.highest_lod() }).collect();

    let times: Vec<Option<f32>> = if rest.len() > 2 {
        rest[2..].iter().map(|t| t.parse().ok()).collect()
    } else {
        vec![None, Some(0.0), Some(duration / 3.0), Some(duration * 2.0 / 3.0)]
    };
    for (i, t) in times.iter().enumerate() {
        match t {
            Some(t) => pose.apply_clip(clip, |h| anims.get(&h).copied(), *t, true),
            None => {
                pose.reset();
                pose.update();
            }
        }
        let stats = renderer.render_posed_orbit(&ctx, &targets, &camera, &settings, &pose, &posed);
        let pixels = renderer.read_output(&ctx, &targets);
        let name = format!("{out}_{i}.png");
        image::save_buffer(&name, &pixels, targets.width, targets.height, image::ColorType::Rgba8)?;
        println!("{name}: t = {t:?}, {} draws", stats.draw_calls);
    }
    Ok(())
}
