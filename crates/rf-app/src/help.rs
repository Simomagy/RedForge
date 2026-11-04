//! Help's windows (the keyboard shortcuts, About) and the messages that show
//! over the bottom of the window for a few seconds.

use std::time::Duration;

use eframe::egui::{self, RichText};

use crate::map::tools::Tools;
use crate::theme::{self, icon};

/// The shortcuts, by what they're for (the README's controls, and more).
const SHORTCUTS: [(&str, &[(&str, &str)]); 4] = [
    (
        "Map",
        &[
            ("Drag (left or right button)", "Look around"),
            ("W A S D, R / F", "Move, up / down"),
            ("Shift, wheel", "Faster, speed"),
            ("Right click (or click)", "Select; with Ctrl, add or remove"),
            ("Home, Alt+F", "Look at the selection"),
            ("Esc", "Cancel a drag, clear the selection, leave Place"),
        ],
    ),
    (
        "Tools",
        &[
            ("V, Q, E, T, P", "Select, move, rotate, scale, place"),
            (".", "Gizmo along the world's or the object's axes"),
            ("Shift at the start of a drag", "Drag copies"),
            ("Q / E while placing", "Turn what's placed (Shift: faster)"),
            ("Right click while placing", "Stop placing"),
        ],
    ),
    (
        "Editing",
        &[
            ("Ctrl+Z, Ctrl+Y", "Undo, redo"),
            ("Ctrl+X, Ctrl+C, Ctrl+V", "Cut, copy, paste (into the ymap chosen in Project)"),
            ("Ctrl+D, Del", "Duplicate, delete"),
        ],
    ),
    (
        "Project",
        &[
            ("Ctrl+S", "Save the project's files"),
            ("Ctrl+O", "Open a project"),
            ("Ctrl+E", "Export the RedM resource"),
            ("Ctrl+Q", "Quit"),
            ("F1", "These shortcuts"),
        ],
    ),
];

/// Help > Keyboard shortcuts (F1).
pub fn shortcuts(ctx: &egui::Context, open: &mut bool) {
    egui::Window::new("Keyboard shortcuts")
        .open(open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(theme::card(18))
        .show(ctx, |ui| {
            for (i, (group, keys)) in SHORTCUTS.iter().enumerate() {
                if i > 0 {
                    ui.add_space(8.0);
                }
                ui.label(RichText::new(*group).font(theme::strong(theme::SIZE)));
                egui::Grid::new(("shortcuts", i)).num_columns(2).spacing([24.0, 4.0]).min_col_width(230.0).show(
                    ui,
                    |ui| {
                        for (key, what) in keys.iter() {
                            ui.label(RichText::new(*key).color(theme::ACCENT));
                            ui.label(*what);
                            ui.end_row();
                        }
                    },
                );
            }
        });
}

/// Help > About RedForge.
pub fn about(ctx: &egui::Context, open: &mut bool, icon: &egui::TextureHandle) {
    egui::Window::new("About RedForge")
        .open(open)
        .collapsible(false)
        .resizable(false)
        .title_bar(true)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(theme::card(20))
        .show(ctx, |ui| {
            ui.set_width(340.0);
            ui.vertical_centered(|ui| {
                ui.add(egui::Image::new(icon).fit_to_exact_size(egui::vec2(72.0, 72.0)));
                ui.label(RichText::new("RedForge").font(theme::heading(theme::SIZE + 12.0)));
                ui.label(RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(theme::WEAK));
                ui.add_space(6.0);
                ui.label("A map editor for RedM, by Prysma Studio.");
                ui.add_space(6.0);
                ui.hyperlink_to("github.com/Simomagy/RedForge", "https://github.com/Simomagy/RedForge");
                ui.hyperlink_to(
                    "What's new in each version",
                    "https://github.com/Simomagy/RedForge/blob/master/CHANGELOG.md",
                );
                ui.add_space(6.0);
                ui.label(RichText::new("MIT or Apache-2.0").small().color(theme::WEAK));
                ui.label(
                    RichText::new("Red Dead Redemption 2 is Rockstar Games'. RedForge isn't affiliated with them.")
                        .small()
                        .color(theme::WEAK),
                );
            });
        });
}

/// The message showing now, over the bottom of the window: what an action
/// did, or in red what went wrong. It fades away at its end.
pub fn toast(ctx: &egui::Context, tools: &Tools) {
    let Some(t) = tools.toast() else { return };
    let left = t.lasts().saturating_sub(t.at.elapsed());
    ctx.request_repaint_after(left.min(Duration::from_millis(50)));
    let (glyph, colour) = if t.error {
        (icon::WARNING_CIRCLE, ctx.global_style().visuals.error_fg_color)
    } else {
        (icon::CHECK_CIRCLE, theme::ACCENT)
    };
    egui::Area::new(egui::Id::new("toast"))
        .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -40.0])
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            ui.set_opacity((left.as_secs_f32() / 0.4).min(1.0));
            theme::card(10).show(ui, |ui| {
                ui.set_max_width(560.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(glyph).size(18.0).color(colour));
                    ui.add(egui::Label::new(&t.text).wrap());
                });
            });
        });
}
