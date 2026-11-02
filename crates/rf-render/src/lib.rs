//! RedForge's renderer: the game's models and textures on the GPU, with wgpu
//! (Vulkan on Linux; Vulkan or DX12 on Windows) and WGSL shaders.
//!
//! - [`GpuContext`]: the device; the app shares eframe's.
//! - [`GpuTexture`]: a game texture, every format and mip.
//! - [`GpuModel`]: a drawable's geometries and materials by LOD.
//! - [`Renderer`]: draws models into [`RenderTargets`] (HDR, then tonemapped
//!   to 8-bit sRGB that egui shows directly).

pub mod assets;
mod camera;
mod context;
pub mod mesh;
pub mod model;
pub mod preview;
mod renderer;
pub mod texture;

pub use camera::{CameraView, FlyCamera, Frustum, OrbitCamera, Ray};
pub use context::{GpuContext, wanted_features};
pub use model::{AlphaMode, DecalMode, GpuModel, MaterialDesc, NoTextures, Slot, TerrainDesc, TextureSource};
pub use preview::{PreviewMode, PreviewTarget, TexturePreview};
pub use renderer::{
    DebugView, DrawBatch, GpuMaterial, Instance, MaterialKind, OUTPUT_FORMAT, Pick, RenderStats, RenderTargets,
    Renderer, ViewSettings,
};
pub use texture::{GpuTexture, TextureKind};

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("GPU device: {0}")]
    Device(String),
    #[error("texture: {0}")]
    Texture(String),
}

pub type Result<T, E = RenderError> = std::result::Result<T, E>;
