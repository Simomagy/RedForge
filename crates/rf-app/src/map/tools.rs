//! Editing in the map: picking what's under the mouse, the selection's boxes,
//! the gizmo, and placing new entities.
//!
//! The controls follow the C# RedForge: right click selects (Ctrl adds or
//! removes; a left click selects too), left drag looks around or drags the
//! gizmo, Shift at the start of a gizmo drag drags copies. Q moves, E
//! rotates, T scales, P places, V goes back to plain selecting, `.` switches
//! between world and object axes. The editing keys (undo, copy, delete...)
//! work wherever the focus is ([`Tools::edit_keys`]).

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Event, Key, Pos2, Rect, Stroke};
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
    /// Asks the world map to drop the selection to the ground (End).
    pub drop: bool,
    /// Asks the app to send the selection to Blender (Ctrl+B).
    pub blender: bool,
    /// Whether a Blender with the add-on is listening, for the toolbar.
    pub blender_connected: bool,
    /// Asks the app to export the selection's models (Edit > Export selection…).
    pub export_selection: bool,
    /// Where the mouse points at the scene, from the last pick.
    pub hover_point: Option<Vec3>,
    /// The entity last clicked in the map, and where on it from its
    /// position: the Move gizmo goes there when the position is out of view
    /// (a big model seen from close by).
    pub grab: Option<(EntityId, Vec3)>,
    pending: Option<Pending>,
    /// Where the mouse was and how the camera looked for the last pick, and
    /// when: a still mouse over a still map isn't picked again (for a while).
    picked: Option<(Pos2, Mat4, Instant)>,
    toast: Option<Toast>,
    /// What copying put on the clipboard: pasting it pastes the copied
    /// objects (Ctrl+V comes as the clipboard's text).
    copied: Option<String>,
    /// The gizmo drag was cancelled (Esc, or its copies couldn't be made):
    /// the gizmo waits for the button to go up.
    blocked: bool,
    /// The last drag's move and turn, for Ctrl+Shift+D to copy again.
    last_move: Option<(Vec3, Quat)>,
    /// Where the gizmo thinks the mouse is: slower than it while Alt is held.
    cursor: Option<(Pos2, Pos2)>,
    /// How far the drag went, shown by the mouse.
    readout: Option<(String, Pos2)>,
}

/// A message for a few seconds, over the bottom of the window.
pub struct Toast {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

impl Toast {
    /// How long it shows: errors longer.
    pub fn lasts(&self) -> Duration {
        Duration::from_secs(if self.error { 7 } else { 4 })
    }
}

/// A gizmo drag: the dragged entities where they started and are now.
struct Drag {
    start: Vec<(EntityId, Placement)>,
    now: Vec<(EntityId, Placement)>,
    /// Copies are dragged (Shift at the start): their duplicate is a step
    /// already, which the move joins.
    copy: bool,
    /// The gizmo's offset from the first entity's position, when it's at
    /// the clicked point rather than at the selection's ([`Tools::grab`]).
    grab: Option<Vec3>,
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
            drop: false,
            hover_point: None,
            grab: None,
            blender: false,
            blender_connected: false,
            export_selection: false,
            pending: None,
            picked: None,
            toast: None,
            copied: None,
            blocked: false,
            last_move: None,
            cursor: None,
            readout: None,
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
    /// Shows a message for a few seconds.
    pub fn say(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast { text: text.into(), error: false, at: Instant::now() });
    }

    /// Shows what went wrong, a little longer, in red.
    pub fn fail(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast { text: text.into(), error: true, at: Instant::now() });
    }

    /// The message showing now.
    pub fn toast(&self) -> Option<&Toast> {
        self.toast.as_ref().filter(|t| t.at.elapsed() < t.lasts())
    }

    pub fn undo(&mut self, editor: &mut Editor) {
        self.finish_drag(editor);
        match editor.undo() {
            Some(l) => self.say(format!("Undid {l}")),
            None => self.say("Nothing to undo"),
        }
    }

    pub fn redo(&mut self, editor: &mut Editor) {
        match editor.redo() {
            Some(l) => self.say(format!("Redid {l}")),
            None => self.say("Nothing to redo"),
        }
    }

    /// Copies the selection, with a line about it on the clipboard (which
    /// [`Tools::paste`] knows again).
    pub fn copy(&mut self, ctx: &egui::Context, editor: &mut Editor) {
        let n = editor.selection().len();
        if n == 0 {
            self.say("Nothing selected to copy");
            return;
        }
        editor.copy();
        let names: Vec<String> = editor
            .selection()
            .iter()
            .take(3)
            .filter_map(|id| editor.world.entity(*id))
            .map(|e| editor.hashes().name(e.archetype))
            .collect();
        let more = if n > names.len() { ", …" } else { "" };
        let text = format!("RedForge: {} ({}{more})", count(n, "object"), names.join(", "));
        ctx.copy_text(text.clone());
        self.copied = Some(text);
        self.say(format!("Copied {}", count(n, "object")));
    }

    pub fn paste(&mut self, editor: &mut Editor) {
        match editor.paste() {
            Ok(()) => self.say(format!("Pasted {}", count(editor.selection().len(), "object"))),
            Err(e) => self.fail(e),
        }
    }

    /// Ctrl+Shift+D: a copy of the selection, moved and turned as the last
    /// drag (the next fence of a row).
    pub fn duplicate_again(&mut self, editor: &mut Editor) {
        let Some((offset, turn)) = self.last_move else {
            self.say("Move or copy something first: Ctrl+Shift+D copies the selection that far again");
            return;
        };
        if let Err(e) = editor.duplicate_selected() {
            self.fail(e);
            return;
        }
        let moved: Vec<(EntityId, Placement)> = editor
            .selection()
            .iter()
            .filter_map(|id| {
                let mut p = editor.placement(*id)?;
                p.position += offset;
                p.orientation = (turn * p.orientation).normalize();
                Some((*id, p))
            })
            .collect();
        editor.set_placements("Move", &moved);
        editor.merge_last_steps("Copy again");
        self.say(format!("Copied {} again", count(moved.len(), "object")));
    }

    pub fn duplicate(&mut self, editor: &mut Editor) {
        match editor.duplicate_selected() {
            Ok(()) => self.say(format!("Duplicated {}", count(editor.selection().len(), "object"))),
            Err(e) => self.fail(e),
        }
    }

    pub fn delete(&mut self, editor: &mut Editor) {
        let n = editor.selection().len();
        if n == 0 {
            return;
        }
        match editor.delete_selected() {
            Ok(()) => self.say(format!("Deleted {}", count(n, "object"))),
            Err(e) => self.fail(e),
        }
    }

    /// The editing keys, wherever the focus is (the app calls this when no
    /// text field has it): undo, redo, cut, copy, paste, duplicate, delete,
    /// Home (or Alt+F) to look at the selection, End to drop it to the
    /// ground.
    pub fn edit_keys(&mut self, ctx: &egui::Context, editor: &mut Editor) {
        let (cmd, shift, alt) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift, i.modifiers.alt));
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
        // egui-winit turns Ctrl+C, X and V into these (V only when the
        // clipboard has text); text selected in a label copies itself.
        let (copy, cut, paste) = ctx.input(|i| {
            let mut found = (false, false, None);
            for e in &i.events {
                match e {
                    Event::Copy => found.0 = true,
                    Event::Cut => found.1 = true,
                    Event::Paste(text) => found.2 = Some(text.clone()),
                    _ => {}
                }
            }
            found
        });
        let label_text =
            ctx.with_plugin(|s: &mut egui::text_selection::LabelSelectionState| s.has_selection()).unwrap_or(false);
        if cmd && pressed(Key::Z) && !shift {
            self.undo(editor);
        } else if cmd && (pressed(Key::Y) || (pressed(Key::Z) && shift)) {
            self.redo(editor);
        } else if (copy || cut) && !label_text {
            self.copy(ctx, editor);
            if cut {
                self.delete(editor);
            }
        } else if let Some(text) = paste {
            if self.copied.as_deref() == Some(text.as_str()) {
                self.paste(editor);
            }
        } else if cmd && pressed(Key::D) {
            if shift {
                self.duplicate_again(editor);
            } else {
                self.duplicate(editor);
            }
        }
        if pressed(Key::Delete) {
            self.delete(editor);
        }
        if pressed(Key::Home) || (alt && pressed(Key::F)) {
            self.focus = true;
        }
        if pressed(Key::End) {
            self.drop = true;
        }
        if cmd && pressed(Key::B) {
            self.blender = true;
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Whether a hover pick waits on the GPU (a frame more brings it).
    pub fn picking(&self) -> bool {
        self.pending.is_some()
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
        // Its mouse ray starts at the near plane, so a gizmo nearer than that
        // can't be picked: keep it a centimetre away.
        let aspect = rect.width() / rect.height().max(1.0);
        let projection = glam::camera::rh::proj::opengl::perspective(fov_y, aspect, 0.01, 20_000.0);
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
        if self.blocked {
            if pointer.button_down(egui::PointerButton::Primary) {
                return true;
            }
            self.blocked = false;
        }
        let real = pointer.hover_pos().unwrap_or_default();
        // Alt during a drag: the gizmo follows the mouse a tenth as fast.
        let cursor = match self.cursor {
            Some((last, at)) if self.drag.is_some() => {
                let slow = if ui.input(|i| i.modifiers.alt) { 0.1 } else { 1.0 };
                at + (real - last) * slow
            }
            _ => real,
        };
        self.cursor = Some((real, cursor));
        let pressed = pointer.button_pressed(egui::PointerButton::Primary);
        if pressed {
            self.gizmo_press = response.hovered() && self.gizmo.pick_preview((cursor.x, cursor.y));
        }
        // Moving, with the selection's position out of view: the gizmo goes
        // where its entity was clicked, and the drag moves them all as much.
        let grab = match &self.drag {
            Some(d) => d.grab,
            None if self.tool == Tool::Move => {
                let pivot = current.iter().map(|(_, p)| p.position).sum::<Vec3>() / current.len() as f32;
                let in_view = |p: Vec3| view.project(p).is_some_and(|n| n.abs().max_element() < 0.95);
                self.grab
                    .filter(|_| !in_view(pivot))
                    .and_then(|(id, offset)| {
                        let p = current.iter().find(|(i, _)| *i == id)?.1;
                        Some(p.position + offset)
                    })
                    .filter(|&at| in_view(at))
                    .map(|at| at - current[0].1.position)
            }
            None => None,
        };
        let targets: Vec<Transform> = match grab {
            Some(offset) => {
                let at = Placement { position: current[0].1.position + offset, ..current[0].1 };
                vec![to_transform(&at)]
            }
            None => current.iter().map(|(_, p)| to_transform(p)).collect(),
        };
        let interaction = GizmoInteraction {
            cursor_pos: (cursor.x, cursor.y),
            hovered: response.hovered() || self.drag.is_some(),
            drag_started: pressed && self.gizmo_press,
            dragging: pointer.button_down(egui::PointerButton::Primary) && self.gizmo_press,
        };
        match self.gizmo.update(interaction, &targets) {
            Some((_, moved)) => {
                if self.drag.is_none() {
                    // Shift at the start drags copies, as in the C#; if they
                    // can't be made, nothing moves.
                    let copy = ui.input(|i| i.modifiers.shift);
                    if copy {
                        match editor.duplicate_selected() {
                            Ok(()) => {
                                let copies = editor.selection().to_vec();
                                for (c, id) in current.iter_mut().zip(copies) {
                                    c.0 = id;
                                }
                            }
                            Err(e) => {
                                self.fail(e);
                                self.blocked = true;
                                return true;
                            }
                        }
                    }
                    self.drag = Some(Drag { start: current.clone(), now: current, copy, grab });
                }
                let d = self.drag.as_mut().unwrap();
                d.now = match d.grab {
                    Some(offset) => {
                        let from = d.start[0].1.position + offset;
                        let by = moved.first().map_or(from, |t| from_transform(t).position) - from;
                        d.start.iter().map(|(id, p)| (*id, Placement { position: p.position + by, ..*p })).collect()
                    }
                    None => d
                        .start
                        .iter()
                        .zip(moved)
                        .map(|((id, start), t)| (*id, from_transform(&t).lock_scale_xy(start)))
                        .collect(),
                };
                // The move snap puts the first entity on the world's grid,
                // along the axes it moves on, and the others with it.
                if self.tool == Tool::Move && self.snap_move > 0.0 && !self.local {
                    let (start, now) = (d.start[0].1.position, d.now[0].1.position);
                    let step = self.snap_move;
                    let snap = |a: f32, b: f32| if (a - b).abs() > 1e-4 { (a / step).round() * step - a } else { 0.0 };
                    let fix = Vec3::new(snap(now.x, start.x), snap(now.y, start.y), snap(now.z, start.z));
                    for (_, p) in &mut d.now {
                        p.position += fix;
                    }
                }
                editor.preview(&d.now);
                self.readout = readout(self.tool, &d.start[0].1, &d.now[0].1).map(|t| (t, real));
            }
            None => {
                self.finish_drag(editor);
                self.readout = None;
            }
        }
        self.gizmo_press || self.gizmo.is_focused() || self.drag.is_some()
    }

    /// The drag let go: one step (a copy dragged and its move, one too).
    fn finish_drag(&mut self, editor: &mut Editor) {
        let Some(d) = self.drag.take() else { return };
        if d.now == d.start {
            // Copies that didn't move would sit on the originals.
            if d.copy {
                editor.undo();
            }
            return;
        }
        let label = match self.tool {
            Tool::Rotate => "Rotate",
            Tool::Scale => "Scale",
            _ => "Move",
        };
        let (start, now) = (d.start[0].1, d.now[0].1);
        self.last_move = Some((now.position - start.position, now.orientation * start.orientation.inverse()));
        editor.set_placements(label, &d.now);
        if d.copy {
            editor.merge_last_steps(&format!("Copy and {}", label.to_lowercase()));
        }
    }

    /// The arrows (and Page Up / Down) move the selection a step of the move
    /// snap, or 10 cm (Shift: ten times as far).
    fn nudge(&mut self, ui: &egui::Ui, editor: &mut Editor, forward: Vec3, shift: bool) {
        if self.tool == Tool::Place || self.drag.is_some() || editor.selection().is_empty() {
            return;
        }
        let axis = |v: Vec2| {
            if v.x.abs() >= v.y.abs() { Vec3::X * v.x.signum() } else { Vec3::Y * v.y.signum() }
        };
        let ahead = axis(Vec2::new(forward.x, forward.y));
        let right = Vec3::new(ahead.y, -ahead.x, 0.0);
        let step = if self.snap_move > 0.0 { self.snap_move } else { 0.1 } * if shift { 10.0 } else { 1.0 };
        let mut by = Vec3::ZERO;
        ui.input(|i| {
            for (key, dir) in [
                (Key::ArrowUp, ahead),
                (Key::ArrowDown, -ahead),
                (Key::ArrowRight, right),
                (Key::ArrowLeft, -right),
                (Key::PageUp, Vec3::Z),
                (Key::PageDown, -Vec3::Z),
            ] {
                if i.key_pressed(key) {
                    by += dir * step;
                }
            }
        });
        if by == Vec3::ZERO {
            return;
        }
        let moved: Vec<(EntityId, Placement)> = editor
            .selection()
            .iter()
            .filter(|id| editor.is_editable(**id))
            .filter_map(|id| {
                let mut p = editor.placement(*id)?;
                p.position += by;
                Some((*id, p))
            })
            .collect();
        if !moved.is_empty() {
            editor.set_placements("Nudge", &moved);
        }
    }

    /// Esc during a drag: everything goes back, copies too.
    fn cancel_drag(&mut self, editor: &mut Editor) -> bool {
        let Some(d) = self.drag.take() else { return false };
        editor.preview(&d.start);
        if d.copy {
            editor.undo();
        }
        self.blocked = true;
        self.say("Cancelled");
        true
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
        let painter = ui.painter().with_clip_rect(rect);
        painter.add(mesh);
        // How far the drag went, by the mouse.
        if let Some((text, at)) = &self.readout {
            let font = egui::FontId::monospace(13.0);
            let at = *at + egui::vec2(18.0, -26.0);
            let galley = painter.layout_no_wrap(text.clone(), font, egui::Color32::WHITE);
            let back = Rect::from_min_size(at, galley.size()).expand(4.0);
            painter.rect_filled(back, 3.0, egui::Color32::from_black_alpha(170));
            painter.galley(at, galley, egui::Color32::WHITE);
        }
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
        let still = self
            .picked
            .is_some_and(|(at, v, t)| at == pos && v == view.view && t.elapsed() < Duration::from_millis(500));
        if still {
            return;
        }
        self.picked = Some((pos, view.view, Instant::now()));
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

    /// The keys of the tools, when the map has the keyboard (the editing
    /// keys are [`Tools::edit_keys`]). `forward` is the camera's: the arrows
    /// move the selection along the world's axis nearest it, and its right.
    pub fn keys(&mut self, ui: &egui::Ui, editor: &mut Editor, forward: Vec3) {
        let (cmd, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
        let pressed = |k: Key| ui.input(|i| i.key_pressed(k));
        if cmd {
            return;
        }
        self.nudge(ui, editor, forward, shift);
        if self.tool == Tool::Place {
            // Q and E turn what's placed: a step of the angle snap a press,
            // or 45° a second while held (Shift: 180°).
            let turn = |k: Key| (pressed(k), ui.input(|i| i.key_down(k)));
            let ((q, q_down), (e, e_down)) = (turn(Key::Q), turn(Key::E));
            if self.snap_angle > 0.0 {
                let step = self.snap_angle.to_radians();
                self.place_yaw += step * (q as i32 - e as i32) as f32;
            } else {
                let dt = ui.input(|i| i.stable_dt).min(0.1);
                let speed = if shift { 180f32 } else { 45.0 }.to_radians() * dt;
                self.place_yaw += speed * (q_down as i32 - e_down as i32) as f32;
            }
        } else if pressed(Key::Q) {
            self.tool = if self.tool == Tool::Move { Tool::Select } else { Tool::Move };
        } else if pressed(Key::E) {
            self.tool = if self.tool == Tool::Rotate { Tool::Select } else { Tool::Rotate };
        } else if pressed(Key::T) {
            self.tool = if self.tool == Tool::Scale { Tool::Select } else { Tool::Scale };
        }
        if pressed(Key::P) {
            match self.place_archetype {
                Some(_) => self.tool = if self.tool == Tool::Place { Tool::Select } else { Tool::Place },
                None => self.say("Choose what to place in Objects first"),
            }
        }
        if pressed(Key::V) {
            self.tool = Tool::Select;
        }
        if pressed(Key::Period) {
            self.local = !self.local;
        }
        if pressed(Key::Escape) && !self.cancel_drag(editor) {
            if self.tool == Tool::Place {
                self.tool = Tool::Select;
            } else {
                editor.select(Vec::new());
                self.helper_selection.clear();
                self.selected_parts.clear();
            }
        }
    }
}

/// How far a drag went: the move, the turn or the scale.
fn readout(tool: Tool, start: &Placement, now: &Placement) -> Option<String> {
    match tool {
        Tool::Move => {
            let d = now.position - start.position;
            Some(format!("{:.2} m  ({:+.2}, {:+.2}, {:+.2})", d.length(), d.x, d.y, d.z))
        }
        Tool::Rotate => {
            let turn = now.orientation * start.orientation.inverse();
            Some(format!("{:.1}°", turn.angle_between(Quat::IDENTITY).to_degrees()))
        }
        Tool::Scale => {
            let s = now.scale / start.scale.max(Vec3::splat(1e-6));
            Some(format!("× {:.2}, {:.2}", s.x, s.z))
        }
        Tool::Select | Tool::Place => None,
    }
}

/// "1 object", "3 objects".
pub fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
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
