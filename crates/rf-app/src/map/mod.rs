//! The map: the streamed world (or one ymap's entities) with a free camera.

pub mod models;
mod resolve;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Sense, Vec2};
use glam::{Mat4, Quat, Vec3};
use rf_formats::formats::meta::Meta;
use rf_formats::rpf8::FileRef;
use rf_render::{DrawBatch, FlyCamera, GpuModel, RenderStats, RenderTargets, ViewSettings};
use rf_world::{EntityId, VisibleEntity, World};

use crate::game::Game;
use crate::viewers::Gpu;
use models::ModelCache;

enum Source {
    /// The streamed world around the camera.
    World(Box<World>),
    /// One ymap's entities, all of them.
    Ymap(Vec<VisibleEntity>),
}

pub struct MapView {
    pub title: String,
    pub path: String,
    game: Arc<Game>,
    source: Source,
    models: ModelCache,
    camera: FlyCamera,
    view: ViewSettings,
    hour: f32,
    /// Metres per second.
    speed: f32,
    targets: Option<RenderTargets>,
    texture_id: Option<egui::TextureId>,
    last_frame: Instant,
    frame_ms: f32,
    stats: RenderStats,
    last_evict: Instant,
}

/// `T * R * S` from a ymap entity's fields. The game stores the inverse
/// rotation, as `RDR2Entity` reads it.
fn entity_transform(position: [f32; 3], rotation: [f32; 4], scale_xy: f32, scale_z: f32) -> Mat4 {
    let q = Quat::from_xyzw(rotation[0], rotation[1], rotation[2], rotation[3]);
    let q = if q.length_squared() > 0.0 { q.normalize().inverse() } else { Quat::IDENTITY };
    Mat4::from_scale_rotation_translation(Vec3::new(scale_xy, scale_xy, scale_z), q, position.into())
}

/// Every entity of a ymap (no LODs, no streaming): for looking at one file.
fn ymap_entities(game: &Game, file: FileRef) -> anyhow::Result<(Vec<VisibleEntity>, Option<(Vec3, Vec3)>)> {
    let meta = Meta::read(&game.fs.extract_loose(file)?)?;
    let root = meta.root.as_ref().ok_or_else(|| anyhow::anyhow!("empty map data"))?;
    let mut out = Vec::new();
    let mut bounds: Option<(Vec3, Vec3)> = None;
    for (i, e) in root.bags("entities").unwrap_or(&[]).iter().enumerate() {
        let Some(e) = e else { continue };
        let p = e.vec3("position");
        let t = entity_transform(p, e.vec4("rotation"), e.f32("scaleXY"), e.f32("scaleZ"));
        let v = Vec3::from(p);
        bounds = Some(match bounds {
            Some((a, b)) => (a.min(v), b.max(v)),
            None => (v, v),
        });
        out.push(VisibleEntity {
            archetype: e.hash("archetypeName"),
            transform: t.to_cols_array(),
            distance: 0.0,
            lod_level: e.u32("lodLevel") as u8,
            tint: e.u32("tintValue"),
            id: EntityId { node: 0, entity: i as u32, interior_entity: u32::MAX },
        });
    }
    Ok((out, bounds))
}

impl MapView {
    pub fn world(game: &Arc<Game>, gpu: &Gpu) -> Self {
        let world = World::new(game.fs.clone(), game.hashes.clone(), game.data.clone());
        let mut v = Self::new(game, gpu, Source::World(Box::new(world)), "World".into(), String::new());
        if let Source::World(w) = &v.source
            && let Some((_, p)) = w.places().into_iter().next()
        {
            v.camera.position = p.into();
        }
        v
    }

    pub fn ymap(game: &Arc<Game>, gpu: &Gpu, file: FileRef) -> anyhow::Result<Self> {
        let path = game.path(file);
        let (entities, bounds) = ymap_entities(game, file)?;
        let title = path.rsplit('/').next().unwrap_or(&path).to_owned();
        let mut v = Self::new(game, gpu, Source::Ymap(entities), title, path);
        if let Some((min, max)) = bounds {
            let centre = (min + max) * 0.5;
            let size = (max - min).length().max(20.0);
            v.camera.position = centre + Vec3::new(0.0, -size * 0.6, size * 0.4);
            v.camera.yaw = 0.0;
            v.camera.pitch = -0.5;
        }
        Ok(v)
    }

    fn new(game: &Arc<Game>, _gpu: &Gpu, source: Source, title: String, path: String) -> Self {
        Self {
            title,
            path,
            game: game.clone(),
            source,
            models: ModelCache::new(game.clone()),
            camera: FlyCamera::default(),
            view: ViewSettings::default(),
            hour: 12.0,
            speed: 30.0,
            targets: None,
            texture_id: None,
            last_frame: Instant::now(),
            frame_ms: 0.0,
            stats: RenderStats::default(),
            last_evict: Instant::now(),
        }
    }

    pub fn free(&mut self, gpu: &Gpu) {
        gpu.free_texture(self.texture_id.take());
    }

    fn controls(&mut self, ui: &egui::Ui, response: &egui::Response, dt: f32) {
        if response.dragged_by(egui::PointerButton::Secondary) || response.dragged_by(egui::PointerButton::Primary) {
            let d = response.drag_delta();
            self.camera.look(d.x * 0.004, d.y * 0.004);
        }
        if !(response.hovered() || response.has_focus()) {
            return;
        }
        let (mut fwd, mut side, mut up, fast, scroll) =
            (0.0, 0.0, 0.0, ui.input(|i| i.modifiers.shift), ui.input(|i| i.smooth_scroll_delta.y));
        ui.input(|i| {
            if i.key_down(Key::W) {
                fwd += 1.0;
            }
            if i.key_down(Key::S) {
                fwd -= 1.0;
            }
            if i.key_down(Key::D) {
                side += 1.0;
            }
            if i.key_down(Key::A) {
                side -= 1.0;
            }
            if i.key_down(Key::E) {
                up += 1.0;
            }
            if i.key_down(Key::Q) {
                up -= 1.0;
            }
        });
        if scroll != 0.0 {
            self.speed = (self.speed * (scroll * 0.003).exp()).clamp(1.0, 5000.0);
        }
        let speed = self.speed * if fast { 5.0 } else { 1.0 } * dt;
        let c = &mut self.camera;
        let mv = c.forward() * fwd + c.right() * side + Vec3::Z * up;
        c.position += mv * speed;
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, gpu: &Gpu) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        self.frame_ms = self.frame_ms * 0.9 + dt * 1000.0 * 0.1;

        egui::Panel::top(egui::Id::new(("map-bar", &self.path))).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if let Source::World(w) = &self.source {
                    let places = w.places();
                    egui::ComboBox::from_id_salt(("places", &self.path)).selected_text("Go to…").show_ui(ui, |ui| {
                        for (name, p) in &places {
                            if ui.button(name).clicked() {
                                self.camera.position = Vec3::from(*p);
                            }
                        }
                    });
                    ui.add(egui::Slider::new(&mut self.hour, 0.0..=24.0).text("Hour"));
                }
                ui.add(egui::Slider::new(&mut self.speed, 1.0..=5000.0).logarithmic(true).text("Speed m/s"));
                ui.add(egui::Slider::new(&mut self.view.exposure, 0.2..=6.0).text("Exposure"));
            });
        });

        let size = ui.available_size().max(Vec2::splat(16.0));
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        self.controls(ui, &response, dt);
        ui.ctx().request_repaint();

        // What to draw.
        let visible: Vec<VisibleEntity> = match &mut self.source {
            Source::World(w) => {
                w.update(self.camera.position.into(), self.hour);
                w.visible().to_vec()
            }
            Source::Ymap(e) => e.clone(),
        };
        let ppp = ui.ctx().pixels_per_point();
        let (w, h) = ((rect.width() * ppp) as u32, (rect.height() * ppp) as u32);
        if self.targets.as_ref().is_none_or(|t| t.width != w || t.height != h) {
            self.targets = Some(gpu.renderer.create_targets(&gpu.ctx, w, h));
        }
        let targets = self.targets.as_ref().unwrap();
        let camera = self.camera.camera_view(w as f32 / h as f32);
        let frustum = camera.frustum();

        // Group the entities by model and LOD; each group is one batch of instances.
        let mut groups: HashMap<(usize, usize), (Arc<GpuModel>, Vec<Mat4>)> = HashMap::new();
        for e in &visible {
            let Some(model) = self.models.get(e.archetype) else { continue };
            let t = Mat4::from_cols_array(&e.transform);
            let (min, max) = (Vec3::from(model.min), Vec3::from(model.max));
            let centre = t.transform_point3((min + max) * 0.5);
            let scale = t.x_axis.truncate().length().max(t.z_axis.truncate().length());
            let radius = (max - min).length() * 0.5 * scale;
            if !frustum.sphere_visible(centre, radius) {
                continue;
            }
            let dist = centre.distance(camera.eye) - radius;
            let lod = model.lod_for_distance(dist.max(0.0));
            let key = (Arc::as_ptr(&model) as usize, lod);
            groups.entry(key).or_insert_with(|| (model.clone(), Vec::new())).1.push(t);
        }
        let mut instances = Vec::new();
        let mut ranges = Vec::new();
        for ((_, lod), (model, list)) in &groups {
            let start = instances.len() as u32;
            instances.extend_from_slice(list);
            ranges.push((model.clone(), *lod, start..instances.len() as u32));
        }
        let batches: Vec<DrawBatch> =
            ranges.iter().map(|(m, lod, r)| DrawBatch { model: m, lod: *lod, instances: r.clone() }).collect();
        self.stats = gpu.renderer.render(&gpu.ctx, targets, &camera, &self.view, &instances, &batches);
        gpu.show_texture(&mut self.texture_id, &targets.display_view);
        if let Some(id) = self.texture_id {
            ui.painter().image(
                id,
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }

        // Load more models without stalling the frame.
        self.models.upload(&gpu.ctx, &gpu.renderer, Duration::from_millis(8));
        if self.last_evict.elapsed() > Duration::from_secs(5) {
            self.models.evict(Duration::from_secs(60));
            self.last_evict = Instant::now();
        }

        let (ready, loading, textures) = self.models.stats();
        let p = self.camera.position;
        let mut text = format!(
            "{:.0} {:.0} {:.0}   {:.1} ms\n{} entities, {} drawn in {} draw calls\nmodels {ready} (+{loading} loading), textures {textures}",
            p.x,
            p.y,
            p.z,
            self.frame_ms,
            visible.len(),
            self.stats.instances,
            self.stats.draw_calls
        );
        if let Source::World(w) = &self.source {
            let s = w.stats();
            text.push_str(&format!(
                "\nmap nodes {} loaded (+{} loading) of {}",
                s.nodes_loaded, s.nodes_loading, s.nodes
            ));
        }
        ui.painter().text(
            rect.left_top() + Vec2::new(8.0, 8.0),
            egui::Align2::LEFT_TOP,
            text,
            egui::FontId::monospace(12.0),
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 220),
        );
        let _ = &self.game;
    }
}
