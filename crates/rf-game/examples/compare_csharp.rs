//! Compares the game data with the C# RedForge's startup cache, written by the
//! C# from the same game install:
//!
//! ```sh
//! REDFORGE_GAME_DIR=... cargo run --release -p rf-game --example compare_csharp -- path/to/RedForge.Games.RDR2.startup.dat
//! ```

use std::path::Path;

use anyhow::{Context, Result, bail};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{Rpf8FileSystem, Rpf8Keys};
use rf_game::{GameData, csharp};

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let Some(cache) = std::env::args().nth(1) else { bail!("usage: compare_csharp <RedForge.Games.RDR2.startup.dat>") };
    let game = std::env::var("REDFORGE_GAME_DIR").context("set REDFORGE_GAME_DIR")?;
    let strings = HashIndex::new();
    strings.load_file(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt"))?;
    let fs = Rpf8FileSystem::open(Path::new(&game), Rpf8Keys::from_game_folder(Path::new(&game))?, &strings)?;
    let gd = GameData::load(&fs, &strings);
    let cs = csharp::read(&std::fs::read(&cache).with_context(|| cache.clone())?)?;
    let c = csharp::compare(&fs, &gd, &cs);
    for l in &c.lines {
        println!("{l}");
    }
    println!("{} differences", c.differences.len());
    Ok(())
}
