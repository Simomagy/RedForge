//! The map: the streamed world, edited through the app's [`Editor`], or one
//! ymap's entities, with a free camera.

mod batches;
mod globals;
pub mod helpers;
pub mod models;
pub mod tools;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Sense, Stroke, Vec2};
use glam::{Mat4, Quat, Vec3};
use rf_editor::Editor;
use rf_formats::formats::meta::Meta;
use rf_formats::rpf8::FileRef;
use rf_render::sky::STAR_COLOUR;
use rf_render::{
    CASCADES, Cascades, DrawBatch, FlyCamera, GpuModel, HelperBatch, HelperMesh, Instance, Light, LightKind, Outline,
    RenderStats, RenderTargets, Scene, Sky, SkyLights, ViewSettings,
};
use rf_world::{EntityId, VisibleEntity};

use crate::game::Game;
use crate::viewers::Gpu;
use models::ModelCache;
use tools::{Tool, Tools};

enum Source {
    /// The streamed world around the camera, in the app's editor.
    World,
    /// One ymap's entities, all of them, and its prop and grass batches.
    Ymap(Vec<VisibleEntity>, Vec<rf_world::VisibleBatch>),
}

/// The C#'s `RDR2Light.LuxScale`: what the game's light units are in the
/// renderer's (the sun keeps its own brightness).
const LUX_SCALE: f32 = 0.01;

/// The C#'s default hover and selection outline colours
/// (`Editor.HoverOutlineColour`, `Editor.SelectionOutlineColour`).
const HOVER_OUTLINE: [f32; 4] = [1.0, 1.0, 1.0, 0.3];
const SELECTION_OUTLINE: [f32; 4] = [0.0, 1.0, 0.0, 0.5];

/// An entity's light in the world, if it's on at the hour and in view: placed
/// on its bone at rest, then by the entity (the C#'s `GetAnimatedBasis`).
fn map_light(
    l: &rf_game::lights::LightEffect,
    t: &Mat4,
    model: &GpuModel,
    hour: u32,
    frustum: &rf_render::Frustum,
) -> Option<Light> {
    if !l.active_at(hour) || l.range <= 0.0 {
        return None;
    }
    let bone = (l.bone_tag != 0).then(|| model.bones.iter().find(|b| b.0 == l.bone_tag).map(|b| b.1)).flatten();
    let m = *t * bone.unwrap_or(Mat4::IDENTITY);
    let position = m.transform_point3(Vec3::from(l.position));
    let half_length = l.extents[1];
    if !frustum.sphere_visible(position, l.range + half_length) {
        return None;
    }
    use rf_game::lights::LightKind as K;
    Some(Light {
        kind: match l.kind {
            K::Point => LightKind::Point,
            K::Spot => LightKind::Spot,
            K::Capsule => LightKind::Capsule,
        },
        position,
        direction: m.transform_vector3(Vec3::from(l.direction)).normalize_or(Vec3::NEG_Z),
        colour: Vec3::from(l.colour) * LUX_SCALE,
        range: l.range,
        falloff_exponent: l.falloff_exponent,
        cone_inner: l.cone_inner,
        cone_outer: l.cone_outer,
        half_length,
    })
}

pub struct MapView {
    pub title: String,
    pub path: String,
    game: Arc<Game>,
    source: Source,
    models: ModelCache,
    /// The instanced props' and grass's buffers.
    batch_buffers: batches::BatchBuffers,
    camera: FlyCamera,
    view: ViewSettings,
    hour: f32,
    /// The weather's lighting through the day (the world map only).
    timecycle: Option<rf_game::timecycle::TimeCycle>,
    /// The user's exposure, before the time of day's, in EV.
    exposure: f32,
    /// How many of the map's lights the last frame drew.
    lights_drawn: usize,
    /// How many instanced props and grass, in how many batches, it drew.
    batch_instances_drawn: (usize, usize),
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
    /// What each instance id stands for, last frame.
    drawn: Arc<batches::Picks>,
    /// The last frame's camera and where the view was on screen.
    last_view: Option<(rf_render::CameraView, egui::Rect)>,
    /// The selection mode's helpers.
    helpers: helpers::Helpers,
    /// Whether the last frame drew the place tool's preview.
    preview_shown: bool,
    /// The statistics over the map (the gear menu's).
    show_stats: bool,
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

fn ymap_entities(
    game: &Game,
    file: FileRef,
) -> anyhow::Result<(Vec<VisibleEntity>, Vec<rf_world::VisibleBatch>, Bounds)> {
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
    let name = game.fs.entry(file).name_hash;
    let batches: Vec<rf_world::VisibleBatch> = rf_world::read_batches(root)
        .into_iter()
        .enumerate()
        .map(|(i, b)| rf_world::VisibleBatch { key: (name, i as u32), batch: Arc::new(b) })
        .collect();
    for b in &batches {
        let (lo, hi) = (Vec3::from(b.batch.bounds.min), Vec3::from(b.batch.bounds.max));
        bounds = Some(match bounds {
            Some((a, c)) => (a.min(lo), c.max(hi)),
            None => (lo, hi),
        });
    }
    Ok((out, batches, bounds))
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
        let (entities, batches, bounds) = ymap_entities(game, file)?;
        let title = path.rsplit('/').next().unwrap_or(&path).to_owned();
        let mut v = Self::new(game, gpu, Source::Ymap(entities, batches), title, path);
        if let Some((min, max)) = bounds {
            let centre = (min + max) * 0.5;
            let size = (max - min).length().max(20.0);
            v.camera.position = centre + Vec3::new(0.0, -size * 0.6, size * 0.4);
            v.camera.yaw = 0.0;
            v.camera.pitch = -0.5;
        }
        Ok(v)
    }

    fn new(game: &Arc<Game>, gpu: &Gpu, source: Source, title: String, path: String) -> Self {
        globals::load(game, gpu);
        Self {
            title,
            path,
            game: game.clone(),
            source,
            models: ModelCache::new(game.clone()),
            camera: FlyCamera::default(),
            view: ViewSettings::default(),
            // REDFORGE_HOUR sets the starting hour (for testing).
            hour: std::env::var("REDFORGE_HOUR").ok().and_then(|h| h.parse().ok()).unwrap_or(12.0),
            timecycle: None,
            exposure: 0.0,
            lights_drawn: 0,
            batch_buffers: Default::default(),
            batch_instances_drawn: (0, 0),
            scripted: true,
            speed: 30.0,
            targets: None,
            texture_id: None,
            last_frame: Instant::now(),
            frame_ms: 0.0,
            stats: RenderStats::default(),
            last_evict: Instant::now(),
            drawn: Arc::default(),
            last_view: None,
            helpers: Default::default(),
            preview_shown: false,
            show_stats: false,
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

    /// The view's lighting at the hour, from the weather's timecycle, as the
    /// C#'s `Player` sets it: the sky lit by the sun, the moon opposite and
    /// the night's glow; the sun's light through the atmosphere by day, the
    /// moon's at night, each times the timecycle's light colour; its fill
    /// light, exposure and night vision. Without a timecycle, a day's sky.
    fn light(&mut self, eye: Vec3) {
        self.view.hour = Some(self.hour);
        let Some(tc) = &self.timecycle else {
            let sky = SkyLights::default_day();
            self.view.sky = Some(sky);
            self.view.sun_dir = sky.sun_dir;
            self.view.sun_colour = Sky::attenuated_light(eye, STAR_COLOUR, sky.sun_dir);
            self.view.exposure_ev = self.exposure;
            return;
        };
        let s = tc.state(self.hour);
        let to_sun = Vec3::from(rf_game::timecycle::TimeCycle::sun_direction(s.sun_angle));
        self.view.sky = Some(SkyLights {
            sun_dir: to_sun,
            sun_colour: STAR_COLOUR * s.sky_sun_intensity,
            moon_colour: Vec3::splat(s.moon_intensity * s.sky_moon_intensity),
            glow_colour: Vec3::from(s.sky_glow),
        });
        let (dir, colour) =
            if to_sun.z >= 0.0 { (to_sun, STAR_COLOUR) } else { (-to_sun, Vec3::splat(s.moon_intensity)) };
        self.view.sun_dir = dir;
        self.view.sun_colour = Sky::attenuated_light(eye, colour, dir) * Vec3::from(s.light_colour);
        self.view.fill = Vec3::from(s.fill_ambient);
        self.view.exposure_ev = self.exposure + s.exposure_ev;
        self.view.scotopic = s.scotopic;
    }

    /// Selects what's drawn in the middle of the view (for testing).
    pub fn select_centre(&mut self, gpu: &Gpu, editor: &mut Editor, tools: &mut Tools) -> bool {
        let Some(t) = &self.targets else { return false };
        let pick = gpu.renderer.pick(&gpu.ctx, t, t.width / 2, t.height / 2);
        if tools.mode != helpers::Mode::Entity {
            let hit = pick.and_then(|p| self.drawn.helper(p.id));
            tools.helper_selection = hit.into_iter().collect();
            return hit.is_some();
        }
        let hit = pick.and_then(|p| self.drawn.get(p.id));
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

    /// The map's toolbar, in groups of icons (their names on hover).
    fn toolbar(&mut self, ui: &mut egui::Ui, editor: Option<&mut Editor>, tools: &mut Tools) {
        use crate::theme::{self, icon, icon_text};
        let frame = egui::Frame::new().inner_margin(egui::Margin { left: 0, right: 0, top: 0, bottom: 6 });
        let bar = egui::Panel::top(egui::Id::new(("map-bar", &self.path))).frame(frame).show_separator_line(false);
        let mut world_editor = editor;
        bar.show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if let Some(editor) = world_editor.as_deref_mut() {
                    theme::group(ui, |ui| {
                        egui::ComboBox::from_id_salt(("mode", &self.path))
                            .selected_text(icon_text(icon::CROSSHAIR, tools.mode.name()))
                            .show_ui(ui, |ui| {
                                for m in helpers::Mode::ALL {
                                    if ui.selectable_label(tools.mode == m, m.name()).on_hover_text(m.tip()).clicked() {
                                        tools.set_mode(m);
                                    }
                                }
                            })
                            .response
                            .on_hover_text("Selection mode: what the map shows and selects");
                    });
                    theme::group(ui, |ui| {
                        for (tool, glyph, tip) in [
                            (Tool::Select, icon::CURSOR, "Select (V): right click or click"),
                            (Tool::Move, icon::ARROWS_OUT_CARDINAL, "Move the selection (Q)"),
                            (Tool::Rotate, icon::ARROW_CLOCKWISE, "Rotate the selection (E)"),
                            (Tool::Scale, icon::ARROWS_OUT, "Scale the selection (T)"),
                            (Tool::Place, icon::STAMP, "Place the object chosen in Objects (P)"),
                        ] {
                            if theme::tool(ui, tools.tool == tool, glyph, tip).clicked() {
                                tools.tool = tool;
                            }
                        }
                    });
                    theme::group(ui, |ui| {
                        let (glyph, tip) = if tools.local {
                            (icon::CUBE_FOCUS, "Gizmo along the object's axes (.)")
                        } else {
                            (icon::GLOBE_HEMISPHERE_WEST, "Gizmo along the world's axes (.)")
                        };
                        if theme::tool(ui, false, glyph, tip).clicked() {
                            tools.local = !tools.local;
                        }
                        let snap = |v: f32, unit: &str| if v == 0.0 { "off".to_owned() } else { format!("{v} {unit}") };
                        egui::ComboBox::from_id_salt("snap-move")
                            .width(64.0)
                            .selected_text(icon_text(icon::MAGNET, &snap(tools.snap_move, "m")))
                            .show_ui(ui, |ui| {
                                for v in tools::MOVE_SNAPS {
                                    ui.selectable_value(&mut tools.snap_move, v, snap(v, "m"));
                                }
                            })
                            .response
                            .on_hover_text("Snap moves to");
                        egui::ComboBox::from_id_salt("snap-angle")
                            .width(64.0)
                            .selected_text(icon_text(icon::ANGLE, &snap(tools.snap_angle, "°")))
                            .show_ui(ui, |ui| {
                                for v in tools::ANGLE_SNAPS {
                                    ui.selectable_value(&mut tools.snap_angle, v, snap(v, "°"));
                                }
                            })
                            .response
                            .on_hover_text("Snap turns to");
                    });
                    theme::group(ui, |ui| {
                        let undo = editor.undo_labels().next().map(|l| format!("Undo {l} (Ctrl+Z)"));
                        let undone = ui
                            .add_enabled_ui(undo.is_some(), |ui| {
                                theme::tool(
                                    ui,
                                    false,
                                    icon::ARROW_U_UP_LEFT,
                                    undo.as_deref().unwrap_or("Nothing to undo"),
                                )
                            })
                            .inner;
                        if undone.clicked() {
                            tools.undo(editor);
                        }
                        let redo = editor.redo_labels().next().map(|l| format!("Redo {l} (Ctrl+Y)"));
                        let redone = ui
                            .add_enabled_ui(redo.is_some(), |ui| {
                                theme::tool(
                                    ui,
                                    false,
                                    icon::ARROW_U_UP_RIGHT,
                                    redo.as_deref().unwrap_or("Nothing to redo"),
                                )
                            })
                            .inner;
                        if redone.clicked() {
                            tools.redo(editor);
                        }
                    });
                    theme::group(ui, |ui| {
                        let places = editor.world.places();
                        egui::ComboBox::from_id_salt(("places", &self.path))
                            .selected_text(icon_text(icon::MAP_PIN, "Go to"))
                            .show_ui(ui, |ui| {
                                for (name, p) in &places {
                                    if ui.button(name).clicked() {
                                        self.view_place(Vec3::from(*p));
                                    }
                                }
                            });
                        // The hour and the weather, in a menu of their own.
                        let weather = self.timecycle.as_ref().map_or("sunny", |t| t.weather.as_str()).to_owned();
                        let mut chosen = weather.clone();
                        let (h, m) = (self.hour as u32 % 24, (self.hour.fract() * 60.0) as u32);
                        let glyph = if (6..20).contains(&h) { icon::SUN } else { icon::MOON };
                        ui.menu_button(icon_text(glyph, &format!("{h:02}:{m:02}")), |ui| {
                            ui.set_min_width(220.0);
                            ui.label(icon_text(icon::CLOCK, "Hour"));
                            ui.add(egui::Slider::new(&mut self.hour, 0.0..=24.0).fixed_decimals(1));
                            ui.label(icon_text(icon::CLOUD_SUN, "Weather"));
                            egui::ComboBox::from_id_salt(("weather", &self.path)).selected_text(&weather).show_ui(
                                ui,
                                |ui| {
                                    for w in rf_game::timecycle::WEATHERS {
                                        ui.selectable_value(&mut chosen, w.to_owned(), w);
                                    }
                                },
                            );
                        })
                        .response
                        .on_hover_text(format!("The hour and the weather ({weather})"));
                        if chosen != weather || self.timecycle.is_none() {
                            self.timecycle = Some(rf_game::timecycle::TimeCycle::load(&self.game.fs, &chosen));
                        }
                    });
                }
                // What's set once in a while: the speed, the exposure, the scripted maps.
                theme::group(ui, |ui| {
                    let gear = egui::RichText::new(icon::GEAR_SIX).size(theme::SIZE + 1.0);
                    ui.menu_button(gear, |ui| {
                        ui.set_min_width(240.0);
                        ui.label(theme::icon_text(icon::GAUGE, "Speed (m/s)"));
                        ui.add(egui::Slider::new(&mut self.speed, 1.0..=5000.0).logarithmic(true).fixed_decimals(0));
                        ui.label(theme::icon_text(icon::APERTURE, "Exposure (EV)"));
                        ui.add(egui::Slider::new(&mut self.exposure, -4.0..=4.0).fixed_decimals(1));
                        ui.separator();
                        ui.checkbox(&mut self.show_stats, "Statistics")
                            .on_hover_text("What's drawn and loaded, and the frame time, over the map");
                        if let Some(world) = world_editor {
                            ui.separator();
                            let tip = "Ymaps the game's scripts switch on and off: lockdowns, camps, mission states";
                            if ui.checkbox(&mut self.scripted, "Scripted maps").on_hover_text(tip).changed() {
                                world.world.set_hide_scripted(!self.scripted);
                            }
                        }
                    })
                    .response
                    .on_hover_text("View settings: speed, exposure, statistics, scripted maps");
                });
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
        }
        if let Some(ed) = editor.as_deref_mut()
            && std::mem::take(&mut tools.focus)
        {
            self.focus_selection(ed);
        }

        // What to draw.
        let (visible, visible_batches): (Vec<VisibleEntity>, Vec<rf_world::VisibleBatch>) =
            match (&self.source, editor.as_deref_mut()) {
                (Source::World, Some(ed)) => {
                    ed.world.update(self.camera.position.into(), self.hour);
                    (ed.world.visible().to_vec(), ed.world.visible_batches().to_vec())
                }
                (Source::Ymap(e, b), _) => (e.clone(), b.clone()),
                _ => (Vec::new(), Vec::new()),
            };
        self.batch_buffers.upload(gpu, &visible_batches, Duration::from_millis(6));
        self.light(camera.eye);
        let targets = self.targets.as_ref().unwrap();
        let frustum = camera.frustum();
        self.last_view = Some((camera, rect));

        // Group the entities by model and LOD; each group is one batch of
        // instances. Each instance's id is its entity's place in `drawn`, + 1.
        // What casts into each of the sun's shadow cascades goes in groups of
        // its own (the C#'s shadow views of the same entities).
        type Groups = HashMap<(usize, usize), (Arc<GpuModel>, Vec<Instance>)>;
        let cascades = self.view.shadows.then(|| Cascades::new(&camera, self.view.sun_dir));
        let mut groups: Groups = HashMap::new();
        let mut casters: [Groups; CASCADES] = Default::default();
        let mut drawn = Vec::new();
        let mut lights = Vec::new();
        // What's outlined: the hovered entity, then the selection.
        let selection: HashSet<EntityId> =
            editor.as_deref().map(|e| e.selection().iter().copied().collect()).unwrap_or_default();
        let hovered = tools.hover.filter(|id| !selection.contains(id) && tools.tool != Tool::Place);
        let mut outlined: [Groups; 2] = Default::default();
        let mut visible_models = Vec::new();
        let data = self.game.data();
        let hour = self.hour as u32 % 24;
        for e in &visible {
            let Some(model) = self.models.get(e.archetype) else { continue };
            let t = Mat4::from_cols_array(&e.transform);
            if let Some(effects) = data.archetypes.get(&e.archetype).and_then(|a| a.lights.as_ref()) {
                lights.extend(effects.iter().filter_map(|l| map_light(l, &t, &model, hour, &frustum)));
            }
            let (min, max) = (Vec3::from(model.min), Vec3::from(model.max));
            let centre = t.transform_point3((min + max) * 0.5);
            let scale = t.x_axis.truncate().length().max(t.z_axis.truncate().length());
            let radius = (max - min).length() * 0.5 * scale;
            let dist = centre.distance(camera.eye) - radius;
            let lod = model.lod_for_distance(dist.max(0.0));
            let key = (Arc::as_ptr(&model) as usize, lod);
            if let Some(c) = &cascades {
                for (i, g) in casters.iter_mut().enumerate() {
                    if c.casts(i, centre, radius) {
                        g.entry(key).or_insert_with(|| (model.clone(), Vec::new())).1.push(Instance::new(t, e.tint, 0));
                    }
                }
            }
            if !frustum.sphere_visible(centre, radius) {
                continue;
            }
            drawn.push(e.id);
            let id = drawn.len() as u32;
            if tools.mode.wants_models() {
                visible_models.push(helpers::VisibleModel {
                    id: e.id,
                    archetype: e.archetype,
                    transform: t,
                    model: model.clone(),
                });
            }
            let layer = if hovered == Some(e.id) { Some(0) } else { selection.contains(&e.id).then_some(1) };
            if let Some(l) = layer {
                outlined[l]
                    .entry(key)
                    .or_insert_with(|| (model.clone(), Vec::new()))
                    .1
                    .push(Instance::new(t, e.tint, id));
            }
            groups.entry(key).or_insert_with(|| (model.clone(), Vec::new())).1.push(Instance::new(t, e.tint, id));
        }
        let mut instances = Vec::new();
        let mut ranges = |groups: &Groups| {
            let mut out = Vec::new();
            for ((_, lod), (model, list)) in groups {
                let start = instances.len() as u32;
                instances.extend_from_slice(list);
                out.push((model.clone(), *lod, start..instances.len() as u32));
            }
            out
        };
        let ranges_seen = ranges(&groups);
        let ranges_cast: Vec<_> = casters.iter().map(&mut ranges).collect();
        let ranges_outlined: Vec<_> = outlined.iter().map(&mut ranges).collect();
        fn to_batches(r: &[(Arc<GpuModel>, usize, std::ops::Range<u32>)]) -> Vec<DrawBatch<'_>> {
            r.iter().map(|(m, lod, r)| DrawBatch { model: m, lod: *lod, instances: r.clone(), buffer: None }).collect()
        }
        let mut batches = to_batches(&ranges_seen);
        let mut cast: [Vec<DrawBatch>; CASCADES] = std::array::from_fn(|i| to_batches(&ranges_cast[i]));

        // The instanced props and grass: each batch's cells in view, at their
        // LOD. Grass only casts into the nearer cascades.
        let mut batch_models = Vec::new();
        for vb in &visible_batches {
            let Some(model) = self.models.get(vb.batch.archetype) else { continue };
            let lod_dist = data.archetypes.get(&vb.batch.archetype).map_or(0.0, |a| a.lod_dist);
            batch_models.push((vb, model, lod_dist));
        }
        let mut drawn_batch_instances = 0;
        for (vb, model, lod_dist) in &batch_models {
            let Some(buffer) = self.batch_buffers.buffer(vb) else { continue };
            let dists = batches::lod_dists(&vb.batch, model, *lod_dist);
            let eye = camera.eye;
            let seen = |c: Vec3, r: f32| frustum.sphere_visible(c, r);
            for (lod, r) in batches::runs(&vb.batch, model, dists, eye, &seen) {
                drawn_batch_instances += (r.end - r.start) as usize;
                batches.push(DrawBatch { model, lod, instances: r, buffer: Some(buffer) });
            }
            if let Some(c) = &cascades {
                let reach = if vb.batch.grass { 2 } else { CASCADES };
                for (i, list) in cast.iter_mut().enumerate().take(reach) {
                    let casts = |p: Vec3, r: f32| c.casts(i, p, r);
                    for (lod, r) in batches::runs(&vb.batch, model, dists, eye, &casts) {
                        list.push(DrawBatch { model, lod, instances: r, buffer: Some(buffer) });
                    }
                }
            }
        }
        self.batch_instances_drawn = (drawn_batch_instances, visible_batches.len());

        // The selection mode's helpers in view, each with a pick id of its own;
        // the hovered and selected ones outlined too.
        type HelperGroups = HashMap<usize, (Arc<HelperMesh>, Vec<Instance>)>;
        let mut helper_keys = Vec::new();
        let mut helper_groups: HelperGroups = HashMap::new();
        let mut helper_outlined: [HelperGroups; 2] = Default::default();
        tools.selected_parts.clear();
        if tools.mode != helpers::Mode::Entity
            && let (Source::World, Some(ed)) = (&self.source, editor.as_deref())
        {
            let c = helpers::Collect {
                gpu,
                game: &self.game,
                world: &ed.world,
                eye: camera.eye,
                entities: &visible_models,
            };
            self.helpers.collect(tools.mode, &c);
            for p in &self.helpers.parts {
                if !frustum.sphere_visible(p.centre, p.radius) {
                    continue;
                }
                let id = batches::FIRST_HELPER_ID + helper_keys.len() as u32;
                helper_keys.push(p.key);
                let inst = Instance::helper(p.transform, id, p.colour);
                let key = Arc::as_ptr(&p.mesh) as usize;
                let add =
                    |g: &mut HelperGroups| g.entry(key).or_insert_with(|| (p.mesh.clone(), Vec::new())).1.push(inst);
                add(&mut helper_groups);
                let selected = tools.helper_selection.contains(&p.key);
                if selected {
                    add(&mut helper_outlined[1]);
                } else if tools.hover_helper == Some(p.key) {
                    add(&mut helper_outlined[0]);
                }
            }
            tools.selected_parts = self.helpers.parts_of(&tools.helper_selection).cloned().collect();
        }
        let mut helper_ranges = |groups: &HelperGroups| -> Vec<(Arc<HelperMesh>, std::ops::Range<u32>)> {
            groups
                .values()
                .map(|(mesh, list)| {
                    let start = instances.len() as u32;
                    instances.extend_from_slice(list);
                    (mesh.clone(), start..instances.len() as u32)
                })
                .collect()
        };
        let helper_seen = helper_ranges(&helper_groups);
        let helper_lit: Vec<_> = helper_outlined.iter().map(&mut helper_ranges).collect();
        fn to_helper_batches(r: &[(Arc<HelperMesh>, std::ops::Range<u32>)]) -> Vec<HelperBatch<'_>> {
            r.iter().map(|(mesh, r)| HelperBatch { mesh, instances: r.clone() }).collect()
        }
        let helper_batches = to_helper_batches(&helper_seen);
        self.drawn = Arc::new(self.batch_buffers.picks(drawn).with_helpers(helper_keys));

        // The place tool's preview: the archetype's model where a click would
        // put it, in the world as the C#'s preview entity (lit, casting
        // shadows), but not picked, so the mouse stays on what's behind it.
        let mut unpicked = Vec::new();
        self.preview_shown = false;
        if tools.tool == Tool::Place
            && let (Some(ed), Some(archetype)) = (editor.as_deref(), tools.place_archetype)
            && let Some(pos) = response.hover_pos()
        {
            let point =
                tools.hover_point.unwrap_or_else(|| camera.ray(tools::to_ndc(rect, pos)).at(tools.place_distance(ed)));
            if let Some(p) = tools.place_at(ed, point, self.camera.yaw)
                && let Some(model) = self.models.get(archetype)
            {
                let m = p.matrix();
                let (min, max) = (Vec3::from(model.min), Vec3::from(model.max));
                let centre = m.transform_point3((min + max) * 0.5);
                let radius = (max - min).length() * 0.5;
                let lod = model.lod_for_distance((centre.distance(camera.eye) - radius).max(0.0));
                let at = instances.len() as u32;
                instances.push(Instance::new(m, 0, 0));
                unpicked.push((model, lod, at..at + 1, centre, radius));
                self.preview_shown = true;
            }
        }
        let unpicked: Vec<DrawBatch> = unpicked
            .iter()
            .map(|(model, lod, r, centre, radius)| {
                if let Some(c) = &cascades {
                    for (i, list) in cast.iter_mut().enumerate() {
                        if c.casts(i, *centre, *radius) {
                            list.push(DrawBatch { model, lod: *lod, instances: r.clone(), buffer: None });
                        }
                    }
                }
                DrawBatch { model, lod: *lod, instances: r.clone(), buffer: None }
            })
            .collect();

        let mut outlines: Vec<Outline> = [HOVER_OUTLINE, SELECTION_OUTLINE]
            .into_iter()
            .zip(&ranges_outlined)
            .zip(&helper_lit)
            .map(|((colour, r), h)| Outline { colour, batches: to_batches(r), helpers: to_helper_batches(h) })
            .collect();
        // Instances of batches outlined: each one alone from its batch's buffer.
        let outlined_instances = hovered.into_iter().map(|id| (0, id)).chain(selection.iter().map(|&id| (1, id)));
        for (layer, id) in outlined_instances {
            let Some((grass, b, i)) = id.batch_instance() else { continue };
            let found = batch_models
                .iter()
                .find(|(vb, ..)| vb.key.0 == id.node && vb.batch.grass == grass && vb.batch.list_index == b);
            let Some((vb, model, lod_dist)) = found else { continue };
            let (Some(buffer), Some(at)) = (self.batch_buffers.buffer(vb), vb.batch.index_of(i)) else { continue };
            let at_eye = Vec3::from_slice(&vb.batch.transform(at)[12..15]).distance(camera.eye);
            let dists = batches::lod_dists(&vb.batch, model, *lod_dist);
            let lod = dists.iter().position(|&d| at_eye <= d).unwrap_or(2);
            let lod = if model.lods[lod].is_empty() { model.highest_lod() } else { lod };
            let at = at as u32;
            outlines[layer].batches.push(DrawBatch { model, lod, instances: at..at + 1, buffer: Some(buffer) });
        }
        outlines.retain(|o| !o.batches.is_empty() || !o.helpers.is_empty());
        let scene = Scene {
            instances: &instances,
            batches: &batches,
            shadows: cascades.as_ref().map(|c| (c, &cast)),
            lights: &lights,
            outlines: &outlines,
            helpers: &helper_batches,
            unpicked: &unpicked,
        };
        self.lights_drawn = lights.len();
        self.stats = gpu.renderer.render_scene(&gpu.ctx, targets, &camera, &self.view, &scene);
        gpu.show_texture(&mut self.texture_id, &targets.display_view);
        if let Some(id) = self.texture_id {
            // Rounded as the islands.
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            let corners = egui::CornerRadius::same(crate::theme::RADIUS);
            ui.painter().add(egui::epaint::RectShape::filled(rect, corners, egui::Color32::WHITE).with_texture(id, uv));
        }

        if let Some(ed) = editor.as_deref_mut() {
            self.edit_ui(ui, &response, rect, gpu, ed, tools, &camera);
        }

        // Load more models without stalling the frame.
        self.models.upload(&gpu.ctx, &gpu.renderer, Duration::from_millis(8));
        if self.last_evict.elapsed() > Duration::from_secs(5) {
            self.models.evict(Duration::from_secs(60));
            self.batch_buffers.evict(Duration::from_secs(20));
            self.last_evict = Instant::now();
        }

        // Where the camera is; with Statistics (the gear menu), what's drawn.
        let (ready, loading, textures) = self.models.stats();
        let p = self.camera.position;
        let mut text = format!("{:.0} {:.0} {:.0}", p.x, p.y, p.z);
        if self.show_stats {
            text = format!(
                "{:.0} {:.0} {:.0}   {:.1} ms\n{} entities, {} drawn in {} draw calls, {} lights\n{} instanced props and grass drawn, of {} batches\nmodels {ready} (+{loading} loading), textures {textures}",
                p.x,
                p.y,
                p.z,
                self.frame_ms,
                visible.len(),
                self.stats.instances,
                self.stats.draw_calls,
                self.lights_drawn,
                self.batch_instances_drawn.0,
                self.batch_instances_drawn.1
            );
        }
        if let Some(ed) = editor.as_deref()
            && self.show_stats
        {
            let s = ed.world.stats();
            text.push_str(&format!(
                "\nmap nodes {} loaded (+{} loading) of {}",
                s.nodes_loaded, s.nodes_loading, s.nodes
            ));
            if tools.mode != helpers::Mode::Entity {
                text.push_str(&format!("\n{}: {} helpers", tools.mode.name(), self.helpers.parts.len()));
            }
            if let Some(id) = tools.hover {
                let map = ed.world.map_file_name(id.node).unwrap_or_default();
                text.push_str(&format!("\nhovered in {map}"));
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
                    None => camera.ray(ndc).at(tools.place_distance(editor)),
                };
                if let Some(placement) = tools.place_at(editor, point, self.camera.yaw) {
                    let archetype = tools.place_archetype.unwrap_or_default();
                    if let Err(e) = editor.place(archetype, placement) {
                        tools.fail(e);
                    }
                } else {
                    tools.say("Choose what to place in Objects");
                }
            } else if tools.tool == Tool::Place {
                tools.tool = Tool::Select; // a right click leaves placing, as in the C#
            } else if tools.mode != helpers::Mode::Entity {
                // The mode's helpers are what's selected.
                let hit = pick.and_then(|p| self.drawn.helper(p.id));
                let add = ui.input(|i| i.modifiers.command);
                let selection = &mut tools.helper_selection;
                match (hit, add) {
                    (Some(k), true) => match selection.iter().position(|s| *s == k) {
                        Some(i) => {
                            selection.remove(i);
                        }
                        None => selection.push(k),
                    },
                    (Some(k), false) => *selection = vec![k],
                    (None, false) => selection.clear(),
                    (None, true) => {}
                }
                if hit.is_some() && !editor.selection().is_empty() {
                    editor.select(Vec::new());
                }
            } else {
                let hit = pick.and_then(|p| self.drawn.get(p.id));
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
        // The hovered entity's name under the mouse (the C#'s
        // `Editor.HoverNameLabel` "Below"), with a shadow; a helper's owner and name.
        let hovered = match (tools.hover, tools.hover_helper) {
            (_, Some(k)) => self.helpers.parts.iter().find(|p| p.key == k).map(|p| p.info.label()),
            (Some(id), None) => editor.world.entity(id).map(|e| {
                let name = editor.hashes().name(e.archetype);
                match id.batch_instance() {
                    Some((_, _, i)) => format!("{name} : {i}"),
                    None => name,
                }
            }),
            (None, None) => None,
        };
        if let Some(name) = hovered
            && tools.tool != Tool::Place
            && let Some(mouse) = response.hover_pos()
        {
            let font = egui::FontId::proportional(14.0);
            let at = mouse + Vec2::new(0.0, 30.0);
            let shadow = egui::Color32::from_black_alpha(200);
            painter.text(at + Vec2::splat(1.0), egui::Align2::CENTER_TOP, &name, font.clone(), shadow);
            painter.text(at, egui::Align2::CENTER_TOP, &name, font, egui::Color32::WHITE);
        }
        // Where a click would place the archetype, while its model loads.
        if tools.tool == Tool::Place
            && !self.preview_shown
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
