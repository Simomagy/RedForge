use crate::{RenderError, Result};

/// The GPU device the renderer draws with. The app shares eframe's; tools and
/// tests make their own with [`GpuContext::headless`].
#[derive(Clone)]
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// The device features the renderer wants: block-compressed textures (all of
/// RDR2's textures are BC) and 16-bit normalized textures when available.
pub fn wanted_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    let optional = wgpu::Features::TEXTURE_COMPRESSION_BC | wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
    adapter.features() & optional
}

impl GpuContext {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self { device, queue }
    }

    /// A device without a window, on the most capable adapter.
    pub async fn headless() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
            .await
            .map_err(|e| RenderError::Device(e.to_string()))?;
        log::info!("GPU: {:?}", adapter.get_info().name);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("rf-render"),
                required_features: wanted_features(&adapter),
                required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .map_err(|e| RenderError::Device(e.to_string()))?;
        Ok(Self { device, queue })
    }

    pub fn has_bc(&self) -> bool {
        self.device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
    }

    pub fn has_norm16(&self) -> bool {
        self.device.features().contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM)
    }
}
