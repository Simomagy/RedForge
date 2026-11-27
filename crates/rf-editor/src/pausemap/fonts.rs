//! The fonts labels are lettered in: two bundled open ones (Zilla Slab), the
//! game's own (from its Scaleform font library, see [`super::gfxfont`]) and
//! any TrueType or OpenType file the user adds. A font is its glyphs'
//! outlines and advances, in ems, y downwards.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::path::Point;

pub const BUNDLED: [&str; 2] = ["Zilla Slab Bold", "Zilla Slab Italic"];
pub const DEFAULT_FONT: &str = BUNDLED[0];

static ZILLA_BOLD: &[u8] = include_bytes!("../../assets/fonts/ZillaSlab-Bold.ttf");
static ZILLA_ITALIC: &[u8] = include_bytes!("../../assets/fonts/ZillaSlab-SemiBoldItalic.ttf");

/// A glyph: closed contours (flattened) and the advance, in ems.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Glyph {
    pub contours: Vec<Vec<Point>>,
    pub advance: f64,
    /// Filled even-odd (the game's fonts), not by winding (TrueType's).
    pub even_odd: bool,
}

enum Source {
    /// A TrueType or OpenType file.
    File(Arc<Vec<u8>>),
    /// Glyphs decoded already.
    Glyphs(HashMap<char, Arc<Glyph>>),
}

pub struct Font {
    pub name: String,
    /// Where it comes from: "bundled", "game" or the file's path.
    pub origin: String,
    source: Source,
    cache: Mutex<HashMap<char, Option<Arc<Glyph>>>>,
}

impl Font {
    /// A font file; `Err` when it isn't one.
    pub fn from_file(name: &str, origin: &str, data: Vec<u8>) -> Result<Font, String> {
        let face = ttf_parser::Face::parse(&data, 0).map_err(|e| format!("not a font: {e}"))?;
        if face.units_per_em() == 0 {
            return Err("not a font: no units per em".into());
        }
        Ok(Font {
            name: name.into(),
            origin: origin.into(),
            source: Source::File(Arc::new(data)),
            cache: Mutex::default(),
        })
    }

    pub fn from_glyphs(name: &str, origin: &str, glyphs: HashMap<char, Glyph>) -> Font {
        let glyphs = glyphs.into_iter().map(|(c, g)| (c, Arc::new(g))).collect();
        Font { name: name.into(), origin: origin.into(), source: Source::Glyphs(glyphs), cache: Mutex::default() }
    }

    /// A character's glyph; `None` when the font hasn't got it.
    pub fn glyph(&self, ch: char) -> Option<Arc<Glyph>> {
        match &self.source {
            Source::Glyphs(g) => g.get(&ch).cloned(),
            Source::File(data) => {
                let mut cache = self.cache.lock().unwrap();
                cache.entry(ch).or_insert_with(|| ttf_glyph(data, ch).map(Arc::new)).clone()
            }
        }
    }

    pub fn has(&self, ch: char) -> bool {
        self.glyph(ch).is_some()
    }
}

/// Curves are cut into this many chords.
const CURVE_STEPS: usize = 12;

struct Outline {
    scale: f64,
    contours: Vec<Vec<Point>>,
    at: Point,
}

impl Outline {
    fn p(&self, x: f32, y: f32) -> Point {
        [x as f64 * self.scale, -(y as f64) * self.scale]
    }
    fn push(&mut self, p: Point) {
        self.at = p;
        self.contours.last_mut().unwrap().push(p);
    }
}

impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.contours.push(Vec::new());
        let p = self.p(x, y);
        self.push(p);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.p(x, y);
        self.push(p);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (a, c, b) = (self.at, self.p(x1, y1), self.p(x, y));
        for s in 1..=CURVE_STEPS {
            let t = s as f64 / CURVE_STEPS as f64;
            let u = 1.0 - t;
            self.push([
                u * u * a[0] + 2.0 * u * t * c[0] + t * t * b[0],
                u * u * a[1] + 2.0 * u * t * c[1] + t * t * b[1],
            ]);
        }
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let seg = [self.at, self.p(x1, y1), self.p(x2, y2), self.p(x, y)];
        for s in 1..=CURVE_STEPS {
            self.push(super::path::bezier_at(&seg, s as f64 / CURVE_STEPS as f64));
        }
    }
    fn close(&mut self) {}
}

fn ttf_glyph(data: &[u8], ch: char) -> Option<Glyph> {
    let face = ttf_parser::Face::parse(data, 0).ok()?;
    let id = face.glyph_index(ch)?;
    let scale = 1.0 / face.units_per_em() as f64;
    let advance = face.glyph_hor_advance(id).unwrap_or(0) as f64 * scale;
    let mut o = Outline { scale, contours: Vec::new(), at: [0.0; 2] };
    face.outline_glyph(id, &mut o);
    Some(Glyph { contours: o.contours, advance, even_odd: false })
}

/// The fonts there are, by name.
pub struct Fonts {
    list: Vec<Arc<Font>>,
}

impl Default for Fonts {
    fn default() -> Self {
        let bundled = [(BUNDLED[0], ZILLA_BOLD), (BUNDLED[1], ZILLA_ITALIC)];
        let list = bundled
            .into_iter()
            .map(|(name, data)| Arc::new(Font::from_file(name, "bundled", data.to_vec()).expect("a bundled font")))
            .collect();
        Fonts { list }
    }
}

impl Fonts {
    pub fn names(&self) -> Vec<String> {
        self.list.iter().map(|f| f.name.clone()).collect()
    }
    pub fn all(&self) -> &[Arc<Font>] {
        &self.list
    }
    pub fn get(&self, name: &str) -> Option<&Arc<Font>> {
        self.list.iter().find(|f| f.name == name)
    }
    /// The font a label asks for, or the default one when it's not there.
    pub fn or_default(&self, name: &str) -> &Arc<Font> {
        self.get(name).unwrap_or(&self.list[0])
    }
    /// Adds a font, in place of one of the same name.
    pub fn add(&mut self, font: Font) {
        self.list.retain(|f| f.name != font.name);
        self.list.push(Arc::new(font));
    }
    pub fn remove(&mut self, name: &str) {
        self.list.retain(|f| f.name != name || f.origin == "bundled");
    }

    /// Adds the game's own fonts from its font library (a `.gfx`), after the
    /// bundled ones. Returns how many.
    pub fn add_game_fonts(&mut self, library: &[u8]) -> Result<usize, String> {
        let fonts = super::gfxfont::read_library(library)?;
        let n = fonts.len();
        for f in fonts {
            self.add(Font::from_glyphs(&f.name, "game", f.glyphs));
        }
        Ok(n)
    }
}

/// A font file's name for labels: the file's name without its extension (as
/// the web app names the fonts added to it).
pub fn file_font_name(path: &std::path::Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().trim().to_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_glyphs() {
        let fonts = Fonts::default();
        assert_eq!(fonts.names(), BUNDLED.to_vec());
        let a = fonts.get(DEFAULT_FONT).unwrap().glyph('A').unwrap();
        assert!(a.advance > 0.4 && a.advance < 1.0, "{}", a.advance);
        assert!(!a.contours.is_empty());
        // y downwards: the glyph sits above the baseline, at negative y.
        let top = a.contours.iter().flatten().map(|p| p[1]).fold(f64::MAX, f64::min);
        assert!(top < -0.5, "{top}");
        assert!(fonts.or_default("nope").name == DEFAULT_FONT);
        assert!(fonts.get(DEFAULT_FONT).unwrap().glyph('\u{4e00}').is_none());
    }
}
