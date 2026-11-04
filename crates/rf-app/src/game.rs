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
    pub fs: Arc<Rpf8FileSystem>,
    pub hashes: Arc<HashIndex>,
    pub index: AssetIndex,
    /// Archetypes, texture dictionary relationships and the like: the game's,
    /// or with the project's files on top (see [`Game::set_data`]), and how
    /// many times it changed.
    data: std::sync::RwLock<(u64, Arc<GameData>)>,
    pub tree: Folder,
    /// Every path, lowercase, for searching.
    pub paths: Vec<(String, FileRef)>,
}

impl Game {
    /// The game data now.
    pub fn data(&self) -> Arc<GameData> {
        self.data.read().unwrap().1.clone()
    }

    /// Changes each time the data does, for caches built from it.
    pub fn data_version(&self) -> u64 {
        self.data.read().unwrap().0
    }

    /// Uses other game data (the project's files on top of the game's).
    pub fn set_data(&self, data: Arc<GameData>) {
        let mut d = self.data.write().unwrap();
        if !Arc::ptr_eq(&d.1, &data) {
            *d = (d.0 + 1, data);
        }
    }

    pub fn path(&self, f: FileRef) -> String {
        self.fs.path(f)
    }
}

/// The strings file next to the executable (as the Windows installer puts
/// it), in `../share/redforge` from it (as the Linux packages do), or in the
/// source tree.
fn strings_file() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(dir) = std::env::var("REDFORGE_DATA") {
        candidates.push(PathBuf::from(dir).join("rdr2/strings.txt"));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join("data/rdr2/strings.txt"));
        candidates.push(dir.join("../share/redforge/data/rdr2/strings.txt"));
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
    log::info!("names of the files read ({:.2?})", t.elapsed());
    progress("Finding the keys in RDR2.exe…");
    let keys = Rpf8Keys::from_game_folder(&folder)?;
    log::info!("keys found ({:.2?})", t.elapsed());
    progress("Reading the archives…");
    let fs = Rpf8FileSystem::open(&folder, keys, &hashes)?;
    log::info!("archives read ({:.2?})", t.elapsed());
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
    Ok(Game {
        folder,
        fs: Arc::new(fs),
        hashes: Arc::new(hashes),
        index,
        data: std::sync::RwLock::new((0, Arc::new(data))),
        tree,
        paths,
    })
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

/// Where the launchers say the game is installed, as the C# looks for it:
/// the folders that have RDR2.exe, with the launcher's name.
pub fn find_installs() -> Vec<(PathBuf, &'static str)> {
    let mut found: Vec<(PathBuf, &'static str)> = Vec::new();
    for (dir, launcher) in install_candidates() {
        let same = |d: &PathBuf| d == &dir || d.canonicalize().ok() == dir.canonicalize().ok();
        if is_game_folder(&dir) && !found.iter().any(|(d, _)| same(d)) {
            found.push((dir, launcher));
        }
    }
    found
}

const STEAM_FOLDER: &str = "steamapps/common/Red Dead Redemption 2";

/// Steam's libraries: its own folder and the ones `libraryfolders.vdf` lists.
fn steam_libraries(steam: &Path) -> Vec<PathBuf> {
    let mut libraries = vec![steam.to_path_buf()];
    if let Ok(vdf) = std::fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf")) {
        libraries.extend(quoted_values(&vdf, "path").into_iter().map(PathBuf::from));
    }
    libraries
}

/// The values of a key's `"key" "value"` (VDF) or `"key": "value"` (JSON)
/// pairs, their `\\` unescaped.
fn quoted_values(text: &str, key: &str) -> Vec<String> {
    let quoted = format!("\"{key}\"");
    let value = |at: usize| {
        let rest = text[at + quoted.len()..].trim_start().trim_start_matches(':').trim_start();
        let value = rest.strip_prefix('"')?;
        let end = value.find('"')?;
        Some(value[..end].replace("\\\\", "\\"))
    };
    text.match_indices(&quoted).filter_map(|(at, _)| value(at)).collect()
}

#[cfg(windows)]
fn install_candidates() -> Vec<(PathBuf, &'static str)> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    let read = |root, key: &str, value: &str| -> Option<PathBuf> {
        RegKey::predef(root).open_subkey(key).ok()?.get_value::<String, _>(value).ok().map(PathBuf::from)
    };
    let mut found = Vec::new();
    if let Some(dir) =
        read(HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Rockstar Games\Red Dead Redemption 2", "InstallFolder")
    {
        found.push((dir, "Rockstar Games Launcher"));
    }
    found.push((PathBuf::from(r"C:\Program Files\Rockstar Games\Red Dead Redemption 2"), "Rockstar Games Launcher"));
    let steam = read(HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Valve\Steam", "InstallPath")
        .or_else(|| read(HKEY_CURRENT_USER, r"SOFTWARE\Valve\Steam", "SteamPath"))
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)\Steam"));
    for library in steam_libraries(&steam) {
        found.push((library.join(STEAM_FOLDER), "Steam"));
    }
    let program_data = std::env::var_os("ProgramData").map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from);
    let manifests = program_data.join(r"Epic\EpicGamesLauncher\Data\Manifests");
    for item in std::fs::read_dir(manifests).into_iter().flatten().flatten() {
        let Ok(text) = std::fs::read_to_string(item.path()) else { continue };
        if text.contains("Red Dead Redemption 2") {
            found.extend(
                quoted_values(&text, "InstallLocation").into_iter().map(|d| (PathBuf::from(d), "Epic Games Launcher")),
            );
        }
    }
    found.push((PathBuf::from(r"C:\Program Files\Epic Games\RedDeadRedemption2"), "Epic Games Launcher"));
    found
}

/// On Linux: Steam's libraries (native and Flatpak), Heroic's and
/// Legendary's Epic installs, and the Rockstar Games Launcher in the Wine
/// prefixes under `~/Games` (Lutris').
#[cfg(not(windows))]
fn install_candidates() -> Vec<(PathBuf, &'static str)> {
    let Some(home) = dirs::home_dir() else { return Vec::new() };
    let mut found = Vec::new();
    for steam in [".local/share/Steam", ".steam/steam", ".var/app/com.valvesoftware.Steam/.local/share/Steam"] {
        for library in steam_libraries(&home.join(steam)) {
            found.push((library.join(STEAM_FOLDER), "Steam"));
        }
    }
    for (installed, launcher) in [
        (".config/heroic/legendaryConfig/legendary/installed.json", "Heroic"),
        (".var/app/com.heroicgameslauncher.hgl/config/heroic/legendaryConfig/legendary/installed.json", "Heroic"),
        (".config/legendary/installed.json", "Legendary"),
    ] {
        if let Ok(text) = std::fs::read_to_string(home.join(installed)) {
            found.extend(quoted_values(&text, "install_path").into_iter().map(|d| (PathBuf::from(d), launcher)));
        }
    }
    for prefix in std::fs::read_dir(home.join("Games")).into_iter().flatten().flatten() {
        let dir = prefix.path().join("drive_c/Program Files/Rockstar Games/Red Dead Redemption 2");
        found.push((dir, "Rockstar Games Launcher"));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::quoted_values;

    #[test]
    fn library_folders() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/me/.local/share/Steam\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}";
        let paths = quoted_values(vdf, "path");
        assert_eq!(paths, ["/home/me/.local/share/Steam", "D:\\SteamLibrary"]);
        let json = r#"{"Heroic": {"install_path": "/home/me/Games/Heroic/RDR2", "title": "x"}}"#;
        assert_eq!(quoted_values(json, "install_path"), ["/home/me/Games/Heroic/RDR2"]);
    }
}
