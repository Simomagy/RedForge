//! The Game Files pane: the user's own folders and the game's archives as
//! trees (only the rows in view are laid out), and search over every path
//! (on a thread of its own: half a million paths take a moment).
//!
//! The user's folders are folders on disk (a RedM resource, exported
//! models...), kept in the settings. Their files open like the game's: one is
//! [mounted](rf_formats::rpf8::Rpf8FileSystem::mount) when it's opened.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui::{self, Align, Align2, Color32, CornerRadius, FontId, Id, Layout, Rect, RichText, Sense, Vec2};
use rf_formats::rpf8::FileRef;

use crate::export::How;
use crate::game::{Folder, Game};
use crate::settings::Settings;
use crate::theme::{self, icon};

/// Most files a search lists.
const MAX_RESULTS: usize = 3000;
/// Most files read from one of the user's folders (it could be a whole disk).
const MAX_SCANNED: usize = 200_000;
/// The setting the user's folders are kept in, `|` between them.
const FOLDERS: &str = "GameFiles.Folders";
/// How far a row is from its folder's.
const INDENT: f32 = 14.0;

/// The sections of the tree.
const MINE: usize = 0;
const GAME: usize = 1;

/// A file listed: in the game's archives, or on disk in one of the user's
/// folders.
#[derive(Clone, PartialEq, Eq)]
enum Item {
    Game(FileRef),
    Disk(Arc<Path>),
}

impl Item {
    /// Its path: in the archives, or on disk.
    fn path(&self, game: &Game) -> String {
        match self {
            Item::Game(f) => game.path(*f),
            Item::Disk(p) => p.display().to_string(),
        }
    }
}

/// The files to show: all, or one kind.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Kind {
    #[default]
    All,
    Models,
    Textures,
    Maps,
    Collisions,
    Animations,
}

impl Kind {
    const EVERY: [Kind; 6] = [Kind::All, Kind::Models, Kind::Textures, Kind::Maps, Kind::Collisions, Kind::Animations];

    fn name(self) -> &'static str {
        match self {
            Kind::All => "All files",
            Kind::Models => "Models",
            Kind::Textures => "Textures",
            Kind::Maps => "Maps and archetypes",
            Kind::Collisions => "Collisions",
            Kind::Animations => "Animations",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Kind::All => icon::FILES,
            Kind::Models => icon::CUBE,
            Kind::Textures => icon::IMAGE,
            Kind::Maps => icon::MAP_TRIFOLD,
            Kind::Collisions => icon::CUBE_TRANSPARENT,
            Kind::Animations => icon::PERSON_ARMS_SPREAD,
        }
    }

    fn extensions(self) -> &'static [&'static str] {
        match self {
            Kind::All => &[],
            Kind::Models => &["ydr", "ydd", "yft"],
            Kind::Textures => &["ytd"],
            Kind::Maps => &["ymap", "ytyp"],
            Kind::Collisions => &["ybn"],
            Kind::Animations => &["ycd"],
        }
    }

    /// Whether a file of that name is of this kind.
    fn has(self, name: &str) -> bool {
        let ext = extension(name);
        self == Kind::All || self.extensions().iter().any(|e| e.eq_ignore_ascii_case(ext))
    }
}

fn extension(name: &str) -> &str {
    name.rsplit_once('.').map_or("", |(_, e)| e)
}

/// A file's icon and its colour, by what the file is.
fn file_look(name: &str) -> (&'static str, Color32) {
    match extension(name).to_ascii_lowercase().as_str() {
        "ydr" | "ydd" | "yft" => (icon::CUBE, Color32::from_rgb(120, 180, 255)),
        "ytd" => (icon::IMAGE, Color32::from_rgb(230, 170, 90)),
        "ymap" => (icon::MAP_PIN, Color32::from_rgb(140, 210, 130)),
        "ytyp" => (icon::STACK, Color32::from_rgb(140, 210, 130)),
        "ybn" => (icon::CUBE_TRANSPARENT, Color32::from_rgb(200, 140, 220)),
        "ycd" => (icon::PERSON_ARMS_SPREAD, Color32::from_rgb(110, 200, 200)),
        "rpf" => (icon::PACKAGE, theme::WEAK),
        "xml" | "meta" | "ymt" | "json" | "txt" | "lua" | "ini" | "dat" => (icon::FILE_CODE, theme::WEAK),
        _ => (icon::FILE, theme::WEAK),
    }
}

/// A folder on disk as read: its folders that have files, and its files.
#[derive(Default, PartialEq)]
struct DiskFolder {
    path: PathBuf,
    folders: Vec<(String, DiskFolder)>,
    files: Vec<(String, Arc<Path>)>,
    /// How many files are in it, its folders' included.
    count: usize,
}

/// Reads a folder, until `left` files were read. Hidden folders (`.git`) are
/// left out, and so are links to folders (they can loop).
fn scan(dir: &Path, left: &mut usize) -> DiskFolder {
    let mut folder = DiskFolder { path: dir.to_owned(), ..Default::default() };
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if *left == 0 {
            break;
        }
        let Ok(kind) = e.file_type() else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if kind.is_dir() {
            if name.starts_with('.') {
                continue;
            }
            let sub = scan(&path, left);
            if sub.count > 0 {
                folder.folders.push((name, sub));
            }
        } else if kind.is_file() || path.is_file() {
            *left -= 1;
            folder.files.push((name, path.into()));
        }
    }
    folder.folders.sort_by_cached_key(|(n, _)| n.to_lowercase());
    folder.files.sort_by_cached_key(|(n, _)| n.to_lowercase());
    folder.count = folder.files.len() + folder.folders.iter().map(|(_, f)| f.count).sum::<usize>();
    folder
}

/// What reading a folder gives: `None` when it isn't there, and whether
/// there were more files than are read.
type Scanned = (Option<DiskFolder>, bool);

/// One of the user's folders.
struct UserFolder {
    path: PathBuf,
    name: String,
    /// As last read; `None` before, and when it isn't there.
    tree: Option<DiskFolder>,
    missing: bool,
    /// It has more files than are read.
    truncated: bool,
    /// To read (again).
    stale: bool,
    running: Option<Receiver<Scanned>>,
}

impl UserFolder {
    fn new(path: PathBuf) -> Self {
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        Self { path, name, tree: None, missing: false, truncated: false, stale: true, running: None }
    }

    fn id(&self) -> Id {
        Id::new(("game-files-mine", &self.path))
    }

    /// Reads it on a thread.
    fn read(&mut self, ctx: &egui::Context) {
        let (tx, rx) = channel();
        let (path, ctx) = (self.path.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut left = MAX_SCANNED;
            let tree = path.is_dir().then(|| scan(&path, &mut left));
            let _ = tx.send((tree, left == 0));
            ctx.request_repaint();
        });
        self.running = Some(rx);
        self.stale = false;
    }
}

/// A path with `~` for the user's home.
fn short_path(path: &Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(home).ok().map(|rest| Path::new("~").join(rest))) {
        Some(p) => p.display().to_string(),
        None => path.display().to_string(),
    }
}

/// Where a search's result is, short: its first folder (the archive) and
/// its last.
fn short_location(folder: &str) -> String {
    let parts: Vec<&str> = folder.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        [first, _, .., last] if parts.len() > 2 => format!("{first} › … › {last}"),
        _ => parts.join(" › "),
    }
}

/// `12,345`.
fn count_text(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Shows a folder in the system's file manager.
fn reveal(folder: &Path) {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let folder = folder.to_owned();
    std::thread::spawn(move || {
        if let Err(e) = std::process::Command::new(program).arg(&folder).status() {
            log::warn!("{program} {}: {e}", folder.display());
        }
    });
}

/// What a file is opened as.
#[derive(Clone, Copy)]
enum As {
    Itself,
    Xml,
    Hex,
}

/// What was asked for while the rows were drawn, done after.
enum Cmd {
    Toggle(Id),
    Fold(usize),
    Select(Item),
    Open(Item, As),
    /// Files to write into a folder, each at its path there.
    Export(Vec<(Item, PathBuf)>, How),
    Copy(String),
    Reveal(PathBuf),
    Add,
    Refresh(usize),
    Remove(usize),
}

/// A row of the tree.
enum Node<'a> {
    Section(usize),
    /// Nothing in My folders yet: how to add one.
    AddFirst,
    Note(&'static str),
    Mine(usize, &'a UserFolder),
    Disk(&'a str, &'a DiskFolder),
    Game(&'a str, &'a Folder),
    GameFile(&'a str, FileRef),
    DiskFile(&'a str, &'a Arc<Path>),
}

struct Row<'a> {
    depth: u16,
    /// A folder's, for whether it's open.
    id: Id,
    node: Node<'a>,
}

/// How a row looks.
#[derive(Default)]
struct Look<'a> {
    depth: u16,
    /// A folder's caret: open or closed.
    caret: Option<bool>,
    glyph: &'a str,
    tint: Option<Color32>,
    name: &'a str,
    dim: bool,
    /// After the name, dimmed.
    note: &'a str,
    /// At the right end, dimmed.
    tail: &'a str,
    selected: bool,
}

/// A row of the tree or of a search: as [`theme::row`], indented, with a
/// folder's caret and what goes at the right end; the name gives way to it.
fn row(ui: &mut egui::Ui, l: Look) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), theme::ROW), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let painter = ui.painter();
    if l.selected {
        painter.rect_filled(rect, CornerRadius::same(theme::RADIUS), theme::WIDGET);
        let bar = Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height()));
        painter.rect_filled(bar, CornerRadius::same(2), theme::ACCENT);
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(theme::RADIUS), theme::HOVER.gamma_multiply(0.6));
    }
    let font = FontId::proportional(theme::SIZE);
    let small = FontId::proportional(theme::SIZE - 2.0);
    let y = rect.center().y;
    let mut x = rect.min.x + 8.0 + f32::from(l.depth) * INDENT;
    if let Some(open) = l.caret {
        let caret = if open { icon::CARET_DOWN } else { icon::CARET_RIGHT };
        painter.text(egui::pos2(x, y), Align2::LEFT_CENTER, caret, small.clone(), theme::WEAK);
    }
    x += 16.0;
    if !l.glyph.is_empty() {
        let tint = l.tint.unwrap_or(if l.selected { theme::ACCENT } else { theme::WEAK });
        painter.text(egui::pos2(x, y), Align2::LEFT_CENTER, l.glyph, font.clone(), tint);
        x += theme::SIZE + 6.0;
    }
    let mut right = rect.max.x - 8.0;
    if !l.tail.is_empty() {
        let tail = painter.text(egui::pos2(right, y), Align2::RIGHT_CENTER, l.tail, small.clone(), theme::WEAK);
        right = tail.min.x - 8.0;
    }
    let clip = Rect::from_min_max(egui::pos2(x, rect.min.y), egui::pos2(right, rect.max.y));
    let painter = painter.with_clip_rect(clip.intersect(painter.clip_rect()));
    let colour = if l.dim { theme::WEAK } else { theme::TEXT };
    let name = painter.text(egui::pos2(x, y), Align2::LEFT_CENTER, l.name, font, colour);
    if !l.note.is_empty() {
        painter.text(egui::pos2(name.max.x + 10.0, y), Align2::LEFT_CENTER, l.note, small, theme::WEAK);
    }
    resp
}

/// The export commands of a file's or a folder's menu.
fn export_menu(ui: &mut egui::Ui, cmds: &mut Vec<Cmd>, all: bool, files: impl Fn() -> Vec<(Item, PathBuf)>) {
    let items = [
        (How::Xml, "Export as XML…", "As Sollumz reads it, textures as DDS files in a folder next to it"),
        (How::Loose, "Extract…", "As a RedM resource streams it"),
        (How::Uncompressed, "Extract uncompressed…", "Decrypted and decompressed"),
    ];
    for (how, label, tip) in items {
        let label = if all { label.replace('…', " all…") } else { label.to_owned() };
        if ui.button(label).on_hover_text(tip).clicked() {
            cmds.push(Cmd::Export(files(), how));
            ui.close();
        }
    }
}

/// Every file under a folder of the game, with its path from `at`.
fn game_files(folder: &Folder, at: &Path, out: &mut Vec<(Item, PathBuf)>) {
    for (name, f) in &folder.files {
        out.push((Item::Game(*f), at.join(name)));
    }
    for (name, sub) in &folder.folders {
        game_files(sub, &at.join(name), out);
    }
}

/// Every file under a folder on disk, with its path from `at`.
fn disk_files(folder: &DiskFolder, at: &Path, out: &mut Vec<(Item, PathBuf)>) {
    for (name, p) in &folder.files {
        out.push((Item::Disk(p.clone()), at.join(name)));
    }
    for (name, sub) in &folder.folders {
        disk_files(sub, &at.join(name), out);
    }
}

/// Every file under a folder on disk for searching: its path from the
/// user's folder, lowercase.
fn index(folder: &DiskFolder, at: &str, out: &mut Vec<(String, Arc<Path>)>) {
    for (name, p) in &folder.files {
        out.push((format!("{at}/{}", name.to_lowercase()), p.clone()));
    }
    for (name, sub) in &folder.folders {
        index(sub, &format!("{at}/{}", name.to_lowercase()), out);
    }
}

/// What was last searched for: the words, the kind of file, and which
/// listing of the user's folders.
type Searched = (String, Kind, u64);

#[derive(Default)]
pub struct Browser {
    search: String,
    searched: Searched,
    results: Vec<Item>,
    /// The search running, and which one it is (a newer one stops it).
    running: Option<Receiver<Vec<Item>>>,
    generation: Arc<AtomicU64>,
    selected: Option<Item>,
    kind: Kind,
    folders: Vec<UserFolder>,
    /// The user's folders' files for searching, and which listing it is.
    disk: Arc<Vec<(String, Arc<Path>)>>,
    disk_version: u64,
    /// The folders open in the tree.
    open: HashSet<Id>,
    /// The sections closed.
    folded: [bool; 2],
    /// Whether the window had the focus last frame (the user's folders are
    /// read again when it comes back).
    focused: bool,
    /// What went wrong opening a file.
    problem: Option<String>,
}

/// What the user asked for.
pub enum BrowserAction {
    Open(FileRef),
    OpenXml(FileRef),
    OpenHex(FileRef),
    /// Files to write into a folder, each at its path there.
    Export(Vec<(FileRef, PathBuf)>, How),
}

impl Browser {
    /// With the user's folders kept in the settings, and a search to start
    /// with (`REDFORGE_FILES_SEARCH`, for testing).
    pub fn new(settings: &mut Settings, search: String) -> Self {
        let mut browser = Self { search, ..Default::default() };
        let kept = settings.get(FOLDERS).unwrap_or_default();
        browser.folders = kept.split('|').filter(|p| !p.is_empty()).map(|p| UserFolder::new(p.into())).collect();
        // (tests: a folder added as the button adds it)
        if let Some(folder) = std::env::var_os("REDFORGE_TEST_ADD_FOLDER") {
            browser.add(folder.into(), settings);
        }
        browser
    }

    /// Adds one of the user's folders (once), open, and keeps it.
    fn add(&mut self, path: PathBuf, settings: &mut Settings) {
        let same = |u: &UserFolder| u.path == path || u.path.canonicalize().ok() == path.canonicalize().ok();
        let at = match self.folders.iter().position(same) {
            Some(i) => i,
            None => {
                self.folders.push(UserFolder::new(path));
                self.remember(settings);
                self.folders.len() - 1
            }
        };
        self.open.insert(self.folders[at].id());
        self.folded[MINE] = false;
    }

    fn remember(&self, settings: &mut Settings) {
        let paths: Vec<String> = self.folders.iter().map(|u| u.path.display().to_string()).collect();
        settings.set(FOLDERS, paths.join("|"));
    }

    /// The user's folders' files for searching, listed again.
    fn reindex(&mut self) {
        let mut files = Vec::new();
        for u in &self.folders {
            if let Some(tree) = &u.tree {
                index(tree, &u.name.to_lowercase(), &mut files);
            }
        }
        self.disk = Arc::new(files);
        self.disk_version += 1;
    }

    /// Takes what the threads read, and reads the user's folders again when
    /// the window gets the focus back (files were likely written there
    /// meanwhile).
    fn poll(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.focused);
        if focused && !self.focused {
            self.folders.iter_mut().for_each(|u| u.stale = true);
        }
        self.focused = focused;
        let mut changed = false;
        for u in &mut self.folders {
            match u.running.as_ref().map(Receiver::try_recv) {
                Some(Ok((tree, truncated))) => {
                    u.running = None;
                    u.missing = tree.is_none();
                    u.truncated = truncated;
                    if tree != u.tree {
                        u.tree = tree;
                        changed = true;
                    }
                }
                Some(Err(TryRecvError::Disconnected)) => u.running = None,
                Some(Err(TryRecvError::Empty)) | None => {}
            }
            if u.stale && u.running.is_none() {
                u.read(ctx);
            }
        }
        if changed {
            self.reindex();
        }
        if let Some(rx) = &self.running {
            match rx.try_recv() {
                Ok(found) => {
                    self.results = found;
                    self.running = None;
                }
                Err(TryRecvError::Disconnected) => self.running = None,
                Err(TryRecvError::Empty) => {}
            }
        }
    }

    /// Searches every path for all the words of the search, on a thread; a
    /// newer search stops it. The user's files come first.
    fn start_search(&mut self, ctx: &egui::Context, game: &Arc<Game>) {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let words: Vec<String> = self.searched.0.split_whitespace().map(str::to_owned).collect();
        if words.iter().map(String::len).sum::<usize>() < 2 {
            self.running = None;
            self.results.clear();
            return;
        }
        let (tx, rx) = channel();
        self.running = Some(rx);
        let (game, latest, ctx) = (game.clone(), self.generation.clone(), ctx.clone());
        let (disk, kind) = (self.disk.clone(), self.kind);
        std::thread::spawn(move || {
            let wanted = |path: &str| kind.has(path) && words.iter().all(|w| path.contains(w.as_str()));
            let mut found: Vec<Item> =
                disk.iter().filter(|(path, _)| wanted(path)).map(|(_, p)| Item::Disk(p.clone())).collect();
            found.truncate(MAX_RESULTS);
            for (i, (path, f)) in game.paths.iter().enumerate() {
                if i % 8192 == 0 && latest.load(Ordering::Relaxed) != generation {
                    return;
                }
                if found.len() >= MAX_RESULTS {
                    break;
                }
                if wanted(path) {
                    found.push(Item::Game(*f));
                }
            }
            let _ = tx.send(found);
            ctx.request_repaint();
        });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, game: &Arc<Game>, settings: &mut Settings) -> Option<BrowserAction> {
        self.poll(ui.ctx());
        let mut cmds = Vec::new();
        self.bar(ui, &mut cmds);
        let searched = (self.search.trim().to_lowercase(), self.kind, self.disk_version);
        if searched != self.searched {
            self.searched = searched;
            self.start_search(ui.ctx(), game);
        }
        ui.add_space(2.0);

        // The file chosen, at the bottom: its whole path.
        let foot = if self.selected.is_some() || self.problem.is_some() { theme::ROW + 4.0 } else { 0.0 };
        let height = (ui.available_height() - foot).max(theme::ROW);
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if self.searched.0.is_empty() {
                self.tree(ui, game, height, &mut cmds);
            } else {
                self.found(ui, game, height, &mut cmds);
            }
        });
        if foot > 0.0 {
            self.foot(ui, game, &mut cmds);
        }
        self.apply(cmds, ui.ctx(), game, settings)
    }

    /// The search, the kind of files, and Add folder.
    fn bar(&mut self, ui: &mut egui::Ui, cmds: &mut Vec<Cmd>) {
        let size = Vec2::new(ui.available_width(), ui.spacing().interact_size.y);
        ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if theme::tool(ui, false, icon::FOLDER_PLUS, "Add a folder of your own files").clicked() {
                cmds.push(Cmd::Add);
            }
            let filter = if self.kind == Kind::All {
                RichText::new(icon::FUNNEL)
            } else {
                RichText::new(format!("{} {}", icon::FUNNEL, self.kind.name())).color(theme::ACCENT)
            };
            ui.menu_button(filter, |ui| {
                for kind in Kind::EVERY {
                    if ui.selectable_label(self.kind == kind, theme::icon_text(kind.glyph(), kind.name())).clicked() {
                        self.kind = kind;
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("The kind of files to show");
            if !self.search.is_empty() && theme::tool(ui, false, icon::X, "Clear the search").clicked() {
                self.search.clear();
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.label(RichText::new(icon::MAGNIFYING_GLASS).color(theme::WEAK));
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Search all files…")
                        .desired_width(f32::INFINITY),
                );
            });
        });
    }

    /// The tree's rows now: the sections, and what's in the open folders.
    fn rows<'a>(&'a self, game: &'a Game) -> Vec<Row<'a>> {
        let mut rows = Vec::new();
        let section = |s: usize| Row { depth: 0, id: Id::NULL, node: Node::Section(s) };
        rows.push(section(MINE));
        if !self.folded[MINE] {
            if self.folders.is_empty() {
                rows.push(Row { depth: 0, id: Id::NULL, node: Node::AddFirst });
            }
            for (i, u) in self.folders.iter().enumerate() {
                let id = u.id();
                rows.push(Row { depth: 0, id, node: Node::Mine(i, u) });
                if !self.open.contains(&id) {
                    continue;
                }
                match &u.tree {
                    Some(tree) if tree.count > 0 => self.disk_rows(tree, id, 1, &mut rows),
                    Some(_) => rows.push(Row { depth: 1, id, node: Node::Note("No files in this folder") }),
                    None if u.missing => rows.push(Row { depth: 1, id, node: Node::Note("This folder isn't there") }),
                    None => rows.push(Row { depth: 1, id, node: Node::Note("Reading…") }),
                }
                if u.truncated {
                    rows.push(Row { depth: 1, id, node: Node::Note("… and more: too many files to list them all") });
                }
            }
        }
        rows.push(section(GAME));
        if !self.folded[GAME] {
            self.game_rows(&game.tree, Id::new("game-files-game"), 0, &mut rows);
        }
        rows
    }

    fn game_rows<'a>(&self, folder: &'a Folder, id: Id, depth: u16, rows: &mut Vec<Row<'a>>) {
        for (name, sub) in &folder.folders {
            let id = id.with(name);
            rows.push(Row { depth, id, node: Node::Game(name, sub) });
            if self.open.contains(&id) {
                self.game_rows(sub, id, depth + 1, rows);
            }
        }
        let files = folder.files.iter().filter(|(name, _)| self.kind.has(name));
        rows.extend(files.map(|(name, f)| Row { depth, id, node: Node::GameFile(name, *f) }));
    }

    fn disk_rows<'a>(&self, folder: &'a DiskFolder, id: Id, depth: u16, rows: &mut Vec<Row<'a>>) {
        for (name, sub) in &folder.folders {
            let id = id.with(name);
            rows.push(Row { depth, id, node: Node::Disk(name, sub) });
            if self.open.contains(&id) {
                self.disk_rows(sub, id, depth + 1, rows);
            }
        }
        let files = folder.files.iter().filter(|(name, _)| self.kind.has(name));
        rows.extend(files.map(|(name, p)| Row { depth, id, node: Node::DiskFile(name, p) }));
    }

    fn tree(&self, ui: &mut egui::Ui, game: &Game, height: f32, cmds: &mut Vec<Cmd>) {
        let rows = self.rows(game);
        egui::ScrollArea::vertical().id_salt("tree").auto_shrink(false).max_height(height).show_rows(
            ui,
            theme::ROW,
            rows.len(),
            |ui, range| {
                for r in &rows[range] {
                    self.tree_row(ui, game, r, cmds);
                }
            },
        );
    }

    fn tree_row(&self, ui: &mut egui::Ui, game: &Game, r: &Row, cmds: &mut Vec<Cmd>) {
        let open = self.open.contains(&r.id);
        let folder = |glyph: &'static str| if open { icon::FOLDER_OPEN } else { glyph };
        match &r.node {
            Node::Section(s) => self.section(ui, game, *s, cmds),
            Node::AddFirst => {
                let look = Look {
                    glyph: icon::FOLDER_PLUS,
                    name: "Add a folder of your own files…",
                    dim: true,
                    ..Default::default()
                };
                let tip = "A RedM resource, the models you exported… Its files open like the game's, and it's \
                           still here the next time.";
                if row(ui, look).on_hover_text(tip).clicked() {
                    cmds.push(Cmd::Add);
                }
            }
            Node::Note(text) => {
                row(ui, Look { depth: r.depth, name: text, dim: true, ..Default::default() });
            }
            Node::Mine(i, u) => {
                let place = u.path.parent().map(short_path).unwrap_or_default();
                let count = u.tree.as_ref().map(|t| count_text(t.count)).unwrap_or_default();
                let look = Look {
                    caret: Some(open),
                    glyph: if u.missing { icon::WARNING } else { folder(icon::FOLDER_USER) },
                    name: &u.name,
                    dim: u.missing,
                    note: &place,
                    tail: if u.missing { "not found" } else { &count },
                    ..Default::default()
                };
                let resp = row(ui, look).on_hover_text(u.path.display().to_string());
                if resp.clicked() {
                    cmds.push(Cmd::Toggle(r.id));
                }
                resp.context_menu(|ui| {
                    if ui.button("Read again").on_hover_text("Lists the folder's files again").clicked() {
                        cmds.push(Cmd::Refresh(*i));
                        ui.close();
                    }
                    self.disk_folder_menu(ui, &u.name, &u.path, u.tree.as_ref(), cmds);
                    ui.separator();
                    let tip = "Takes the folder out of Game Files. Nothing is deleted on disk.";
                    if ui.button("Remove from Game Files").on_hover_text(tip).clicked() {
                        cmds.push(Cmd::Remove(*i));
                        ui.close();
                    }
                });
            }
            Node::Disk(name, sub) => {
                let count = count_text(sub.count);
                let look = Look {
                    depth: r.depth,
                    caret: Some(open),
                    glyph: folder(icon::FOLDER),
                    name,
                    tail: &count,
                    ..Default::default()
                };
                let resp = row(ui, look);
                if resp.clicked() {
                    cmds.push(Cmd::Toggle(r.id));
                }
                resp.context_menu(|ui| self.disk_folder_menu(ui, name, &sub.path, Some(sub), cmds));
            }
            Node::Game(name, sub) => {
                let count = count_text(sub.count);
                let glyph = if name.ends_with(".rpf") { icon::PACKAGE } else { folder(icon::FOLDER) };
                let look = Look { depth: r.depth, caret: Some(open), glyph, name, tail: &count, ..Default::default() };
                let resp = row(ui, look);
                if resp.clicked() {
                    cmds.push(Cmd::Toggle(r.id));
                }
                // A folder's files, in a folder of its name.
                resp.context_menu(|ui| {
                    export_menu(ui, cmds, true, || {
                        let mut files = Vec::new();
                        game_files(sub, Path::new(name), &mut files);
                        files
                    });
                });
            }
            Node::GameFile(name, f) => self.file_row(ui, game, r.depth, name, "", Item::Game(*f), cmds),
            Node::DiskFile(name, p) => self.file_row(ui, game, r.depth, name, "", Item::Disk((*p).clone()), cmds),
        }
    }

    /// A section's header; My folders' has Add folder at its end.
    fn section(&self, ui: &mut egui::Ui, game: &Game, s: usize, cmds: &mut Vec<Cmd>) {
        let (glyph, name, count) = match s {
            MINE => (
                icon::FOLDER_USER,
                "My folders",
                self.folders.iter().filter_map(|u| u.tree.as_ref()).map(|t| t.count).sum(),
            ),
            _ => (icon::GAME_CONTROLLER, "Game", game.tree.count),
        };
        let right = ui.max_rect().right();
        let resp = theme::section(ui, !self.folded[s], glyph, name, count);
        if resp.clicked() {
            cmds.push(Cmd::Fold(s));
        }
        if s == MINE {
            let at = Rect::from_min_max(egui::pos2(right - 28.0, resp.rect.min.y), egui::pos2(right, resp.rect.max.y));
            let add = egui::Button::new(RichText::new(icon::PLUS).color(theme::WEAK)).frame(false);
            if ui.put(at, add).on_hover_text("Add a folder of your own files").clicked() {
                cmds.push(Cmd::Add);
            }
        }
    }

    /// What a folder on disk's menu has, the user's folders' too.
    fn disk_folder_menu(
        &self,
        ui: &mut egui::Ui,
        name: &str,
        path: &Path,
        tree: Option<&DiskFolder>,
        cmds: &mut Vec<Cmd>,
    ) {
        if ui.button("Show in the file manager").clicked() {
            cmds.push(Cmd::Reveal(path.to_owned()));
            ui.close();
        }
        if ui.button("Copy path").clicked() {
            cmds.push(Cmd::Copy(path.display().to_string()));
            ui.close();
        }
        if let Some(tree) = tree {
            ui.separator();
            export_menu(ui, cmds, true, || {
                let mut files = Vec::new();
                disk_files(tree, Path::new(name), &mut files);
                files
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn file_row(
        &self,
        ui: &mut egui::Ui,
        game: &Game,
        depth: u16,
        name: &str,
        note: &str,
        item: Item,
        cmds: &mut Vec<Cmd>,
    ) {
        let (glyph, tint) = file_look(name);
        let selected = self.selected.as_ref() == Some(&item);
        let look = Look { depth, glyph, tint: Some(tint), name, note, selected, ..Default::default() };
        let resp = row(ui, look).on_hover_ui(|ui| {
            ui.label(item.path(game));
        });
        if resp.clicked() {
            cmds.push(Cmd::Select(item.clone()));
        }
        if resp.double_clicked() {
            cmds.push(Cmd::Open(item.clone(), As::Itself));
        }
        resp.context_menu(|ui| {
            for (label, how) in [("Open", As::Itself), ("View as XML", As::Xml), ("View as hex", As::Hex)] {
                if ui.button(label).clicked() {
                    cmds.push(Cmd::Select(item.clone()));
                    cmds.push(Cmd::Open(item.clone(), how));
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("Copy path").clicked() {
                cmds.push(Cmd::Copy(item.path(game)));
                ui.close();
            }
            if let Item::Disk(p) = &item
                && let Some(folder) = p.parent()
                && ui.button("Show in the file manager").clicked()
            {
                cmds.push(Cmd::Reveal(folder.to_owned()));
                ui.close();
            }
            ui.separator();
            export_menu(ui, cmds, false, || vec![(item.clone(), PathBuf::from(name))]);
        });
    }

    /// A search's results: how many, and each with where it is.
    fn found(&self, ui: &mut egui::Ui, game: &Game, height: f32, cmds: &mut Vec<Cmd>) {
        let words: usize = self.searched.0.split_whitespace().map(str::len).sum();
        let count = if words < 2 {
            "Type two letters or more".to_owned()
        } else if self.running.is_some() {
            "Searching…".to_owned()
        } else {
            let more = if self.results.len() >= MAX_RESULTS { "+" } else { "" };
            let of = if self.kind == Kind::All { String::new() } else { format!(" in {}", self.kind.name()) };
            match self.results.len() {
                0 => format!("Nothing found{of}"),
                1 => format!("1 file{of}"),
                n => format!("{}{more} files{of}", count_text(n)),
            }
        };
        let status = theme::ROW - 2.0;
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), status),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.add_space(8.0);
                ui.label(RichText::new(count).small().color(theme::WEAK));
            },
        );
        let results = &self.results;
        egui::ScrollArea::vertical()
            .id_salt("found")
            .auto_shrink(false)
            .max_height((height - status).max(theme::ROW))
            .show_rows(ui, theme::ROW, results.len(), |ui, range| {
                for item in &results[range] {
                    let path = match item {
                        Item::Game(f) => game.path(*f),
                        // From the user's folder, as it was searched.
                        Item::Disk(p) => self.path_in_mine(p),
                    };
                    let (folder, name) = path.rsplit_once(['/', '\\']).unwrap_or(("", &path));
                    // Files of the same name apart: where each is, dimmed.
                    self.file_row(ui, game, 0, name, &short_location(folder), item.clone(), cmds);
                }
            });
    }

    /// A file's path from the user's folder it's in, that folder's name first.
    fn path_in_mine(&self, file: &Path) -> String {
        let inside = |u: &UserFolder| Some(Path::new(&u.name).join(file.strip_prefix(&u.path).ok()?));
        let path = self.folders.iter().find_map(inside).unwrap_or_else(|| file.to_owned());
        path.display().to_string()
    }

    /// The chosen file's whole path (a click copies it), or what went wrong.
    fn foot(&self, ui: &mut egui::Ui, game: &Game, cmds: &mut Vec<Cmd>) {
        ui.add_space(4.0);
        if let Some(problem) = &self.problem {
            ui.add(egui::Label::new(RichText::new(problem).small().color(ui.visuals().warn_fg_color)).truncate());
        } else if let Some(item) = &self.selected {
            let path = match item {
                Item::Game(f) => game.path(*f),
                Item::Disk(p) => short_path(p),
            };
            let label = egui::Label::new(RichText::new(&path).small().color(theme::WEAK)).truncate();
            let resp = ui.add(label.sense(Sense::click())).on_hover_text(format!("{path}\n\nClick to copy"));
            if resp.clicked() {
                cmds.push(Cmd::Copy(item.path(game)));
            }
        }
    }

    /// A listed file as the viewers and the exports take it: one on disk is
    /// mounted beside the game's.
    fn file(&mut self, game: &Game, item: &Item) -> Option<FileRef> {
        match item {
            Item::Game(f) => Some(*f),
            Item::Disk(p) => match game.fs.mount(p) {
                Ok(f) => Some(f),
                Err(e) => {
                    log::warn!("{}: {e}", p.display());
                    self.problem = Some(format!("{}: {e}", p.display()));
                    None
                }
            },
        }
    }

    fn apply(
        &mut self,
        cmds: Vec<Cmd>,
        ctx: &egui::Context,
        game: &Game,
        settings: &mut Settings,
    ) -> Option<BrowserAction> {
        let mut action = None;
        for cmd in cmds {
            match cmd {
                Cmd::Toggle(id) => {
                    if !self.open.remove(&id) {
                        self.open.insert(id);
                    }
                }
                Cmd::Fold(s) => self.folded[s] ^= true,
                Cmd::Select(item) => {
                    self.selected = Some(item);
                    self.problem = None;
                }
                Cmd::Open(item, how) => {
                    self.problem = None;
                    action = self.file(game, &item).map(|f| match how {
                        As::Itself => BrowserAction::Open(f),
                        As::Xml => BrowserAction::OpenXml(f),
                        As::Hex => BrowserAction::OpenHex(f),
                    });
                }
                Cmd::Export(files, how) => {
                    self.problem = None;
                    let files: Vec<_> =
                        files.into_iter().filter_map(|(item, to)| Some((self.file(game, &item)?, to))).collect();
                    action = Some(BrowserAction::Export(files, how));
                }
                Cmd::Copy(text) => ctx.copy_text(text),
                Cmd::Reveal(folder) => reveal(&folder),
                Cmd::Add => {
                    let mut dialog = rfd::FileDialog::new().set_title("Add a folder to Game Files");
                    if let Some(last) = self.folders.last().and_then(|u| u.path.parent()).filter(|p| p.is_dir()) {
                        dialog = dialog.set_directory(last);
                    }
                    if let Some(folder) = dialog.pick_folder() {
                        self.add(folder, settings);
                    }
                }
                Cmd::Refresh(i) => {
                    if let Some(u) = self.folders.get_mut(i) {
                        u.stale = true;
                    }
                }
                Cmd::Remove(i) => {
                    if i < self.folders.len() {
                        self.folders.remove(i);
                        self.remember(settings);
                        self.reindex();
                    }
                }
            }
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_texts() {
        assert!(Kind::Models.has("levels/Val_01.YDR"));
        assert!(!Kind::Models.has("val_01.ytd"));
        assert!(Kind::All.has("readme"));
        assert_eq!(count_text(7), "7");
        assert_eq!(count_text(1234), "1,234");
        assert_eq!(count_text(495_787), "495,787");
        assert_eq!(short_location("levels_0.rpf/x64/levels/val_01"), "levels_0.rpf › … › val_01");
        assert_eq!(short_location("stream/props"), "stream › props");
        assert_eq!(short_location(""), "");
    }

    #[test]
    fn reads_a_folder() {
        let dir = std::env::temp_dir().join(format!("redforge-browser-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("stream/Props")).unwrap();
        std::fs::create_dir_all(dir.join("empty")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        for f in ["fxmanifest.lua", "stream/b.ytd", "stream/A.ydr", "stream/Props/c.ydr", ".git/config"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let mut left = MAX_SCANNED;
        let tree = scan(&dir, &mut left);
        // Folders with no files, and hidden ones, aren't listed.
        assert_eq!(tree.count, 4);
        assert_eq!(tree.folders.len(), 1);
        let stream = &tree.folders[0].1;
        assert_eq!(stream.files.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["A.ydr", "b.ytd"]);
        let mut files = Vec::new();
        index(&tree, "mine", &mut files);
        assert!(files.iter().any(|(p, _)| p == "mine/stream/props/c.ydr"));
        // No more than asked for.
        let mut left = 2;
        assert_eq!(scan(&dir, &mut left).count, 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
