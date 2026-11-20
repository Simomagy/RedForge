//! The animation editor's Dope Sheet and Graph Editor, after Blender's.
//!
//! Both show the keyframes of the animation's F-curves over the frames. The
//! Dope Sheet has a row for each bone (and, opened, for each of its
//! channels) with a diamond at every key, and a Summary row over them all.
//! The Graph Editor draws the curves themselves, the selected keys with
//! their handles.
//!
//! As in Blender: a click selects a key (Shift adds), a drag from a key moves
//! the selected keys, a drag from nothing selects in a box, a drag on the
//! ruler at the top goes through the frames. G grabs the selected keys
//! (click to drop them, right click or Esc to put them back), Shift+D
//! duplicates them, X or Delete deletes them, A selects all and Alt+A none, T
//! sets how they go to the next key and V their handles. The wheel zooms the
//! frames, the middle button pans, Home shows everything.

use std::collections::HashSet;

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use rf_editor::anim::{AnimSource, BoneKeys, Channel, Handle, Interpolation, Path};

use crate::theme::{self, icon};

/// How many changes can be undone.
const UNDO_STEPS: usize = 100;

/// The animation as it was before each change.
#[derive(Default)]
pub(super) struct History {
    undo: Vec<AnimSource>,
    redo: Vec<AnimSource>,
    /// Changed since it was saved.
    pub dirty: bool,
}

impl History {
    /// Before a change: what undo goes back to.
    pub fn step(&mut self, source: &AnimSource) {
        self.undo.push(source.clone());
        if self.undo.len() > UNDO_STEPS {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
    }

    /// The last step wasn't a change after all.
    pub fn forget(&mut self) {
        self.undo.pop();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self, source: &mut AnimSource) {
        if let Some(before) = self.undo.pop() {
            self.redo.push(std::mem::replace(source, before));
            self.dirty = true;
        }
    }

    pub fn redo(&mut self, source: &mut AnimSource) {
        if let Some(after) = self.redo.pop() {
            self.undo.push(std::mem::replace(source, after));
            self.dirty = true;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    DopeSheet,
    Graph,
}

/// A key: its bone's tag, its channel, its frame.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct KeyRef {
    pub bone: u16,
    pub path: Path,
    pub index: u8,
    pub frame: f32,
}

const ROW: f32 = 20.0;
const NAMES: f32 = 220.0;
const RULER: f32 = 22.0;
const PICK: f32 = 7.0;

/// Blender's blue for the frame shown.
const PLAYHEAD: Color32 = Color32::from_rgb(73, 128, 217);
const KEY: Color32 = Color32::from_rgb(210, 210, 210);

/// A channel's colour: X red, Y green, Z blue, W pale.
fn colour(c: &Channel) -> Color32 {
    let axis = if c.path == Path::RotationQuaternion { c.index.wrapping_sub(1) } else { c.index };
    match axis {
        0 => Color32::from_rgb(235, 80, 80),
        1 => Color32::from_rgb(110, 200, 70),
        2 => Color32::from_rgb(80, 140, 240),
        _ => Color32::from_rgb(225, 205, 110),
    }
}

/// What a channel's numbers are shown times: angles in degrees.
pub(super) fn unit(path: Path) -> f32 {
    if path == Path::RotationEuler { 180.0 / std::f32::consts::PI } else { 1.0 }
}

#[derive(Clone, Copy, PartialEq)]
enum RowKind {
    Summary,
    Bone(usize),
    Channel(usize, usize),
}

/// What the pointer has.
enum Drag {
    None,
    /// Going through the frames, on the ruler.
    Scrub,
    Box(Pos2),
    /// The selected keys, from where they were (`before`): by the mouse
    /// button held, or (G) until a click.
    Move { origin: Pos2, before: Vec<BoneKeys>, modal: bool, frames: f32 },
    /// A handle of a key in the Graph Editor: bone and channel (their
    /// indices), key, and which.
    Handle { bone: usize, channel: usize, key: usize, right: bool },
}

#[derive(Clone, Copy, PartialEq)]
enum Menu {
    Interpolation,
    Handles,
}

pub(super) struct Sheet {
    pub mode: Mode,
    /// The frame at the left edge, and the points a frame is wide.
    first: f32,
    per_frame: f32,
    /// The Graph Editor's value at mid height, and the points a unit is tall.
    mid: f32,
    per_unit: f32,
    /// The view was set to show everything, for this mode.
    fitted: Option<Mode>,
    /// How far down the Dope Sheet's rows are scrolled, in points.
    scroll: f32,
    /// The bones whose channels show in the Dope Sheet.
    expanded: HashSet<u16>,
    /// Only the selected bone's keys show.
    pub only_selected: bool,
    drag: Drag,
    menu: Option<(Menu, Pos2)>,
    /// The key last clicked: the one the side panel shows.
    pub active: Option<KeyRef>,
}

impl Default for Sheet {
    fn default() -> Self {
        Sheet {
            mode: Mode::DopeSheet,
            first: -5.0,
            per_frame: 10.0,
            mid: 0.0,
            per_unit: 100.0,
            fitted: None,
            scroll: 0.0,
            expanded: HashSet::new(),
            only_selected: false,
            drag: Drag::None,
            menu: None,
            active: None,
        }
    }
}

/// Every key of the bones shown: `f(bone, channel, key)`.
fn each_key(source: &mut AnimSource, shown: &[usize], mut f: impl FnMut(usize, usize, &mut rf_editor::anim::Keyframe)) {
    for &b in shown {
        for (c, channel) in source.bones[b].channels.iter_mut().enumerate() {
            for key in &mut channel.curve.keys {
                f(b, c, key);
            }
        }
    }
}

/// Deletes the selected keys of the bones shown.
fn delete_selected(source: &mut AnimSource, shown: &[usize]) {
    for &b in shown {
        for c in &mut source.bones[b].channels {
            let n = c.curve.keys.len();
            c.curve.keys.retain(|k| !k.selected);
            if c.curve.keys.len() != n {
                c.curve.recalc();
            }
        }
        source.bones[b].prune();
    }
}

fn any_selected(source: &AnimSource, shown: &[usize]) -> bool {
    shown.iter().any(|b| source.bones[*b].channels.iter().any(|c| c.curve.keys.iter().any(|k| k.selected)))
}

/// A step between grid lines that's at least `least` wide: 1, 2, 5, 10...
fn nice_step(least: f32) -> f32 {
    let p = 10f32.powf(least.max(1e-6).log10().floor());
    [1.0, 2.0, 5.0, 10.0].into_iter().map(|m| m * p).find(|s| *s >= least).unwrap_or(10.0 * p)
}

impl Sheet {
    fn x(&self, area: Rect, frame: f32) -> f32 {
        area.left() + (frame - self.first) * self.per_frame
    }

    fn frame(&self, area: Rect, x: f32) -> f32 {
        self.first + (x - area.left()) / self.per_frame
    }

    fn y(&self, area: Rect, value: f32) -> f32 {
        area.center().y - (value - self.mid) * self.per_unit
    }

    /// The bones whose keys show: the ones with keys, or of them only the selected.
    fn shown(&self, source: &AnimSource, selected: Option<u16>) -> Vec<usize> {
        let keyed = |i: &usize| source.bones[*i].is_keyed();
        let all: Vec<usize> = (0..source.bones.len()).filter(keyed).collect();
        match selected.filter(|_| self.only_selected) {
            Some(tag) => all.into_iter().filter(|i| source.bones[*i].bone == tag).collect(),
            None => all,
        }
    }

    fn rows(&self, source: &AnimSource, shown: &[usize]) -> Vec<RowKind> {
        let mut rows = vec![RowKind::Summary];
        for &b in shown {
            rows.push(RowKind::Bone(b));
            if self.expanded.contains(&source.bones[b].bone) {
                rows.extend((0..source.bones[b].channels.len()).map(|c| RowKind::Channel(b, c)));
            }
        }
        rows
    }

    /// The channels a row stands for: (bone, channel).
    fn row_channels(source: &AnimSource, shown: &[usize], row: RowKind) -> Vec<(usize, usize)> {
        let of = |b: usize| (0..source.bones[b].channels.len()).map(move |c| (b, c));
        match row {
            RowKind::Summary => shown.iter().flat_map(|b| of(*b)).collect(),
            RowKind::Bone(b) => of(b).collect(),
            RowKind::Channel(b, c) => vec![(b, c)],
        }
    }

    /// A row's diamonds: the frames with a key in any of its channels, and
    /// whether all the keys there are selected.
    fn row_keys(source: &AnimSource, channels: &[(usize, usize)]) -> Vec<(f32, bool)> {
        let mut keys: Vec<(f32, bool)> = channels
            .iter()
            .flat_map(|(b, c)| source.bones[*b].channels[*c].curve.keys.iter().map(|k| (k.frame, k.selected)))
            .collect();
        keys.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut out: Vec<(f32, bool)> = Vec::new();
        for (frame, selected) in keys {
            match out.last_mut().filter(|l| (l.0 - frame).abs() < 1e-3) {
                Some(last) => last.1 &= selected,
                None => out.push((frame, selected)),
            }
        }
        out
    }

    /// Shows everything: every frame of the animation, and in the Graph
    /// Editor every value of the curves drawn.
    fn fit(&mut self, area: Rect, source: &AnimSource, shown: &[usize]) {
        let (mut f0, mut f1) = (0.0f32, f32::from(source.frames.saturating_sub(1)).max(1.0));
        let (mut v0, mut v1) = (f32::MAX, f32::MIN);
        for &b in shown {
            for c in source.bones[b].channels.iter().filter(|c| !c.hidden) {
                for k in &c.curve.keys {
                    (f0, f1) = (f0.min(k.frame), f1.max(k.frame));
                    let v = k.value * unit(c.path);
                    (v0, v1) = (v0.min(v), v1.max(v));
                }
            }
        }
        let span = (f1 - f0).max(1.0);
        self.per_frame = (area.width() / (span * 1.1)).max(0.01);
        self.first = f0 - span * 0.05;
        if v0 <= v1 {
            let tall = (v1 - v0).max(0.2);
            self.mid = 0.5 * (v0 + v1);
            self.per_unit = ((area.height() - RULER) / (tall * 1.3)).max(1e-3);
        }
        self.fitted = Some(self.mode);
    }

    /// The active key's channel and index, when it's still there.
    pub fn active_key<'a>(&self, source: &'a mut AnimSource) -> Option<(&'a mut Channel, usize)> {
        let a = self.active?;
        let channel = source.bones.iter_mut().find(|b| b.bone == a.bone)?.channel_mut(a.path, a.index)?;
        let i = channel.curve.key_at(a.frame)?;
        Some((channel, i))
    }

    /// The sheet. Returns whether the frame was set by hand (playing stops).
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        source: &mut AnimSource,
        history: &mut History,
        frame: &mut f32,
        selected: &mut Option<u16>,
    ) -> bool {
        let size = ui.available_size().max(Vec2::new(NAMES + 50.0, RULER + ROW));
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let names = Rect::from_min_max(rect.min, Pos2::new(rect.left() + NAMES, rect.bottom()));
        let area = Rect::from_min_max(Pos2::new(names.right(), rect.top()), rect.max);
        let keys_area = Rect::from_min_max(Pos2::new(area.left(), area.top() + RULER), area.max);
        let shown = self.shown(source, *selected);
        if self.fitted != Some(self.mode) {
            self.fit(area, source, &shown);
        }
        let rows = self.rows(source, &shown);
        let row_y = |i: usize, scroll: f32| keys_area.top() + (i as f32 + 0.5) * ROW - scroll;
        self.scroll = self.scroll.clamp(0.0, (rows.len() as f32 * ROW - keys_area.height()).max(0.0));
        let graph = self.mode == Mode::Graph;

        // Where every key is on screen: (bone, channel, key, point, selected).
        let mut points: Vec<(usize, usize, usize, Pos2, bool)> = Vec::new();
        // The handles of the selected keys: (bone, channel, key, right, point).
        let mut handles: Vec<(usize, usize, usize, bool, Pos2)> = Vec::new();
        if graph {
            for &b in &shown {
                for (c, channel) in source.bones[b].channels.iter().enumerate().filter(|(_, c)| !c.hidden) {
                    let u = unit(channel.path);
                    for (k, key) in channel.curve.keys.iter().enumerate() {
                        let p = Pos2::new(self.x(area, key.frame), self.y(keys_area, key.value * u));
                        points.push((b, c, k, p, key.selected));
                        if key.selected && key.interpolation == Interpolation::Bezier
                            || key.selected && k > 0 && channel.curve.keys[k - 1].interpolation == Interpolation::Bezier
                        {
                            for (right, h) in [(false, key.left), (true, key.right)] {
                                handles.push((b, c, k, right, Pos2::new(self.x(area, h[0]), self.y(keys_area, h[1] * u))));
                            }
                        }
                    }
                }
            }
        }
        // The Dope Sheet's diamonds: (row, frame, point, selected).
        let mut diamonds: Vec<(usize, f32, Pos2, bool)> = Vec::new();
        if !graph {
            for (i, row) in rows.iter().enumerate() {
                let y = row_y(i, self.scroll);
                if y < keys_area.top() - ROW || y > keys_area.bottom() + ROW {
                    continue;
                }
                for (f, sel) in Self::row_keys(source, &Self::row_channels(source, &shown, *row)) {
                    diamonds.push((i, f, Pos2::new(self.x(area, f), y), sel));
                }
            }
        }
        let near = |p: Pos2, q: Pos2| (p - q).length() <= PICK;

        let pointer = ui.input(|i| i.pointer.clone());
        let hover = pointer.hover_pos().filter(|p| rect.contains(*p));
        let press = pointer.press_origin().unwrap_or_default();
        let shift = ui.input(|i| i.modifiers.shift);
        let mut scrubbed = false;
        let last = f32::from(source.frames.saturating_sub(1));
        let go_to = |x: f32, sheet: &Sheet, frame: &mut f32| {
            *frame = sheet.frame(area, x).round().clamp(0.0, last);
        };

        // Selecting what's under a point: returns whether there was a key.
        let pick = |at: Pos2, source: &mut AnimSource, sheet: &mut Sheet, selected: &mut Option<u16>, toggle: bool| {
            let hit_point = points.iter().find(|p| near(p.3, at)).map(|p| (p.0, p.1, p.2, p.4));
            let hit_diamond = diamonds.iter().find(|d| near(d.2, at)).map(|d| (d.0, d.1, d.3));
            if hit_point.is_none() && hit_diamond.is_none() {
                return false;
            }
            let was = hit_point.map(|p| p.3).or(hit_diamond.map(|d| d.2)).unwrap_or(false);
            if !toggle && !was {
                each_key(source, &shown, |_, _, k| k.selected = false);
            }
            let to = if toggle { !was } else { true };
            if let Some((b, c, k, _)) = hit_point {
                let bone = &mut source.bones[b];
                bone.channels[c].curve.keys[k].selected = to;
                let (ch, key) = (&bone.channels[c], bone.channels[c].curve.keys[k]);
                sheet.active = Some(KeyRef { bone: bone.bone, path: ch.path, index: ch.index, frame: key.frame });
                *selected = Some(bone.bone);
            } else if let Some((row, f, _)) = hit_diamond {
                let channels = Self::row_channels(source, &shown, rows[row]);
                for (b, c) in &channels {
                    for key in &mut source.bones[*b].channels[*c].curve.keys {
                        if (key.frame - f).abs() < 1e-3 {
                            key.selected = to;
                        }
                    }
                }
                if let Some((b, c)) = channels.first().filter(|_| rows[row] != RowKind::Summary) {
                    let bone = &source.bones[*b];
                    let ch = &bone.channels[*c];
                    sheet.active = Some(KeyRef { bone: bone.bone, path: ch.path, index: ch.index, frame: f });
                    *selected = Some(bone.bone);
                }
            }
            true
        };

        // A grab (G) follows the mouse until a click.
        let modal = matches!(self.drag, Drag::Move { modal: true, .. });
        if modal {
            let cancel = ui.input(|i| i.key_pressed(egui::Key::Escape)) || pointer.button_pressed(egui::PointerButton::Secondary);
            let confirm =
                pointer.button_pressed(egui::PointerButton::Primary) || ui.input(|i| i.key_pressed(egui::Key::Enter));
            if let (Drag::Move { origin, before, .. }, Some(p)) = (&self.drag, pointer.latest_pos()) {
                let (frames, value) = self.moved(*origin, p);
                Self::apply_move(source, before, frames, if graph { value } else { 0.0 });
                if let Drag::Move { frames: f, .. } = &mut self.drag {
                    *f = frames;
                }
            }
            if cancel {
                if let Drag::Move { before, .. } = std::mem::replace(&mut self.drag, Drag::None) {
                    source.bones = before;
                    history.forget();
                }
            } else if confirm {
                self.finish_move();
            }
            ui.ctx().request_repaint();
        } else if response.drag_started_by(egui::PointerButton::Primary) {
            let handle = handles.iter().find(|h| near(h.4, press)).map(|h| (h.0, h.1, h.2, h.3));
            if press.y < area.top() + RULER && press.x >= area.left() {
                self.drag = Drag::Scrub;
            } else if press.x < area.left() {
                self.drag = Drag::None;
            } else if let Some((bone, channel, key, right)) = handle {
                history.step(source);
                self.drag = Drag::Handle { bone, channel, key, right };
            } else if pick(press, source, self, selected, false) {
                history.step(source);
                self.drag = Drag::Move { origin: press, before: source.bones.clone(), modal: false, frames: 0.0 };
            } else {
                self.drag = Drag::Box(press);
            }
        }
        // (The button is up by the time a click is one: where it was pressed is gone.)
        let clicked = response.interact_pointer_pos().or(pointer.interact_pos()).unwrap_or_default();
        if !modal && response.clicked_by(egui::PointerButton::Primary) && clicked.x >= area.left() {
            if clicked.y < area.top() + RULER {
                go_to(clicked.x, self, frame);
                scrubbed = true;
            } else if !pick(clicked, source, self, selected, shift) && !shift {
                each_key(source, &shown, |_, _, k| k.selected = false);
                self.active = None;
            }
        }
        if !modal && response.dragged_by(egui::PointerButton::Primary) {
            let at = pointer.latest_pos().unwrap_or(press);
            match &mut self.drag {
                Drag::Scrub => {
                    *frame = (self.first + (at.x - area.left()) / self.per_frame)
                        .round()
                        .clamp(0.0, f32::from(source.frames.saturating_sub(1)));
                    scrubbed = true;
                }
                Drag::Move { origin, before, frames, .. } => {
                    let dx = ((at.x - origin.x) / self.per_frame).round();
                    let dv = -(at.y - origin.y) / self.per_unit;
                    *frames = dx;
                    Self::apply_move(source, before, dx, if graph { dv } else { 0.0 });
                }
                Drag::Handle { bone, channel, key, right } => {
                    let ch = &mut source.bones[*bone].channels[*channel];
                    let u = unit(ch.path);
                    let to =
                        [self.first + (at.x - area.left()) / self.per_frame, (self.mid - (at.y - keys_area.center().y) / self.per_unit) / u];
                    drag_handle(&mut ch.curve.keys[*key], *right, to);
                    ch.curve.recalc();
                }
                Drag::Box(_) | Drag::None => {}
            }
        }
        if !modal && response.drag_stopped_by(egui::PointerButton::Primary) {
            match std::mem::replace(&mut self.drag, Drag::None) {
                Drag::Box(from) => {
                    let to = pointer.latest_pos().or(pointer.interact_pos()).unwrap_or(from);
                    let r = Rect::from_two_pos(from, to);
                    if !shift {
                        each_key(source, &shown, |_, _, k| k.selected = false);
                    }
                    for (b, c, k, p, _) in &points {
                        if r.contains(*p) {
                            source.bones[*b].channels[*c].curve.keys[*k].selected = true;
                        }
                    }
                    for (row, f, p, _) in &diamonds {
                        if r.contains(*p) {
                            for (b, c) in Self::row_channels(source, &shown, rows[*row]) {
                                for key in &mut source.bones[b].channels[c].curve.keys {
                                    key.selected |= (key.frame - f).abs() < 1e-3;
                                }
                            }
                        }
                    }
                }
                moved @ Drag::Move { .. } => {
                    self.drag = moved;
                    self.finish_move();
                }
                _ => {}
            }
        }

        // The middle button pans; the wheel zooms the frames (and, with Ctrl, the values).
        if response.dragged_by(egui::PointerButton::Middle) {
            let d = response.drag_delta();
            self.first -= d.x / self.per_frame;
            if graph {
                self.mid += d.y / self.per_unit;
            } else {
                self.scroll -= d.y;
            }
        }
        if let Some(p) = hover {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
            if scroll.y != 0.0 && p.x >= area.left() {
                let at = self.frame(area, p.x);
                self.per_frame = (self.per_frame * (scroll.y * 0.004).exp()).clamp(0.05, 200.0);
                self.first = at - (p.x - area.left()) / self.per_frame;
            } else if scroll.y != 0.0 {
                self.scroll -= scroll.y;
            }
            if scroll.x != 0.0 {
                self.first -= scroll.x / self.per_frame;
            }
            if zoom != 1.0 && graph {
                let at = self.mid - (p.y - keys_area.center().y) / self.per_unit;
                self.per_unit = (self.per_unit * zoom).clamp(1e-3, 1e5);
                self.mid = at + (p.y - keys_area.center().y) / self.per_unit;
            }
        }

        // The keys Blender has.
        if let Some(p) = hover.filter(|_| !modal && !ui.ctx().egui_wants_keyboard_input()) {
            let key = |k: egui::Key| ui.input(|i| i.key_pressed(k));
            let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
            if key(egui::Key::A) {
                each_key(source, &shown, |_, _, k| k.selected = !alt);
            }
            if key(egui::Key::Home) {
                self.fit(area, source, &shown);
            }
            let some = any_selected(source, &shown);
            if (key(egui::Key::X) || key(egui::Key::Delete)) && some {
                history.step(source);
                delete_selected(source, &shown);
                self.active = None;
            }
            if key(egui::Key::D) && shift && some {
                history.step(source);
                for &b in &shown {
                    for c in &mut source.bones[b].channels {
                        let copies: Vec<_> = c.curve.keys.iter().filter(|k| k.selected).copied().collect();
                        c.curve.keys.iter_mut().for_each(|k| k.selected = false);
                        c.curve.keys.extend(copies);
                    }
                }
                self.drag = Drag::Move { origin: p, before: source.bones.clone(), modal: true, frames: 0.0 };
            } else if key(egui::Key::G) && !alt && some {
                history.step(source);
                self.drag = Drag::Move { origin: p, before: source.bones.clone(), modal: true, frames: 0.0 };
            }
            if key(egui::Key::T) && some {
                self.menu = Some((Menu::Interpolation, p));
            }
            if key(egui::Key::V) && some {
                self.menu = Some((Menu::Handles, p));
            }
            if key(egui::Key::I) && !alt {
                // A key at the frame shown in every channel shown, at the curve's value there.
                history.step(source);
                for &b in &shown {
                    for c in source.bones[b].channels.iter_mut().filter(|c| !c.hidden) {
                        if let Some(v) = c.curve.evaluate(*frame) {
                            c.curve.insert(*frame, v);
                        }
                    }
                }
            }
        }
        self.menu_ui(ui, source, history, &shown);

        // Drawing.
        let painter = ui.painter().with_clip_rect(rect);
        let visuals = ui.visuals().clone();
        painter.rect_filled(area, 0.0, visuals.extreme_bg_color);
        // The frames played are lighter than the ones before and after.
        let played = Rect::from_min_max(
            Pos2::new(self.x(area, 0.0).max(area.left()), keys_area.top()),
            Pos2::new(self.x(area, f32::from(source.frames.saturating_sub(1))).min(area.right()), area.bottom()),
        );
        if played.width() > 0.0 {
            painter.rect_filled(played, 0.0, visuals.faint_bg_color);
        }
        let font = FontId::proportional(11.0);
        let step = nice_step(60.0 / self.per_frame).max(1.0);
        let mut f = (self.first / step).floor() * step;
        let clip = painter.with_clip_rect(area);
        while self.x(area, f) < area.right() {
            let x = self.x(area, f);
            clip.line_segment([Pos2::new(x, area.top() + RULER), Pos2::new(x, area.bottom())], Stroke::new(1.0, Color32::from_white_alpha(14)));
            clip.text(Pos2::new(x + 3.0, area.top() + 4.0), Align2::LEFT_TOP, format!("{f:.0}"), font.clone(), theme::WEAK);
            f += step;
        }
        let keys_clip = painter.with_clip_rect(keys_area);
        if graph {
            let vstep = nice_step(40.0 / self.per_unit);
            let top = self.mid + keys_area.height() * 0.5 / self.per_unit;
            let mut v = ((self.mid - keys_area.height() * 0.5 / self.per_unit) / vstep).floor() * vstep;
            while v <= top {
                let y = self.y(keys_area, v);
                let strong = if v.abs() < vstep * 0.5 { 40 } else { 14 };
                keys_clip.line_segment([Pos2::new(area.left(), y), Pos2::new(area.right(), y)], Stroke::new(1.0, Color32::from_white_alpha(strong)));
                let digits = if vstep < 0.1 { 2 } else if vstep < 1.0 { 1 } else { 0 };
                keys_clip.text(Pos2::new(area.left() + 3.0, y - 1.0), Align2::LEFT_BOTTOM, format!("{v:.digits$}"), font.clone(), theme::WEAK);
                v += vstep;
            }
            for &b in &shown {
                for c in source.bones[b].channels.iter().filter(|c| !c.hidden) {
                    let u = unit(c.path);
                    let mut line = Vec::new();
                    let mut x = area.left();
                    while x <= area.right() + 2.0 {
                        if let Some(v) = c.curve.evaluate(self.frame(area, x)) {
                            line.push(Pos2::new(x, self.y(keys_area, v * u)));
                        }
                        x += 2.0;
                    }
                    keys_clip.add(Shape::line(line, Stroke::new(1.5, colour(c))));
                }
            }
            for (b, c, k, right, p) in &handles {
                let key = source.bones[*b].channels[*c].curve.keys[*k];
                let u = unit(source.bones[*b].channels[*c].path);
                let at = Pos2::new(self.x(area, key.frame), self.y(keys_area, key.value * u));
                let kind = if *right { key.right_type } else { key.left_type };
                let tint = match kind {
                    Handle::Free => Color32::from_rgb(220, 90, 90),
                    Handle::Aligned => Color32::from_rgb(220, 120, 200),
                    Handle::Vector => Color32::from_rgb(110, 200, 70),
                    Handle::Auto | Handle::AutoClamped => Color32::from_rgb(230, 200, 80),
                };
                keys_clip.line_segment([at, *p], Stroke::new(1.0, tint));
                keys_clip.circle_stroke(*p, 3.0, Stroke::new(1.5, tint));
            }
            for (_, _, _, p, sel) in &points {
                keys_clip.circle_filled(*p, 3.5, if *sel { theme::ACCENT } else { Color32::BLACK });
                keys_clip.circle_stroke(*p, 3.5, Stroke::new(1.0, if *sel { Color32::WHITE } else { KEY }));
            }
        } else {
            for (i, row) in rows.iter().enumerate() {
                let y = row_y(i, self.scroll);
                let band = Rect::from_min_max(Pos2::new(area.left(), y - ROW * 0.5), Pos2::new(area.right(), y + ROW * 0.5));
                let shade = match row {
                    RowKind::Summary => 26,
                    RowKind::Bone(_) => 16,
                    RowKind::Channel(..) => 6,
                };
                keys_clip.rect_filled(band.shrink2(Vec2::new(0.0, 0.5)), 0.0, Color32::from_white_alpha(shade));
            }
            for (_, _, p, sel) in &diamonds {
                let r = 5.0;
                let shape = vec![
                    Pos2::new(p.x, p.y - r),
                    Pos2::new(p.x + r, p.y),
                    Pos2::new(p.x, p.y + r),
                    Pos2::new(p.x - r, p.y),
                ];
                let fill = if *sel { theme::ACCENT } else { KEY };
                keys_clip.add(Shape::convex_polygon(shape, fill, Stroke::new(1.0, Color32::BLACK)));
            }
        }
        if let (Drag::Box(from), Some(to)) = (&self.drag, pointer.latest_pos()) {
            let r = Rect::from_two_pos(*from, to);
            painter.rect_filled(r, 0.0, Color32::from_white_alpha(18));
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_white_alpha(120)), egui::StrokeKind::Inside);
        }
        // The ruler's line, and the frame shown.
        clip.line_segment([Pos2::new(area.left(), area.top() + RULER), Pos2::new(area.right(), area.top() + RULER)], Stroke::new(1.0, Color32::from_white_alpha(30)));
        let x = self.x(area, *frame);
        clip.line_segment([Pos2::new(x, area.top()), Pos2::new(x, area.bottom())], Stroke::new(2.0, PLAYHEAD));
        let label = format!("{:.0}", frame.round());
        let galley = painter.layout_no_wrap(label, font.clone(), Color32::WHITE);
        let tag = Rect::from_center_size(Pos2::new(x, area.top() + RULER * 0.5), galley.size() + Vec2::new(10.0, 4.0));
        clip.rect_filled(tag, 3.0, PLAYHEAD);
        clip.galley(tag.center() - galley.size() * 0.5, galley, Color32::WHITE);

        self.names_ui(ui, names, source, &shown, &rows, selected);
        if matches!(self.drag, Drag::Move { .. } | Drag::Handle { .. } | Drag::Scrub) {
            ui.ctx().request_repaint();
        }
        scrubbed
    }

    /// How far the keys grabbed have gone: whole frames, and value.
    fn moved(&self, origin: Pos2, at: Pos2) -> (f32, f32) {
        (((at.x - origin.x) / self.per_frame).round(), -(at.y - origin.y) / self.per_unit)
    }

    /// The keys as they were, the selected ones moved (the value in the
    /// units shown).
    fn apply_move(source: &mut AnimSource, before: &[BoneKeys], frames: f32, value: f32) {
        source.bones = before.to_vec();
        for b in &mut source.bones {
            for c in &mut b.channels {
                let u = unit(c.path);
                let mut any = false;
                for k in c.curve.keys.iter_mut().filter(|k| k.selected) {
                    k.shift(frames, value / u);
                    any = true;
                }
                if any {
                    c.curve.tidy();
                }
            }
        }
    }

    fn finish_move(&mut self) {
        if let Drag::Move { frames, .. } = std::mem::replace(&mut self.drag, Drag::None)
            && let Some(a) = &mut self.active
        {
            a.frame += frames;
        }
    }

    /// The T and V menus, where the key was pressed.
    fn menu_ui(&mut self, ui: &egui::Ui, source: &mut AnimSource, history: &mut History, shown: &[usize]) {
        let Some((menu, at)) = self.menu else { return };
        let mut close = ui.input(|i| i.key_pressed(egui::Key::Escape));
        let area = egui::Area::new(ui.id().with("anim-sheet-menu")).fixed_pos(at).order(egui::Order::Foreground);
        let shown_menu = area.show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(150.0);
                match menu {
                    Menu::Interpolation => {
                        ui.label(egui::RichText::new("Interpolation").weak());
                        for kind in Interpolation::ALL {
                            if ui.button(kind.label()).clicked() {
                                history.step(source);
                                each_key(source, shown, |_, _, k| {
                                    if k.selected {
                                        k.interpolation = kind;
                                    }
                                });
                                close = true;
                            }
                        }
                    }
                    Menu::Handles => {
                        ui.label(egui::RichText::new("Handle Type").weak());
                        for kind in Handle::ALL {
                            if ui.button(kind.label()).clicked() {
                                history.step(source);
                                each_key(source, shown, |_, _, k| {
                                    if k.selected {
                                        (k.left_type, k.right_type) = (kind, kind);
                                    }
                                });
                                for &b in shown {
                                    source.bones[b].channels.iter_mut().for_each(|c| c.curve.recalc());
                                }
                                close = true;
                            }
                        }
                    }
                }
            });
        });
        if shown_menu.response.clicked_elsewhere() {
            close = true;
        }
        if close {
            self.menu = None;
        }
    }

    /// The channel list on the left.
    fn names_ui(
        &mut self,
        ui: &mut egui::Ui,
        names: Rect,
        source: &mut AnimSource,
        shown: &[usize],
        rows: &[RowKind],
        selected: &mut Option<u16>,
    ) {
        let painter = ui.painter().with_clip_rect(names);
        painter.rect_filled(names, 0.0, ui.visuals().panel_fill);
        painter.line_segment([names.right_top(), names.right_bottom()], Stroke::new(1.0, Color32::from_white_alpha(30)));
        let font = FontId::proportional(12.5);
        let top = names.top() + RULER;
        // The Graph Editor lists the channels drawn; the Dope Sheet its rows.
        let list: Vec<RowKind> = if self.mode == Mode::Graph {
            shown
                .iter()
                .flat_map(|b| {
                    std::iter::once(RowKind::Bone(*b))
                        .chain((0..source.bones[*b].channels.len()).map(move |c| RowKind::Channel(*b, c)))
                })
                .collect()
        } else {
            rows.to_vec()
        };
        let click = ui.input(|i| i.pointer.primary_clicked().then(|| i.pointer.interact_pos()).flatten());
        for (i, row) in list.iter().enumerate() {
            let y = top + (i as f32 + 0.5) * ROW - self.scroll;
            if y < top - ROW || y > names.bottom() + ROW {
                continue;
            }
            let band = Rect::from_min_max(Pos2::new(names.left(), y - ROW * 0.5), Pos2::new(names.right(), y + ROW * 0.5));
            if band.center().y < top {
                continue;
            }
            let hit = click.filter(|p| band.contains(*p));
            match *row {
                RowKind::Summary => {
                    painter.text(Pos2::new(names.left() + 8.0, y), Align2::LEFT_CENTER, "Summary", font.clone(), theme::TEXT);
                }
                RowKind::Bone(b) => {
                    let bone = &source.bones[b];
                    let open = self.mode == Mode::Graph || self.expanded.contains(&bone.bone);
                    let active = *selected == Some(bone.bone);
                    if active {
                        painter.rect_filled(band, 0.0, Color32::from_white_alpha(16));
                    }
                    let arrow = if open { icon::CARET_DOWN } else { icon::CARET_RIGHT };
                    painter.text(Pos2::new(names.left() + 8.0, y), Align2::LEFT_CENTER, arrow, font.clone(), theme::WEAK);
                    let name = if bone.name.is_empty() { format!("bone {}", bone.bone) } else { bone.name.clone() };
                    let tint = if active { theme::ACCENT } else { theme::TEXT };
                    painter.text(Pos2::new(names.left() + 24.0, y), Align2::LEFT_CENTER, name, font.clone(), tint);
                    if let Some(p) = hit {
                        if p.x < names.left() + 22.0 && self.mode == Mode::DopeSheet {
                            if !self.expanded.remove(&bone.bone) {
                                self.expanded.insert(bone.bone);
                            }
                        } else {
                            *selected = Some(bone.bone);
                        }
                    }
                }
                RowKind::Channel(b, c) => {
                    let channel = &mut source.bones[b].channels[c];
                    let tint = if channel.hidden { theme::WEAK } else { colour(channel) };
                    painter.circle_filled(Pos2::new(names.left() + 30.0, y), 3.5, tint);
                    let label = channel.path.label(channel.index);
                    let text = if channel.hidden { theme::WEAK } else { theme::TEXT };
                    painter.text(Pos2::new(names.left() + 42.0, y), Align2::LEFT_CENTER, label, font.clone(), text);
                    if self.mode == Mode::Graph {
                        let eye = if channel.hidden { icon::EYE_SLASH } else { icon::EYE };
                        painter.text(Pos2::new(names.right() - 10.0, y), Align2::RIGHT_CENTER, eye, font.clone(), theme::WEAK);
                        if hit.is_some() {
                            channel.hidden = !channel.hidden;
                        }
                    }
                }
            }
        }
    }
}

/// A handle dragged to a point (frame, value), as Blender does it: an
/// automatic handle becomes aligned and a vector one free, a handle stays on
/// its side of the key, and an aligned one keeps the other in line with it.
fn drag_handle(key: &mut rf_editor::anim::Keyframe, right: bool, mut to: [f32; 2]) {
    to[0] = if right { to[0].max(key.frame + 1e-3) } else { to[0].min(key.frame - 1e-3) };
    for kind in [&mut key.left_type, &mut key.right_type] {
        if matches!(*kind, Handle::Auto | Handle::AutoClamped) {
            *kind = Handle::Aligned;
        }
    }
    let (kind, other_kind) = if right { (&mut key.right_type, key.left_type) } else { (&mut key.left_type, key.right_type) };
    if *kind == Handle::Vector {
        *kind = Handle::Free;
    }
    let aligned = *kind == Handle::Aligned && other_kind == Handle::Aligned;
    let (at, frame, value) = ([key.frame, key.value], key.frame, key.value);
    let (handle, other) = if right { (&mut key.right, &mut key.left) } else { (&mut key.left, &mut key.right) };
    *handle = to;
    if aligned {
        // The other handle keeps its length, opposite this one.
        let d = [to[0] - frame, to[1] - value];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
        let o = [other[0] - at[0], other[1] - at[1]];
        let other_len = (o[0] * o[0] + o[1] * o[1]).sqrt();
        if len > 1e-6 {
            *other = [frame - d[0] / len * other_len, value - d[1] / len * other_len];
        }
    }
}
