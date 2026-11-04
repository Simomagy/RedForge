use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use eframe::egui;
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use rf_editor::Editor;
use rf_render::{GpuContext, Renderer, TexturePreview};

use crate::blender::BlenderLink;
use crate::browser::{Browser, BrowserAction};
use crate::game::{Game, LoadMessage, find_installs, is_game_folder, start_loading};
use crate::hierarchy::Hierarchy;
use crate::map::MapView;
use crate::map::tools::Tools;
use crate::objects::Objects;
use crate::project_pane::{Ask, ProjectPane};
use crate::properties::Properties;
use crate::settings::{GAME_FOLDER, Settings};
use crate::theme::{self, icon};
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
        /// Where the launchers installed the game.
        found: Vec<(PathBuf, &'static str)>,
        /// The game open before (File > Game folder…), to go back to.
        previous: Option<Arc<Game>>,
    },
    Loading {
        rx: Receiver<LoadMessage>,
        status: String,
        started: Instant,
    },
    Ready(Arc<Game>),
}

impl GameState {
    /// Asking for the game folder: `input` to start with, or the first
    /// install found.
    fn ask(input: String, error: Option<String>, previous: Option<Arc<Game>>) -> Self {
        let found = find_installs();
        let input = match (input.is_empty(), found.first()) {
            (true, Some((dir, _))) => dir.display().to_string(),
            _ => input,
        };
        GameState::NoFolder { input, error, found, previous }
    }
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
/// `REDFORGE_OPEN` can also be `welcome` (nothing opened), `folder` (the game
/// folder's dialog), `world`, `world:<place>` or
/// `world:x,y,z[,yaw,pitch]` for the world map, and
/// `REDFORGE_SCREENSHOT_DELAY` sets the seconds to wait (6 by default), and
/// `REDFORGE_SELECT_CENTRE=1` selects what's in the middle of the world map
/// two seconds before, with the Move tool; `REDFORGE_TEST_DRAG=1` then drags
/// the gizmo's X arrow 100 pixels with made-up mouse events and logs what
/// happened (`=copy`: with Shift, dragging copies). `REDFORGE_OBJECTS_SEARCH=<text>` shows the Objects pane beside
/// the world map with that search, and chooses the first object shown.
/// `REDFORGE_TEST_QUIT=1` closes the window a second and a half before, to
/// see the question about unsaved changes; `REDFORGE_TEST_KEY=<key>` (`F1`,
/// `ctrl+d`...) presses a key a second before.
struct AutoShot {
    open: String,
    out: PathBuf,
    delay: f32,
    select_centre: bool,
    drag_test: Option<DragTest>,
    /// Whether to close the window before the screenshot, and if it was.
    quit_test: Option<bool>,
    /// A key to press before the screenshot, and how far its press got.
    key_test: Option<(egui::Key, egui::Modifiers, u8)>,
    tab_shown: bool,
    objects_search: Option<String>,
    opened: Option<Instant>,
    requested: bool,
}

/// Points a world map at a place (`Valentine`) or a camera (`x,y,z[,yaw,pitch]`),
/// with or without a leading `:` (as `AutoShot`'s `world:<target>` has it);
/// false when there's no such place.
fn aim(v: &mut MapView, editor: &Editor, target: &str) -> bool {
    let at = target.strip_prefix(':').unwrap_or(target).trim();
    if at.is_empty() {
        return true;
    }
    let numbers: Vec<f32> = at.split(',').filter_map(|n| n.trim().parse().ok()).collect();
    if numbers.len() >= 3 {
        let (yaw, pitch) = (numbers.get(3).copied().unwrap_or(0.0), numbers.get(4).copied().unwrap_or(-0.3));
        v.set_camera(glam::Vec3::new(numbers[0], numbers[1], numbers[2]), yaw, pitch);
        return true;
    }
    let Some((_, p)) = editor.world.places().into_iter().find(|(n, _)| n.eq_ignore_ascii_case(at)) else {
        return false;
    };
    v.view_place(p.into());
    true
}

/// A tab's kind and file, to find it open already.
fn tab_key(t: &Tab) -> Option<(&'static str, &str)> {
    Some(match t {
        Tab::Model(v) => ("model", v.path.as_str()),
        Tab::Anim(v) => ("anim", v.path.as_str()),
        Tab::Texture(v) => ("texture", v.path.as_str()),
        Tab::Text(v) => ("text", v.path.as_str()),
        Tab::Hex(v) => ("hex", v.path.as_str()),
        Tab::Map(v) => ("map", v.path.as_str()),
        _ => return None,
    })
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
    /// A project to open once the game is loaded: from the command line,
    /// another RedForge, or dropped on the window.
    incoming: Option<PathBuf>,
    /// The projects other RedForges hand over.
    from_others: Option<Receiver<PathBuf>>,
    /// Game files dropped on the window, to add to the project.
    adding: Vec<PathBuf>,
    /// Files are dragged over the window (Wayland, where egui doesn't say).
    dragging: bool,
    /// The drags over the window on Wayland, which winit doesn't report.
    #[cfg(target_os = "linux")]
    wayland_drops: Option<Receiver<crate::wayland_drop::WaylandDrop>>,
    /// The window closes: the unsaved changes were saved or let go.
    quitting: bool,
    /// Help's windows.
    shortcuts_open: bool,
    about_open: bool,
    /// The window's title as last set.
    title: String,
    /// The world map was open last time: it opens again once the game is.
    reopen_world: bool,
}

/// The setting that's set once the tour was seen (or skipped).
const TOUR_DONE: &str = "Onboarding.Done";

impl RedForgeApp {
    pub fn new(cc: &eframe::CreationContext, project: Option<PathBuf>) -> Self {
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
                drag_test: std::env::var("REDFORGE_TEST_DRAG")
                    .ok()
                    .map(|v| DragTest { copy: v == "copy", ..Default::default() }),
                quit_test: std::env::var("REDFORGE_TEST_QUIT").is_ok().then_some(false),
                key_test: std::env::var("REDFORGE_TEST_KEY").ok().and_then(|k| {
                    let mut parts: Vec<&str> = k.split('+').collect();
                    let name = parts.pop()?;
                    let mut modifiers = egui::Modifiers::NONE;
                    for m in parts {
                        match m.to_lowercase().as_str() {
                            "ctrl" => modifiers |= egui::Modifiers::COMMAND,
                            "shift" => modifiers |= egui::Modifiers::SHIFT,
                            "alt" => modifiers |= egui::Modifiers::ALT,
                            _ => return None,
                        }
                    }
                    let key = egui::Key::from_name(&name.to_uppercase()).or_else(|| egui::Key::from_name(name))?;
                    Some((key, modifiers, 0))
                }),
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
            game: GameState::ask(folder.clone().unwrap_or_default(), None, None),
            dock: default_dock(),
            browser: Browser::with_search(std::env::var("REDFORGE_FILES_SEARCH").unwrap_or_default()),
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
            incoming: project,
            from_others: {
                let c = cc.egui_ctx.clone();
                crate::instance::listen(move || c.request_repaint())
            },
            dragging: std::env::var("REDFORGE_DROP_HOVER").is_ok(),
            // REDFORGE_TEST_DROP=<path;path...>: as if they were dropped (for testing).
            adding: std::env::var("REDFORGE_TEST_DROP")
                .map(|l| l.split(';').filter(|p| !p.is_empty()).map(PathBuf::from).collect())
                .unwrap_or_default(),
            #[cfg(target_os = "linux")]
            wayland_drops: crate::wayland_drop::start(cc),
            quitting: false,
            shortcuts_open: false,
            about_open: false,
            title: "RedForge".into(),
            reopen_world: false,
        };
        // The layout, the world map and the tools as they were last time.
        if app.keeps_session() {
            let saved = cc.storage.and_then(|s| eframe::get_value(s, crate::session::DOCK));
            if let Some((dock, world)) = saved.as_ref().and_then(crate::session::restore_dock) {
                app.dock = dock;
                app.reopen_world = world;
            }
            crate::session::restore_tools(&mut app.tools, &app.settings);
        }
        // `REDFORGE_OPEN=folder`: the game folder's dialog, for its screenshot.
        if let Some(f) = folder.filter(|_| app.auto.as_ref().is_none_or(|a| a.open != "folder")) {
            app.load_game(&cc.egui_ctx, PathBuf::from(f));
        }
        app
    }

    /// Whether the layout, the world map and the tools are kept between runs:
    /// not while testing, unless `REDFORGE_SESSION=1`.
    fn keeps_session(&self) -> bool {
        self.auto.is_none() || std::env::var("REDFORGE_SESSION").is_ok()
    }

    /// The window's title: the project, with a dot while files have unsaved
    /// changes.
    fn set_title(&mut self, ctx: &egui::Context) {
        let title = match self.editor.as_ref().and_then(|e| Some((e.project.as_ref()?.name(), e.has_unsaved()))) {
            Some((name, dirty)) => format!("{name}{} – RedForge", if dirty { " •" } else { "" }),
            None if self.editor.as_ref().is_some_and(Editor::has_unsaved) => "• RedForge".into(),
            None => "RedForge".into(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    /// File > Game folder…: the folder's dialog, with Cancel back to the game.
    fn ask_game_folder(&mut self) {
        let input = self.settings.get(GAME_FOLDER).unwrap_or_default().to_owned();
        let previous = match &self.game {
            GameState::Ready(g) => Some(g.clone()),
            _ => None,
        };
        self.game = GameState::ask(input, None, previous);
    }

    fn load_game(&mut self, ctx: &egui::Context, folder: PathBuf) {
        if !is_game_folder(&folder) {
            let previous = match &self.game {
                GameState::NoFolder { previous, .. } => previous.clone(),
                _ => None,
            };
            let error = Some(format!("RDR2.exe isn't in {}", folder.display()));
            self.game = GameState::ask(folder.display().to_string(), error, previous);
            return;
        }
        self.settings.set(GAME_FOLDER, folder.display().to_string());
        let c = ctx.clone();
        let rx = start_loading(folder, move || c.request_repaint());
        self.game = GameState::Loading { rx, status: "Starting…".into(), started: Instant::now() };
        self.loading = Some(crate::loading::LoadingScreen::new());
    }

    fn open(&mut self, ctx: &egui::Context, game: &Arc<Game>, action: BrowserAction) {
        // Open already: brought to the front.
        let path = game.path(match action {
            BrowserAction::Open(f) | BrowserAction::OpenXml(f) | BrowserAction::OpenHex(f) => f,
        });
        let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
        let kind = match (&action, ext.as_str()) {
            (BrowserAction::OpenHex(_), _) => "hex",
            (BrowserAction::Open(_), "ydr" | "ydd" | "yft") => "model",
            (BrowserAction::Open(_), "ycd") => "anim",
            (BrowserAction::Open(_), "ytd") => "texture",
            (BrowserAction::Open(_), "ymap") => "map",
            _ => "text",
        };
        if self.focus_tab(|t| tab_key(t).is_some_and(|(k, p)| k == kind && p == path)) {
            return;
        }
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
    /// One world map only: open already, it's brought to the front and
    /// looks at the place.
    fn open_world(&mut self, game: &Arc<Game>, place: Option<&str>) -> bool {
        self.editor.get_or_insert_with(|| Editor::new(game.fs.clone(), game.hashes.clone(), game.data()));
        let place = place.unwrap_or_default();
        let is_world = |t: &Tab| matches!(t, Tab::Map(v) if v.is_world());
        if self.focus_tab(is_world) {
            let editor = self.editor.as_ref().unwrap();
            let open = self.dock.iter_all_tabs_mut().find_map(|(_, t)| match t {
                Tab::Map(v) if v.is_world() => Some(v),
                _ => None,
            });
            return open.is_some_and(|v| aim(v, editor, place));
        }
        let keeps = self.keeps_session();
        let editor = self.editor.as_mut().unwrap();
        let mut view = MapView::world(game, &self.gpu, editor);
        if keeps {
            // As it was last time; at the project, when it's another one.
            let project = editor.project.as_ref().and_then(|p| p.file.clone());
            let kept = crate::session::kept_map(&self.settings);
            if let Some(s) = &kept {
                view.set_state(s, editor);
            }
            let same = kept.is_some() && crate::session::camera_project(&self.settings) == project;
            if !same && let Some(c) = editor.project_centre() {
                view.view_place(c);
            }
        }
        if !aim(&mut view, editor, place) {
            return false;
        }
        self.show_tab(Tab::Map(Box::new(view)));
        self.show_side_tab(Tab::Properties, false);
        self.show_side_tab(Tab::Objects, false);
        true
    }

    /// A pane of the left column (View's): to the front, or back beside the
    /// others.
    fn show_pane(&mut self, tab: Tab) {
        let kind = std::mem::discriminant(&tab);
        if self.focus_tab(|t| std::mem::discriminant(t) == kind) {
            return;
        }
        let left = |t: &Tab| matches!(t, Tab::Project | Tab::GameFiles | Tab::Hierarchy);
        match self.dock.find_tab_from(left).and_then(|p| self.dock.leaf_mut(p.node_path()).ok()) {
            Some(leaf) => leaf.append_tab(tab),
            None => {
                self.dock.main_surface_mut().split_left(NodeIndex::root(), 0.22, vec![tab]);
            }
        }
    }

    /// View > Reset layout: the panes where they start, with the documents.
    fn reset_layout(&mut self) {
        let mut documents = Vec::new();
        self.dock.retain_tabs(|t| {
            if tab_key(t).is_some() {
                documents.push(std::mem::replace(t, Tab::Welcome));
            }
            false
        });
        self.dock = default_dock();
        let world = documents.iter().any(|t| matches!(t, Tab::Map(v) if v.is_world()));
        for d in documents {
            self.show_tab(d);
        }
        if world {
            self.show_side_tab(Tab::Properties, false);
            self.show_side_tab(Tab::Objects, false);
        }
    }

    /// Brings the first tab `which` matches to the front; false when none does.
    fn focus_tab(&mut self, which: impl Fn(&Tab) -> bool) -> bool {
        let Some(path) = self.dock.find_tab_from(which) else { return false };
        self.dock.set_focused_node_and_surface(path.node_path());
        let _ = self.dock.set_active_tab(path);
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

    /// The projects from outside: handed over by another RedForge (which
    /// brings this one to the front), or dropped on the window (egui's drops
    /// on X11 and Windows, ours on Wayland), with a card while files are
    /// dragged over it. They open once the game is loaded; game files
    /// dropped join the project then.
    fn take_projects(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.from_others {
            while let Ok(file) = rx.try_recv() {
                self.incoming = Some(file);
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                let attention = egui::UserAttentionType::Informational;
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(attention));
            }
        }
        let (hovered, dropped) = ctx.input(|i| {
            let hovered: Vec<Option<PathBuf>> = i.raw.hovered_files.iter().map(|f| f.path.clone()).collect();
            let dropped: Vec<PathBuf> = i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect();
            (hovered, dropped)
        });
        #[cfg(target_os = "linux")]
        let dropped = self.wayland_drops(dropped);
        if !dropped.is_empty() {
            match dropped.iter().find(|f| is_project(f)) {
                Some(file) => self.incoming = Some(file.clone()),
                None if dropped.iter().any(|f| crate::project_pane::can_add(f)) => self.adding.extend(dropped),
                None => self.tools.fail(
                    "Drop a RedForge project (.rfproj) to open it, or game files (ydr, ytd, ymap…, or their XML) \
                     to add them",
                ),
            }
        }
        if hovered.is_empty() && !self.dragging {
            return;
        }
        // X11 and Windows say which files while they're dragged, Wayland only
        // at the drop.
        let known: Vec<&PathBuf> = hovered.iter().flatten().collect();
        let what = if known.is_empty() {
            Dropping::Unknown
        } else if known.iter().any(|f| is_project(f)) {
            Dropping::Project
        } else if known.iter().any(|f| crate::project_pane::can_add(f)) {
            Dropping::Files
        } else {
            Dropping::Refused
        };
        drop_card(ctx, what);
    }

    /// What Wayland's drags did since the last frame: whether files are over
    /// the window, and the files dropped (added to `dropped`).
    #[cfg(target_os = "linux")]
    fn wayland_drops(&mut self, mut dropped: Vec<PathBuf>) -> Vec<PathBuf> {
        use crate::wayland_drop::WaylandDrop;
        if let Some(rx) = &self.wayland_drops {
            while let Ok(event) = rx.try_recv() {
                match event {
                    WaylandDrop::Hover(on) => self.dragging = on,
                    WaylandDrop::Files(files) => dropped.extend(files),
                }
            }
        }
        dropped
    }

    /// The game folder's dialog: a card with the installs the launchers
    /// know of, the folder, and Open.
    fn folder_ui(&mut self, ui: &mut egui::Ui) {
        let GameState::NoFolder { input, error, found, previous } = &mut self.game else { return };
        let (mut chosen, mut back) = (None, false);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BG)).show(ui, |_| {});
        let card = theme::card(28);
        egui::Area::new(egui::Id::new("game-folder")).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(
            ui.ctx(),
            |ui| {
                card.show(ui, |ui| {
                    ui.set_width(520.0);
                    ui.horizontal(|ui| {
                        ui.add(egui::Image::new(&self.icon).fit_to_exact_size(egui::vec2(52.0, 52.0)));
                        ui.vertical(|ui| {
                            let title = if previous.is_some() { "The game folder" } else { "Welcome to RedForge" };
                            ui.label(egui::RichText::new(title).font(theme::heading(theme::SIZE + 10.0)));
                            ui.label(
                                egui::RichText::new("Where is Red Dead Redemption 2 installed?").color(theme::WEAK),
                            );
                        });
                    });
                    ui.add_space(10.0);
                    ui.label(
                        "RedForge reads the game's files from its folder, the one with RDR2.exe, and never \
                         changes them.",
                    );
                    ui.add_space(14.0);
                    if !found.is_empty() {
                        ui.label(egui::RichText::new("Found on this computer").font(theme::strong(theme::SIZE)));
                        for (dir, launcher) in found.iter() {
                            let path = dir.display().to_string();
                            let selected = input.trim() == path;
                            let row = install_row(ui, selected, launcher, &path);
                            if row.clicked() {
                                *input = path.clone();
                            }
                            if row.double_clicked() {
                                chosen = Some(dir.clone());
                            }
                        }
                        ui.add_space(10.0);
                    }
                    ui.label(egui::RichText::new("Folder").font(theme::strong(theme::SIZE)));
                    ui.horizontal(|ui| {
                        let browse = 96.0;
                        let width = ui.available_width() - browse - ui.spacing().item_spacing.x;
                        let field = ui.add(
                            egui::TextEdit::singleline(input)
                                .desired_width(width)
                                .min_size(egui::vec2(width, 30.0))
                                .vertical_align(egui::Align::Center)
                                .hint_text("The folder with RDR2.exe"),
                        );
                        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            chosen = Some(PathBuf::from(input.trim()));
                        }
                        let button = egui::Button::new(theme::icon_text(icon::FOLDER_OPEN, "Browse…"))
                            .min_size(egui::vec2(browse, 30.0));
                        if ui.add(button).clicked()
                            && let Some(dir) =
                                rfd::FileDialog::new().set_title("The Red Dead Redemption 2 folder").pick_folder()
                        {
                            *input = dir.display().to_string();
                        }
                    });
                    if let Some(e) = error {
                        ui.add_space(4.0);
                        let red = ui.visuals().error_fg_color;
                        ui.label(egui::RichText::new(format!("{}  {e}", icon::WARNING)).color(red));
                    }
                    ui.add_space(18.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(!input.trim().is_empty(), theme::primary("Open")).clicked() {
                            chosen = Some(PathBuf::from(input.trim()));
                        }
                        if previous.is_some() && ui.add(theme::secondary("Cancel")).clicked() {
                            back = true;
                        }
                    });
                });
            },
        );
        // The folder of the game open: nothing to load again.
        let same = |g: &Arc<Game>| chosen.as_deref().is_some_and(|f| same_folder(f, &g.folder));
        if let GameState::NoFolder { previous: Some(g), .. } = &self.game
            && (back || same(g))
        {
            self.game = GameState::Ready(g.clone());
        } else if let Some(f) = chosen {
            let ctx = ui.ctx().clone();
            self.load_game(&ctx, f);
        }
    }
}

fn same_folder(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn is_project(file: &Path) -> bool {
    file.extension().is_some_and(|e| e.eq_ignore_ascii_case("rfproj"))
}

/// What files dragged over the window would do.
#[derive(PartialEq)]
enum Dropping {
    Project,
    Files,
    /// Wayland doesn't say which files until they're dropped.
    Unknown,
    Refused,
}

/// The card over the window while files are dragged over it: what dropping
/// them does.
fn drop_card(ctx: &egui::Context, what: Dropping) {
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop-card")));
    let screen = ctx.content_rect();
    painter.rect_filled(screen, 0, egui::Color32::from_black_alpha(150));
    let card = egui::Rect::from_center_size(screen.center(), egui::vec2(440.0, 180.0));
    let colour = if what == Dropping::Refused { theme::WEAK } else { theme::ACCENT };
    let stroke = egui::Stroke::new(2.0, colour);
    painter.rect(card, theme::BIG_RADIUS, theme::PANEL, stroke, egui::StrokeKind::Inside);
    let (glyph, title, note) = match what {
        Dropping::Project => {
            (icon::FILE_ARROW_DOWN, "Drop to open the project", "RedForge asks before closing the one open")
        }
        Dropping::Files => {
            (icon::FILE_PLUS, "Drop to add to the project", "XML from Sollumz or CodeX becomes the game's file")
        }
        Dropping::Unknown => {
            (icon::FILE_ARROW_DOWN, "Drop a project to open it", "or game files and their XML to add them")
        }
        Dropping::Refused => {
            (icon::PROHIBIT, "Not a project or a game file", "Projects open here, game files join the project")
        }
    };
    let at = |dy: f32| card.center() + egui::vec2(0.0, dy);
    let centre = egui::Align2::CENTER_CENTER;
    painter.text(at(-38.0), centre, glyph, egui::FontId::proportional(46.0), colour);
    painter.text(at(20.0), centre, title, theme::heading(theme::SIZE + 6.0), theme::TEXT);
    painter.text(at(50.0), centre, note, egui::FontId::proportional(theme::SIZE - 1.0), theme::WEAK);
}

/// An install in the game folder's dialog: the launcher and the folder.
fn install_row(ui: &mut egui::Ui, selected: bool, launcher: &str, path: &str) -> egui::Response {
    let glyph = if launcher == "Steam" { icon::STEAM_LOGO } else { icon::GAME_CONTROLLER };
    let frame = egui::Frame::new()
        .fill(if selected { theme::WIDGET } else { theme::CARD })
        .stroke(egui::Stroke::new(1.0, if selected { theme::ACCENT } else { theme::CARD }))
        .corner_radius(egui::CornerRadius::same(theme::RADIUS))
        .inner_margin(egui::Margin::symmetric(12, 8));
    let shown = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let colour = if selected { theme::ACCENT } else { theme::WEAK };
            ui.label(egui::RichText::new(glyph).size(22.0).color(colour));
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(launcher).font(theme::strong(theme::SIZE)));
                ui.add(egui::Label::new(egui::RichText::new(path).small().color(theme::WEAK)).truncate());
            });
        });
    });
    let row = shown.response.interact(egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    if row.hovered() && !selected {
        ui.painter().rect_stroke(
            row.rect,
            egui::CornerRadius::same(theme::RADIUS),
            egui::Stroke::new(1.0, theme::HOVER),
            egui::StrokeKind::Inside,
        );
    }
    row
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
    surface.split_below(left, 0.6, vec![Tab::Hierarchy]);
    dock
}

/// Made-up mouse events for testing the gizmo.
#[derive(Default)]
struct DragTest {
    /// Where to press, and which way to drag.
    start: Option<(egui::Pos2, egui::Vec2)>,
    entity: Option<(rf_editor::EntityId, glam::Vec3)>,
    step: u32,
    /// With Shift held (`REDFORGE_TEST_DRAG=copy`): copies are dragged.
    copy: bool,
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

    /// `REDFORGE_OPEN=folder`: the game folder's dialog's screenshot.
    fn folder_shot(&mut self, ctx: &egui::Context) {
        let Some(a) = self.auto.as_mut().filter(|a| a.open == "folder") else { return };
        ctx.request_repaint();
        let opened = *a.opened.get_or_insert_with(Instant::now);
        if !a.requested && opened.elapsed().as_secs_f32() > a.delay {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            a.requested = true;
        }
        self.save_shot(ctx);
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
            Some(t) if a.quit_test == Some(false) && t.elapsed().as_secs_f32() > a.delay - 1.5 => {
                a.quit_test = Some(true);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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
        self.save_shot(ctx);
    }

    /// Saves the screenshot asked for, when it comes, and quits.
    fn save_shot(&mut self, ctx: &egui::Context) {
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
            self.quitting = true;
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
        let all_places = self.editor.as_deref().map(|e| e.world.places()).unwrap_or_default();
        let places: Vec<String> = all_places.iter().map(|p| p.0.clone()).collect();
        let kept = crate::session::kept_map(self.settings);
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.16).max(24.0));
            ui.add(egui::Image::new(self.icon).fit_to_exact_size(egui::vec2(96.0, 96.0)));
            ui.label(egui::RichText::new("RedForge").font(crate::theme::heading(34.0)));
            ui.label(egui::RichText::new("A map editor for RedM").weak());
            ui.add_space(24.0);
            // Where the camera was last time, or the map's start.
            let (label, tip) = match kept {
                Some(_) => ("Continue where you left off", "The world map, with the camera where it was"),
                None => ("Open the world map", "The game's map, streamed around you (View > World map)"),
            };
            let text = format!("{}  {label}", crate::theme::icon::MAP_TRIFOLD);
            let open = egui::Button::new(egui::RichText::new(text).size(18.0).color(crate::theme::ON_ACCENT))
                .fill(crate::theme::ACCENT)
                .corner_radius(egui::CornerRadius::same(crate::theme::BIG_RADIUS))
                .min_size(egui::vec2(300.0, 44.0));
            let enabled = self.game.is_some();
            if ui.add_enabled(enabled, open).on_hover_text(tip).clicked() {
                *self.open_world = Some(String::new());
            }
            if let Some(k) = &kept {
                let project = self.editor.as_deref().and_then(|e| e.project.as_ref().map(|p| p.name()));
                let near = crate::session::nearest_place(&all_places, k.position).map(|n| format!("near {n}"));
                let what: Vec<String> = project.into_iter().chain(near).collect();
                ui.label(egui::RichText::new(what.join(", ")).small().weak());
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
            // Projects: the recent ones, a new one, one from disk.
            if let Some(ed) = self.editor.as_deref_mut() {
                ui.add_space(28.0);
                ui.label(egui::RichText::new("Projects").font(crate::theme::strong(crate::theme::SIZE + 1.0)));
                ui.add_space(4.0);
                let open = ed.project.as_ref().and_then(|p| p.file.clone());
                let recent: Vec<PathBuf> = crate::project_pane::recent(self.settings)
                    .into_iter()
                    .filter(|f| open.as_deref() != Some(f.as_path()))
                    .take(5)
                    .collect();
                let width = ui.available_width().min(560.0);
                ui.allocate_ui(egui::vec2(width, 0.0), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        use crate::theme::{icon, icon_text};
                        if ui.button(icon_text(icon::PLUS, "New…")).clicked() {
                            self.project.new_asking(ed, self.settings);
                        }
                        if ui.button(icon_text(icon::FOLDER_OPEN, "Open…")).clicked() {
                            self.project.open_dialog(ed, self.settings);
                        }
                        for file in recent {
                            let name = file.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                            let link = ui.link(icon_text(icon::FILE, &name)).on_hover_text(file.display().to_string());
                            if link.clicked() {
                                self.project.open_asking(ed, self.settings, file);
                            }
                        }
                    });
                });
            }
            ui.add_space(28.0);
            ui.label(
                egui::RichText::new(
                    "Or open a model (ydr, ydd, yft), a texture dictionary (ytd), a clip dictionary (ycd) or any other \
                     file from Game Files.",
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
            Tab::Model(_) | Tab::Anim(_) | Tab::Texture(_) | Tab::Text(_) | Tab::Hex(_) | Tab::Map(_) => {
                egui::Id::new(tab_key(tab))
            }
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
            Tab::Map(v) => {
                if v.is_world() {
                    let project = self.editor.as_deref().and_then(|e| e.project.as_ref()?.file.clone());
                    crate::session::remember_map(self.settings, &v.state(), project.as_deref());
                }
                v.free(self.gpu)
            }
            Tab::Objects => self.objects.free(self.gpu),
            _ => {}
        }
        egui_dock::widgets::tab_viewer::OnCloseResponse::Close
    }
}

impl eframe::App for RedForgeApp {
    /// Now and then and when quitting: the layout (in eframe's storage, with
    /// the window), and the world map (in the settings).
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if !self.keeps_session() {
            return;
        }
        eframe::set_value(storage, crate::session::DOCK, &crate::session::saved_dock(&self.dock));
        let world = self.dock.iter_all_tabs().find_map(|(_, t)| match t {
            Tab::Map(v) if v.is_world() => Some(v.state()),
            _ => None,
        });
        if let Some(s) = world {
            let project = self.editor.as_ref().and_then(|e| e.project.as_ref()?.file.clone());
            crate::session::remember_map(&mut self.settings, &s, project.as_deref());
        }
    }

    /// Closing the window (its X, Alt+F4, File > Quit) asks first when files
    /// have unsaved changes; here, as this runs when the window is minimised
    /// too.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested())
            && !self.quitting
            && self.auto.as_ref().is_none_or(|a| a.quit_test.is_some())
            && matches!(self.game, GameState::Ready(_))
            && self.editor.as_ref().is_some_and(Editor::has_unsaved)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            self.project_pane.ask(Ask::Quit);
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        // The game folder's screenshot: no clicks from the real mouse.
        if self.auto.as_ref().is_some_and(|a| a.open == "folder") {
            raw.events.retain(|e| !matches!(e, egui::Event::PointerButton { .. }));
        }
        // REDFORGE_TEST_KEY: down a second before the screenshot, then up
        // (egui takes Ctrl from the modifiers' events).
        if let Some(a) = &mut self.auto
            && let Some((key, modifiers, step)) = &mut a.key_test
            && *step < 2
            && a.opened.is_some_and(|t| t.elapsed().as_secs_f32() > a.delay - 1.0)
        {
            let pressed = *step == 0;
            let held = if pressed { *modifiers } else { egui::Modifiers::NONE };
            if pressed {
                raw.events.push(egui::Event::ModifiersChanged(held));
            }
            raw.events.push(egui::Event::Key {
                key: *key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: *modifiers,
            });
            if !pressed {
                raw.events.push(egui::Event::ModifiersChanged(held));
            }
            *step += 1;
        }
        let Some(d) = self.auto.as_mut().and_then(|a| a.drag_test.as_mut()) else { return };
        let Some((start, dir)) = d.start else { return };
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let at = |k: u32| start + dir * (k as f32 * 10.0);
        if d.copy && (d.step == 0 || d.step == 13) {
            let shift = if d.step == 0 { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE };
            raw.events.push(egui::Event::ModifiersChanged(shift));
        }
        match d.step {
            0 => raw.events.push(egui::Event::PointerMoved(start)),
            1 => raw.events.push(button(start, true)),
            2..=11 => raw.events.push(egui::Event::PointerMoved(at(d.step - 1))),
            12 => raw.events.push(button(at(10), false)),
            // Shift goes up.
            13 => {}
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
                            // Another game folder: the project open goes on in it.
                            let open = self.editor.as_ref().and_then(|e| e.project.as_ref()?.file.clone());
                            if self.incoming.is_none() {
                                self.incoming = open;
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
                            self.game = GameState::ask(input, Some(format!("{e:#}")), None);
                            break;
                        }
                    },
                }
            }
        }

        self.take_projects(ctx);
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Q)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F1)) {
            self.shortcuts_open = !self.shortcuts_open;
        }
        crate::help::shortcuts(ctx, &mut self.shortcuts_open);
        crate::help::about(ctx, &mut self.about_open, &self.icon);

        let game_ready = matches!(self.game, GameState::Ready(_));
        let bar = egui::Frame::new().fill(crate::theme::BG).inner_margin(egui::Margin::symmetric(8, 4));
        egui::Panel::top("menu").frame(bar).show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.add(egui::Image::new(&self.icon).fit_to_exact_size(egui::vec2(18.0, 18.0)));
                ui.menu_button("File", |ui| {
                    if let Some(ed) = self.editor.as_mut().filter(|_| game_ready) {
                        self.project_pane.file_menu(ui, ed, &mut self.settings);
                        ui.separator();
                    }
                    if ui.button("Game folder…").clicked() {
                        match &self.editor {
                            Some(ed) if ed.has_unsaved() && game_ready => self.project_pane.ask(Ask::GameFolder),
                            _ => self.ask_game_folder(),
                        }
                        ui.close();
                    }
                    ui.separator();
                    if ui.add(egui::Button::new("Quit").shortcut_text("Ctrl+Q")).clicked() {
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
                                self.tools.undo(ed);
                            }
                            let redo = ed.redo_labels().next().map(|l| format!("Redo {l}"));
                            if ui
                                .add_enabled(
                                    redo.is_some(),
                                    egui::Button::new(redo.as_deref().unwrap_or("Redo")).shortcut_text("Ctrl+Y"),
                                )
                                .clicked()
                            {
                                self.tools.redo(ed);
                            }
                            ui.separator();
                            let any = !ed.selection().is_empty();
                            let mut result = Ok(());
                            if ui.add_enabled(any, egui::Button::new("Cut").shortcut_text("Ctrl+X")).clicked() {
                                self.tools.copy(ctx, ed);
                                self.tools.delete(ed);
                            }
                            if ui.add_enabled(any, egui::Button::new("Copy").shortcut_text("Ctrl+C")).clicked() {
                                self.tools.copy(ctx, ed);
                            }
                            if ui
                                .add_enabled(ed.can_paste(), egui::Button::new("Paste").shortcut_text("Ctrl+V"))
                                .clicked()
                            {
                                self.tools.paste(ed);
                            }
                            if ui.add_enabled(any, egui::Button::new("Duplicate").shortcut_text("Ctrl+D")).clicked() {
                                self.tools.duplicate(ed);
                            }
                            if ui.add_enabled(any, egui::Button::new("Delete").shortcut_text("Del")).clicked() {
                                self.tools.delete(ed);
                            }
                            ui.separator();
                            let drop = egui::Button::new("Drop to ground").shortcut_text("End");
                            if ui
                                .add_enabled(any, drop)
                                .on_hover_text("Onto the terrain or building under it")
                                .clicked()
                            {
                                self.tools.drop = true;
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
                                self.tools.fail(e);
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
                                self.show_pane(tab);
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
                        ui.separator();
                        if ui.button("Reset layout").on_hover_text("The panes where they are at first").clicked() {
                            self.reset_layout();
                            ui.close();
                        }
                    });
                }
                ui.menu_button("Help", |ui| {
                    if ui.add(egui::Button::new("Keyboard shortcuts").shortcut_text("F1")).clicked() {
                        self.shortcuts_open = true;
                        ui.close();
                    }
                    if ui.add_enabled(game_ready, egui::Button::new("Tour of the panels")).clicked() {
                        self.tour.start();
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("About RedForge").clicked() {
                        self.about_open = true;
                        ui.close();
                    }
                });
            });
        });

        let game = match &self.game {
            GameState::Ready(g) => Some(g.clone()),
            GameState::NoFolder { .. } => {
                self.folder_ui(ui);
                self.folder_shot(ctx);
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

        // The project's and the editing keys, wherever the focus is (but in
        // a text field, or while a question waits).
        if let Some(ed) = &mut self.editor {
            let (pane, settings) = (&mut self.project_pane, &mut self.settings);
            let key = |k| ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, k));
            if key(egui::Key::S) {
                pane.save_all(ed, settings);
            } else if key(egui::Key::O) {
                pane.open_dialog(ed, settings);
            } else if key(egui::Key::E) {
                pane.export_now(ed, settings);
            }
            if !ctx.text_edit_focused() && !pane.asking() && !self.tour.is_open() {
                self.tools.edit_keys(ctx, ed);
            }
        }
        // The project to open at start, or from outside, with the Project
        // pane in front; and its questions.
        if let Some(ed) = &mut self.editor {
            let incoming = self.incoming.take();
            if incoming.is_some()
                && let Some(path) = self.dock.find_tab_from(|t| matches!(t, Tab::Project))
            {
                let _ = self.dock.set_active_tab(path);
            }
            match self.project_pane.update(ctx, ed, &mut self.settings, incoming) {
                Some(Ask::Quit) => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Some(Ask::GameFolder) => self.ask_game_folder(),
                None => {}
            }
        }
        if std::mem::take(&mut self.reopen_world)
            && let Some(g) = &game
        {
            self.open_world(g, None);
        }
        // Game files dropped: into the project, with its pane in front.
        if !self.adding.is_empty()
            && let Some(ed) = &mut self.editor
        {
            let files = std::mem::take(&mut self.adding);
            self.project_pane.add_files(ed, &mut self.settings, files);
            self.focus_tab(|t| matches!(t, Tab::Project));
        }
        if self.keeps_session() {
            crate::session::remember_tools(&self.tools, &mut self.settings);
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

        if let Some((text, error)) = self.project_pane.toast.take() {
            if error {
                self.tools.fail(text);
            } else {
                self.tools.say(text);
            }
        }
        crate::help::toast(ctx, &self.tools);
        self.set_title(ctx);

        if let Some(g) = &game {
            self.auto_shot(ctx, g);
        }

        // Viewers waiting on a background job need frames to notice it finishing.
        let waiting = self.dock.iter_all_tabs().any(|(_, t)| match t {
            Tab::Model(_) | Tab::Anim(_) | Tab::Texture(_) | Tab::Text(_) | Tab::Hex(_) => true,
            // The map and Objects ask for frames themselves while they load.
            Tab::Map(_)
            | Tab::Welcome
            | Tab::Properties
            | Tab::Project
            | Tab::Hierarchy
            | Tab::GameFiles
            | Tab::Objects => false,
        });
        if waiting {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}
