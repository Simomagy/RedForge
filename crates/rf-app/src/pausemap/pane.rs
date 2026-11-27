//! The Pause map tab: the game's map drawn on the GPU with what's drawn on
//! it, the tools and their gestures (ported from the web app's map canvas),
//! the editing chrome over it and the status bar.

use std::collections::{HashMap, HashSet};

use eframe::egui::{self, Color32, CursorIcon, Key, Pos2, Rect, Sense, Stroke, Vec2};
use rf_editor::Editor;
use rf_editor::pausemap::commands;
use rf_editor::pausemap::doc::{
    drawables_in, drill_target, index_of, is_hidden, is_locked, parent_of, pick_target, with_descendants,
};
use rf_editor::pausemap::families::{Layer, WORLD_H, WORLD_W, detail, map_to_world, tile_at, world_to_map};
use rf_editor::pausemap::fonts::DEFAULT_FONT;
use rf_editor::pausemap::geometry::{
    Affine, box_of, hits, is_boxed, line_path, selection_bounds, touches_rect, transform_object, translate,
};
use rf_editor::pausemap::linework::Bounds;
use rf_editor::pausemap::path::{PathNode, Point, insert_node, nearest_on_path};
use rf_editor::pausemap::project::*;

use super::gpu::{self, MapGpu, Paint, Theme};
use super::{PauseMap, Tool};
use crate::map::tools::{Tools, count};
use crate::theme::{self, icon};
use crate::viewers::Gpu;

/// Points a click may wander before it counts as a drag.
const CLICK_SLOP: f32 = 4.0;
/// A click with the building tool places one this big (map units).
const BLOCK: (f64, f64) = (10.0, 7.0);
/// The anchor's square, a handle's knob, the transform's corners, in points.
const ANCHOR: f32 = 7.0;
const HANDLE_TIP: f32 = 4.0;
const CORNER: f32 = 8.0;
const KNOB_GAP: f32 = 24.0;
const FRAME_PAD: f32 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameHandle {
    Nw,
    N,
    Ne,
    E,
    Se,
    S,
    Sw,
    W,
}

impl FrameHandle {
    const ALL: [FrameHandle; 8] = [
        FrameHandle::Nw,
        FrameHandle::N,
        FrameHandle::Ne,
        FrameHandle::E,
        FrameHandle::Se,
        FrameHandle::S,
        FrameHandle::Sw,
        FrameHandle::W,
    ];
    fn west(self) -> bool {
        matches!(self, FrameHandle::Nw | FrameHandle::W | FrameHandle::Sw)
    }
    fn east(self) -> bool {
        matches!(self, FrameHandle::Ne | FrameHandle::E | FrameHandle::Se)
    }
    fn north(self) -> bool {
        matches!(self, FrameHandle::Nw | FrameHandle::N | FrameHandle::Ne)
    }
    fn south(self) -> bool {
        matches!(self, FrameHandle::Sw | FrameHandle::S | FrameHandle::Se)
    }
    fn corner(self) -> bool {
        (self.west() || self.east()) && (self.north() || self.south())
    }
}

enum Gesture {
    Pan {
        last: Pos2,
    },
    /// The text tool: a click places a label, a drag pans.
    Click {
        at: Pos2,
    },
    Marquee {
        from: Point,
        to: Point,
        base: Vec<String>,
        nodes: bool,
    },
    Move {
        ids: HashSet<String>,
        last: Point,
    },
    Nodes {
        id: String,
        last: Point,
    },
    Handle {
        id: String,
        i: usize,
        out: bool,
    },
    Rotate {
        id: String,
        center: Point,
        start: f64,
        rot0: f64,
    },
    Scale {
        id: String,
        center: Point,
        d0: f64,
        o0: MapObject,
    },
    Pen {
        id: String,
        i: usize,
    },
    FScale {
        handle: FrameHandle,
        b: Bounds,
        originals: HashMap<String, MapObject>,
    },
    FRotate {
        center: Point,
        start: f64,
        originals: HashMap<String, MapObject>,
    },
    Erase {
        id: String,
    },
    Build {
        id: String,
        from: Point,
        at: Pos2,
    },
}

impl Gesture {
    /// What the step it makes is called.
    fn label(&self) -> &'static str {
        match self {
            Gesture::Move { .. } => "Move",
            Gesture::Nodes { .. } | Gesture::Handle { .. } => "Edit points",
            Gesture::Rotate { .. } | Gesture::FRotate { .. } => "Rotate",
            Gesture::Scale { .. } | Gesture::FScale { .. } => "Resize",
            Gesture::Pen { .. } => "Draw line",
            Gesture::Erase { .. } => "Erase",
            Gesture::Build { .. } => "Add building",
            _ => "Edit",
        }
    }
}

impl PauseMap {
    /// A gesture on the map is going on (its draft isn't a field's).
    pub fn busy(&self) -> bool {
        self.view.gesture.is_some()
    }
}

/// The view's own state, besides what [`PauseMap`] shares.
#[derive(Default)]
pub struct ViewState {
    gesture: Option<Gesture>,
    /// The line the pen is drawing.
    pen: Option<String>,
    hover: Option<String>,
    /// The pen is over the first anchor: a click closes the path.
    closing: bool,
    /// The map point under the pointer.
    cursor: Option<Point>,
    rect: Option<Rect>,
    fitted: bool,
    /// Frames since the tiles started loading (the first shows a note).
    loading: u8,
    /// Where the context menu was opened, in map units.
    menu_at: Option<Point>,
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

/// Shift keeps a vector to the nearest 45°.
fn constrain([x, y]: Point) -> Point {
    let step = std::f64::consts::FRAC_PI_4;
    let a = (y.atan2(x) / step).round() * step;
    let r = x.hypot(y);
    [a.cos() * r, a.sin() * r]
}

fn rect_of(a: Point, b: Point) -> Bounds {
    Bounds { x0: a[0].min(b[0]), y0: a[1].min(b[1]), x1: a[0].max(b[0]), y1: a[1].max(b[1]) }
}

/// Map units to the screen and back.
#[derive(Clone, Copy)]
struct View {
    rect: Rect,
    cam: super::Camera,
}

impl View {
    fn screen(&self, [x, y]: Point) -> Pos2 {
        Pos2::new(
            self.rect.min.x + ((x - self.cam.x) * self.cam.zoom) as f32,
            self.rect.min.y + ((y - self.cam.y) * self.cam.zoom) as f32,
        )
    }
    fn map(&self, p: Pos2) -> Point {
        [
            self.cam.x + (p.x - self.rect.min.x) as f64 / self.cam.zoom,
            self.cam.y + (p.y - self.rect.min.y) as f64 / self.cam.zoom,
        ]
    }
    fn near(&self, a: Point, b: Point, px: f32) -> bool {
        self.screen(a).distance(self.screen(b)) <= px
    }
}

impl PauseMap {
    fn fit(&mut self, rect: Rect) {
        self.frame(rect, Bounds { x0: 0.0, y0: 0.0, x1: WORLD_W, y1: WORLD_H }, 0.95, 64.0);
    }

    /// A camera that shows a box of the map.
    fn frame(&mut self, rect: Rect, b: Bounds, fill: f64, max_zoom: f64) {
        let (w, h) = (rect.width().max(100.0) as f64, rect.height().max(100.0) as f64);
        let zoom = (w / (b.x1 - b.x0).max(1.0)).min(h / (b.y1 - b.y0).max(1.0)) * fill;
        let zoom = zoom.min(max_zoom);
        let (cx, cy) = ((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
        self.camera = super::Camera { zoom, x: cx - w / 2.0 / zoom, y: cy - h / 2.0 / zoom };
    }

    fn zoom_at(&mut self, factor: f64, at: Pos2, rect: Rect) {
        let c = self.camera;
        let zoom = (c.zoom * factor).clamp(0.05, 64.0);
        let (sx, sy) = ((at.x - rect.min.x) as f64, (at.y - rect.min.y) as f64);
        let (wx, wy) = (c.x + sx / c.zoom, c.y + sy / c.zoom);
        self.camera = super::Camera { zoom, x: wx - sx / zoom, y: wy - sy / zoom };
    }

    /// Lines whose anchors show: every selected line with the direct tool
    /// (and the one under the pointer), the single selected one with Select.
    fn edit_lines(&self, objects: &[MapObject]) -> Vec<String> {
        let is_line = |id: &str| objects.iter().any(|o| o.id() == id && matches!(o, MapObject::Line(_)));
        let mut lines: Vec<String> = self.selection.iter().filter(|id| is_line(id)).cloned().collect();
        match self.tool {
            Tool::Direct => {
                if let Some(h) = &self.view.hover
                    && !lines.contains(h)
                    && is_line(h)
                {
                    lines.push(h.clone());
                }
                lines
            }
            Tool::Select if self.selection.len() == 1 => lines,
            _ => Vec::new(),
        }
    }

    /// The single label, building or stamp the Select tool transforms.
    fn transform_target<'a>(&self, objects: &'a [MapObject]) -> Option<&'a MapObject> {
        if self.tool != Tool::Select || self.selection.len() != 1 {
            return None;
        }
        let index = index_of(objects);
        let o = index.get(self.selection[0].as_str()).copied()?;
        (is_boxed(o) && !is_locked(o, &index)).then_some(o)
    }

    /// The frame around a selection that isn't one boxed object.
    fn frame_target(&self, objects: &[MapObject], ed: &Editor) -> Option<Bounds> {
        if self.tool != Tool::Select || self.selection.is_empty() || self.transform_target(objects).is_some() {
            return None;
        }
        selection_bounds(&self.selection, objects, &ed.pause_assets)
    }
}

/// The screen points of a boxed object's corners and rotation knob.
fn transform_handles(o: &MapObject, v: &View, ed: &Editor) -> (Vec<Pos2>, Pos2, Pos2) {
    let corners: Vec<Pos2> = box_of(o, &ed.pause_assets).into_iter().map(|p| v.screen(p)).collect();
    if corners.len() < 2 {
        return (corners, Pos2::ZERO, Pos2::ZERO);
    }
    let (a, b) = (corners[0], corners[1]);
    let mid = a + (b - a) / 2.0;
    let e = b - a;
    let len = e.length().max(1.0);
    let knob = Pos2::new(mid.x + e.y / len * KNOB_GAP, mid.y - e.x / len * KNOB_GAP);
    (corners, mid, knob)
}

/// A selection frame: its rect, the eight handles, the top and the knob.
fn frame_handles(b: &Bounds, v: &View) -> (Rect, Vec<(FrameHandle, Pos2)>, Pos2, Pos2) {
    let (p0, p1) = (v.screen([b.x0, b.y0]), v.screen([b.x1, b.y1]));
    let r = Rect::from_min_max(p0, p1).expand(FRAME_PAD);
    let (mx, my) = (r.center().x, r.center().y);
    let at = |h: FrameHandle| match h {
        FrameHandle::Nw => r.left_top(),
        FrameHandle::N => Pos2::new(mx, r.top()),
        FrameHandle::Ne => r.right_top(),
        FrameHandle::E => Pos2::new(r.right(), my),
        FrameHandle::Se => r.right_bottom(),
        FrameHandle::S => Pos2::new(mx, r.bottom()),
        FrameHandle::Sw => r.left_bottom(),
        FrameHandle::W => Pos2::new(r.left(), my),
    };
    let handles = FrameHandle::ALL.iter().map(|&h| (h, at(h))).collect();
    let top = Pos2::new(mx, r.top());
    (r, handles, top, Pos2::new(mx, r.top() - KNOB_GAP))
}

enum PointHit {
    Anchor { id: String, i: usize },
    Handle { id: String, i: usize, out: bool },
}

/// The top unlocked, shown object under a point.
fn drawable_at<'a>(
    objects: &'a [MapObject],
    p: Point,
    slop: f64,
    only_labels: bool,
    ed: &Editor,
) -> Option<&'a MapObject> {
    let index = index_of(objects);
    objects.iter().rev().find(|o| {
        !o.is_group()
            && (!only_labels || matches!(o, MapObject::Label(_)))
            && !is_hidden(o, &index)
            && !is_locked(o, &index)
            && hits(o, p, slop, &ed.pause_assets)
    })
}

/// Whether anchor `i`'s handles show: chosen, or next to a chosen one.
fn handles_show(chosen: &[usize], i: usize, n: usize, closed: bool) -> bool {
    let has = |k: usize| chosen.contains(&k);
    has(i) || (i > 0 && has(i - 1)) || has(i + 1) || (closed && (has((i + 1) % n) || has((i + n - 1) % n)))
}

fn point_at(pm: &PauseMap, objects: &[MapObject], p: Point, v: &View) -> Option<PointHit> {
    for id in pm.edit_lines(objects).iter().rev() {
        let Some(MapObject::Line(l)) = objects.iter().find(|o| o.id() == id) else { continue };
        let chosen: &[usize] = if pm.primary() == Some(id) { &pm.nodes } else { &[] };
        let n = l.nodes.len();
        for (i, node) in l.nodes.iter().enumerate() {
            if !handles_show(chosen, i, n, l.closed) {
                continue;
            }
            for (out, h) in [(true, node.out), (false, node.inn)] {
                if let Some(h) = h
                    && v.near([node.p[0] + h[0], node.p[1] + h[1]], p, HANDLE_TIP + 3.0)
                {
                    return Some(PointHit::Handle { id: id.clone(), i, out });
                }
            }
        }
        for i in (0..n).rev() {
            if v.near(l.nodes[i].p, p, ANCHOR / 2.0 + 3.0) {
                return Some(PointHit::Anchor { id: id.clone(), i });
            }
        }
    }
    None
}

fn line_mut<'a>(objects: &'a mut [MapObject], id: &str) -> Option<&'a mut LineObject> {
    match objects.iter_mut().find(|o| o.id() == id) {
        Some(MapObject::Line(l)) => Some(l),
        _ => None,
    }
}

/// Draws the tab.
pub fn ui(ui: &mut egui::Ui, pm: &mut PauseMap, ed: &mut Editor, gpu: &Gpu, tools: &mut Tools) {
    // The game's tiles, the first time: reading them takes a second.
    let ready = gpu.egui.read().callback_resources.get::<MapGpu>().is_some_and(MapGpu::loaded);
    if !ready {
        if pm.view.loading == 0 {
            pm.view.loading = 1;
            ui.centered_and_justified(|ui| ui.label("Reading the game's map tiles…"));
            ui.ctx().request_repaint();
            return;
        }
        let lib = ed.pause_tiles();
        for (path, why) in &lib.failed {
            log::warn!("{path}: {why}");
        }
        let mut r = gpu.egui.write();
        let mut map = MapGpu::new(&gpu.ctx.device, gpu.format);
        map.upload(&gpu.ctx.device, &gpu.ctx.queue, &lib);
        for p in &map.problems {
            tools.fail(p.clone());
        }
        r.callback_resources.insert(map);
    }
    if pm.label_font.is_empty() {
        pm.label_font =
            if ed.pause_assets.fonts.get("RDR Lino").is_some() { "RDR Lino".into() } else { DEFAULT_FONT.into() };
    }
    toolbar(ui, pm, ed, tools);
    status_bar(ui, pm, ed);
    canvas(ui, pm, ed, gpu, tools);
}

fn toolbar(ui: &mut egui::Ui, pm: &mut PauseMap, ed: &mut Editor, tools: &mut Tools) {
    let frame = egui::Frame::new().inner_margin(egui::Margin { left: 0, right: 0, top: 0, bottom: 6 });
    egui::Panel::top(egui::Id::new("pause-map-bar")).frame(frame).show_separator_line(false).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            theme::group(ui, |ui| {
                for (tool, glyph, tip) in [
                    (
                        Tool::Select,
                        icon::CURSOR,
                        "Select (V): click, drag to move, Shift to add; the handles resize and turn",
                    ),
                    (
                        Tool::Direct,
                        icon::CURSOR_CLICK,
                        "Direct select (A): a line's anchors and handles; Alt+click adds one",
                    ),
                    (Tool::Pen, icon::PEN_NIB, "Pen (P): click for corners, drag for curves; Enter ends"),
                    (Tool::Text, icon::TEXT_T, "Label (T): click to place one"),
                    (Tool::Building, icon::HOUSE_LINE, "Building (B): click, or drag its size"),
                    (Tool::Erase, icon::ERASER, "Eraser (E): takes the game's own marks away ([ and ] size it)"),
                    (Tool::Hand, icon::HAND, "Hand (H, or hold Space): drag the map"),
                ] {
                    if theme::tool(ui, pm.tool == tool, glyph, tip).clicked() {
                        set_tool(pm, ed, tool);
                    }
                }
                if theme::tool(ui, false, icon::IMAGE, "Picture stamp (I): a picture's dark pixels become ink")
                    .clicked()
                {
                    pick_stamp(pm, ed, tools);
                }
            });
            theme::group(ui, |ui| {
                for (i, (glyph, name)) in [
                    (icon::TEXT_AA, "Labels"),
                    (icon::PATH, "Roads and lines"),
                    (icon::WAVES, "Water"),
                    (icon::MOUNTAINS, "Relief and buildings"),
                ]
                .into_iter()
                .enumerate()
                {
                    let tip = format!("{name}: {}", if pm.show[i] { "shown" } else { "hidden" });
                    if theme::tool(ui, pm.show[i], glyph, &tip).clicked() {
                        pm.show[i] = !pm.show[i];
                    }
                }
                if theme::tool(ui, pm.grid, icon::GRID_FOUR, "The tiles' seams").clicked() {
                    pm.grid = !pm.grid;
                }
            });
            theme::group(ui, |ui| {
                if theme::tool(ui, false, icon::CORNERS_OUT, "The whole map (0)").clicked()
                    && let Some(r) = pm.view.rect
                {
                    pm.fit(r);
                }
                let world = pm.world_camera.map(|(p, _)| p);
                let b = ui.add_enabled_ui(world.is_some(), |ui| {
                    theme::tool(ui, false, icon::CROSSHAIR, "Where the world map's camera is")
                });
                if b.inner.clicked()
                    && let (Some(p), Some(r)) = (world, pm.view.rect)
                {
                    let m = world_to_map(p[0], p[1]);
                    pm.frame(
                        r,
                        Bounds { x0: m[0] - 60.0, y0: m[1] - 60.0, x1: m[0] + 60.0, y1: m[1] + 60.0 },
                        1.0,
                        64.0,
                    );
                }
            });
            theme::group(ui, |ui| {
                if theme::tool(ui, false, icon::DOWNLOAD_SIMPLE, "Import a Map Editor or Aether Cartography project")
                    .clicked()
                {
                    import(pm, ed, tools);
                }
                if theme::tool(
                    ui,
                    false,
                    icon::UPLOAD_SIMPLE,
                    "Save the drawing as a Map Editor project (map-project.json)",
                )
                .clicked()
                {
                    export(ed, tools);
                }
            });
        });
    });
}

pub fn set_tool(pm: &mut PauseMap, ed: &mut Editor, tool: Tool) {
    if pm.tool == Tool::Pen && tool != Tool::Pen {
        finish_pen(pm, ed);
    }
    pm.tool = tool;
}

fn import(pm: &mut PauseMap, ed: &mut Editor, tools: &mut Tools) {
    let Some(path) =
        rfd::FileDialog::new().add_filter("Map projects", &["json"]).set_title("Import a map project").pick_file()
    else {
        return;
    };
    let before = ed.pause_map().objects.len();
    match ed.import_map_project(&path) {
        Ok(notes) => {
            let n = ed.pause_map().objects.len() - before;
            let mut text = format!("Imported {}", count(n, "layer"));
            for note in notes {
                text.push_str(". ");
                text.push_str(&note);
            }
            tools.say(text);
            pm.focus_on = None;
            pm.select(Vec::new());
        }
        Err(e) => tools.fail(e),
    }
}

fn export(ed: &Editor, tools: &mut Tools) {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("Map Editor project", &["json"])
        .set_file_name("map-project.json")
        .set_title("Save the drawing as a Map Editor project")
        .save_file()
    else {
        return;
    };
    match ed.export_map_project(&path) {
        Ok(()) => tools.say(format!("Saved {}", path.display())),
        Err(e) => tools.fail(e),
    }
}

fn pick_stamp(pm: &mut PauseMap, ed: &mut Editor, tools: &mut Tools) {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("Pictures", &["png", "jpg", "jpeg", "webp", "bmp", "gif"])
        .set_title("A picture to stamp on the map")
        .pick_file()
    else {
        return;
    };
    let added = std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| PauseMap::add_picture(ed, b));
    match added {
        Ok((asset, aspect)) => {
            let centre = pm
                .view
                .rect
                .map(|r| View { rect: r, cam: pm.camera }.map(r.center()))
                .unwrap_or([WORLD_W / 2.0, WORLD_H / 2.0]);
            let id = new_id();
            let mut objects = pm.objects(ed).to_vec();
            objects.push(MapObject::Stamp(StampObject {
                common: Common { id: id.clone(), ..Default::default() },
                asset,
                aspect,
                target: Layer::Text,
                x: centre[0],
                y: centre[1],
                width: 80.0,
                rotation: 0.0,
                threshold: 0.5,
                invert: false,
            }));
            pm.edit(ed, "Add picture stamp", objects);
            pm.select(vec![id]);
            pm.tool = Tool::Select;
        }
        Err(_) => tools.fail("That file couldn't be opened as a picture: use PNG, JPEG or WebP"),
    }
}

fn status_bar(ui: &mut egui::Ui, pm: &PauseMap, ed: &Editor) {
    let frame = egui::Frame::new().inner_margin(egui::Margin { left: 4, right: 4, top: 4, bottom: 0 });
    egui::Panel::bottom(egui::Id::new("pause-map-status")).frame(frame).show_separator_line(false).show(ui, |ui| {
        ui.horizontal(|ui| {
            let weak = |t: String| egui::RichText::new(t).color(theme::WEAK);
            match pm.view.cursor {
                Some(p) => {
                    let w = map_to_world(p);
                    ui.label(weak(format!("map {}", super::point_text(p))));
                    ui.label(weak(format!("world {:.0}, {:.0}", w[0], w[1])));
                    if let Some((row, col)) = tile_at(detail(Layer::Text), p[0], p[1]) {
                        ui.label(weak(format!("tile {row}_{col}")));
                    }
                }
                None => {
                    ui.label(weak(
                        "Scroll to zoom, drag with the middle button or Space to pan, 0 for the whole map".into(),
                    ));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(weak(format!("{:.0}%", pm.camera.zoom * 100.0)));
                let n = pm.objects(ed).iter().filter(|o| !o.is_group()).count();
                ui.label(weak(count(n, "layer")));
                if !pm.selection.is_empty() {
                    ui.label(weak(format!("{} selected", pm.selection.len())));
                }
            });
        });
    });
}

fn canvas(ui: &mut egui::Ui, pm: &mut PauseMap, ed: &mut Editor, gpu: &Gpu, tools: &mut Tools) {
    let rect = ui.available_rect_before_wrap();
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    if !pm.view.fitted || pm.view.rect.is_none() {
        pm.fit(rect);
        pm.view.fitted = true;
    }
    pm.view.rect = Some(rect);
    if let Some([x0, y0, x1, y1]) = pm.frame_on.take() {
        pm.frame(rect, Bounds { x0, y0, x1, y1 }, 1.0, 64.0);
    }

    let pointer = ui.input(|i| i.pointer.hover_pos());
    let hovered = response.hovered() || pm.view.gesture.is_some();
    if ui.input(|i| i.pointer.any_pressed()) && response.hovered() {
        pm.active = true;
    }
    // Zoom at the pointer.
    if response.hovered() {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        if let Some(at) = pointer {
            if scroll != 0.0 {
                pm.zoom_at((scroll as f64 * 0.0025).exp(), at, rect);
            }
            if pinch != 1.0 {
                pm.zoom_at(pinch as f64, at, rect);
            }
        }
    }
    if let Some(id) = pm.focus_on.take()
        && let Some(b) = selection_bounds(std::slice::from_ref(&id), pm.objects(ed), &ed.pause_assets)
    {
        let v = View { rect, cam: pm.camera };
        let (a, z) = (v.map(rect.min), v.map(rect.max));
        if b.x0 < a[0] || b.y0 < a[1] || b.x1 > z[0] || b.y1 > z[1] {
            pm.frame(rect, b, 0.7, pm.camera.zoom.max(1.0));
        }
    }

    if hovered && !ui.ctx().text_edit_focused() {
        keys(ui, pm, ed, tools, rect);
    }
    pointer_input(ui, &response, pm, ed, rect);
    context_menu(&response, pm, ed, tools);

    // The map, then what's drawn changed this frame, then the chrome.
    let objects = pm.objects(ed).to_vec();
    {
        let mut r = gpu.egui.write();
        if let Some(map) = r.callback_resources.get_mut::<MapGpu>() {
            pm.sync.sync(map, &gpu.ctx.queue, ed, &objects);
        }
    }
    let theme = Theme { backdrop: rgb(theme::BG), grid: rgb(theme::BORDER), seam: rgb(theme::ACCENT) };
    let paint = Paint { camera: pm.camera, show: pm.show, grid: pm.grid, theme, origin: rect.min };
    ui.painter_at(rect).add(gpu::callback(rect, paint));
    overlay(ui, pm, ed, rect, &objects);
    if pm.view.gesture.is_some() {
        ui.ctx().request_repaint();
    }
}

fn rgb(c: Color32) -> [f32; 3] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0]
}

/// The pen's line ends: one too short goes.
fn finish_pen(pm: &mut PauseMap, ed: &mut Editor) {
    let Some(id) = pm.view.pen.take() else { return };
    pm.view.closing = false;
    let objects = pm.objects(ed);
    if let Some(MapObject::Line(l)) = objects.iter().find(|o| o.id() == id)
        && l.nodes.len() < 2
    {
        let left: Vec<MapObject> = objects.iter().filter(|o| o.id() != id).cloned().collect();
        pm.edit(ed, "Draw line", left);
        pm.select(Vec::new());
    }
}

fn pointer_input(ui: &egui::Ui, response: &egui::Response, pm: &mut PauseMap, ed: &mut Editor, rect: Rect) {
    let v = View { rect, cam: pm.camera };
    let (pos, pressed, middle, released, double, mods, space) = ui.input(|i| {
        (
            i.pointer.interact_pos().or(i.pointer.hover_pos()),
            i.pointer.primary_pressed(),
            i.pointer.button_pressed(egui::PointerButton::Middle),
            i.pointer.any_released(),
            i.pointer.button_double_clicked(egui::PointerButton::Primary),
            i.modifiers,
            i.key_down(Key::Space),
        )
    });
    let Some(pos) = pos else { return };
    let p = v.map(pos);
    pm.view.cursor = rect.contains(pos).then_some(p);
    let assets_slop = 6.0 / pm.camera.zoom;

    // Down. A field's change still being typed is one step of its own first.
    if (pressed || middle) && response.hovered() && pm.view.gesture.is_none() && pm.draft.is_some() {
        pm.commit(ed, "Edit");
    }
    if (pressed || middle) && response.hovered() {
        if middle || pm.tool == Tool::Hand || space {
            pm.view.gesture = Some(Gesture::Pan { last: pos });
        } else {
            match pm.tool {
                Tool::Text => pm.view.gesture = Some(Gesture::Click { at: pos }),
                Tool::Pen => pen_down(pm, ed, p, &v),
                Tool::Building => {
                    let id = new_id();
                    let mut objects = pm.objects(ed).to_vec();
                    objects.push(MapObject::Building(BuildingObject {
                        common: Common { id: id.clone(), ..Default::default() },
                        shape: pm.building_shape,
                        x: p[0],
                        y: p[1],
                        w: BLOCK.0,
                        h: BLOCK.1,
                        rotation: 0.0,
                        outline: 0.8,
                        dot: None,
                    }));
                    pm.draft = Some(objects);
                    pm.select(vec![id.clone()]);
                    pm.view.gesture = Some(Gesture::Build { id, from: p, at: pos });
                }
                Tool::Erase => {
                    let id = new_id();
                    let mut objects = pm.objects(ed).to_vec();
                    objects.push(MapObject::Erase(EraseObject {
                        common: Common { id: id.clone(), ..Default::default() },
                        target: pm.erase_target,
                        radius: pm.erase_radius,
                        points: vec![p],
                    }));
                    pm.draft = Some(objects);
                    pm.view.gesture = Some(Gesture::Erase { id });
                }
                Tool::Select | Tool::Direct => select_down(pm, ed, p, &v, mods),
                Tool::Hand => {}
            }
        }
    }

    // Moves.
    let shift = mods.shift;
    if let Some(g) = &mut pm.view.gesture {
        if let Gesture::Click { at } = g
            && at.distance(pos) > CLICK_SLOP
        {
            *g = Gesture::Pan { last: *at };
        }
        match g {
            Gesture::Pan { last } => {
                let d = pos - *last;
                *last = pos;
                pm.camera.x -= d.x as f64 / pm.camera.zoom;
                pm.camera.y -= d.y as f64 / pm.camera.zoom;
            }
            Gesture::Marquee { to, .. } => *to = p,
            Gesture::Move { ids, last } => {
                let [mut dx, mut dy] = sub(p, *last);
                if shift {
                    if dx.abs() > dy.abs() {
                        dy = 0.0;
                    } else {
                        dx = 0.0;
                    }
                }
                *last = [last[0] + dx, last[1] + dy];
                if let Some(d) = &mut pm.draft {
                    for o in d.iter_mut().filter(|o| ids.contains(o.id())) {
                        translate(o, dx, dy);
                    }
                }
            }
            Gesture::Nodes { id, last } => {
                let [dx, dy] = sub(p, *last);
                *last = p;
                if let Some(l) = pm.draft.as_mut().and_then(|d| line_mut(d, id)) {
                    for &i in &pm.nodes {
                        if let Some(n) = l.nodes.get_mut(i) {
                            n.p = [n.p[0] + dx, n.p[1] + dy];
                        }
                    }
                }
            }
            Gesture::Handle { id, i, out } => {
                if let Some(n) = pm.draft.as_mut().and_then(|d| line_mut(d, id)).and_then(|l| l.nodes.get_mut(*i)) {
                    let mut h = sub(p, n.p);
                    if shift {
                        h = constrain(h);
                    }
                    let other = if *out { n.inn } else { n.out };
                    if *out {
                        n.out = Some(h);
                    } else {
                        n.inn = Some(h);
                    }
                    // A smooth anchor keeps its handles in line (each its own
                    // length); Alt breaks it.
                    if let Some(o2) = other
                        && !mods.alt
                    {
                        let (len, hl) = (o2[0].hypot(o2[1]), h[0].hypot(h[1]).max(1e-9));
                        let opposite = Some([-h[0] / hl * len, -h[1] / hl * len]);
                        if *out {
                            n.inn = opposite;
                        } else {
                            n.out = opposite;
                        }
                    }
                }
            }
            Gesture::Rotate { id, center, start, rot0 } => {
                let mut rot = *rot0 + ((p[1] - center[1]).atan2(p[0] - center[0]) - *start).to_degrees();
                if shift {
                    rot = (rot / 15.0).round() * 15.0;
                }
                rot = (rot + 180.0).rem_euclid(360.0) - 180.0;
                let rot = (rot * 10.0).round() / 10.0;
                if let Some(o) = pm.draft.as_mut().and_then(|d| d.iter_mut().find(|o| o.id() == id)) {
                    match o {
                        MapObject::Label(l) => l.rotation = rot,
                        MapObject::Building(b) => b.rotation = rot,
                        MapObject::Stamp(s) => s.rotation = rot,
                        _ => {}
                    }
                }
            }
            Gesture::Scale { id, center, d0, o0 } => {
                let f = ((p[0] - center[0]).hypot(p[1] - center[1]) / d0.max(1e-6)).max(0.05);
                if let Some(o) = pm.draft.as_mut().and_then(|d| d.iter_mut().find(|o| o.id() == id)) {
                    match (o, &*o0) {
                        (MapObject::Label(l), MapObject::Label(l0)) => {
                            l.size = ((l0.size * f * 10.0).round() / 10.0).max(4.0)
                        }
                        (MapObject::Stamp(s), MapObject::Stamp(s0)) => s.width = (s0.width * f).max(4.0),
                        (MapObject::Building(b), MapObject::Building(b0)) if b0.shape == BuildingShape::Station => {
                            // A station scales whole: ring and dot together.
                            let d = (b0.w.min(b0.h) * f).max(2.0);
                            (b.w, b.h) = (d, d);
                            b.dot = b0.dot.map(|x| x * f);
                        }
                        (MapObject::Building(b), MapObject::Building(b0)) => {
                            if shift {
                                (b.w, b.h) = ((b0.w * f).max(2.0), (b0.h * f).max(2.0));
                            } else {
                                // Free, from the centre, in the building's turned frame.
                                let (sin, cos) = b0.rotation.to_radians().sin_cos();
                                let (dx, dy) = (p[0] - b0.x, p[1] - b0.y);
                                b.w = (2.0 * (dx * cos + dy * sin).abs()).max(2.0);
                                b.h = (2.0 * (-dx * sin + dy * cos).abs()).max(2.0);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Gesture::FScale { handle, b, originals } => {
                let (cx, cy) = ((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
                let ax = if handle.west() {
                    b.x1
                } else if handle.east() {
                    b.x0
                } else {
                    cx
                };
                let ay = if handle.north() {
                    b.y1
                } else if handle.south() {
                    b.y0
                } else {
                    cy
                };
                // From the other side (the centre with Alt); Shift keeps proportions.
                let pivot = if mods.alt { [cx, cy] } else { [ax, ay] };
                let (gx, gy) = (if handle.west() { b.x0 } else { b.x1 }, if handle.north() { b.y0 } else { b.y1 });
                let fx = |v: f64, grab: f64, piv: f64| {
                    let d = grab - piv;
                    ((v - piv) / if d == 0.0 { 1e-6 } else { d }).max(0.02)
                };
                let mut sx = if handle.east() || handle.west() { fx(p[0], gx, pivot[0]) } else { 1.0 };
                let mut sy = if handle.north() || handle.south() { fx(p[1], gy, pivot[1]) } else { 1.0 };
                if shift {
                    let s = if handle.corner() {
                        sx.max(sy)
                    } else if matches!(handle, FrameHandle::N | FrameHandle::S) {
                        sy
                    } else {
                        sx
                    };
                    (sx, sy) = (s, s);
                }
                let m = Affine::scale_about(pivot, sx, sy);
                transform_all(pm.draft.as_mut(), originals, &m);
            }
            Gesture::FRotate { center, start, originals } => {
                let mut a = (p[1] - center[1]).atan2(p[0] - center[0]) - *start;
                if shift {
                    let step = std::f64::consts::PI / 12.0;
                    a = (a / step).round() * step;
                }
                let m = Affine::rotate_about(*center, a);
                transform_all(pm.draft.as_mut(), originals, &m);
            }
            Gesture::Pen { id, i } => {
                if let Some(n) = pm.draft.as_mut().and_then(|d| line_mut(d, id)).and_then(|l| l.nodes.get_mut(*i)) {
                    let mut h = sub(p, n.p);
                    if shift {
                        h = constrain(h);
                    }
                    if (h[0].hypot(h[1]) * pm.camera.zoom) < 3.0 {
                        n.out = None;
                        if !mods.alt {
                            n.inn = None;
                        }
                    } else {
                        n.out = Some(h);
                        if !mods.alt {
                            n.inn = Some([-h[0], -h[1]]);
                        }
                    }
                }
            }
            Gesture::Erase { id } => {
                if let Some(MapObject::Erase(e)) = pm.draft.as_mut().and_then(|d| d.iter_mut().find(|o| o.id() == id)) {
                    let last = *e.points.last().unwrap();
                    if (p[0] - last[0]).hypot(p[1] - last[1]) >= e.radius * 0.25 {
                        e.points.push(p);
                    }
                }
            }
            Gesture::Build { id, from, at } => {
                if at.distance(pos) > CLICK_SLOP
                    && let Some(MapObject::Building(b)) =
                        pm.draft.as_mut().and_then(|d| d.iter_mut().find(|o| o.id() == id))
                {
                    b.x = (from[0] + p[0]) / 2.0;
                    b.y = (from[1] + p[1]) / 2.0;
                    b.w = (p[0] - from[0]).abs().max(2.0);
                    b.h = (p[1] - from[1]).abs().max(2.0);
                }
            }
            Gesture::Click { .. } => {}
        }
    } else if response.hovered() {
        update_hover(pm, ed, p, &v, assets_slop);
    } else {
        pm.view.hover = None;
    }

    // Up.
    if released && let Some(g) = pm.view.gesture.take() {
        match g {
            Gesture::Click { .. } if pm.tool == Tool::Text => place_label(pm, ed, p, assets_slop),
            Gesture::Marquee { from, to, base, nodes } => finish_marquee(pm, ed, rect_of(from, to), base, nodes),
            Gesture::Pan { .. } | Gesture::Click { .. } => {}
            other => {
                let label = other.label();
                pm.commit(ed, label);
            }
        }
    }

    // Double-click: into a group a level at a time; a label's text, a
    // line's points.
    if double && response.hovered() && matches!(pm.tool, Tool::Select | Tool::Direct) {
        let objects = pm.objects(ed).to_vec();
        if let Some(PointHit::Anchor { id, i }) = point_at(pm, &objects, p, &v) {
            let left = commands::delete_nodes(&objects, &id, &[i]);
            pm.edit(ed, "Delete anchor", left);
            pm.nodes.clear();
            return;
        }
        let Some(hit) = drawable_at(&objects, p, assets_slop, false, ed) else { return };
        let index = index_of(&objects);
        let target = drill_target(hit, &index, &pm.selection).to_owned();
        if target != hit.id() {
            pm.select(vec![target]);
        } else {
            let id = hit.id().to_owned();
            match hit {
                MapObject::Label(_) => {
                    pm.select(vec![id.clone()]);
                    pm.focus_text = Some(id);
                }
                MapObject::Line(_) => {
                    pm.select(vec![id]);
                    pm.tool = Tool::Direct;
                }
                _ => pm.select(vec![id]),
            }
        }
    }
}

fn transform_all(draft: Option<&mut Vec<MapObject>>, originals: &HashMap<String, MapObject>, m: &Affine) {
    let Some(d) = draft else { return };
    for o in d.iter_mut() {
        if let Some(orig) = originals.get(o.id()) {
            *o = transform_object(orig, m);
        }
    }
}

fn update_hover(pm: &mut PauseMap, ed: &Editor, p: Point, v: &View, slop: f64) {
    pm.view.hover = None;
    pm.view.closing = false;
    let objects = pm.objects(ed);
    let mut cursor = None;
    match pm.tool {
        Tool::Pen => {
            if let Some(MapObject::Line(l)) = pm.view.pen.as_ref().and_then(|id| objects.iter().find(|o| o.id() == id))
            {
                pm.view.closing = l.nodes.len() >= 2 && v.near(l.nodes[0].p, p, ANCHOR + 3.0);
            }
            cursor = Some(CursorIcon::Crosshair);
        }
        Tool::Select | Tool::Direct | Tool::Text => {
            let sp = v.screen(p);
            if let Some(t) = pm.transform_target(objects) {
                let (corners, _, knob) = transform_handles(t, v, ed);
                if sp.distance(knob) <= 8.0 {
                    cursor = Some(CursorIcon::Grab);
                } else if corners.iter().any(|c| sp.distance(*c) <= 7.0) {
                    cursor = Some(CursorIcon::ResizeNwSe);
                }
            }
            if cursor.is_none()
                && pm.tool == Tool::Select
                && let Some(b) = pm.frame_target(objects, ed)
            {
                let (_, handles, _, knob) = frame_handles(&b, v);
                if sp.distance(knob) <= 8.0 {
                    cursor = Some(CursorIcon::Grab);
                } else if let Some((h, _)) = handles.iter().find(|(_, at)| sp.distance(*at) <= 7.0) {
                    cursor = Some(match h {
                        FrameHandle::Nw | FrameHandle::Se => CursorIcon::ResizeNwSe,
                        FrameHandle::Ne | FrameHandle::Sw => CursorIcon::ResizeNeSw,
                        FrameHandle::N | FrameHandle::S => CursorIcon::ResizeVertical,
                        _ => CursorIcon::ResizeHorizontal,
                    });
                }
            }
            if cursor.is_none() && pm.tool != Tool::Text && point_at(pm, objects, p, v).is_some() {
                cursor = Some(CursorIcon::PointingHand);
            }
            if cursor.is_none()
                && let Some(hit) = drawable_at(objects, p, slop, pm.tool == Tool::Text, ed)
            {
                let index = index_of(objects);
                pm.view.hover = Some(if pm.tool == Tool::Select {
                    pick_target(hit, &index, &pm.selection).to_owned()
                } else {
                    hit.id().to_owned()
                });
                if pm.tool != Tool::Text {
                    cursor = Some(CursorIcon::Move);
                }
            }
            if pm.tool == Tool::Text && cursor.is_none() {
                cursor = Some(CursorIcon::Text);
            }
        }
        Tool::Hand => cursor = Some(CursorIcon::Grab),
        Tool::Erase | Tool::Building => cursor = Some(CursorIcon::Crosshair),
    }
    if let Some(c) = cursor {
        // Set by the canvas each frame it's hovered.
        PENDING_CURSOR.with(|p| p.set(Some(c)));
    }
}

thread_local! {
    static PENDING_CURSOR: std::cell::Cell<Option<CursorIcon>> = const { std::cell::Cell::new(None) };
}

fn pen_down(pm: &mut PauseMap, ed: &mut Editor, p: Point, v: &View) {
    let objects = pm.objects(ed).to_vec();
    let active = pm.view.pen.as_ref().and_then(|id| objects.iter().find(|o| o.id() == id)).cloned();
    if let Some(MapObject::Line(l)) = active {
        if l.nodes.len() >= 2 && v.near(l.nodes[0].p, p, ANCHOR + 3.0) {
            let mut d = objects;
            if let Some(line) = line_mut(&mut d, &l.common.id) {
                line.closed = true;
            }
            pm.edit(ed, "Close line", d);
            finish_pen(pm, ed);
            return;
        }
        let mut d = objects;
        let i = l.nodes.len();
        if let Some(line) = line_mut(&mut d, &l.common.id) {
            line.nodes.push(PathNode::corner(p));
        }
        pm.draft = Some(d);
        pm.view.gesture = Some(Gesture::Pen { id: l.common.id.clone(), i });
        return;
    }
    // Clicking an end of the selected open line draws on from there.
    if let ([sel], Some(MapObject::Line(l))) =
        (pm.selection.as_slice(), pm.primary().and_then(|id| objects.iter().find(|o| o.id() == id)))
        && !l.closed
        && !l.nodes.is_empty()
    {
        let (at_start, at_end) =
            (v.near(l.nodes[0].p, p, ANCHOR + 3.0), v.near(l.nodes.last().unwrap().p, p, ANCHOR + 3.0));
        if at_start || at_end {
            let id = sel.clone();
            if at_start && !at_end {
                let reversed = commands::reverse_line(&objects, &id);
                pm.edit(ed, "Reverse line", reversed);
            }
            pm.view.pen = Some(id);
            return;
        }
    }
    let info = style_info(pm.line_style);
    let id = new_id();
    let mut d = objects;
    d.push(MapObject::Line(LineObject {
        common: Common { id: id.clone(), ..Default::default() },
        style: pm.line_style,
        width: info.width,
        spacing: info.spacing,
        closed: false,
        nodes: vec![PathNode::corner(p)],
    }));
    pm.draft = Some(d);
    pm.view.pen = Some(id.clone());
    pm.select(vec![id.clone()]);
    pm.view.gesture = Some(Gesture::Pen { id, i: 0 });
}

fn select_down(pm: &mut PauseMap, ed: &mut Editor, p: Point, v: &View, mods: egui::Modifiers) {
    let objects = pm.objects(ed).to_vec();
    let index = index_of(&objects);
    let sp = v.screen(p);
    let slop = 6.0 / pm.camera.zoom;
    // A single boxed object's handles.
    if let Some(t) = pm.transform_target(&objects) {
        let (corners, _, knob) = transform_handles(t, v, ed);
        let (x, y, rot) = t.boxed().unwrap();
        if sp.distance(knob) <= 8.0 {
            let start = (p[1] - y).atan2(p[0] - x);
            pm.draft = Some(objects.clone());
            pm.view.gesture = Some(Gesture::Rotate { id: t.id().to_owned(), center: [x, y], start, rot0: rot });
            return;
        }
        if corners.iter().any(|c| sp.distance(*c) <= 7.0) {
            let d0 = (p[0] - x).hypot(p[1] - y);
            pm.draft = Some(objects.clone());
            pm.view.gesture = Some(Gesture::Scale { id: t.id().to_owned(), center: [x, y], d0, o0: t.clone() });
            return;
        }
    }
    // The frame around lines, groups and several layers.
    if let Some(b) = pm.frame_target(&objects, ed) {
        let (_, handles, _, knob) = frame_handles(&b, v);
        let ids = with_descendants(pm.selection.iter().map(String::as_str), &objects);
        let originals: HashMap<String, MapObject> =
            objects.iter().filter(|o| ids.contains(o.id())).map(|o| (o.id().to_owned(), o.clone())).collect();
        if sp.distance(knob) <= 8.0 {
            let center = [(b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0];
            let start = (p[1] - center[1]).atan2(p[0] - center[0]);
            pm.draft = Some(objects.clone());
            pm.view.gesture = Some(Gesture::FRotate { center, start, originals });
            return;
        }
        if let Some((h, _)) = handles.iter().find(|(_, at)| sp.distance(*at) <= 7.0) {
            pm.draft = Some(objects.clone());
            pm.view.gesture = Some(Gesture::FScale { handle: *h, b, originals });
            return;
        }
    }
    // Anchors and handles.
    match point_at(pm, &objects, p, v) {
        Some(PointHit::Handle { id, i, out }) => {
            pm.draft = Some(objects);
            pm.view.gesture = Some(Gesture::Handle { id, i, out });
            return;
        }
        Some(PointHit::Anchor { id, i }) => {
            if mods.alt {
                let curved = matches!(objects.iter().find(|o| o.id() == id), Some(MapObject::Line(l)) if l.nodes[i].inn.is_some() || l.nodes[i].out.is_some());
                let changed = commands::convert_nodes(&objects, &id, &[i], !curved);
                pm.edit(ed, if curved { "Make corner" } else { "Make smooth" }, changed);
                pm.selection = vec![id];
                pm.nodes = vec![i];
                return;
            }
            let current: Vec<usize> = if pm.primary() == Some(&id) { pm.nodes.clone() } else { Vec::new() };
            let nodes = if mods.shift {
                if current.contains(&i) {
                    current.into_iter().filter(|&k| k != i).collect()
                } else {
                    [current, vec![i]].concat()
                }
            } else if current.contains(&i) {
                current
            } else {
                vec![i]
            };
            pm.selection = vec![id.clone()];
            pm.nodes = nodes;
            pm.draft = Some(objects);
            pm.view.gesture = Some(Gesture::Nodes { id, last: p });
            return;
        }
        None => {}
    }
    let hit = drawable_at(&objects, p, slop, false, ed).cloned();
    // Alt+click on a line whose points show adds an anchor, the curve the same.
    if let Some(MapObject::Line(l)) = &hit
        && mods.alt
        && pm.edit_lines(&objects).contains(&l.common.id)
    {
        let at = nearest_on_path(&l.nodes, l.closed, p);
        let mut d = objects.clone();
        if let Some(line) = line_mut(&mut d, &l.common.id) {
            line.nodes = insert_node(&l.nodes, l.closed, at.i, at.t);
        }
        pm.edit(ed, "Add anchor", d.clone());
        pm.selection = vec![l.common.id.clone()];
        pm.nodes = vec![at.i + 1];
        pm.draft = Some(d);
        pm.view.gesture = Some(Gesture::Nodes { id: l.common.id.clone(), last: p });
        return;
    }
    if let Some(hit) = hit {
        // Ctrl+click (as the Direct tool) reaches a single layer in any group.
        let target = if pm.tool == Tool::Direct || mods.command {
            hit.id().to_owned()
        } else {
            pick_target(&hit, &index, &pm.selection).to_owned()
        };
        if mods.shift {
            if pm.selection.contains(&target) {
                pm.selection.retain(|s| *s != target);
            } else {
                pm.selection.push(target);
            }
            pm.nodes.clear();
            return;
        }
        if !pm.selection.contains(&target) {
            pm.select(vec![target]);
        } else if pm.tool == Tool::Direct && matches!(hit, MapObject::Line(_)) {
            pm.nodes.clear();
        }
        let mut d = objects.clone();
        // Alt+drag leaves the originals and moves copies.
        if mods.alt && pm.tool == Tool::Select {
            let (copied, selected) = commands::duplicate(&d, &pm.selection, 0.0, 0.0);
            d = copied;
            pm.select(selected);
        }
        let ids = with_descendants(pm.selection.iter().map(String::as_str), &d);
        pm.draft = Some(d);
        pm.view.gesture = Some(Gesture::Move { ids, last: p });
        return;
    }
    // Nothing there: a box selects.
    let base = if mods.shift { pm.selection.clone() } else { Vec::new() };
    pm.view.gesture = Some(Gesture::Marquee { from: p, to: p, base, nodes: pm.tool == Tool::Direct });
    if !mods.shift {
        pm.select(Vec::new());
    }
}

fn finish_marquee(pm: &mut PauseMap, ed: &Editor, r: Bounds, base: Vec<String>, nodes: bool) {
    let objects = pm.objects(ed);
    let index = index_of(objects);
    if nodes {
        // Anchors in the box; the last line with some is the one edited.
        let mut picked: Vec<(String, Vec<usize>)> = Vec::new();
        for o in objects {
            let MapObject::Line(l) = o else { continue };
            if is_hidden(o, &index) || is_locked(o, &index) {
                continue;
            }
            let inside: Vec<usize> = (0..l.nodes.len())
                .filter(|&i| {
                    let q = l.nodes[i].p;
                    q[0] >= r.x0 && q[0] <= r.x1 && q[1] >= r.y0 && q[1] <= r.y1
                })
                .collect();
            if !inside.is_empty() {
                picked.push((o.id().to_owned(), inside));
            }
        }
        if let Some((_, last_nodes)) = picked.last().cloned() {
            let mut sel = base;
            for (id, _) in &picked {
                if !sel.contains(id) {
                    sel.push(id.clone());
                }
            }
            sel.retain(|s| s != &picked.last().unwrap().0);
            sel.push(picked.last().unwrap().0.clone());
            pm.selection = sel;
            pm.nodes = last_nodes;
        }
        return;
    }
    let mut sel = base.clone();
    for o in objects {
        if o.is_group() || is_hidden(o, &index) || is_locked(o, &index) || !touches_rect(o, &r, &ed.pause_assets) {
            continue;
        }
        let t = pick_target(o, &index, &base).to_owned();
        if !sel.contains(&t) {
            sel.push(t);
        }
    }
    pm.select(sel);
}

fn place_label(pm: &mut PauseMap, ed: &mut Editor, p: Point, slop: f64) {
    let objects = pm.objects(ed).to_vec();
    if let Some(hit) = drawable_at(&objects, p, slop, true, ed) {
        pm.select(vec![hit.id().to_owned()]);
        return;
    }
    let preset = &LABEL_PRESETS[pm.label_preset.min(LABEL_PRESETS.len() - 1)];
    let id = new_id();
    let mut d = objects;
    d.push(MapObject::Label(LabelObject {
        common: Common { id: id.clone(), ..Default::default() },
        text: "New place".into(),
        font: pm.label_font.clone(),
        size: preset.size,
        x: p[0],
        y: p[1],
        rotation: 0.0,
        tracking: preset.tracking,
        curve: 0.0,
        weight: preset.weight,
        caps: preset.caps,
    }));
    pm.edit(ed, "Add label", d);
    pm.select(vec![id.clone()]);
    pm.focus_text = Some(id);
}

fn keys(ui: &egui::Ui, pm: &mut PauseMap, ed: &mut Editor, tools: &mut Tools, rect: Rect) {
    let (mods, pressed): (egui::Modifiers, Vec<Key>) = ui.input(|i| {
        let keys = i
            .events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { key, pressed: true, .. } => Some(*key),
                _ => None,
            })
            .collect();
        (i.modifiers, keys)
    });
    for key in pressed {
        if mods.command {
            continue; // Ctrl's keys: edit_keys.
        }
        let pen = pm.view.pen.is_some();
        match key {
            Key::Enter | Key::Escape if pen => finish_pen(pm, ed),
            Key::Backspace if pen => {
                let id = pm.view.pen.clone().unwrap();
                let mut d = pm.objects(ed).to_vec();
                let mut empty = false;
                if let Some(l) = line_mut(&mut d, &id) {
                    l.nodes.pop();
                    empty = l.nodes.is_empty();
                }
                if empty {
                    d.retain(|o| o.id() != id);
                }
                pm.edit(ed, "Draw line", d);
                if empty {
                    pm.view.pen = None;
                }
            }
            Key::V => set_tool(pm, ed, Tool::Select),
            Key::A => set_tool(pm, ed, Tool::Direct),
            Key::P | Key::L => set_tool(pm, ed, Tool::Pen),
            Key::E => set_tool(pm, ed, Tool::Erase),
            Key::T => set_tool(pm, ed, Tool::Text),
            Key::B => set_tool(pm, ed, Tool::Building),
            Key::H => set_tool(pm, ed, Tool::Hand),
            Key::I => pick_stamp(pm, ed, tools),
            Key::OpenBracket | Key::CloseBracket if pm.tool == Tool::Erase => {
                let f = if key == Key::CloseBracket { 1.25 } else { 0.8 };
                pm.erase_radius = (pm.erase_radius * f).clamp(2.0, 80.0);
            }
            Key::Escape => {
                let objects = pm.objects(ed);
                let parent =
                    (pm.selection.len() == 1).then(|| parent_of(&pm.selection[0], &index_of(objects))).flatten();
                if !pm.nodes.is_empty() {
                    pm.nodes.clear();
                } else if let Some(g) = parent {
                    pm.select(vec![g]);
                } else {
                    pm.select(Vec::new());
                }
            }
            Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown if !pm.selection.is_empty() => {
                let step = if mods.shift { 10.0 } else { 1.0 };
                let (dx, dy) = match key {
                    Key::ArrowLeft => (-step, 0.0),
                    Key::ArrowRight => (step, 0.0),
                    Key::ArrowUp => (0.0, -step),
                    _ => (0.0, step),
                };
                let moved = commands::nudge(pm.objects(ed), &pm.selection, &pm.nodes, dx, dy);
                pm.edit(ed, "Nudge", moved);
            }
            Key::Num0 => pm.fit(rect),
            Key::Plus | Key::Equals => pm.zoom_at(1.25, rect.center(), rect),
            Key::Minus => pm.zoom_at(0.8, rect.center(), rect),
            _ => {}
        }
    }
}

/// The editing keys while the pause map is the one in use: undo and redo
/// (the editor's, so they go back through the world's steps too), cut,
/// copy, paste, duplicate, delete, select all, group, lock and hide.
pub fn edit_keys(ctx: &egui::Context, pm: &mut PauseMap, ed: &mut Editor, tools: &mut Tools) {
    let (mods, pressed) = ctx.input(|i| (i.modifiers, i.keys_down.clone()));
    let _ = pressed;
    let key = |k: Key| ctx.input(|i| i.key_pressed(k));
    let (copy, cut, paste) = ctx.input(|i| {
        let mut found = (false, false, None);
        for e in &i.events {
            match e {
                egui::Event::Copy => found.0 = true,
                egui::Event::Cut => found.1 = true,
                egui::Event::Paste(text) => found.2 = Some(text.clone()),
                _ => {}
            }
        }
        found
    });
    let cmd = mods.command;
    if cmd && key(Key::Z) && !mods.shift {
        tools.undo(ed);
        pm.draft = None;
        pm.keep_selection(ed);
    } else if cmd && (key(Key::Y) || (key(Key::Z) && mods.shift)) {
        tools.redo(ed);
        pm.draft = None;
        pm.keep_selection(ed);
    } else if copy || cut {
        if let Some(text) = commands::clipboard_text(pm.objects(ed), &pm.selection, PauseMap::pictures(ed)) {
            ctx.copy_text(text.clone());
            pm.copied = Some(text);
            tools.say(format!("Copied {}", count(pm.selection.len(), "layer")));
            if cut {
                delete(pm, ed);
            }
        }
    } else if let Some(text) = paste {
        let text = if text.contains(commands::CLIPBOARD_APP) { Some(text) } else { pm.copied.clone() };
        if let Some(text) = text {
            let at = pm.view.cursor;
            if let Some(p) = commands::paste(pm.objects(ed), &text, at, &ed.pause_assets) {
                for (id, bytes) in p.pictures {
                    if !ed.pause_map().pictures.contains_key(&id) {
                        let _ = ed.add_pause_picture(bytes);
                    }
                }
                let n = p.selected.len();
                pm.edit(ed, "Paste", p.objects);
                pm.select(p.selected);
                tools.say(format!("Pasted {}", count(n, "layer")));
            }
        }
    } else if cmd && key(Key::D) {
        let (d, sel) = commands::duplicate(pm.objects(ed), &pm.selection, 12.0, 12.0);
        if !sel.is_empty() {
            pm.edit(ed, "Duplicate", d);
            pm.select(sel);
        }
    } else if cmd && key(Key::A) {
        pm.select(commands::select_all(pm.objects(ed)));
    } else if cmd && key(Key::G) {
        let result = if mods.shift {
            commands::ungroup(pm.objects(ed), &pm.selection).map(|r| ("Ungroup", r))
        } else {
            commands::group(pm.objects(ed), &pm.selection).map(|r| ("Group", r))
        };
        if let Some((label, (d, sel))) = result {
            pm.edit(ed, label, d);
            pm.select(sel);
        }
    } else if cmd && key(Key::L) {
        let d = if mods.shift {
            commands::unlock_all(pm.objects(ed))
        } else {
            commands::toggle_lock(pm.objects(ed), &pm.selection)
        };
        pm.edit(ed, "Lock", d);
        pm.keep_selection(ed);
    } else if cmd && mods.shift && key(Key::H) {
        let d = commands::toggle_hidden(pm.objects(ed), &pm.selection);
        pm.edit(ed, "Hide", d);
    } else if (key(Key::Delete) || key(Key::Backspace)) && pm.view.pen.is_none() {
        delete(pm, ed);
    }
}

/// Deletes the selected anchors (editing a line's points) or layers.
pub fn delete(pm: &mut PauseMap, ed: &mut Editor) {
    if !pm.nodes.is_empty() && pm.selection.len() == 1 {
        let d = commands::delete_nodes(pm.objects(ed), &pm.selection[0], &pm.nodes.clone());
        pm.edit(ed, "Delete anchors", d);
        pm.nodes.clear();
        return;
    }
    if pm.selection.is_empty() {
        return;
    }
    let d = commands::delete(pm.objects(ed), &pm.selection);
    pm.edit(ed, "Delete", d);
    pm.select(Vec::new());
}

fn context_menu(response: &egui::Response, pm: &mut PauseMap, ed: &mut Editor, tools: &mut Tools) {
    if response.secondary_clicked()
        && let (Some(p), Some(rect)) = (pm.view.cursor, pm.view.rect)
    {
        pm.view.menu_at = Some(p);
        let objects = pm.objects(ed).to_vec();
        let slop = 6.0 / pm.camera.zoom;
        if let Some(hit) = drawable_at(&objects, p, slop, false, ed) {
            let index = index_of(&objects);
            let target = if pm.tool == Tool::Direct {
                hit.id().to_owned()
            } else {
                pick_target(hit, &index, &pm.selection).to_owned()
            };
            if !pm.selection.contains(&target) {
                pm.select(vec![target]);
            }
        }
        let _ = rect;
    }
    response.context_menu(|ui| {
        let any = !pm.selection.is_empty();
        if ui.add_enabled(any, egui::Button::new("Duplicate").shortcut_text("Ctrl+D")).clicked() {
            let (d, sel) = commands::duplicate(pm.objects(ed), &pm.selection, 12.0, 12.0);
            pm.edit(ed, "Duplicate", d);
            pm.select(sel);
            ui.close();
        }
        if ui.add_enabled(any, egui::Button::new("Delete").shortcut_text("Del")).clicked() {
            delete(pm, ed);
            ui.close();
        }
        ui.separator();
        if ui.add_enabled(any, egui::Button::new("Group").shortcut_text("Ctrl+G")).clicked()
            && let Some((d, sel)) = commands::group(pm.objects(ed), &pm.selection)
        {
            pm.edit(ed, "Group", d);
            pm.select(sel);
            ui.close();
        }
        let groups = pm.selection.iter().any(|id| pm.objects(ed).iter().any(|o| o.id() == id && o.is_group()));
        if ui.add_enabled(groups, egui::Button::new("Ungroup").shortcut_text("Ctrl+Shift+G")).clicked()
            && let Some((d, sel)) = commands::ungroup(pm.objects(ed), &pm.selection)
        {
            pm.edit(ed, "Ungroup", d);
            pm.select(sel);
            ui.close();
        }
        if ui.add_enabled(any, egui::Button::new("Lock").shortcut_text("Ctrl+L")).clicked() {
            let d = commands::toggle_lock(pm.objects(ed), &pm.selection);
            pm.edit(ed, "Lock", d);
            pm.keep_selection(ed);
            ui.close();
        }
        if ui.add_enabled(any, egui::Button::new("Hide").shortcut_text("Ctrl+Shift+H")).clicked() {
            let d = commands::toggle_hidden(pm.objects(ed), &pm.selection);
            pm.edit(ed, "Hide", d);
            ui.close();
        }
        if let Some(MapObject::Line(l)) =
            pm.primary().and_then(|id| pm.objects(ed).iter().find(|o| o.id() == id)).cloned()
        {
            ui.separator();
            let nodes: Vec<usize> = if pm.nodes.is_empty() { (0..l.nodes.len()).collect() } else { pm.nodes.clone() };
            for (label, smooth) in [("Make smooth", true), ("Make corners", false)] {
                if ui.button(label).clicked() {
                    let d = commands::convert_nodes(pm.objects(ed), &l.common.id, &nodes, smooth);
                    pm.edit(ed, label, d);
                    ui.close();
                }
            }
            if ui.button("Reverse direction").clicked() {
                let d = commands::reverse_line(pm.objects(ed), &l.common.id);
                pm.edit(ed, "Reverse line", d);
                ui.close();
            }
        }
        ui.separator();
        if let Some(p) = pm.view.menu_at
            && ui.button("Show here in the world map").clicked()
        {
            pm.jump_world = Some(p);
            ui.close();
        }
        let _ = &tools;
    });
}

fn overlay(ui: &egui::Ui, pm: &PauseMap, ed: &Editor, rect: Rect, objects: &[MapObject]) {
    if let Some(c) = PENDING_CURSOR.with(|p| p.take())
        && ui.rect_contains_pointer(rect)
    {
        ui.ctx().set_cursor_icon(c);
    }
    let v = View { rect, cam: pm.camera };
    let painter = ui.painter_at(rect);
    let accent = theme::ACCENT;
    let thin = Stroke::new(1.0, accent);
    let path = |pts: &[Point]| -> Vec<Pos2> { pts.iter().map(|&p| v.screen(p)).collect() };
    let assets = &ed.pause_assets;
    let outline = |o: &MapObject, strong: bool| match o {
        MapObject::Line(l) => {
            let pts = path(&line_path(o));
            let w = (l.width * pm.camera.zoom) as f32 + 10.0;
            let halo = accent.gamma_multiply(if strong { 0.32 } else { 0.2 });
            painter.add(egui::Shape::line(pts.clone(), Stroke::new(w, halo)));
            painter.add(egui::Shape::line(pts, thin));
        }
        MapObject::Erase(e) => {
            let w = (e.radius * 2.0 * pm.camera.zoom) as f32;
            painter.add(egui::Shape::line(
                path(&e.points),
                Stroke::new(w, accent.gamma_multiply(if strong { 0.32 } else { 0.2 })),
            ));
        }
        MapObject::Group(_) => {}
        _ => {
            let pts = path(&box_of(o, assets));
            if pts.len() > 2 {
                painter.add(egui::Shape::convex_polygon(
                    pts.clone(),
                    accent.gamma_multiply(if strong { 0.1 } else { 0.06 }),
                    Stroke::NONE,
                ));
                painter.add(egui::Shape::closed_line(pts, Stroke::new(if strong { 1.25 } else { 1.0 }, accent)));
            }
        }
    };
    let square = |at: Pos2, size: f32, filled: bool| {
        let r = Rect::from_center_size(at, Vec2::splat(size - 1.0));
        painter.rect(r, 0.0, if filled { accent } else { Color32::WHITE }, thin, egui::StrokeKind::Middle);
    };
    let handle = |from: Point, rel: Option<Point>| {
        let Some(rel) = rel else { return };
        let (a, b) = (v.screen(from), v.screen([from[0] + rel[0], from[1] + rel[1]]));
        painter.line_segment([a, b], thin);
        painter.circle_filled(b, HANDLE_TIP, accent);
    };

    // The world map's camera.
    if let Some((w, yaw)) = pm.world_camera {
        let c = v.screen(world_to_map(w[0], w[1]));
        // The world's yaw 0 looks north (up on the map), turning left.
        let dir = Vec2::new(-yaw.sin(), -yaw.cos());
        let side = Vec2::new(-dir.y, dir.x);
        let tip = c + dir * 14.0;
        let pts = vec![tip, c - dir * 6.0 + side * 7.0, c - dir * 2.0, c - dir * 6.0 - side * 7.0];
        painter.add(egui::Shape::convex_polygon(
            pts,
            Color32::from_rgb(70, 140, 230),
            Stroke::new(1.5, Color32::WHITE),
        ));
    }

    let hover = pm.view.hover.clone().or_else(|| pm.list_hover.clone());
    if let Some(h) = hover.filter(|h| !pm.selection.contains(h)) {
        for o in drawables_in(&[h], objects) {
            outline(o, false);
        }
    }
    for o in drawables_in(&pm.selection, objects) {
        outline(o, true);
    }
    let gesture_move = matches!(pm.view.gesture, Some(Gesture::Move { .. }));
    let frame = if gesture_move { None } else { pm.frame_target(objects, ed) };
    let many = pm.selection.len() > 1
        || (pm.selection.len() == 1 && objects.iter().any(|o| o.id() == pm.selection[0] && o.is_group()));
    if many
        && frame.is_none()
        && let Some(b) = selection_bounds(&pm.selection, objects, assets)
    {
        let r = Rect::from_min_max(v.screen([b.x0, b.y0]), v.screen([b.x1, b.y1])).expand(4.0);
        let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
        painter.extend(egui::Shape::dashed_line(&corners, thin, 4.0, 3.0));
    }

    // Anchors and handles of the lines whose points are edited.
    for id in pm.edit_lines(objects) {
        let Some(MapObject::Line(l)) = objects.iter().find(|o| o.id() == id) else { continue };
        let chosen: &[usize] = if pm.primary() == Some(&id) { &pm.nodes } else { &[] };
        let n = l.nodes.len();
        for (i, node) in l.nodes.iter().enumerate() {
            if handles_show(chosen, i, n, l.closed) {
                handle(node.p, node.inn);
                handle(node.p, node.out);
            }
        }
        for (i, node) in l.nodes.iter().enumerate() {
            square(v.screen(node.p), ANCHOR, chosen.contains(&i));
        }
    }

    // The pen: its anchors, the last handles and the next segment.
    if let Some(id) = &pm.view.pen
        && let Some(MapObject::Line(l)) = objects.iter().find(|o| o.id() == id)
        && let Some(last) = l.nodes.last()
    {
        handle(last.p, last.out);
        handle(last.p, last.inn);
        let dragging = matches!(pm.view.gesture, Some(Gesture::Pen { .. }));
        if let (Some(c), false) = (pm.view.cursor, dragging) {
            let a = v.screen(last.p);
            let c1 = v.screen([last.p[0] + last.out.map_or(0.0, |o| o[0]), last.p[1] + last.out.map_or(0.0, |o| o[1])]);
            let b = v.screen(c);
            let curve = egui::epaint::CubicBezierShape::from_points_stroke(
                [a, c1, b, b],
                false,
                Color32::TRANSPARENT,
                Stroke::new(1.25, accent),
            );
            let pts = curve.flatten(Some(0.5));
            painter.extend(egui::Shape::dashed_line(&pts, Stroke::new(1.25, accent), 5.0, 4.0));
        }
        for (i, node) in l.nodes.iter().enumerate() {
            square(v.screen(node.p), ANCHOR, i == l.nodes.len() - 1);
        }
        if pm.view.closing {
            painter.circle_stroke(v.screen(l.nodes[0].p), ANCHOR + 2.0, Stroke::new(1.5, accent));
        }
    }

    // A single boxed object's corners and knob.
    let transforming =
        matches!(pm.view.gesture, Some(Gesture::Rotate { .. } | Gesture::Scale { .. } | Gesture::Move { .. }));
    if !transforming && let Some(t) = pm.transform_target(objects) {
        let (corners, mid, knob) = transform_handles(t, &v, ed);
        painter.line_segment([mid, knob], thin);
        painter.circle(knob, 5.0, Color32::WHITE, thin);
        for c in corners {
            square(c, CORNER, false);
        }
    }

    // The frame: scale from its edges and corners, turn from the knob.
    if let Some(b) = frame {
        let (r, handles, top, knob) = frame_handles(&b, &v);
        painter.rect_stroke(r, 0.0, thin, egui::StrokeKind::Middle);
        painter.line_segment([top, knob], thin);
        painter.circle(knob, 5.0, Color32::WHITE, thin);
        for (_, at) in handles {
            square(at, CORNER, false);
        }
    }

    if let Some(Gesture::Marquee { from, to, .. }) = &pm.view.gesture {
        let r = Rect::from_two_pos(v.screen(*from), v.screen(*to));
        painter.rect(
            r,
            0.0,
            accent.gamma_multiply(0.08),
            Stroke::new(1.0, accent.gamma_multiply(0.9)),
            egui::StrokeKind::Middle,
        );
    }

    if pm.tool == Tool::Erase
        && let Some(c) = pm.view.cursor
    {
        let r = (pm.erase_radius * pm.camera.zoom).max(2.0) as f32;
        painter.circle_stroke(v.screen(c), r, Stroke::new(1.5, Color32::from_black_alpha(90)));
        let pts: Vec<Pos2> = (0..=48)
            .map(|i| {
                let a = i as f32 / 48.0 * std::f32::consts::TAU;
                v.screen(c) + Vec2::new(a.cos(), a.sin()) * r
            })
            .collect();
        painter.extend(egui::Shape::dashed_line(&pts, Stroke::new(1.0, Color32::WHITE), 4.0, 3.0));
    }
}
