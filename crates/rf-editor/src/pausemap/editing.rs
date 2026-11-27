//! The pause map as a document of the editor: its objects in the project
//! (the `.rfproj`'s `pauseMap`), changed in undo steps like the files.

use std::path::Path;
use std::sync::Arc;

use serde_json::Value;

use super::export;
use super::library::Library;
use super::project::MapObject;
use super::project_file::{self, PauseMapData};
use crate::{DocKey, Editor, Project, Root};

/// What the project calls the pause map's document.
pub const DOC_NAME: &str = "Pause map";

impl Editor {
    /// What's drawn on the pause map now.
    pub fn pause_map(&self) -> &Arc<PauseMapData> {
        &self.pause_map
    }

    pub fn pause_map_dirty(&self) -> bool {
        !Arc::ptr_eq(&self.pause_map, &self.pause_map_saved) && *self.pause_map != *self.pause_map_saved
    }

    pub(crate) fn apply_pause_map(&mut self, root: Arc<PauseMapData>) {
        self.pause_map = root;
        self.decode_pictures();
    }

    /// The pause map as a project has it (or empty), unchanged since saved.
    pub(crate) fn load_pause_map(&mut self, json: Option<&Value>) {
        let data = Arc::new(json.map(project_file::read).unwrap_or_default());
        self.pause_map_saved = data.clone();
        self.apply_pause_map(data);
    }

    /// The pictures of the stamps decoded, for drawing them.
    fn decode_pictures(&mut self) {
        for (id, bytes) in &self.pause_map.pictures {
            if self.pause_assets.pictures.get(id).is_none()
                && let Err(e) = self.pause_assets.pictures.insert(id, bytes.to_vec())
            {
                log::warn!("pause map picture {id}: {e}");
            }
        }
    }

    /// Changes the pause map in one undo step (nothing when it's the same).
    pub fn edit_pause_map(&mut self, label: &str, data: PauseMapData) {
        if data == *self.pause_map {
            return;
        }
        self.project.get_or_insert_with(Project::new);
        self.commit_roots(label, vec![(DocKey::PauseMap, Root::PauseMap(Arc::new(data)))]);
    }

    /// [`edit_pause_map`](Self::edit_pause_map) with the objects changed.
    pub fn edit_pause_objects(&mut self, label: &str, objects: Vec<MapObject>) {
        let data = PauseMapData { objects, pictures: self.pause_map.pictures.clone() };
        self.edit_pause_map(label, data);
    }

    /// Adds a picture to the pause map's (not in an undo step: it's kept
    /// only while a stamp uses it). Returns its id and aspect.
    pub fn add_pause_picture(&mut self, bytes: Vec<u8>) -> Result<(String, f64), String> {
        let (id, aspect) = self.pause_assets.pictures.add(bytes.clone())?;
        let mut data = (*self.pause_map).clone();
        data.pictures.insert(id.clone(), Arc::new(bytes));
        // Kept in the current state and the saved one alike, so that adding a
        // picture alone changes nothing to save or undo.
        let saved_had = self.pause_map_saved.pictures.contains_key(&id);
        self.pause_map = Arc::new(data);
        if !saved_had && !self.pause_map_dirty() {
            self.pause_map_saved = self.pause_map.clone();
        }
        Ok((id, aspect))
    }

    /// The game's pause map tiles (read from the archives the first time,
    /// in about a second).
    pub fn pause_tiles(&self) -> Arc<Library> {
        self.pause_tiles.get_or_init(|| Arc::new(Library::load(&self.fs))).clone()
    }

    /// Whether the tiles were read already.
    pub fn pause_tiles_loaded(&self) -> bool {
        self.pause_tiles.get().is_some()
    }

    /// The edited tiles for the RedM resource's `stream` (none when nothing's
    /// drawn), and the tiles where marks can't show.
    pub fn pause_map_export(&self) -> Result<(export::TileFiles, Vec<String>), String> {
        if self.pause_map.objects.iter().all(MapObject::is_group) {
            return Ok((Vec::new(), Vec::new()));
        }
        let lib = self.pause_tiles();
        let (files, plan) = export::build(&lib, &self.pause_map.objects, &self.pause_assets)?;
        Ok((files, plan.skipped))
    }

    /// Opens a Map Editor web app or Aether Cartography project into the
    /// pause map, in one step: in place of nothing, or added on top. Returns
    /// what the user should know.
    pub fn import_map_project(&mut self, path: &Path) -> Result<Vec<String>, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let opened = project_file::open(&text, &self.pause_assets.fonts.names())?;
        let mut data = (*self.pause_map).clone();
        let taken: std::collections::HashSet<String> = data.objects.iter().map(|o| o.id().to_owned()).collect();
        // Ids that clash get new ones, and the members of a group follow it.
        let mut renamed = std::collections::HashMap::new();
        for o in &opened.data.objects {
            if taken.contains(o.id()) {
                renamed.insert(o.id().to_owned(), super::project::new_id());
            }
        }
        for mut o in opened.data.objects {
            let c = o.common_mut();
            if let Some(id) = renamed.get(&c.id) {
                c.id = id.clone();
            }
            if let Some(g) = c.group.as_ref().and_then(|g| renamed.get(g)) {
                c.group = Some(g.clone());
            }
            data.objects.push(o);
        }
        for (id, bytes) in opened.data.pictures {
            data.pictures.entry(id).or_insert(bytes);
        }
        let label = if opened.aether { "Import Aether project" } else { "Import map project" };
        self.edit_pause_map(label, data);
        Ok(opened.notes)
    }

    /// Writes the pause map as a web app project (`map-project.json`).
    pub fn export_map_project(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&self.pause_map.to_json()).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| format!("{}: {e}", path.display()))
    }
}
