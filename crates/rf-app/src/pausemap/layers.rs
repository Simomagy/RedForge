//! The Map layers pane: the pause map's layers as a tree (groups open and
//! closed), top of the list on top of the map; selecting, showing, hiding,
//! locking, renaming, and dragging layers above, below or into a group.

use eframe::egui::{self, Color32, Sense, Stroke, Vec2};
use rf_editor::Editor;
use rf_editor::pausemap::commands::{self, Where};
use rf_editor::pausemap::doc::children_of;
use rf_editor::pausemap::project::{MapObject, object_name};

use super::PauseMap;
use crate::theme::{self, icon};

/// A layer being renamed.
struct Renaming {
    id: String,
    /// The name so far.
    text: String,
    /// The field took the keyboard (it's asked for once: asked for every
    /// frame, it never lets go).
    focused: bool,
}

/// The pane's own state.
#[derive(Default)]
pub struct Layers {
    /// The layer being renamed, and its name so far.
    renaming: Option<Renaming>,
    /// The layers being dragged.
    dragging: Option<Vec<String>>,
    /// Where they'd go.
    drop: Option<(String, Where)>,
}

fn glyph(o: &MapObject) -> &'static str {
    match o {
        MapObject::Line(_) => icon::PATH,
        MapObject::Label(_) => icon::TEXT_T,
        MapObject::Building(_) => icon::HOUSE_LINE,
        MapObject::Stamp(_) => icon::IMAGE,
        MapObject::Erase(_) => icon::ERASER,
        MapObject::Group(g) if g.collapsed => icon::FOLDER,
        MapObject::Group(_) => icon::FOLDER_OPEN,
    }
}

impl Layers {
    pub fn ui(&mut self, ui: &mut egui::Ui, pm: &mut PauseMap, ed: &mut Editor) {
        if ui.input(|i| i.pointer.any_pressed()) && ui.rect_contains_pointer(ui.max_rect()) {
            pm.active = true;
        }
        let objects = pm.objects(ed).to_vec();
        ui.horizontal(|ui| {
            let any = !pm.selection.is_empty();
            if ui.add_enabled(any, egui::Button::new(theme::icon_text(icon::FOLDER_PLUS, "Group"))).clicked()
                && let Some((d, sel)) = commands::group(&objects, &pm.selection)
            {
                pm.edit(ed, "Group", d);
                pm.select(sel);
            }
            if ui
                .add_enabled(any, egui::Button::new(theme::icon_text(icon::TRASH, "")))
                .on_hover_text("Delete")
                .clicked()
            {
                super::pane::delete(pm, ed);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Show all").clicked() {
                    pm.edit(ed, "Show all", commands::show_all(&objects));
                }
                if ui.small_button("Unlock all").clicked() {
                    pm.edit(ed, "Unlock all", commands::unlock_all(&objects));
                }
            });
        });
        ui.separator();
        if objects.is_empty() {
            ui.label(egui::RichText::new("Nothing is drawn on the pause map yet. Draw in the Pause map tab, or import a Map Editor project there.").color(theme::WEAK));
            return;
        }
        pm.list_hover = None;
        let released = ui.input(|i| i.pointer.any_released());
        self.drop = None;
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            self.tree(ui, pm, ed, &objects, None, 0);
        });
        if released {
            if let (Some(ids), Some((target, at))) = (self.dragging.take(), self.drop.take())
                && let Some(d) = commands::move_layers(&objects, &ids, &target, at)
            {
                pm.edit(ed, "Move layers", d);
            }
            self.dragging = None;
        }
    }

    fn tree(
        &mut self,
        ui: &mut egui::Ui,
        pm: &mut PauseMap,
        ed: &mut Editor,
        objects: &[MapObject],
        parent: Option<&str>,
        depth: usize,
    ) {
        // Top of the list first: the last in the order.
        let children: Vec<MapObject> = children_of(parent, objects).into_iter().rev().cloned().collect();
        for o in children {
            self.row(ui, pm, ed, objects, &o, depth);
            if let MapObject::Group(g) = &o
                && !g.collapsed
            {
                self.tree(ui, pm, ed, objects, Some(o.id()), depth + 1);
            }
        }
    }

    fn row(
        &mut self,
        ui: &mut egui::Ui,
        pm: &mut PauseMap,
        ed: &mut Editor,
        objects: &[MapObject],
        o: &MapObject,
        depth: usize,
    ) {
        let id = o.id().to_owned();
        let selected = pm.selection.contains(&id);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), theme::ROW), Sense::click_and_drag());
        let painter = ui.painter().clone();
        if selected {
            painter.rect_filled(rect, theme::RADIUS, theme::WIDGET);
            painter.rect_filled(egui::Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height())), 2, theme::ACCENT);
        } else if resp.hovered() {
            painter.rect_filled(rect, theme::RADIUS, theme::HOVER.gamma_multiply(0.6));
        }
        if resp.hovered() {
            pm.list_hover = Some(id.clone());
        }
        let font = egui::FontId::proportional(theme::SIZE);
        let y = rect.center().y;
        let mut x = rect.min.x + 8.0 + depth as f32 * 16.0;
        // A group's caret opens and closes it.
        if let MapObject::Group(g) = o {
            let caret = egui::Rect::from_center_size(egui::pos2(x + 6.0, y), Vec2::splat(16.0));
            let c = ui.interact(caret, resp.id.with("caret"), Sense::click());
            painter.text(
                caret.center(),
                egui::Align2::CENTER_CENTER,
                if g.collapsed { icon::CARET_RIGHT } else { icon::CARET_DOWN },
                font.clone(),
                theme::WEAK,
            );
            if c.clicked() {
                let mut d = objects.to_vec();
                if let Some(MapObject::Group(g)) = d.iter_mut().find(|x| x.id() == id) {
                    g.collapsed = !g.collapsed;
                }
                pm.edit(ed, "Open or close group", d);
            }
        }
        x += 16.0;
        let dim = o.common().hidden;
        let colour = if dim { theme::WEAK } else { theme::TEXT };
        painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            glyph(o),
            font.clone(),
            if selected { theme::ACCENT } else { theme::WEAK },
        );
        x += theme::SIZE + 8.0;
        // The eye and the lock, on the right.
        let right = rect.max.x - 8.0;
        let toggle = |ui: &mut egui::Ui, at: f32, on: bool, on_glyph: &str, off_glyph: &str, salt: &str| {
            let r = egui::Rect::from_center_size(egui::pos2(at, y), Vec2::splat(18.0));
            let c = ui.interact(r, resp.id.with(salt), Sense::click());
            let shown = c.hovered() || !on || salt == "eye";
            if shown {
                painter.text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    if on { on_glyph } else { off_glyph },
                    font.clone(),
                    if c.hovered() { theme::TEXT } else { theme::WEAK },
                );
            }
            c.clicked()
        };
        if toggle(ui, right - 8.0, !o.common().hidden, icon::EYE, icon::EYE_SLASH, "eye") {
            pm.edit(ed, "Hide", commands::toggle_hidden(objects, std::slice::from_ref(&id)));
        }
        if toggle(ui, right - 30.0, o.common().locked, icon::LOCK, icon::LOCK_OPEN, "lock") {
            pm.edit(ed, "Lock", commands::toggle_lock(objects, std::slice::from_ref(&id)));
            pm.keep_selection(ed);
        }
        // The name, or its field while renaming.
        let renaming = self.renaming.as_ref().is_some_and(|r| r.id == id);
        if renaming {
            let field =
                egui::Rect::from_min_max(egui::pos2(x, rect.min.y + 2.0), egui::pos2(right - 44.0, rect.max.y - 2.0));
            let edit = self.renaming.as_mut().unwrap();
            let r = ui.put(field, egui::TextEdit::singleline(&mut edit.text));
            if !edit.focused {
                r.request_focus();
                edit.focused = true;
            }
            // Enter, Tab or a click elsewhere keeps the name; Esc doesn't.
            let clicked_away = ui.input(|i| i.pointer.any_pressed()) && !r.hovered();
            if r.lost_focus() || (clicked_away && !r.has_focus()) {
                let edit = self.renaming.take().unwrap();
                // Unchanged, it makes no step (and keeps an automatic name).
                let same = edit.text.trim() == object_name(o).as_str();
                if !same && !ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    let mut d = objects.to_vec();
                    if let Some(x) = d.iter_mut().find(|x| x.id() == id) {
                        let name = edit.text.trim();
                        x.common_mut().name = (!name.is_empty()).then(|| name.to_owned());
                    }
                    pm.edit(ed, "Rename", d);
                }
            }
        } else {
            let galley = painter.layout_no_wrap(object_name(o), font.clone(), colour);
            let clip = egui::Rect::from_min_max(rect.min, egui::pos2(right - 44.0, rect.max.y));
            painter.with_clip_rect(clip).galley(egui::pos2(x, y - galley.size().y / 2.0), galley, colour);
        }

        if resp.double_clicked() {
            self.renaming = Some(Renaming { id: id.clone(), text: object_name(o), focused: false });
        } else if resp.clicked() {
            let mods = ui.input(|i| i.modifiers);
            if mods.command || mods.shift {
                if selected {
                    pm.selection.retain(|s| *s != id);
                } else {
                    pm.selection.push(id.clone());
                }
                pm.nodes.clear();
            } else {
                pm.select(vec![id.clone()]);
            }
            pm.focus_on = Some(id.clone());
        }
        // Dragging layers to another place in the list.
        if resp.drag_started() {
            let ids = if selected { pm.selection.clone() } else { vec![id.clone()] };
            self.dragging = Some(ids);
        }
        if let Some(ids) = &self.dragging
            && let Some(at) = ui.input(|i| i.pointer.interact_pos())
            && rect.contains(at)
            && !ids.contains(&id)
        {
            let f = (at.y - rect.min.y) / rect.height();
            let place = match o {
                MapObject::Group(_) if (0.3..0.7).contains(&f) => Where::Into,
                _ if f < 0.5 => Where::Above,
                _ => Where::Below,
            };
            let line = Stroke::new(2.0, theme::ACCENT);
            match place {
                Where::Above => {
                    painter.hline(rect.x_range(), rect.min.y, line);
                }
                Where::Below => {
                    painter.hline(rect.x_range(), rect.max.y, line);
                }
                Where::Into => {
                    painter.rect_stroke(rect, theme::RADIUS, line, egui::StrokeKind::Inside);
                }
            }
            self.drop = Some((id.clone(), place));
        }
        if self.dragging.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        let _ = Color32::TRANSPARENT;
    }
}
