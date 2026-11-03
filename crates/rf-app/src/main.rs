//! RedForge: a map editor for RedM.

// No console window on Windows, but in debug builds (for the logs).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod blender;
mod browser;
mod game;
mod hierarchy;
mod loading;
mod map;
mod objects;
mod project_pane;
mod properties;
mod settings;
mod tour;
mod viewers;

use std::sync::Arc;

use eframe::egui_wgpu::{WgpuSetup, WgpuSetupCreateNew};
use eframe::wgpu;

/// The C# RedForge's icon, its largest size.
pub(crate) fn icon() -> eframe::egui::IconData {
    let ico = include_bytes!("../assets/redforge.ico");
    match image::load_from_memory_with_format(ico, image::ImageFormat::Ico) {
        Ok(img) => {
            let img = img.into_rgba8();
            eframe::egui::IconData { width: img.width(), height: img.height(), rgba: img.into_raw() }
        }
        Err(_) => eframe::egui::IconData::default(),
    }
}

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn"))
        .init();

    let mut options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("RedForge")
            // The desktop entry the window goes with: the Flatpak's is named after its id.
            .with_app_id(std::env::var("FLATPAK_ID").unwrap_or_else(|_| "redforge".into()))
            .with_icon(icon())
            .with_inner_size([1600.0, 950.0]),
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
