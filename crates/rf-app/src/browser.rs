//! The Game Files pane: the archives as folders, and search over every path.

use eframe::egui;
use rf_formats::rpf8::FileRef;

use crate::game::{Folder, Game};

/// Most files a folder or a search lists at once.
const MAX_LISTED: usize = 3000;

#[derive(Default)]
pub struct Browser {
    search: String,
    searched: String,
    results: Vec<FileRef>,
    pub selected: Option<FileRef>,
}

/// What the user asked for.
pub enum BrowserAction {
    Open(FileRef),
    OpenXml(FileRef),
    OpenHex(FileRef),
}

fn file_colour(name: &str, ui: &egui::Ui) -> egui::Color32 {
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext {
        "ydr" | "ydd" | "yft" => egui::Color32::from_rgb(120, 180, 255),
        "ytd" => egui::Color32::from_rgb(230, 170, 90),
        "ymap" | "ytyp" => egui::Color32::from_rgb(140, 210, 130),
        "ybn" => egui::Color32::from_rgb(200, 140, 220),
        _ => ui.visuals().text_color(),
    }
}

impl Browser {
    pub fn ui(&mut self, ui: &mut egui::Ui, game: &Game) -> Option<BrowserAction> {
        let mut action = None;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(crate::theme::icon::MAGNIFYING_GLASS).color(crate::theme::WEAK));
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search all files…")
                    .desired_width(f32::INFINITY),
            );
        });
        let query = self.search.trim().to_lowercase();
        if query != self.searched {
            self.searched = query.clone();
            self.results.clear();
            if query.len() >= 2 {
                self.results =
                    game.paths.iter().filter(|(p, _)| p.contains(&query)).take(MAX_LISTED).map(|(_, f)| *f).collect();
            }
        }
        ui.separator();

        if !self.searched.is_empty() {
            ui.label(
                egui::RichText::new(format!(
                    "{} results{}",
                    self.results.len(),
                    if self.results.len() == MAX_LISTED { "+" } else { "" }
                ))
                .small(),
            );
            let row = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
            let results = std::mem::take(&mut self.results);
            egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, row, results.len(), |ui, range| {
                for &f in &results[range] {
                    let path = game.path(f);
                    let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
                    self.file_row(ui, &name, f, &mut action).on_hover_text(&path);
                }
            });
            self.results = results;
        } else {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                for (name, folder) in &game.tree.folders {
                    self.folder_ui(ui, name, name, folder, &mut action);
                }
            });
        }
        action
    }

    fn file_row(
        &mut self,
        ui: &mut egui::Ui,
        name: &str,
        f: FileRef,
        action: &mut Option<BrowserAction>,
    ) -> egui::Response {
        let text = egui::RichText::new(name).color(file_colour(name, ui));
        let r = ui.selectable_label(self.selected == Some(f), text);
        if r.clicked() {
            self.selected = Some(f);
        }
        if r.double_clicked() {
            *action = Some(BrowserAction::Open(f));
        }
        r.context_menu(|ui| {
            if ui.button("Open").clicked() {
                *action = Some(BrowserAction::Open(f));
                ui.close();
            }
            if ui.button("View as XML").clicked() {
                *action = Some(BrowserAction::OpenXml(f));
                ui.close();
            }
            if ui.button("View as hex").clicked() {
                *action = Some(BrowserAction::OpenHex(f));
                ui.close();
            }
            if ui.button("Copy path").clicked() {
                ui.ctx().copy_text(name.to_owned());
                ui.close();
            }
        });
        r
    }

    fn folder_ui(
        &mut self,
        ui: &mut egui::Ui,
        id: &str,
        name: &str,
        folder: &Folder,
        action: &mut Option<BrowserAction>,
    ) {
        let icon = if name.ends_with(".rpf") { crate::theme::icon::PACKAGE } else { crate::theme::icon::FOLDER };
        egui::CollapsingHeader::new(format!("{icon} {name}")).id_salt(id).show(ui, |ui| {
            for (sub, f) in &folder.folders {
                self.folder_ui(ui, &format!("{id}/{sub}"), sub, f, action);
            }
            for (fname, f) in folder.files.iter().take(MAX_LISTED) {
                self.file_row(ui, fname, *f, action);
            }
            if folder.files.len() > MAX_LISTED {
                ui.label(
                    egui::RichText::new(format!("… {} more, use the search", folder.files.len() - MAX_LISTED)).weak(),
                );
            }
        });
    }
}
