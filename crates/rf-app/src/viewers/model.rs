//! The model viewer: a drawable, ydd entry or fragment in 3D.

use std::sync::Arc;

use eframe::egui::{self, Sense, Vec2};
use rf_formats::formats::ytd::TextureDictionary;
use rf_formats::hash::HashIndex;
use rf_formats::rpf8::FileRef;
use rf_render::assets::{DictionaryTextures, ModelFile, read_model, read_texture_dictionary, related_dictionaries};
use rf_render::{DebugView, GpuModel, OrbitCamera, RenderTargets, ViewSettings};

use super::{Gpu, Job, status_ui};
use crate::game::Game;

struct Loaded {
    file: ModelFile,
    dictionaries: Vec<TextureDictionary>,
}

struct Ready {
    file: ModelFile,
    textures: DictionaryTextures,
    models: Vec<Option<GpuModel>>,
    selected: usize,
    camera: OrbitCamera,
    view: ViewSettings,
    targets: Option<RenderTargets>,
    texture_id: Option<egui::TextureId>,
    dirty: bool,
}

pub struct ModelViewer {
    pub title: String,
    pub path: String,
    job: Job<Loaded>,
    ready: Option<Box<Ready>>,
}

impl ModelViewer {
    pub fn open(ctx: &egui::Context, game: &Arc<Game>, file: FileRef) -> Self {
        let path = game.path(file);
        let title = path.rsplit('/').next().unwrap_or(&path).to_owned();
        let g = game.clone();
        let p = path.clone();
        let job = Job::spawn(ctx, move || {
            let data = g.fs.extract_loose(file)?;
            let file = read_model(&p, &data, &g.hashes)?;
            // The dictionaries the game would use for each drawable's archetype,
            // then the ones named like the file.
            let file_name = p.rsplit('/').next().unwrap_or(&p);
            let file_hash = rf_game::file_hash(file_name);
            let mut refs: Vec<FileRef> = Vec::new();
            for (name, _) in &file.drawables {
                let piece = rf_formats::hash::joaat(&name.to_lowercase());
                refs.extend(g.data.txds_for_piece(piece, file_hash));
            }
            refs.extend(related_dictionaries(&g.fs, &g.index, &p));
            let mut seen = std::collections::HashSet::new();
            refs.retain(|f| seen.insert(*f));
            let mut dictionaries = Vec::new();
            for f in refs {
                let loaded =
                    g.fs.extract_loose(f)
                        .map_err(anyhow::Error::from)
                        .and_then(|d| Ok(read_texture_dictionary(&d, &g.hashes)?));
                match loaded {
                    Ok(d) => dictionaries.push(d),
                    Err(e) => log::warn!("{}: {e:#}", g.path(f)),
                }
            }
            Ok(Loaded { file, dictionaries })
        });
        Self { title, path, job, ready: None }
    }

    pub fn free(&mut self, gpu: &Gpu) {
        if let Some(r) = &mut self.ready {
            gpu.free_texture(r.texture_id.take());
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, gpu: &Gpu, hashes: &HashIndex) {
        if self.job.poll()
            && let Job::Done(_) = &self.job
        {
            let Job::Done(loaded) = std::mem::replace(&mut self.job, Job::Failed(String::new())) else {
                unreachable!()
            };
            let count = loaded.file.drawables.len();
            let textures = DictionaryTextures::new(gpu.ctx.clone(), loaded.dictionaries);
            let mut r = Ready {
                file: loaded.file,
                textures,
                models: (0..count).map(|_| None).collect(),
                selected: 0,
                camera: OrbitCamera::default(),
                view: ViewSettings::default(),
                targets: None,
                texture_id: None,
                dirty: true,
            };
            r.select(0, gpu, hashes);
            self.ready = Some(Box::new(r));
        }
        let Some(r) = &mut self.ready else {
            status_ui(ui, &self.job, &self.title);
            return;
        };
        if r.file.drawables.is_empty() {
            ui.label("This file has no drawables.");
            return;
        }

        egui::Panel::right(egui::Id::new(("model-side", &self.path))).default_size(260.0).show(ui, |ui| {
            r.side_ui(ui, gpu, hashes, &self.path);
        });
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| r.viewport_ui(ui, gpu));
    }
}

impl Ready {
    fn select(&mut self, i: usize, gpu: &Gpu, hashes: &HashIndex) {
        self.selected = i;
        if self.models[i].is_none() {
            let (name, d) = &self.file.drawables[i];
            self.models[i] =
                Some(GpuModel::from_drawable(&gpu.ctx, &gpu.renderer, name, d, hashes, &mut self.textures));
        }
        let m = self.models[i].as_ref().unwrap();
        self.camera.frame(m.min.into(), m.max.into());
        self.view.lod = m.highest_lod();
        self.dirty = true;
    }

    fn side_ui(&mut self, ui: &mut egui::Ui, gpu: &Gpu, hashes: &HashIndex, path: &str) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(egui::RichText::new(path).small().weak());
            ui.separator();
            if self.file.drawables.len() > 1 {
                ui.label(format!("{} drawables", self.file.drawables.len()));
                let mut pick = None;
                egui::ScrollArea::vertical().id_salt("drawables").max_height(180.0).show(ui, |ui| {
                    for (i, (name, _)) in self.file.drawables.iter().enumerate() {
                        if ui.selectable_label(i == self.selected, name).clicked() {
                            pick = Some(i);
                        }
                    }
                });
                if let Some(i) = pick {
                    self.select(i, gpu, hashes);
                }
                ui.separator();
            }
            let Some(m) = &self.models[self.selected] else { return };
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label("LOD");
                for (i, name) in ["High", "Med", "Low", "VLow"].iter().enumerate() {
                    let enabled = !m.lods[i].is_empty();
                    changed |= ui.add_enabled(enabled, egui::Button::selectable(self.view.lod == i, *name)).clicked()
                        && {
                            self.view.lod = i;
                            true
                        };
                }
            });
            egui::ComboBox::from_label("View").selected_text(format!("{:?}", self.view.debug)).show_ui(ui, |ui| {
                for v in [
                    DebugView::Lit,
                    DebugView::Albedo,
                    DebugView::Normals,
                    DebugView::Material,
                    DebugView::VertexColour,
                ] {
                    changed |= ui.selectable_value(&mut self.view.debug, v, format!("{v:?}")).changed();
                }
            });
            changed |= ui.add(egui::Slider::new(&mut self.view.exposure, 0.2..=6.0).text("Exposure")).changed();
            if ui.button("Frame model").clicked() {
                self.camera.frame(m.min.into(), m.max.into());
                changed = true;
            }
            self.dirty |= changed;

            ui.separator();
            let lod = &m.lods[self.view.lod.min(3)];
            let size = [m.max[0] - m.min[0], m.max[1] - m.min[1], m.max[2] - m.min[2]];
            ui.label(format!("Size {:.2} × {:.2} × {:.2} m", size[0], size[1], size[2]));
            ui.label(format!("{} draws in this LOD", lod.len()));
            ui.collapsing("Materials", |ui| {
                for (i, d) in lod.iter().enumerate() {
                    ui.collapsing(format!("{i}: {}", d.material.name), |ui| {
                        ui.label(format!("{:?}", d.alpha));
                        for (slot, t) in ["Diffuse", "Normal", "Material"].iter().zip(&d.material.textures) {
                            let name = t.as_ref().map(|t| t.name.as_str()).unwrap_or("—");
                            ui.label(format!("{slot}: {name}"));
                        }
                    });
                }
            });
            if !m.missing_textures.is_empty() {
                ui.collapsing(format!("{} missing textures", m.missing_textures.len()), |ui| {
                    for t in &m.missing_textures {
                        ui.label(t);
                    }
                });
            }
        });
    }

    fn viewport_ui(&mut self, ui: &mut egui::Ui, gpu: &Gpu) {
        let size = ui.available_size().max(Vec2::splat(16.0));
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let ppp = ui.ctx().pixels_per_point();
        let (w, h) = ((rect.width() * ppp) as u32, (rect.height() * ppp) as u32);

        // Left drag orbits, right or middle drag pans, the wheel zooms.
        let d = response.drag_delta();
        if response.dragged_by(egui::PointerButton::Primary) && d != Vec2::ZERO {
            self.camera.orbit(d.x * 0.008, d.y * 0.008);
            self.dirty = true;
        }
        if (response.dragged_by(egui::PointerButton::Secondary) || response.dragged_by(egui::PointerButton::Middle))
            && d != Vec2::ZERO
        {
            self.camera.pan(d.x / rect.height(), d.y / rect.height());
            self.dirty = true;
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.camera.zoom((-scroll * 0.002).exp());
                self.dirty = true;
            }
        }
        if response.double_clicked()
            && let Some(m) = &self.models[self.selected]
        {
            self.camera.frame(m.min.into(), m.max.into());
            self.dirty = true;
        }

        if self.targets.as_ref().is_none_or(|t| t.width != w || t.height != h) {
            self.targets = Some(gpu.renderer.create_targets(&gpu.ctx, w, h));
            self.dirty = true;
        }
        let targets = self.targets.as_ref().unwrap();
        if self.dirty
            && let Some(m) = &self.models[self.selected]
        {
            gpu.renderer.render_model(&gpu.ctx, targets, &self.camera, &self.view, m);
            gpu.show_texture(&mut self.texture_id, &targets.display_view);
            self.dirty = false;
        }
        if let Some(id) = self.texture_id {
            ui.painter().image(
                id,
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
    }
}
