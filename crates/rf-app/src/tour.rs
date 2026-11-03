//! A short tour of the panels, the first time RedForge opens the game (and
//! from Help): each step brings a panel to the front, dims the rest and says
//! in a few words what it's for.

use std::collections::HashMap;

use eframe::egui::{self, Align2, Color32, Rect, RichText, Stroke, Vec2};

/// The panels the steps show, by the names the app records their rects by.
pub const GAME_FILES: &str = "game-files";
pub const PROJECT: &str = "project";
pub const HIERARCHY: &str = "hierarchy";
pub const DOCUMENTS: &str = "documents";
pub const PROPERTIES: &str = "properties";
pub const OBJECTS: &str = "objects";

/// Each step: the panel it shows (none: the middle of the window), its title
/// and what it says.
const STEPS: [(Option<&str>, &str, &str); 8] = [
    (
        None,
        "Welcome to RedForge",
        "A quick look at the panels, a minute at most. Help > Tour of the panels shows it again.",
    ),
    (
        Some(GAME_FILES),
        "Game Files",
        "The game's archives as folders. Double-click a model, texture or clip to open it; right-click a file \
         to see it as XML or hex.",
    ),
    (
        Some(PROJECT),
        "Project",
        "Your mod: the ymaps and ytyps you edit, the models and textures you add, grouped by type. Save all \
         (Ctrl+S) writes them next to the .rfproj; RedM resource exports them ready to stream.",
    ),
    (
        Some(HIERARCHY),
        "Hierarchy",
        "The objects of the selected object's ymap or interior, by LOD. Click one to select it, double-click to \
         look at it; the eye hides it on the map.",
    ),
    (
        Some(DOCUMENTS),
        "The map",
        "The world map opens here (the button on the welcome page, or View > World map), with what you open from \
         Game Files beside it. Drag to look around, WASD to move, right click to select. The toolbar has the \
         tools (Q moves, E rotates), the selection modes (collisions, navmeshes…), the hour and the weather.",
    ),
    (
        Some(PROPERTIES),
        "Properties",
        "What's selected, to edit: its archetype, position, rotation and the rest of its fields. In the other \
         selection modes, the helper's details.",
    ),
    (
        Some(OBJECTS),
        "Objects",
        "Every archetype of the game and of your project, with 3D thumbnails. Click one, then click on the map \
         to place it; Q and E turn it, a right click stops.",
    ),
    (None, "That's it", "Hover over the buttons for tips. Happy mapping!"),
];

#[derive(Default)]
pub struct Tour {
    step: Option<usize>,
}

/// What the tour did this frame.
#[derive(PartialEq, Eq)]
pub enum TourEvent {
    None,
    /// Finished or skipped.
    Closed,
}

impl Tour {
    pub fn start(&mut self) {
        self.step = Some(0);
    }

    /// At a step (for testing: `REDFORGE_TOUR`).
    pub fn start_at(&mut self, step: usize) {
        self.step = Some(step.min(STEPS.len() - 1));
    }

    pub fn is_open(&self) -> bool {
        self.step.is_some()
    }

    /// The panel the current step shows, to bring to the front.
    pub fn target(&self) -> Option<&'static str> {
        STEPS[self.step?].0
    }

    /// Draws the step over the app; `rects` are where the panels were drawn
    /// this frame.
    pub fn ui(&mut self, ctx: &egui::Context, rects: &HashMap<&'static str, Rect>) -> TourEvent {
        let Some(step) = self.step else { return TourEvent::None };
        let (target, title, text) = STEPS[step];
        let screen = ctx.content_rect();
        let lit = target.and_then(|t| rects.get(t)).copied();

        // Everything dimmed but the panel, which nothing else can be clicked through.
        let dim = Color32::from_black_alpha(170);
        let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("tour-dim"));
        let painter = ctx.layer_painter(layer);
        match lit {
            Some(r) => {
                let r = r.expand(2.0);
                painter.rect_filled(Rect::from_min_max(screen.min, egui::pos2(screen.max.x, r.min.y)), 0.0, dim);
                painter.rect_filled(Rect::from_min_max(egui::pos2(screen.min.x, r.max.y), screen.max), 0.0, dim);
                painter.rect_filled(
                    Rect::from_min_max(egui::pos2(screen.min.x, r.min.y), egui::pos2(r.min.x, r.max.y)),
                    0.0,
                    dim,
                );
                painter.rect_filled(
                    Rect::from_min_max(egui::pos2(r.max.x, r.min.y), egui::pos2(screen.max.x, r.max.y)),
                    0.0,
                    dim,
                );
                painter.rect_stroke(
                    r,
                    4.0,
                    Stroke::new(2.0, Color32::from_rgb(255, 106, 26)),
                    egui::StrokeKind::Outside,
                );
            }
            None => {
                painter.rect_filled(screen, 0.0, dim);
            }
        }
        egui::Area::new(egui::Id::new("tour-block")).order(egui::Order::Foreground).fixed_pos(screen.min).show(
            ctx,
            |ui| {
                ui.allocate_rect(screen, egui::Sense::click_and_drag());
            },
        );

        // The card: beside the panel where there's room, else in it.
        const WIDTH: f32 = 340.0;
        let (pos, pivot) = match lit {
            Some(r) if r.max.x + WIDTH + 24.0 < screen.max.x && r.width() < screen.width() * 0.5 => {
                (egui::pos2(r.max.x + 16.0, r.center().y), Align2::LEFT_CENTER)
            }
            Some(r) if r.min.x - WIDTH - 24.0 > screen.min.x && r.width() < screen.width() * 0.5 => {
                (egui::pos2(r.min.x - 16.0, r.center().y), Align2::RIGHT_CENTER)
            }
            Some(r) => (r.center(), Align2::CENTER_CENTER),
            None => (screen.center(), Align2::CENTER_CENTER),
        };
        let mut event = TourEvent::None;
        egui::Area::new(egui::Id::new("tour-card")).order(egui::Order::Tooltip).pivot(pivot).fixed_pos(pos).show(
            ctx,
            |ui| {
                egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
                    ui.set_width(WIDTH);
                    ui.label(RichText::new(format!("{} of {}", step + 1, STEPS.len())).small().weak());
                    ui.label(RichText::new(title).heading().color(Color32::from_rgb(255, 138, 61)));
                    ui.add_space(4.0);
                    ui.label(text);
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let last = step + 1 == STEPS.len();
                        if ui
                            .add(egui::Button::new(if last { "Done" } else { "Next" }).min_size(Vec2::new(72.0, 0.0)))
                            .clicked()
                        {
                            self.step = (!last).then_some(step + 1);
                            if last {
                                event = TourEvent::Closed;
                            }
                        }
                        if step > 0 && ui.button("Back").clicked() {
                            self.step = Some(step - 1);
                        }
                        if !last && ui.button("Skip").clicked() {
                            self.step = None;
                            event = TourEvent::Closed;
                        }
                    });
                });
            },
        );
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.step = None;
            event = TourEvent::Closed;
        }
        event
    }
}
