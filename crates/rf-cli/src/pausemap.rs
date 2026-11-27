//! `rf pausemap-export`: what's drawn on the pause map (a project's, or a
//! Map Editor web app or Aether Cartography project) drawn into the game's
//! tiles, written as a RedM resource.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rf_editor::pausemap::fonts::{Font, file_font_name};
use rf_editor::pausemap::project_file::{self, PauseMapData};
use rf_editor::pausemap::{Assets, Library, export};
use rf_editor::{Project, redm};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::Rpf8FileSystem;

pub struct Options {
    /// An `.rfproj`, a `map-project.json` or an `aether-project.json`.
    pub map: PathBuf,
    /// The resource's folder (its name is the folder's).
    pub out: PathBuf,
    /// Font files labels are lettered in, besides the bundled ones.
    pub fonts: Vec<PathBuf>,
}

fn read_map(path: &Path, assets: &Assets) -> Result<PauseMapData> {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("rfproj")) {
        let p = Project::load(path).map_err(anyhow::Error::msg)?;
        let Some(json) = p.pause_map.as_ref() else { bail!("{}: nothing is drawn on the pause map", path.display()) };
        return Ok(project_file::read(json));
    }
    let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    let opened = project_file::open(&text, &assets.fonts.names()).map_err(anyhow::Error::msg)?;
    for n in &opened.notes {
        println!("note: {n}");
    }
    Ok(opened.data)
}

pub fn run(fs: &Rpf8FileSystem, strings: &HashIndex, o: &Options) -> Result<()> {
    let mut assets = Assets::default();
    // The game's own fonts, as the editor has them.
    if let Some(f) = rf_editor::pausemap::gfxfont::LIBRARIES.iter().find_map(|p| fs.find(p)) {
        let data = fs.extract(f).context("the game's font library")?;
        assets.fonts.add_game_fonts(&data).map_err(anyhow::Error::msg)?;
    }
    for f in &o.fonts {
        let data = std::fs::read(f).with_context(|| f.display().to_string())?;
        let font = Font::from_file(&file_font_name(f), &f.display().to_string(), data).map_err(anyhow::Error::msg)?;
        assets.fonts.add(font);
    }
    let data = read_map(&o.map, &assets)?;
    for (id, bytes) in &data.pictures {
        if let Err(e) = assets.pictures.insert(id, bytes.to_vec()) {
            println!("note: picture {id}: {e}");
        }
    }
    let t = std::time::Instant::now();
    let lib = Library::load(fs);
    for (path, why) in &lib.failed {
        log::warn!("{path}: {why}");
    }
    let (files, plan) = export::build(&lib, &data.objects, &assets).map_err(anyhow::Error::msg)?;
    println!("{} objects drawn into {} tiles in {:.2?}", data.objects.len(), files.len(), t.elapsed());
    if !plan.skipped.is_empty() {
        println!("where the game has no tile, so what's drawn doesn't show: {}", plan.skipped.join(", "));
    }
    let info: BTreeMap<String, String> =
        [("description".to_owned(), "Pause map tiles drawn with RedForge.".to_owned())].into();
    let done = redm::export(&o.out, &[], &info, &[], rf_editor::amv::FIRST_FREE_ZONE, strings, &files)
        .map_err(anyhow::Error::msg)?;
    println!("{}: {} files in stream", o.out.display(), done.written.len());
    Ok(())
}
