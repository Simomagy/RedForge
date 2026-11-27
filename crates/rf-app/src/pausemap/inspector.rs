//! The pause map's side of Properties: the selected layer's fields (a value
//! dragged is one undo step when let go, a text one when the field is left),
//! or, with nothing selected, the tool's settings and the fonts.

use eframe::egui::{self, DragValue, RichText};
use rf_editor::Editor;
use rf_editor::pausemap::commands;
use rf_editor::pausemap::families::Layer;
use rf_editor::pausemap::fonts::{Font, file_font_name};
use rf_editor::pausemap::project::*;

use super::{PauseMap, Tool};
use crate::settings::Settings;
use crate::theme;

/// A number of an object: its name, value, ranges, step, unit and where it
/// goes.
struct Field<T> {
    name: &'static str,
    value: f64,
    /// The slider's range, and the value's.
    soft: std::ops::RangeInclusive<f64>,
    hard: std::ops::RangeInclusive<f64>,
    step: f64,
    unit: &'static str,
    field: fn(&mut T) -> &mut f64,
}

/// A [`Field`]: name, value, the slider's range, the value's, step, unit,
/// where it goes.
macro_rules! field {
    ($name:expr, $value:expr, $soft:expr, $hard:expr, $step:expr, $unit:expr, $f:expr) => {
        Field { name: $name, value: $value, soft: $soft, hard: $hard, step: $step, unit: $unit, field: $f }
    };
}

/// The font files added, one path a line.
const FONTS: &str = "PauseMap.Fonts";

/// The fonts added before, into the editor.
pub fn load_user_fonts(ed: &mut Editor, settings: &Settings) {
    let Some(list) = settings.get(FONTS) else { return };
    for path in list.lines().filter(|l| !l.is_empty()) {
        let p = std::path::Path::new(path);
        match std::fs::read(p).map_err(|e| e.to_string()).and_then(|d| Font::from_file(&file_font_name(p), path, d)) {
            Ok(f) => ed.pause_assets.fonts.add(f),
            Err(e) => log::warn!("{path}: {e}"),
        }
    }
    ed.pause_assets.lettering.clear();
}

/// A change to one object from a field: kept as a draft while the field is
/// dragged or typed in, one step when it's let go.
fn change(
    pm: &mut PauseMap,
    ed: &mut Editor,
    id: &str,
    label: &str,
    r: &egui::Response,
    f: impl FnOnce(&mut MapObject),
) {
    if r.changed() {
        let mut d = pm.objects(ed).to_vec();
        if let Some(o) = d.iter_mut().find(|o| o.id() == id) {
            f(o);
        }
        if r.dragged() || r.has_focus() {
            pm.draft = Some(d);
        } else {
            pm.edit(ed, label, d);
        }
    }
    if (r.drag_stopped() || r.lost_focus()) && pm.draft.is_some() && !pm.busy() {
        pm.commit(ed, label);
    }
}

/// A number's slider and its value (typed or dragged): the slider goes over
/// `soft`, the value may go anywhere in `hard`.
fn number(
    ui: &mut egui::Ui,
    label: &str,
    v: &mut f64,
    soft: std::ops::RangeInclusive<f64>,
    hard: std::ops::RangeInclusive<f64>,
    step: f64,
    unit: &str,
) -> [egui::Response; 2] {
    ui.label(label);
    let rs = ui
        .horizontal(|ui| {
            let s = theme::slider(ui, v, soft, step, 132.0);
            let decimals = if step >= 1.0 {
                0
            } else if step >= 0.1 {
                1
            } else {
                2
            };
            let d = ui.add(DragValue::new(v).speed(step).range(hard).suffix(unit).fixed_decimals(decimals));
            [s, d]
        })
        .inner;
    ui.end_row();
    rs
}

/// [`change`] for a number's slider and value.
fn change_number(
    pm: &mut PauseMap,
    ed: &mut Editor,
    id: &str,
    label: &str,
    rs: &[egui::Response; 2],
    f: impl Fn(&mut MapObject),
) {
    for r in rs {
        change(pm, ed, id, label, r, &f);
    }
}

pub fn ui(ui: &mut egui::Ui, pm: &mut PauseMap, ed: &mut Editor, settings: &mut Settings) {
    if ui.input(|i| i.pointer.any_pressed()) && ui.rect_contains_pointer(ui.max_rect()) {
        pm.active = true;
    }
    let objects = pm.objects(ed).to_vec();
    match pm.selection.len() {
        0 => tool_ui(ui, pm, ed, settings),
        n => {
            let Some(o) = pm.primary().and_then(|id| objects.iter().find(|o| o.id() == id)).cloned() else { return };
            if n > 1 {
                ui.label(RichText::new(format!("{n} layers selected; the last one's fields")).color(theme::WEAK));
                ui.horizontal(|ui| {
                    if ui.button("Group").clicked()
                        && let Some((d, sel)) = commands::group(&objects, &pm.selection)
                    {
                        pm.edit(ed, "Group", d);
                        pm.select(sel);
                    }
                    if ui.button("Delete").clicked() {
                        super::pane::delete(pm, ed);
                    }
                });
                ui.separator();
            }
            object_ui(ui, pm, ed, &o);
        }
    }
}

fn object_ui(ui: &mut egui::Ui, pm: &mut PauseMap, ed: &mut Editor, o: &MapObject) {
    let id = o.id().to_owned();
    ui.horizontal(|ui| {
        ui.label(RichText::new(object_name(o)).font(theme::strong(theme::SIZE + 1.0)));
    });
    egui::Grid::new(("pause-common", &id)).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Name");
        let mut name = o.common().name.clone().unwrap_or_default();
        let r = ui.add(egui::TextEdit::singleline(&mut name).hint_text(object_name(o)));
        ui.end_row();
        change(pm, ed, &id, "Rename", &r, |x| {
            let n = name.trim();
            x.common_mut().name = (!n.is_empty()).then(|| n.to_owned());
        });
        ui.label("");
        ui.horizontal(|ui| {
            let mut hidden = o.common().hidden;
            let r = ui.checkbox(&mut hidden, "Hidden");
            change(pm, ed, &id, "Hide", &r, |x| x.common_mut().hidden = hidden);
            let mut locked = o.common().locked;
            let r = ui.checkbox(&mut locked, "Locked");
            change(pm, ed, &id, "Lock", &r, |x| x.common_mut().locked = locked);
        });
        ui.end_row();
    });
    ui.separator();
    match o {
        MapObject::Line(l) => line_ui(ui, pm, ed, l),
        MapObject::Label(l) => label_ui(ui, pm, ed, l),
        MapObject::Building(b) => building_ui(ui, pm, ed, b),
        MapObject::Stamp(s) => stamp_ui(ui, pm, ed, s),
        MapObject::Erase(e) => erase_ui(ui, pm, ed, e),
        MapObject::Group(_) => {
            ui.label(
                RichText::new("A group: double-click it on the map to work inside it, Ctrl+Shift+G takes it apart.")
                    .color(theme::WEAK),
            );
            if ui.button("Ungroup").clicked()
                && let Some((d, sel)) = commands::ungroup(pm.objects(ed), &pm.selection)
            {
                pm.edit(ed, "Ungroup", d);
                pm.select(sel);
            }
        }
    }
}

fn style_combo(ui: &mut egui::Ui, salt: &str, style: &mut LineStyle) -> egui::Response {
    let mut r = egui::ComboBox::from_id_salt(salt).selected_text(style_info(*style).label).show_ui(ui, |ui| {
        let mut changed = false;
        for s in &LINE_STYLES {
            changed |= ui.selectable_value(style, s.style, s.label).changed();
        }
        changed
    });
    if r.inner == Some(true) {
        r.response.mark_changed();
    }
    r.response
}

fn line_ui(ui: &mut egui::Ui, pm: &mut PauseMap, ed: &mut Editor, l: &LineObject) {
    let id = l.common.id.clone();
    egui::Grid::new(("pause-line", &id)).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Style");
        let mut style = l.style;
        let r = style_combo(ui, "line-style", &mut style);
        ui.end_row();
        change(pm, ed, &id, "Line style", &r, |x| {
            if let MapObject::Line(x) = x {
                // A new style starts with its own width and spacing.
                let info = style_info(style);
                (x.style, x.width, x.spacing) = (style, info.width, info.spacing);
            }
        });
        let mut width = l.width;
        let r = number(ui, "Width", &mut width, 0.1..=6.0, 0.1..=40.0, 0.05, "");
        change_number(pm, ed, &id, "Line width", &r, |x| {
            if let MapObject::Line(x) = x {
                x.width = width;
            }
        });
        if matches!(l.style, LineStyle::Trail | LineStyle::Boundary | LineStyle::Railroad) {
            let mut spacing = l.spacing;
            let r = number(ui, "Spacing", &mut spacing, 1.0..=40.0, 0.5..=200.0, 0.5, "");
            change_number(pm, ed, &id, "Line spacing", &r, |x| {
                if let MapObject::Line(x) = x {
                    x.spacing = spacing;
                }
            });
        }
        ui.label("");
        let mut closed = l.closed;
        let r = ui.checkbox(&mut closed, "Closed");
        ui.end_row();
        change(pm, ed, &id, "Close line", &r, |x| {
            if let MapObject::Line(x) = x {
                x.closed = closed && x.nodes.len() > 2;
            }
        });
    });
    ui.label(
        RichText::new(format!(
            "{} anchors{}",
            l.nodes.len(),
            if pm.nodes.is_empty() { String::new() } else { format!(", {} chosen", pm.nodes.len()) }
        ))
        .color(theme::WEAK),
    );
    ui.horizontal(|ui| {
        let nodes: Vec<usize> = if pm.nodes.is_empty() { (0..l.nodes.len()).collect() } else { pm.nodes.clone() };
        if ui.button("Smooth").on_hover_text("Curves through the chosen anchors (or all)").clicked() {
            pm.edit(ed, "Make smooth", commands::convert_nodes(pm.objects(ed), &id, &nodes, true));
        }
        if ui.button("Corners").on_hover_text("Straight through the chosen anchors (or all)").clicked() {
            pm.edit(ed, "Make corners", commands::convert_nodes(pm.objects(ed), &id, &nodes, false));
        }
        if ui.button("Reverse").clicked() {
            pm.edit(ed, "Reverse line", commands::reverse_line(pm.objects(ed), &id));
        }
    });
    ui.label(RichText::new("Double-click the line for its anchors; Alt+click on it adds one, on an anchor makes it a corner or smooth.").color(theme::WEAK).small());
}

/// The fonts' previews made so far.
#[derive(Default)]
pub struct Previews {
    textures: std::collections::HashMap<String, egui::TextureHandle>,
    named: std::collections::HashSet<String>,
}

impl Previews {
    /// Forgets a font's (it was added again or removed).
    pub fn forget(&mut self, name: &str) {
        self.textures.remove(name);
        self.named.remove(name);
    }
}

/// How tall a font's preview is, in points.
const PREVIEW: f32 = 26.0;

/// A font's name lettered in it (made once); `named` gets the fonts whose
/// preview isn't their name, to show it next to it.
fn preview(
    ui: &egui::Ui,
    previews: &mut std::collections::HashMap<String, egui::TextureHandle>,
    named: &mut std::collections::HashSet<String>,
    ed: &Editor,
    name: &str,
) -> Option<egui::TextureHandle> {
    if let Some(t) = previews.get(name) {
        return Some(t.clone());
    }
    let fonts = &ed.pause_assets.fonts;
    let font = fonts.get(name)?;
    let ppp = ui.ctx().pixels_per_point();
    let height = (PREVIEW * ppp).round() as usize;
    let mut lettered = font.preview(&fonts.all()[0], name, height, (360.0 * ppp) as usize);
    // A font of figures only (RDR Lino Numbers) has blank letters: its figures.
    if lettered.0.iter().all(|&a| a == 0) {
        lettered = font.preview(&fonts.all()[0], "0123456789", height, (360.0 * ppp) as usize);
        named.insert(name.to_owned());
    }
    let (alpha, w, h) = lettered;
    let rgba: Vec<u8> = alpha.iter().flat_map(|&a| [255, 255, 255, a]).collect();
    let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
    let t = ui.ctx().load_texture(format!("font preview {name}"), image, egui::TextureOptions::LINEAR);
    previews.insert(name.to_owned(), t.clone());
    Some(t)
}

/// The fonts to choose from, each shown in itself.
fn font_combo(
    ui: &mut egui::Ui,
    salt: &str,
    font: &mut String,
    ed: &Editor,
    previews: &mut Previews,
) -> egui::Response {
    let Previews { textures, named } = previews;
    let fonts = ed.pause_assets.fonts.all();
    let ppp = ui.ctx().pixels_per_point();
    let shown = |t: &egui::TextureHandle, tint: egui::Color32, scale: f32| {
        let [w, h] = t.size();
        egui::Image::new(t).fit_to_exact_size(egui::vec2(w as f32 / ppp * scale, h as f32 / ppp * scale)).tint(tint)
    };
    let current = preview(ui, textures, named, ed, font);
    let mut r =
        egui::ComboBox::from_id_salt(salt).selected_text(font.as_str()).width(220.0).height(420.0).show_ui(ui, |ui| {
            let mut changed = false;
            for (origin, title) in [("bundled", "Bundled"), ("game", "The game's"), ("", "Added")] {
                let these: Vec<String> = fonts
                    .iter()
                    .filter(|f| {
                        if origin.is_empty() { f.origin != "bundled" && f.origin != "game" } else { f.origin == origin }
                    })
                    .map(|f| f.name.clone())
                    .collect();
                if these.is_empty() {
                    continue;
                }
                ui.label(RichText::new(title).small().color(theme::WEAK));
                for name in these {
                    let on = *font == name;
                    let r = match preview(ui, textures, named, ed, &name) {
                        Some(t) if named.contains(&name) => ui.add(
                            egui::Button::selectable(
                                on,
                                (shown(&t, theme::TEXT, 0.8), RichText::new(&name).small().color(theme::WEAK)),
                            )
                            .frame_when_inactive(false),
                        ),
                        Some(t) => {
                            ui.add(egui::Button::selectable(on, shown(&t, theme::TEXT, 1.0)).frame_when_inactive(false))
                        }
                        None => ui.add(egui::Button::selectable(on, name.as_str())),
                    }
                    .on_hover_text(&name);
                    if r.clicked() && !on {
                        *font = name;
                        changed = true;
                    }
                }
            }
            changed
        });
    if r.inner == Some(true) {
        r.response.mark_changed();
    }
    // The chosen one, as it looks.
    if let Some(t) = current {
        ui.end_row();
        ui.label("");
        ui.add(shown(&t, theme::WEAK, 0.8));
    }
    r.response
}

fn label_ui(ui: &mut egui::Ui, pm: &mut PauseMap, ed: &mut Editor, l: &LabelObject) {
    let id = l.common.id.clone();
    let mut text = l.text.clone();
    let r = ui.add(egui::TextEdit::multiline(&mut text).desired_rows(1).desired_width(f32::INFINITY));
    if pm.focus_text.as_deref() == Some(id.as_str()) {
        r.request_focus();
        pm.focus_text = None;
    }
    change(pm, ed, &id, "Label text", &r, |x| {
        if let MapObject::Label(x) = x {
            x.text = text.clone();
        }
    });
    ui.horizontal_wrapped(|ui| {
        for p in &LABEL_PRESETS {
            if ui.button(p.label).on_hover_text(p.note).clicked() {
                let mut d = pm.objects(ed).to_vec();
                if let Some(MapObject::Label(x)) = d.iter_mut().find(|o| o.id() == id) {
                    (x.size, x.tracking, x.weight, x.caps) = (p.size, p.tracking, p.weight, p.caps);
                }
                pm.edit(ed, "Label preset", d);
            }
        }
    });
    egui::Grid::new(("pause-label", &id)).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Font");
        let mut font = l.font.clone();
        let r = font_combo(ui, "label-font", &mut font, ed, &mut pm.font_previews);
        ui.end_row();
        change(pm, ed, &id, "Label font", &r, |x| {
            if let MapObject::Label(x) = x {
                x.font = font.clone();
            }
        });
        let fields: [Field<LabelObject>; 5] = [
            field!("Size", l.size, 6.0..=100.0, 4.0..=400.0, 0.5, "", |x| &mut x.size),
            field!("Letter spacing", l.tracking, 0.0..=2.0, 0.0..=3.0, 0.01, "", |x| &mut x.tracking),
            field!("Curve", l.curve, -1.0..=1.0, -2.0..=2.0, 0.01, "", |x| &mut x.curve),
            field!("Weight", l.weight, -1.0..=4.0, -1.0..=6.0, 0.05, "", |x| &mut x.weight),
            field!("Rotation", l.rotation, -180.0..=180.0, -180.0..=180.0, 0.5, "°", |x| &mut x.rotation),
        ];
        for f in fields {
            let mut value = f.value;
            let r = number(ui, f.name, &mut value, f.soft, f.hard, f.step, f.unit);
            change_number(pm, ed, &id, &format!("Label {}", f.name.to_lowercase()), &r, |x| {
                if let MapObject::Label(x) = x {
                    *(f.field)(x) = value;
                }
            });
        }
        ui.label("");
        let mut caps = l.caps;
        let r = ui.checkbox(&mut caps, "Capitals");
        ui.end_row();
        change(pm, ed, &id, "Label capitals", &r, |x| {
            if let MapObject::Label(x) = x {
                x.caps = caps;
            }
        });
    });
    if ed.pause_assets.fonts.get(&l.font).is_none() {
        ui.colored_label(
            egui::Color32::from_rgb(230, 170, 90),
            format!(
                "{} isn't among the fonts: {} stands in. Add its file under Fonts (nothing selected).",
                l.font,
                ed.pause_assets.fonts.all()[0].name
            ),
        );
    }
}

fn building_ui(ui: &mut egui::Ui, pm: &mut PauseMap, ed: &mut Editor, b: &BuildingObject) {
    let id = b.common.id.clone();
    egui::Grid::new(("pause-building", &id)).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Shape");
        let mut shape = b.shape;
        let mut r = egui::ComboBox::from_id_salt("building-shape")
            .selected_text(BUILDING_SHAPES.iter().find(|(s, _)| *s == shape).unwrap().1)
            .show_ui(ui, |ui| {
                BUILDING_SHAPES.iter().any(|(s, name)| ui.selectable_value(&mut shape, *s, *name).changed())
            });
        if r.inner == Some(true) {
            r.response.mark_changed();
        }
        ui.end_row();
        change(pm, ed, &id, "Building shape", &r.response, |x| {
            if let MapObject::Building(x) = x {
                x.shape = shape;
            }
        });
        let station = b.shape == BuildingShape::Station;
        let fields: Vec<Field<BuildingObject>> = if station {
            vec![field!("Ring", b.w, 2.0..=60.0, 1.0..=400.0, 0.5, "", |x| &mut x.w)]
        } else {
            vec![
                field!("Width", b.w, 2.0..=120.0, 1.0..=400.0, 0.5, "", |x| &mut x.w),
                field!("Depth", b.h, 2.0..=120.0, 1.0..=400.0, 0.5, "", |x| &mut x.h),
            ]
        };
        for f in fields {
            let mut value = f.value;
            let r = number(ui, f.name, &mut value, f.soft, f.hard, f.step, f.unit);
            change_number(pm, ed, &id, "Resize", &r, |x| {
                if let MapObject::Building(x) = x {
                    *(f.field)(x) = value;
                    if station {
                        x.h = value;
                    }
                }
            });
        }
        let mut rot = b.rotation;
        let r = number(ui, "Rotation", &mut rot, -180.0..=180.0, -180.0..=180.0, 0.5, "°");
        change_number(pm, ed, &id, "Rotate", &r, |x| {
            if let MapObject::Building(x) = x {
                x.rotation = rot;
            }
        });
        let mut outline = b.outline;
        let r = number(ui, "Outline", &mut outline, 0.2..=3.0, 0.1..=10.0, 0.05, "");
        change_number(pm, ed, &id, "Building outline", &r, |x| {
            if let MapObject::Building(x) = x {
                x.outline = outline;
            }
        });
        if station {
            let mut dot = b.dot.unwrap_or(b.w.min(b.h) * 0.3);
            let r = number(ui, "Dot", &mut dot, 0.0..=20.0, 0.0..=100.0, 0.1, "");
            change_number(pm, ed, &id, "Station dot", &r, |x| {
                if let MapObject::Building(x) = x {
                    x.dot = Some(dot);
                }
            });
        }
    });
}

fn target_combo(ui: &mut egui::Ui, salt: &str, target: &mut Layer, with_relief: bool) -> egui::Response {
    let name = |l: Layer| match l {
        Layer::Text => "Labels",
        Layer::Lines => "Roads and lines",
        _ => "Fills",
    };
    let mut options = vec![Layer::Text, Layer::Lines];
    if with_relief {
        options.push(Layer::Relief);
    }
    let mut r = egui::ComboBox::from_id_salt(salt)
        .selected_text(name(*target))
        .show_ui(ui, |ui| options.iter().any(|l| ui.selectable_value(target, *l, name(*l)).changed()));
    if r.inner == Some(true) {
        r.response.mark_changed();
    }
    r.response
}

fn stamp_ui(ui: &mut egui::Ui, pm: &mut PauseMap, ed: &mut Editor, s: &StampObject) {
    let id = s.common.id.clone();
    if ed.pause_assets.pictures.get(&s.asset).is_none() {
        ui.colored_label(egui::Color32::from_rgb(230, 170, 90), "Its picture isn't in the project: choose it again.");
    }
    if ui.button("Another picture…").clicked()
        && let Some(path) =
            rfd::FileDialog::new().add_filter("Pictures", &["png", "jpg", "jpeg", "webp", "bmp", "gif"]).pick_file()
        && let Ok(bytes) = std::fs::read(path)
        && let Ok((asset, aspect)) = ed.add_pause_picture(bytes)
    {
        let mut d = pm.objects(ed).to_vec();
        if let Some(MapObject::Stamp(x)) = d.iter_mut().find(|o| o.id() == id) {
            (x.asset, x.aspect) = (asset, aspect);
        }
        pm.edit(ed, "Stamp picture", d);
    }
    egui::Grid::new(("pause-stamp", &id)).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Drawn as");
        let mut target = s.target;
        let r = target_combo(ui, "stamp-target", &mut target, false);
        ui.end_row();
        change(pm, ed, &id, "Stamp layer", &r, |x| {
            if let MapObject::Stamp(x) = x {
                x.target = target;
            }
        });
        let fields: [Field<StampObject>; 3] = [
            field!("Width", s.width, 8.0..=800.0, 2.0..=4000.0, 1.0, "", |x| &mut x.width),
            field!("Rotation", s.rotation, -180.0..=180.0, -180.0..=180.0, 0.5, "°", |x| &mut x.rotation),
            field!("Ink below", s.threshold, 0.0..=1.0, 0.0..=1.0, 0.01, "", |x| &mut x.threshold),
        ];
        for f in fields {
            let mut value = f.value;
            let r = number(ui, f.name, &mut value, f.soft, f.hard, f.step, f.unit);
            change_number(pm, ed, &id, "Stamp", &r, |x| {
                if let MapObject::Stamp(x) = x {
                    *(f.field)(x) = value;
                }
            });
        }
        ui.label("");
        let mut invert = s.invert;
        let r = ui.checkbox(&mut invert, "Light pixels are ink");
        ui.end_row();
        change(pm, ed, &id, "Stamp", &r, |x| {
            if let MapObject::Stamp(x) = x {
                x.invert = invert;
            }
        });
    });
}

fn erase_ui(ui: &mut egui::Ui, pm: &mut PauseMap, ed: &mut Editor, e: &EraseObject) {
    let id = e.common.id.clone();
    egui::Grid::new(("pause-erase", &id)).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Takes away");
        let mut target = e.target;
        let r = target_combo(ui, "erase-target", &mut target, true);
        ui.end_row();
        change(pm, ed, &id, "Eraser layer", &r, |x| {
            if let MapObject::Erase(x) = x {
                x.target = target;
            }
        });
        let mut radius = e.radius;
        let r = number(ui, "Radius", &mut radius, 2.0..=80.0, 1.0..=120.0, 0.5, "");
        change_number(pm, ed, &id, "Eraser size", &r, |x| {
            if let MapObject::Erase(x) = x {
                x.radius = radius;
            }
        });
    });
    ui.label(
        RichText::new("An eraser takes away the game's own marks under it, never what's drawn.")
            .color(theme::WEAK)
            .small(),
    );
}

/// Nothing selected: the tool's settings and the fonts.
fn tool_ui(ui: &mut egui::Ui, pm: &mut PauseMap, ed: &mut Editor, settings: &mut Settings) {
    let (title, tip) = match pm.tool {
        Tool::Select => ("Select", "Click a layer on the map, or drag a box around some."),
        Tool::Direct => ("Direct select", "Click a line to edit its anchors and handles."),
        Tool::Pen => ("Pen", "Click for corners, drag for curves; click the first anchor to close, Enter to end."),
        Tool::Text => ("Label", "Click the map to place a label."),
        Tool::Building => ("Building", "Click the map for a block, or drag its size."),
        Tool::Erase => ("Eraser", "Paint over the game's own marks to take them away."),
        Tool::Hand => ("Hand", "Drag the map."),
    };
    ui.label(RichText::new(title).font(theme::strong(theme::SIZE + 1.0)));
    ui.label(RichText::new(tip).color(theme::WEAK));
    egui::Grid::new("pause-tool").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| match pm.tool {
        Tool::Pen => {
            ui.label("Style");
            style_combo(ui, "pen-style", &mut pm.line_style);
            ui.end_row();
        }
        Tool::Erase => {
            ui.label("Takes away");
            target_combo(ui, "tool-erase", &mut pm.erase_target, true);
            ui.end_row();
            number(ui, "Radius", &mut pm.erase_radius, 2.0..=80.0, 2.0..=80.0, 0.5, "");
        }
        Tool::Building => {
            ui.label("Shape");
            egui::ComboBox::from_id_salt("tool-shape")
                .selected_text(BUILDING_SHAPES.iter().find(|(s, _)| *s == pm.building_shape).unwrap().1)
                .show_ui(ui, |ui| {
                    for (s, name) in BUILDING_SHAPES {
                        ui.selectable_value(&mut pm.building_shape, s, name);
                    }
                });
            ui.end_row();
        }
        Tool::Text => {
            ui.label("Preset");
            egui::ComboBox::from_id_salt("tool-preset").selected_text(LABEL_PRESETS[pm.label_preset].label).show_ui(
                ui,
                |ui| {
                    for (i, p) in LABEL_PRESETS.iter().enumerate() {
                        ui.selectable_value(&mut pm.label_preset, i, p.label).on_hover_text(p.note);
                    }
                },
            );
            ui.end_row();
            ui.label("Font");
            let mut font = pm.label_font.clone();
            font_combo(ui, "tool-font", &mut font, ed, &mut pm.font_previews);
            pm.label_font = font;
            ui.end_row();
        }
        _ => {}
    });
    ui.separator();
    ui.label(RichText::new("Fonts").font(theme::strong(theme::SIZE)));
    ui.label(
        RichText::new("Labels can use Zilla Slab, the game's own fonts, and TrueType or OpenType files added here (named after the file).")
            .color(theme::WEAK),
    );
    let added: Vec<(String, String)> = ed
        .pause_assets
        .fonts
        .all()
        .iter()
        .filter(|f| f.origin != "bundled" && f.origin != "game")
        .map(|f| (f.name.clone(), f.origin.clone()))
        .collect();
    for (name, path) in &added {
        ui.horizontal(|ui| {
            ui.label(name).on_hover_text(path);
            if ui.small_button("Remove").clicked() {
                ed.pause_assets.fonts.remove(name);
                ed.pause_assets.lettering.clear();
                pm.font_previews.forget(name);
                let list: Vec<String> =
                    settings.get(FONTS).unwrap_or_default().lines().filter(|l| l != path).map(str::to_owned).collect();
                settings.set(FONTS, list.join("\n"));
            }
        });
    }
    if ui.button("Add font files…").clicked()
        && let Some(files) = rfd::FileDialog::new().add_filter("Fonts", &["ttf", "otf"]).pick_files()
    {
        let mut list: Vec<String> = settings.get(FONTS).unwrap_or_default().lines().map(str::to_owned).collect();
        for f in files {
            let path = f.display().to_string();
            match std::fs::read(&f)
                .map_err(|e| e.to_string())
                .and_then(|d| Font::from_file(&file_font_name(&f), &path, d))
            {
                Ok(font) => {
                    pm.font_previews.forget(&font.name);
                    ed.pause_assets.fonts.add(font);
                    if !list.contains(&path) {
                        list.push(path);
                    }
                }
                Err(e) => log::warn!("{path}: {e}"),
            }
        }
        ed.pause_assets.lettering.clear();
        settings.set(FONTS, list.join("\n"));
    }
}
