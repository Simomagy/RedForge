use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use rayon::prelude::*;
use rf_formats::hash::{HashIndex, joaat};
use rf_formats::rpf8::{FileRef, Rpf8FileSystem, Rpf8Keys};

#[derive(Parser)]
#[command(name = "rf", version, about = "RedForge command-line tools for RDR2 game files")]
struct Cli {
    /// The RDR2 game folder (the one with RDR2.exe).
    #[arg(long, global = true, env = "REDFORGE_GAME_DIR")]
    game: Option<PathBuf>,

    /// The strings dictionary (default: data/rdr2/strings.txt).
    #[arg(long, global = true)]
    strings: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Find the AES key in RDR2.exe and decode the RPF8 keys.
    Keys,
    /// List the files in the archives.
    Ls {
        /// Only paths containing this text (case-insensitive).
        filter: Option<String>,
        /// List archives instead of files.
        #[arg(long)]
        archives: bool,
        /// Show entry details (size, compression, encryption).
        #[arg(short, long)]
        long: bool,
    },
    /// Extract files by path, or every file whose path contains --filter.
    Extract {
        paths: Vec<String>,
        #[arg(long)]
        filter: Option<String>,
        /// Output folder.
        #[arg(short, long, default_value = ".")]
        out: PathBuf,
        /// Write resources as loose files (RSC8 header, compressed), the way RedM
        /// loads them from a stream folder, instead of their decompressed data.
        #[arg(long)]
        loose: bool,
        /// Keep the archive folder structure under the output folder.
        #[arg(long)]
        tree: bool,
    },
    /// Counts of archives, files, extensions, encryption and compression.
    Stats,
    /// Decrypt and decompress files to check that they read, reporting failures.
    Check {
        /// Only paths containing this text.
        filter: Option<String>,
        /// Check at most this many files (spread over all of them).
        #[arg(long)]
        limit: Option<usize>,
        /// Print every failure, not just the first few.
        #[arg(short, long)]
        verbose: bool,
    },
    /// Print the joaat hash of each string.
    Hash { strings: Vec<String> },
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();

    match &cli.command {
        Command::Hash { strings } => {
            for s in strings {
                let h = joaat(&s.to_lowercase());
                println!("0x{h:08X}  {h:>10}  {s}");
            }
            Ok(())
        }
        Command::Keys => {
            let game = game_folder(&cli)?;
            let t = Instant::now();
            let keys = Rpf8Keys::from_game_folder(&game)?;
            println!("AES key found and keys decoded in {:.2?}", t.elapsed());
            println!("AES key SHA-1 verified; first bytes {:02X?}...", &keys.aes_key[..4]);
            Ok(())
        }
        _ => {
            let fs = open_fs(&cli)?;
            run_fs_command(&cli.command, &fs)
        }
    }
}

fn run_fs_command(cmd: &Command, fs: &Rpf8FileSystem) -> Result<()> {
    match cmd {
        Command::Ls { filter, archives, long } => {
            let filter = filter.as_deref().map(str::to_lowercase);
            if *archives {
                for a in &fs.archives {
                    if filter.as_ref().is_none_or(|f| a.path.contains(f.as_str())) {
                        println!("{:>8} files  {:>12}  {}", a.entries.len(), a.size, a.path);
                    }
                }
                return Ok(());
            }
            let mut files: Vec<(String, FileRef)> = fs
                .files()
                .filter(|(p, _)| filter.as_ref().is_none_or(|f| p.contains(f.as_str())))
                .map(|(_, f)| (fs.path(f), f))
                .collect();
            files.sort();
            for (path, f) in &files {
                if *long {
                    let e = fs.entry(*f);
                    println!(
                        "{:>11} {:>11} {} {} {}  {}",
                        e.file_size(),
                        e.decompressed_size(),
                        ["-", "D", "K"].get(e.compressor() as usize).unwrap_or(&"?"),
                        if e.is_encrypted() { "E" } else { "-" },
                        if e.is_resource() { format!("R{:<3}", e.version()) } else { "    ".into() },
                        path
                    );
                } else {
                    println!("{path}");
                }
            }
            eprintln!("{} files", files.len());
            Ok(())
        }
        Command::Extract { paths, filter, out, loose, tree } => {
            let mut targets: Vec<FileRef> = Vec::new();
            for p in paths {
                targets.push(fs.find(p).with_context(|| format!("not found: {p}"))?);
            }
            if let Some(f) = filter {
                let f = f.to_lowercase();
                targets.extend(fs.files().filter(|(p, _)| p.contains(f.as_str())).map(|(_, r)| r));
            }
            if targets.is_empty() {
                bail!("nothing to extract: give paths or --filter");
            }
            targets.sort_by_cached_key(|&f| fs.path(f));
            targets.dedup();
            if !*tree {
                // Flat output: of several files with the same name, only the first is written.
                let mut seen = std::collections::HashSet::new();
                let before = targets.len();
                targets.retain(|&f| seen.insert(fs.path(f).rsplit('/').next().unwrap_or_default().to_lowercase()));
                if targets.len() < before {
                    eprintln!(
                        "{} files skipped because another one has the same name (use --tree to keep them all)",
                        before - targets.len()
                    );
                }
            }
            std::fs::create_dir_all(out)?;
            let failed = AtomicUsize::new(0);
            targets.par_iter().for_each(|&f| {
                let path = fs.path(f);
                let rel = if *tree { path.clone() } else { path.rsplit('/').next().unwrap_or(&path).to_owned() };
                let dest = out.join(&rel);
                let result = if *loose { fs.extract_loose(f) } else { fs.extract(f) };
                let result = result.map_err(anyhow::Error::from).and_then(|data| {
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&dest, data)?;
                    Ok(())
                });
                if let Err(e) = result {
                    failed.fetch_add(1, Ordering::Relaxed);
                    eprintln!("{path}: {e}");
                }
            });
            let failed = failed.into_inner();
            println!("extracted {} of {} files to {}", targets.len() - failed, targets.len(), out.display());
            Ok(())
        }
        Command::Stats => {
            let mut by_ext: BTreeMap<String, (usize, u64)> = BTreeMap::new();
            let (mut enc, mut res, mut unknown) = (0, 0, 0);
            let mut comp = [0usize; 4];
            for a in &fs.archives {
                for (i, e) in a.entries.iter().enumerate() {
                    let name = &a.names[i];
                    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
                    let slot = by_ext.entry(ext).or_default();
                    slot.0 += 1;
                    slot.1 += e.decompressed_size();
                    enc += e.is_encrypted() as usize;
                    res += e.is_resource() as usize;
                    comp[(e.compressor() as usize).min(3)] += 1;
                    unknown += name.starts_with("0x") as usize;
                }
            }
            let total: usize = by_ext.values().map(|v| v.0).sum();
            println!(
                "{} archives ({} on disk), {} files",
                fs.archives.len(),
                fs.archives.iter().filter(|a| a.parent.is_none()).count(),
                total
            );
            println!("resources {res}, encrypted {enc}, unknown names {unknown}");
            println!("stored {}, deflate {}, oodle {}, other {}", comp[0], comp[1], comp[2], comp[3]);
            let mut exts: Vec<_> = by_ext.into_iter().collect();
            exts.sort_by_key(|e| std::cmp::Reverse(e.1.0));
            for (ext, (n, size)) in exts.iter().take(40) {
                println!("  .{ext:<8} {n:>9}  {:>10.1} MB", *size as f64 / 1e6);
            }
            Ok(())
        }
        Command::Check { filter, limit, verbose } => {
            let filter = filter.as_deref().map(str::to_lowercase);
            let mut files: Vec<(&str, FileRef)> =
                fs.files().filter(|(p, _)| filter.as_ref().is_none_or(|f| p.contains(f.as_str()))).collect();
            files.sort_by_key(|(p, _)| *p);
            if let Some(n) = *limit
                && n < files.len()
            {
                let step = files.len() as f64 / n as f64;
                files = (0..n).map(|i| files[(i as f64 * step) as usize]).collect();
            }
            let t = Instant::now();
            let bytes = AtomicUsize::new(0);
            let failures: Vec<(String, String, String)> = files
                .par_iter()
                .filter_map(|&(_, f)| {
                    let e = fs.entry(f);
                    // Nested archives are read as archives, not decompressed.
                    if e.is_archive_like() {
                        return None;
                    }
                    match fs.extract(f) {
                        Ok(d) => {
                            bytes.fetch_add(d.len(), Ordering::Relaxed);
                            None
                        }
                        Err(err) => {
                            let kind = format!(
                                "{} {}{}",
                                ["stored", "deflate", "oodle"].get(e.compressor() as usize).unwrap_or(&"?"),
                                if e.is_encrypted() {
                                    format!("enc cfg 0x{:02X}", e.enc_config)
                                } else {
                                    "plain".into()
                                },
                                if e.is_resource() { " resource" } else { "" }
                            );
                            Some((fs.path(f), kind, err.to_string()))
                        }
                    }
                })
                .collect();
            let elapsed = t.elapsed();
            let mb = bytes.into_inner() as f64 / 1e6;
            println!(
                "checked {} files in {:.2?} ({:.0} MB, {:.0} MB/s): {} failed",
                files.len(),
                elapsed,
                mb,
                mb / elapsed.as_secs_f64(),
                failures.len()
            );
            let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
            for (_, k, _) in &failures {
                *kinds.entry(k).or_default() += 1;
            }
            for (k, n) in kinds {
                println!("  {n:>7}  {k}");
            }
            for (p, k, e) in failures.iter().take(if *verbose { usize::MAX } else { 20 }) {
                println!("  {p} [{k}]: {e}");
            }
            Ok(())
        }
        Command::Keys | Command::Hash { .. } => unreachable!(),
    }
}

fn game_folder(cli: &Cli) -> Result<PathBuf> {
    cli.game.clone().context("no game folder: pass --game or set REDFORGE_GAME_DIR")
}

/// Finds a data file shipped with RedForge: next to the executable, in the source
/// tree when running from `target/`, or in the current folder.
fn data_file(rel: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(dir) = std::env::var("REDFORGE_DATA") {
        candidates.push(PathBuf::from(dir).join(rel));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join("data").join(rel));
        candidates.push(dir.join("../../data").join(rel));
    }
    candidates.push(Path::new("data").join(rel));
    candidates.into_iter().find(|p| p.is_file())
}

fn open_fs(cli: &Cli) -> Result<Rpf8FileSystem> {
    let game = game_folder(cli)?;
    let strings_path = match &cli.strings {
        Some(p) => p.clone(),
        None => data_file("rdr2/strings.txt").context("strings.txt not found: pass --strings")?,
    };

    let t = Instant::now();
    let mut strings = HashIndex::new();
    let n = strings.load_file(&strings_path).with_context(|| format!("reading {}", strings_path.display()))?;
    log::info!("{n} strings loaded in {:.2?}", t.elapsed());

    let t = Instant::now();
    let keys = Rpf8Keys::from_game_folder(&game)?;
    log::info!("keys decoded in {:.2?}", t.elapsed());

    let t = Instant::now();
    let fs = Rpf8FileSystem::open(&game, keys, &strings)?;
    log::info!("{} archives, {} files indexed in {:.2?}", fs.archives.len(), fs.file_count(), t.elapsed());
    for e in &fs.errors {
        log::warn!("{}: {}", e.path, e.error);
    }
    Ok(fs)
}
