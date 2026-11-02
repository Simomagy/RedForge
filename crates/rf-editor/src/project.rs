//! Projects: the files a mod is made of, on disk, as the C# RedForge keeps
//! them in a `.rfproj`. That's JSON, with paths relative to it so the
//! project's folder can be moved or shared:
//!
//! ```json
//! {
//!   "game": "RDR2",
//!   "folders": ["maps"],
//!   "files": ["../other/thing.ymap"],
//!   "excluded": ["maps/old.ymap"],
//!   "export": { "author": "..." }
//! }
//! ```
//!
//! A folder brings in every game file (`.y*`) in it and its subfolders,
//! except the excluded ones; `files` are those from anywhere else. Paths are
//! written with `/` (which Windows reads too) and read with either separator.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The `.rfproj` as written: the C#'s `ProjectFile`, camelCase.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ProjectFile {
    game: Option<String>,
    folders: Vec<String>,
    files: Vec<String>,
    excluded: Vec<String>,
    export: BTreeMap<String, String>,
}

/// The order the game loads files in, by extension (`RDR2Project.FileLoadOrder`);
/// `*` is everything else.
pub const LOAD_ORDER: [&str; 8] = ["ytd", "ydr", "ydd", "yft", "ybn", "ytyp", "ymap", "*"];

/// Where a file comes in [`LOAD_ORDER`].
pub fn load_order(path: &Path) -> usize {
    let ext = extension(path);
    LOAD_ORDER.iter().position(|e| *e == ext).unwrap_or(LOAD_ORDER.len() - 1)
}

/// A file's extension, lowercase.
pub fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Whether a file is one of the game's (`.ymap`, `.ytyp`, `.ydr`, `.ytd`, `.ybn`...).
pub fn is_game_file(path: &Path) -> bool {
    extension(path).starts_with('y')
}

#[derive(Clone, Debug)]
pub struct Project {
    /// The `.rfproj`; `None` until it's saved.
    pub file: Option<PathBuf>,
    pub game: String,
    /// Absolute.
    pub folders: Vec<PathBuf>,
    /// Absolute; the files not in a folder.
    pub files: Vec<PathBuf>,
    /// Files in the folders that were taken out of the project.
    pub excluded: BTreeSet<PathBuf>,
    /// The export settings, kept with the project.
    pub export: BTreeMap<String, String>,
    /// What the `.rfproj` holds, so it's only written when something changed.
    saved_json: Option<String>,
}

/// A path relative to `dir`, with `/` (`..` where needed).
fn relative(dir: &Path, path: &Path) -> String {
    let base: Vec<Component> = dir.components().collect();
    let target: Vec<Component> = path.components().collect();
    let common = base.iter().zip(&target).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return path.to_string_lossy().replace('\\', "/");
    }
    let mut parts: Vec<String> = vec!["..".into(); base.len() - common];
    parts.extend(target[common..].iter().map(|c| c.as_os_str().to_string_lossy().into_owned()));
    if parts.is_empty() { ".".into() } else { parts.join("/") }
}

/// A path from the `.rfproj`, made absolute against `dir`.
fn absolute(dir: &Path, path: &str) -> PathBuf {
    let path = path.replace('\\', "/");
    let p = Path::new(&path);
    let joined = if p.is_absolute() { p.to_path_buf() } else { dir.join(p) };
    // Resolve `..` and `.` without touching the disk (`Path.GetFullPath`).
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

impl Default for Project {
    fn default() -> Self {
        Self {
            file: None,
            game: "RDR2".into(),
            folders: Vec::new(),
            files: Vec::new(),
            excluded: BTreeSet::new(),
            export: BTreeMap::new(),
            saved_json: None,
        }
    }
}

impl Project {
    /// A new project, not saved yet.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(file: &Path) -> Result<Self, String> {
        let json = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
        let data: ProjectFile = serde_json::from_str(json.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("{}: {e}", file.display()))?;
        let dir = file.parent().unwrap_or(Path::new("."));
        Ok(Self {
            file: Some(file.to_path_buf()),
            game: data.game.unwrap_or_else(|| "RDR2".into()),
            folders: data.folders.iter().map(|f| absolute(dir, f)).collect(),
            files: data.files.iter().map(|f| absolute(dir, f)).collect(),
            excluded: data.excluded.iter().map(|f| absolute(dir, f)).collect(),
            export: data.export,
            saved_json: Some(json),
        })
    }

    /// Writes the `.rfproj` (to `file`, or where it was) when something
    /// changed. Returns whether it wrote.
    pub fn save(&mut self, file: Option<&Path>) -> Result<bool, String> {
        let file = file.map(Path::to_path_buf).or_else(|| self.file.clone()).ok_or("the project has no file yet")?;
        let dir = file.parent().unwrap_or(Path::new("."));
        let rel = |p: &PathBuf| relative(dir, p);
        let data = ProjectFile {
            game: Some(self.game.clone()),
            folders: self.folders.iter().map(rel).collect(),
            files: self.files.iter().filter(|f| !self.in_folders(f)).map(rel).collect(),
            excluded: self.excluded.iter().map(rel).collect(),
            export: self.export.clone(),
        };
        let json = serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?;
        if self.file.as_deref() == Some(file.as_path()) && self.saved_json.as_deref() == Some(json.as_str()) {
            return Ok(false);
        }
        std::fs::write(&file, &json).map_err(|e| format!("{}: {e}", file.display()))?;
        self.saved_json = Some(json);
        self.file = Some(file);
        Ok(true)
    }

    /// The `.rfproj`'s folder, where new files are saved.
    pub fn dir(&self) -> Option<&Path> {
        self.file.as_deref().and_then(Path::parent)
    }

    pub fn name(&self) -> String {
        self.file
            .as_deref()
            .and_then(Path::file_stem)
            .map_or_else(|| "New project".into(), |s| s.to_string_lossy().into_owned())
    }

    pub fn in_folders(&self, path: &Path) -> bool {
        self.folders.iter().any(|f| path.starts_with(f) && path != f)
    }

    /// Every file of the project, in the game's load order: the folders' game
    /// files (but the excluded ones), and the others.
    pub fn items(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for folder in &self.folders {
            walk(folder, &mut out);
        }
        out.retain(|f| is_game_file(f) && !self.excluded.contains(f));
        for f in &self.files {
            if !out.contains(f) {
                out.push(f.clone());
            }
        }
        out.sort_by(|a, b| load_order(a).cmp(&load_order(b)).then_with(|| a.cmp(b)));
        out
    }

    /// Adds a file (taking it back if it was excluded).
    pub fn add_file(&mut self, path: &Path) {
        self.excluded.remove(path);
        if !self.in_folders(path) && !self.files.iter().any(|f| f == path) {
            self.files.push(path.to_path_buf());
        }
    }

    /// Takes a file out; one in a folder stays out (`excluded`).
    pub fn remove_file(&mut self, path: &Path) {
        self.files.retain(|f| f != path);
        if self.in_folders(path) {
            self.excluded.insert(path.to_path_buf());
        }
    }

    /// Adds a folder, unless a folder it's in is already there (its
    /// subfolders are then covered by it).
    pub fn add_folder(&mut self, folder: &Path) {
        if self.folders.iter().any(|f| folder.starts_with(f)) {
            return;
        }
        self.folders.retain(|f| !f.starts_with(folder));
        self.folders.push(folder.to_path_buf());
    }

    pub fn remove_folder(&mut self, folder: &Path) {
        self.folders.retain(|f| f != folder);
        self.excluded.retain(|f| !f.starts_with(folder));
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_csharp_format_and_writes_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("maps/sub")).unwrap();
        for f in ["maps/a.ymap", "maps/sub/b.ytyp", "maps/old.ymap", "maps/notes.txt"] {
            std::fs::write(root.join(f), b"x").unwrap();
        }
        std::fs::create_dir_all(root.join("other")).unwrap();
        std::fs::write(root.join("other/c.ydr"), b"x").unwrap();
        // As the C# writes it on Windows: camelCase, backslashes.
        let json = r#"{
  "game": "RDR2",
  "folders": [ "../maps" ],
  "files": [ "..\\other\\c.ydr" ],
  "excluded": [ "..\\maps\\old.ymap" ],
  "export": { "author": "me" }
}"#;
        let file = root.join("proj/test.rfproj");
        std::fs::create_dir_all(root.join("proj")).unwrap();
        std::fs::write(&file, json).unwrap();
        let mut p = Project::load(&file).unwrap();
        assert_eq!(p.folders, [root.join("maps")]);
        assert_eq!(p.export["author"], "me");
        let items = p.items();
        // Game files only, in load order: ydr, ytyp, ymap.
        assert_eq!(items, [root.join("other/c.ydr"), root.join("maps/sub/b.ytyp"), root.join("maps/a.ymap")]);

        p.remove_file(&root.join("maps/a.ymap"));
        assert!(p.excluded.contains(&root.join("maps/a.ymap")));
        p.add_file(&root.join("maps/a.ymap"));
        assert!(!p.excluded.contains(&root.join("maps/a.ymap")) && !p.files.contains(&root.join("maps/a.ymap")));
        // The first save writes it the way this writes JSON; then only changes do.
        assert!(p.save(None).unwrap());
        assert!(!p.save(None).unwrap(), "nothing changed, so nothing written");
        p.export.insert("version".into(), "1.0".into());
        assert!(p.save(None).unwrap());
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"../maps\"") && text.contains("\"../other/c.ydr\"") && text.contains("\"version\""));
        let back = Project::load(&file).unwrap();
        assert_eq!(back.items(), items);
    }

    #[test]
    fn folders_cover_their_subfolders() {
        let mut p = Project::new();
        p.add_folder(Path::new("/m/maps/sub"));
        p.add_folder(Path::new("/m/maps"));
        assert_eq!(p.folders, [PathBuf::from("/m/maps")]);
        p.add_folder(Path::new("/m/maps/other"));
        assert_eq!(p.folders.len(), 1);
        assert_eq!(relative(Path::new("/a/b"), Path::new("/a/c/d.ymap")), "../c/d.ymap");
        assert_eq!(absolute(Path::new("/a/b"), "..\\c\\d.ymap"), PathBuf::from("/a/c/d.ymap"));
    }
}
