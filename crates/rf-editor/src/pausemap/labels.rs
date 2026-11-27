//! Lettering as a signed distance field, encoded like the game's text
//! families.
//!
//! The glyph outline sits exactly on the game's ink threshold, so every size
//! shows the font's real shape. Around it the field follows the game's own
//! profile, measured over all of its text tiles: ~10 per pixel at the
//! threshold, steepening to ~19 far out.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::edt::edt;
use super::families::Fit;
use super::fonts::{Font, Fonts, Glyph};
use super::path::Point;
use super::project::LabelObject;

/// The game draws text ink below this.
pub const TEXT_EDGE: f64 = 68.0;
const IN: f64 = 10.0;
const FLOOR: f64 = 40.0;
/// Outside: `EDGE + A (e^(B d) - 1)`, a slope of 10 rising to 19.
const OUT_A: f64 = 169.0;
const OUT_B: f64 = 0.058;
const MAX_CANVAS: f64 = 4096.0;

/// Map units from the ink to where the field reaches 255 (~12.9).
fn halo() -> f64 {
    (1.0 + (255.0 - TEXT_EDGE) / OUT_A).ln() / OUT_B
}

pub fn encode_text(sd: f64) -> u8 {
    let v = if sd >= 0.0 { TEXT_EDGE + OUT_A * ((OUT_B * sd).exp() - 1.0) } else { TEXT_EDGE + IN * sd };
    v.clamp(FLOOR, 255.0).round() as u8
}

/// A field of a mark in a family's atlas.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub data: Vec<u8>,
    pub w: usize,
    pub h: usize,
    /// The atlas pixel of `data[0]`.
    pub x: i64,
    pub y: i64,
}

/// An ink mask (1 = ink) at `ss` samples per atlas pixel made a field at
/// atlas resolution. Distances are in map units (`unit` atlas pixels each),
/// like the game's LOD tiles, which are their detail fields scaled down: a
/// thin stroke keeps its dark core instead of fading out zoomed out. `grow`
/// is in map units too.
pub fn mask_to_field(
    ink: &[u8],
    w: usize,
    h: usize,
    ss: usize,
    grow: f64,
    unit: f64,
    encode: fn(f64) -> u8,
) -> (Vec<u8>, usize, usize) {
    let bg: Vec<u8> = ink.iter().map(|&i| (i == 0) as u8).collect();
    let (d_out, d_in) = (edt(ink, w, h), edt(&bg, w, h));
    let (fw, fh) = (w / ss, h / ss);
    let mut data = vec![0u8; fw * fh];
    // The distance at the pixel's centre (its central samples), as a distance
    // field is meant to be read: averaging over the pixel would wash out
    // strokes thinner than one. The game's LOD tiles match their detail
    // fields at the pixel centres too.
    let c0 = ss >> 1;
    let c1 = if ss & 1 == 1 { c0 } else { c0 - 1 };
    for fy in 0..fh {
        for fx in 0..fw {
            let mut sum = 0.0;
            for y in [c0, c1] {
                for x in [c0, c1] {
                    let i = (fy * ss + y) * w + fx * ss + x;
                    sum += (d_out[i] as f64).sqrt() - (d_in[i] as f64).sqrt();
                }
            }
            data[fy * fw + fx] = encode(sum / 4.0 / ss as f64 / unit - grow);
        }
    }
    (data, fw, fh)
}

/// Fills closed polygons (by nonzero winding, or even-odd) into a mask,
/// sampling at the samples' centres: what a canvas fill gives once its alpha
/// is cut at half.
pub fn fill_polygons(mask: &mut [u8], w: usize, h: usize, polys: &[Vec<Point>], even_odd: bool) {
    let mut crossings: Vec<(f64, i32)> = Vec::new();
    let (mut y0, mut y1) = (f64::MAX, f64::MIN);
    for p in polys.iter().flatten() {
        y0 = y0.min(p[1]);
        y1 = y1.max(p[1]);
    }
    if y0 > y1 {
        return;
    }
    let first = (y0 - 0.5).floor().max(0.0) as usize;
    let last = ((y1 - 0.5).ceil().max(0.0) as usize).min(h.saturating_sub(1));
    for row in first..=last {
        let sy = row as f64 + 0.5;
        crossings.clear();
        for poly in polys {
            let n = poly.len();
            for i in 0..n {
                let (a, b) = (poly[i], poly[(i + 1) % n]);
                if (a[1] <= sy) != (b[1] <= sy) {
                    let x = a[0] + (sy - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
                    crossings.push((x, if b[1] > a[1] { 1 } else { -1 }));
                }
            }
        }
        crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut winding = 0;
        for k in 0..crossings.len() {
            winding += if even_odd { 1 } else { crossings[k].1 };
            let inside = if even_odd { winding % 2 == 1 } else { winding != 0 };
            if inside && k + 1 < crossings.len() {
                let xa = (crossings[k].0 - 0.5).ceil().max(0.0) as usize;
                let xb = ((crossings[k + 1].0 - 0.5).ceil().max(0.0) as usize).min(w);
                for x in xa..xb {
                    mask[row * w + x] = 1;
                }
            }
        }
    }
}

/// A glyph placed in label space (map units, the string's middle at 0,0).
#[derive(Clone, Debug)]
struct Placed {
    glyph: Arc<Glyph>,
    u: f64,
    v: f64,
    angle: f64,
    advance: f64,
}

/// A character's glyph in the label's font, or in the default one.
fn glyph_of(font: &Font, fonts: &Fonts, ch: char) -> Arc<Glyph> {
    font.glyph(ch)
        .or_else(|| fonts.all()[0].glyph(ch))
        .unwrap_or_else(|| Arc::new(Glyph { contours: Vec::new(), advance: 0.3, even_odd: false }))
}

fn layout(l: &LabelObject, fonts: &Fonts) -> Vec<Placed> {
    let font = fonts.or_default(&l.font);
    let text = if l.caps { l.text.to_uppercase() } else { l.text.clone() };
    let glyphs: Vec<Arc<Glyph>> = text.chars().map(|c| glyph_of(font, fonts, c)).collect();
    let track = l.tracking * l.size;
    let adv: Vec<f64> = glyphs.iter().map(|g| g.advance * l.size).collect();
    let total = adv.iter().sum::<f64>() + track * glyphs.len().saturating_sub(1) as f64;
    let bend = if glyphs.len() > 1 { l.curve * l.size } else { 0.0 };
    let mut pen = -total / 2.0;
    glyphs
        .into_iter()
        .zip(adv)
        .map(|(glyph, advance)| {
            let u = pen + advance / 2.0;
            pen += advance + track;
            let t = if total > 0.0 { 2.0 * u / total } else { 0.0 };
            // A parabolic arc: the middle rises by `bend`, glyphs turn with
            // the slope.
            let angle = (4.0 * bend * t / total.max(1e-6)).atan();
            Placed { glyph, u, v: -bend * (1.0 - t * t), angle, advance }
        })
        .collect()
}

/// The glyphs' outlines in label space.
fn outlines(glyphs: &[Placed], size: f64) -> Vec<Vec<Point>> {
    let mut out = Vec::new();
    for g in glyphs {
        let (sin, cos) = g.angle.sin_cos();
        for c in &g.glyph.contours {
            out.push(
                c.iter()
                    .map(|p| {
                        let (x, y) = (p[0] * size - g.advance / 2.0, p[1] * size);
                        [g.u + x * cos - y * sin, g.v + x * sin + y * cos]
                    })
                    .collect(),
            );
        }
    }
    out
}

/// A label's glyphs, their outlines and their ink's box, wherever it is.
#[derive(Clone, Debug)]
struct Shape {
    outlines: Vec<Vec<Point>>,
    /// The font's fill rule.
    even_odd: bool,
    cx: f64,
    cy: f64,
    hw: f64,
    hh: f64,
}

fn shape_of(l: &LabelObject, fonts: &Fonts) -> Option<Shape> {
    let glyphs = layout(l, fonts);
    let even_odd = glyphs.iter().any(|g| g.glyph.even_odd);
    let outlines = outlines(&glyphs, l.size);
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in outlines.iter().flatten() {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    (x0 < x1).then(|| Shape {
        outlines,
        even_odd,
        cx: (x0 + x1) / 2.0,
        cy: (y0 + y1) / 2.0,
        hw: (x1 - x0) / 2.0,
        hh: (y1 - y0) / 2.0,
    })
}

/// Labels' shapes and fields, kept while they're the same: moving a label
/// is a whole-pixel translation of its field.
#[derive(Default)]
pub struct Lettering {
    shapes: Mutex<HashMap<String, Option<Arc<Shape>>>>,
    fields: Mutex<HashMap<String, Option<Arc<Field>>>>,
}

fn remember<T: Clone>(m: &Mutex<HashMap<String, T>>, key: String, make: impl FnOnce() -> T) -> T {
    if let Some(v) = m.lock().unwrap().get(&key) {
        return v.clone();
    }
    let v = make();
    let mut m = m.lock().unwrap();
    if m.len() > 200 {
        m.clear();
    }
    m.insert(key, v.clone());
    v
}

impl Lettering {
    /// Forgets everything: after fonts change.
    pub fn clear(&self) {
        self.shapes.lock().unwrap().clear();
        self.fields.lock().unwrap().clear();
    }

    fn shape(&self, l: &LabelObject, fonts: &Fonts) -> Option<Arc<Shape>> {
        let key = format!("{:?}", (&l.text, &l.font, l.size, l.tracking, l.curve, l.caps));
        remember(&self.shapes, key, || shape_of(l, fonts).map(Arc::new))
    }

    /// The corners of the rotated ink box around the anchor, in map units.
    pub fn corners(&self, l: &LabelObject, fonts: &Fonts) -> Vec<Point> {
        let Some(s) = self.shape(l, fonts) else { return Vec::new() };
        let (sin, cos) = l.rotation.to_radians().sin_cos();
        [[-s.hw, -s.hh], [s.hw, -s.hh], [s.hw, s.hh], [-s.hw, s.hh]]
            .into_iter()
            .map(|[u, v]| [u * cos - v * sin, u * sin + v * cos])
            .collect()
    }

    /// A label's field in a family's atlas.
    pub fn field(&self, l: &LabelObject, fit: &Fit, fonts: &Fonts) -> Option<Field> {
        let ax = ((l.x + fit.ox) * fit.sx).round() as i64;
        let ay = ((l.y + fit.oy) * fit.sy).round() as i64;
        let key = format!(
            "{:?}",
            (
                &l.text, &l.font, l.size, l.rotation, l.tracking, l.curve, l.weight, l.caps, fit.sx, fit.sy, fit.ox,
                fit.oy
            )
        );
        let f = remember(&self.fields, key, || self.render(l, fit, fonts).map(Arc::new))?;
        Some(Field { x: f.x + ax, y: f.y + ay, ..(*f).clone() })
    }

    fn render(&self, l: &LabelObject, fit: &Fit, fonts: &Fonts) -> Option<Field> {
        let shape = self.shape(l, fonts)?;
        let (sin, cos) = l.rotation.to_radians().sin_cos();
        let unit = fit.unit();
        let grow = l.weight;
        // The atlas box (the anchor at 0,0) of the rotated ink box and the
        // field's halo.
        let corners: Vec<Point> = self.corners(l, fonts).iter().map(|c| [c[0] * fit.sx, c[1] * fit.sy]).collect();
        let pad = ((grow + halo()) * unit + 2.0).ceil();
        let x0 = corners.iter().map(|c| c[0]).fold(f64::MAX, f64::min).floor() - pad;
        let x1 = corners.iter().map(|c| c[0]).fold(f64::MIN, f64::max).ceil() + pad;
        let y0 = corners.iter().map(|c| c[1]).fold(f64::MAX, f64::min).floor() - pad;
        let y1 = corners.iter().map(|c| c[1]).fold(f64::MIN, f64::max).ceil() + pad;
        // Small lettering gets more samples a pixel: its strokes are a pixel
        // or two wide.
        let ss = (MAX_CANVAS / (x1 - x0).max(y1 - y0)).floor().clamp(1.0, 8.0);
        let (w, h) = (((x1 - x0) * ss) as usize, ((y1 - y0) * ss) as usize);
        // Label space, about the ink's centre, rotated, to the atlas (both
        // ways for LOD families), to the samples.
        let polys: Vec<Vec<Point>> = shape
            .outlines
            .iter()
            .map(|c| {
                c.iter()
                    .map(|p| {
                        let (x, y) = (p[0] - shape.cx, p[1] - shape.cy);
                        let (rx, ry) = (x * cos - y * sin, x * sin + y * cos);
                        [(rx * fit.sx - x0) * ss, (ry * fit.sy - y0) * ss]
                    })
                    .collect()
            })
            .collect();
        let mut ink = vec![0u8; w * h];
        fill_polygons(&mut ink, w, h, &polys, shape.even_odd);
        let (data, fw, fh) = mask_to_field(&ink, w, h, ss as usize, grow, unit, encode_text);
        Some(Field { data, w: fw, h: fh, x: x0 as i64, y: y0 as i64 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A vertical bar `width` map units wide, at `ss` samples per atlas pixel
    /// and `unit` atlas pixels per map unit: the ink width the game draws.
    fn ink_width(width: f64, unit: f64, grow: f64) -> (f64, u8) {
        let (ss, px) = (8usize, 64usize);
        let (w, h) = (px * ss, 4 * ss);
        let mut ink = vec![0u8; w * h];
        let half = width * unit * ss as f64 / 2.0;
        let mid = (w / 2 + ss / 2) as f64;
        for y in 0..h {
            for x in 0..w {
                if (x as f64 + 0.5 - mid).abs() < half {
                    ink[y * w + x] = 1;
                }
            }
        }
        let (data, fw, _) = mask_to_field(&ink, w, h, ss, grow, unit, encode_text);
        let row = &data[fw..fw * 2];
        let covered: f64 = row.iter().map(|&v| ((TEXT_EDGE - v as f64) * unit / 10.0 + 0.5).clamp(0.0, 1.0)).sum();
        (covered / unit, *row.iter().min().unwrap())
    }

    #[test]
    fn a_stroke_is_as_wide_as_it_is() {
        for unit in [1.0, 0.56] {
            for width in [1.0, 1.5, 3.0, 6.0, 12.0] {
                let (w, core) = ink_width(width, unit, 0.0);
                assert!((core as f64) < TEXT_EDGE, "core {width}@{unit}");
                assert!((w - width).abs() < 0.6, "width {width}@{unit}: {w}");
            }
        }
    }

    #[test]
    fn the_weight_is_in_map_units() {
        assert!((ink_width(3.0, 1.0, 1.0).0 - 5.0).abs() < 0.5);
        assert!((ink_width(12.0, 1.0, 1.0).0 - 14.0).abs() < 0.5);
    }

    #[test]
    fn filling_squares() {
        let mut m = vec![0u8; 10 * 10];
        // A square with a hole the other way round (nonzero: a hole).
        let outer = vec![[1.0, 1.0], [9.0, 1.0], [9.0, 9.0], [1.0, 9.0]];
        let inner = vec![[3.0, 3.0], [3.0, 7.0], [7.0, 7.0], [7.0, 3.0]];
        fill_polygons(&mut m, 10, 10, &[outer.clone(), inner.clone()], false);
        assert_eq!(m[5 * 10 + 1], 1);
        assert_eq!(m[5 * 10 + 5], 0);
        assert_eq!(m[5 * 10 + 9], 0);
        assert_eq!(m.iter().filter(|&&v| v == 1).count(), 64 - 16);
        // Even-odd: a hole whichever way round.
        let mut e = vec![0u8; 10 * 10];
        let inner_cw: Vec<Point> = inner.iter().rev().copied().collect();
        fill_polygons(&mut e, 10, 10, &[outer, inner_cw], true);
        assert_eq!(e, m);
    }

    #[test]
    fn a_label_is_inked_round_its_anchor() {
        let fonts = Fonts::default();
        let lettering = Lettering::default();
        let l = LabelObject {
            common: Default::default(),
            text: "Valentine".into(),
            font: "Zilla Slab Bold".into(),
            size: 29.0,
            x: 1000.0,
            y: 500.0,
            rotation: 0.0,
            tracking: 0.14,
            curve: 0.0,
            weight: 0.9,
            caps: true,
        };
        let fit = Fit { sx: 1.0, sy: 1.0, ox: 0.0, oy: 0.0 };
        let f = lettering.field(&l, &fit, &fonts).unwrap();
        // The ink's box is centred on the anchor.
        let (mut x0, mut x1, mut y0, mut y1) = (i64::MAX, i64::MIN, i64::MAX, i64::MIN);
        for y in 0..f.h {
            for x in 0..f.w {
                if (f.data[y * f.w + x] as f64) < TEXT_EDGE {
                    x0 = x0.min(f.x + x as i64);
                    x1 = x1.max(f.x + x as i64);
                    y0 = y0.min(f.y + y as i64);
                    y1 = y1.max(f.y + y as i64);
                }
            }
        }
        assert!(((x0 + x1) as f64 / 2.0 - 1000.0).abs() < 2.0, "{x0}..{x1}");
        assert!(((y0 + y1) as f64 / 2.0 - 500.0).abs() < 2.0, "{y0}..{y1}");
        // Caps 29 units tall: about 20 units of ink, plus the weight.
        assert!((18..=26).contains(&(y1 - y0)), "{}", y1 - y0);
        assert_eq!(lettering.corners(&l, &fonts).len(), 4);
    }
}
