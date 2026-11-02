//! RDR2's world: which map data (ymaps) and entities are around a camera, as
//! the C# RedForge's `RDR2Map` streams them.
//!
//! The renderer only sees [`World::update`] and [`World::visible`]: entities
//! with their archetype and final transform. [`model_files`] says which files
//! make an archetype's model and textures.
//!
//! Not ported yet: this is the interface the app is built against.

use std::sync::Arc;

use rf_formats::hash::HashIndex;
use rf_formats::rpf8::{FileRef, Rpf8FileSystem};
use rf_game::GameData;

/// An entity to draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VisibleEntity {
    /// The archetype's name hash.
    pub archetype: u32,
    /// Model to world, column-major (as `glam::Mat4::from_cols_array`), as
    /// the C# engine builds it from the entity's position, rotation and scale
    /// (and its interior's, for an entity in an interior).
    pub transform: [f32; 16],
    /// From the camera, as the C# streaming computes it (0 inside the
    /// entity's bounds); for picking the drawable's own LOD.
    pub distance: f32,
    /// `lodLevel`: 0 orphan HD, 1 HD, 2 LOD, 3+ SLOD.
    pub lod_level: u8,
    pub tint: u32,
    /// Which map node and entity this is, for selecting it later.
    pub id: EntityId,
}

/// A map node (ymap) and an entity's index in it; interior entities also carry
/// their index in the interior.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EntityId {
    pub node: u32,
    pub entity: u32,
    /// For an entity of an interior (MLO): its index there, else `u32::MAX`.
    pub interior_entity: u32,
}

#[derive(Clone, Debug, Default)]
pub struct WorldStats {
    pub nodes: usize,
    pub nodes_loaded: usize,
    pub nodes_loading: usize,
    pub entities_visible: usize,
}

/// The kind of file an archetype's model is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModelKind {
    Ydr,
    /// A drawable dictionary; the model is its entry named `ModelFiles::entry`.
    Ydd,
    Yft,
}

/// What an archetype's model is made of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelFiles {
    pub kind: ModelKind,
    pub file: FileRef,
    /// For a ydd: the entry's name hash (the archetype's `assetName`).
    pub entry: u32,
    /// The texture dictionaries to look in, best first (`GetTxds`).
    pub txds: Vec<FileRef>,
}

/// Where an archetype's model and textures are (`RDR2MapFileCache.GetPiece`).
pub fn model_files(_data: &GameData, _archetype: u32) -> Option<ModelFiles> {
    None
}

pub struct World {
    _fs: Arc<Rpf8FileSystem>,
    _hashes: Arc<HashIndex>,
    _data: Arc<GameData>,
    visible: Vec<VisibleEntity>,
}

impl World {
    pub fn new(fs: Arc<Rpf8FileSystem>, hashes: Arc<HashIndex>, data: Arc<GameData>) -> Self {
        Self { _fs: fs, _hashes: hashes, _data: data, visible: Vec::new() }
    }

    /// Streams around `camera` at hour `hour` (0-24): starts loading the map
    /// data that comes into range on worker threads, drops what went out of
    /// range, and works out the visible entities from what's loaded. Never
    /// blocks on loading; call it every frame (it's cheap when nothing moved).
    pub fn update(&mut self, _camera: [f32; 3], _hour: f32) {}

    pub fn visible(&self) -> &[VisibleEntity] {
        &self.visible
    }

    pub fn stats(&self) -> WorldStats {
        WorldStats::default()
    }

    /// Named places to jump to: towns and landmarks, with a camera position.
    pub fn places(&self) -> Vec<(String, [f32; 3])> {
        Vec::new()
    }
}
