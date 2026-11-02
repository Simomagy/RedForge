use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use eframe::egui;
use egui_dock::{DockArea, DockState, NodeIndex, Style, TabViewer};
use rf_editor::Editor;
use rf_render::{GpuContext, Renderer, TexturePreview};

use crate::browser::{Browser, BrowserAction};
use crate::game::{Game, LoadMessage, is_game_folder, start_loading};
use crate::map::MapView;
use crate::map::tools::Tools;
use crate::properties::Properties;
use crate::settings::{GAME_FOLDER, Settings};
use crate::viewers::Gpu;
use crate::viewers::anim::AnimViewer;
use crate::viewers::model::ModelViewer;
use crate::viewers::text::TextViewer;
use crate::viewers::texture::TextureViewer;

enum GameState {
    /// Asking for the game folder.
    NoFolder {
        input: String,
        error: Option<String>,
    },
    Loading {
        rx: Receiver<LoadMessage>,
        status: String,
        started: Instant,
    },
    Ready(Arc<Game>),
}

pub enum Tab {
    Welcome,
    Model(Box<ModelViewer>),
    /// A clip dictionary's clips played on a ped.
    Anim(Box<AnimViewer>),
    Texture(Box<TextureViewer>),
    Text(Box<TextViewer>),
    Map(Box<MapView>),
    /// The selection's properties and the edited ymaps.
    Properties,
}

/// For testing without anyone at the screen: opens a file, takes a screenshot
/// of the window and quits (`REDFORGE_OPEN=<path> REDFORGE_SCREENSHOT=<png>`).
///
/// `REDFORGE_OPEN` can also be `world`, `world:<place>` or
/// `world:x,y,z[,yaw,pitch]` for the world map, and
/// `REDFORGE_SCREENSHOT_DELAY` sets the seconds to wait (6 by default), and
/// `REDFORGE_SELECT_CENTRE=1` selects what's in the middle of the world map
/// two seconds before, with the Move tool; `REDFORGE_TEST_DRAG=1` then drags
/// the gizmo's X arrow 100 pixels with made-up mouse events and logs what
/// happened.
struct AutoShot {
    open: String,
    out: PathBuf,
    delay: f32,
    select_centre: bool,
    drag_test: Option<DragTest>,
    opened: Option<Instant>,
    requested: bool,
}

/// The world map at an `AutoShot` target (`world:<place>` or `world:x,y,z[,yaw,pitch]`).
fn world_view(game: &Arc<Game>, gpu: &Gpu, editor: &Editor, target: &str) -> Option<MapView> {
    let mut v = MapView::world(game, gpu, editor);
    let Some(at) = target.strip_prefix(':') else { return target.is_empty().then_some(v) };
    let numbers: Vec<f32> = at.split(',').filter_map(|n| n.trim().parse().ok()).collect();
    if numbers.len() >= 3 {
        let (yaw, pitch) = (numbers.get(3).copied().unwrap_or(0.0), numbers.get(4).copied().unwrap_or(-0.3));
        v.set_camera(glam::Vec3::new(numbers[0], numbers[1], numbers[2]), yaw, pitch);
        return Some(v);
    }
    let (_, p) = editor.world.places().into_iter().find(|(n, _)| n.eq_ignore_ascii_case(at))?;
    v.view_place(p.into());
    Some(v)
}

pub struct RedForgeApp {
    auto: Option<AutoShot>,
    settings: Settings,
    gpu: Gpu,
    game: GameState,
    dock: DockState<Tab>,
    browser: Browser,
    /// The world being edited, from when the world map first opens.
    editor: Option<Editor>,
    tools: Tools,
    properties: Properties,
}

impl RedForgeApp {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        let rs = cc.wgpu_render_state.as_ref().expect("RedForge needs the wgpu backend");
        let ctx = GpuContext::new(rs.device.clone(), rs.queue.clone());
        if !ctx.has_bc() {
            log::warn!("the GPU has no BC texture support: textures won't show");
        }
        log::info!("GPU: {} ({:?})", rs.adapter.get_info().name, rs.adapter.get_info().backend);
        let gpu = Gpu {
            renderer: Arc::new(Renderer::new(&ctx)),
            preview: Arc::new(TexturePreview::new(&ctx)),
            egui: rs.renderer.clone(),
            ctx,
        };
        let settings = Settings::load();
        let folder = std::env::var("REDFORGE_GAME_DIR").ok().or_else(|| settings.get(GAME_FOLDER).map(str::to_owned));
        let auto = match (std::env::var("REDFORGE_OPEN"), std::env::var("REDFORGE_SCREENSHOT")) {
            (Ok(open), Ok(out)) => Some(AutoShot {
                open,
                out: out.into(),
                delay: std::env::var("REDFORGE_SCREENSHOT_DELAY").ok().and_then(|d| d.parse().ok()).unwrap_or(6.0),
                select_centre: std::env::var("REDFORGE_SELECT_CENTRE").is_ok(),
                drag_test: std::env::var("REDFORGE_TEST_DRAG").is_ok().then(DragTest::default),
                opened: None,
                requested: false,
            }),
            _ => None,
        };
        let mut app = Self {
            auto,
            settings,
            gpu,
            game: GameState::NoFolder { input: folder.clone().unwrap_or_default(), error: None },
            dock: DockState::new(vec![Tab::Welcome]),
            browser: Browser::default(),
            editor: None,
            tools: Tools::default(),
            properties: Properties::default(),
        };
        if let Some(f) = folder {
            app.load_game(&cc.egui_ctx, PathBuf::from(f));
        }
        app
    }

    fn load_game(&mut self, ctx: &egui::Context, folder: PathBuf) {
        if !is_game_folder(&folder) {
            self.game = GameState::NoFolder {
                input: folder.display().to_string(),
                error: Some("RDR2.exe isn't in that folder".into()),
            };
            return;
        }
        self.settings.set(GAME_FOLDER, folder.display().to_string());
        let c = ctx.clone();
        let rx = start_loading(folder, move || c.request_repaint());
        self.game = GameState::Loading { rx, status: "Starting…".into(), started: Instant::now() };
    }

    fn open(&mut self, ctx: &egui::Context, game: &Arc<Game>, action: BrowserAction) {
        let tab = match action {
            BrowserAction::OpenXml(f) => Tab::Text(Box::new(TextViewer::open_xml(ctx, game, f))),
            BrowserAction::Open(f) => {
                let path = game.path(f).to_lowercase();
                let ext = path.rsplit('.').next().unwrap_or("");
                match ext {
                    "ydr" | "ydd" | "yft" => Tab::Model(Box::new(ModelViewer::open(ctx, game, f))),
                    "ycd" => Tab::Anim(Box::new(AnimViewer::open(ctx, game, f))),
                    "ytd" => Tab::Texture(Box::new(TextureViewer::open(ctx, game, f))),
                    "ymap" => match MapView::ymap(game, &self.gpu, f) {
                        Ok(v) => Tab::Map(Box::new(v)),
                        Err(e) => {
                            log::warn!("{e:#}");
                            Tab::Text(Box::new(TextViewer::open_xml(ctx, game, f)))
                        }
                    },
                    _ => Tab::Text(Box::new(TextViewer::open_xml(ctx, game, f))),
                }
            }
        };
        self.show_tab(tab);
    }

    /// The world map, with the editor (made the first time) and Properties
    /// beside it.
    fn open_world(&mut self, game: &Arc<Game>, place: Option<&str>) -> bool {
        let editor =
            self.editor.get_or_insert_with(|| Editor::new(game.fs.clone(), game.hashes.clone(), game.data.clone()));
        let Some(view) = world_view(game, &self.gpu, editor, place.unwrap_or_default()) else { return false };
        self.show_tab(Tab::Map(Box::new(view)));
        if self.dock.find_tab_from(|t| matches!(t, Tab::Properties)).is_none() {
            self.dock.main_surface_mut().split_right(NodeIndex::root(), 0.76, vec![Tab::Properties]);
        }
        true
    }

    fn show_tab(&mut self, tab: Tab) {
        // Replace the welcome page with the first document.
        if let Some(path) = self.dock.find_tab_from(|t| matches!(t, Tab::Welcome)) {
            self.dock.remove_tab(path);
        }
        self.dock.push_to_focused_leaf(tab);
    }

    fn folder_ui(&mut self, ui: &mut egui::Ui) {
        let mut chosen = None;
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                ui.heading("RedForge");
                ui.label("Where is Red Dead Redemption 2 installed?");
                ui.add_space(8.0);
                let GameState::NoFolder { input, error } = &mut self.game else { return };
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(input)
                            .desired_width(420.0)
                            .hint_text("/path/to/Red Dead Redemption 2"),
                    );
                    if ui.button("Browse…").clicked()
                        && let Some(dir) = rfd::FileDialog::new().set_title("The RDR2 folder").pick_folder()
                    {
                        *input = dir.display().to_string();
                    }
                    if ui.button("Open").clicked() {
                        chosen = Some(PathBuf::from(input.trim()));
                    }
                });
                if let Some(e) = error {
                    ui.colored_label(ui.visuals().error_fg_color, e.as_str());
                }
            });
        });
        if let Some(f) = chosen {
            let ctx = ui.ctx().clone();
            self.load_game(&ctx, f);
        }
    }
}

/// Made-up mouse events for testing the gizmo.
#[derive(Default)]
struct DragTest {
    /// Where to press, and which way to drag.
    start: Option<(egui::Pos2, egui::Vec2)>,
    entity: Option<(rf_editor::EntityId, glam::Vec3)>,
    step: u32,
}

impl RedForgeApp {
    /// After the test drag: what the editor made of it.
    fn report_drag(&self) {
        let Some(d) = self.auto.as_ref().and_then(|a| a.drag_test.as_ref()) else { return };
        let (Some(ed), Some((id, before))) = (&self.editor, d.entity) else {
            log::warn!("drag test: nothing selected");
            return;
        };
        let after = ed.placement(id).map(|p| p.position);
        log::info!(
            "drag test: undo has {:?}, the entity moved from {before:?} to {after:?}",
            ed.undo_labels().collect::<Vec<_>>()
        );
    }

    fn auto_shot(&mut self, ctx: &egui::Context, game: &Arc<Game>) {
        let Some(a) = &mut self.auto else { return };
        ctx.request_repaint();
        match a.opened {
            None => {
                a.opened = Some(Instant::now());
                if let Some(target) = a.open.strip_prefix("world") {
                    let target = target.to_owned();
                    if !self.open_world(game, Some(&target)) {
                        log::error!("no place {target}");
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    return;
                }
                match game.fs.find(&a.open) {
                    Some(f) => self.open(ctx, game, BrowserAction::Open(f)),
                    None => {
                        log::error!("{} not found", a.open);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
            Some(t) if a.select_centre && t.elapsed().as_secs_f32() > a.delay - 2.0 => {
                a.select_centre = false;
                if let Some(ed) = &mut self.editor {
                    for (_, tab) in self.dock.iter_all_tabs_mut() {
                        if let Tab::Map(v) = tab
                            && v.is_world()
                            && v.select_centre(&self.gpu, ed)
                        {
                            self.tools.tool = crate::map::tools::Tool::Move;
                            // The X arrow: from the entity along world X on screen.
                            if let (Some(d), Some(id)) = (&mut a.drag_test, ed.selection().first().copied())
                                && let Some(p) = ed.placement(id)
                                && let (Some(o), Some(x)) =
                                    (v.screen_point(p.position), v.screen_point(p.position + glam::Vec3::X))
                            {
                                let dir = (x - o).normalized();
                                d.start = Some((o + dir * 50.0, dir));
                                d.entity = Some((id, p.position));
                            }
                        }
                    }
                }
            }
            Some(t) if !a.requested && t.elapsed().as_secs_f32() > a.delay => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                a.requested = true;
            }
            _ => {}
        }
        let shot = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            self.report_drag();
            let a = self.auto.as_ref().unwrap();
            let [w, h] = img.size;
            let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            match image::save_buffer(&a.out, &bytes, w as u32, h as u32, image::ColorType::Rgba8) {
                Ok(()) => log::info!("screenshot saved to {}", a.out.display()),
                Err(e) => log::error!("screenshot: {e}"),
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

struct Tabs<'a> {
    gpu: &'a Gpu,
    game: Option<&'a Arc<Game>>,
    editor: Option<&'a mut Editor>,
    tools: &'a mut Tools,
    properties: &'a mut Properties,
}

impl TabViewer for Tabs<'_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        match tab {
            Tab::Welcome => "Welcome".into(),
            Tab::Model(v) => format!("🧊 {}", v.title).into(),
            Tab::Anim(v) => format!("🏃 {}", v.title).into(),
            Tab::Texture(v) => format!("🖼 {}", v.title).into(),
            Tab::Text(v) => format!("📄 {}", v.title).into(),
            Tab::Map(v) => format!("🗺 {}", v.title).into(),
            Tab::Properties => "Properties".into(),
        }
    }

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        match tab {
            Tab::Welcome => egui::Id::new("welcome"),
            Tab::Model(v) => egui::Id::new(("model", &v.path)),
            Tab::Anim(v) => egui::Id::new(("anim", &v.path)),
            Tab::Texture(v) => egui::Id::new(("texture", &v.path)),
            Tab::Text(v) => egui::Id::new(("text", &v.path)),
            Tab::Map(v) => egui::Id::new(("map", &v.path)),
            Tab::Properties => egui::Id::new("properties"),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Welcome => {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        "Open a model (ydr, ydd, yft), a texture dictionary (ytd), a clip dictionary (ycd) or any \
                         other file from Game Files.",
                    );
                });
            }
            Tab::Model(v) => match self.game {
                Some(g) => v.ui(ui, self.gpu, &g.hashes),
                None => {
                    ui.label("The game isn't loaded.");
                }
            },
            Tab::Anim(v) => v.ui(ui, self.gpu),
            Tab::Texture(v) => v.ui(ui, self.gpu),
            Tab::Text(v) => v.ui(ui),
            Tab::Map(v) => v.ui(ui, self.gpu, self.editor.as_deref_mut(), self.tools),
            Tab::Properties => match self.editor.as_deref_mut() {
                Some(ed) => self.properties.ui(ui, ed, Some(self.tools)),
                None => {
                    ui.label("Open the world map (View → World map) to edit.");
                }
            },
        }
    }

    fn on_close(&mut self, tab: &mut Tab) -> egui_dock::widgets::tab_viewer::OnCloseResponse {
        match tab {
            Tab::Model(v) => v.free(self.gpu),
            Tab::Anim(v) => v.free(self.gpu),
            Tab::Texture(v) => v.free(self.gpu),
            Tab::Map(v) => v.free(self.gpu),
            _ => {}
        }
        egui_dock::widgets::tab_viewer::OnCloseResponse::Close
    }
}

impl eframe::App for RedForgeApp {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        let Some(d) = self.auto.as_mut().and_then(|a| a.drag_test.as_mut()) else { return };
        let Some((start, dir)) = d.start else { return };
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let at = |k: u32| start + dir * (k as f32 * 10.0);
        match d.step {
            0 => raw.events.push(egui::Event::PointerMoved(start)),
            1 => raw.events.push(button(start, true)),
            2..=11 => raw.events.push(egui::Event::PointerMoved(at(d.step - 1))),
            12 => raw.events.push(button(at(10), false)),
            _ => return,
        }
        d.step += 1;
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        if let GameState::Loading { rx, status, .. } = &mut self.game {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    LoadMessage::Progress(s) => *status = s,
                    LoadMessage::Done(result) => match *result {
                        Ok(g) => {
                            self.game = GameState::Ready(Arc::new(g));
                            self.editor = None;
                            break;
                        }
                        Err(e) => {
                            let input = self.settings.get(GAME_FOLDER).unwrap_or_default().to_owned();
                            self.game = GameState::NoFolder { input, error: Some(format!("{e:#}")) };
                            break;
                        }
                    },
                }
            }
        }

        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                if let GameState::Ready(g) = &self.game {
                    let g = g.clone();
                    ui.menu_button("View", |ui| {
                        if ui.button("World map").clicked() {
                            self.open_world(&g, None);
                            ui.close();
                        }
                        if ui.button("Properties").clicked() {
                            if self.dock.find_tab_from(|t| matches!(t, Tab::Properties)).is_none() {
                                self.dock.main_surface_mut().split_right(
                                    NodeIndex::root(),
                                    0.76,
                                    vec![Tab::Properties],
                                );
                            }
                            ui.close();
                        }
                    });
                    if let Some(ed) = &mut self.editor {
                        ui.menu_button("Edit", |ui| {
                            let undo = ed.undo_labels().next().map(|l| format!("Undo {l}"));
                            if ui
                                .add_enabled(
                                    undo.is_some(),
                                    egui::Button::new(undo.as_deref().unwrap_or("Undo")).shortcut_text("Ctrl+Z"),
                                )
                                .clicked()
                            {
                                ed.undo();
                            }
                            let redo = ed.redo_labels().next().map(|l| format!("Redo {l}"));
                            if ui
                                .add_enabled(
                                    redo.is_some(),
                                    egui::Button::new(redo.as_deref().unwrap_or("Redo")).shortcut_text("Ctrl+Y"),
                                )
                                .clicked()
                            {
                                ed.redo();
                            }
                            ui.separator();
                            let any = !ed.selection().is_empty();
                            let mut result = Ok(());
                            if ui.add_enabled(any, egui::Button::new("Copy").shortcut_text("Ctrl+C")).clicked() {
                                ed.copy();
                            }
                            if ui
                                .add_enabled(ed.can_paste(), egui::Button::new("Paste").shortcut_text("Ctrl+V"))
                                .clicked()
                            {
                                result = ed.paste();
                            }
                            if ui.add_enabled(any, egui::Button::new("Duplicate").shortcut_text("Ctrl+D")).clicked() {
                                result = ed.duplicate_selected();
                            }
                            if ui.add_enabled(any, egui::Button::new("Delete").shortcut_text("Del")).clicked() {
                                result = ed.delete_selected();
                            }
                            if let Err(e) = result {
                                self.tools.say(e);
                            }
                        });
                    }
                }
                ui.menu_button("File", |ui| {
                    if ui.button("Game folder…").clicked() {
                        let input = self.settings.get(GAME_FOLDER).unwrap_or_default().to_owned();
                        self.game = GameState::NoFolder { input, error: None };
                        ui.close();
                    }
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
        });

        let game = match &self.game {
            GameState::Ready(g) => Some(g.clone()),
            GameState::NoFolder { .. } => {
                self.folder_ui(ui);
                return;
            }
            GameState::Loading { status, started, .. } => {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(format!("{status} ({:.1} s)", started.elapsed().as_secs_f32()));
                        });
                    });
                });
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
        };

        egui::Panel::bottom("status").show(ui, |ui| {
            if let Some(g) = &game {
                ui.horizontal(|ui| {
                    ui.label(format!("{} files in {} archives", g.fs.file_count(), g.fs.archives.len()));
                    ui.separator();
                    ui.label(egui::RichText::new(g.folder.display().to_string()).weak());
                });
            }
        });

        let mut action = None;
        if let Some(g) = &game {
            egui::Panel::left("game-files").default_size(340.0).show(ui, |ui| {
                ui.heading("Game Files");
                action = self.browser.ui(ui, g);
            });
        }
        if let (Some(a), Some(g)) = (action, &game) {
            self.open(ctx, g, a);
        }

        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let mut tabs = Tabs {
                gpu: &self.gpu,
                game: game.as_ref(),
                editor: self.editor.as_mut(),
                tools: &mut self.tools,
                properties: &mut self.properties,
            };
            DockArea::new(&mut self.dock).style(Style::from_egui(ui.style().as_ref())).show_inside(ui, &mut tabs);
        });

        if let Some(g) = &game {
            self.auto_shot(ctx, g);
        }

        // Viewers waiting on a background job need frames to notice it finishing.
        let waiting = self.dock.iter_all_tabs().any(|(_, t)| match t {
            Tab::Model(_) | Tab::Anim(_) | Tab::Texture(_) | Tab::Text(_) | Tab::Map(_) => true,
            Tab::Welcome | Tab::Properties => false,
        });
        if waiting {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}
