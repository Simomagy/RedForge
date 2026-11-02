//! Streams the map at a few places and prints what's visible there:
//!
//! ```sh
//! REDFORGE_GAME_DIR=... cargo run --release -p rf-world --example stream
//! ```
//!
//! Per place: the nodes streamed and loaded, the entities visible by LOD level
//! (with their median distance), those whose archetype has no model, LOD
//! parents shown along with their children, and how long loading and an update
//! take. Then checks a flight high above Valentine, an hour change, an interior
//! and a cull box.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};
use rf_game::{AssetType, GameData};
use rf_world::{EntityId, World, model_files};

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let game = std::env::var("REDFORGE_GAME_DIR").context("set REDFORGE_GAME_DIR")?;
    let strings = HashIndex::new();
    strings.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt"))?;
    let t = Instant::now();
    let fs = Rpf8FileSystem::open(Path::new(&game), Rpf8Keys::from_game_folder(Path::new(&game))?, &strings)?;
    let gd = Arc::new(GameData::load(&fs, &strings));
    println!("game loaded in {:.2?}", t.elapsed());
    let t = Instant::now();
    let strings = Arc::new(strings);
    let mut world = World::new(Arc::new(fs), strings.clone(), gd.clone());
    let s = world.stats();
    println!("world: {} nodes, {} cull boxes, in {:.2?}", s.nodes, world.cull_boxes().len(), t.elapsed());

    let places = world.places();
    println!("\nplaces:");
    for (name, p) in &places {
        println!("  {name:<20} ({:7.0}, {:7.0}, {:5.0})", p[0], p[1], p[2]);
    }
    let place = |name: &str| places.iter().find(|(n, _)| n == name).map(|(_, p)| *p).unwrap();

    let noon = 12.0;
    for (name, p) in &places[..10] {
        let (t, updates) = settle(&mut world, *p, noon);
        report(&world, &gd, name);
        timings(&mut world, *p, t, updates);
    }

    // High above Valentine: only the big LOD maps' boxes reach up there.
    let val = place("Valentine");
    for z in [400.0, 1000.0] {
        settle(&mut world, [val[0], val[1], z], noon);
        report(&world, &gd, &format!("Valentine at {z} m"));
    }

    // Time archetypes: the same spot at night.
    settle(&mut world, val, noon);
    let day: HashSet<EntityId> = world.visible().iter().map(|e| e.id).collect();
    settle(&mut world, val, 22.0);
    let night: HashSet<EntityId> = world.visible().iter().map(|e| e.id).collect();
    println!(
        "\nValentine at 22:00 instead of noon: {} entities appear, {} go",
        night.difference(&day).count(),
        day.difference(&night).count()
    );

    // Scripted ymaps (lockdowns, camps, story and MP states) hidden, an option.
    settle(&mut world, val, noon);
    let shown = (world.stats().nodes_streamed, world.visible().len());
    world.set_hide_scripted(true);
    settle(&mut world, val, noon);
    let hidden = (world.stats().nodes_streamed, world.visible().len());
    world.set_hide_scripted(false);
    println!(
        "Valentine with scripted ymaps hidden: {} nodes streamed instead of {}, {} entities visible instead of {}",
        hidden.0, shown.0, hidden.1, shown.1
    );

    interior(&mut world, &gd, &strings, val);
    cull_box(&mut world);

    // Eviction: what's not streamed any more is dropped after a while.
    world.set_evict_after(Duration::from_millis(100));
    settle(&mut world, place("Rhodes"), noon);
    let before = world.stats().nodes_loaded;
    std::thread::sleep(Duration::from_millis(1200));
    world.update(place("Rhodes"), noon);
    let s = world.stats();
    println!("\neviction: {before} maps loaded, {} after (streamed {})", s.nodes_loaded, s.nodes_streamed);
    Ok(())
}

/// Updates at a place until everything streamed there is loaded (the app never
/// waits like this).
fn settle(world: &mut World, p: [f32; 3], hour: f32) -> (Duration, usize) {
    let t = Instant::now();
    let mut updates = 0;
    loop {
        world.update(p, hour);
        updates += 1;
        if world.stats().nodes_loading == 0 {
            return (t.elapsed(), updates);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn report(world: &World, gd: &GameData, name: &str) {
    let s = world.stats();
    let vis = world.visible();
    let mut by_lod: BTreeMap<u8, Vec<f32>> = BTreeMap::new();
    for e in vis {
        by_lod.entry(e.lod_level).or_default().push(e.distance);
    }
    let lods: Vec<String> = by_lod
        .iter_mut()
        .map(|(l, d)| {
            d.sort_by(f32::total_cmp);
            format!("{}: {} (median {:.0} m)", lod_name(*l), d.len(), d[d.len() / 2])
        })
        .collect();
    let (mut no_arch, mut assetless, mut no_file) = (HashSet::new(), HashSet::new(), HashSet::new());
    let mut no_model = 0;
    for e in vis {
        if model_files(gd, e.archetype).is_none() {
            no_model += 1;
            match gd.archetypes.get(&e.archetype) {
                None => no_arch.insert(e.archetype),
                Some(a) if a.asset_type == AssetType::ASSETLESS || a.asset_type == AssetType::UNINITIALIZED => {
                    assetless.insert(e.archetype)
                }
                Some(_) => no_file.insert(e.archetype),
            };
        }
    }
    let ids: HashSet<EntityId> = vis.iter().map(|e| e.id).collect();
    let pairs = vis.iter().filter(|e| world.lod_parent(e.id).is_some_and(|p| ids.contains(&p))).count();
    let interior = vis.iter().filter(|e| e.id.interior_entity != u32::MAX).count();
    println!("\n== {name}");
    println!(
        "  nodes: {} streamed, {} loaded; cull boxes active {}; interiors {} ({interior} entities)",
        s.nodes_streamed, s.nodes_loaded, s.cull_boxes_active, s.interiors_visible
    );
    println!("  visible {}: {}", vis.len(), lods.join(", "));
    println!(
        "  no model: {no_model} entities, {} archetypes ({} unknown, {} assetless, {} without their file)",
        no_arch.len() + assetless.len() + no_file.len(),
        no_arch.len(),
        assetless.len(),
        no_file.len()
    );
    println!("  shown with their LOD parent: {pairs}");
}

fn lod_name(l: u8) -> String {
    match l {
        0 => "orphan HD".into(),
        1 => "HD".into(),
        2 => "LOD".into(),
        l => format!("SLOD{}", l - 2),
    }
}

/// How long loading a place took, and an update there when moving a little and
/// when standing still.
fn timings(world: &mut World, p: [f32; 3], loading: Duration, updates: usize) {
    let t = Instant::now();
    world.update([p[0] + 1.5, p[1], p[2]], 12.0);
    let step = t.elapsed();
    let t = Instant::now();
    world.update([p[0] + 1.6, p[1], p[2]], 12.0);
    let still = t.elapsed();
    println!(
        "  loaded in {loading:.2?} ({updates} updates); an update moving 1.5 m: {step:.2?}, standing still: {still:.2?}"
    );
}

/// Inside an interior: the nearest one visible from Valentine's centre.
fn interior(world: &mut World, gd: &GameData, strings: &HashIndex, val: [f32; 3]) {
    settle(world, val, 12.0);
    let mut by_int: HashMap<(u32, u32), Vec<[f32; 3]>> = HashMap::new();
    for e in world.visible().iter().filter(|e| e.id.interior_entity != u32::MAX) {
        by_int.entry((e.id.node, e.id.entity)).or_default().push([e.transform[12], e.transform[13], e.transform[14]]);
    }
    let Some((key, pts)) = by_int.into_iter().max_by_key(|(_, p)| p.len()) else {
        println!("\nno interior near Valentine");
        return;
    };
    let mut c = [0.0f32; 3];
    for p in &pts {
        for i in 0..3 {
            c[i] += p[i] / pts.len() as f32;
        }
    }
    settle(world, c, 12.0);
    let inside = world.visible().iter().filter(|e| e.id.node == key.0 && e.id.entity == key.1).count();
    println!(
        "\ninterior {} #{} at ({:.0}, {:.0}, {:.0}): {} entities seen from Valentine's centre, {inside} from inside",
        strings.name(key.0),
        key.1,
        c[0],
        c[1],
        c[2],
        pts.len()
    );
    report(world, gd, "inside it");
}

/// A cull box: what it hides while the camera is in it.
fn cull_box(world: &mut World) {
    let Some(c) = world.cull_boxes().iter().find(|c| c.enabled && !c.culled.is_empty()).cloned() else {
        println!("\nno enabled cull box");
        return;
    };
    let centre = [0, 1, 2].map(|i| (c.min[i] + c.max[i]) * 0.5);
    settle(world, centre, 12.0);
    let on = (world.stats(), world.visible().len());
    let culled_on = world.visible().iter().filter(|e| c.culled.contains(&e.id.node)).count();
    world.set_cull_box_enabled(&c.name, false);
    settle(world, centre, 12.0);
    let off = (world.stats(), world.visible().len());
    let culled_off = world.visible().iter().filter(|e| c.culled.contains(&e.id.node)).count();
    world.set_cull_box_enabled(&c.name, true);
    println!(
        "\ncull box {} ({} ymaps) at ({:.0}, {:.0}, {:.0}): on, {} nodes streamed, {} visible ({culled_on} from its ymaps); \
         off, {} nodes, {} visible ({culled_off} from its ymaps)",
        c.name,
        c.culled.len(),
        centre[0],
        centre[1],
        centre[2],
        on.0.nodes_streamed,
        on.1,
        off.0.nodes_streamed,
        off.1
    );
}
