//! Exporting the game's files into a folder, as the C#'s Game Files and
//! Export Selection did: as XML (Sollumz and CodeX's, textures as DDS files
//! next to it), extracted as RedM loads them from a resource, or
//! decompressed; and the selection's models with the textures they use. On a
//! thread of its own: a folder of an archive can be thousands of files.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use rf_bridge::{ModelSource, export_model};
use rf_formats::formats::{self, Kind, XmlContext, externals_folder};
use rf_formats::rpf8::FileRef;

use crate::game::Game;
use crate::settings::Settings;

/// The folder last exported to.
const FOLDER_SETTING: &str = "Explorer.SelectedFolder";
/// Most errors a message lists.
const ERRORS_SHOWN: usize = 3;

/// How game files are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    /// As XML, with their textures and other externals in a folder next to it.
    Xml,
    /// As loose files, the way a RedM resource streams them (resources with
    /// their `RSC8` header, compressed as the game has them).
    Loose,
    /// Decrypted and decompressed.
    Uncompressed,
}

/// An export running.
pub struct Job {
    done: Receiver<Result<String, String>>,
}

impl Job {
    /// Its message once it's done: what it wrote, or what went wrong.
    pub fn poll(&mut self) -> Option<Result<String, String>> {
        match self.done.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("The export stopped".into())),
        }
    }
}

/// Asks where to export, starting from the last folder; `None` when the
/// dialog was cancelled.
pub fn choose_folder(settings: &mut Settings, title: &str) -> Option<PathBuf> {
    // (tests: the folder without asking)
    if let Some(folder) = std::env::var_os("REDFORGE_TEST_EXPORT") {
        return Some(folder.into());
    }
    let mut dialog = rfd::FileDialog::new().set_title(title);
    if let Some(last) = settings.get(FOLDER_SETTING).map(PathBuf::from).filter(|p| p.is_dir()) {
        dialog = dialog.set_directory(last);
    }
    let folder = dialog.pick_folder()?;
    settings.set(FOLDER_SETTING, folder.to_string_lossy());
    Some(folder)
}

/// Writes game files into `out`, each at its path there (a file's name, or
/// its place under the folder that was exported).
pub fn game_files(game: &Arc<Game>, files: Vec<(FileRef, PathBuf)>, how: How, out: PathBuf) -> Job {
    let game = game.clone();
    run(move || {
        let (mut written, mut skipped) = (0, 0);
        let mut errors = Vec::new();
        for (f, rel) in &files {
            let dest = out.join(rel);
            match write_file(&game, *f, how, &dest) {
                Ok(true) => written += 1,
                Ok(false) => skipped += 1,
                Err(e) => errors.push(format!("{}: {e}", rel.display())),
            }
        }
        if written == 0 && errors.is_empty() && skipped > 0 {
            return Err(format!("RedForge has no XML for {}", if skipped == 1 { "this file" } else { "these files" }));
        }
        let what = match how {
            How::Xml => "as XML",
            How::Loose => "extracted",
            How::Uncompressed => "extracted, uncompressed",
        };
        report(format!("{} {what} to {}", count(written, "file"), out.display()), written, &errors)
    })
}

/// Writes the selection's models as XML into `out`, each with its textures,
/// its own and its texture dictionaries', as DDS files in a folder named
/// after it: what Sollumz imports.
pub fn models(game: &Arc<Game>, archetypes: Vec<u32>, out: PathBuf) -> Job {
    let game = game.clone();
    run(move || {
        let data = game.data();
        let mut files = HashSet::new();
        let mut errors = Vec::new();
        for a in archetypes {
            let name = game.hashes.name(a);
            let Some(source) = ModelSource::of_archetype(&game.fs, &data, &game.hashes, a) else {
                errors.push(format!("{name}: no model"));
                continue;
            };
            // Objects with the same model write it once.
            if !files.insert(source.file.to_lowercase()) {
                continue;
            }
            let written = (source.load)().and_then(|m| export_model(&source.file, &m, &game.hashes, &out));
            if let Err(e) = written {
                files.remove(&source.file.to_lowercase());
                errors.push(format!("{}: {e}", source.file));
            }
        }
        report(format!("{} as XML to {}", count(files.len(), "model"), out.display()), files.len(), &errors)
    })
}

/// Writes one file; `false` for one skipped (no XML for its kind).
fn write_file(game: &Game, f: FileRef, how: How, dest: &Path) -> Result<bool, String> {
    let dir = dest.parent().unwrap_or(Path::new(""));
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let write = |path: &Path, data: &[u8]| std::fs::write(path, data).map_err(|e| e.to_string());
    match how {
        How::Loose => write(dest, &game.fs.extract_loose(f).map_err(|e| e.to_string())?)?,
        How::Uncompressed => write(dest, &game.fs.extract(f).map_err(|e| e.to_string())?)?,
        How::Xml => {
            let data = game.fs.extract_loose(f).map_err(|e| e.to_string())?;
            let Some(kind) = Kind::of_file(&name, &data) else { return Ok(false) };
            let folder = kind.has_folder().then(|| externals_folder(dir, &name));
            let cx = XmlContext { hashes: &game.hashes, folder };
            let xml = formats::to_xml(&name, &data, &cx).map_err(|e| e.to_string())?;
            write(&dir.join(kind.xml_file_name(&name)), xml.as_bytes())?;
        }
    }
    Ok(true)
}

fn run(work: impl FnOnce() -> Result<String, String> + Send + 'static) -> Job {
    let (tx, done) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    Job { done }
}

/// What an export did: an error when nothing was written.
fn report(done: String, written: usize, errors: &[String]) -> Result<String, String> {
    if errors.is_empty() {
        return Ok(done);
    }
    let mut text = format!("{done}; {} failed: {}", errors.len(), errors[..errors.len().min(ERRORS_SHOWN)].join("; "));
    if errors.len() > ERRORS_SHOWN {
        text.push('…');
    }
    for e in errors {
        log::warn!("export: {e}");
    }
    if written == 0 { Err(text) } else { Ok(text) }
}

fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}
