//! RedForge's renderer: the game's models and textures on the GPU, with wgpu
//! (Vulkan on Linux; Vulkan or DX12 on Windows) and WGSL shaders.
//!
//! - [`GpuContext`]: the device; the app shares eframe's.
//! - [`GpuTexture`]: a game texture, every format and mip.
//! - [`GpuModel`]: a drawable's geometries and materials by LOD.
//! - [`Renderer`]: draws models into [`RenderTargets`], deferred: a G-buffer
//!   lit by the sun (with cascaded shadows, [`Cascades`]) and the sky, then
//!   tonemapped to 8-bit sRGB that egui shows directly.
//! - [`Pose`]: a skeleton posed by a clip, which [`Renderer::render_posed`]
//!   skins models with.

pub mod anim;
pub mod assets;
mod camera;
mod context;
pub mod mesh;
pub mod model;
mod post;
pub mod preview;
mod renderer;
mod shadows;
pub mod sky;
mod targets;
pub mod texture;

pub use anim::Pose;
pub use camera::{CameraView, FlyCamera, Frustum, OrbitCamera, Ray};
pub use context::{GpuContext, wanted_features};
pub use model::{
    AlphaMode, DecalMode, GpuModel, MaterialDesc, ModelOptions, NoTextures, Slot, TerrainDesc, TextureSource,
};
pub use post::{Post, PostSettings, PostTargets, Tonemap};
pub use preview::{PreviewMode, PreviewTarget, TexturePreview};
pub use renderer::{
    DebugView, DrawBatch, GpuMaterial, Instance, MaterialKind, Pick, PickRequest, PosedModel, RenderStats, Renderer,
    Scene, ViewSettings,
};
pub use shadows::{CASCADES, Cascade, Cascades};
pub use sky::{Sky, SkyLights, SkyTargets};
pub use targets::{OUTPUT_FORMAT, RenderTargets};
pub use texture::{GpuTexture, TextureKind};

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("GPU device: {0}")]
    Device(String),
    #[error("texture: {0}")]
    Texture(String),
}

pub type Result<T, E = RenderError> = std::result::Result<T, E>;
