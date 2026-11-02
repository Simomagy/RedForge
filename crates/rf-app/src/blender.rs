//! The app's side of the Blender bridge ([`rf_bridge`]): the selection's
//! models and placements for Blender, the models it sends back into the
//! project, and its moves applied as undo steps.

use std::sync::Arc;

use rf_bridge::{Bridge, BridgeHost, ModelSource, SelectedObject, Transform, messages};
use rf_editor::{Editor, EntityId, Placement};
use rf_formats::hash::HashIndex;

use crate::game::Game;
use crate::settings::Settings;

/// A map object sent to Blender: its id, and its archetype then, so a move
/// for an id whose entity changed since (indices shift on deletes) is refused
/// instead of moving another one.
#[derive(Clone, PartialEq)]
pub struct Sent {
    id: EntityId,
    archetype: u32,
}

/// The bridge, started with the app's game.
pub struct BlenderLink {
    bridge: Result<Bridge<Sent>, String>,
}

impl BlenderLink {
    pub fn start(ctx: &eframe::egui::Context, settings: &Settings) -> Self {
        let port = settings.get(rf_bridge::PORT_SETTING).and_then(|p| p.trim().parse().ok());
        let ctx = ctx.clone();
        let bridge = Bridge::start(port.unwrap_or(rf_bridge::DEFAULT_PORT), move || ctx.request_repaint())
            .map_err(|e| format!("Blender bridge unavailable: {e}"));
        if let Err(e) = &bridge {
            log::warn!("{e}");
        }
        Self { bridge }
    }

    /// Answers Blender's requests; every frame.
    pub fn poll(&mut self, editor: &mut Editor, game: &Arc<Game>) {
        if let Ok(b) = &mut self.bridge {
            b.poll(&mut Host { editor, game });
        }
    }

    pub fn connected(&self) -> bool {
        self.bridge.as_ref().is_ok_and(|b| b.listeners() > 0)
    }

    /// "Edit in Blender": the listening Blenders ask for the selection.
    pub fn edit_selection(&self) -> Result<&'static str, String> {
        match &self.bridge {
            Ok(b) if b.edit_selection() => Ok(messages::SENT),
            Ok(_) => Err(messages::NOT_CONNECTED.into()),
            Err(e) => Err(e.clone()),
        }
    }
}

struct Host<'a> {
    editor: &'a mut Editor,
    game: &'a Arc<Game>,
}

impl Host<'_> {
    fn archetype(&self, id: EntityId) -> Option<u32> {
        self.editor.world.entity(id).map(|e| e.archetype)
    }
}

impl BridgeHost for Host<'_> {
    type Object = Sent;

    fn project_name(&self) -> Option<String> {
        self.editor.project.as_ref().map(|p| p.name())
    }

    fn hashes(&self) -> Option<Arc<HashIndex>> {
        Some(self.game.hashes.clone())
    }

    fn selection(&mut self) -> Vec<SelectedObject<Sent>> {
        let g = self.game;
        let mut out = Vec::new();
        for &id in self.editor.selection() {
            let (Some(archetype), Some(p)) = (self.archetype(id), self.editor.placement(id)) else { continue };
            let Some(model) = ModelSource::of_archetype(&g.fs, &g.data(), &g.hashes, archetype) else { continue };
            out.push(SelectedObject {
                object: Sent { id, archetype },
                name: g.hashes.name(archetype),
                transform: Transform { position: p.position, rotation: p.orientation, scale: p.scale },
                model,
            });
        }
        out
    }

    /// Writes the file over the project's of that name, or in the project's
    /// folder, and loads it.
    fn import(&mut self, file: &str, data: Vec<u8>) -> Result<(), String> {
        let existing = self
            .editor
            .project_entries()
            .into_iter()
            .filter_map(|e| e.path)
            .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(file)));
        let path = match existing {
            Some(p) => p,
            None => {
                let project = self.editor.project.as_ref().ok_or(messages::NO_PROJECT)?;
                project.dir().ok_or(messages::NO_PROJECT_FOLDER)?.join(file)
            }
        };
        std::fs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()))?;
        let problems = self.editor.add_project_files(&[path]);
        self.editor.save_project_file()?;
        match problems.first() {
            Some(p) => Err(p.clone()),
            None => Ok(()),
        }
    }

    fn transform(&mut self, object: &Sent, transform: Transform) -> Result<(), String> {
        if self.archetype(object.id) != Some(object.archetype) {
            return Err(
                "This object changed in RedForge since it went to Blender: use Edit in Blender on it again.".into()
            );
        }
        let current = self.editor.placement(object.id).ok_or(messages::UNKNOWN_OBJECT)?;
        let now = Transform { position: current.position, rotation: current.orientation, scale: current.scale };
        let Some(t) = transform.changed_from(&now) else { return Ok(()) };
        let placement =
            Placement { position: t.position, orientation: t.rotation, scale: t.scale }.lock_scale_xy(&current);
        self.editor.set_placements("Move in Blender", &[(object.id, placement)]);
        Ok(())
    }
}
