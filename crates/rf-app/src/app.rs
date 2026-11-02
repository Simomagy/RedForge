use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use eframe::egui;
use egui_dock::{DockArea, DockState, Style, TabViewer};
use rf_render::{GpuContext, Renderer, TexturePreview};

use crate::browser::{Browser, BrowserAction};
use crate::game::{Game, LoadMessage, is_game_folder, start_loading};
use crate::settings::{GAME_FOLDER, Settings};
use crate::viewers::Gpu;
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
    Texture(Box<TextureViewer>),
    Text(Box<TextViewer>),
}

/// For testing without anyone at the screen: opens a file, takes a screenshot
/// of the window and quits (`REDFORGE_OPEN=<path> REDFORGE_SCREENSHOT=<png>`).
struct AutoShot {
    open: String,
    out: PathBuf,
    opened: Option<Instant>,
    requested: bool,
}

pub struct RedForgeApp {
    auto: Option<AutoShot>,
    settings: Settings,
    gpu: Gpu,
    game: GameState,
    dock: DockState<Tab>,
    browser: Browser,
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
            (Ok(open), Ok(out)) => Some(AutoShot { open, out: out.into(), opened: None, requested: false }),
            _ => None,
        };
        let mut app = Self {
            auto,
            settings,
            gpu,
            game: GameState::NoFolder { input: folder.clone().unwrap_or_default(), error: None },
            dock: DockState::new(vec![Tab::Welcome]),
            browser: Browser::default(),
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
                    "ytd" => Tab::Texture(Box::new(TextureViewer::open(ctx, game, f))),
                    _ => Tab::Text(Box::new(TextViewer::open_xml(ctx, game, f))),
                }
            }
        };
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

impl RedForgeApp {
    fn auto_shot(&mut self, ctx: &egui::Context, game: &Arc<Game>) {
        let Some(a) = &mut self.auto else { return };
        ctx.request_repaint();
        match a.opened {
            None => {
                a.opened = Some(Instant::now());
                match game.fs.find(&a.open) {
                    Some(f) => self.open(ctx, game, BrowserAction::Open(f)),
                    None => {
                        log::error!("{} not found", a.open);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
            Some(t) if !a.requested && t.elapsed().as_secs_f32() > 6.0 => {
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
}

impl TabViewer for Tabs<'_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        match tab {
            Tab::Welcome => "Welcome".into(),
            Tab::Model(v) => format!("🧊 {}", v.title).into(),
            Tab::Texture(v) => format!("🖼 {}", v.title).into(),
            Tab::Text(v) => format!("📄 {}", v.title).into(),
        }
    }

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        match tab {
            Tab::Welcome => egui::Id::new("welcome"),
            Tab::Model(v) => egui::Id::new(("model", &v.path)),
            Tab::Texture(v) => egui::Id::new(("texture", &v.path)),
            Tab::Text(v) => egui::Id::new(("text", &v.path)),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Welcome => {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        "Open a model (ydr, ydd, yft), a texture dictionary (ytd) or any other file from Game Files.",
                    );
                });
            }
            Tab::Model(v) => match self.game {
                Some(g) => v.ui(ui, self.gpu, &g.hashes),
                None => {
                    ui.label("The game isn't loaded.");
                }
            },
            Tab::Texture(v) => v.ui(ui, self.gpu),
            Tab::Text(v) => v.ui(ui),
        }
    }

    fn on_close(&mut self, tab: &mut Tab) -> egui_dock::widgets::tab_viewer::OnCloseResponse {
        match tab {
            Tab::Model(v) => v.free(self.gpu),
            Tab::Texture(v) => v.free(self.gpu),
            _ => {}
        }
        egui_dock::widgets::tab_viewer::OnCloseResponse::Close
    }
}

impl eframe::App for RedForgeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        if let GameState::Loading { rx, status, .. } = &mut self.game {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    LoadMessage::Progress(s) => *status = s,
                    LoadMessage::Done(Ok(g)) => {
                        self.game = GameState::Ready(Arc::new(g));
                        break;
                    }
                    LoadMessage::Done(Err(e)) => {
                        let input = self.settings.get(GAME_FOLDER).unwrap_or_default().to_owned();
                        self.game = GameState::NoFolder { input, error: Some(format!("{e:#}")) };
                        break;
                    }
                }
            }
        }

        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
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
            let mut tabs = Tabs { gpu: &self.gpu, game: game.as_ref() };
            DockArea::new(&mut self.dock).style(Style::from_egui(ui.style().as_ref())).show_inside(ui, &mut tabs);
        });

        if let Some(g) = &game {
            self.auto_shot(ctx, g);
        }

        // Viewers waiting on a background job need frames to notice it finishing.
        let waiting = self.dock.iter_all_tabs().any(|(_, t)| match t {
            Tab::Model(_) | Tab::Texture(_) | Tab::Text(_) => true,
            Tab::Welcome => false,
        });
        if waiting {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}
