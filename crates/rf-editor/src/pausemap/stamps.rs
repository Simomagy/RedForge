//! Picture stamps: a picture becomes an ink mask, the mask a distance field
//! encoded like the layer it lands in, so that the game draws its edge the
//! way it draws its own marks.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::families::{Fit, Layer};
use super::labels::{Field, encode_text, mask_to_field};
use super::linework::encode_line;
use super::path::Point;
use super::project::StampObject;

const MAX_CANVAS: f64 = 4096.0;
/// Map units kept around the ink for the field's fade.
const HALO: f64 = 14.0;

/// A decoded picture, with its halvings for drawing it small.
pub struct Picture {
    /// The file as it was added (PNG, JPEG...), to save with the project.
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// RGBA, full size first, each next one half the last.
    levels: Vec<(u32, u32, Vec<u8>)>,
}

impl Picture {
    pub fn decode(bytes: Vec<u8>) -> Result<Picture, String> {
        let img = image::load_from_memory(&bytes).map_err(|e| format!("not a picture: {e}"))?.to_rgba8();
        let (width, height) = img.dimensions();
        if width == 0 || height == 0 {
            return Err("an empty picture".into());
        }
        let mut levels = vec![(width, height, img.into_raw())];
        while let Some((w, h, px)) = levels.last()
            && (*w > 1 || *h > 1)
        {
            let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
            let mut out = vec![0u8; (nw * nh * 4) as usize];
            for y in 0..nh {
                for x in 0..nw {
                    for c in 0..4 {
                        let mut sum = 0u32;
                        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            let (sx, sy) = ((x * 2 + dx).min(w - 1), (y * 2 + dy).min(h - 1));
                            sum += px[((sy * w + sx) * 4 + c) as usize] as u32;
                        }
                        out[((y * nw + x) * 4 + c) as usize] = ((sum + 2) / 4) as u8;
                    }
                }
            }
            levels.push((nw, nh, out));
        }
        Ok(Picture { bytes, width, height, levels })
    }

    pub fn aspect(&self) -> f64 {
        self.height as f64 / self.width as f64
    }

    /// Bilinear RGBA at `(u, v)` (0 to 1 across), from the level with about
    /// `texels` picture pixels per sample.
    fn sample(&self, u: f64, v: f64, texels: f64) -> [f64; 4] {
        let level = (texels.max(1.0).log2().floor() as usize).min(self.levels.len() - 1);
        let (w, h, px) = &self.levels[level];
        let (x, y) = (u * *w as f64 - 0.5, v * *h as f64 - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |xi: f64, yi: f64, c: usize| {
            let xi = xi.clamp(0.0, *w as f64 - 1.0) as u32;
            let yi = yi.clamp(0.0, *h as f64 - 1.0) as u32;
            px[((yi * w + xi) * 4) as usize + c] as f64
        };
        let mut out = [0.0; 4];
        for (c, o) in out.iter_mut().enumerate() {
            let top = at(x0, y0, c) * (1.0 - fx) + at(x0 + 1.0, y0, c) * fx;
            let bottom = at(x0, y0 + 1.0, c) * (1.0 - fx) + at(x0 + 1.0, y0 + 1.0, c) * fx;
            *o = top * (1.0 - fy) + bottom * fy;
        }
        out
    }
}

/// The corners of a stamp's rotated box around its centre, in map units.
pub fn stamp_corners(s: &StampObject) -> [Point; 4] {
    let (sin, cos) = s.rotation.to_radians().sin_cos();
    let (hw, hh) = (s.width / 2.0, s.width * s.aspect / 2.0);
    [[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]].map(|[u, v]| [u * cos - v * sin, u * sin + v * cos])
}

/// The project's pictures, by id, and the stamps' fields made from them.
#[derive(Default)]
pub struct Pictures {
    pub images: HashMap<String, Arc<Picture>>,
    fields: Mutex<HashMap<String, Option<Arc<Field>>>>,
}

impl Pictures {
    pub fn get(&self, id: &str) -> Option<&Arc<Picture>> {
        self.images.get(id)
    }

    /// Adds a picture under a new id.
    pub fn add(&mut self, bytes: Vec<u8>) -> Result<(String, f64), String> {
        let p = Picture::decode(bytes)?;
        let aspect = p.aspect();
        let id = loop {
            let id = format!("{}{}", super::project::new_id(), &super::project::new_id()[..4]);
            if !self.images.contains_key(&id) {
                break id;
            }
        };
        self.images.insert(id.clone(), Arc::new(p));
        Ok((id, aspect))
    }

    /// Puts a picture in under its id (when opening a project).
    pub fn insert(&mut self, id: &str, bytes: Vec<u8>) -> Result<(), String> {
        let p = Picture::decode(bytes)?;
        self.images.insert(id.to_owned(), Arc::new(p));
        self.fields.lock().unwrap().clear();
        Ok(())
    }

    /// A stamp's field in a family's atlas; `None` without its picture or
    /// without ink.
    pub fn field(&self, s: &StampObject, fit: &Fit) -> Option<Field> {
        let picture = self.images.get(&s.asset)?;
        let ax = ((s.x + fit.ox) * fit.sx).round() as i64;
        let ay = ((s.y + fit.oy) * fit.sy).round() as i64;
        let key = format!(
            "{:?}",
            (&s.asset, s.width, s.rotation, s.threshold, s.invert, s.target, fit.sx, fit.sy, fit.ox, fit.oy)
        );
        let cached = self.fields.lock().unwrap().get(&key).cloned();
        let f = match cached {
            Some(f) => f,
            None => {
                let f = render(s, picture, fit).map(Arc::new);
                let mut m = self.fields.lock().unwrap();
                if m.len() > 100 {
                    m.clear();
                }
                m.insert(key, f.clone());
                f
            }
        }?;
        Some(Field { x: f.x + ax, y: f.y + ay, ..(*f).clone() })
    }
}

fn render(s: &StampObject, picture: &Picture, fit: &Fit) -> Option<Field> {
    let unit = fit.unit();
    let corners = stamp_corners(s).map(|[x, y]| [x * fit.sx, y * fit.sy]);
    let pad = (HALO * unit + 2.0).ceil();
    let x0 = corners.iter().map(|c| c[0]).fold(f64::MAX, f64::min).floor() - pad;
    let x1 = corners.iter().map(|c| c[0]).fold(f64::MIN, f64::max).ceil() + pad;
    let y0 = corners.iter().map(|c| c[1]).fold(f64::MAX, f64::min).floor() - pad;
    let y1 = corners.iter().map(|c| c[1]).fold(f64::MIN, f64::max).ceil() + pad;
    let ss = (MAX_CANVAS / (x1 - x0).max(y1 - y0)).floor().clamp(1.0, 4.0);
    let (w, h) = (((x1 - x0) * ss) as usize, ((y1 - y0) * ss) as usize);
    let (sin, cos) = s.rotation.to_radians().sin_cos();
    let (hw, hh) = (s.width / 2.0, s.width * s.aspect / 2.0);
    // Picture pixels per sample.
    let texels = picture.width as f64 / (2.0 * hw * unit * ss);
    let cut = s.threshold * 255.0;
    let mut ink = vec![0u8; w * h];
    let mut any = false;
    for y in 0..h {
        for x in 0..w {
            // The sample back in the stamp's own space (map units, about its
            // centre, unrotated).
            let ax = ((x as f64 + 0.5) / ss + x0) / fit.sx;
            let ay = ((y as f64 + 0.5) / ss + y0) / fit.sy;
            let (u, v) = (ax * cos + ay * sin, -ax * sin + ay * cos);
            if u.abs() > hw || v.abs() > hh {
                continue;
            }
            let [r, g, b, a] = picture.sample((u + hw) / (2.0 * hw), (v + hh) / (2.0 * hh), texels);
            let lum = 0.299 * r + 0.587 * g + 0.114 * b;
            // Transparent pixels are never ink, whichever way it's read.
            if a >= 128.0 && (if s.invert { lum > cut } else { lum < cut }) {
                ink[y * w + x] = 1;
                any = true;
            }
        }
    }
    if !any {
        return None;
    }
    let encode = if s.target == Layer::Text { encode_text } else { encode_line };
    let (data, fw, fh) = mask_to_field(&ink, w, h, ss as usize, 0.0, unit, encode);
    Some(Field { data, w: fw, h: fh, x: x0 as i64, y: y0 as i64 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba(f(x, y)));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn a_dark_square_becomes_ink() {
        // Black in the middle half, white around.
        let bytes =
            png(64, 32, |x, y| if (16..48).contains(&x) && (8..24).contains(&y) { [0, 0, 0, 255] } else { [255; 4] });
        let mut pics = Pictures::default();
        let (id, aspect) = pics.add(bytes).unwrap();
        assert_eq!(aspect, 0.5);
        let s = StampObject {
            common: Default::default(),
            asset: id,
            target: Layer::Text,
            x: 300.0,
            y: 200.0,
            width: 40.0,
            rotation: 0.0,
            threshold: 0.5,
            invert: false,
            aspect,
        };
        let fit = Fit { sx: 1.0, sy: 1.0, ox: 0.0, oy: 0.0 };
        let f = pics.field(&s, &fit).unwrap();
        let at = |x: i64, y: i64| f.data[((y - f.y) as usize) * f.w + (x - f.x) as usize];
        assert!((at(300, 200) as f64) < super::super::labels::TEXT_EDGE);
        // The picture's white edge isn't ink: 15 units right of the middle.
        assert!((at(315, 200) as f64) > super::super::labels::TEXT_EDGE);
        // Inverted, the middle is paper.
        let inv = StampObject { invert: true, ..s };
        let f = pics.field(&inv, &fit).unwrap();
        assert!(f.data[((200 - f.y) as usize) * f.w + (300 - f.x) as usize] as f64 > 68.0);
    }
}
