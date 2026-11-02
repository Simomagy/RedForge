//! RedForge: a map editor for RedM.

mod app;
mod browser;
mod game;
mod settings;
mod viewers;

use std::sync::Arc;

use eframe::egui_wgpu::{WgpuSetup, WgpuSetupCreateNew};
use eframe::wgpu;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn"))
        .init();

    let mut options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_title("RedForge").with_inner_size([1600.0, 950.0]),
        ..Default::default()
    };
    // RDR2's textures are block compressed: ask for BC support.
    let setup = WgpuSetupCreateNew {
        power_preference: wgpu::PowerPreference::HighPerformance,
        device_descriptor: Arc::new(|adapter| wgpu::DeviceDescriptor {
            label: Some("RedForge"),
            required_features: rf_render::wanted_features(adapter),
            required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
            ..Default::default()
        }),
        ..WgpuSetupCreateNew::without_display_handle()
    };
    options.wgpu_options.wgpu_setup = WgpuSetup::CreateNew(setup);

    eframe::run_native("RedForge", options, Box::new(|cc| Ok(Box::new(app::RedForgeApp::new(cc)))))
}
