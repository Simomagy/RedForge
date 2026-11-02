//! The Properties pane: the selected entity's placement and fields, the ymaps
//! being edited, and the archetype the Place tool places.
//!
//! Fields edit every selected entity (as in the C#); the placement edits the
//! one shown. A value dragged with the mouse is one undo step when let go.

use eframe::egui::{self, DragValue, RichText};
use glam::{EulerRot, Quat, Vec3};
use rf_editor::{Editor, EntityId, Placement};
use rf_formats::formats::databag::{DataBag, SchemaField, ValueType};
use rf_formats::formats::meta::rsc_schema;
use rf_formats::hash::joaat;

use crate::map::tools::Tools;

/// `RDR2EntityFlags`, by bit (the C#'s names; the others are unused).
const ENTITY_FLAGS: [(u32, &str); 24] = [
    (0, "Allow full rotation"),
    (1, "Stream low priority"),
    (2, "Disable embedded collisions"),
    (3, "LOD in parent map"),
    (4, "LOD adopt me"),
    (5, "Static entity"),
    (6, "Interior LOD"),
    (15, "LOD use alt fade"),
    (16, "Underwater"),
    (17, "Doesn't touch water"),
    (18, "Doesn't spawn peds"),
    (19, "Cast static shadows"),
    (20, "Cast dynamic shadows"),
    (21, "Ignore time settings"),
    (22, "Don't render shadows"),
    (23, "Only render shadows"),
    (24, "Don't render reflections"),
    (25, "Only render reflections"),
    (26, "Don't render water reflections"),
    (27, "Only render water reflections"),
    (28, "Don't render mirror reflections"),
    (29, "Only render mirror reflections"),
    (30, "Unused 30"),
    (31, "Unused 31"),
];

/// Fields shown elsewhere in the pane, or not editable here.
const SKIPPED: [&str; 6] = ["archetypeName", "position", "rotation", "scaleXY", "scaleZ", "extensions"];

#[derive(Default)]
pub struct Properties {
    /// A placement being dragged: committed when let go.
    pending: Option<(EntityId, Placement)>,
    /// A field being dragged.
    pending_field: Option<(u32, f64)>,
    archetype_text: String,
    archetype_for: Option<EntityId>,
    error: Option<String>,
}

impl Properties {
    pub fn ui(&mut self, ui: &mut egui::Ui, editor: &mut Editor, tools: Option<&mut Tools>) {
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            self.selection_ui(ui, editor);
            ui.separator();
            if let Some(tools) = tools {
                self.place_ui(ui, editor, tools);
                ui.separator();
            }
            if let Some(e) = &self.error {
                ui.separator();
                ui.colored_label(egui::Color32::from_rgb(230, 110, 90), e);
            }
        });
    }

    fn selection_ui(&mut self, ui: &mut egui::Ui, editor: &mut Editor) {
        let selection = editor.selection().to_vec();
        let Some(&id) = selection.last() else {
            ui.label(RichText::new("Nothing selected. Right click (or click) something in the map.").weak());
            return;
        };
        if selection.len() > 1 {
            ui.label(format!("{} selected; showing the last, fields change all of them", selection.len()));
        }
        let hashes = editor.hashes();
        let info = editor.world.entity(id);
        let map = editor.world.map_file_name(id.node).unwrap_or("?").to_owned();
        let edited = editor.map(id.node).is_some_and(|m| m.is_edited());
        ui.horizontal(|ui| {
            ui.label("ymap");
            ui.label(RichText::new(&map).strong());
            if edited {
                ui.label(RichText::new("edited").color(egui::Color32::from_rgb(255, 160, 40)));
            }
        });
        if let Some((grass, batch, instance)) = id.batch_instance() {
            let kind = if grass { "grass" } else { "props" };
            ui.label(format!("Instance {instance} of the {kind} batch {batch}"));
            let name = info.map(|e| hashes.name(e.archetype)).unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label("Archetype");
                ui.label(RichText::new(name).strong());
            });
            ui.label(
                RichText::new(if grass {
                    "Instanced grass moves within its batch's box, turns about Z and scales evenly."
                } else {
                    "An instance of its batch: its archetype is the batch's."
                })
                .weak(),
            );
            self.placement_ui(ui, editor, id);
            let Some(def) = editor.entity_def(id).cloned() else { return };
            ui.add_space(4.0);
            egui::Grid::new("fields").num_columns(2).striped(true).show(ui, |ui| {
                for f in def.class.fields().to_vec() {
                    let name = editor.hashes().name(f.name);
                    if f.name == 0x100 || ["InstMat", "Position"].contains(&name.as_str()) {
                        continue;
                    }
                    if self.field_ui(ui, editor, &def, &f, &name) {
                        ui.end_row();
                    }
                }
            });
            return;
        }
        if id.interior_entity != u32::MAX {
            let mlo = editor.entity_def(rf_editor::EntityId { interior_entity: u32::MAX, ..id });
            let name = mlo.map(|d| hashes.name(d.hash("archetypeName"))).unwrap_or_default();
            ui.label(format!("Entity {} of the interior {name} (entity {})", id.interior_entity, id.entity));
            ui.label(RichText::new("Changes go to the interior's ytyp: every place it's in.").weak());
        } else {
            ui.label(format!("Entity {}", id.entity));
        }
        let _ = info;

        // The archetype, by name.
        if self.archetype_for != Some(id) {
            self.archetype_for = Some(id);
            self.archetype_text =
                editor.entity_def(id).map(|d| hashes.name(d.hash("archetypeName"))).unwrap_or_default();
        }
        ui.horizontal(|ui| {
            ui.label("Archetype");
            let r = ui.text_edit_singleline(&mut self.archetype_text);
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let h = parse_hash(&self.archetype_text);
                if editor.data().archetypes.contains_key(&h) {
                    editor.modify("Archetype", |d| d.set_hash("archetypeName", h));
                    self.error = None;
                } else {
                    self.error = Some(format!("No archetype called {}", self.archetype_text));
                }
            }
        });

        self.placement_ui(ui, editor, id);
        let Some(def) = editor.entity_def(id).cloned() else { return };
        ui.add_space(4.0);
        egui::Grid::new("fields").num_columns(2).striped(true).show(ui, |ui| {
            for f in def.class.fields().to_vec() {
                let name = editor.hashes().name(f.name);
                if f.name == 0x100 || SKIPPED.contains(&name.as_str()) {
                    continue;
                }
                if self.field_ui(ui, editor, &def, &f, &name) {
                    ui.end_row();
                }
            }
        });
    }

    fn placement_ui(&mut self, ui: &mut egui::Ui, editor: &mut Editor, id: EntityId) {
        let Some(saved) = editor.placement(id) else { return };
        let mut p = match self.pending {
            Some((pid, p)) if pid == id => p,
            _ => saved,
        };
        let (z, y, x) = p.orientation.to_euler(EulerRot::ZYX);
        let mut euler = Vec3::new(x, y, z).to_array().map(f32::to_degrees);
        let mut pos = p.position.to_array();
        let mut scale = [p.scale.x, p.scale.z];
        let mut changed = false;
        let mut released = false;
        let mut row = |ui: &mut egui::Ui, label: &str, values: &mut [f32], speed: f64| {
            ui.horizontal(|ui| {
                ui.label(label);
                for v in values.iter_mut() {
                    let r = ui.add(DragValue::new(v).speed(speed).max_decimals(4));
                    changed |= r.changed();
                    released |= r.drag_stopped() || (r.changed() && !r.dragged());
                }
            });
        };
        row(ui, "Position", &mut pos, 0.05);
        row(ui, "Rotation °", &mut euler, 0.5);
        row(ui, "Scale XY, Z", &mut scale, 0.01);
        if changed {
            let [x, y, z] = euler.map(f32::to_radians);
            p = Placement {
                position: Vec3::from(pos),
                orientation: Quat::from_euler(EulerRot::ZYX, z, y, x),
                scale: Vec3::new(scale[0], scale[0], scale[1]),
            };
            self.pending = Some((id, p));
            editor.preview(&[(id, p)]);
        }
        if released && let Some((pid, p)) = self.pending.take() {
            editor.set_placements("Placement", &[(pid, p)]);
        }
    }

    /// One field's row; false when it isn't shown.
    fn field_ui(&mut self, ui: &mut egui::Ui, editor: &mut Editor, def: &DataBag, f: &SchemaField, name: &str) -> bool {
        let key = f.name;
        let label = name.to_owned();
        // An entity's flags are a plain number in the schema.
        let entity_flags = name == "flags" && def.class_name() == joaat("CEntityDef");
        let data_type = if entity_flags { ValueType::Flags32 } else { f.data_type };
        match data_type {
            ValueType::Float => {
                let v = self.pending_value(key).map_or(def.f32(key), |v| v as f32);
                let mut x = v;
                ui.label(name);
                let r = ui.add(DragValue::new(&mut x).speed(0.1).max_decimals(3));
                self.number_edit(editor, &r, key, x as f64, &label, move |d, v| d.set_f32(key, v as f32));
            }
            ValueType::UInt8
            | ValueType::UInt16
            | ValueType::UInt32
            | ValueType::Int8
            | ValueType::Int16
            | ValueType::Int32 => {
                let raw: i64 = match f.data_type {
                    ValueType::UInt8 => def.u8(key) as i64,
                    ValueType::UInt16 => def.u16(key) as i64,
                    ValueType::UInt32 => def.u32(key) as i64,
                    ValueType::Int8 => def.i8(key) as i64,
                    ValueType::Int16 => def.i16(key) as i64,
                    _ => def.i32(key) as i64,
                };
                let mut x = self.pending_value(key).map_or(raw, |v| v as i64);
                let t = f.data_type;
                ui.label(name);
                let r = ui.add(DragValue::new(&mut x).speed(0.2));
                self.number_edit(editor, &r, key, x as f64, &label, move |d, v| {
                    let v = v as i64;
                    match t {
                        ValueType::UInt8 => d.set_u8(key, v as u8),
                        ValueType::UInt16 => d.set_u16(key, v as u16),
                        ValueType::UInt32 => d.set_u32(key, v as u32),
                        ValueType::Int8 => d.set_i8(key, v as i8),
                        ValueType::Int16 => d.set_i16(key, v as i16),
                        _ => d.set_i32(key, v as i32),
                    }
                });
            }
            ValueType::UInt64 | ValueType::Int64 => {
                ui.label(name);
                let mut text = format!("0x{:016X}", def.u64(key));
                let r = ui.text_edit_singleline(&mut text);
                if r.lost_focus() {
                    let t = text.trim().trim_start_matches("0x").trim_start_matches("0X");
                    if let Ok(v) = u64::from_str_radix(t, 16)
                        && v != def.u64(key)
                    {
                        editor.modify(&label, |d| d.set_u64(key, v));
                    }
                }
            }
            ValueType::Boolean => {
                ui.label(name);
                let mut b = def.bool(key);
                if ui.checkbox(&mut b, "").changed() {
                    editor.modify(&label, |d| d.set_bool(key, b));
                }
            }
            ValueType::Hash => {
                ui.label(name);
                let mut text = editor.hashes().name(def.hash(key));
                if ui.text_edit_singleline(&mut text).lost_focus() {
                    let h = parse_hash(&text);
                    if h != def.hash(key) {
                        editor.modify(&label, |d| d.set_hash(key, h));
                    }
                }
            }
            ValueType::Enum8 | ValueType::Enum16 | ValueType::Enum32 => {
                ui.label(name);
                let v = def.enum_value(key);
                let values = rsc_schema().enumeration(f.type_name).and_then(|e| e.values.clone()).unwrap_or_default();
                let value_name = |v: i64| {
                    values.iter().find(|e| e.value == v).map_or_else(|| v.to_string(), |e| editor.hashes().name(e.name))
                };
                let mut chosen = v;
                egui::ComboBox::from_id_salt(("enum", key)).selected_text(value_name(v)).show_ui(ui, |ui| {
                    for e in &values {
                        ui.selectable_value(&mut chosen, e.value, value_name(e.value));
                    }
                });
                if chosen != v {
                    editor.modify(&label, |d| d.set_enum_value(key, chosen));
                }
            }
            ValueType::Flags32 => {
                ui.label(name);
                let flags = def.u32(key);
                ui.collapsing(format!("0x{flags:08X}"), |ui| {
                    let bits: Vec<(u32, String)> = if entity_flags {
                        ENTITY_FLAGS.iter().map(|(b, n)| (*b, n.to_string())).collect()
                    } else {
                        (0..32).map(|b| (b, format!("bit {b}"))).collect()
                    };
                    for (bit, text) in bits {
                        let mut on = flags & (1 << bit) != 0;
                        if ui.checkbox(&mut on, text).changed() {
                            // Only this bit changes, in every selected entity.
                            editor.modify(&label, |d| {
                                let f = d.u32(key);
                                d.set_u32(key, if on { f | (1 << bit) } else { f & !(1 << bit) });
                            });
                        }
                    }
                    if entity_flags {
                        let other = flags & !ENTITY_FLAGS.iter().fold(0, |m, (b, _)| m | (1 << b));
                        if other != 0 {
                            ui.label(RichText::new(format!("Other bits: 0x{other:08X}")).weak());
                        }
                    }
                });
            }
            _ => return false,
        }
        true
    }

    fn pending_value(&self, key: u32) -> Option<f64> {
        self.pending_field.filter(|(k, _)| *k == key).map(|(_, v)| v)
    }

    /// A number field's edit: shown while dragged, one step when let go.
    fn number_edit(
        &mut self,
        editor: &mut Editor,
        r: &egui::Response,
        key: u32,
        value: f64,
        label: &str,
        set: impl Fn(&mut DataBag, f64),
    ) {
        if r.changed() {
            self.pending_field = Some((key, value));
        }
        if (r.drag_stopped() || (r.changed() && !r.dragged()) || r.lost_focus())
            && let Some((k, v)) = self.pending_field.take()
            && k == key
        {
            editor.modify(label, |d| set(d, v));
        }
    }

    /// The archetype the Place tool places (chosen in Objects).
    fn place_ui(&mut self, ui: &mut egui::Ui, editor: &mut Editor, tools: &mut Tools) {
        ui.heading("Place");
        match tools.place_archetype {
            Some(a) => {
                ui.label(format!("Archetype: {}", editor.hashes().name(a)));
            }
            None => {
                ui.label(
                    RichText::new("Choose an archetype in Objects (View → Objects), then click in the map.").weak(),
                );
            }
        }
    }
}

/// A name or a `0x…` hash.
fn parse_hash(text: &str) -> u32 {
    let t = text.trim();
    match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16).unwrap_or(0),
        None => joaat(&t.to_lowercase()),
    }
}
