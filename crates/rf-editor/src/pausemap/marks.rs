//! A mark is what one object leaves on one tile family. Erasers clear the
//! game's own pixels first; everything else is merged with `min`, so that
//! what's drawn is never erased.

use super::buildings::{FillShape, fill_shape, outline_shapes, stamp_fill};
use super::families::{Family, Layer, fit_of};
use super::fonts::Fonts;
use super::labels::{Field, Lettering};
use super::linework::{Bounds, Shapes, line_shapes, stamp_shapes, tile_range};
use super::path::Point;
use super::project::MapObject;
use super::stamps::Pictures;

/// What marks are made with besides the objects: fonts and pictures.
#[derive(Default)]
pub struct Assets {
    pub fonts: Fonts,
    pub lettering: Lettering,
    pub pictures: Pictures,
}

#[derive(Clone, Debug)]
pub enum Mark {
    Shapes(Shapes),
    Fill(FillShape),
    Field(Field),
    /// A brush stroke in atlas pixels, with the brush's radii.
    Erase {
        points: Vec<Point>,
        rx: f64,
        ry: f64,
        bounds: Bounds,
    },
}

impl Mark {
    /// Atlas pixels.
    pub fn bounds(&self) -> Bounds {
        match self {
            Mark::Shapes(s) => s.bounds,
            Mark::Fill(f) => f.bounds,
            Mark::Field(f) => {
                Bounds { x0: f.x as f64, y0: f.y as f64, x1: (f.x + f.w as i64) as f64, y1: (f.y + f.h as i64) as f64 }
            }
            Mark::Erase { bounds, .. } => *bounds,
        }
    }

    pub fn erases(&self) -> bool {
        matches!(self, Mark::Erase { .. })
    }

    /// Writes the mark into a tile whose top-left is at `(tx, ty)` in atlas
    /// pixels.
    pub fn stamp(&self, tile: &mut [u8], size: usize, tx: f64, ty: f64) {
        match self {
            Mark::Shapes(s) => {
                stamp_shapes(tile, size, tx, ty, s);
            }
            Mark::Fill(f) => stamp_fill(tile, size, tx, ty, f),
            Mark::Field(f) => stamp_field(tile, size, tx as i64, ty as i64, f),
            Mark::Erase { points, rx, ry, .. } => erase_stroke(tile, size, tx, ty, points, *rx, *ry),
        }
    }
}

pub fn mark_for(f: &Family, o: &MapObject, assets: &Assets) -> Option<Mark> {
    if o.common().hidden {
        return None;
    }
    let fit = fit_of(f);
    match o {
        MapObject::Line(l) if f.layer == Layer::Lines && !l.nodes.is_empty() => {
            Some(Mark::Shapes(line_shapes(l, &fit)))
        }
        MapObject::Building(b) if f.layer == Layer::Lines => Some(Mark::Shapes(outline_shapes(b, &fit))),
        MapObject::Building(b) if f.layer == Layer::Relief => fill_shape(b, &fit).map(Mark::Fill),
        MapObject::Stamp(s) if f.layer == s.target => assets.pictures.field(s, &fit).map(Mark::Field),
        MapObject::Erase(e) if f.layer == e.target && !e.points.is_empty() => {
            let points: Vec<Point> = e.points.iter().map(|&p| fit.apply(p)).collect();
            let (rx, ry) = (e.radius * fit.sx, e.radius * fit.sy);
            let b = Bounds::of_points(&points, 0.0);
            let bounds = Bounds { x0: b.x0 - rx, y0: b.y0 - ry, x1: b.x1 + rx, y1: b.y1 + ry };
            Some(Mark::Erase { points, rx, ry, bounds })
        }
        MapObject::Label(l) if f.layer == Layer::Text && !l.text.trim().is_empty() => {
            assets.lettering.field(l, &fit, &assets.fonts).map(Mark::Field)
        }
        _ => None,
    }
}

/// `tile = min(tile, field)` where they overlap.
fn stamp_field(tile: &mut [u8], size: usize, tx: i64, ty: i64, f: &Field) {
    let (x0, y0) = (f.x.max(tx), f.y.max(ty));
    let (x1, y1) = ((f.x + f.w as i64).min(tx + size as i64), (f.y + f.h as i64).min(ty + size as i64));
    for y in y0..y1 {
        for x in x0..x1 {
            let v = f.data[((y - f.y) as usize) * f.w + (x - f.x) as usize];
            let i = ((y - ty) as usize) * size + (x - tx) as usize;
            if v < tile[i] {
                tile[i] = v;
            }
        }
    }
}

/// Sets to 255 every pixel within an elliptical brush swept along the stroke.
fn erase_stroke(tile: &mut [u8], size: usize, tx: f64, ty: f64, pts: &[Point], rx: f64, ry: f64) {
    let segs: Vec<(Point, Point)> =
        if pts.len() == 1 { vec![(pts[0], pts[0])] } else { pts.windows(2).map(|w| (w[0], w[1])).collect() };
    for (a, b) in segs {
        let bb = Bounds {
            x0: a[0].min(b[0]) - rx,
            y0: a[1].min(b[1]) - ry,
            x1: a[0].max(b[0]) + rx,
            y1: a[1].max(b[1]) + ry,
        };
        let (x0, y0, x1, y1) = tile_range(&bb, size, tx, ty);
        // In the brush's space the ellipse is a unit circle.
        let (ax, ay, vx, vy) = (a[0] / rx, a[1] / ry, (b[0] - a[0]) / rx, (b[1] - a[1]) / ry);
        let len2 = vx * vx + vy * vy;
        for y in y0..y1 {
            let py = (ty + y as f64 + 0.5) / ry - ay;
            for x in x0..x1 {
                let px = (tx + x as f64 + 0.5) / rx - ax;
                let s = if len2 > 0.0 { ((px * vx + py * vy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                if (px - vx * s).powi(2) + (py - vy * s).powi(2) <= 1.0 {
                    tile[y * size + x] = 255;
                }
            }
        }
    }
}
