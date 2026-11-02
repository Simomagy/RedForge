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
mod renderer;
pub mod texture;

pub use camera::OrbitCamera;
pub use context::{GpuContext, wanted_features};
pub use model::{AlphaMode, GpuModel, MaterialDesc, NoTextures, TextureSource};
pub use renderer::{DebugView, GpuMaterial, OUTPUT_FORMAT, RenderTargets, Renderer, ViewSettings};
pub use texture::{GpuTexture, TextureKind};

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("GPU device: {0}")]
    Device(String),
    #[error("texture: {0}")]
    Texture(String),
}

pub type Result<T, E = RenderError> = std::result::Result<T, E>;
