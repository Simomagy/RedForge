//! Saving the game's own ymaps: their extents and flags worked out again from
//! their contents, compared with what the files hold. This only runs when
//! `REDFORGE_GAME_DIR` points at the game folder; `--nocapture` prints the
//! comparison. The `save` module's docs say how all the game's ymaps compare.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rf_formats::formats::databag::DataBag;
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{FileRef, Rpf8FileSystem, Rpf8Keys};
use rf_game::GameData;

fn game() -> Option<(Rpf8FileSystem, HashIndex, GameData)> {
    let dir = PathBuf::from(std::env::var_os("REDFORGE_GAME_DIR")?);
    let strings = HashIndex::new();
    strings.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt")).unwrap();
    let fs = Rpf8FileSystem::open(&dir, Rpf8Keys::from_game_folder(&dir).unwrap(), &strings).unwrap();
    let gd = GameData::load(&fs, &strings);
    Some((fs, strings, gd))
}

/// About 100 ymaps: a spread over all of them (HD, LOD and SLOD maps, grass
/// and props, interiors), more of the area LODs (interiors, occluders), and
/// some of each kind that's rare.
fn sample(fs: &Rpf8FileSystem) -> Vec<(String, FileRef)> {
    let mut all: Vec<(String, FileRef)> =
        fs.files().filter(|(p, _)| p.ends_with(".ymap")).map(|(p, f)| (p.to_string(), f)).collect();
    all.sort();
    let mut picked: Vec<_> = all.iter().step_by(all.len() / 50).cloned().collect();
    let parts = [
        ("_area_lod_combine_metadata.rpf/", 20),
        ("/instance_placement/", 6),
        ("/ambient_camps/", 3),
        ("/occlusion/", 4),
        ("/map_mods.rpf/", 3),
        ("/lodlights.rpf/", 4),
        // Box occluders: alone, and with entities and occlude models.
        ("nopq_new_area_lod_combine_metadata.rpf/ser_01_.ymap", 1),
        ("mp_lobby_01.rpf/0x393e8b43.ymap", 1),
        ("jklm_15_18_area_lod_combine_metadata.rpf/ann_01_.ymap", 1),
        ("rstu_96_99_area_lod_combine_metadata.rpf/arm_01.ymap", 1),
        ("nopq_7_10_area_lod_combine_metadata.rpf/the_01.ymap", 1),
        ("nopq_new_area_lod_combine_metadata.rpf/new_gar_01.ymap", 1),
    ];
    for (part, n) in parts {
        let matching: Vec<_> = all.iter().filter(|(p, _)| p.contains(part)).collect();
        let step = (matching.len() / n).max(1);
        picked.extend(matching.into_iter().step_by(step).take(n).cloned());
    }
    picked.sort();
    picked.dedup();
    picked
}

/// What a ymap holds (the rarest of it), for the summary.
fn kind(b: &DataBag) -> &'static str {
    let has = |name| b.bags(name).is_some_and(|v| !v.is_empty());
    let entities: Vec<&DataBag> = b.bags("entities").unwrap_or_default().iter().flatten().collect();
    let levels: Vec<u32> = entities.iter().map(|e| e.u32("lodLevel")).collect();
    let batches = b.bag("instancedData").is_some_and(|d| d.objects().next().is_some());
    let lights =
        ["LODLightsSOA", "DistantLODLightsSOA"].iter().any(|s| b.bag(*s).is_some_and(|l| l.objects().next().is_some()));
    if entities.iter().any(|e| e.class_name() == 0x09CC_28BE) {
        "interiors"
    } else if has("boxOccluders") {
        "box occluders"
    } else if has("occludeModels") {
        "occlude models"
    } else if has("timeCycleModifiers") {
        "tc modifiers"
    } else if batches {
        "batches"
    } else if lights {
        "lights"
    } else if levels.iter().any(|l| matches!(l, 2 | 3 | 4 | 6)) {
        "SLOD"
    } else if levels.contains(&1) {
        "LOD"
    } else if !entities.is_empty() {
        "HD"
    } else {
        "empty"
    }
}

/// The largest difference between two boxes' corners.
fn box_diff(a: [[f32; 3]; 2], b: [[f32; 3]; 2]) -> f32 {
    (0..2).flat_map(|i| (0..3).map(move |j| (a[i][j] - b[i][j]).abs())).fold(0.0, f32::max)
}

fn extents(b: &DataBag, prefix: &str) -> [[f32; 3]; 2] {
    [b.vec3(format!("{prefix}ExtentsMin").as_str()), b.vec3(format!("{prefix}ExtentsMax").as_str())]
}

#[derive(Default)]
struct Tally {
    n: u32,
    /// Entities' extents within 1 cm, within 1 m.
    entities_exact: u32,
    entities_close: u32,
    /// Streaming extents within 1 cm, or holding the game's.
    streaming_exact: u32,
    streaming_wider: u32,
    flags: u32,
}

#[test]
fn recomputes_the_game_ymaps() {
    let Some((fs, strings, gd)) = game() else { return };
    let files = sample(&fs);
    assert!(files.len() >= 90, "{} files", files.len());
    let mut tally: BTreeMap<&str, Tally> = BTreeMap::new();
    let mut total = Tally::default();
    for (path, f) in &files {
        let game = rf_game::load_doc(&fs, &strings, *f).unwrap_or_else(|| panic!("{path} doesn't read")).bag;
        let mut ours = game.clone();
        rf_editor::save::update_map(&mut ours, &gd.archetypes);

        let ents = box_diff(extents(&game, "entities"), extents(&ours, "entities"));
        let [gmin, gmax] = extents(&game, "streaming");
        let [omin, omax] = extents(&ours, "streaming");
        let strm = box_diff([gmin, gmax], [omin, omax]);
        let wider = (0..3).all(|i| omin[i] <= gmin[i] + 0.01 && omax[i] >= gmax[i] - 0.01);
        let flags = |b: &DataBag| (b.u32("flags"), b.u32("contentFlags"));
        // The game's LOD flag on ymaps with nothing LOD in them can't be
        // worked out; any other difference is a wrong rule.
        let (g, o) = (flags(&game), flags(&ours));
        assert!(g == o || (g.0 == o.0 | 2 && g.1 == o.1), "{path}: flags {g:?}, worked out {o:?}");
        let k = kind(&game);
        for t in [tally.entry(k).or_default(), &mut total] {
            t.n += 1;
            t.entities_exact += (ents < 0.01) as u32;
            t.entities_close += (0.01..1.0).contains(&ents) as u32;
            t.streaming_exact += (strm < 0.01) as u32;
            t.streaming_wider += (strm >= 0.01 && wider) as u32;
            t.flags += (flags(&game) == flags(&ours)) as u32;
        }
        if ents >= 0.01 || strm >= 0.01 || flags(&game) != flags(&ours) {
            println!(
                "{k:<14} entities {ents:8.2}  streaming {strm:8.2}{}  flags {:?} -> {:?}  {path}",
                if wider { " (wider)" } else { "        " },
                flags(&game),
                flags(&ours)
            );
        }
    }

    println!(
        "\n{:<15}{:>5}  {:>22}  {:>22}  {:>6}",
        "", "maps", "entities exact/close", "streaming exact/wider", "flags"
    );
    for (k, t) in tally.iter().map(|(k, t)| (*k, t)).chain([("all", &total)]) {
        println!(
            "{k:<15}{:>5}  {:>22}  {:>22}  {:>6}",
            t.n,
            format!("{}/{}", t.entities_exact, t.entities_close),
            format!("{}/{}", t.streaming_exact, t.streaming_wider),
            t.flags
        );
    }

    // The entities' extents mostly come out as stored. What differs (see the
    // `save` module): turned models, whose box the game takes tighter,
    // archetypes missing from the game data, interiors (their archetypes have
    // no box in RDR2) and LOD lights (left out).
    let n = total.n as f32;
    assert!(total.entities_exact as f32 > 0.8 * n, "entities' extents exact for {}", total.entities_exact);
    // Each entity's box is grown by its lodDist where the game grows its
    // position, so these mostly hold the game's.
    let streaming = total.streaming_exact + total.streaming_wider;
    assert!(streaming as f32 > 0.6 * n, "streaming extents exact or wider for {streaming}");
    // The flags follow the game's rules.
    assert!(total.flags as f32 > 0.9 * n, "flags exact for {}", total.flags);
    // Box occluders turned by their angle about Z.
    let boxes = &tally["box occluders"];
    assert!(
        boxes.n >= 6 && boxes.entities_exact == boxes.n,
        "box occluders' extents exact for {}",
        boxes.entities_exact
    );
}
