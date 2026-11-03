use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use eframe::egui;
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use rf_editor::Editor;
use rf_render::{GpuContext, Renderer, TexturePreview};

use crate::blender::BlenderLink;
use crate::browser::{Browser, BrowserAction};
use crate::game::{Game, LoadMessage, is_game_folder, start_loading};
use crate::hierarchy::Hierarchy;
use crate::map::MapView;
use crate::map::tools::Tools;
use crate::objects::Objects;
use crate::project_pane::ProjectPane;
use crate::properties::Properties;
use crate::settings::{GAME_FOLDER, Settings};
use crate::tour::{self, Tour, TourEvent};
use crate::viewers::Gpu;
use crate::viewers::anim::AnimViewer;
use crate::viewers::hex::HexViewer;
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
    Hex(Box<HexViewer>),
    Map(Box<MapView>),
    /// The selection's properties.
    Properties,
    /// The project's files, saving, the RedM export.
    Project,
    /// The objects of the selection's ymap or interior.
    Hierarchy,
    /// The game's archives.
    GameFiles,
    /// The game's archetypes, to choose the one to place.
    Objects,
}

impl Tab {
    /// The panes beside the map, which share a leaf of the dock.
    fn is_side(&self) -> bool {
        matches!(self, Tab::Properties | Tab::Objects)
    }
}

/// For testing without anyone at the screen: opens a file, takes a screenshot
/// of the window and quits (`REDFORGE_OPEN=<path> REDFORGE_SCREENSHOT=<png>`).
///
/// `REDFORGE_OPEN` can also be `welcome` (nothing opened), `world`, `world:<place>` or
/// `world:x,y,z[,yaw,pitch]` for the world map, and
/// `REDFORGE_SCREENSHOT_DELAY` sets the seconds to wait (6 by default), and
/// `REDFORGE_SELECT_CENTRE=1` selects what's in the middle of the world map
/// two seconds before, with the Move tool; `REDFORGE_TEST_DRAG=1` then drags
/// the gizmo's X arrow 100 pixels with made-up mouse events and logs what
/// happened. `REDFORGE_OBJECTS_SEARCH=<text>` shows the Objects pane beside
/// the world map with that search, and chooses the first object shown.
struct AutoShot {
    open: String,
    out: PathBuf,
    delay: f32,
    select_centre: bool,
    drag_test: Option<DragTest>,
    tab_shown: bool,
    objects_search: Option<String>,
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
    /// The world being edited, from when the game is loaded.
    editor: Option<Editor>,
    tools: Tools,
    properties: Properties,
    project_pane: ProjectPane,
    hierarchy: Hierarchy,
    /// The Blender add-on's link, from when the game is loaded.
    blender: Option<BlenderLink>,
    objects: Objects,
    /// The loading screen, while the game loads and as it fades away.
    loading: Option<crate::loading::LoadingScreen>,
    /// The tour of the panels, and where each panel was drawn last frame.
    tour: Tour,
    panel_rects: HashMap<&'static str, egui::Rect>,
    /// The app's icon, for the welcome page.
    icon: egui::TextureHandle,
}

/// The setting that's set once the tour was seen (or skipped).
const TOUR_DONE: &str = "Onboarding.Done";

impl RedForgeApp {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        crate::theme::apply(&cc.egui_ctx);
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
                tab_shown: false,
                objects_search: std::env::var("REDFORGE_OBJECTS_SEARCH").ok(),
                opened: None,
                requested: false,
            }),
            _ => None,
        };
        let tile_size = settings.get(crate::objects::TILE_SIZE).and_then(|v| v.parse().ok());
        let mut app = Self {
            auto,
            settings,
            gpu,
            game: GameState::NoFolder { input: folder.clone().unwrap_or_default(), error: None },
            dock: default_dock(),
            browser: Browser::default(),
            editor: None,
            tools: Tools::default(),
            properties: Properties::default(),
            project_pane: ProjectPane::default(),
            hierarchy: Hierarchy::default(),
            blender: None,
            objects: Objects::new(tile_size.unwrap_or(crate::objects::DEFAULT_TILE)),
            loading: None,
            tour: Tour::default(),
            panel_rects: HashMap::new(),
            icon: {
                let i = crate::icon();
                let image = egui::ColorImage::from_rgba_unmultiplied([i.width as usize, i.height as usize], &i.rgba);
                cc.egui_ctx.load_texture("redforge icon", image, egui::TextureOptions::LINEAR)
            },
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
        self.loading = Some(crate::loading::LoadingScreen::new());
    }

    fn open(&mut self, ctx: &egui::Context, game: &Arc<Game>, action: BrowserAction) {
        let tab = match action {
            BrowserAction::OpenXml(f) => Tab::Text(Box::new(TextViewer::open_xml(ctx, game, f))),
            BrowserAction::OpenHex(f) => Tab::Hex(Box::new(HexViewer::open(ctx, game, f))),
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

    /// The world map, with the editor (made the first time), and Properties
    /// and Objects beside it.
    fn open_world(&mut self, game: &Arc<Game>, place: Option<&str>) -> bool {
        let editor = self.editor.get_or_insert_with(|| Editor::new(game.fs.clone(), game.hashes.clone(), game.data()));
        let Some(view) = world_view(game, &self.gpu, editor, place.unwrap_or_default()) else { return false };
        self.show_tab(Tab::Map(Box::new(view)));
        self.show_side_tab(Tab::Properties, false);
        self.show_side_tab(Tab::Objects, false);
        true
    }

    /// Shows a document with the others (in place of the welcome page).
    fn show_tab(&mut self, tab: Tab) {
        let document = |t: &Tab| {
            matches!(
                t,
                Tab::Welcome
                    | Tab::Map(_)
                    | Tab::Model(_)
                    | Tab::Anim(_)
                    | Tab::Texture(_)
                    | Tab::Text(_)
                    | Tab::Hex(_)
            )
        };
        if let Some(path) = self.dock.find_tab_from(document) {
            self.dock.set_focused_node_and_surface(egui_dock::NodePath { surface: path.surface, node: path.node });
        }
        self.dock.push_to_focused_leaf(tab);
        if let Some(path) = self.dock.find_tab_from(|t| matches!(t, Tab::Welcome)) {
            self.dock.remove_tab(path);
        }
    }

    /// Opens a side pane (Properties, Objects) if it isn't open: beside the
    /// other one, or on the right of everything. `activate` brings it to the
    /// front.
    fn show_side_tab(&mut self, tab: Tab, activate: bool) {
        let kind = std::mem::discriminant(&tab);
        if let Some(path) = self.dock.find_tab_from(|t| std::mem::discriminant(t) == kind) {
            if activate {
                let _ = self.dock.set_active_tab(path);
            }
            return;
        }
        match self.dock.find_tab_from(Tab::is_side).and_then(|p| self.dock.leaf_mut(p.node_path()).ok()) {
            Some(leaf) => {
                let active = leaf.active;
                leaf.append_tab(tab);
                if !activate {
                    leaf.active = active;
                }
            }
            None => {
                self.dock.main_surface_mut().split_right(NodeIndex::root(), 0.76, vec![tab]);
            }
        }
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

/// The fixed tabs' titles.
fn tab_title(t: &Tab) -> &'static str {
    match t {
        Tab::Project => "Project",
        Tab::Hierarchy => "Hierarchy",
        Tab::GameFiles => "Game Files",
        Tab::Properties => "Properties",
        _ => "",
    }
}

/// The layout to start with, as the C#'s: project and game files on the left
/// with the hierarchy under them, documents in the middle (Properties joins
/// on the right with the world map).
fn default_dock() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Welcome]);
    let surface = dock.main_surface_mut();
    let [_, left] = surface.split_left(NodeIndex::root(), 0.22, vec![Tab::Project, Tab::GameFiles]);
    surface.split_below(left, 0.5, vec![Tab::Hierarchy]);
    dock
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
                    let search = a.objects_search.clone();
                    if !self.open_world(game, Some(&target)) {
                        log::error!("no place {target}");
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if let Some(s) = search {
                        self.show_side_tab(Tab::Objects, true);
                        self.objects.set_search(&s);
                        self.objects.choose_first = true;
                    }
                    return;
                }
                // `welcome`: the app as it starts.
                if a.open == "welcome" {
                    return;
                }
                let (path, hex) = match a.open.strip_prefix("hex:") {
                    Some(p) => (p.to_owned(), true),
                    None => (a.open.clone(), false),
                };
                match game.fs.find(&path) {
                    Some(f) if hex => self.open(ctx, game, BrowserAction::OpenHex(f)),
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
                            && v.select_centre(&self.gpu, ed, &mut self.tools)
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
            Some(t) if !a.requested && t.elapsed().as_secs_f32() > a.delay - 1.0 && !a.tab_shown => {
                // REDFORGE_SHOW_TAB=<title> brings a tab to the front for the screenshot.
                a.tab_shown = true;
                if let Ok(title) = std::env::var("REDFORGE_SHOW_TAB")
                    && let Some(path) = self.dock.find_tab_from(|t| tab_title(t) == title)
                {
                    let _ = self.dock.set_active_tab(path);
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
    project: &'a mut ProjectPane,
    hierarchy: &'a mut Hierarchy,
    browser: &'a mut Browser,
    settings: &'a mut Settings,
    /// What the Game Files browser asked to open.
    action: &'a mut Option<BrowserAction>,
    objects: &'a mut Objects,
    /// The welcome page asked for the world map (at a place, or where it starts).
    open_world: &'a mut Option<String>,
    /// Where each panel is drawn ([`crate::tour`]'s names).
    rects: &'a mut HashMap<&'static str, egui::Rect>,
    icon: &'a egui::TextureHandle,
}

impl Tabs<'_> {
    /// The welcome page: the world map a click away (and at a town), and
    /// what else there is to open.
    fn welcome_ui(&mut self, ui: &mut egui::Ui) {
        let places: Vec<String> =
            self.editor.as_deref().map(|e| e.world.places().into_iter().map(|p| p.0).collect()).unwrap_or_default();
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.2).max(24.0));
            ui.add(egui::Image::new(self.icon).fit_to_exact_size(egui::vec2(96.0, 96.0)));
            ui.label(egui::RichText::new("RedForge").font(crate::theme::heading(34.0)));
            ui.label(egui::RichText::new("A map editor for RedM").weak());
            ui.add_space(24.0);
            let text = format!("{}  Open the world map", crate::theme::icon::MAP_TRIFOLD);
            let open = egui::Button::new(egui::RichText::new(text).size(18.0).color(crate::theme::ON_ACCENT))
                .fill(crate::theme::ACCENT)
                .corner_radius(egui::CornerRadius::same(crate::theme::BIG_RADIUS))
                .min_size(egui::vec2(260.0, 44.0));
            let enabled = self.game.is_some();
            if ui
                .add_enabled(enabled, open)
                .on_hover_text("The game's map, streamed around you (View > World map)")
                .clicked()
            {
                *self.open_world = Some(String::new());
            }
            if enabled && !places.is_empty() {
                ui.add_space(12.0);
                ui.label(egui::RichText::new("or start at").weak());
                ui.add_space(4.0);
                // The towns, in rows centred under the button.
                let width = ui.available_width().min(560.0);
                ui.allocate_ui(egui::vec2(width, 0.0), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for p in &places {
                            if ui.small_button(p).clicked() {
                                *self.open_world = Some(p.clone());
                            }
                        }
                    });
                });
            }
            ui.add_space(28.0);
            ui.label(
                egui::RichText::new(
                    "Or open a model (ydr, ydd, yft), a texture dictionary (ytd), a clip dictionary (ycd) or any other \
                     file from Game Files, and a project from the Project pane.",
                )
                .weak(),
            );
        });
    }
}

impl TabViewer for Tabs<'_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        use crate::theme::{icon, icon_text};
        match tab {
            Tab::Welcome => icon_text(icon::HOUSE, "Welcome"),
            Tab::Model(v) => icon_text(icon::CUBE, &v.title),
            Tab::Anim(v) => icon_text(icon::PERSON_SIMPLE_RUN, &v.title),
            Tab::Texture(v) => icon_text(icon::IMAGE, &v.title),
            Tab::Text(v) => icon_text(icon::FILE_CODE, &v.title),
            Tab::Hex(v) => icon_text(icon::BINARY, &format!("{} (hex)", v.title)),
            Tab::Map(v) => icon_text(icon::MAP_TRIFOLD, &v.title),
            Tab::Properties => icon_text(icon::SLIDERS_HORIZONTAL, "Properties"),
            Tab::Project => icon_text(icon::FOLDER_OPEN, "Project"),
            Tab::Hierarchy => icon_text(icon::TREE_STRUCTURE, "Hierarchy"),
            Tab::GameFiles => icon_text(icon::PACKAGE, "Game Files"),
            Tab::Objects => icon_text(icon::SHAPES, "Objects"),
        }
        .into()
    }

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        match tab {
            Tab::Welcome => egui::Id::new("welcome"),
            Tab::Model(v) => egui::Id::new(("model", &v.path)),
            Tab::Anim(v) => egui::Id::new(("anim", &v.path)),
            Tab::Texture(v) => egui::Id::new(("texture", &v.path)),
            Tab::Text(v) => egui::Id::new(("text", &v.path)),
            Tab::Hex(v) => egui::Id::new(("hex", &v.path)),
            Tab::Map(v) => egui::Id::new(("map", &v.path)),
            Tab::Properties => egui::Id::new("properties"),
            Tab::Project => egui::Id::new("project"),
            Tab::Hierarchy => egui::Id::new("hierarchy"),
            Tab::GameFiles => egui::Id::new("game-files"),
            Tab::Objects => egui::Id::new("objects"),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        let panel = match tab {
            Tab::Project => tour::PROJECT,
            Tab::Hierarchy => tour::HIERARCHY,
            Tab::GameFiles => tour::GAME_FILES,
            Tab::Properties => tour::PROPERTIES,
            Tab::Objects => tour::OBJECTS,
            _ => tour::DOCUMENTS,
        };
        self.rects.insert(panel, ui.max_rect());
        match tab {
            Tab::Welcome => self.welcome_ui(ui),
            Tab::Model(v) => match self.game {
                Some(g) => v.ui(ui, self.gpu, &g.hashes),
                None => {
                    ui.label("The game isn't loaded.");
                }
            },
            Tab::Anim(v) => v.ui(ui, self.gpu),
            Tab::Texture(v) => v.ui(ui, self.gpu),
            Tab::Text(v) => v.ui(ui),
            Tab::Hex(v) => v.ui(ui),
            Tab::Map(v) => v.ui(ui, self.gpu, self.editor.as_deref_mut(), self.tools),
            Tab::Properties => match self.editor.as_deref_mut() {
                Some(ed) => self.properties.ui(ui, ed, Some(self.tools)),
                None => {
                    ui.label("The game isn't loaded.");
                }
            },
            Tab::Project => match self.editor.as_deref_mut() {
                Some(ed) => self.project.ui(ui, ed, self.settings),
                None => {
                    ui.label("The game isn't loaded.");
                }
            },
            Tab::Hierarchy => match self.editor.as_deref_mut() {
                Some(ed) => {
                    if let Some(node) = self.project.picked_map.take() {
                        self.hierarchy.show_map(node, ed);
                    }
                    self.hierarchy.ui(ui, ed, self.tools)
                }
                None => {
                    ui.label("The game isn't loaded.");
                }
            },
            Tab::GameFiles => match self.game {
                Some(g) => {
                    if let Some(a) = self.browser.ui(ui, g) {
                        *self.action = Some(a);
                    }
                }
                None => {
                    ui.label("The game isn't loaded.");
                }
            },
            Tab::Objects => match self.game {
                Some(g) => self.objects.ui(ui, self.gpu, g, self.editor.as_deref_mut(), self.tools),
                None => {
                    ui.label("The game isn't loaded.");
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
            Tab::Objects => self.objects.free(self.gpu),
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
                    LoadMessage::Progress(s) => {
                        if let Some(l) = &mut self.loading {
                            l.set_status(&s);
                        }
                        *status = s;
                    }
                    LoadMessage::Done(result) => match *result {
                        Ok(g) => {
                            if let Some(l) = &mut self.loading {
                                l.finish();
                            }
                            let g = Arc::new(g);
                            self.editor = Some(Editor::new(g.fs.clone(), g.hashes.clone(), g.data()));
                            self.blender.get_or_insert_with(|| BlenderLink::start(ctx, &self.settings));
                            self.game = GameState::Ready(g);
                            self.objects.reset(&self.gpu);
                            break;
                        }
                        Err(e) => {
                            self.loading = None;
                            let input = self.settings.get(GAME_FOLDER).unwrap_or_default().to_owned();
                            self.game = GameState::NoFolder { input, error: Some(format!("{e:#}")) };
                            break;
                        }
                    },
                }
            }
        }

        let game_ready = matches!(self.game, GameState::Ready(_));
        let bar = egui::Frame::new().fill(crate::theme::BG).inner_margin(egui::Margin::symmetric(8, 4));
        egui::Panel::top("menu").frame(bar).show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.add(egui::Image::new(&self.icon).fit_to_exact_size(egui::vec2(18.0, 18.0)));
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
                if let GameState::Ready(g) = &self.game {
                    let g = g.clone();
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
                            ui.separator();
                            if ui.add_enabled(any, egui::Button::new("Edit in Blender")).clicked() {
                                result = match self.blender.as_ref().map(BlenderLink::edit_selection) {
                                    Some(Ok(msg)) => {
                                        self.tools.say(msg);
                                        Ok(())
                                    }
                                    Some(Err(e)) => Err(e),
                                    None => Ok(()),
                                };
                            }
                            if let Err(e) = result {
                                self.tools.say(e);
                            }
                        });
                    }
                    ui.menu_button("View", |ui| {
                        if ui.button("World map").clicked() {
                            self.open_world(&g, None);
                            ui.close();
                        }
                        for (label, tab) in
                            [("Project", Tab::Project), ("Hierarchy", Tab::Hierarchy), ("Game Files", Tab::GameFiles)]
                        {
                            if ui.button(label).clicked() {
                                let missing = self
                                    .dock
                                    .find_tab_from(|t| std::mem::discriminant(t) == std::mem::discriminant(&tab))
                                    .is_none();
                                if missing {
                                    self.dock.main_surface_mut().split_left(NodeIndex::root(), 0.22, vec![tab]);
                                }
                                ui.close();
                            }
                        }
                        if ui.button("Properties").clicked() {
                            self.show_side_tab(Tab::Properties, true);
                            ui.close();
                        }
                        if ui.button("Objects").clicked() {
                            self.show_side_tab(Tab::Objects, true);
                            ui.close();
                        }
                    });
                }
                ui.menu_button("Help", |ui| {
                    if ui.add_enabled(game_ready, egui::Button::new("Tour of the panels")).clicked() {
                        self.tour.start();
                        ui.close();
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
                log::debug!("{status} ({:.1} s)", started.elapsed().as_secs_f32());
                if let Some(l) = &mut self.loading {
                    l.ui(ctx);
                }
                return;
            }
        };
        // The loading screen fading away over the app.
        if let Some(l) = &mut self.loading {
            l.ui(ctx);
            if l.gone() {
                self.loading = None;
            }
        }

        egui::Panel::bottom("status").frame(bar).show(ui, |ui| {
            use crate::theme::icon;
            if let Some(g) = &game {
                ui.horizontal(|ui| {
                    let files =
                        format!("{}  {} files in {} archives", icon::PACKAGE, g.fs.file_count(), g.fs.archives.len());
                    ui.label(egui::RichText::new(files).small().weak());
                    ui.separator();
                    ui.label(egui::RichText::new(g.folder.display().to_string()).small().weak());
                    if self.blender.as_ref().is_some_and(BlenderLink::connected) {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let on = egui::Color32::from_rgb(110, 200, 120);
                            ui.label(egui::RichText::new(format!("{}  Blender", icon::CIRCLE)).small().color(on))
                                .on_hover_text("The Blender add-on is connected");
                        });
                    }
                });
            }
        });

        // Blender's requests, answered between frames.
        if let (Some(b), Some(ed), Some(g)) = (&mut self.blender, &mut self.editor, &game) {
            b.poll(ed, g);
        }
        // The project's files changed the game data: the caches start over.
        if let (Some(ed), Some(g)) = (&mut self.editor, &game)
            && let Some(data) = ed.take_data_change()
        {
            g.set_data(data);
        }

        // Ctrl+S saves the project's files, wherever the focus is.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S))
            && let Some(ed) = &mut self.editor
        {
            self.project_pane.save_all(ed, &mut self.settings);
        }

        // The tour: the first time the game is open (once the loading screen
        // is gone), with its step's panel in front.
        if self.settings.get(TOUR_DONE).is_none() && self.loading.is_none() && !self.tour.is_open() {
            match (&self.auto, std::env::var("REDFORGE_TOUR").ok().and_then(|s| s.parse().ok())) {
                (None, _) => self.tour.start(),
                (Some(_), Some(step)) => self.tour.start_at(step),
                (Some(_), None) => {}
            }
        }
        match self.tour.target() {
            Some(tour::PROJECT) => self.show_side_tab(Tab::Project, true),
            Some(tour::HIERARCHY) => self.show_side_tab(Tab::Hierarchy, true),
            Some(tour::GAME_FILES) => self.show_side_tab(Tab::GameFiles, true),
            Some(tour::PROPERTIES) => self.show_side_tab(Tab::Properties, true),
            Some(tour::OBJECTS) => self.show_side_tab(Tab::Objects, true),
            _ => {}
        }
        self.panel_rects.clear();
        let mut action = None;
        let mut open_world = None;
        // The islands, with the gap around them too.
        let gap = crate::theme::GAP;
        let around = egui::Frame::new().fill(crate::theme::BG).inner_margin(egui::Margin::same(gap));
        egui::CentralPanel::default().frame(around).show(ui, |ui| {
            let mut tabs = Tabs {
                gpu: &self.gpu,
                game: game.as_ref(),
                editor: self.editor.as_mut(),
                tools: &mut self.tools,
                properties: &mut self.properties,
                project: &mut self.project_pane,
                hierarchy: &mut self.hierarchy,
                browser: &mut self.browser,
                settings: &mut self.settings,
                action: &mut action,
                objects: &mut self.objects,
                open_world: &mut open_world,
                rects: &mut self.panel_rects,
                icon: &self.icon,
            };
            DockArea::new(&mut self.dock).style(crate::theme::dock(ui.style())).show_inside(ui, &mut tabs);
        });
        if let (Some(a), Some(g)) = (action, &game) {
            self.open(ctx, g, a);
        }
        if let (Some(place), Some(g)) = (open_world, &game) {
            self.open_world(g, (!place.is_empty()).then_some(place.as_str()));
        }
        if self.tour.ui(ctx, &self.panel_rects) == TourEvent::Closed {
            self.settings.set(TOUR_DONE, "true");
        }
        if std::mem::take(&mut self.objects.tile_size_changed) {
            self.settings.set(crate::objects::TILE_SIZE, self.objects.tile_size.to_string());
        }

        if let Some(g) = &game {
            self.auto_shot(ctx, g);
        }

        // Viewers waiting on a background job need frames to notice it finishing.
        let waiting = self.dock.iter_all_tabs().any(|(_, t)| match t {
            Tab::Model(_) | Tab::Anim(_) | Tab::Texture(_) | Tab::Text(_) | Tab::Hex(_) | Tab::Map(_) => true,
            // Objects asks for frames itself while its thumbnails load.
            Tab::Welcome | Tab::Properties | Tab::Project | Tab::Hierarchy | Tab::GameFiles | Tab::Objects => false,
        });
        if waiting {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}
