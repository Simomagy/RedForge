//! The C#'s selection modes ("Entity", "Light", "Physics"...): every mode but
//! "Entity" shows helpers over the map (collisions, navmeshes, occluders,
//! interiors' rooms...), and what's hovered and selected is one of them.
//!
//! A mode's helpers are [`HelperPart`]s: a mesh, where it goes and its
//! colour, and what Properties shows of it. Each mode's source makes them for
//! what's streamed around the camera ([`Helpers::collect`]).

mod maps;
pub mod shapes;

use std::sync::Arc;

use glam::{Mat4, Vec3};
use rf_render::HelperMesh;

use crate::game::Game;
use crate::viewers::Gpu;
pub use shapes::Shapes;

/// `RDR2Map.GetSelectionModes`, in its order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    #[default]
    Entity,
    Light,
    Physics,
    Navmesh,
    Skeleton,
    Occlusion,
    InteriorInstance,
    TimeCycleModifier,
    AmbientMaskVolume,
    Scenario,
}

impl Mode {
    pub const ALL: [Mode; 10] = [
        Mode::Entity,
        Mode::Light,
        Mode::Physics,
        Mode::Navmesh,
        Mode::Skeleton,
        Mode::Occlusion,
        Mode::InteriorInstance,
        Mode::TimeCycleModifier,
        Mode::AmbientMaskVolume,
        Mode::Scenario,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Entity => "Entity",
            Mode::Light => "Light",
            Mode::Physics => "Physics",
            Mode::Navmesh => "Navmesh",
            Mode::Skeleton => "Skeleton",
            Mode::Occlusion => "Occlusion",
            Mode::InteriorInstance => "Interior Instance",
            Mode::TimeCycleModifier => "Time Cycle Modifier",
            Mode::AmbientMaskVolume => "Ambient Mask Volume",
            Mode::Scenario => "Scenario",
        }
    }

    /// What the mode shows, for its menu.
    pub fn tip(self) -> &'static str {
        match self {
            Mode::Entity => "Select the map's entities",
            Mode::Light => "Select the entities' lights",
            Mode::Physics => "The collisions: the world's and the entities'",
            Mode::Navmesh => "The navigation meshes",
            Mode::Skeleton => "The skeletons of the entities that have one",
            Mode::Occlusion => "The ymaps' box occluders and occlude models",
            Mode::InteriorInstance => "The interiors' rooms, portals and time cycle modifiers",
            Mode::TimeCycleModifier => "The ymaps' time cycle modifier boxes",
            Mode::AmbientMaskVolume => "The ambient mask volumes",
            Mode::Scenario => "The scenario points",
        }
    }
}

/// Which helper a part is, the same from frame to frame: for hovering and
/// selecting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HelperKey {
    pub mode: Mode,
    /// What it's from: a ymap's name hash, a file's...
    pub source: u32,
    /// The kind of part in its source (each source numbers its own).
    pub group: u32,
    pub index: u32,
}

/// What Properties shows of a part.
#[derive(Clone, Debug, Default)]
pub struct PartInfo {
    /// The C#'s `PartTypeName`: "Box Occluder", "Room"...
    pub type_name: String,
    pub name: String,
    /// What it's in: a ymap, an interior...
    pub owner: String,
    pub fields: Vec<(String, String)>,
}

impl PartInfo {
    /// The C#'s hover label of a part: its owner, then its name.
    pub fn label(&self) -> String {
        match (self.owner.is_empty(), self.name.is_empty()) {
            (false, false) => format!("{} : {}", self.owner, self.name),
            (false, true) => format!("{} : {}", self.owner, self.type_name),
            (true, false) => self.name.clone(),
            (true, true) => self.type_name.clone(),
        }
    }
}

/// One helper: a mesh drawn with a transform and a colour (which multiplies
/// the mesh's own).
#[derive(Clone)]
pub struct HelperPart {
    pub key: HelperKey,
    pub mesh: Arc<HelperMesh>,
    pub transform: Mat4,
    /// Straight RGBA.
    pub colour: [u8; 4],
    /// A sphere around it in world space, for culling.
    pub centre: Vec3,
    pub radius: f32,
    pub info: Arc<PartInfo>,
}

/// What a mode's source has to make its parts from.
pub struct Collect<'a> {
    pub gpu: &'a Gpu,
    pub game: &'a Arc<Game>,
    pub world: &'a rf_world::World,
}

/// The modes' sources, and the parts of the last [`Helpers::collect`].
#[derive(Default)]
pub struct Helpers {
    pub shapes: Shapes,
    maps: maps::MapParts,
    pub parts: Vec<HelperPart>,
}

impl Helpers {
    /// The parts `mode` shows now (none for "Entity").
    pub fn collect(&mut self, mode: Mode, c: &Collect) -> &[HelperPart] {
        self.parts.clear();
        match mode {
            Mode::Occlusion | Mode::InteriorInstance | Mode::TimeCycleModifier => {
                self.maps.collect(mode, c, &mut self.shapes, &mut self.parts)
            }
            _ => {}
        }
        &self.parts
    }

    /// The parts of the last collect with these keys.
    pub fn parts_of<'a>(&'a self, keys: &'a [HelperKey]) -> impl Iterator<Item = &'a HelperPart> + 'a {
        self.parts.iter().filter(|p| keys.contains(&p.key))
    }
}

/// A colour as the C#'s `ColourF` (0-1), in bytes.
pub fn rgba(r: f32, g: f32, b: f32, a: f32) -> [u8; 4] {
    [r, g, b, a].map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}
