//! The map: the streamed world, edited through the app's [`Editor`], or one
//! ymap's entities, with a free camera.

pub mod models;
pub mod tools;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Sense, Stroke, Vec2};
use glam::{Mat4, Quat, Vec3};
use rf_editor::Editor;
use rf_formats::formats::meta::Meta;
use rf_formats::rpf8::FileRef;
use rf_render::{DrawBatch, FlyCamera, GpuModel, Instance, RenderStats, RenderTargets, ViewSettings};
use rf_world::{EntityId, VisibleEntity};

use crate::game::Game;
use crate::viewers::Gpu;
use models::ModelCache;
use tools::{Tool, Tools};

enum Source {
    /// The streamed world around the camera, in the app's editor.
    World,
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
    /// Whether scripted ymaps (lockdowns, gang camps...) show, as in the C#.
    scripted: bool,
    /// Metres per second.
    speed: f32,
    targets: Option<RenderTargets>,
    texture_id: Option<egui::TextureId>,
    last_frame: Instant,
    frame_ms: f32,
    stats: RenderStats,
    last_evict: Instant,
    /// The entity drawn with each instance id (id - 1), last frame.
    drawn: Arc<Vec<EntityId>>,
    /// The last frame's camera and where the view was on screen.
    last_view: Option<(rf_render::CameraView, egui::Rect)>,
}

/// `T * R * S` from a ymap entity's fields. The game stores the inverse
/// rotation, as `RDR2Entity` reads it.
fn entity_transform(position: [f32; 3], rotation: [f32; 4], scale_xy: f32, scale_z: f32) -> Mat4 {
    let q = Quat::from_xyzw(rotation[0], rotation[1], rotation[2], rotation[3]);
    let q = if q.length_squared() > 0.0 { q.normalize().inverse() } else { Quat::IDENTITY };
    Mat4::from_scale_rotation_translation(Vec3::new(scale_xy, scale_xy, scale_z), q, position.into())
}

/// Every entity of a ymap (no LODs, no streaming): for looking at one file.
/// A box around some points: (min, max).
type Bounds = Option<(Vec3, Vec3)>;

fn ymap_entities(game: &Game, file: FileRef) -> anyhow::Result<(Vec<VisibleEntity>, Bounds)> {
    let meta = Meta::read(&game.fs.extract_loose(file)?)?;
    let root = meta.root.as_ref().ok_or_else(|| anyhow::anyhow!("empty map data"))?;
    let mut out = Vec::new();
    let mut bounds: Bounds = None;
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
    pub fn world(game: &Arc<Game>, gpu: &Gpu, editor: &Editor) -> Self {
        let mut v = Self::new(game, gpu, Source::World, "World".into(), String::new());
        if let Some((_, p)) = editor.world.places().into_iter().next() {
            v.view_place(p.into());
        }
        v
    }

    pub fn is_world(&self) -> bool {
        matches!(self.source, Source::World)
    }

    /// Looks at a place from the south, a little above it.
    pub fn view_place(&mut self, place: Vec3) {
        self.camera.position = place + Vec3::new(0.0, -90.0, 30.0);
        self.camera.yaw = 0.0;
        self.camera.pitch = -0.3;
    }

    /// Puts the camera at a position, looking along `yaw` and `pitch` (radians).
    pub fn set_camera(&mut self, position: Vec3, yaw: f32, pitch: f32) {
        self.camera.position = position;
        self.camera.yaw = yaw;
        self.camera.pitch = pitch;
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
            scripted: true,
            speed: 30.0,
            targets: None,
            texture_id: None,
            last_frame: Instant::now(),
            frame_ms: 0.0,
            stats: RenderStats::default(),
            last_evict: Instant::now(),
            drawn: Arc::new(Vec::new()),
            last_view: None,
        }
    }

    /// Where a world point showed on screen last frame.
    pub fn screen_point(&self, p: Vec3) -> Option<egui::Pos2> {
        let (view, rect) = self.last_view.as_ref()?;
        let ndc = view.project(p)?;
        Some(egui::pos2(
            rect.min.x + (ndc.x + 1.0) * 0.5 * rect.width(),
            rect.min.y + (1.0 - ndc.y) * 0.5 * rect.height(),
        ))
    }

    pub fn free(&mut self, gpu: &Gpu) {
        gpu.free_texture(self.texture_id.take());
    }

    /// Flying: a mouse drag looks around (unless it's the gizmo's), WASD
    /// moves, R/F go up and down, Shift is faster, the wheel sets the speed
    /// (keys as in the C#; Ctrl is for shortcuts).
    fn controls(&mut self, ui: &egui::Ui, response: &egui::Response, dt: f32, gizmo: bool) {
        let look = response.dragged_by(egui::PointerButton::Secondary)
            || (response.dragged_by(egui::PointerButton::Primary) && !gizmo);
        if look {
            let d = response.drag_delta();
            self.camera.look(d.x * 0.004, d.y * 0.004);
        }
        if !(response.hovered() || response.has_focus())
            || ui.ctx().text_edit_focused()
            || ui.input(|i| i.modifiers.command)
        {
            return;
        }
        let (mut fwd, mut side, mut up) = (0.0, 0.0, 0.0);
        let (fast, scroll) = ui.input(|i| (i.modifiers.shift, i.smooth_scroll_delta.y));
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
            if i.key_down(Key::R) {
                up += 1.0;
            }
            if i.key_down(Key::F) {
                up -= 1.0;
            }
        });
        if scroll != 0.0 {
            self.speed = (self.speed * (scroll * 0.003).exp()).clamp(1.0, 5000.0);
        }
        let factor = if fast { 5.0 } else { 1.0 };
        let c = &mut self.camera;
        let mv = c.forward() * fwd + c.right() * side + Vec3::Z * up;
        c.position += mv * self.speed * factor * dt;
    }

    /// Selects what's drawn in the middle of the view (for testing).
    pub fn select_centre(&mut self, gpu: &Gpu, editor: &mut Editor) -> bool {
        let Some(t) = &self.targets else { return false };
        let pick = gpu.renderer.pick(&gpu.ctx, t, t.width / 2, t.height / 2);
        let hit = pick.and_then(|p| p.id.checked_sub(1)).and_then(|i| self.drawn.get(i as usize)).copied();
        if let Some(id) = hit {
            editor.select(vec![id]);
        }
        hit.is_some()
    }

    /// Moves the camera to look at the selection.
    fn focus_selection(&mut self, editor: &Editor) {
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for id in editor.selection() {
            if let Some((m, min, max)) = tools::entity_box(editor, *id) {
                for c in [min, max] {
                    let p = m.transform_point3(c);
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
            }
        }
        if lo.x > hi.x {
            return;
        }
        let centre = (lo + hi) * 0.5;
        let radius = ((hi - lo).length() * 0.5).max(1.0);
        let distance = radius / (self.camera.fov_y * 0.5).sin() * 1.2;
        self.camera.position = centre - self.camera.forward() * distance;
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, editor: Option<&mut Editor>, tools: &mut Tools) {
        egui::Panel::top(egui::Id::new(("map-bar", &self.path))).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if let Some(editor) = editor {
                    for (tool, label, tip) in [
                        (Tool::Select, "Select", "Select things (V); right click or click"),
                        (Tool::Move, "Move", "Move the selection (Q)"),
                        (Tool::Rotate, "Rotate", "Rotate the selection (E)"),
                        (Tool::Scale, "Scale", "Scale the selection"),
                        (Tool::Place, "Place", "Place the archetype chosen in Properties"),
                    ] {
                        if ui.selectable_label(tools.tool == tool, label).on_hover_text(tip).clicked() {
                            tools.tool = tool;
                        }
                    }
                    let axes = if tools.local { "Object axes" } else { "World axes" };
                    if ui.button(axes).on_hover_text("Gizmo axes (.)").clicked() {
                        tools.local = !tools.local;
                    }
                    let snap = |v: f32, unit: &str| if v == 0.0 { "off".to_owned() } else { format!("{v} {unit}") };
                    egui::ComboBox::from_id_salt("snap-move")
                        .selected_text(format!("Snap {}", snap(tools.snap_move, "m")))
                        .show_ui(ui, |ui| {
                            for v in tools::MOVE_SNAPS {
                                ui.selectable_value(&mut tools.snap_move, v, snap(v, "m"));
                            }
                        });
                    egui::ComboBox::from_id_salt("snap-angle")
                        .selected_text(format!("Snap {}", snap(tools.snap_angle, "°")))
                        .show_ui(ui, |ui| {
                            for v in tools::ANGLE_SNAPS {
                                ui.selectable_value(&mut tools.snap_angle, v, snap(v, "°"));
                            }
                        });
                    ui.separator();
                    let undo = editor.undo_labels().next().map(|l| format!("Undo {l}"));
                    if ui
                        .add_enabled(undo.is_some(), egui::Button::new("Undo"))
                        .on_hover_text(undo.as_deref().unwrap_or("Nothing to undo"))
                        .clicked()
                    {
                        editor.undo();
                    }
                    let redo = editor.redo_labels().next().map(|l| format!("Redo {l}"));
                    if ui
                        .add_enabled(redo.is_some(), egui::Button::new("Redo"))
                        .on_hover_text(redo.as_deref().unwrap_or("Nothing to redo"))
                        .clicked()
                    {
                        editor.redo();
                    }
                    ui.separator();
                    let places = editor.world.places();
                    egui::ComboBox::from_id_salt(("places", &self.path)).selected_text("Go to…").show_ui(ui, |ui| {
                        for (name, p) in &places {
                            if ui.button(name).clicked() {
                                self.view_place(Vec3::from(*p));
                            }
                        }
                    });
                    ui.add(egui::Slider::new(&mut self.hour, 0.0..=24.0).text("Hour"));
                    if ui
                        .checkbox(&mut self.scripted, "Scripted maps")
                        .on_hover_text("Ymaps the game's scripts switch on and off: lockdowns, camps, mission states")
                        .changed()
                    {
                        editor.world.set_hide_scripted(!self.scripted);
                    }
                }
                ui.add(egui::Slider::new(&mut self.speed, 1.0..=5000.0).logarithmic(true).text("Speed m/s"));
                ui.add(egui::Slider::new(&mut self.view.exposure, 0.2..=6.0).text("Exposure"));
            });
        });
    }

    /// Draws the map; `editor` is the app's, for the world (`tools` too).
    pub fn ui(&mut self, ui: &mut egui::Ui, gpu: &Gpu, mut editor: Option<&mut Editor>, tools: &mut Tools) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        self.frame_ms = self.frame_ms * 0.9 + dt * 1000.0 * 0.1;
        if self.is_world() && editor.is_none() {
            ui.label("The world isn't loaded.");
            return;
        }
        if !self.is_world() {
            editor = None;
        }

        self.toolbar(ui, editor.as_deref_mut(), tools);

        let size = ui.available_size().max(Vec2::splat(16.0));
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        if response.clicked() || response.drag_started() || response.secondary_clicked() {
            response.request_focus();
        }
        ui.ctx().request_repaint();

        let ppp = ui.ctx().pixels_per_point();
        let (w, h) = ((rect.width() * ppp) as u32, (rect.height() * ppp) as u32);
        if self.targets.as_ref().is_none_or(|t| t.width != w || t.height != h) {
            self.targets = Some(gpu.renderer.create_targets(&gpu.ctx, w, h));
        }
        let camera = self.camera.camera_view(w.max(1) as f32 / h.max(1) as f32);

        // The gizmo first: a drag that starts on it isn't the camera's.
        let mut gizmo = false;
        if let Some(ed) = editor.as_deref_mut() {
            gizmo = tools.gizmo(ui, &response, ed, &camera, self.camera.fov_y, rect);
        }
        self.controls(ui, &response, dt, gizmo);
        let camera = self.camera.camera_view(w.max(1) as f32 / h.max(1) as f32);
        let has_keys = (response.hovered() || response.has_focus()) && !ui.ctx().text_edit_focused();
        if let Some(ed) = editor.as_deref_mut()
            && has_keys
        {
            tools.keys(ui, ed);
            if ui.input(|i| i.key_pressed(Key::Home) || (i.key_pressed(Key::F) && i.modifiers.alt)) {
                self.focus_selection(ed);
            }
        }

        // What to draw.
        let visible: Vec<VisibleEntity> = match (&self.source, editor.as_deref_mut()) {
            (Source::World, Some(ed)) => {
                ed.world.update(self.camera.position.into(), self.hour);
                ed.world.visible().to_vec()
            }
            (Source::Ymap(e), _) => e.clone(),
            _ => Vec::new(),
        };
        let targets = self.targets.as_ref().unwrap();
        let frustum = camera.frustum();
        self.last_view = Some((camera, rect));

        // Group the entities by model and LOD; each group is one batch of
        // instances. Each instance's id is its entity's place in `drawn`, + 1.
        let mut groups: HashMap<(usize, usize), (Arc<GpuModel>, Vec<Instance>)> = HashMap::new();
        let mut drawn = Vec::new();
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
            drawn.push(e.id);
            let id = drawn.len() as u32;
            groups.entry(key).or_insert_with(|| (model.clone(), Vec::new())).1.push(Instance::new(t, e.tint, id));
        }
        self.drawn = Arc::new(drawn);
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

        if let Some(ed) = editor.as_deref_mut() {
            self.edit_ui(ui, &response, rect, gpu, ed, tools, &camera);
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
        if let Some(ed) = editor.as_deref() {
            let s = ed.world.stats();
            text.push_str(&format!(
                "\nmap nodes {} loaded (+{} loading) of {}",
                s.nodes_loaded, s.nodes_loading, s.nodes
            ));
            if let Some(id) = tools.hover {
                let name = ed.world.entity(id).map(|e| ed.hashes().name(e.archetype)).unwrap_or_default();
                let map = ed.world.map_file_name(id.node).unwrap_or_default();
                text.push_str(&format!("\n{name}  ({map})"));
            }
            if let Some(s) = tools.status() {
                text.push_str(&format!("\n{s}"));
            }
        }
        let painter = ui.painter();
        let galley = painter.layout_no_wrap(text, egui::FontId::monospace(12.0), egui::Color32::from_gray(235));
        let at = rect.left_top() + Vec2::new(8.0, 8.0);
        let backing = egui::Rect::from_min_size(at, galley.size()).expand(4.0);
        painter.rect_filled(backing, 3.0, egui::Color32::from_black_alpha(150));
        painter.galley(at, galley, egui::Color32::WHITE);
        let _ = &self.game;
    }

    /// Picking, the boxes of what's hovered and selected, and placing.
    #[allow(clippy::too_many_arguments)]
    fn edit_ui(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        rect: egui::Rect,
        gpu: &Gpu,
        editor: &mut Editor,
        tools: &mut Tools,
        camera: &rf_render::CameraView,
    ) {
        let targets = self.targets.as_ref().unwrap();
        let mouse = response.hover_pos().map(|p| (p, rect));
        tools.hover_pick(gpu, targets, &self.drawn, camera, mouse);

        // A click (no drag): select what's under it, or place there.
        let clicked = (response.clicked() || response.secondary_clicked()) && !tools.is_dragging();
        if clicked && let Some(pos) = response.interact_pointer_pos() {
            let ppp = rect.width() / targets.width as f32;
            let (x, y) = (((pos.x - rect.min.x) / ppp) as u32, ((pos.y - rect.min.y) / ppp) as u32);
            let pick = gpu.renderer.pick(&gpu.ctx, targets, x, y);
            let ndc = tools::to_ndc(rect, pos);
            if tools.tool == Tool::Place && response.clicked() {
                let point = match pick.filter(|p| p.depth > 0.0) {
                    Some(p) => camera.unproject(ndc, p.depth),
                    None => camera.ray(ndc).at(20.0),
                };
                if let Some(placement) = tools.place_at(editor, point, self.camera.yaw) {
                    let archetype = tools.place_archetype.unwrap_or_default();
                    if let Err(e) = editor.place(archetype, placement) {
                        tools.say(e);
                    }
                } else {
                    tools.say("Choose an archetype to place in Properties");
                }
            } else if tools.tool == Tool::Place {
                tools.tool = Tool::Select; // a right click leaves placing, as in the C#
            } else {
                let hit = pick.and_then(|p| p.id.checked_sub(1)).and_then(|i| self.drawn.get(i as usize)).copied();
                let add = ui.input(|i| i.modifiers.command);
                match (hit, add) {
                    (Some(id), true) => editor.toggle(id),
                    (Some(id), false) => editor.select(vec![id]),
                    (None, false) => editor.select(Vec::new()),
                    (None, true) => {}
                }
            }
        }

        let painter = ui.painter().with_clip_rect(rect);
        if let Some(id) = tools.hover.filter(|id| !editor.selection().contains(id))
            && tools.tool != Tool::Place
            && let Some((m, min, max)) = tools::entity_box(editor, id)
        {
            tools::draw_box(&painter, camera, rect, m, min, max, Stroke::new(1.0, tools::HOVER));
        }
        for id in editor.selection() {
            if let Some((m, min, max)) = tools::entity_box(editor, *id) {
                tools::draw_box(&painter, camera, rect, m, min, max, Stroke::new(1.5, tools::SELECTED));
            }
        }
        // Where a click would place the archetype.
        if tools.tool == Tool::Place
            && let Some(point) = tools.hover_point
            && let Some(p) = tools.place_at(editor, point, self.camera.yaw)
            && let Some(a) = tools.place_archetype.and_then(|a| editor.data().archetypes.get(&a))
        {
            let stroke = Stroke::new(1.5, egui::Color32::from_rgb(90, 200, 255));
            tools::draw_box(&painter, camera, rect, p.matrix(), a.bb_min.into(), a.bb_max.into(), stroke);
        }
        tools.paint_gizmo(ui, rect);
    }
}
