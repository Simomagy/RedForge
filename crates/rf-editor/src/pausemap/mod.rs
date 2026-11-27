//! The pause map and minimap: their tiles (`minimap_sdf_*` texture
//! dictionaries, signed distance fields) and what's drawn on them, ported
//! from the Map Editor web app. The objects (roads, railroads, buildings,
//! labels, picture stamps, erasers) are kept in map units and drawn into the
//! game's tiles the way the game draws its own marks; the tiles they touch
//! are exported with the project.

pub mod aether;
pub mod buildings;
pub mod compose;
pub mod doc;
mod editing;
pub mod edt;
pub mod export;
pub mod families;
pub mod fonts;
pub mod gfxfont;
pub mod labels;
pub mod library;
pub mod linework;
pub mod marks;
pub mod path;
pub mod project;
pub mod project_file;
pub mod stamps;
pub mod tile;

pub use editing::DOC_NAME;
pub use families::{FAMILIES, Family, Fit, Layer, WORLD_H, WORLD_W, fit_of};
pub use library::{FamilyData, Library};
pub use marks::{Assets, Mark, mark_for};
pub use project::MapObject;
pub use project_file::PauseMapData;
