//! Streaming a real RDR2 install. These only run when `REDFORGE_GAME_DIR`
//! points at the game folder.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rf_formats::hash::{HashIndex, joaat};
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};
use rf_game::GameData;
use rf_world::{EntityId, World, model_files};

fn world() -> Option<(World, Arc<GameData>)> {
    let dir = PathBuf::from(std::env::var_os("REDFORGE_GAME_DIR")?);
    let strings = HashIndex::new();
    strings.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt")).unwrap();
    let fs = Rpf8FileSystem::open(&dir, Rpf8Keys::from_game_folder(&dir).unwrap(), &strings).unwrap();
    let gd = Arc::new(GameData::load(&fs, &strings));
    Some((World::new(Arc::new(fs), Arc::new(strings), gd.clone()), gd))
}

fn settle(world: &mut World, p: [f32; 3], hour: f32) {
    let start = Instant::now();
    loop {
        world.update(p, hour);
        if world.stats().nodes_loading == 0 {
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "still loading");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn streams_valentine() {
    let Some((mut world, gd)) = world() else { return };

    // The places are where the towns are.
    let places = world.places();
    let near = |name: &str, x: f32, y: f32| {
        let p = places.iter().find(|(n, _)| n == name).unwrap().1;
        assert!((p[0] - x).hypot(p[1] - y) < 250.0, "{name} at {p:?}");
        p
    };
    let val = near("Valentine", -280.0, 790.0);
    near("Saint Denis", 2630.0, -1250.0);
    near("Blackwater", -810.0, -1320.0);
    near("Rhodes", 1300.0, -1300.0);

    // In town: HD close by, LODs and SLODs further, every LOD level once.
    settle(&mut world, val, 12.0);
    let s = world.stats();
    assert!(s.nodes > 8000 && s.nodes_streamed > 100, "{s:?}");
    let vis = world.visible();
    assert!(vis.len() > 1000, "{} visible", vis.len());
    let median = |lods: &[u8]| {
        let mut d: Vec<f32> = vis.iter().filter(|e| lods.contains(&e.lod_level)).map(|e| e.distance).collect();
        d.sort_by(f32::total_cmp);
        d[d.len() / 2]
    };
    assert!(median(&[0, 1]) < 100.0 && median(&[2]) > median(&[0, 1]) && median(&[3, 4, 5, 6]) > 500.0);
    let ids: HashSet<EntityId> = vis.iter().map(|e| e.id).collect();
    assert_eq!(ids.len(), vis.len());
    let pairs = vis.iter().filter(|e| world.lod_parent(e.id).is_some_and(|p| ids.contains(&p))).count();
    assert_eq!(pairs, 0, "entities shown with their LOD parent");
    assert!(s.interiors_visible > 0);
    let no_model = vis.iter().filter(|e| model_files(&gd, e.archetype).is_none()).count();
    assert!(no_model < 10, "{no_model} without a model");

    // Its trees, bushes, rocks and grass: instanced batches, by cell.
    let batches = world.visible_batches();
    let grass = batches.iter().filter(|b| b.batch.grass).count();
    assert!(grass > 20 && batches.len() - grass > 20, "{grass} grass batches of {}", batches.len());
    for b in batches {
        assert!(
            b.batch.cells.iter().all(|c| c.start < c.end)
                && b.batch.cells.last().unwrap().end as usize == b.batch.len()
        );
        assert!(model_files(&gd, b.batch.archetype).is_some(), "{:x}", b.batch.archetype);
    }
    let tree = batches.iter().find(|b| b.batch.archetype == joaat("p_tree_engoak_01")).expect("an oak batch");
    let t = tree.batch.transform(0);
    assert!((t[12] - val[0]).hypot(t[13] - val[1]) < 1000.0 && t[14] > 50.0, "{t:?}");

    // A barn of Valentine is there, placed where its ymap says.
    let barn = vis.iter().find(|e| e.archetype == joaat("val_01_aucbarns_barn2"));
    if let Some(b) = barn {
        assert!((b.transform[12] - val[0]).hypot(b.transform[13] - val[1]) < 500.0);
    }

    // High up: SLODs.
    settle(&mut world, [val[0], val[1], 1000.0], 12.0);
    let vis = world.visible();
    let slods = vis.iter().filter(|e| e.lod_level >= 3).count();
    assert!(slods * 10 > vis.len() * 9, "{slods} SLODs of {}", vis.len());

    // Moving back: what was loaded stays until it's dropped.
    settle(&mut world, val, 12.0);
    world.set_evict_after(Duration::ZERO);
    settle(&mut world, [val[0], val[1], 1000.0], 12.0);
    std::thread::sleep(Duration::from_millis(1100));
    world.update([val[0], val[1], 1000.0], 12.0);
    assert_eq!(world.stats().nodes_loaded, world.stats().nodes_streamed);
}
