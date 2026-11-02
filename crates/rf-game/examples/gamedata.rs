//! Loads the game data and prints what it holds, and the texture dictionaries of
//! the models named on the command line:
//!
//! ```sh
//! REDFORGE_GAME_DIR=... cargo run --release -p rf-game --example gamedata -- [model.ydr...]
//! ```

use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};
use rf_game::GameData;

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let models = &args;

    let game = std::env::var("REDFORGE_GAME_DIR").context("set REDFORGE_GAME_DIR")?;
    let strings_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt");
    let strings = HashIndex::new();
    strings.load_file(&strings_path)?;
    let t = Instant::now();
    let fs = Rpf8FileSystem::open(Path::new(&game), Rpf8Keys::from_game_folder(Path::new(&game))?, &strings)?;
    println!("archives: {} in {:.2?}", fs.archives.len(), t.elapsed());

    let t = Instant::now();
    let gd = GameData::load(&fs, &strings);
    println!("game data in {:.2?}", t.elapsed());
    println!(
        "enabled archives {}, archetypes {}, gtxd {}, hd {:?}, asset owners {}, map data groups {}, interiors {}",
        gd.enabled_rpfs.len(),
        gd.archetypes.len(),
        gd.gtxd_hierarchy.len(),
        gd.hd_txd_bindings.iter().map(|m| m.len()).collect::<Vec<_>>(),
        gd.asset_owner_map.len(),
        gd.map_data_groups.len(),
        gd.interiors.len(),
    );
    println!(
        "caches: bounds {}, map data {}, interior proxies {}; ytyps {}, ychs {}, ymfs {}",
        gd.bounds_cache.len(),
        gd.map_data_cache.len(),
        gd.interior_proxy_cache.len(),
        gd.ytyp_files.len(),
        gd.ych_files.len(),
        gd.ymf_files.len()
    );
    for (i, t) in gd.stream_entries.iter().enumerate() {
        if !t.is_empty() {
            let id = if i >= 14 { i - 14 + 64 } else { i } as u8;
            let ext = rf_formats::rpf8::ext_name(id).unwrap_or("?");
            println!("  {ext}: {}", t.len());
        }
    }
    for m in models {
        println!("{m}:");
        for f in gd.txds_for_model(m) {
            println!("  {}", fs.path(f));
        }
    }
    Ok(())
}
