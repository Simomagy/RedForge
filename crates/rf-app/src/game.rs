//! The game's files: opened on a background thread when the app starts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use anyhow::{Context, Result};
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{FileRef, Rpf8FileSystem, Rpf8Keys};
use rf_game::GameData;
use rf_render::assets::AssetIndex;

/// A folder of the archives' file tree (archives count as folders).
#[derive(Default)]
pub struct Folder {
    pub folders: BTreeMap<String, Folder>,
    pub files: Vec<(String, FileRef)>,
}

impl Folder {
    fn insert(&mut self, path: &str, f: FileRef) {
        match path.split_once('/') {
            Some((head, rest)) => self.folders.entry(head.to_owned()).or_default().insert(rest, f),
            None => self.files.push((path.to_owned(), f)),
        }
    }

    fn sort(&mut self) {
        self.files.sort_by(|a, b| a.0.cmp(&b.0));
        self.folders.values_mut().for_each(Folder::sort);
    }
}

pub struct Game {
    pub folder: PathBuf,
    pub fs: Rpf8FileSystem,
    pub hashes: Arc<HashIndex>,
    pub index: AssetIndex,
    /// Archetypes, texture dictionary relationships and the like.
    pub data: GameData,
    pub tree: Folder,
    /// Every path, lowercase, for searching.
    pub paths: Vec<(String, FileRef)>,
}

impl Game {
    pub fn path(&self, f: FileRef) -> String {
        self.fs.path(f)
    }
}

/// The strings file next to the executable or in the source tree.
fn strings_file() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(dir) = std::env::var("REDFORGE_DATA") {
        candidates.push(PathBuf::from(dir).join("rdr2/strings.txt"));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join("data/rdr2/strings.txt"));
        candidates.push(dir.join("../../data/rdr2/strings.txt"));
    }
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/rdr2/strings.txt"));
    candidates.into_iter().find(|p| p.is_file())
}

fn load(folder: PathBuf, progress: impl Fn(&str)) -> Result<Game> {
    let t = Instant::now();
    progress("Reading the names of the game's files…");
    let hashes = HashIndex::new();
    let strings = strings_file().context("data/rdr2/strings.txt not found")?;
    hashes.load_file(&strings)?;
    progress("Finding the keys in RDR2.exe…");
    let keys = Rpf8Keys::from_game_folder(&folder)?;
    progress("Reading the archives…");
    let fs = Rpf8FileSystem::open(&folder, keys, &hashes)?;
    progress("Reading the game's data files…");
    let data = GameData::load(&fs, &hashes);
    progress("Indexing the files…");
    let index = AssetIndex::new(&fs);
    let mut tree = Folder::default();
    let mut paths = Vec::with_capacity(fs.file_count());
    for (lower, f) in fs.files() {
        paths.push((lower.to_owned(), f));
    }
    paths.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    for (_, f) in &paths {
        tree.insert(&fs.path(*f), *f);
    }
    tree.sort();
    log::info!("{} files in {} archives, ready in {:.2?}", fs.file_count(), fs.archives.len(), t.elapsed());
    Ok(Game { folder, fs, hashes: Arc::new(hashes), index, data, tree, paths })
}

pub enum LoadMessage {
    Progress(String),
    Done(Box<Result<Game>>),
}

/// Opens the game on a thread; poll the receiver every frame.
pub fn start_loading(folder: PathBuf, repaint: impl Fn() + Send + 'static) -> Receiver<LoadMessage> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let progress_tx = tx.clone();
        let result = load(folder, |s| {
            let _ = progress_tx.send(LoadMessage::Progress(s.to_owned()));
            repaint();
        });
        let _ = tx.send(LoadMessage::Done(Box::new(result)));
        repaint();
    });
    rx
}

/// Whether a folder looks like the game's.
pub fn is_game_folder(folder: &Path) -> bool {
    rf_formats::rpf8::find_case_insensitive(folder, "rdr2.exe").is_some()
}
