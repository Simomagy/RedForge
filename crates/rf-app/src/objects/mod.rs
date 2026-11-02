//! The Objects pane (the C#'s `ObjectsPane`): every archetype of the game as a
//! tile with a 3D thumbnail, searched by name or ytyp and filtered by type.
//! Choosing one makes it the archetype the Place tool places.
//!
//! The list (100k+ archetypes) is made once on a worker thread, with the
//! names in lowercase for searching; the grid only lays out the rows on
//! screen.

mod thumbnails;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{self, Align2, Color32, FontId, RichText, Sense, Stroke, TextStyle, TextWrapMode, Vec2, WidgetText};
use rf_editor::Editor;
use rf_formats::rpf8::FileRef;
use rf_game::{Archetype, AssetType};

use crate::game::Game;
use crate::map::tools::{Tool, Tools};
use crate::viewers::{Gpu, Job, status_ui};
use thumbnails::{Thumb, Thumbnails};

/// The tile size setting, in points (the C#'s name and range).
pub const TILE_SIZE: &str = "Objects.TileSize";
const TILE_SIZES: std::ops::RangeInclusive<f32> = 64.0..=256.0;
pub const DEFAULT_TILE: f32 = 128.0;
/// Between tiles, points.
const GAP: f32 = 4.0;
const CORNER: f32 = 4.0;

/// What an archetype is, as the filter list sorts them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Drawable,
    Dictionary,
    Fragment,
    Interior,
    /// No model: assetless and the like.
    Other,
}

const FILTERS: [(Option<Kind>, &str); 5] = [
    (None, "All"),
    (Some(Kind::Drawable), "Props (ydr)"),
    (Some(Kind::Dictionary), "Dictionaries (ydd)"),
    (Some(Kind::Fragment), "Fragments (yft)"),
    (Some(Kind::Interior), "Interiors (MLO)"),
];

impl Kind {
    fn of(a: &Archetype) -> Self {
        if a.class == Archetype::MLO_CLASS {
            return Kind::Interior;
        }
        match a.asset_type {
            AssetType::DRAWABLE => Kind::Drawable,
            AssetType::DRAWABLE_DICTIONARY => Kind::Dictionary,
            AssetType::FRAGMENT => Kind::Fragment,
            _ => Kind::Other,
        }
    }

    fn has_model(self) -> bool {
        matches!(self, Kind::Drawable | Kind::Dictionary | Kind::Fragment)
    }

    fn description(self) -> &'static str {
        match self {
            Kind::Drawable => "Prop (ydr)",
            Kind::Dictionary => "In a drawable dictionary (ydd)",
            Kind::Fragment => "Fragment (yft)",
            Kind::Interior => "Interior (MLO)",
            Kind::Other => "No model",
        }
    }

    /// Shown while there's no picture.
    fn glyph(self) -> &'static str {
        match self {
            Kind::Interior => "🏠",
            Kind::Other => "∅",
            _ => "📦",
        }
    }
}

struct Row {
    hash: u32,
    name: String,
    /// `name` in lowercase, for searching.
    lower: String,
    /// Into [`List::ytyps`].
    ytyp: u32,
    kind: Kind,
}

/// Every archetype, by name (the unnamed ones, `0x…`, last).
struct List {
    rows: Vec<Row>,
    /// The ytyps' names without the extension: as they are, and in lowercase.
    ytyps: Vec<(String, String)>,
}

impl List {
    fn build(game: &Game) -> List {
        let started = std::time::Instant::now();
        let mut ytyp_index: HashMap<FileRef, u32> = HashMap::new();
        let mut ytyps = Vec::new();
        let data = game.data();
        let mut rows = Vec::with_capacity(data.archetypes.len());
        for a in data.archetypes.values() {
            let ytyp = *ytyp_index.entry(a.ytyp).or_insert_with(|| {
                let stem = file_stem(&game.path(a.ytyp)).to_owned();
                ytyps.push((stem.clone(), stem.to_lowercase()));
                ytyps.len() as u32 - 1
            });
            let name = game.hashes.get(a.name).unwrap_or_else(|| format!("0x{:08X}", a.name));
            rows.push(Row { hash: a.name, lower: name.to_lowercase(), name, ytyp, kind: Kind::of(a) });
        }
        rows.sort_unstable_by(|a, b| {
            (a.name.starts_with("0x"), &a.lower, a.hash).cmp(&(b.name.starts_with("0x"), &b.lower, b.hash))
        });
        log::info!("objects: {} archetypes listed in {:.2?}", rows.len(), started.elapsed());
        List { rows, ytyps }
    }

    fn ytyp(&self, row: &Row) -> &str {
        &self.ytyps[row.ytyp as usize].0
    }

    /// The rows of a kind with every word of `search` in their name or their
    /// ytyp's, ignoring case.
    fn filter(&self, search: &str, kind: Option<Kind>) -> Vec<u32> {
        let words: Vec<String> = search.split_whitespace().map(str::to_lowercase).collect();
        (0..self.rows.len() as u32)
            .filter(|&i| {
                let r = &self.rows[i as usize];
                kind.is_none_or(|k| r.kind == k)
                    && words.iter().all(|w| r.lower.contains(w) || self.ytyps[r.ytyp as usize].1.contains(w))
            })
            .collect()
    }
}

/// A file name without its folder and extension.
fn file_stem(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.rsplit_once('.').map_or(file, |(stem, _)| stem)
}

pub struct Objects {
    list: Option<Job<Arc<List>>>,
    /// The game data's version the list was made from.
    list_version: Option<u64>,
    search: String,
    filter: Option<Kind>,
    /// The rows passing the search and the filter, and which those were.
    shown: Vec<u32>,
    shown_for: Option<(String, Option<Kind>)>,
    /// Points.
    pub tile_size: f32,
    /// Set when the tile size was changed: the app saves it.
    pub tile_size_changed: bool,
    /// Whether the tile size is being dragged: the pictures are made again at
    /// the new size once it's let go.
    resizing: bool,
    thumbnails: Option<Thumbnails>,
    /// For testing: chooses the first object shown, as a click would.
    pub choose_first: bool,
}

impl Objects {
    pub fn new(tile_size: f32) -> Self {
        Self {
            list: None,
            list_version: None,
            search: String::new(),
            filter: None,
            shown: Vec::new(),
            shown_for: None,
            tile_size: tile_size.clamp(*TILE_SIZES.start(), *TILE_SIZES.end()),
            tile_size_changed: false,
            resizing: false,
            thumbnails: None,
            choose_first: false,
        }
    }

    pub fn set_search(&mut self, search: &str) {
        self.search = search.to_owned();
    }

    /// Frees the thumbnails (the pane was closed); they're made again when
    /// it's shown.
    pub fn free(&mut self, gpu: &Gpu) {
        if let Some(mut t) = self.thumbnails.take() {
            t.free(gpu);
        }
    }

    /// Forgets the game's objects, for another game.
    pub fn reset(&mut self, gpu: &Gpu) {
        self.free(gpu);
        self.list = None;
        self.shown.clear();
        self.shown_for = None;
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        gpu: &Gpu,
        game: &Arc<Game>,
        editor: Option<&mut Editor>,
        tools: &mut Tools,
    ) {
        // The list again when the game data changes (a project's new archetypes).
        if self.list_version != Some(game.data_version()) {
            self.list_version = Some(game.data_version());
            self.list = None;
            self.shown_for = None;
        }
        let job = self.list.get_or_insert_with(|| {
            let g = game.clone();
            Job::spawn(ui.ctx(), move || Ok(Arc::new(List::build(&g))))
        });
        job.poll();
        let list = match job {
            Job::Done(list) => list.clone(),
            _ => {
                status_ui(ui, job, "the objects");
                return;
            }
        };

        let mut changed = false;
        egui::Panel::top("objects-header").show(ui, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let filter_name = FILTERS.iter().find(|(k, _)| *k == self.filter).map_or("All", |(_, n)| n);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::ComboBox::from_id_salt("objects-filter").selected_text(filter_name).show_ui(ui, |ui| {
                        for (kind, name) in FILTERS {
                            ui.selectable_value(&mut self.filter, kind, name);
                        }
                    });
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("Search objects")
                            .desired_width(ui.available_width()),
                    );
                });
            });
            let key = (self.search.clone(), self.filter);
            if self.shown_for.as_ref() != Some(&key) {
                self.shown = list.filter(&self.search, self.filter);
                self.shown_for = Some(key);
                changed = true;
            }
            ui.horizontal(|ui| {
                let total = list.rows.len();
                let count = if self.shown.len() == total {
                    format!("{} objects", thousands(total))
                } else {
                    format!("{} of {}", thousands(self.shown.len()), thousands(total))
                };
                ui.label(RichText::new(count).weak());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.resizing = false;
                    ui.menu_button("⚙", |ui| {
                        ui.label("Preview size");
                        let r = ui.add(egui::Slider::new(&mut self.tile_size, TILE_SIZES).step_by(16.0));
                        self.resizing = r.dragged();
                        if r.drag_stopped() || (r.changed() && !r.dragged()) {
                            self.tile_size_changed = true;
                        }
                    })
                    .response
                    .on_hover_text("Preview size");
                });
            });
        });

        if self.choose_first
            && let Some(&i) = self.shown.first()
        {
            self.choose_first = false;
            choose(&list.rows[i as usize], tools);
        }

        egui::Panel::bottom("objects-details").show(ui, |ui| {
            ui.add_space(4.0);
            details_ui(ui, game, tools.place_archetype);
            if let Some(ed) = editor {
                place_in_ui(ui, ed);
            }
            ui.add_space(2.0);
        });

        let px = picture_size(self.tile_size, ui.ctx().pixels_per_point());
        let thumbs = self.thumbnails.get_or_insert_with(|| Thumbnails::new(game.clone(), px));
        if !self.resizing {
            thumbs.set_size(gpu, px);
        }
        let mut chosen = None;
        egui::CentralPanel::default().frame(egui::Frame::NONE.inner_margin(GAP)).show(ui, |ui| {
            if self.shown.is_empty() {
                ui.centered_and_justified(|ui| ui.label(RichText::new("No objects match.").weak()));
                return;
            }
            let tile = self.tile_size;
            let label_height = ui.text_style_height(&TextStyle::Small) + 4.0;
            ui.spacing_mut().item_spacing = Vec2::splat(GAP);
            let columns = (((ui.available_width() + GAP) / (tile + GAP)).floor() as usize).max(1);
            let rows = self.shown.len().div_ceil(columns);
            let background = thumbs.background();
            let mut area = egui::ScrollArea::vertical().auto_shrink([false; 2]);
            if changed {
                area = area.vertical_scroll_offset(0.0);
            }
            area.show_rows(ui, tile + label_height, rows, |ui, range| {
                for r in range {
                    ui.horizontal(|ui| {
                        for &i in self.shown.iter().skip(r * columns).take(columns) {
                            let row = &list.rows[i as usize];
                            let tile = Tile { size: tile, label_height, background };
                            let selected = tools.place_archetype == Some(row.hash);
                            if tile.ui(ui, row, &list, thumbs, selected).clicked() {
                                chosen = Some(i);
                            }
                        }
                    });
                }
            });
        });
        if let Some(i) = chosen {
            choose(&list.rows[i as usize], tools);
        }
        thumbs.update(gpu);
        if thumbs.busy() {
            ui.ctx().request_repaint_after(Duration::from_millis(30));
        }
    }
}

/// Makes an object the one to place, and picks the Place tool (not for
/// interiors, which can't be placed yet).
fn choose(row: &Row, tools: &mut Tools) {
    tools.place_archetype = Some(row.hash);
    if row.kind == Kind::Interior {
        tools.say("Interiors (MLO) can't be placed yet");
        if tools.tool == Tool::Place {
            tools.tool = Tool::Select;
        }
    } else {
        tools.tool = Tool::Place;
    }
}

/// How a tile looks.
struct Tile {
    /// The picture's side, points; the name goes under it.
    size: f32,
    label_height: f32,
    background: Color32,
}

impl Tile {
    fn ui(&self, ui: &mut egui::Ui, row: &Row, list: &List, thumbs: &mut Thumbnails, selected: bool) -> egui::Response {
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(self.size, self.size + self.label_height), Sense::click());
        let image = egui::Rect::from_min_size(rect.min, Vec2::splat(self.size));
        let thumb = if row.kind.has_model() { thumbs.get(row.hash) } else { Thumb::None };
        let painter = ui.painter();
        painter.rect_filled(image, CORNER, self.background);
        match thumb {
            Thumb::Ready(id) => egui::Image::new((id, image.size())).corner_radius(CORNER).paint_at(ui, image),
            Thumb::Loading | Thumb::None => {
                let font = FontId::proportional(self.size * 0.25);
                painter.text(image.center(), Align2::CENTER_CENTER, row.kind.glyph(), font, Color32::from_gray(130));
                if matches!(thumb, Thumb::Loading) {
                    let side = (self.size * 0.12).max(10.0);
                    let spot =
                        egui::Rect::from_min_size(image.right_bottom() - Vec2::splat(side + 6.0), Vec2::splat(side));
                    egui::Spinner::new().size(side).color(Color32::from_gray(150)).paint_at(ui, spot);
                }
            }
        }
        let stroke = if selected {
            Stroke::new(2.0, ui.visuals().selection.bg_fill)
        } else if response.hovered() {
            ui.visuals().widgets.hovered.bg_stroke
        } else {
            Stroke::NONE
        };
        painter.rect_stroke(image, CORNER, stroke, egui::StrokeKind::Inside);
        let name = WidgetText::from(row.name.as_str());
        let galley = name.into_galley(ui, Some(TextWrapMode::Truncate), self.size, TextStyle::Small);
        let colour = if selected { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
        let at = egui::pos2(rect.center().x - galley.size().x * 0.5, image.bottom() + 2.0);
        painter.galley(at, galley, colour);
        response.on_hover_text(format!("{}\n{}.ytyp", row.name, list.ytyp(row)))
    }
}

/// The chosen object's name, type, ytyp, size and textures.
fn details_ui(ui: &mut egui::Ui, game: &Game, chosen: Option<u32>) {
    let data = game.data();
    let Some(a) = chosen.and_then(|h| data.archetypes.get(&h)) else {
        ui.label(RichText::new("Choose an object, then click in the map to place it.").weak());
        return;
    };
    ui.add(egui::Label::new(RichText::new(game.hashes.name(a.name)).strong()).truncate());
    let ytyp = game.path(a.ytyp);
    ui.label(format!("{}, in {}.ytyp", Kind::of(a).description(), file_stem(&ytyp)));
    // Interiors have no box in the game data (see rf-game).
    let size: [f32; 3] = std::array::from_fn(|i| a.bb_max[i] - a.bb_min[i]);
    let lod = format!("LOD distance {:.0} m", a.lod_dist);
    if size == [0.0; 3] {
        ui.label(lod);
    } else {
        ui.label(format!("Size {} × {} × {} m, {lod}", short(size[0]), short(size[1]), short(size[2])));
    }
    if a.texture_dictionary != 0 {
        ui.label(format!("Textures: {}", game.hashes.name(a.texture_dictionary)));
    }
}

/// Where placed objects go: one of the edited ymaps, as in the Project pane.
fn place_in_ui(ui: &mut egui::Ui, editor: &mut Editor) {
    let maps: Vec<(u32, String)> = editor.edited_maps().iter().map(|m| (m.name, m.file_name.clone())).collect();
    ui.horizontal(|ui| {
        ui.label("Place in");
        if maps.is_empty() {
            ui.label(RichText::new("no ymap yet: make one in Project").weak());
            return;
        }
        let current = editor.target.and_then(|t| maps.iter().find(|(n, _)| *n == t)).map_or("Choose a ymap", |m| &m.1);
        egui::ComboBox::from_id_salt("objects-place-in").selected_text(current).show_ui(ui, |ui| {
            for (name, file) in &maps {
                ui.selectable_value(&mut editor.target, Some(*name), file);
            }
        });
    });
}

/// The pictures' size in pixels: twice the tile's (egui's linear filter then
/// smooths the edges), in steps of 32, up to 256.
fn picture_size(tile: f32, pixels_per_point: f32) -> u32 {
    let px = (tile * pixels_per_point * 2.0).min(256.0);
    ((px / 32.0).ceil() as u32 * 32).max(64)
}

/// `1234567` as `1,234,567`.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A size with up to two decimals, without trailing zeros.
fn short(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(123456789), "123,456,789");
        assert_eq!(short(1.5), "1.5");
        assert_eq!(short(2.0), "2");
        assert_eq!(short(0.126), "0.13");
        assert_eq!(file_stem("a/b/c_props.ytyp"), "c_props");
        assert_eq!(picture_size(128.0, 1.0), 256);
        assert_eq!(picture_size(64.0, 1.0), 128);
        assert_eq!(picture_size(72.0, 1.0), 160);
    }
}
