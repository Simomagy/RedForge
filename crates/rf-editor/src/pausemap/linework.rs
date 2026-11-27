//! Linework as a distance field, the way the game stores it in the
//! "discovered" families: about 64 on the stroke's edge, rising ~7 per detail
//! pixel outwards, darker inside. Shapes are built analytically (capsules,
//! minus round holes), so the field is exact at any scale and each segment
//! only touches the pixels near it.
//!
//! Distances are counted in pixels of the full-resolution family in every
//! family: the game's LOD tiles are its detail fields scaled down (the same
//! values at the same map point), which keeps thin roads dark instead of
//! fading out when zoomed out.

use super::families::{FAMILIES, Fit, fit_of};
use super::path::{Point, flatten};
use super::project::{LineObject, LineStyle};

pub const LINE_EDGE: f64 = 64.0;
/// Per detail pixel.
const SLOPE: f64 = 7.0;
const FLOOR: f64 = 40.0;
/// The thinnest stroke that still reaches the game's ink (detail pixels).
const MIN_HALF_WIDTH: f64 = 0.75;

/// Detail pixels per map unit.
fn detail_unit() -> f64 {
    fit_of(&FAMILIES[2]).unit()
}

/// The line encoding of a signed distance in map units (for marks made from
/// masks).
pub fn encode_line(sd_map: f64) -> u8 {
    (LINE_EDGE + sd_map * detail_unit() * SLOPE).clamp(FLOOR, 255.0).round() as u8
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Capsule {
    pub ax: f64,
    pub ay: f64,
    pub bx: f64,
    pub by: f64,
    pub r: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hole {
    pub x: f64,
    pub y: f64,
    pub r: f64,
}

/// A box in atlas pixels (or map units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Bounds {
    pub const EMPTY: Bounds =
        Bounds { x0: f64::INFINITY, y0: f64::INFINITY, x1: f64::NEG_INFINITY, y1: f64::NEG_INFINITY };

    pub fn of_points<'a>(pts: impl IntoIterator<Item = &'a Point>, pad: f64) -> Bounds {
        let mut b = Bounds::EMPTY;
        for p in pts {
            b.x0 = b.x0.min(p[0]);
            b.y0 = b.y0.min(p[1]);
            b.x1 = b.x1.max(p[0]);
            b.y1 = b.y1.max(p[1]);
        }
        b.pad(pad)
    }
    pub fn pad(self, p: f64) -> Bounds {
        Bounds { x0: self.x0 - p, y0: self.y0 - p, x1: self.x1 + p, y1: self.y1 + p }
    }
    pub fn union(self, o: Bounds) -> Bounds {
        Bounds { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }
    pub fn is_empty(&self) -> bool {
        !(self.x0 <= self.x1 && self.y0 <= self.y1)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shapes {
    pub caps: Vec<Capsule>,
    pub holes: Vec<Hole>,
    /// Atlas pixels, the halo included.
    pub bounds: Bounds,
    /// The field's change per atlas pixel of this family.
    pub slope: f64,
    /// Atlas pixels from the stroke where the field reaches 255.
    pub halo: f64,
}

/// Dashes along a polyline: dash k covers arc length `[k period - phase, + on)`.
/// Each segment meets the dashes over it, so the work is bounded by its
/// length (stepping by remainders stalls on rounding).
fn along(path: &[Point], period: f64, on: f64, phase: f64, mut out: impl FnMut(f64, f64, f64, f64)) {
    let mut c0 = 0.0;
    for w in path.windows(2) {
        let ([ax, ay], [bx, by]) = (w[0], w[1]);
        let len = (bx - ax).hypot(by - ay);
        if len < 1e-9 {
            continue;
        }
        let c1 = c0 + len;
        let mut k = ((c0 + phase) / period).floor();
        while k * period - phase < c1 {
            let d0 = c0.max(k * period - phase);
            let d1 = c1.min(k * period - phase + on);
            if d1 > d0 + 1e-9 {
                let (t0, t1) = ((d0 - c0) / len, (d1 - c0) / len);
                out(ax + (bx - ax) * t0, ay + (by - ay) * t0, ax + (bx - ax) * t1, ay + (by - ay) * t1);
            }
            k += 1.0;
        }
        c0 = c1;
    }
}

/// Points every `interval` of arc length, from `offset`.
fn evenly(path: &[Point], interval: f64, offset: f64, mut out: impl FnMut(f64, f64)) {
    let step = interval.max(1e-3);
    let mut next = offset;
    let mut travelled = 0.0;
    for w in path.windows(2) {
        let ([ax, ay], [bx, by]) = (w[0], w[1]);
        let len = (bx - ax).hypot(by - ay);
        while next <= travelled + len {
            let t = (next - travelled) / len.max(1e-6);
            out(ax + (bx - ax) * t, ay + (by - ay) * t);
            next += step;
        }
        travelled += len;
    }
}

/// A line's shapes in a family's atlas.
pub fn line_shapes(o: &LineObject, fit: &Fit) -> Shapes {
    polyline_shapes(&flatten(&o.nodes, o.closed), o.style, o.width, o.spacing, fit)
}

/// The shapes of a styled polyline given in map units.
pub fn polyline_shapes(points: &[Point], style: LineStyle, width: f64, spacing_map: f64, fit: &Fit) -> Shapes {
    let path: Vec<Point> = points.iter().map(|&p| fit.apply(p)).collect();
    let scale = fit.unit();
    let ratio = detail_unit() / scale;
    let slope = SLOPE * ratio;
    let halo = (255.0 - LINE_EDGE) / slope;
    let half = (MIN_HALF_WIDTH / ratio).max(width * scale / 2.0);
    let spacing = (spacing_map * scale).max(1.0);
    let mut caps = Vec::new();
    let mut holes = Vec::new();
    let mut seg = |ax, ay, bx, by| caps.push(Capsule { ax, ay, bx, by, r: half });
    match style {
        LineStyle::Trail => along(&path, spacing * 1.6, spacing, 0.0, &mut seg),
        LineStyle::Boundary => evenly(&path, spacing, 0.0, |x, y| seg(x, y, x, y)),
        _ => {
            for w in path.windows(2) {
                seg(w[0][0], w[0][1], w[1][0], w[1][1]);
            }
            if let [p] = path.as_slice() {
                seg(p[0], p[1], p[0], p[1]);
            }
            // A railroad is a heavy stroke with small parchment markers in it.
            if style == LineStyle::Railroad {
                evenly(&path, spacing, spacing / 2.0, |x, y| holes.push(Hole { x, y, r: half * 0.55 }));
            }
        }
    }
    let mut b = Bounds::EMPTY;
    for c in &caps {
        b = b.union(Bounds { x0: c.ax.min(c.bx), y0: c.ay.min(c.by), x1: c.ax.max(c.bx), y1: c.ay.max(c.by) });
    }
    Shapes { caps, holes, bounds: b.pad(half + halo + 1.0), slope, halo }
}

/// The pixel range of a box in a tile whose top-left is at `(tx, ty)`.
pub(super) fn tile_range(b: &Bounds, size: usize, tx: f64, ty: f64) -> (usize, usize, usize, usize) {
    let clamp = |v: f64| v.clamp(0.0, size as f64) as usize;
    (clamp((b.x0 - tx).floor()), clamp((b.y0 - ty).floor()), clamp((b.x1 - tx).ceil()), clamp((b.y1 - ty).ceil()))
}

/// Writes shapes into one tile whose top-left is at `(tx, ty)` in atlas
/// pixels: `tile = min(tile, field)`. Pixel centres are at +0.5.
pub fn stamp_shapes(tile: &mut [u8], size: usize, tx: f64, ty: f64, s: &Shapes) -> bool {
    let (rx0, ry0, rx1, ry1) = tile_range(&s.bounds, size, tx, ty);
    if rx1 <= rx0 || ry1 <= ry0 {
        return false;
    }
    let (w, h) = (rx1 - rx0, ry1 - ry0);
    // The signed distance to the stroke's surface, per pixel of the region.
    let mut d = vec![s.halo + 1.0; w * h];
    for c in &s.caps {
        let reach = c.r + s.halo;
        let cb = Bounds { x0: c.ax.min(c.bx), y0: c.ay.min(c.by), x1: c.ax.max(c.bx), y1: c.ay.max(c.by) }.pad(reach);
        let (cx0, cy0, cx1, cy1) = tile_range(&cb, size, tx, ty);
        let (vx, vy) = (c.bx - c.ax, c.by - c.ay);
        let len2 = vx * vx + vy * vy;
        for y in cy0.max(ry0)..cy1.min(ry1) {
            let py = ty + y as f64 + 0.5 - c.ay;
            for x in cx0.max(rx0)..cx1.min(rx1) {
                let px = tx + x as f64 + 0.5 - c.ax;
                let t = if len2 > 0.0 { ((px * vx + py * vy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                let dist = (px - vx * t).hypot(py - vy * t) - c.r;
                let i = (y - ry0) * w + (x - rx0);
                if dist < d[i] {
                    d[i] = dist;
                }
            }
        }
    }
    for o in &s.holes {
        let hb = Bounds { x0: o.x, y0: o.y, x1: o.x, y1: o.y }.pad(o.r + s.halo);
        let (hx0, hy0, hx1, hy1) = tile_range(&hb, size, tx, ty);
        for y in hy0.max(ry0)..hy1.min(ry1) {
            for x in hx0.max(rx0)..hx1.min(rx1) {
                let i = (y - ry0) * w + (x - rx0);
                let inside = o.r - (tx + x as f64 + 0.5 - o.x).hypot(ty + y as f64 + 0.5 - o.y);
                // The hole taken away: max(stroke, -hole).
                if inside > d[i] {
                    d[i] = inside;
                }
            }
        }
    }
    let mut changed = false;
    for y in 0..h {
        for x in 0..w {
            let v = (LINE_EDGE + d[y * w + x] * s.slope).clamp(FLOOR, 255.0).round() as u8;
            let i = (ry0 + y) * size + rx0 + x;
            if v < tile[i] {
                tile[i] = v;
                changed = true;
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pausemap::compose::tiles_in;
    use crate::pausemap::path::from_points;

    const IDENTITY: Fit = Fit { sx: 1.0, sy: 1.0, ox: 0.0, oy: 0.0 };

    fn line(style: LineStyle, width: f64, spacing: f64) -> LineObject {
        LineObject {
            common: Default::default(),
            style,
            width,
            spacing,
            closed: false,
            nodes: from_points(&[[10.0, 50.0], [190.0, 50.0]], false),
        }
    }

    fn render(o: &LineObject) -> impl Fn(usize, usize) -> u8 + use<> {
        let mut tile = vec![255u8; 200 * 200];
        stamp_shapes(&mut tile, 200, 0.0, 0.0, &line_shapes(o, &IDENTITY));
        move |x, y| tile[y * 200 + x]
    }

    #[test]
    fn a_road_is_a_field() {
        let at = render(&line(LineStyle::Road, 2.0, 10.0));
        assert!(at(100, 50) < 64);
        assert!(at(100, 53) > 64);
        assert!(at(100, 45) > at(100, 48));
        assert_eq!(at(100, 120), 255);
    }

    #[test]
    fn trails_have_gaps_and_railroads_markers() {
        let trail = render(&line(LineStyle::Trail, 2.0, 10.0));
        let row: Vec<u8> = (0..160).map(|i| trail(20 + i, 50)).collect();
        assert!(*row.iter().min().unwrap() < 64 && *row.iter().max().unwrap() > 64);
        let rail = render(&line(LineStyle::Railroad, 4.0, 12.0));
        let track: Vec<u8> = (0..160).map(|i| rail(20 + i, 50)).collect();
        assert!(*track.iter().min().unwrap() < 64 && *track.iter().max().unwrap() > 64);
    }

    #[test]
    fn long_wandering_lines_always_finish() {
        let mut seed: u64 = 7;
        let mut rnd = || {
            seed = (seed * 16807) % 2147483647;
            seed as f64 / 2147483647.0
        };
        for _ in 0..300 {
            let mut pts = vec![[rnd() * 5000.0, rnd() * 4000.0]];
            for _ in 0..60 {
                let l = *pts.last().unwrap();
                pts.push([l[0] + (rnd() - 0.5) * 40.0, l[1] + (rnd() - 0.5) * 40.0]);
            }
            for style in [LineStyle::Trail, LineStyle::Boundary, LineStyle::Railroad] {
                let spacing = 1.0 + rnd() * 12.0;
                let s = polyline_shapes(&pts, style, 1.0, spacing, &IDENTITY);
                assert!(!s.caps.is_empty() && s.caps.len() < 20000);
            }
        }
    }

    #[test]
    fn a_mark_on_a_seam_goes_in_both_tiles() {
        let f = &FAMILIES[2];
        assert_eq!(tiles_in(f, &Bounds { x0: 254.0, y0: 10.0, x1: 255.0, y1: 11.0 }), vec![(0, 0), (0, 1)]);
        assert_eq!(tiles_in(f, &Bounds { x0: 10.0, y0: 10.0, x1: 20.0, y1: 20.0 }), vec![(0, 0)]);
    }
}
