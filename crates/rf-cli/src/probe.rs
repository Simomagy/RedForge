//! `rf probe-bake`: a reflection probe for an interior, drawn from the models
//! next to its files.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use glam::Vec3;
use rf_editor::amv;
use rf_editor::placement::Placement;
use rf_editor::probe::{self, SIZES, Scene};
use rf_formats::formats::databag::DataBag;
use rf_formats::formats::meta::Meta;
use rf_formats::hash::HashIndex;

pub struct Options {
    pub ytyp: PathBuf,
    pub ymap: PathBuf,
    pub models: Vec<PathBuf>,
    pub out: Option<PathBuf>,
    /// Where it is drawn from, in the interior's space (default: the middle of its first room).
    pub from: Option<Vec3>,
    pub guid: Option<u64>,
}

fn root(path: &Path) -> Result<DataBag> {
    let data = std::fs::read(path).with_context(|| path.display().to_string())?;
    Meta::read(&data)?.root.with_context(|| format!("{}: nothing in it", path.display()))
}

pub fn run(o: &Options, hashes: &HashIndex) -> Result<()> {
    let (ytyps, maps) = ([root(&o.ytyp)?], [root(&o.ymap)?]);
    let Some(interior) = amv::placed(&ytyps, &maps).into_iter().next() else {
        bail!("{} places no interior of {}", o.ymap.display(), o.ytyp.display());
    };
    let folders = [o.ytyp.parent(), o.ymap.parent()].map(|f| f.map(Path::to_owned).unwrap_or_default());
    let beside = folders
        .iter()
        .filter_map(|f| std::fs::read_dir(if f.as_os_str().is_empty() { Path::new(".") } else { f }).ok());
    let files = amv::model_files(beside.flatten().flatten().map(|e| e.path()).chain(o.models.iter().cloned()));
    let scene = Scene::of_interior(&interior, &files, hashes).map_err(anyhow::Error::msg)?;
    if scene.is_empty() {
        bail!("no model of the interior was found next to its files (its ydr files, named as its archetypes are)");
    }
    let dir = o.out.clone().unwrap_or_else(|| folders[0].clone());
    std::fs::create_dir_all(&dir)?;
    if o.from.is_none() && o.guid.is_none() {
        // As an export does it: from the middle of the interior, and with its ytyp given the probe.
        let baked = probe::bake_interior(&interior, &files, hashes)
            .map_err(anyhow::Error::msg)?
            .context("the interior's first room has no size")?;
        for (name, data) in &baked.dictionaries {
            std::fs::write(dir.join(name), data)?;
            println!("{}", dir.join(name).display());
        }
        if probe::has_probe(interior.mlo) {
            println!("{} has a probe already: it is left as it is", o.ytyp.display());
            return Ok(());
        }
        let name = interior.name();
        let mut ytyp = ytyps[0].clone();
        let mlos = rf_editor::interior::archetypes_mut(&mut ytyp).into_iter().flatten().flatten();
        for mlo in mlos.filter(|m| m.hash("name") == name) {
            probe::add_to_room(mlo, baked.data.clone());
        }
        // Never over the ytyp it was read from.
        let file = o.ytyp.file_name().unwrap_or_default();
        let to = if dir.join(file) == o.ytyp { o.ytyp.with_extension("probe.ytyp") } else { dir.join(file) };
        std::fs::write(&to, Meta::new(Some(ytyp)).save()?)?;
        println!("{} -> {} (with the probe in its room)", o.ytyp.display(), to.display());
        return Ok(());
    }

    let whole =
        interior.mlo.bags("rooms").and_then(|r| r.iter().flatten().next()).context("the interior has no room")?;
    let (min, max) = (Vec3::from(whole.vec3("bbMin")), Vec3::from(whole.vec3("bbMax")));
    let from = o.from.unwrap_or((min + max) * 0.5);
    let placed = Placement::of_entity(interior.instance);
    let origin = placed.position + placed.orientation * from;
    let guid = o.guid.unwrap_or_else(|| amv::uuid_for(interior.name(), 0));
    let name = probe::texture_name(guid, interior.name());
    let mut drawn = probe::draw(&scene, origin, SIZES[0].0);
    for (size, suffix) in SIZES {
        while drawn.size > size {
            drawn = drawn.halved();
        }
        std::fs::write(dir.join(format!("{name}{suffix}.ytd")), probe::dictionary(drawn.textures(name)))?;
    }
    println!(
        "probe {guid:#018X} drawn from {:.3} {:.3} {:.3} -> {}/{name}.ytd, _lo, _hi, _ul",
        origin.x,
        origin.y,
        origin.z,
        dir.display()
    );
    Ok(())
}
