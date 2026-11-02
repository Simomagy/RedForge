//! The XML viewer: any file the formats convert, as RedForge's XML.

use std::sync::Arc;

use eframe::egui;
use rf_formats::formats::{self, XmlContext};
use rf_formats::rpf8::FileRef;

use super::{Job, status_ui};
use crate::game::Game;

pub struct TextViewer {
    pub title: String,
    pub path: String,
    job: Job<Vec<String>>,
}

impl TextViewer {
    /// Converts a game file to XML (without writing its textures anywhere).
    pub fn open_xml(ctx: &egui::Context, game: &Arc<Game>, file: FileRef) -> Self {
        let path = game.path(file);
        let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
        let g = game.clone();
        let n = name.clone();
        let job = Job::spawn(ctx, move || {
            let data = g.fs.extract_loose(file)?;
            let cx = XmlContext { hashes: &g.hashes, folder: None };
            let xml = formats::to_xml(&n, &data, &cx)?;
            Ok(xml.lines().map(str::to_owned).collect())
        });
        Self { title: format!("{name}.xml"), path, job }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.job.poll();
        let Job::Done(lines) = &self.job else {
            status_ui(ui, &self.job, &self.title);
            return;
        };
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&self.path).small().weak());
            if ui.button("Copy all").clicked() {
                ui.ctx().copy_text(lines.join("\n"));
            }
            ui.label(format!("{} lines", lines.len()));
        });
        ui.separator();
        let row = ui.text_style_height(&egui::TextStyle::Monospace);
        egui::ScrollArea::both().auto_shrink(false).show_rows(ui, row, lines.len(), |ui, range| {
            for line in &lines[range] {
                ui.label(egui::RichText::new(line).monospace());
            }
        });
    }
}
