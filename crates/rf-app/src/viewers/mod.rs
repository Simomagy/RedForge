//! The document tabs: model, animation, texture and XML viewers.

pub mod anim;
pub mod hex;
pub mod model;
pub mod text;
pub mod texture;

use std::sync::Arc;

use rf_render::{GpuContext, Renderer, TexturePreview};

/// The GPU things the viewers share.
#[derive(Clone)]
pub struct Gpu {
    pub ctx: GpuContext,
    pub renderer: Arc<Renderer>,
    pub preview: Arc<TexturePreview>,
    pub egui: Arc<eframe::egui::mutex::RwLock<eframe::egui_wgpu::Renderer>>,
}

impl Gpu {
    /// Shows a texture of ours in egui: registers it the first time, then
    /// points the same id at the new view.
    pub fn show_texture(
        &self,
        id: &mut Option<eframe::egui::TextureId>,
        view: &eframe::wgpu::TextureView,
    ) -> eframe::egui::TextureId {
        let mut egui = self.egui.write();
        match *id {
            Some(existing) => {
                egui.update_egui_texture_from_wgpu_texture(
                    &self.ctx.device,
                    view,
                    eframe::wgpu::FilterMode::Linear,
                    existing,
                );
                existing
            }
            None => {
                let new = egui.register_native_texture(&self.ctx.device, view, eframe::wgpu::FilterMode::Linear);
                *id = Some(new);
                new
            }
        }
    }

    pub fn free_texture(&self, id: Option<eframe::egui::TextureId>) {
        if let Some(id) = id {
            self.egui.write().free_texture(&id);
        }
    }
}

/// A background job's result, polled every frame.
pub enum Job<T> {
    Running(std::sync::mpsc::Receiver<anyhow::Result<T>>),
    Done(T),
    Failed(String),
}

impl<T: Send + 'static> Job<T> {
    pub fn spawn(ctx: &eframe::egui::Context, f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f());
            ctx.request_repaint();
        });
        Job::Running(rx)
    }

    /// Moves a finished job's result in; returns true when it just finished.
    pub fn poll(&mut self) -> bool {
        if let Job::Running(rx) = self
            && let Ok(result) = rx.try_recv()
        {
            *self = match result {
                Ok(v) => Job::Done(v),
                Err(e) => Job::Failed(format!("{e:#}")),
            };
            return true;
        }
        false
    }
}

/// A centred "working on it" or error message.
pub fn status_ui<T>(ui: &mut eframe::egui::Ui, job: &Job<T>, what: &str) {
    ui.centered_and_justified(|ui| match job {
        Job::Running(_) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Loading {what}…"));
            });
        }
        Job::Failed(e) => {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        Job::Done(_) => {}
    });
}
