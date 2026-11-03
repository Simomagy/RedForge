//! A dummy RedForge, to try the UI's styles on before they go in the app:
//! its layout (menu, Project and Game Files, Hierarchy, the map with its
//! toolbar, Properties and Objects, the status bar) with made-up content,
//! and the styles of [`themes`] to switch between.
//!
//! `LAB_STYLE=<0-3>` starts in a style; with `LAB_SCREENSHOT=<png>` it saves
//! a screenshot of its window and quits.

mod themes;

use std::time::Instant;

use eframe::egui::{self, Color32, CornerRadius, RichText, Sense, Stroke, Vec2};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use egui_phosphor::regular as icon;
use themes::{Kind, THEMES, Theme};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Project,
    GameFiles,
    Hierarchy,
    World,
    Properties,
    Objects,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Select,
    Move,
    Rotate,
    Scale,
    Place,
}

struct Lab {
    theme: usize,
    applied: Option<usize>,
    dock: DockState<Tab>,
    world: egui::TextureHandle,
    icon: egui::TextureHandle,
    tool: Tool,
    mode: usize,
    hour: f32,
    search: String,
    position: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 2],
    selected_row: usize,
    started: Instant,
    shot: Option<(String, bool)>,
}

fn texture(ctx: &egui::Context, name: &str, bytes: &[u8]) -> egui::TextureHandle {
    let img = image::load_from_memory(bytes).expect("an image").into_rgba8();
    let image = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], &img);
    ctx.load_texture(name, image, egui::TextureOptions::LINEAR)
}

fn layout() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::World]);
    let surface = dock.main_surface_mut();
    let [centre, left] = surface.split_left(NodeIndex::root(), 0.2, vec![Tab::Project, Tab::GameFiles]);
    surface.split_below(left, 0.55, vec![Tab::Hierarchy]);
    surface.split_right(centre, 0.78, vec![Tab::Properties, Tab::Objects]);
    dock
}

impl Lab {
    fn new(cc: &eframe::CreationContext) -> Self {
        let theme =
            std::env::var("LAB_STYLE").ok().and_then(|s| s.parse().ok()).unwrap_or(0usize).min(THEMES.len() - 1);
        cc.egui_ctx.set_fonts(THEMES[theme].fonts());
        Self {
            theme,
            applied: None,
            dock: layout(),
            world: texture(&cc.egui_ctx, "world", include_bytes!("../../../docs/images/world.jpg")),
            icon: texture(&cc.egui_ctx, "icon", include_bytes!("../../../packaging/linux/icons/redforge-128.png")),
            tool: Tool::Move,
            mode: 0,
            hour: 17.5,
            search: String::new(),
            position: [-288.005, 672.005, 113.643],
            rotation: [0.0, 0.0, 90.0],
            scale: [1.0, 1.0],
            selected_row: 2,
            started: Instant::now(),
            shot: std::env::var("LAB_SCREENSHOT").ok().map(|p| (p, false)),
        }
    }

    fn t(&self) -> &'static Theme {
        &THEMES[self.theme]
    }
}

/// An icon and a label, the icon in its colour.
fn icon_text(t: &Theme, glyph: &str, kind: Kind, label: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let font = egui::FontId::proportional(t.size);
    job.append(glyph, 0.0, egui::TextFormat { font_id: font.clone(), color: t.icon(kind), ..Default::default() });
    if !label.is_empty() {
        job.append(label, 6.0, egui::TextFormat { font_id: font, color: t.text, ..Default::default() });
    }
    job
}

/// A toolbar button: highlighted in the accent when it's on.
fn tool_button(ui: &mut egui::Ui, t: &Theme, on: bool, glyph: &str, kind: Kind, label: &str, tip: &str) -> bool {
    let text = if t.labels { label } else { "" };
    let mut job = icon_text(t, glyph, kind, text);
    if on {
        for s in &mut job.sections {
            s.format.color = t.on_accent;
        }
    }
    let mut b = egui::Button::new(job).corner_radius(CornerRadius::same(t.radius));
    if on {
        b = b.fill(t.accent);
    } else {
        b = b.fill(Color32::TRANSPARENT);
    }
    ui.add(b).on_hover_text(tip).clicked()
}

/// A group of toolbar controls, on a card.
fn group(ui: &mut egui::Ui, t: &Theme, add: impl FnOnce(&mut egui::Ui)) {
    let stroke = if t.islands { Stroke::NONE } else { Stroke::new(1.0, t.border) };
    egui::Frame::new()
        .fill(t.card)
        .corner_radius(CornerRadius::same(t.radius + 2))
        .inner_margin(egui::Margin::symmetric(4, 3))
        .stroke(stroke)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(add);
        });
}

/// A heading row of a panel: an icon, a name, how many.
fn section(ui: &mut egui::Ui, t: &Theme, glyph: &str, kind: Kind, name: &str, count: Option<usize>, open: bool) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(if open { icon::CARET_DOWN } else { icon::CARET_RIGHT }).color(t.weak));
        ui.label(icon_text(t, glyph, kind, ""));
        ui.label(RichText::new(name).font(t.strong(t.size)).color(t.text));
        if let Some(n) = count {
            egui::Frame::new()
                .fill(t.widget)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(6, 0))
                .show(ui, |ui| {
                    ui.label(RichText::new(n.to_string()).small().color(t.weak));
                });
        }
    });
}

/// A row of a list: selectable, with an icon, a name and a dim note.
fn row(ui: &mut egui::Ui, t: &Theme, selected: bool, glyph: &str, kind: Kind, name: &str, note: &str) -> bool {
    let h = t.size + t.padding.y * 2.0 + 2.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::click());
    let painter = ui.painter();
    if selected {
        // Islands: a neutral grey, the accent only in the bar and the icon.
        let fill = if t.islands { t.widget } else { t.accent.gamma_multiply(0.28) };
        painter.rect_filled(rect, CornerRadius::same(t.radius), fill);
        if t.accent_bar {
            let bar = egui::Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height()));
            painter.rect_filled(bar, CornerRadius::same(2), t.accent);
        }
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(t.radius), t.hover.gamma_multiply(0.6));
    }
    let font = egui::FontId::proportional(t.size);
    let mut x = rect.min.x + 18.0;
    let y = rect.center().y;
    let icon_colour = if selected && t.islands { t.accent } else { t.icon(kind) };
    painter.text(egui::pos2(x, y), egui::Align2::LEFT_CENTER, glyph, font.clone(), icon_colour);
    x += t.size + 8.0;
    let g = painter.text(egui::pos2(x, y), egui::Align2::LEFT_CENTER, name, font.clone(), t.text);
    if !note.is_empty() {
        painter.text(
            egui::pos2(g.max.x + 10.0, y),
            egui::Align2::LEFT_CENTER,
            note,
            egui::FontId::proportional(t.size - 2.0),
            t.weak,
        );
    }
    resp.clicked()
}

struct Tabs<'a> {
    lab: &'a mut Lab,
}

impl TabViewer for Tabs<'_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        let t = self.lab.t();
        let (glyph, kind, name) = match tab {
            Tab::Project => (icon::FOLDER_OPEN, Kind::Folder, "Project"),
            Tab::GameFiles => (icon::PACKAGE, Kind::Archive, "Game Files"),
            Tab::Hierarchy => (icon::TREE_STRUCTURE, Kind::Select, "Hierarchy"),
            Tab::World => (icon::MAP_TRIFOLD, Kind::Map, "World"),
            Tab::Properties => (icon::SLIDERS, Kind::Transform, "Properties"),
            Tab::Objects => (icon::CUBE, Kind::Model, "Objects"),
        };
        icon_text(t, glyph, kind, name).into()
    }

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(("lab-tab", *tab as u8))
    }

    fn closeable(&mut self, tab: &mut Tab) -> bool {
        *tab == Tab::World
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        let t = self.lab.t();
        if *tab == Tab::World {
            return self.lab.world_ui(ui, t);
        }
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| match tab {
            Tab::Project => self.lab.project(ui, t),
            Tab::GameFiles => self.lab.game_files(ui, t),
            Tab::Hierarchy => self.lab.hierarchy(ui, t),
            Tab::Properties => self.lab.properties(ui, t),
            Tab::Objects => self.lab.objects(ui, t),
            Tab::World => {}
        });
    }
}

const MODES: [&str; 10] = [
    "Entity",
    "Light",
    "Physics",
    "Navmesh",
    "Skeleton",
    "Occlusion",
    "Interior Instance",
    "Time Cycle Modifier",
    "Ambient Mask Volume",
    "Scenario",
];

impl Lab {
    fn project(&mut self, ui: &mut egui::Ui, t: &Theme) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Furnace").font(t.heading(t.size + 4.0)).color(t.text));
            ui.label(RichText::new("30 files").small().color(t.weak));
        });
        ui.horizontal_wrapped(|ui| {
            for (g, k, l) in [
                (icon::PLUS, Kind::Place, "New"),
                (icon::FOLDER_OPEN, Kind::Folder, "Open"),
                (icon::FLOPPY_DISK, Kind::Save, "Save all"),
            ] {
                let _ = ui.button(icon_text(t, g, k, l));
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.search).hint_text(format!("{}  Search files", icon::MAGNIFYING_GLASS)),
        );
        ui.add_space(4.0);
        section(ui, t, icon::MAP_TRIFOLD, Kind::Map, "Maps", Some(5), true);
        let maps =
            ["fnx_prisonmex.ymap", "pry_prisonmap.ymap", "rat_01.ymap", "rat_01_strm_0.ymap", "rstu_96_99_occ.ymap"];
        for (i, m) in maps.iter().enumerate() {
            if row(ui, t, self.selected_row == i, icon::MAP_PIN, Kind::Map, m, "map assets") {
                self.selected_row = i;
            }
        }
        section(ui, t, icon::STACK, Kind::Archetype, "Archetypes", Some(4), true);
        for (i, a) in ["fnx_prisonmex.ytyp", "pry_prison_tower.ytyp"].iter().enumerate() {
            if row(ui, t, self.selected_row == 10 + i, icon::FILE_CODE, Kind::Archetype, a, "map assets") {
                self.selected_row = 10 + i;
            }
        }
        section(ui, t, icon::CUBE, Kind::Model, "Models", Some(13), false);
        section(ui, t, icon::IMAGE, Kind::Texture, "Textures", Some(6), false);
        section(ui, t, icon::CUBE_TRANSPARENT, Kind::Collision, "Collisions", Some(2), false);
        ui.add_space(6.0);
        egui::CollapsingHeader::new(RichText::new("RedM resource").font(t.strong(t.size))).show(ui, |ui| {
            ui.label("Resource name");
        });
    }

    fn game_files(&mut self, ui: &mut egui::Ui, t: &Theme) {
        ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text(format!("{}  Search all files…", icon::MAGNIFYING_GLASS)),
        );
        for a in ["anim_0.rpf", "appdata0_update.rpf", "common_0.rpf", "data_0.rpf", "levels_0.rpf", "levels_1.rpf"] {
            row(ui, t, false, icon::PACKAGE, Kind::Archive, a, "");
        }
        row(ui, t, true, icon::FOLDER_OPEN, Kind::Folder, "levels_3.rpf", "");
        for (g, k, f) in [
            (icon::FOLDER, Kind::Folder, "    interiors"),
            (icon::CUBE, Kind::Model, "    van_03_monster.ydr"),
            (icon::IMAGE, Kind::Texture, "    van_03_monster.ytd"),
            (icon::CUBE_TRANSPARENT, Kind::Collision, "    van_03_monster.ybn"),
        ] {
            row(ui, t, false, g, k, f, "");
        }
    }

    fn hierarchy(&mut self, ui: &mut egui::Ui, t: &Theme) {
        ui.label(RichText::new("l_09__strm_0.ymap").font(t.strong(t.size)).color(t.text));
        section(ui, t, icon::CUBE, Kind::Model, "HD", Some(58), true);
        for (i, n) in
            ["p_barrel01x  #12", "p_crate03x  #13", "l_09__hd_0_-2_-1  #32", "p_fencepost01x  #33"].iter().enumerate()
        {
            let selected = i == 2;
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::EYE).color(t.weak));
                row(ui, t, selected, icon::CUBE, Kind::Model, n, "");
            });
        }
        section(ui, t, icon::STACK, Kind::Archetype, "LOD", Some(12), false);
    }

    fn world_ui(&mut self, ui: &mut egui::Ui, t: &Theme) {
        // The toolbar: the mode, the tools, the snaps, history, time and place.
        ui.horizontal_wrapped(|ui| {
            group(ui, t, |ui| {
                egui::ComboBox::from_id_salt("mode")
                    .selected_text(icon_text(t, icon::CROSSHAIR, Kind::Select, MODES[self.mode]))
                    .show_ui(ui, |ui| {
                        for (i, m) in MODES.iter().enumerate() {
                            ui.selectable_value(&mut self.mode, i, *m);
                        }
                    });
            });
            group(ui, t, |ui| {
                for (tool, g, l, tip) in [
                    (Tool::Select, icon::CURSOR, "Select", "Select (V)"),
                    (Tool::Move, icon::ARROWS_OUT_CARDINAL, "Move", "Move (Q)"),
                    (Tool::Rotate, icon::ARROW_CLOCKWISE, "Rotate", "Rotate (E)"),
                    (Tool::Scale, icon::ARROWS_OUT, "Scale", "Scale"),
                    (Tool::Place, icon::PLUS, "Place", "Place the object chosen in Objects"),
                ] {
                    let kind = if tool == Tool::Place { Kind::Place } else { Kind::Transform };
                    if tool_button(ui, t, self.tool == tool, g, kind, l, tip) {
                        self.tool = tool;
                    }
                }
            });
            group(ui, t, |ui| {
                tool_button(ui, t, false, icon::GLOBE_HEMISPHERE_WEST, Kind::Neutral, "World", "Gizmo axes (.)");
                tool_button(ui, t, false, icon::MAGNET, Kind::Neutral, "Snap off", "Snapping");
            });
            group(ui, t, |ui| {
                tool_button(ui, t, false, icon::ARROW_U_UP_LEFT, Kind::History, "", "Undo (Ctrl+Z)");
                tool_button(ui, t, false, icon::ARROW_U_UP_RIGHT, Kind::History, "", "Redo (Ctrl+Y)");
            });
            group(ui, t, |ui| {
                tool_button(ui, t, false, icon::MAP_PIN, Kind::Map, "Go to", "Jump to a town");
                ui.label(RichText::new(icon::SUN).color(t.icon(Kind::Light)));
                ui.spacing_mut().slider_width = 90.0;
                ui.add(egui::Slider::new(&mut self.hour, 0.0..=24.0).show_value(true).fixed_decimals(1));
                tool_button(ui, t, false, icon::CLOUD_SUN, Kind::Light, "Sunny", "Weather");
            });
        });
        ui.add_space(2.0);

        // The map (the map's part of a screenshot of the app, cropped to
        // fill), its stats over it.
        let rect = ui.available_rect_before_wrap();
        let map = egui::Rect::from_min_max(
            egui::pos2(276.0 / 1600.0, 180.0 / 950.0),
            egui::pos2(1208.0 / 1600.0, 918.0 / 950.0),
        );
        let (map_aspect, aspect) = (map.width() * 1600.0 / (map.height() * 950.0), rect.width() / rect.height());
        let uv = if aspect > map_aspect {
            let h = map.height() * map_aspect / aspect;
            egui::Rect::from_center_size(map.center(), Vec2::new(map.width(), h))
        } else {
            let w = map.width() * aspect / map_aspect;
            egui::Rect::from_center_size(map.center(), Vec2::new(w, map.height()))
        };
        let corners = CornerRadius::same(if t.islands { t.radius } else { 0 });
        ui.painter()
            .add(egui::epaint::RectShape::filled(rect, corners, Color32::WHITE).with_texture(self.world.id(), uv));
        ui.allocate_rect(rect, Sense::click_and_drag());
        let stats = "-300 640 122   10.0 ms\n1627 entities, 1104 drawn in 4258 draw calls\nmodels 1481, textures 1870";
        let galley = ui.painter().layout_no_wrap(stats.into(), egui::FontId::monospace(t.size - 2.0), t.text);
        let at = rect.left_top() + Vec2::splat(10.0);
        let back = egui::Rect::from_min_size(at, galley.size()).expand(6.0);
        ui.painter().rect_filled(back, CornerRadius::same(t.radius), t.bg.gamma_multiply(0.8));
        ui.painter().galley(at, galley, t.text);
        let label = rect.center() + Vec2::new(60.0, 60.0);
        ui.painter().text(label, egui::Align2::CENTER_TOP, "p_barrel01x", egui::FontId::proportional(t.size), t.text);
    }

    fn properties(&mut self, ui: &mut egui::Ui, t: &Theme) {
        ui.horizontal(|ui| {
            ui.label(icon_text(t, icon::CUBE, Kind::Model, ""));
            ui.label(RichText::new("p_barrel01x").font(t.heading(t.size + 4.0)).color(t.text));
        });
        ui.label(RichText::new("Entity 32 in l_09__strm_0.ymap").small().color(t.weak));
        ui.add_space(6.0);
        let card = |ui: &mut egui::Ui, title: &str, add: &mut dyn FnMut(&mut egui::Ui)| {
            egui::Frame::new()
                .fill(t.card)
                .corner_radius(CornerRadius::same(t.big_radius))
                .inner_margin(egui::Margin::same(10))
                .stroke(Stroke::new(1.0, t.border))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(title).font(t.strong(t.size - 1.0)).color(t.weak));
                    ui.add_space(2.0);
                    add(ui);
                });
            ui.add_space(6.0);
        };
        let (mut p, mut r, mut s) = (self.position, self.rotation, self.scale);
        card(ui, "TRANSFORM", &mut |ui| {
            ui.spacing_mut().interact_size.x = 48.0;
            egui::Grid::new("transform").num_columns(4).spacing([6.0, 6.0]).show(ui, |ui| {
                ui.label("Position");
                for v in &mut p {
                    ui.add(egui::DragValue::new(v).speed(0.1).fixed_decimals(2));
                }
                ui.end_row();
                ui.label("Rotation °");
                for v in &mut r {
                    ui.add(egui::DragValue::new(v).speed(1.0).fixed_decimals(1));
                }
                ui.end_row();
                ui.label("Scale XY, Z");
                ui.add(egui::DragValue::new(&mut s[0]).speed(0.01).fixed_decimals(2));
                ui.add(egui::DragValue::new(&mut s[1]).speed(0.01).fixed_decimals(2));
                ui.end_row();
            });
        });
        (self.position, self.rotation, self.scale) = (p, r, s);
        card(ui, "ENTITY", &mut |ui| {
            egui::Grid::new("fields").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                for (k, v) in [
                    ("lodDist", "150.0"),
                    ("lodLevel", "LODTYPES_DEPTH_HD"),
                    ("priorityLevel", "PRI_REQUIRED"),
                    ("tintValue", "0"),
                    ("flags", "0x00180008"),
                ] {
                    ui.label(RichText::new(k).color(t.weak));
                    ui.label(v);
                    ui.end_row();
                }
            });
        });
        ui.horizontal(|ui| {
            let _ = ui.add(
                egui::Button::new(RichText::new(format!("{}  Look at it", icon::CROSSHAIR)).color(t.on_accent))
                    .fill(t.accent),
            );
            let _ = ui.button(icon_text(t, icon::COPY, Kind::Neutral, "Duplicate"));
        });
    }

    fn objects(&mut self, ui: &mut egui::Ui, t: &Theme) {
        ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text(format!("{}  Search objects", icon::MAGNIFYING_GLASS)),
        );
        ui.label(RichText::new("123,440 objects").small().color(t.weak));
        let names = ["p_barrel01x", "p_crate03x", "p_bench02x", "p_lantern05x", "p_cart01x", "p_chair04x"];
        egui::Grid::new("thumbs").spacing([8.0, 8.0]).show(ui, |ui| {
            for (i, n) in names.iter().enumerate() {
                ui.vertical(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(104.0), Sense::click());
                    ui.painter().rect_filled(rect, CornerRadius::same(t.big_radius), t.card);
                    if i == 0 {
                        ui.painter().rect_stroke(
                            rect,
                            CornerRadius::same(t.big_radius),
                            Stroke::new(2.0, t.accent),
                            egui::StrokeKind::Inside,
                        );
                    }
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        icon::CUBE,
                        egui::FontId::proportional(40.0),
                        t.icon(Kind::Model),
                    );
                    ui.label(RichText::new(*n).small().color(t.weak));
                });
                if i % 2 == 1 {
                    ui.end_row();
                }
            }
        });
    }
}

impl eframe::App for Lab {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.applied != Some(self.theme) {
            self.t().apply(&ctx);
            self.applied = Some(self.theme);
        }
        let t = self.t();

        egui::Panel::top("menu").frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin::symmetric(8, 4))).show(
            ui,
            |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.add(egui::Image::new(&self.icon).fit_to_exact_size(Vec2::splat(t.size + 4.0)));
                    for m in ["File", "Edit", "View", "Help"] {
                        ui.menu_button(m, |ui| {
                            let _ = ui.button("…");
                        });
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (i, th) in THEMES.iter().enumerate().rev() {
                            let on = i == self.theme;
                            let text = RichText::new(th.name).color(if on { t.on_accent } else { t.text });
                            let b = egui::Button::new(text).fill(if on { t.accent } else { t.widget });
                            if ui.add(b).on_hover_text(th.tagline).clicked() {
                                self.theme = i;
                            }
                        }
                        ui.label(RichText::new("Style").color(t.weak));
                    });
                });
            },
        );
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(t.bg).inner_margin(egui::Margin::symmetric(10, 4)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{}  495,787 files in 4,962 archives", icon::PACKAGE))
                            .small()
                            .color(t.weak),
                    );
                    ui.separator();
                    ui.label(RichText::new("/run/media/…/Red Dead Redemption 2").small().color(t.weak));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(format!("{}  Blender", icon::CIRCLE)).small().color(rgb_ok()));
                    });
                });
            });
        let style = t.dock(ui);
        let mut dock = std::mem::replace(&mut self.dock, DockState::new(vec![]));
        let gap = if t.islands { 8 } else { 0 };
        let frame =
            egui::Frame::new().fill(t.bg).inner_margin(egui::Margin { left: gap, right: gap, top: 0, bottom: 0 });
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            DockArea::new(&mut dock).style(style).show_inside(ui, &mut Tabs { lab: self });
        });
        self.dock = dock;

        if let Some((path, requested)) = &mut self.shot {
            if !*requested && self.started.elapsed().as_secs_f32() > 1.5 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                *requested = true;
            }
            let shot = ctx.input(|i| {
                i.raw.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(img) = shot {
                let [w, h] = img.size;
                let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                if let Err(e) = image::save_buffer(&*path, &bytes, w as u32, h as u32, image::ColorType::Rgba8) {
                    log::error!("screenshot: {e}");
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint();
        }
    }
}

fn rgb_ok() -> Color32 {
    Color32::from_rgb(110, 200, 120)
}

fn main() -> eframe::Result {
    env_logger::init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("RedForge UI lab").with_inner_size([1600.0, 950.0]),
        ..Default::default()
    };
    eframe::run_native("RedForge UI lab", options, Box::new(|cc| Ok(Box::new(Lab::new(cc)))))
}
