//! Editing in the map: picking what's under the mouse, the selection's boxes,
//! the gizmo, and placing new entities.
//!
//! The controls follow the C# RedForge: right click selects (Ctrl adds or
//! removes; a left click selects too), left drag looks around or drags the
//! gizmo, Shift at the start of a gizmo drag drags copies. Q moves, E
//! rotates, V goes back to plain selecting, `.` switches between world and
//! object axes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Pos2, Rect, Stroke};
use glam::{Mat4, Quat, Vec2, Vec3};
use rf_editor::{Editor, EntityId, Placement};
use rf_render::{CameraView, PickRequest};

use super::helpers::{HelperKey, HelperPart, Mode};
use transform_gizmo_egui::math::Transform;
use transform_gizmo_egui::{Gizmo, GizmoConfig, GizmoInteraction, GizmoMode, GizmoOrientation};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Move,
    Rotate,
    Scale,
    /// Clicks place [`Tools::place_archetype`].
    Place,
}

/// The snapping steps the C# offers.
pub const MOVE_SNAPS: [f32; 8] = [0.0, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0];
pub const ANGLE_SNAPS: [f32; 6] = [0.0, 1.0, 5.0, 15.0, 45.0, 90.0];

pub struct Tools {
    pub tool: Tool,
    /// Gizmo axes along the selection's own instead of the world's.
    pub local: bool,
    /// Metres; 0 for none.
    pub snap_move: f32,
    /// Degrees; 0 for none.
    pub snap_angle: f32,
    pub place_archetype: Option<u32>,
    /// Turn of placed entities, radians.
    pub place_yaw: f32,
    gizmo: Gizmo,
    drag: Option<Drag>,
    /// Whether the left button went down on the gizmo: that drag isn't the camera's.
    gizmo_press: bool,
    /// Whether the gizmo has a selection to show this frame.
    gizmo_shown: bool,
    pub hover: Option<EntityId>,
    /// The selection mode, as the C#'s: every mode but "Entity" hovers and
    /// selects its helpers.
    pub mode: Mode,
    pub hover_helper: Option<HelperKey>,
    pub helper_selection: Vec<HelperKey>,
    /// The selected helpers as last drawn, for Properties.
    pub selected_parts: Vec<HelperPart>,
    /// Asks the map view to look at the selection (Hierarchy's double click).
    pub focus: bool,
    /// Where the mouse points at the scene, from the last pick.
    pub hover_point: Option<Vec3>,
    pending: Option<Pending>,
    status: Option<(String, Instant)>,
}

/// A gizmo drag: the dragged entities where they started and are now.
struct Drag {
    start: Vec<(EntityId, Placement)>,
    now: Vec<(EntityId, Placement)>,
}

/// A hover pick on its way back from the GPU, with what was drawn then.
struct Pending {
    request: PickRequest,
    drawn: Arc<super::batches::Picks>,
    view: CameraView,
    ndc: Vec2,
}

impl Default for Tools {
    fn default() -> Self {
        Self {
            tool: Tool::Select,
            local: false,
            snap_move: 0.0,
            snap_angle: 0.0,
            place_archetype: None,
            place_yaw: 0.0,
            gizmo: Gizmo::default(),
            drag: None,
            gizmo_press: false,
            gizmo_shown: false,
            hover: None,
            // REDFORGE_MODE=<name> starts in that selection mode (for testing).
            mode: std::env::var("REDFORGE_MODE")
                .ok()
                .and_then(|m| Mode::ALL.into_iter().find(|x| x.name() == m))
                .unwrap_or_default(),
            hover_helper: None,
            helper_selection: Vec::new(),
            selected_parts: Vec::new(),
            focus: false,
            hover_point: None,
            pending: None,
            status: None,
        }
    }
}

impl Tools {
    /// Switches the selection mode: its helpers' hover and selection go.
    pub fn set_mode(&mut self, mode: Mode) {
        if mode != self.mode {
            self.mode = mode;
            self.hover_helper = None;
            self.helper_selection.clear();
            self.selected_parts.clear();
        }
    }
}

fn rows(m: Mat4) -> mint::RowMatrix4<f64> {
    let r = |i: usize| {
        let v = m.row(i).as_dvec4();
        mint::Vector4 { x: v.x, y: v.y, z: v.z, w: v.w }
    };
    mint::RowMatrix4 { x: r(0), y: r(1), z: r(2), w: r(3) }
}

fn to_transform(p: &Placement) -> Transform {
    let v = |v: Vec3| mint::Vector3 { x: v.x as f64, y: v.y as f64, z: v.z as f64 };
    let q = p.orientation;
    let rotation = mint::Quaternion { v: mint::Vector3 { x: q.x as f64, y: q.y as f64, z: q.z as f64 }, s: q.w as f64 };
    Transform::from_scale_rotation_translation(v(p.scale), rotation, v(p.position))
}

fn from_transform(t: &Transform) -> Placement {
    let v = |v: mint::Vector3<f64>| Vec3::new(v.x as f32, v.y as f32, v.z as f32);
    let q = t.rotation;
    Placement {
        position: v(t.translation),
        orientation: Quat::from_xyzw(q.v.x as f32, q.v.y as f32, q.v.z as f32, q.s as f32).normalize(),
        scale: v(t.scale),
    }
}

/// Where a point of the view is on screen.
fn to_screen(rect: Rect, ndc: Vec2) -> Pos2 {
    Pos2::new(rect.min.x + (ndc.x + 1.0) * 0.5 * rect.width(), rect.min.y + (1.0 - ndc.y) * 0.5 * rect.height())
}

/// A screen point as normalised device coordinates of the view.
pub fn to_ndc(rect: Rect, p: Pos2) -> Vec2 {
    Vec2::new((p.x - rect.min.x) / rect.width() * 2.0 - 1.0, 1.0 - (p.y - rect.min.y) / rect.height() * 2.0)
}

impl Tools {
    /// Shows a message over the map for a few seconds.
    pub fn say(&mut self, text: impl Into<String>) {
        self.status = Some((text.into(), Instant::now()));
    }

    pub fn status(&self) -> Option<&str> {
        self.status.as_ref().filter(|(_, t)| t.elapsed() < Duration::from_secs(5)).map(|(s, _)| s.as_str())
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The gizmo for the selection: moves the entities while it's dragged
    /// (one undo step per drag); [`Tools::paint_gizmo`] draws it. Returns
    /// whether it has the mouse.
    #[allow(clippy::too_many_arguments)]
    pub fn gizmo(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        editor: &mut Editor,
        view: &CameraView,
        fov_y: f32,
        rect: Rect,
    ) -> bool {
        self.gizmo_shown = false;
        let modes = match self.tool {
            Tool::Move => GizmoMode::all_translate(),
            Tool::Rotate => GizmoMode::all_rotate(),
            Tool::Scale => GizmoMode::all_scale(),
            Tool::Select | Tool::Place => {
                self.finish_drag(editor);
                return false;
            }
        };
        let mut current = match &self.drag {
            Some(d) => d.now.clone(),
            None => editor
                .selection()
                .iter()
                .filter(|id| editor.is_editable(**id))
                .filter_map(|id| Some((*id, editor.placement(*id)?)))
                .collect::<Vec<_>>(),
        };
        if current.is_empty() {
            return false;
        }
        self.gizmo_shown = true;
        // The gizmo works in OpenGL's clip space: give it its own projection.
        let aspect = rect.width() / rect.height().max(1.0);
        let projection = glam::camera::rh::proj::opengl::perspective(fov_y, aspect, 0.5, 20_000.0);
        let snapping = match self.tool {
            Tool::Move => self.snap_move > 0.0,
            Tool::Rotate => self.snap_angle > 0.0,
            _ => false,
        };
        self.gizmo.update_config(GizmoConfig {
            view_matrix: rows(view.view),
            projection_matrix: rows(projection),
            viewport: rect,
            modes,
            orientation: if self.local { GizmoOrientation::Local } else { GizmoOrientation::Global },
            snapping,
            snap_distance: self.snap_move.max(0.001),
            snap_angle: self.snap_angle.max(0.1).to_radians(),
            pixels_per_point: ui.ctx().pixels_per_point(),
            ..*self.gizmo.config()
        });
        let pointer = ui.input(|i| i.pointer.clone());
        let cursor = pointer.hover_pos().unwrap_or_default();
        let pressed = pointer.button_pressed(egui::PointerButton::Primary);
        if pressed {
            self.gizmo_press = response.hovered() && self.gizmo.pick_preview((cursor.x, cursor.y));
        }
        let targets: Vec<Transform> = current.iter().map(|(_, p)| to_transform(p)).collect();
        let interaction = GizmoInteraction {
            cursor_pos: (cursor.x, cursor.y),
            hovered: response.hovered() || self.drag.is_some(),
            drag_started: pressed && self.gizmo_press,
            dragging: pointer.button_down(egui::PointerButton::Primary) && self.gizmo_press,
        };
        match self.gizmo.update(interaction, &targets) {
            Some((_, moved)) => {
                if self.drag.is_none() {
                    // Shift at the start drags copies, as in the C#.
                    if ui.input(|i| i.modifiers.shift) {
                        match editor.duplicate_selected() {
                            Ok(()) => {
                                let copies = editor.selection().to_vec();
                                for (c, id) in current.iter_mut().zip(copies) {
                                    c.0 = id;
                                }
                            }
                            Err(e) => self.say(e),
                        }
                    }
                    self.drag = Some(Drag { start: current.clone(), now: current });
                }
                let d = self.drag.as_mut().unwrap();
                d.now = d
                    .start
                    .iter()
                    .zip(moved)
                    .map(|((id, start), t)| (*id, from_transform(&t).lock_scale_xy(start)))
                    .collect();
                editor.preview(&d.now);
            }
            None => self.finish_drag(editor),
        }
        self.gizmo_press || self.gizmo.is_focused() || self.drag.is_some()
    }

    fn finish_drag(&mut self, editor: &mut Editor) {
        if let Some(d) = self.drag.take()
            && d.now != d.start
        {
            let label = match self.tool {
                Tool::Rotate => "Rotate",
                Tool::Scale => "Scale",
                _ => "Move",
            };
            editor.set_placements(label, &d.now);
        }
    }

    /// Draws the gizmo (over the map image).
    pub fn paint_gizmo(&self, ui: &egui::Ui, rect: Rect) {
        if !self.gizmo_shown {
            return;
        }
        let draw = self.gizmo.draw();
        let mesh = egui::Mesh {
            indices: draw.indices,
            vertices: draw
                .vertices
                .into_iter()
                .zip(draw.colors)
                .map(|(pos, [r, g, b, a])| egui::epaint::Vertex {
                    pos: pos.into(),
                    uv: Pos2::ZERO,
                    color: egui::Rgba::from_rgba_premultiplied(r, g, b, a).into(),
                })
                .collect(),
            ..Default::default()
        };
        ui.painter().with_clip_rect(rect).add(mesh);
    }

    /// Takes the last hover pick if the GPU has it, and asks for one at the
    /// mouse.
    pub fn hover_pick(
        &mut self,
        gpu: &crate::viewers::Gpu,
        targets: &rf_render::RenderTargets,
        drawn: &Arc<super::batches::Picks>,
        view: &CameraView,
        mouse: Option<(Pos2, Rect)>,
    ) {
        if let Some(p) = &self.pending
            && let Some(pick) = p.request.try_get(&gpu.ctx)
        {
            let entities = self.mode == Mode::Entity;
            self.hover = p.drawn.get(pick.id).filter(|_| entities);
            self.hover_helper = p.drawn.helper(pick.id).filter(|_| !entities);
            self.hover_point = (pick.depth > 0.0).then(|| p.view.unproject(p.ndc, pick.depth));
            self.pending = None;
        }
        let Some((pos, rect)) = mouse else {
            self.hover = None;
            self.hover_helper = None;
            self.hover_point = None;
            return;
        };
        if self.pending.is_some() {
            return;
        }
        let ppp = rect.width().max(1.0) / targets.width as f32;
        let (x, y) = (((pos.x - rect.min.x) / ppp) as u32, ((pos.y - rect.min.y) / ppp) as u32);
        if let Some(request) = gpu.renderer.request_pick(&gpu.ctx, targets, x, y) {
            self.pending = Some(Pending { request, drawn: drawn.clone(), view: *view, ndc: to_ndc(rect, pos) });
        }
    }

    /// Where a new entity of the place archetype would go, at a point of the
    /// scene: standing on it, turned with the camera.
    pub fn place_at(&self, editor: &Editor, point: Vec3, camera_yaw: f32) -> Option<Placement> {
        let a = editor.data().archetypes.get(&self.place_archetype?)?;
        Some(Placement {
            position: point + Vec3::Z * -a.bb_min[2],
            orientation: Quat::from_rotation_z(camera_yaw + self.place_yaw),
            scale: Vec3::ONE,
        })
    }

    /// How far from the camera the place archetype goes when the mouse is on
    /// nothing: three times its size, as the C#'s `RayNoHitDist`.
    pub fn place_distance(&self, editor: &Editor) -> f32 {
        let a = self.place_archetype.and_then(|a| editor.data().archetypes.get(&a));
        a.map_or(20.0, |a| (Vec3::from(a.bb_max) - Vec3::from(a.bb_min)).length() * 3.0).max(2.0)
    }

    /// The keys of the tools, when the map has the keyboard.
    pub fn keys(&mut self, ui: &egui::Ui, editor: &mut Editor) {
        let (cmd, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
        let pressed = |k: Key| ui.input(|i| i.key_pressed(k));
        if cmd {
            if pressed(Key::Z) && !shift {
                self.finish_drag(editor);
                if let Some(l) = editor.undo() {
                    self.say(format!("Undid {l}"));
                }
            } else if pressed(Key::Y) || (pressed(Key::Z) && shift) {
                if let Some(l) = editor.redo() {
                    self.say(format!("Redid {l}"));
                }
            } else if pressed(Key::C) {
                editor.copy();
            } else if pressed(Key::V) {
                if let Err(e) = editor.paste() {
                    self.say(e);
                }
            } else if pressed(Key::D)
                && let Err(e) = editor.duplicate_selected()
            {
                self.say(e);
            }
            return;
        }
        if self.tool == Tool::Place {
            let step = if shift { 1f32 } else { 5.0 }.to_radians();
            if ui.input(|i| i.key_down(Key::Q)) {
                self.place_yaw += step * if shift { 1.0 } else { 0.25 };
            }
            if ui.input(|i| i.key_down(Key::E)) {
                self.place_yaw -= step * if shift { 1.0 } else { 0.25 };
            }
        } else if pressed(Key::Q) {
            self.tool = if self.tool == Tool::Move { Tool::Select } else { Tool::Move };
        } else if pressed(Key::E) {
            self.tool = if self.tool == Tool::Rotate { Tool::Select } else { Tool::Rotate };
        }
        if pressed(Key::V) {
            self.tool = Tool::Select;
        }
        if pressed(Key::Period) {
            self.local = !self.local;
        }
        if pressed(Key::Escape) {
            if self.tool == Tool::Place {
                self.tool = Tool::Select;
            } else {
                editor.select(Vec::new());
            }
        }
        if pressed(Key::Delete)
            && let Err(e) = editor.delete_selected()
        {
            self.say(e);
        }
    }
}

/// Draws an entity's box (its archetype's, through its transform).
pub fn draw_box(
    painter: &egui::Painter,
    view: &CameraView,
    rect: Rect,
    transform: Mat4,
    min: Vec3,
    max: Vec3,
    stroke: Stroke,
) {
    let corner = |i: usize| {
        let p = Vec3::new(
            if i & 1 != 0 { max.x } else { min.x },
            if i & 2 != 0 { max.y } else { min.y },
            if i & 4 != 0 { max.z } else { min.z },
        );
        view.project(transform.transform_point3(p)).map(|ndc| to_screen(rect, ndc))
    };
    let corners: Vec<Option<Pos2>> = (0..8).map(corner).collect();
    for (a, b) in [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)] {
        if let (Some(a), Some(b)) = (corners[a], corners[b]) {
            painter.line_segment([a, b], stroke);
        }
    }
}

/// An entity's box to draw: its archetype's, or a small one without.
pub fn entity_box(editor: &Editor, id: EntityId) -> Option<(Mat4, Vec3, Vec3)> {
    let e = editor.world.entity(id)?;
    let (min, max) = match editor.data().archetypes.get(&e.archetype) {
        Some(a) => (Vec3::from(a.bb_min), Vec3::from(a.bb_max)),
        None => (Vec3::splat(-0.5), Vec3::splat(0.5)),
    };
    Some((Mat4::from_cols_array(&e.transform), min, max))
}
