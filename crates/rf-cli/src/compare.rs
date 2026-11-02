//! `rf compare`: converts game files to XML (and back) with both the Rust port and
//! the C# RedForge (`rf-oracle`), and reports every difference.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use rf_formats::formats::{self, Kind, XmlContext, externals_folder};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{FileRef, Rpf8FileSystem};

pub struct Options {
    pub ext: String,
    pub filter: Option<String>,
    pub limit: usize,
    pub work: PathBuf,
    pub write: bool,
    pub oracle: Option<PathBuf>,
    pub verbose: bool,
}

struct Item {
    dir: PathBuf,
    name: String,
    kind: Kind,
}

impl Item {
    fn file(&self) -> PathBuf {
        self.dir.join(&self.name)
    }
    fn xml_in(&self, sub: &str) -> PathBuf {
        self.dir.join(sub).join(self.kind.xml_file_name(&self.name))
    }
    fn bin_in(&self, sub: &str) -> PathBuf {
        self.dir.join(sub).join(&self.name)
    }
    fn externals_in(&self, sub: &str) -> PathBuf {
        externals_folder(&self.dir.join(sub), &self.name)
    }
}

pub fn oracle_path(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_owned());
    }
    if let Ok(p) = std::env::var("REDFORGE_ORACLE") {
        return Ok(p.into());
    }
    let exe = std::env::current_exe()?;
    let p = exe.parent().unwrap().join("../oracle/bin/Oracle/release/rf-oracle");
    if p.is_file() {
        return Ok(p);
    }
    bail!("rf-oracle not found: build tools/oracle, or pass --oracle / set REDFORGE_ORACLE")
}

/// Runs the oracle on `(input, output)` pairs; returns the inputs that failed.
fn run_oracle(oracle: &Path, mode: &str, pairs: &[(PathBuf, PathBuf)]) -> Result<BTreeMap<PathBuf, String>> {
    let mut child = Command::new(oracle)
        .arg(mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting {}", oracle.display()))?;
    // Feed the input from another thread while the output is read here: with
    // many files both pipes fill up, and writing everything first deadlocks.
    let mut stdin = child.stdin.take().unwrap();
    let input: String = pairs.iter().map(|(i, o)| format!("{}\t{}\n", i.display(), o.display())).collect();
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let out = child.wait_with_output()?;
    writer.join().map_err(|_| anyhow::anyhow!("writing to the oracle panicked"))??;
    let mut failed = BTreeMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(rest) = line.strip_prefix("FAIL ")
            && let Some((path, err)) = rest.split_once(": ")
        {
            failed.insert(PathBuf::from(path), err.to_owned());
        }
    }
    Ok(failed)
}

/// The first line where two texts differ, as (line number, a, b).
fn first_diff(a: &str, b: &str) -> Option<(usize, String, String)> {
    let mut la = a.lines();
    let mut lb = b.lines();
    for n in 1.. {
        match (la.next(), lb.next()) {
            (None, None) => return None,
            (x, y) if x == y => continue,
            (x, y) => return Some((n, x.unwrap_or("<end>").to_owned(), y.unwrap_or("<end>").to_owned())),
        }
    }
    None
}

/// Differences between two folders of external files.
fn diff_folders(a: &Path, b: &Path) -> Vec<String> {
    let list = |p: &Path| -> BTreeMap<String, PathBuf> {
        std::fs::read_dir(p)
            .map(|rd| {
                rd.filter_map(|e| e.ok()).map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).collect()
            })
            .unwrap_or_default()
    };
    let (fa, fb) = (list(a), list(b));
    let mut out = Vec::new();
    for (name, pa) in &fa {
        match fb.get(name) {
            None => out.push(format!("{name}: missing on the Rust side")),
            Some(pb) => {
                if std::fs::read(pa).ok() != std::fs::read(pb).ok() {
                    out.push(format!("{name}: contents differ"));
                }
            }
        }
    }
    for name in fb.keys() {
        if !fa.contains_key(name) {
            out.push(format!("{name}: only on the Rust side"));
        }
    }
    out
}

#[derive(Default)]
struct Tally {
    same: usize,
    differ: Vec<String>,
    rust_failed: Vec<String>,
    oracle_failed: Vec<String>,
}

impl Tally {
    fn report(&self, title: &str, total: usize, verbose: bool) {
        println!(
            "{title}: {} identical, {} differ, {} Rust errors, {} C# errors (of {total})",
            self.same,
            self.differ.len(),
            self.rust_failed.len(),
            self.oracle_failed.len()
        );
        let n = if verbose { usize::MAX } else { 10 };
        for (label, list) in
            [("differs", &self.differ), ("Rust error", &self.rust_failed), ("C# error", &self.oracle_failed)]
        {
            for line in list.iter().take(n) {
                println!("  {label}: {line}");
            }
            if list.len() > n {
                println!("  ... and {} more", list.len() - n);
            }
        }
    }
}

/// Compares the XML (and externals) at `sub_a` and `sub_b` for each item.
fn compare_xml(
    items: &[&Item],
    a: &str,
    b: &str,
    rust_failed: &BTreeMap<PathBuf, String>,
    oracle_failed: &BTreeMap<PathBuf, String>,
    oracle_input: impl Fn(&Item) -> PathBuf,
) -> Tally {
    let mut t = Tally::default();
    for it in items {
        let label = it.file().display().to_string();
        if let Some(e) = oracle_failed.get(&oracle_input(it)) {
            t.oracle_failed.push(format!("{label}: {e}"));
            continue;
        }
        if let Some(e) = rust_failed.get(&it.file()) {
            t.rust_failed.push(format!("{label}: {e}"));
            continue;
        }
        let xa = std::fs::read_to_string(it.xml_in(a)).unwrap_or_default();
        let xb = std::fs::read_to_string(it.xml_in(b)).unwrap_or_default();
        let mut problems = Vec::new();
        if let Some((n, la, lb)) = first_diff(&xa, &xb) {
            problems.push(format!("line {n}:\n      C#:   {}\n      Rust: {}", la.trim(), lb.trim()));
        }
        if it.kind.has_folder() {
            problems.extend(diff_folders(&it.externals_in(a), &it.externals_in(b)));
        }
        if problems.is_empty() {
            t.same += 1;
        } else {
            t.differ.push(format!("{label}\n    {}", problems.join("\n    ")));
        }
    }
    t
}

fn rust_to_xml(it: &Item, sub: &str, input: &Path, hashes: &HashIndex) -> Result<()> {
    let data = std::fs::read(input)?;
    let out_dir = it.dir.join(sub);
    std::fs::create_dir_all(&out_dir)?;
    let cx = XmlContext { hashes, folder: it.kind.has_folder().then(|| externals_folder(&out_dir, &it.name)) };
    let xml = formats::to_xml(&it.name, &data, &cx)?;
    std::fs::write(it.xml_in(sub), xml)?;
    Ok(())
}

pub fn run(fs: &Rpf8FileSystem, hashes: &HashIndex, o: &Options) -> Result<()> {
    let oracle = oracle_path(o.oracle.as_deref())?;
    let ext = format!(".{}", o.ext.trim_start_matches('.').to_lowercase());
    let filter = o.filter.as_deref().map(str::to_lowercase);

    // One file per name, spread evenly over all of them.
    let mut by_name: BTreeMap<String, FileRef> = BTreeMap::new();
    for (path, f) in fs.files() {
        if path.ends_with(&ext) && filter.as_ref().is_none_or(|x| path.contains(x.as_str())) {
            let name = path.rsplit('/').next().unwrap().to_owned();
            by_name.entry(name).and_modify(|e| *e = (*e).min(f)).or_insert(f);
        }
    }
    let all: Vec<(String, FileRef)> = by_name.into_iter().collect();
    if all.is_empty() {
        bail!("no {ext} files match");
    }
    let n = o.limit.min(all.len());
    let step = all.len() as f64 / n as f64;
    let picked: Vec<&(String, FileRef)> = (0..n).map(|i| &all[(i as f64 * step) as usize]).collect();

    if o.work.exists() {
        std::fs::remove_dir_all(&o.work)?;
    }
    let mut items = Vec::new();
    for (i, (name, f)) in picked.iter().enumerate() {
        let dir = o.work.join(format!("{i:05}"));
        std::fs::create_dir_all(&dir)?;
        let data = fs.extract_loose_deflate(*f).with_context(|| fs.path(*f))?;
        std::fs::write(dir.join(name), &data)?;
        let Some(kind) = Kind::of_file(name, &data) else { continue };
        items.push(Item { dir, name: name.clone(), kind });
    }
    println!("{} {ext} files (of {} with distinct names) in {}", items.len(), all.len(), o.work.display());

    // Game file -> XML, both ways.
    let pairs: Vec<(PathBuf, PathBuf)> = items
        .iter()
        .map(|it| {
            let _ = std::fs::create_dir_all(it.dir.join("cs"));
            (it.file(), it.xml_in("cs"))
        })
        .collect();
    let oracle_failed = run_oracle(&oracle, "toxml", &pairs)?;
    let mut rust_failed = BTreeMap::new();
    for it in &items {
        if let Err(e) = rust_to_xml(it, "rs", &it.file(), hashes) {
            rust_failed.insert(it.file(), format!("{e:#}"));
        }
    }
    let refs: Vec<&Item> = items.iter().collect();
    let t = compare_xml(&refs, "cs", "rs", &rust_failed, &oracle_failed, |it| it.file());
    t.report("to XML", items.len(), o.verbose);

    if !o.write {
        return Ok(());
    }

    // XML -> game file: the Rust writer read back by the C#, against the C#
    // writer read back by the C#.
    let ok: Vec<&Item> = items.iter().filter(|it| !oracle_failed.contains_key(&it.file())).collect();
    let mut write_failed = BTreeMap::new();
    for it in &ok {
        let xml = std::fs::read_to_string(it.xml_in("cs"))?;
        let cx = XmlContext { hashes, folder: Some(it.externals_in("cs")) };
        let xml_name = it.kind.xml_file_name(&it.name);
        match formats::from_xml(&xml_name, &xml, &cx) {
            Ok(bin) => {
                std::fs::create_dir_all(it.dir.join("rsw"))?;
                std::fs::write(it.bin_in("rsw"), bin)?;
            }
            Err(e) => {
                write_failed.insert(it.file(), format!("{e:#}"));
            }
        }
    }
    let pairs: Vec<(PathBuf, PathBuf)> = ok
        .iter()
        .map(|it| {
            let _ = std::fs::create_dir_all(it.dir.join("csw"));
            (it.xml_in("cs"), it.bin_in("csw"))
        })
        .collect();
    let csw_failed = run_oracle(&oracle, "fromxml", &pairs)?;
    let mut pairs = Vec::new();
    for it in &ok {
        if !csw_failed.contains_key(&it.xml_in("cs")) {
            pairs.push((it.bin_in("csw"), it.xml_in("csw")));
        }
        if !write_failed.contains_key(&it.file()) {
            pairs.push((it.bin_in("rsw"), it.xml_in("rsw")));
        }
    }
    let read_failed = run_oracle(&oracle, "toxml", &pairs)?;
    let mut failed_any = write_failed.clone();
    for it in &ok {
        if let Some(e) = read_failed.get(&it.bin_in("rsw")) {
            failed_any.entry(it.file()).or_insert(format!("the C# can't read the Rust file: {e}"));
        }
    }
    let mut cs_failed: BTreeMap<PathBuf, String> = BTreeMap::new();
    for it in &ok {
        if let Some(e) = csw_failed.get(&it.xml_in("cs")).or(read_failed.get(&it.bin_in("csw"))) {
            cs_failed.insert(it.bin_in("csw"), e.clone());
        }
    }
    let t = compare_xml(&ok, "csw", "rsw", &failed_any, &cs_failed, |it| it.bin_in("csw"));
    t.report("from XML (read back by the C#)", ok.len(), o.verbose);

    // How close the files themselves are.
    let mut identical = 0;
    for it in &ok {
        let a = std::fs::read(it.bin_in("csw")).ok().and_then(|d| rf_formats::rsc8::read_loose(&d).ok());
        let b = std::fs::read(it.bin_in("rsw")).ok().and_then(|d| rf_formats::rsc8::read_loose(&d).ok());
        if a.is_some() && a == b {
            identical += 1;
        }
    }
    println!("from XML: {identical} of {} files byte-identical to the C#'s (decompressed)", ok.len());
    Ok(())
}
