//! The hex viewer: any game file's bytes (decrypted and decompressed), 16 to a
//! row with their text, as the C# RedForge's `HexViewer` shows them.

use std::sync::Arc;

use eframe::egui;
use rf_formats::rpf8::FileRef;

use super::{Job, status_ui};
use crate::game::Game;

pub struct HexViewer {
    pub title: String,
    pub path: String,
    job: Job<Vec<u8>>,
    /// Where to scroll to, typed as hex.
    goto: String,
}

/// One row: offset, bytes and their text.
fn row(data: &[u8], start: usize) -> String {
    let bytes = &data[start..(start + 16).min(data.len())];
    let mut s = format!("{start:08X}  ");
    for i in 0..16 {
        match bytes.get(i) {
            Some(b) => s.push_str(&format!("{b:02X} ")),
            None => s.push_str("   "),
        }
        if i == 7 {
            s.push(' ');
        }
    }
    s.push(' ');
    s.extend(bytes.iter().map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '.' }));
    s
}

impl HexViewer {
    pub fn open(ctx: &egui::Context, game: &Arc<Game>, file: FileRef) -> Self {
        let path = game.path(file);
        let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
        let g = game.clone();
        let job = Job::spawn(ctx, move || Ok(g.fs.extract(file)?));
        Self { title: name, path, job, goto: String::new() }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.job.poll();
        let Job::Done(data) = &self.job else {
            status_ui(ui, &self.job, &self.title);
            return;
        };
        let mut scroll_to = None;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&self.path).small().weak());
            ui.label(format!("{} bytes", data.len()));
            ui.label("Go to");
            let r = ui.add(egui::TextEdit::singleline(&mut self.goto).desired_width(80.0).hint_text("offset"));
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let t = self.goto.trim().trim_start_matches("0x");
                scroll_to = usize::from_str_radix(t, 16).ok().map(|o| o / 16);
            }
        });
        ui.separator();
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
        let rows = data.len().div_ceil(16);
        let mut area = egui::ScrollArea::both().auto_shrink(false);
        if let Some(r) = scroll_to {
            let spacing = ui.spacing().item_spacing.y;
            area = area.vertical_scroll_offset(r.min(rows) as f32 * (row_height + spacing));
        }
        area.show_rows(ui, row_height, rows, |ui, range| {
            for r in range {
                ui.label(egui::RichText::new(row(data, r * 16)).monospace());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::row;

    #[test]
    fn rows() {
        let data: Vec<u8> = (0x40..0x55).collect();
        assert_eq!(row(&data, 0), "00000000  40 41 42 43 44 45 46 47  48 49 4A 4B 4C 4D 4E 4F  @ABCDEFGHIJKLMNO");
        assert!(row(&data, 16).starts_with("00000010  50 51 52 53 54    "));
        assert!(row(&data, 16).ends_with("PQRST"));
    }
}
