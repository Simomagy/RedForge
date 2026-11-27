//! The pause map editor: the Pause map tab (the game's map, the tools, the
//! status bar), the Map layers pane and the selection's fields in
//! Properties. What's drawn is the editor's pause map document
//! (`rf_editor::pausemap`); a gesture works on a draft, shown as it goes and
//! committed as one undo step when it ends.

pub mod gpu;
pub mod inspector;
pub mod layers;
pub mod pane;
mod sync;

use std::sync::Arc;

use rf_editor::Editor;
use rf_editor::pausemap::doc::index_of;
use rf_editor::pausemap::families::Layer;
use rf_editor::pausemap::path::Point;
use rf_editor::pausemap::project::{BuildingShape, LineStyle, MapObject};

pub use gpu::Camera;

/// The tools of the Pause map's toolbar (keys as in the web app).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// V: layers, moved, scaled and turned.
    Select,
    /// A: a line's anchors and handles.
    Direct,
    /// P: lines with curves.
    Pen,
    /// E: takes the game's own marks away.
    Erase,
    /// T: labels.
    Text,
    /// B: building footprints.
    Building,
    /// H: drags the map.
    Hand,
}

/// The pause map's state the tab, the layers pane and Properties share.
pub struct PauseMap {
    /// Selected layers (objects or groups), the last the one Properties shows.
    pub selection: Vec<String>,
    /// The chosen anchors of the selected line, editing its points.
    pub nodes: Vec<usize>,
    pub tool: Tool,
    pub line_style: LineStyle,
    pub erase_target: Layer,
    /// Map units.
    pub erase_radius: f64,
    pub building_shape: BuildingShape,
    /// The preset and font new labels get.
    pub label_preset: usize,
    pub label_font: String,
    /// Text, lines, water, relief.
    pub show: [bool; 4],
    pub grid: bool,
    pub camera: Camera,
    /// The objects as a gesture or a field has them so far.
    pub draft: Option<Vec<MapObject>>,
    /// The pause map was used last (not the world map): Properties and the
    /// editing keys are its.
    pub active: bool,
    /// The layer under the pointer in the layers pane, outlined on the map.
    pub list_hover: Option<String>,
    /// A layer to bring into view.
    pub focus_on: Option<String>,
    /// The label whose text field should take the keyboard.
    pub focus_text: Option<String>,
    /// The world map's camera: where and which way (radians).
    pub world_camera: Option<([f64; 2], f32)>,
    /// A map point the world map should go to.
    pub jump_world: Option<[f64; 2]>,
    /// A box of the map to show (x0, y0, x1, y1).
    pub frame_on: Option<[f64; 4]>,
    /// Pasted text the pause map put on the clipboard (to know it again).
    copied: Option<String>,
    sync: sync::TileSync,
    pub(crate) view: pane::ViewState,
    /// Each font's name lettered in it, for the font picker.
    pub(crate) font_previews: inspector::Previews,
}

impl Default for PauseMap {
    fn default() -> Self {
        Self {
            selection: Vec::new(),
            nodes: Vec::new(),
            tool: Tool::Select,
            line_style: LineStyle::Road,
            erase_target: Layer::Text,
            erase_radius: 10.0,
            building_shape: BuildingShape::Rect,
            label_preset: 2,
            label_font: String::new(),
            show: [true; 4],
            grid: false,
            camera: Camera::default(),
            draft: None,
            active: false,
            list_hover: None,
            focus_on: None,
            focus_text: None,
            world_camera: None,
            jump_world: None,
            frame_on: None,
            copied: None,
            sync: sync::TileSync::default(),
            view: pane::ViewState::default(),
            font_previews: Default::default(),
        }
    }
}

impl PauseMap {
    /// The objects shown: the draft's, or the document's.
    pub fn objects<'a>(&'a self, ed: &'a Editor) -> &'a [MapObject] {
        self.draft.as_deref().unwrap_or(&ed.pause_map().objects)
    }

    /// The layer Properties shows: the last selected.
    pub fn primary(&self) -> Option<&str> {
        self.selection.last().map(String::as_str)
    }

    pub fn select(&mut self, ids: Vec<String>) {
        self.selection = ids;
        self.nodes.clear();
    }

    /// A change of the objects, one undo step (the draft goes).
    pub fn edit(&mut self, ed: &mut Editor, label: &str, objects: Vec<MapObject>) {
        self.draft = None;
        ed.edit_pause_objects(label, objects);
        self.keep_selection(ed);
    }

    /// The draft, committed as one step if it changed anything.
    pub fn commit(&mut self, ed: &mut Editor, label: &str) {
        if let Some(objects) = self.draft.take() {
            ed.edit_pause_objects(label, objects);
        }
        self.keep_selection(ed);
    }

    /// Drops what's selected that isn't there any more (after undo).
    pub fn keep_selection(&mut self, ed: &Editor) {
        let (ids, anchors): (std::collections::HashSet<String>, Option<usize>) = {
            let objects = self.objects(ed);
            let index = index_of(objects);
            let anchors = match self.primary().and_then(|id| index.get(id).copied()) {
                Some(MapObject::Line(l)) => Some(l.nodes.len()),
                _ => None,
            };
            (objects.iter().map(|o| o.id().to_owned()).collect(), anchors)
        };
        let before = self.selection.len();
        self.selection.retain(|id| ids.contains(id));
        if self.selection.len() != before {
            self.nodes.clear();
        }
        match anchors {
            Some(n) => self.nodes.retain(|&i| i < n),
            None => self.nodes.clear(),
        }
    }

    /// The pictures of the document (for the clipboard).
    fn pictures(ed: &Editor) -> &std::collections::BTreeMap<String, Arc<Vec<u8>>> {
        &ed.pause_map().pictures
    }

    /// Puts a picture in the document (with no undo step of its own).
    pub fn add_picture(ed: &mut Editor, bytes: Vec<u8>) -> Result<(String, f64), String> {
        ed.add_pause_picture(bytes)
    }
}

/// A map point's name for the status bar.
pub fn point_text(p: Point) -> String {
    format!("{:.0}, {:.0}", p[0], p[1])
}
