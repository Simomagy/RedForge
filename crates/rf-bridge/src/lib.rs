//! The link with RedForge's Blender add-on (`blender/redforge_bridge`), a port
//! of the C# RedForge's `BlenderBridge`. The wire protocol is the C#'s, so the
//! add-on works with either app unchanged.
//!
//! Blender asks for the objects selected on the map: their models come as
//! Sollumz XML (with their textures as DDS files next to it) and with where
//! the objects are on the map. It sends back the models it edited, which go in
//! the project (and so show on the map), and where objects were moved, turned
//! or scaled to. RedForge tells the listening Blenders when to edit the
//! selection ("Edit in Blender").
//!
//! # Protocol
//!
//! One JSON object per line (UTF-8, `\n`), over TCP on 127.0.0.1 only, port
//! [`DEFAULT_PORT`] (the [`PORT_SETTING`] setting). Each command from the
//! add-on opens a connection, sends one line and reads the one-line reply. A
//! listener thread in Blender keeps one more connection open for events.
//! Every reply has `"ok": true` besides what the table shows, or `"ok": false`
//! and an `"error"` message the add-on shows. [`protocol`] has the messages:
//!
//! | Command | Reply |
//! |---|---|
//! | `{"cmd": "hello"}` | `{"app": "RedForge", "project": name or null}` |
//! | `{"cmd": "listen"}` | nothing more; the connection then gets `{"event": "edit"}` lines |
//! | `{"cmd": "selection"}` | `{"assets": [{"file", "xml"}], "placements": [{"id", "name", "file", "piece", "position", "rotation", "scale"}], "errors": [..]}` |
//! | `{"cmd": "import", "xml": path, "file": name}` | `{"file": name}` |
//! | `{"cmd": "transform", "id", "position", "rotation", "scale"}` | nothing more |
//!
//! Positions are world coordinates, rotations `[w, x, y, z]` as the engine and
//! Sollumz have them (the inverse of what a ymap's `CEntityDef` stores). An
//! object's `id` is `<session>:<n>`, the session being 8 hex digits new each
//! time the app starts, so an id from before a restart can't move another
//! object. Exports go to [`default_export_folder`]`/<file>/`, which the
//! add-on also knows (to clean them up and to reload their pictures).
//!
//! # Use
//!
//! The server runs on its own threads; what needs the editor or the project
//! comes to the app's thread through [`Bridge::poll`], called once per frame
//! with the app's [`BridgeHost`]. Heavy work (reading game files, writing XML
//! and DDS files, converting XML back) stays on the bridge's threads.
//!
//! ```ignore
//! let ctx = ctx.clone();
//! let mut bridge = Bridge::<EntityId>::start(port, move || ctx.request_repaint())?;
//! // every frame:
//! bridge.poll(&mut host);
//! let connected = bridge.listeners() > 0;
//! // "Edit in Blender":
//! if !bridge.edit_selection() { show(messages::NOT_CONNECTED) }
//! ```

mod game;
mod model;
pub mod protocol;
mod server;

use std::sync::Arc;

pub use glam::{Quat, Vec3};
use rf_formats::hash::HashIndex;

pub use crate::game::{game_texture_dictionaries, read_game_model};
pub use crate::model::{convert_xml, default_export_folder, export_model};
pub use crate::server::Bridge;

/// The setting with the port (`RedForge.Settings.ini`), as the C# registers it.
pub const PORT_SETTING: &str = "Bridge.Port";
/// The setting's description in the C#.
pub const PORT_SETTING_DESCRIPTION: &str = "The port the Blender add-on connects to (on this computer only)";
/// The port the add-on connects to unless set otherwise in both.
pub const DEFAULT_PORT: u16 = 47810;

/// The C#'s messages, for the host to give the same ones.
pub mod messages {
    /// `selection` with nothing selected that has a model, or before the game is loaded.
    pub const SELECT_FIRST: &str = "Select an object on the map in RedForge first.";
    /// `import` before the game is loaded.
    pub const LOADING: &str = "RedForge is still loading the game.";
    /// For [`BridgeHost::import`](crate::BridgeHost::import) with no project open.
    pub const NO_PROJECT: &str = "Open or create a project in RedForge first: the edited models go in its folder.";
    /// For [`BridgeHost::import`](crate::BridgeHost::import) when the project has no folder for a new file.
    pub const NO_PROJECT_FOLDER: &str =
        "The project has no folder yet: add one in RedForge, the edited models go in it.";
    /// `transform` with an id of this session that was never given out.
    pub const UNKNOWN_OBJECT: &str = "RedForge doesn't know this object.";
    /// `transform` with an id of an earlier session.
    pub const RESTARTED: &str =
        "RedForge was restarted since this object went to Blender: use Edit in Blender on it again to move it.";
    /// The app's status after [`Bridge::edit_selection`](crate::Bridge::edit_selection) reached a Blender.
    pub const SENT: &str = "Sent the selection to Blender";
    /// The app's message when no Blender is listening.
    pub const NOT_CONNECTED: &str = "Blender isn't connected.\nInstall the RedForge Bridge extension in Blender 5 (the \
        blender folder next to RedForge, /usr/share/redforge/blender on Linux) with Sollumz RDR, and enable it: it \
        connects by itself while RedForge is open.";
}

/// Where an object is on the map.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub position: Vec3,
    /// As the engine and Sollumz have it: the inverse of what a ymap's
    /// `CEntityDef` stores (`rf_editor::Placement::orientation`).
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Transform {
    /// What an object at `current` becomes when Blender sends this transform
    /// for it: each part that changed by more than the C# (and the add-on)
    /// count as a change, the rest as it is. `None` when nothing changed.
    pub fn changed_from(&self, current: &Transform) -> Option<Transform> {
        let position = self.position.distance(current.position) > 0.0001;
        let rotation = self.rotation.dot(current.rotation).abs() < 0.999_999_9;
        let scale = self.scale.distance(current.scale) > 0.00001;
        (position || rotation || scale).then_some(Transform {
            position: if position { self.position } else { current.position },
            rotation: if rotation { self.rotation } else { current.rotation },
            scale: if scale { self.scale } else { current.scale },
        })
    }
}

/// A model file and the texture dictionaries it uses, as loose files (a
/// resource with its RSC8 header, as on disk).
#[derive(Clone, Debug, Default)]
pub struct ModelData {
    pub data: Vec<u8>,
    /// Best first: where two have a texture of the same name, the earlier one's
    /// is exported (and it replaces one the model embeds), as the C# does.
    pub texture_dictionaries: Vec<Vec<u8>>,
}

/// Reads a model for exporting it; run on the bridge's thread.
pub type ModelLoader = Box<dyn FnOnce() -> Result<ModelData, String> + Send>;

/// A selected object's model.
pub struct ModelSource {
    /// The model's file name with its extension (`p_lantern05x.ydr`). Its
    /// export is named after it, and Blender shares one import between the
    /// objects with the same file.
    pub file: String,
    /// Which model of the file the object uses, by the name Sollumz gives its
    /// object: for a ydd, the drawable's name as its XML `Hash` has it (Blender
    /// hides the dictionary's other drawables). The add-on ignores it for the
    /// other kinds.
    pub piece: String,
    pub load: ModelLoader,
}

/// A selected map object with a model, as the host gives it.
pub struct SelectedObject<K> {
    /// The host's key for it, given back by [`BridgeHost::transform`].
    pub object: K,
    /// Its archetype's name.
    pub name: String,
    pub transform: Transform,
    pub model: ModelSource,
}

/// What the bridge needs from the app (the C# used its `EditorScene` and
/// project). The bridge calls it only from [`Bridge::poll`], on the app's
/// thread; each call should be quick: anything slow belongs in a
/// [`ModelLoader`].
pub trait BridgeHost {
    /// How the host knows a map object. The bridge keeps the ones it sent to
    /// Blender for the session, so it should stay valid while the app runs
    /// (the C# kept the selection item). Two equal keys are the same object.
    type Object: Clone + PartialEq;

    /// The open project's name, for `hello`.
    fn project_name(&self) -> Option<String>;

    /// The game's strings, for converting models to and from XML; `None` while
    /// the game loads.
    fn hashes(&self) -> Option<Arc<HashIndex>>;

    /// The selected map entities that have a model (others are left out, as in
    /// the C#). [`ModelSource::of_archetype`] makes the model of a game
    /// archetype.
    fn selection(&mut self) -> Vec<SelectedObject<Self::Object>>;

    /// A model edited in Blender (or a ytyp of Sollumz's), converted to the
    /// game file `data`, named `file` (`p_lantern05x.ydr`, `my_props.ytyp`).
    /// The C# writes it over the project's file of that name, which then loads
    /// again, or else adds it in the project's first folder, so the map shows
    /// it. Errors are shown in Blender ([`messages::NO_PROJECT`],
    /// [`messages::NO_PROJECT_FOLDER`]).
    fn import(&mut self, file: &str, data: Vec<u8>) -> Result<(), String>;

    /// An object moved, turned or scaled in Blender, to `transform`: done the
    /// way the Properties pane does it, so it can be undone. The C# also
    /// selects the object. [`Transform::changed_from`] keeps the parts that
    /// didn't change as they are.
    fn transform(&mut self, object: &Self::Object, transform: Transform) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_changed_changes() {
        let current = Transform {
            position: Vec3::new(1000.0, -2000.0, 50.0),
            rotation: Quat::from_rotation_z(0.3),
            scale: Vec3::ONE,
        };
        // Blender's float noise isn't a change.
        let noisy = Transform {
            position: current.position + Vec3::splat(0.00003),
            rotation: Quat::from_rotation_z(0.3 + 1e-5),
            scale: Vec3::splat(1.000_001),
        };
        assert_eq!(noisy.changed_from(&current), None);
        // A move keeps the rotation and scale as they were.
        let moved = Transform { position: current.position + Vec3::X, ..noisy };
        let t = moved.changed_from(&current).unwrap();
        assert_eq!((t.position, t.rotation, t.scale), (moved.position, current.rotation, current.scale));
        // The same orientation the other way round (-q) is no turn.
        let flipped = Transform { rotation: -current.rotation, ..current };
        assert_eq!(flipped.changed_from(&current), None);
        let turned = Transform { rotation: Quat::from_rotation_z(1.0), scale: Vec3::new(1.0, 1.0, 2.0), ..current };
        assert_eq!(turned.changed_from(&current), Some(turned));
    }
}
