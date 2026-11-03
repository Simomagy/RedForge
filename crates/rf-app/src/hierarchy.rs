//! The Hierarchy pane: the objects of the ymap (or the interior) the
//! selection is in, or of a ymap picked in the Project pane until the
//! selection changes, as the C# RedForge shows them: grouped (a ymap's by LOD
//! level, an interior's by room and entity set), searchable, each with an eye
//! that hides it in the map view.

use std::collections::HashSet;

use eframe::egui::{self, RichText};
use rf_editor::{Editor, EntityId, lod};
use rf_formats::formats::databag::{DataBag, Value};

use crate::map::tools::Tools;
use crate::theme::{self, icon};

/// A row: a group's header, or an object.
#[derive(Clone)]
enum Row {
    Group { name: String, count: usize },
    Object { id: EntityId, name: String, index: u32, group: String },
}

/// What the rows were made from, to make them again only when it changes.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Source {
    node: u32,
    /// The interior instance, when showing an interior.
    interior: Option<u32>,
    /// The data's address: it changes with every edit.
    data: usize,
}

#[derive(Default)]
pub struct Hierarchy {
    source: Option<Source>,
    title: String,
    objects: Vec<Row>,
    filter: String,
    collapsed: HashSet<String>,
    /// A ymap picked in the Project pane, and the selection's last object
    /// then: it's shown until that changes.
    picked: Option<(u32, Option<EntityId>)>,
}

fn lod_group(lod_level: i64) -> &'static str {
    match lod_level {
        0 => "HD",
        1 => "LOD",
        2 => "SLOD1",
        3 => "SLOD2",
        4 => "SLOD3",
        6 => "SLOD4",
        _ => "Objects",
    }
}

fn u32s(bag: &DataBag, name: &str) -> Vec<u32> {
    match bag.object(name) {
        Some(Value::U32s(v) | Value::Hashes(v)) => v.clone(),
        _ => Vec::new(),
    }
}

impl Hierarchy {
    /// Shows a ymap's objects (by name hash), until the selection changes.
    pub fn show_map(&mut self, node: u32, editor: &Editor) {
        self.picked = Some((node, editor.selection().last().copied()));
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, editor: &mut Editor, tools: &mut Tools) {
        let last = editor.selection().last().copied();
        if self.picked.is_some_and(|(node, at)| at != last || editor.map(node).is_none()) {
            self.picked = None;
        }
        let shown = match (self.picked, last) {
            (Some((node, _)), _) => EntityId { node, entity: 0, interior_entity: u32::MAX },
            (None, Some(id)) => id,
            (None, None) => {
                ui.label(RichText::new("Select an object on the map, or a ymap in the project").weak());
                self.source = None;
                return;
            }
        };
        self.refresh(editor, shown);
        ui.horizontal(|ui| {
            ui.label(RichText::new(&self.title).strong());
            if ui.small_button("Show all").on_hover_text("Show the hidden objects again").clicked() {
                editor.world.show_all();
            }
        });
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Search objects"));
        let filter = self.filter.trim().to_lowercase();

        // The rows to show: groups, and the objects of the open ones (all when searching).
        let mut rows: Vec<Row> = Vec::new();
        let mut i = 0;
        while i < self.objects.len() {
            let Row::Group { name, .. } = &self.objects[i] else {
                i += 1;
                continue;
            };
            let group = name.clone();
            let members: Vec<&Row> = self.objects[i + 1..]
                .iter()
                .take_while(|r| matches!(r, Row::Object { .. }))
                .filter(|r| match r {
                    Row::Object { name, .. } => filter.is_empty() || name.contains(&filter),
                    _ => false,
                })
                .collect();
            let count = self.objects[i + 1..].iter().take_while(|r| matches!(r, Row::Object { .. })).count();
            if !members.is_empty() {
                rows.push(Row::Group { name: group.clone(), count: members.len() });
                if !self.collapsed.contains(&group) || !filter.is_empty() {
                    rows.extend(members.into_iter().cloned());
                }
            }
            i += 1 + count;
        }

        let row_height = theme::ROW;
        let selection: HashSet<EntityId> = editor.selection().iter().copied().collect();
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show_rows(ui, row_height, rows.len(), |ui, range| {
            for row in &rows[range] {
                match row {
                    Row::Group { name, count } => {
                        let open = !self.collapsed.contains(name);
                        if theme::section(ui, open, icon::STACK, name, *count).clicked() {
                            if open {
                                self.collapsed.insert(name.clone());
                            } else {
                                self.collapsed.remove(name);
                            }
                        }
                    }
                    Row::Object { id, name, index, .. } => {
                        ui.horizontal(|ui| {
                            let hidden = editor.world.is_hidden(*id);
                            let (eye, tip) = if hidden { (icon::EYE_SLASH, "Show") } else { (icon::EYE, "Hide") };
                            let eye = egui::Button::new(RichText::new(eye).color(theme::WEAK)).frame(false);
                            if ui.add(eye).on_hover_text(tip).clicked() {
                                editor.world.set_hidden(*id, !hidden);
                            }
                            let note = if hidden { format!("#{index}  hidden") } else { format!("#{index}") };
                            let r = theme::row(ui, selection.contains(id), icon::CUBE, name, &note);
                            if r.clicked() {
                                if ui.input(|i| i.modifiers.command) {
                                    editor.toggle(*id);
                                } else {
                                    editor.select(vec![*id]);
                                }
                            }
                            if r.double_clicked() {
                                tools.focus = true;
                            }
                        });
                    }
                }
            }
        });
    }

    /// Makes the rows again when the selection's ymap or interior, or its
    /// data, changed.
    fn refresh(&mut self, editor: &Editor, id: EntityId) {
        let hashes = editor.hashes();
        let in_interior = id.interior_entity != u32::MAX;
        let mlo = editor.interior_archetype(id).filter(|_| in_interior);
        let Some(map) = editor.map(id.node) else { return };
        let data = match mlo {
            Some(m) => m as *const DataBag as usize,
            None => std::sync::Arc::as_ptr(&map.root) as usize,
        };
        let source = Source { node: id.node, interior: in_interior.then_some(id.entity), data };
        if self.source == Some(source) {
            return;
        }
        self.source = Some(source);
        let mut objects: Vec<Row> = Vec::new();
        match mlo {
            Some(mlo) => {
                let instance = lod::entities(&map.root).get(id.entity as usize).and_then(Option::as_ref);
                let name = instance.map(|d| hashes.name(d.hash("archetypeName"))).unwrap_or_default();
                self.title = format!("Interior: {name}");
                // Plain entities by the room they're attached to.
                let mut room_of: Vec<Option<String>> = vec![None; lod::entities(mlo).len()];
                for room in mlo.bags("rooms").unwrap_or_default().iter().flatten() {
                    let room_name = room.string("name").unwrap_or_default().to_owned();
                    for i in u32s(room, "attachedObjects") {
                        if let Some(slot) = room_of.get_mut(i as usize) {
                            slot.get_or_insert_with(|| room_name.clone());
                        }
                    }
                }
                let mut k = 0u32;
                for (i, e) in lod::entities(mlo).iter().enumerate() {
                    let group = room_of[i].clone().unwrap_or_else(|| "Objects".into());
                    if let Some(e) = e {
                        let id = EntityId { interior_entity: k, ..id };
                        objects.push(Row::Object { id, name: hashes.name(e.hash("archetypeName")), index: k, group });
                    }
                    k += 1;
                }
                for set in mlo.bags("entitySets").unwrap_or_default().iter().flatten() {
                    let group = format!("Set: {}", hashes.name(set.hash("name")));
                    for e in lod::entities(set) {
                        if let Some(e) = e {
                            let id = EntityId { interior_entity: k, ..id };
                            let name = hashes.name(e.hash("archetypeName"));
                            objects.push(Row::Object { id, name, index: k, group: group.clone() });
                        }
                        k += 1;
                    }
                }
            }
            None => {
                self.title = format!("{}.ymap", map.file_name);
                for (i, e) in lod::entities(&map.root).iter().enumerate() {
                    let Some(e) = e else { continue };
                    let group = if rf_editor::placement::is_mlo_instance(e) {
                        "Interiors".to_owned()
                    } else {
                        lod_group(e.enum_value("lodLevel")).to_owned()
                    };
                    let id = EntityId { node: id.node, entity: i as u32, interior_entity: u32::MAX };
                    objects.push(Row::Object {
                        id,
                        name: hashes.name(e.hash("archetypeName")),
                        index: i as u32,
                        group,
                    });
                }
            }
        }
        // Grouped: "Objects" first, then by name; each group's header before its rows.
        let mut groups: Vec<String> = Vec::new();
        for r in &objects {
            if let Row::Object { group, .. } = r
                && !groups.contains(group)
            {
                groups.push(group.clone());
            }
        }
        groups.sort_by_key(|g| (g != "Objects", g.to_lowercase()));
        self.objects.clear();
        for g in groups {
            let members: Vec<Row> = objects
                .iter()
                .filter(|r| matches!(r, Row::Object { group, .. } if *group == g))
                .map(|r| match r {
                    Row::Object { id, name, index, group } => {
                        Row::Object { id: *id, name: name.to_lowercase(), index: *index, group: group.clone() }
                    }
                    other => other.clone(),
                })
                .collect();
            self.objects.push(Row::Group { name: g, count: members.len() });
            self.objects.extend(members);
        }
    }
}
