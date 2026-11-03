//! The texture viewer: a texture dictionary's textures, mip by mip.

use std::sync::Arc;

use eframe::egui::{self, Vec2};
use rf_formats::formats::ytd::{TextureDictionary, dimension};
use rf_formats::rpf8::FileRef;
use rf_render::assets::read_texture_dictionary;
use rf_render::{GpuTexture, PreviewMode, PreviewTarget};

use super::{Gpu, Job, status_ui};
use crate::game::Game;

struct Ready {
    dict: TextureDictionary,
    uploaded: Vec<Option<Result<Arc<GpuTexture>, String>>>,
    selected: usize,
    mip: u32,
    layer: u32,
    mode: PreviewMode,
    /// 0 fits the view.
    zoom: f32,
    target: Option<PreviewTarget>,
    texture_id: Option<egui::TextureId>,
    dirty: bool,
}

pub struct TextureViewer {
    pub title: String,
    pub path: String,
    job: Job<TextureDictionary>,
    ready: Option<Box<Ready>>,
}

impl TextureViewer {
    pub fn open(ctx: &egui::Context, game: &Arc<Game>, file: FileRef) -> Self {
        let path = game.path(file);
        let title = path.rsplit('/').next().unwrap_or(&path).to_owned();
        let g = game.clone();
        let job = Job::spawn(ctx, move || Ok(read_texture_dictionary(&g.fs.extract_loose(file)?, &g.hashes)?));
        Self { title, path, job, ready: None }
    }

    pub fn free(&mut self, gpu: &Gpu) {
        if let Some(r) = &mut self.ready {
            gpu.free_texture(r.texture_id.take());
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, gpu: &Gpu) {
        if self.job.poll()
            && let Job::Done(_) = &self.job
        {
            let Job::Done(dict) = std::mem::replace(&mut self.job, Job::Failed(String::new())) else { unreachable!() };
            let n = dict.textures.as_ref().map(Vec::len).unwrap_or(0);
            self.ready = Some(Box::new(Ready {
                dict,
                uploaded: vec![None; n],
                selected: 0,
                mip: 0,
                layer: 0,
                mode: PreviewMode::Rgba,
                zoom: 0.0,
                target: None,
                texture_id: None,
                dirty: true,
            }));
        }
        let Some(r) = &mut self.ready else {
            status_ui(ui, &self.job, &self.title);
            return;
        };
        egui::Panel::left(egui::Id::new(("texture-list", &self.path))).default_size(300.0).show(ui, |ui| {
            r.list_ui(ui);
        });
        egui::CentralPanel::default().show(ui, |ui| r.preview_ui(ui, gpu));
    }
}

fn format_name(t: &rf_formats::formats::ytd::Texture) -> String {
    let b = &t.base;
    let kind = match b.dimension {
        dimension::TEXTURE_CUBE => " cube",
        dimension::TEXTURE_3D => " volume",
        _ if b.depth > 1 => " array",
        _ => "",
    };
    format!("{}×{}{} {:?}, {} mips", b.width, b.height, kind, b.format, b.mip_levels)
}

impl Ready {
    fn list_ui(&mut self, ui: &mut egui::Ui) {
        let textures = self.dict.textures.as_deref().unwrap_or(&[]);
        ui.label(format!("{} textures", textures.len()));
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (i, t) in textures.iter().enumerate() {
                let Some(t) = t else { continue };
                let name = t.base.name.as_deref().unwrap_or("(no name)");
                let label = egui::RichText::new(name);
                let response = ui.selectable_label(i == self.selected, label).on_hover_text(format_name(t));
                ui.label(egui::RichText::new(format_name(t)).small().weak());
                if response.clicked() && i != self.selected {
                    self.selected = i;
                    self.mip = 0;
                    self.layer = 0;
                    self.dirty = true;
                }
            }
        });
    }

    fn texture(&mut self, gpu: &Gpu) -> Result<Arc<GpuTexture>, String> {
        let i = self.selected;
        if self.uploaded[i].is_none() {
            let t = self.dict.textures.as_ref().and_then(|v| v[i].as_ref());
            self.uploaded[i] = Some(match t {
                Some(t) => GpuTexture::upload(&gpu.ctx, &t.base).map(Arc::new).map_err(|e| e.to_string()),
                None => Err("empty slot".into()),
            });
        }
        self.uploaded[i].clone().unwrap()
    }

    fn preview_ui(&mut self, ui: &mut egui::Ui, gpu: &Gpu) {
        let tex = match self.texture(gpu) {
            Ok(t) => t,
            Err(e) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
                return;
            }
        };
        ui.horizontal_wrapped(|ui| {
            ui.strong(&tex.name);
            ui.label(format!("{}×{} {:?} to {:?}", tex.width, tex.height, tex.source_format, tex.gpu_format));
        });
        ui.horizontal_wrapped(|ui| {
            let mut changed = false;
            if tex.mips > 1 {
                changed |= ui.add(egui::Slider::new(&mut self.mip, 0..=tex.mips - 1).text("Mip")).changed();
            }
            if tex.layers > 1 {
                let what = if tex.kind == rf_render::TextureKind::Cube { "Face" } else { "Layer" };
                changed |= ui.add(egui::Slider::new(&mut self.layer, 0..=tex.layers - 1).text(what)).changed();
            }
            for (mode, label) in [
                (PreviewMode::Rgba, "RGBA"),
                (PreviewMode::Rgb, "RGB"),
                (PreviewMode::Alpha, "A"),
                (PreviewMode::Red, "R"),
                (PreviewMode::Green, "G"),
                (PreviewMode::Blue, "B"),
                (PreviewMode::Normal, "Normal"),
            ] {
                changed |= ui.selectable_value(&mut self.mode, mode, label).changed();
            }
            ui.separator();
            if ui.selectable_label(self.zoom == 0.0, "Fit").clicked() {
                self.zoom = 0.0;
            }
            if ui.selectable_label(self.zoom == 1.0, "1:1").clicked() {
                self.zoom = 1.0;
            }
            self.dirty |= changed;
        });
        ui.separator();

        let (mw, mh) = ((tex.width >> self.mip).max(1), (tex.height >> self.mip).max(1));
        // The preview is drawn at the mip's size, at most 4096.
        let scale = (4096.0 / mw.max(mh) as f32).min(1.0);
        let (pw, ph) = (((mw as f32 * scale) as u32).max(1), ((mh as f32 * scale) as u32).max(1));
        if self.target.as_ref().is_none_or(|t| t.width != pw || t.height != ph) {
            self.target = Some(gpu.preview.create_target(&gpu.ctx, pw, ph));
            self.dirty = true;
        }
        let target = self.target.as_ref().unwrap();
        if self.dirty {
            gpu.preview.draw(&gpu.ctx, &tex, target, self.mip, self.layer, self.mode);
            gpu.show_texture(&mut self.texture_id, &target.display_view);
            self.dirty = false;
        }
        let Some(id) = self.texture_id else { return };
        let avail = ui.available_size();
        let size = if self.zoom == 0.0 {
            let s = (avail.x / mw as f32).min(avail.y / mh as f32).min(8.0);
            Vec2::new(mw as f32 * s, mh as f32 * s)
        } else {
            Vec2::new(mw as f32, mh as f32) * self.zoom / ui.ctx().pixels_per_point()
        };
        egui::ScrollArea::both().show(ui, |ui| {
            ui.image((id, size));
        });
    }
}
