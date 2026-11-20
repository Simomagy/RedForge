//! `rf amv-bake`: an ambient mask volume for an interior, baked from the
//! models next to its files.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use glam::Vec3;
use rf_editor::amv::{self, Mesh, Settings};
use rf_editor::interior;
use rf_formats::formats::databag::DataBag;
use rf_formats::formats::meta::Meta;
use rf_formats::hash::HashIndex;

pub struct Options {
    pub ytyp: PathBuf,
    pub ymap: PathBuf,
    /// More models than those found next to the ytyp and the ymap.
    pub models: Vec<PathBuf>,
    pub out: Option<PathBuf>,
    pub zone: Option<u32>,
    pub cell: f32,
    pub rays: u32,
    pub centre: Option<Vec3>,
    pub half: Option<Vec3>,
    pub heading: Option<f32>,
    pub cells: Option<[u32; 3]>,
}

/// `x,y,z` as three numbers.
pub fn triple<T: std::str::FromStr>(text: &str) -> Result<[T; 3], String> {
    let parts: Vec<_> = text.split(',').map(|p| p.trim().parse::<T>()).collect();
    match parts.as_slice() {
        [Ok(_), Ok(_), Ok(_)] => {
            let mut it = parts.into_iter().flatten();
            Ok([(); 3].map(|_| it.next().expect("three numbers")))
        }
        _ => Err("three numbers are needed, as x,y,z".into()),
    }
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
    let (mlo, name) = (interior.mlo, interior.name());

    // Every ydr next to the two files, and those given.
    let folders = [o.ytyp.parent(), o.ymap.parent()].map(|f| f.map(Path::to_owned).unwrap_or_default());
    let beside = folders
        .iter()
        .filter_map(|f| std::fs::read_dir(if f.as_os_str().is_empty() { Path::new(".") } else { f }).ok());
    let files = amv::model_files(beside.flatten().flatten().map(|e| e.path()).chain(o.models.iter().cloned()));
    let (triangles, used) = interior.occluders(&files, hashes).map_err(anyhow::Error::msg)?;
    for (path, count) in &used {
        println!("{}: {count} triangles", path.display());
    }
    if triangles.is_empty() {
        bail!("no model of the interior was found next to its files (its ydr files, named as its archetypes are)");
    }
    let mesh = Mesh::new(triangles);

    let mut volume = interior.volume(o.cell).context("the interior's first room has no size")?;
    volume.centre = o.centre.unwrap_or(volume.centre);
    volume.half = o.half.unwrap_or(volume.half);
    volume.heading = o.heading.map_or(volume.heading, f32::to_radians);
    volume.cells = o.cells.unwrap_or(volume.cells);
    if volume.half.min_element() <= 0.0 || volume.cells.contains(&0) {
        bail!("the volume has no size: {volume:?}");
    }

    let baked = amv::bake(&volume, &mesh, &Settings { rays: o.rays, ..Default::default() });
    let mean = baked.values.iter().flatten().sum::<f32>() / (baked.values.len() * 6) as f32;
    println!(
        "volume at {:.3} {:.3} {:.3}, half {:.3} {:.3} {:.3}, heading {:.2}°, {}x{}x{} cells, {} triangles: {:.1}% of the sky seen",
        volume.centre.x,
        volume.centre.y,
        volume.centre.z,
        volume.half.x,
        volume.half.y,
        volume.half.z,
        volume.heading.to_degrees(),
        volume.cells[0],
        volume.cells[1],
        volume.cells[2],
        mesh.len(),
        mean * 100.0
    );

    let zone = o.zone.unwrap_or(amv::FIRST_FREE_ZONE);
    let uuid = amv::uuid_for(name, 0);
    let item = amv::zone_item(&volume, uuid).context("no schema for an ambient mask volume")?;
    let dir = o.out.clone().unwrap_or_else(|| folders[0].clone());
    std::fs::create_dir_all(&dir)?;
    let base = dir.join(amv::zone_name(zone));
    std::fs::write(base.with_extension("ymt"), amv::zone_file(zone, vec![item])?)?;
    std::fs::write(base.with_extension("ytd"), amv::zone_textures(baked.textures(uuid).into()))?;
    println!("{} -> {}.ymt, .ytd", o.ytyp.display(), base.display());

    // What else the interior's lighting lacks, next to the game's own.
    for note in amv::lighting_notes(&hashes.name(name), &interior::lighting(mlo)) {
        println!("{note}");
    }
    Ok(())
}
