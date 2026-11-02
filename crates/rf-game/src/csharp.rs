//! The C# RedForge's startup cache (`RedForge.Games.RDR2.startup.dat`), read to
//! check this port against what the C# built from the same game install.
//!
//! The file is deflated. It holds (`Rpf8FileManager.SaveStartupCache`) the AES
//! key, the strings file's time, every archive's header and TOC (roots, each
//! followed by its children, depth first), then
//! (`Rpf8DataFileMgr.WriteStartupCache`) the GTXD hierarchy, the four HD texture
//! binding tables, and the ytyp, `_cache.ych` and `_manifest.ymf` files the C#
//! read, as their TOC entry and decompressed data, in the order it read them.
//!
//! The stream entries aren't in it: the C# rebuilds them from the archives at
//! every start. Its ytyp files are exactly the ytyp stream entries, in order,
//! and its manifests and caches those of the enabled archives, in the order
//! they were enabled, which checks the rest.

use std::collections::{HashMap, HashSet};
use std::io::{self, Read};

use rayon::prelude::*;
use rf_formats::formats::meta::Meta;
use rf_formats::rpf8::{ENTRY_SIZE, FileRef, Rpf8FileSystem};

use crate::files::Doc;
use crate::manifest::ytyp_archetypes;
use crate::{GameData, stream_index};

/// A TOC entry as stored (24 bytes).
pub type EntryBytes = [u8; ENTRY_SIZE];

/// An archive in the cache.
#[derive(Clone, Debug)]
pub struct CsArchive {
    /// The path from the game folder, lowercase, `/`-separated.
    pub path: String,
    pub entries: Vec<EntryBytes>,
}

/// A file the C# read: its TOC entry and its decompressed data.
#[derive(Clone, Debug)]
pub struct CsFile {
    pub entry: EntryBytes,
    pub data: Vec<u8>,
}

/// What the C#'s startup cache holds.
#[derive(Clone, Debug, Default)]
pub struct CsCache {
    /// Every archive, roots first, each followed by its children (depth first).
    pub archives: Vec<CsArchive>,
    pub roots: usize,
    pub gtxd_hierarchy: Vec<(u32, u32)>,
    pub hd_txd_bindings: [Vec<(u32, u32)>; 4],
    pub ytyps: Vec<CsFile>,
    pub ychs: Vec<CsFile>,
    pub ymfs: Vec<CsFile>,
}

struct Reader<'a> {
    d: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> io::Result<&[u8]> {
        let s = self.d.get(self.pos..self.pos + n).ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        self.pos += n;
        Ok(s)
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> io::Result<usize> {
        let v = i32::from_le_bytes(self.bytes(4)?.try_into().unwrap());
        usize::try_from(v).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, format!("count {v}")))
    }
    fn i64(&mut self) -> io::Result<i64> {
        Ok(i64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }
    fn string(&mut self) -> io::Result<String> {
        let len = self.d[self.pos..].iter().position(|&b| b == 0).ok_or(io::ErrorKind::UnexpectedEof)?;
        let s = String::from_utf8_lossy(&self.d[self.pos..self.pos + len]).into_owned();
        self.pos += len + 1;
        Ok(s)
    }
    fn entry(&mut self) -> io::Result<EntryBytes> {
        Ok(self.bytes(ENTRY_SIZE)?.try_into().unwrap())
    }
    /// `Rpf8File.ReadStartupCache`.
    fn archive(&mut self, path: String, out: &mut Vec<CsArchive>) -> io::Result<()> {
        self.bytes(8 + 4)?; // StartPos, Version
        let count = self.u32()? as usize;
        self.bytes(4 + 2 + 2 + 0x100)?; // NamesLength, DecryptionTag, PlatformId, RsaSignature
        let entries = (0..count).map(|_| self.entry()).collect::<io::Result<_>>()?;
        out.push(CsArchive { path: path.clone(), entries });
        for _ in 0..self.i32()? {
            self.string()?; // name
            let rel = self.string()?;
            self.i64()?; // size
            let child = format!("{path}/{}", rel.replace('\\', "/").to_lowercase());
            self.archive(child, out)?;
        }
        Ok(())
    }
    fn pairs(&mut self) -> io::Result<Vec<(u32, u32)>> {
        (0..self.i32()?).map(|_| Ok((self.u32()?, self.u32()?))).collect()
    }
    fn files(&mut self) -> io::Result<Vec<CsFile>> {
        (0..self.i32()?)
            .map(|_| {
                let entry = self.entry()?;
                let len = self.i32()?;
                Ok(CsFile { entry, data: self.bytes(len)?.to_vec() })
            })
            .collect()
    }
}

/// Reads a startup cache file (still deflated).
pub fn read(file: &[u8]) -> io::Result<CsCache> {
    let mut d = Vec::new();
    flate2::read::DeflateDecoder::new(file).read_to_end(&mut d)?;
    let mut r = Reader { d: &d, pos: 0 };
    r.bytes(32 + 8)?; // the AES key, the strings file's time
    let roots = r.i32()?;
    // Each root's absolute path and the range of its archives, whose paths are
    // made relative to the game folder once all the roots are known.
    let mut ranges = Vec::with_capacity(roots);
    let mut archives = Vec::new();
    for _ in 0..roots {
        let path = r.string()?.replace('\\', "/");
        r.i64()?; // its time
        let start = archives.len();
        r.archive(String::new(), &mut archives)?;
        ranges.push((path, start..archives.len()));
    }
    let folder = common_folder(&ranges.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>());
    for (path, range) in ranges {
        let root = path[folder.len()..].trim_start_matches('/').to_lowercase();
        for a in &mut archives[range] {
            a.path = format!("{root}{}", a.path);
        }
    }
    let gtxd_hierarchy = r.pairs()?;
    let hd_txd_bindings = [r.pairs()?, r.pairs()?, r.pairs()?, r.pairs()?];
    let ytyps = r.files()?;
    let ychs = r.files()?;
    let ymfs = r.files()?;
    Ok(CsCache { archives, roots, gtxd_hierarchy, hd_txd_bindings, ytyps, ychs, ymfs })
}

/// The folder all the paths are in.
fn common_folder(paths: &[&str]) -> String {
    let Some(first) = paths.first() else { return String::new() };
    let mut folder = first.rsplit_once('/').map(|(f, _)| f.to_owned()).unwrap_or_default();
    while !paths.iter().all(|p| p.starts_with(&format!("{folder}/"))) {
        match folder.rsplit_once('/') {
            Some((f, _)) => folder = f.to_owned(),
            None => return String::new(),
        }
    }
    folder
}

/// The outcome of [`compare`]: what matched, and every difference.
#[derive(Debug, Default)]
pub struct Comparison {
    pub lines: Vec<String>,
    pub differences: Vec<String>,
}

impl Comparison {
    fn same(&mut self, what: &str, detail: String) {
        self.lines.push(format!("same  {what}: {detail}"));
    }
    fn check<T: PartialEq + std::fmt::Debug>(&mut self, what: &str, ours: T, theirs: T) {
        if ours == theirs {
            self.same(what, format!("{ours:?}"));
        } else {
            self.differ(what, format!("ours {ours:?}, C# {theirs:?}"));
        }
    }
    fn differ(&mut self, what: &str, detail: String) {
        let l = format!("DIFF  {what}: {detail}");
        self.lines.push(l.clone());
        self.differences.push(l);
    }
}

fn bytes(fs: &Rpf8FileSystem, f: FileRef) -> EntryBytes {
    fs.entry(f).to_bytes()
}

/// Compares the game data with what the C# built: the archives, the GTXD
/// hierarchy, the HD bindings, the ytyp stream entries and the archetypes they
/// hold, and the manifests and caches read.
pub fn compare(fs: &Rpf8FileSystem, gd: &GameData, cs: &CsCache) -> Comparison {
    let mut c = Comparison::default();

    // The archives and their TOCs.
    let roots = fs.archives.iter().filter(|a| a.parent.is_none()).count();
    c.check("root archives", roots, cs.roots);
    c.check("archives", fs.archives.len(), cs.archives.len());
    let ours: HashMap<&str, &rf_formats::rpf8::Rpf8Archive> =
        fs.archives.iter().map(|a| (a.path.as_str(), a)).collect();
    let mut toc_diffs = 0;
    for a in &cs.archives {
        let same = ours.get(a.path.as_str()).is_some_and(|o| {
            o.entries.len() == a.entries.len() && o.entries.iter().zip(&a.entries).all(|(e, b)| e.to_bytes() == *b)
        });
        if !same {
            toc_diffs += 1;
            if toc_diffs <= 5 {
                c.differ("archive", a.path.clone());
            }
        }
    }
    let entries: usize = cs.archives.iter().map(|a| a.entries.len()).sum();
    if toc_diffs == 0 {
        c.same("archive paths and TOCs", format!("{entries} entries"));
    } else {
        c.differ("archive TOCs", format!("{toc_diffs} archives differ"));
    }

    // GTXD and HD bindings.
    let theirs: HashMap<u32, u32> = cs.gtxd_hierarchy.iter().copied().collect();
    c.check("GTXD hierarchy", map_summary(&gd.gtxd_hierarchy, &theirs), map_summary(&theirs, &theirs));
    for i in 0..4 {
        let theirs: HashMap<u32, u32> = cs.hd_txd_bindings[i].iter().copied().collect();
        let what = format!("HD texture bindings [{}]", ["AT_TXD", "AT_DRB", "AT_DWD", "AT_FRG"][i]);
        c.check(&what, map_summary(&gd.hd_txd_bindings[i], &theirs), map_summary(&theirs, &theirs));
    }

    // The files read, in order.
    for (what, ours, theirs) in [
        ("ytyp stream entries (in order)", &gd.ytyp_files, &cs.ytyps),
        ("_cache.ych files read (in order)", &gd.ych_files, &cs.ychs),
        ("_manifest.ymf files read (in order)", &gd.ymf_files, &cs.ymfs),
    ] {
        let o: Vec<EntryBytes> = ours.iter().map(|&f| bytes(fs, f)).collect();
        let t: Vec<EntryBytes> = theirs.iter().map(|f| f.entry).collect();
        if o == t {
            c.same(what, format!("{}", o.len()));
        } else {
            let first = o.iter().zip(&t).position(|(a, b)| a != b).unwrap_or(o.len().min(t.len()));
            let os: HashSet<_> = o.iter().collect();
            let ts: HashSet<_> = t.iter().collect();
            c.differ(
                what,
                format!(
                    "ours {}, C# {}; {} only ours, {} only C#; first difference at {first}",
                    o.len(),
                    t.len(),
                    os.difference(&ts).count(),
                    ts.difference(&os).count()
                ),
            );
        }
    }
    let ytyp_table: Vec<FileRef> = gd.stream_entries[stream_index(74)].values().copied().collect();
    if ytyp_table == gd.ytyp_files {
        c.same("ytyp stream entries = ytyps read", format!("{}", ytyp_table.len()));
    } else {
        c.differ(
            "ytyp stream entries = ytyps read",
            format!("{} entries, {} read", ytyp_table.len(), gd.ytyp_files.len()),
        );
    }

    // The files' data: what the C# extracted is what we extract.
    let mut data_diffs = 0;
    for (ours, theirs) in [(&gd.ytyp_files, &cs.ytyps), (&gd.ych_files, &cs.ychs), (&gd.ymf_files, &cs.ymfs)] {
        for (&f, t) in ours.iter().zip(theirs.iter()) {
            if bytes(fs, f) == t.entry && fs.extract(f).ok().as_deref() != Some(&t.data[..]) {
                data_diffs += 1;
            }
        }
    }
    if data_diffs == 0 {
        c.same("files' data", "identical".into());
    } else {
        c.differ("files' data", format!("{data_diffs} files differ"));
    }

    // The archetypes, from the C#'s ytyp data, the last ytyp winning.
    let names: Vec<Option<Vec<u32>>> = cs
        .ytyps
        .par_iter()
        .map(|y| {
            let e = rf_formats::rpf8::Rpf8Entry::parse(&y.entry);
            let loose = rf_formats::rsc8::write_loose(e.virtual_flags, e.physical_flags, &y.data);
            let doc = Doc { bag: Meta::read(&loose).ok()?.root?, schema: None };
            Some(ytyp_archetypes(&doc, FileRef { archive: 0, entry: 0 }).iter().map(|a| a.name).collect())
        })
        .collect();
    let mut theirs: HashMap<u32, EntryBytes> = HashMap::new();
    let mut unread = 0;
    for (y, names) in cs.ytyps.iter().zip(names) {
        match names {
            Some(names) => theirs.extend(names.into_iter().map(|n| (n, y.entry))),
            None => unread += 1,
        }
    }
    if unread > 0 {
        c.differ("C# ytyps", format!("{unread} couldn't be read"));
    }
    let ours_keys: HashSet<u32> = gd.archetypes.keys().copied().collect();
    let theirs_keys: HashSet<u32> = theirs.keys().copied().collect();
    c.check(
        "archetype keys",
        (ours_keys.len(), ours_keys.difference(&theirs_keys).count()),
        (theirs_keys.len(), theirs_keys.difference(&ours_keys).count()),
    );
    let other_ytyp =
        gd.archetypes.values().filter(|a| theirs.get(&a.name).is_some_and(|e| *e != bytes(fs, a.ytyp))).count();
    if other_ytyp == 0 {
        c.same("each archetype's ytyp", format!("{}", gd.archetypes.len()));
    } else {
        c.differ("each archetype's ytyp", format!("{other_ytyp} archetypes come from another ytyp"));
    }

    // The other stream entries aren't in the cache.
    let counts: Vec<String> = (0..=u8::MAX)
        .filter_map(|id| {
            let n = gd.stream_entries.get(stream_index(id))?.len();
            let ext = rf_formats::rpf8::ext_name(id)?;
            (n > 0).then(|| format!("{ext} {n}"))
        })
        .collect();
    c.lines.push(format!("info  stream entries (only ytyp is in the C# cache): {}", counts.join(", ")));
    c
}

/// Size, plus the count of keys whose value differs from (or is missing in) the
/// other map.
fn map_summary(a: &HashMap<u32, u32>, b: &HashMap<u32, u32>) -> (usize, usize) {
    (a.len(), a.iter().filter(|(k, v)| b.get(k) != Some(v)).count())
}
