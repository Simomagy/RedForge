//! `rf amv-bake`: an ambient mask volume for an interior, baked from the
//! models next to its files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use glam::{Mat4, Vec3};
use rf_editor::amv::{self, Mesh, Settings, Volume};
use rf_editor::placement::{Placement, is_mlo_instance};
use rf_editor::{interior, lod};
use rf_formats::formats::XmlContext;
use rf_formats::formats::databag::DataBag;
use rf_formats::formats::meta::Meta;
use rf_formats::formats::ydr;
use rf_formats::hash::{HashIndex, joaat};

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

/// Whether a model isn't what's seen of the interior: the stand-ins drawn only
/// into shadows or reflections (so flagged, or so named: the shadow ones have
/// no flag of their own), which are closed where the interior is open.
fn is_proxy(path: &Path, flags: u32) -> bool {
    const ONLY_IN_A_PASS: u32 = 0x2A00_0000;
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_ascii_lowercase();
    flags & ONLY_IN_A_PASS != 0
        || ["shadowproxy", "shadow_proxy", "ref_proxy", "refproxy", "lightbox"].iter().any(|p| name.contains(p))
}

fn root(path: &Path) -> Result<DataBag> {
    let data = std::fs::read(path).with_context(|| path.display().to_string())?;
    Meta::read(&data)?.root.with_context(|| format!("{}: nothing in it", path.display()))
}

pub fn run(o: &Options, hashes: &HashIndex) -> Result<()> {
    let (ytyp, ymap) = (root(&o.ytyp)?, root(&o.ymap)?);
    let instances: Vec<_> = lod::entities(&ymap).iter().flatten().filter(|e| is_mlo_instance(e)).collect();
    // The interior the ymap places: the ytyp's archetype with rooms that one of its instances names.
    let found = interior::archetypes(&ytyp).iter().flatten().filter(|a| a.bags("rooms").is_some()).find_map(|a| {
        let instance = instances.iter().find(|i| i.hash("archetypeName") == a.hash("name"))?;
        Some((a, *instance))
    });
    let Some((mlo, instance)) = found else {
        bail!("{} places no interior of {}", o.ymap.display(), o.ytyp.display());
    };
    let name = mlo.hash("name");
    let placed = Placement::of_entity(instance);
    let to_world = Mat4::from_rotation_translation(placed.orientation, placed.position);

    // Every ydr next to the two files, by the hash of its name.
    let mut files: HashMap<u32, PathBuf> = HashMap::new();
    let folders = [o.ytyp.parent(), o.ymap.parent()].map(|f| f.map(Path::to_owned).unwrap_or_default());
    let beside = folders
        .iter()
        .filter_map(|f| std::fs::read_dir(if f.as_os_str().is_empty() { Path::new(".") } else { f }).ok());
    for path in beside.flatten().flatten().map(|e| e.path()).chain(o.models.iter().cloned()) {
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ydr"))
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            files.insert(joaat(&stem.to_ascii_lowercase()), path);
        }
    }

    let cx = XmlContext { hashes, folder: None };
    let mut triangles = Vec::new();
    let mut used = Vec::new();
    let mut add = |def: &DataBag, parent: Mat4| -> Result<()> {
        let Some(path) = files.get(&def.hash("archetypeName")) else { return Ok(()) };
        if is_proxy(path, def.u32("flags")) {
            return Ok(());
        }
        let data = std::fs::read(path).with_context(|| path.display().to_string())?;
        let drawable = ydr::read(&data, &cx).with_context(|| path.display().to_string())?;
        let before = triangles.len();
        amv::drawable_triangles(&drawable.base, parent * Placement::of_entity(def).matrix(), &mut triangles);
        used.push((path.clone(), triangles.len() - before));
        Ok(())
    };
    for def in lod::entities(mlo).iter().flatten() {
        add(def, to_world)?;
    }
    for def in lod::entities(&ymap).iter().flatten().filter(|e| !is_mlo_instance(e)) {
        add(def, Mat4::IDENTITY)?;
    }
    for (path, count) in &used {
        println!("{}: {count} triangles", path.display());
    }
    if triangles.is_empty() {
        bail!("no model of the interior was found next to its files (its ydr files, named as its archetypes are)");
    }
    let mesh = Mesh::new(triangles);

    let rooms = mlo.bags("rooms").unwrap_or_default();
    let Some(whole) = rooms.iter().flatten().next() else { bail!("the interior has no room") };
    let (min, max) = (Vec3::from(whole.vec3("bbMin")), Vec3::from(whole.vec3("bbMax")));
    let mut volume = Volume::of_interior(placed.position, placed.orientation, min, max, o.cell);
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

    let zone = o.zone.unwrap_or_else(|| amv::zone_for(name));
    let uuid = amv::uuid_for(name, 0);
    let item = amv::zone_item(&volume, uuid).context("no schema for an ambient mask volume")?;
    let dir = o.out.clone().unwrap_or_else(|| folders[0].clone());
    std::fs::create_dir_all(&dir)?;
    let base = dir.join(amv::zone_name(zone));
    std::fs::write(base.with_extension("ymt"), amv::zone_file(zone, vec![item])?)?;
    std::fs::write(base.with_extension("ytd"), amv::zone_textures(baked.textures(uuid).into()))?;
    println!("{} -> {}.ymt, .ytd", o.ytyp.display(), base.display());
    Ok(())
}
