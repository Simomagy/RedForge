//! Building footprints. The game draws a building twice: its outline in the
//! line family and a grey fill in the relief family (undiscovered3), whose
//! inside is a signed field (measured on Saint Denis: ~54 at the footprint's
//! edge, falling ~3.3 per detail pixel inwards to 11, rising ~6 per detail
//! pixel outwards).

use super::families::{FAMILIES, Fit, fit_of};
use super::linework::{Bounds, Shapes, polyline_shapes, tile_range};
use super::path::Point;
use super::project::{BuildingObject, BuildingShape, LineStyle};

/// Rings in footprint space, -0.5 to 0.5 of the width and height.
fn rings(shape: BuildingShape) -> Vec<Vec<Point>> {
    match shape {
        BuildingShape::Rect | BuildingShape::Station => vec![vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]]],
        BuildingShape::L => {
            vec![vec![[-0.5, -0.5], [0.5, -0.5], [0.5, -0.05], [-0.05, -0.05], [-0.05, 0.5], [-0.5, 0.5]]]
        }
        BuildingShape::U => vec![vec![
            [-0.5, -0.5],
            [-0.18, -0.5],
            [-0.18, 0.12],
            [0.18, 0.12],
            [0.18, -0.5],
            [0.5, -0.5],
            [0.5, 0.5],
            [-0.5, 0.5],
        ]],
        BuildingShape::T => vec![vec![
            [-0.5, -0.5],
            [0.5, -0.5],
            [0.5, -0.1],
            [0.17, -0.1],
            [0.17, 0.5],
            [-0.17, 0.5],
            [-0.17, -0.1],
            [-0.5, -0.1],
        ]],
        BuildingShape::Courtyard => vec![
            vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]],
            vec![[-0.26, -0.26], [0.26, -0.26], [0.26, 0.26], [-0.26, 0.26]],
        ],
    }
}

fn to_map(b: &BuildingObject) -> impl Fn(Point) -> Point + '_ {
    let (sin, cos) = b.rotation.to_radians().sin_cos();
    move |[u, v]| {
        let (x, y) = (u * b.w, v * b.h);
        [b.x + x * cos - y * sin, b.y + x * sin + y * cos]
    }
}

/// Closed rings in map units (a station's is a circle).
pub fn building_rings(b: &BuildingObject) -> Vec<Vec<Point>> {
    if b.shape == BuildingShape::Station {
        let r = b.w.min(b.h) / 2.0;
        let ring = (0..=32)
            .map(|i| {
                let a = i as f64 / 32.0 * std::f64::consts::TAU;
                [b.x + a.cos() * r, b.y + a.sin() * r]
            })
            .collect();
        return vec![ring];
    }
    let f = to_map(b);
    rings(b.shape).into_iter().map(|ring| ring.iter().chain(ring.first()).map(|&p| f(p)).collect()).collect()
}

/// A station's centre dot's diameter: as set, or 30% of the ring.
pub fn station_dot(b: &BuildingObject) -> f64 {
    b.dot.unwrap_or(b.w.min(b.h) * 0.3)
}

/// The rotated box, for picking and the selection's handles.
pub fn building_corners(b: &BuildingObject) -> [Point; 4] {
    let f = to_map(b);
    [f([-0.5, -0.5]), f([0.5, -0.5]), f([0.5, 0.5]), f([-0.5, 0.5])]
}

pub fn outline_shapes(b: &BuildingObject, fit: &Fit) -> Shapes {
    let mut parts: Vec<Shapes> =
        building_rings(b).iter().map(|ring| polyline_shapes(ring, LineStyle::Solid, b.outline, 0.0, fit)).collect();
    // A station is a ring with a dot in the middle, as the game marks depots.
    if b.shape == BuildingShape::Station {
        let dot = station_dot(b);
        if dot > 0.0 {
            parts.push(polyline_shapes(&[[b.x, b.y]], LineStyle::Solid, dot, 0.0, fit));
        }
    }
    let (slope, halo) = (parts[0].slope, parts[0].halo);
    let bounds = parts.iter().fold(Bounds::EMPTY, |a, p| a.union(p.bounds));
    Shapes { caps: parts.into_iter().flat_map(|p| p.caps).collect(), holes: Vec::new(), bounds, slope, halo }
}

const FILL_EDGE: f64 = 54.0;
const FILL_IN: f64 = 3.3;
const FILL_FLOOR: f64 = 11.0;
const FILL_OUT: f64 = 6.0;
/// Detail pixels outside the edge still worth writing (the terrain is at
/// ~60-90).
const FILL_PAD: f64 = 12.0;

#[derive(Clone, Debug, PartialEq)]
pub struct FillShape {
    /// Atlas pixels.
    pub rings: Vec<Vec<Point>>,
    /// Detail pixels per atlas pixel.
    pub ratio: f64,
    pub bounds: Bounds,
}

pub fn fill_shape(b: &BuildingObject, fit: &Fit) -> Option<FillShape> {
    if b.shape == BuildingShape::Station {
        return None;
    }
    let rings: Vec<Vec<Point>> =
        building_rings(b).into_iter().map(|r| r.into_iter().map(|p| fit.apply(p)).collect()).collect();
    let ratio = fit_of(&FAMILIES[4]).unit() / fit.unit();
    let bounds = Bounds::of_points(rings.iter().flatten(), FILL_PAD / ratio + 1.0);
    Some(FillShape { rings, ratio, bounds })
}

/// `tile = min(tile, fill)` over the footprint and near it.
pub fn stamp_fill(tile: &mut [u8], size: usize, tx: f64, ty: f64, s: &FillShape) {
    let (x0, y0, x1, y1) = tile_range(&s.bounds, size, tx, ty);
    for y in y0..y1 {
        let py = ty + y as f64 + 0.5;
        for x in x0..x1 {
            let px = tx + x as f64 + 0.5;
            let (mut inside, mut best) = (false, f64::INFINITY);
            for ring in &s.rings {
                for w in ring.windows(2) {
                    let ([ax, ay], [bx, by]) = (w[0], w[1]);
                    // Even-odd: a courtyard stays open.
                    if (ay > py) != (by > py) && px < (bx - ax) * (py - ay) / (by - ay) + ax {
                        inside = !inside;
                    }
                    let (vx, vy) = (bx - ax, by - ay);
                    let len2 = vx * vx + vy * vy;
                    let t = if len2 > 0.0 { (((px - ax) * vx + (py - ay) * vy) / len2).clamp(0.0, 1.0) } else { 0.0 };
                    best = best.min((px - ax - vx * t).hypot(py - ay - vy * t));
                }
            }
            let d = best * s.ratio;
            let v =
                if inside { (FILL_EDGE - FILL_IN * d).max(FILL_FLOOR) } else { (FILL_EDGE + FILL_OUT * d).min(255.0) };
            let i = y * size + x;
            if v < tile[i] as f64 {
                tile[i] = v.round() as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fit() -> Fit {
        fit_of(&FAMILIES[4])
    }

    fn building(shape: BuildingShape) -> BuildingObject {
        let fit = fit();
        // Centred on atlas pixel (128, 128) of tile 0_0.
        BuildingObject {
            common: Default::default(),
            shape,
            x: 128.0 / fit.sx - fit.ox,
            y: 128.0 / fit.sy - fit.oy,
            w: 40.0,
            h: 40.0,
            rotation: 0.0,
            outline: 0.8,
            dot: None,
        }
    }

    fn fill(shape: BuildingShape) -> impl Fn(usize, usize) -> u8 {
        let mut tile = vec![255u8; 256 * 256];
        if let Some(s) = fill_shape(&building(shape), &fit()) {
            stamp_fill(&mut tile, 256, 0.0, 0.0, &s);
        }
        move |x, y| tile[y * 256 + x]
    }

    #[test]
    fn a_block_is_filled_like_the_game() {
        let at = fill(BuildingShape::Rect);
        let half = 40.0 * fit().sx / 2.0;
        assert!(at(128, 128) < 20);
        assert!((at((128.0 + half).round() as usize, 128) as i32 - 54).abs() < 8);
        assert!(at((128.0 + half + 6.0).round() as usize, 128) > 80);
    }

    #[test]
    fn a_courtyard_stays_open_and_a_station_has_no_fill() {
        assert!(fill(BuildingShape::Courtyard)(128, 128) > 54);
        assert!(fill_shape(&building(BuildingShape::Station), &fit()).is_none());
    }

    #[test]
    fn a_stations_dot() {
        let fit = fit();
        let station = |dot: Option<f64>| BuildingObject {
            w: 20.0,
            h: 20.0,
            outline: 1.0,
            dot,
            ..building(BuildingShape::Station)
        };
        let radius = |b: BuildingObject| {
            let s = outline_shapes(&b, &fit);
            s.caps
                .iter()
                .find(|c| c.ax == c.bx && c.ay == c.by && (c.ax - 128.0).hypot(c.ay - 128.0) < 1.0)
                .map_or(0.0, |c| c.r / fit.unit())
        };
        assert!((radius(station(None)) - 3.0).abs() < 0.05);
        assert!((radius(station(Some(8.0))) - 4.0).abs() < 0.05);
        assert_eq!(radius(station(Some(0.0))), 0.0);
    }
}
