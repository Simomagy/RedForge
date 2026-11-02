use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use super::{Rpf8Archive, Rpf8Entry, Rpf8Error, Rpf8Keys};
use crate::hash::HashIndex;

/// A file inside the archives: (archive index, entry index).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileRef {
    pub archive: u32,
    pub entry: u32,
}

/// An archive that couldn't be read. The others still load.
#[derive(Debug)]
pub struct LoadError {
    pub path: String,
    pub error: Rpf8Error,
}

/// All the RPF8 archives of a game folder, nested ones included, with every file
/// by path.
pub struct Rpf8FileSystem {
    pub folder: PathBuf,
    pub keys: Rpf8Keys,
    /// The archive files on disk, by [`Rpf8Archive::root`].
    roots: Vec<(PathBuf, File)>,
    /// Root archives first, each followed by the archives nested in it.
    pub archives: Vec<Rpf8Archive>,
    /// Lowercase full path → file.
    files: HashMap<Box<str>, FileRef>,
    pub errors: Vec<LoadError>,
}

impl Rpf8FileSystem {
    /// Reads the TOC of every `.rpf` under `folder`, in parallel.
    pub fn open(folder: &Path, keys: Rpf8Keys, strings: &HashIndex) -> Result<Self, Rpf8Error> {
        let mut paths = Vec::new();
        find_rpfs(folder, &mut paths)?;
        paths.sort();

        let mut roots = Vec::with_capacity(paths.len());
        for p in paths {
            let f = File::open(&p)?;
            roots.push((p, f));
        }

        let loaded: Vec<(Vec<Rpf8Archive>, Vec<LoadError>)> = roots
            .par_iter()
            .enumerate()
            .map(|(root, (path, file))| {
                let rel = path.strip_prefix(folder).unwrap_or(path);
                let rel = rel.to_string_lossy().replace('\\', "/").to_lowercase();
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let mut archives = Vec::new();
                let mut errors = Vec::new();
                let size = match file.metadata() {
                    Ok(m) => m.len(),
                    Err(e) => {
                        errors.push(LoadError { path: rel, error: e.into() });
                        return (archives, errors);
                    }
                };
                if size > 0 {
                    load_tree(file, root, 0, size, name, rel, None, &keys, strings, &mut archives, &mut errors);
                }
                (archives, errors)
            })
            .collect();

        let mut fs = Self {
            folder: folder.to_owned(),
            keys,
            roots,
            archives: Vec::new(),
            files: HashMap::new(),
            errors: Vec::new(),
        };
        for (mut archives, errors) in loaded {
            // Parent indices are local to each root's list until now.
            let base = fs.archives.len();
            for a in &mut archives {
                if let Some((p, _)) = a.parent.as_mut() {
                    *p += base;
                }
            }
            fs.archives.append(&mut archives);
            fs.errors.extend(errors);
        }
        fs.build_index();
        Ok(fs)
    }

    fn build_index(&mut self) {
        let count: usize = self.archives.iter().map(|a| a.entries.len()).sum();
        self.files.reserve(count);
        for (ai, a) in self.archives.iter().enumerate() {
            for ei in 0..a.entries.len() {
                let path = a.entry_path(ei).to_lowercase().into_boxed_str();
                self.files.insert(path, FileRef { archive: ai as u32, entry: ei as u32 });
            }
        }
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Finds a file by its full path (`levels_0.rpf/x64/.../foo.ymap`), ignoring
    /// case and accepting `\` separators.
    pub fn find(&self, path: &str) -> Option<FileRef> {
        let key = path.replace('\\', "/").to_lowercase();
        self.files.get(key.as_str()).copied()
    }

    /// Every file, in no particular order.
    pub fn files(&self) -> impl Iterator<Item = (&str, FileRef)> {
        self.files.iter().map(|(k, v)| (&**k, *v))
    }

    pub fn archive(&self, f: FileRef) -> &Rpf8Archive {
        &self.archives[f.archive as usize]
    }

    pub fn entry(&self, f: FileRef) -> &Rpf8Entry {
        &self.archive(f).entries[f.entry as usize]
    }

    pub fn path(&self, f: FileRef) -> String {
        self.archive(f).entry_path(f.entry as usize)
    }

    /// The archive file on disk that holds an archive.
    pub fn root_path(&self, archive: &Rpf8Archive) -> &Path {
        &self.roots[archive.root].0
    }

    fn file_of(&self, f: FileRef) -> &File {
        &self.roots[self.archive(f).root].1
    }

    /// A file's data, decrypted and decompressed.
    pub fn extract(&self, f: FileRef) -> Result<Vec<u8>, Rpf8Error> {
        self.archive(f).extract(self.file_of(f), f.entry as usize, &self.keys)
    }

    /// A file as it would be on disk: resources with an `RSC8` header.
    pub fn extract_loose(&self, f: FileRef) -> Result<Vec<u8>, Rpf8Error> {
        self.archive(f).extract_loose(self.file_of(f), f.entry as usize, &self.keys)
    }

    /// A resource as a loose file compressed with deflate, the way RedForge saves
    /// them; other files are just decompressed.
    pub fn extract_loose_deflate(&self, f: FileRef) -> Result<Vec<u8>, Rpf8Error> {
        let e = *self.entry(f);
        let data = self.extract(f)?;
        if !e.is_resource() {
            return Ok(data);
        }
        Ok(crate::rsc8::write_loose(e.virtual_flags, e.physical_flags, &data))
    }

    /// The archive an `.rpf` entry opens as, if it was read.
    pub fn child_archive(&self, f: FileRef) -> Option<usize> {
        self.archives.iter().position(|a| a.parent == Some((f.archive as usize, f.entry as usize)))
    }
}

fn find_rpfs(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), Rpf8Error> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let ft = e.file_type()?;
        let p = e.path();
        if ft.is_dir() {
            find_rpfs(&p, out)?;
        } else if ft.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("rpf")) {
            out.push(p);
        }
    }
    Ok(())
}

/// Reads an archive and, depth first, the archives nested in it.
#[allow(clippy::too_many_arguments)]
fn load_tree(
    file: &File,
    root: usize,
    start: u64,
    size: u64,
    name: String,
    path: String,
    parent: Option<(usize, usize)>,
    keys: &Rpf8Keys,
    strings: &HashIndex,
    archives: &mut Vec<Rpf8Archive>,
    errors: &mut Vec<LoadError>,
) {
    let mut archive = match Rpf8Archive::read(file, root, start, size, name, path.clone(), keys, strings) {
        Ok(a) => a,
        Err(error) => {
            errors.push(LoadError { path, error });
            return;
        }
    };
    archive.parent = parent;
    let index = archives.len();
    let children: Vec<(usize, u64, u64, String, String)> = archive
        .entries
        .iter()
        .enumerate()
        .filter(|(i, e)| !e.is_resource() && archive.names[*i].to_ascii_lowercase().ends_with(".rpf"))
        .map(|(i, e)| {
            let full = archive.entry_path(i).to_lowercase();
            let name = archive.names[i].rsplit('/').next().unwrap_or_default().to_owned();
            (i, start + e.offset(), e.file_size(), name, full)
        })
        .collect();
    archives.push(archive);
    for (entry, cstart, csize, cname, cpath) in children {
        load_tree(file, root, cstart, csize, cname, cpath, Some((index, entry)), keys, strings, archives, errors);
    }
}
