//! The Game Files pane: the archives as folders, and search over every path
//! (on a thread of its own: half a million paths take a moment).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, channel};

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
    /// The search running, and which one it is (a newer one stops it).
    running: Option<Receiver<Vec<FileRef>>>,
    generation: Arc<AtomicU64>,
    pub selected: Option<FileRef>,
    /// "Copy path" was clicked on this file.
    copy_path: Option<FileRef>,
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
    /// Starts with a search (`REDFORGE_FILES_SEARCH`, for testing).
    pub fn with_search(search: String) -> Self {
        Self { search, ..Default::default() }
    }

    /// Searches every path for all the words of `query`, on a thread; a
    /// newer search stops it.
    fn search(&mut self, ctx: &egui::Context, game: &Arc<Game>, query: &str) {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let words: Vec<String> = query.split_whitespace().map(str::to_owned).collect();
        if words.iter().map(String::len).sum::<usize>() < 2 {
            self.running = None;
            return;
        }
        let (tx, rx) = channel();
        self.running = Some(rx);
        let (game, latest, ctx) = (game.clone(), self.generation.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut found = Vec::new();
            for (i, (path, f)) in game.paths.iter().enumerate() {
                if i % 8192 == 0 && latest.load(Ordering::Relaxed) != generation {
                    return;
                }
                if words.iter().all(|w| path.contains(w.as_str())) {
                    found.push(*f);
                    if found.len() == MAX_LISTED {
                        break;
                    }
                }
            }
            let _ = tx.send(found);
            ctx.request_repaint();
        });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, game: &Arc<Game>) -> Option<BrowserAction> {
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
            self.search(ui.ctx(), game, &query);
        }
        if let Some(rx) = &self.running
            && let Ok(found) = rx.try_recv()
        {
            self.results = found;
            self.running = None;
        }
        ui.separator();

        if !self.searched.is_empty() {
            let count = if self.running.is_some() {
                "Searching…".to_owned()
            } else {
                format!("{} results{}", self.results.len(), if self.results.len() == MAX_LISTED { "+" } else { "" })
            };
            ui.label(egui::RichText::new(count).small());
            let row = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
            let results = std::mem::take(&mut self.results);
            egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, row, results.len(), |ui, range| {
                for &f in &results[range] {
                    let path = game.path(f);
                    let (folder, name) = path.rsplit_once('/').unwrap_or(("", &path));
                    // Files of the same name apart: where each is, dimmed.
                    ui.horizontal(|ui| {
                        self.file_row(ui, name, f, &mut action).on_hover_text(&path);
                        let folder = folder.rsplit('/').next().unwrap_or(folder);
                        ui.add(egui::Label::new(egui::RichText::new(folder).small().weak()).truncate());
                    });
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
        if let Some(f) = self.copy_path.take() {
            ui.ctx().copy_text(game.path(f));
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
                self.copy_path = Some(f);
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
