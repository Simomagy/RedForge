//! Saving safely, and autosaving: files are written whole or not at all,
//! with the one replaced kept as `<name>.bak`; and the changed files can be
//! written somewhere else now and then, to put back if RedForge closes
//! before they're saved.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rf_formats::formats::meta::Meta;
use serde::{Deserialize, Serialize};

use crate::{DocKey, Editor, MapDoc};

/// Writes a file whole or not at all (a temporary file renamed over it),
/// keeping the one it replaces as `<name>.bak`.
pub fn write_safely(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let error = |e: std::io::Error| format!("{}: {e}", path.display());
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temporary = path.with_file_name(format!("{name}.tmp"));
    std::fs::write(&temporary, bytes).map_err(error)?;
    if path.is_file() {
        let _ = std::fs::copy(path, path.with_file_name(format!("{name}.bak")));
    }
    std::fs::rename(&temporary, path).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        error(e)
    })
}

/// What an autosave holds.
#[derive(Serialize, Deserialize)]
pub struct Autosave {
    /// The project open then.
    pub project: Option<PathBuf>,
    /// When, in seconds since 1970.
    pub time: u64,
    pub files: Vec<Saved>,
}

/// A file in an autosave: which document, and the file in its folder.
#[derive(Serialize, Deserialize)]
pub struct Saved {
    /// `ymap` or `ytyp`.
    pub kind: String,
    /// The ymap's file name, or the ytyp's path in the game's files.
    pub name: String,
    pub file: String,
}

const INDEX: &str = "autosave.json";

impl Autosave {
    /// The autosave in a folder, if there's one.
    pub fn read(dir: &Path) -> Option<Self> {
        serde_json::from_str(&std::fs::read_to_string(dir.join(INDEX)).ok()?).ok()
    }
}

impl Editor {
    /// Writes the changed files, as saving would, into `dir` (what was there
    /// goes), with what they are. Returns how many.
    pub fn autosave(&self, dir: &Path, project: Option<&Path>) -> Result<usize, String> {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut files = Vec::new();
        for (i, e) in self.project_entries().into_iter().filter(|e| e.dirty).enumerate() {
            let (kind, name, bytes) = match e.doc {
                Some(DocKey::Map(n)) => {
                    let doc = self.maps.get(&n).ok_or("no such ymap")?;
                    ("ymap", doc.file_name.clone(), self.map_bytes(n)?)
                }
                Some(DocKey::Types(f)) => ("ytyp", self.fs.path(f), self.types_bytes(f)?),
                None => continue,
            };
            let file = format!("{i}.{kind}");
            std::fs::write(dir.join(&file), bytes).map_err(|e| e.to_string())?;
            files.push(Saved { kind: kind.into(), name, file });
        }
        let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let index = Autosave { project: project.map(Path::to_owned), time, files };
        let json = serde_json::to_string_pretty(&index).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(INDEX), json).map_err(|e| e.to_string())?;
        Ok(index.files.len())
    }

    /// Puts an autosave's files back, as one step: they're the documents'
    /// unsaved changes then, and saving writes them where they go. Returns
    /// how many came back.
    pub fn restore_autosave(&mut self, dir: &Path) -> Result<usize, String> {
        let index = Autosave::read(dir).ok_or("no autosave there")?;
        let mut roots = Vec::new();
        for s in &index.files {
            let bytes = std::fs::read(dir.join(&s.file)).map_err(|e| format!("{}: {e}", s.name))?;
            let mut header = Meta::read(&bytes).map_err(|e| format!("{}: {e}", s.name))?;
            let root = header.root.take().ok_or_else(|| format!("{}: no data", s.name))?;
            match s.kind.as_str() {
                "ymap" => {
                    let name = rf_formats::hash::joaat(&s.name);
                    // A ymap of the game or of the project, or one made in
                    // the editor and never saved.
                    if self.open(name).is_err() {
                        let root = Arc::new(root.clone());
                        let doc = MapDoc {
                            name,
                            file_name: s.name.clone(),
                            source: None,
                            header,
                            root,
                            original: None,
                            saved: None,
                            path: None,
                            exists: false,
                            in_project: true,
                        };
                        self.maps.insert(name, doc);
                    }
                    roots.push((DocKey::Map(name), root));
                }
                "ytyp" => {
                    let file = self.fs.find(&s.name).ok_or_else(|| format!("{}: not in the game's files", s.name))?;
                    self.open_types(file)?;
                    roots.push((DocKey::Types(file), root));
                }
                _ => {}
            }
        }
        let n = roots.len();
        if n > 0 {
            self.commit("Restore unsaved changes", roots, Vec::new());
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_writes() {
        let dir = std::env::temp_dir().join(format!("rf-safe-writes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.ymap");
        write_safely(&file, b"one").unwrap();
        assert!(!dir.join("a.ymap.bak").exists());
        write_safely(&file, b"two").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"two");
        assert_eq!(std::fs::read(dir.join("a.ymap.bak")).unwrap(), b"one");
        assert!(!dir.join("a.ymap.tmp").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
