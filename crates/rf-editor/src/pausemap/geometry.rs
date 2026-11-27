//! The layers' geometry in map units: outlines, boxes, picking, moving and
//! transforming.

use super::buildings::building_corners;
use super::doc::drawables_in;
use super::linework::Bounds;
use super::marks::Assets;
use super::path::{Point, flatten, nearest_on_path};
use super::project::MapObject;
use super::stamps::stamp_corners;

/// The rotated box of a label, building or stamp (placed by its centre).
pub fn box_of(o: &MapObject, assets: &Assets) -> Vec<Point> {
    match o {
        MapObject::Building(b) => building_corners(b).to_vec(),
        MapObject::Label(l) => {
            assets.lettering.corners(l, &assets.fonts).into_iter().map(|[x, y]| [l.x + x, l.y + y]).collect()
        }
        MapObject::Stamp(s) => stamp_corners(s).into_iter().map(|[x, y]| [s.x + x, s.y + y]).collect(),
        _ => Vec::new(),
    }
}

pub fn is_boxed(o: &MapObject) -> bool {
    matches!(o, MapObject::Label(_) | MapObject::Building(_) | MapObject::Stamp(_))
}

/// A line's path, flattened.
pub fn line_path(o: &MapObject) -> Vec<Point> {
    match o {
        MapObject::Line(l) => flatten(&l.nodes, l.closed),
        _ => Vec::new(),
    }
}

pub fn object_bounds(o: &MapObject, assets: &Assets) -> Bounds {
    match o {
        MapObject::Line(l) => Bounds::of_points(&flatten(&l.nodes, l.closed), l.width / 2.0),
        MapObject::Erase(e) => Bounds::of_points(&e.points, e.radius),
        MapObject::Group(_) => Bounds::EMPTY,
        _ => Bounds::of_points(&box_of(o, assets), 0.0),
    }
}

/// The box of a selection, groups' members included; `None` without
/// anything drawn.
pub fn selection_bounds(ids: &[String], objects: &[MapObject], assets: &Assets) -> Option<Bounds> {
    let items = drawables_in(ids, objects);
    let b = items.iter().fold(Bounds::EMPTY, |b, o| b.union(object_bounds(o, assets)));
    (!b.is_empty()).then_some(b)
}

/// Map units from a point to a polyline.
pub fn distance_to_path(p: Point, pts: &[Point]) -> f64 {
    let mut best = f64::INFINITY;
    for (i, a) in pts.iter().enumerate() {
        let b = pts.get(i + 1).unwrap_or(a);
        let (vx, vy) = (b[0] - a[0], b[1] - a[1]);
        let len2 = vx * vx + vy * vy;
        let t = if len2 > 0.0 { (((p[0] - a[0]) * vx + (p[1] - a[1]) * vy) / len2).clamp(0.0, 1.0) } else { 0.0 };
        best = best.min((p[0] - a[0] - vx * t).hypot(p[1] - a[1] - vy * t));
    }
    best
}

/// Whether a map point is on an object, within `slop` map units.
pub fn hits(o: &MapObject, p: Point, slop: f64, assets: &Assets) -> bool {
    match o {
        MapObject::Line(l) => nearest_on_path(&l.nodes, l.closed, p).d <= slop + l.width / 2.0,
        MapObject::Erase(e) => distance_to_path(p, &e.points) <= e.radius,
        MapObject::Group(_) => false,
        _ => {
            let b = box_of(o, assets);
            let [a, b1, _, d] = match b.as_slice() {
                [a, b, c, d] => [*a, *b, *c, *d],
                _ => return false,
            };
            let (ux, uy, vx, vy) = (b1[0] - a[0], b1[1] - a[1], d[0] - a[0], d[1] - a[1]);
            let (px, py) = (p[0] - a[0], p[1] - a[1]);
            let s = (px * ux + py * uy) / (ux * ux + uy * uy);
            let t = (px * vx + py * vy) / (vx * vx + vy * vy);
            let m = slop / ux.hypot(uy).max(1.0);
            s >= -m && s <= 1.0 + m && t >= -m * 3.0 && t <= 1.0 + m * 3.0
        }
    }
}

/// Whether an object is in (or crosses) a box, for the marquee.
pub fn touches_rect(o: &MapObject, r: &Bounds, assets: &Assets) -> bool {
    let b = object_bounds(o, assets);
    if b.is_empty() || b.x1 < r.x0 || b.x0 > r.x1 || b.y1 < r.y0 || b.y0 > r.y1 {
        return false;
    }
    let pts = match o {
        MapObject::Line(_) => line_path(o),
        MapObject::Erase(e) => e.points.clone(),
        _ => box_of(o, assets),
    };
    let inside = |p: Point| p[0] >= r.x0 && p[0] <= r.x1 && p[1] >= r.y0 && p[1] <= r.y1;
    if pts.iter().any(|&p| inside(p)) {
        return true;
    }
    // A long line can cross the box without a vertex in it: its segments'
    // samples.
    pts.windows(2).any(|w| {
        (1..8).any(|k| {
            let t = k as f64 / 8.0;
            inside([w[0][0] + (w[1][0] - w[0][0]) * t, w[0][1] + (w[1][1] - w[0][1]) * t])
        })
    })
}

/// Moves an object by `(dx, dy)`.
pub fn translate(o: &mut MapObject, dx: f64, dy: f64) {
    match o {
        MapObject::Line(l) => {
            for n in &mut l.nodes {
                n.p = [n.p[0] + dx, n.p[1] + dy];
            }
        }
        MapObject::Erase(e) => {
            for p in &mut e.points {
                *p = [p[0] + dx, p[1] + dy];
            }
        }
        MapObject::Label(l) => (l.x, l.y) = (l.x + dx, l.y + dy),
        MapObject::Building(b) => (b.x, b.y) = (b.x + dx, b.y + dy),
        MapObject::Stamp(s) => (s.x, s.y) = (s.x + dx, s.y + dy),
        MapObject::Group(_) => {}
    }
}

/// `x' = a x + c y + e`, `y' = b x + d y + f`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Affine {
    pub fn apply(&self, [x, y]: Point) -> Point {
        [self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f]
    }
    fn apply_vec(&self, [x, y]: Point) -> Point {
        [self.a * x + self.c * y, self.b * x + self.d * y]
    }
    pub fn scale_about([px, py]: Point, sx: f64, sy: f64) -> Affine {
        Affine { a: sx, b: 0.0, c: 0.0, d: sy, e: px - sx * px, f: py - sy * py }
    }
    pub fn rotate_about([px, py]: Point, theta: f64) -> Affine {
        let (sin, cos) = theta.sin_cos();
        Affine { a: cos, b: sin, c: -sin, d: cos, e: px - cos * px + sin * py, f: py - sin * px - cos * py }
    }
}

/// An object carried through a transform. Lines keep their stroke's width
/// (as Illustrator does by default); boxed objects move, turn and resize
/// along their own axes.
pub fn transform_object(o: &MapObject, m: &Affine) -> MapObject {
    let mut out = o.clone();
    match &mut out {
        MapObject::Group(_) => {}
        MapObject::Line(l) => {
            for n in &mut l.nodes {
                n.p = m.apply(n.p);
                n.inn = n.inn.map(|v| m.apply_vec(v));
                n.out = n.out.map(|v| m.apply_vec(v));
            }
        }
        MapObject::Erase(e) => {
            for p in &mut e.points {
                *p = m.apply(*p);
            }
            e.radius *= (m.a * m.d - m.b * m.c).abs().sqrt();
        }
        _ => {
            let (x, y, rotation) = o.boxed().unwrap();
            let r = rotation.to_radians();
            let u = m.apply_vec([r.cos(), r.sin()]);
            let v = m.apply_vec([-r.sin(), r.cos()]);
            // The stretch along the object's own axes.
            let (fu, fv) = (u[0].hypot(u[1]), v[0].hypot(v[1]));
            let [nx, ny] = m.apply([x, y]);
            let rot = (u[1].atan2(u[0]).to_degrees() * 10.0).round() / 10.0;
            match &mut out {
                MapObject::Label(l) => {
                    (l.x, l.y, l.rotation) = (nx, ny, rot);
                    l.size = (l.size * (fu * fv).sqrt()).max(4.0);
                }
                MapObject::Building(b) => {
                    (b.x, b.y, b.rotation) = (nx, ny, rot);
                    b.w = (b.w * fu).max(1.0);
                    b.h = (b.h * fv).max(1.0);
                    b.dot = b.dot.map(|d| d * (fu * fv).sqrt());
                }
                MapObject::Stamp(s) => {
                    (s.x, s.y, s.rotation) = (nx, ny, rot);
                    s.width = (s.width * fu).max(2.0);
                }
                _ => {}
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pausemap::path::from_points;
    use crate::pausemap::project::{BuildingObject, BuildingShape, Common, LineObject, LineStyle};

    fn building() -> MapObject {
        MapObject::Building(BuildingObject {
            common: Common { id: "b".into(), ..Default::default() },
            shape: BuildingShape::Rect,
            x: 100.0,
            y: 50.0,
            w: 20.0,
            h: 10.0,
            rotation: 0.0,
            outline: 0.8,
            dot: None,
        })
    }

    // The web app's geometry tests.
    #[test]
    fn picking_and_marquee() {
        let a = Assets::default();
        let b = building();
        assert!(hits(&b, [105.0, 52.0], 0.0, &a));
        assert!(!hits(&b, [125.0, 52.0], 1.0, &a));
        let line = MapObject::Line(LineObject {
            common: Common { id: "l".into(), ..Default::default() },
            style: LineStyle::Road,
            width: 2.0,
            spacing: 0.0,
            closed: false,
            nodes: from_points(&[[0.0, 0.0], [100.0, 0.0]], false),
        });
        assert!(hits(&line, [50.0, 1.5], 1.0, &a));
        // A line crossing the box with no vertex in it.
        assert!(touches_rect(&line, &Bounds { x0: 40.0, y0: -5.0, x1: 60.0, y1: 5.0 }, &a));
        assert!(!touches_rect(&line, &Bounds { x0: 40.0, y0: 5.0, x1: 60.0, y1: 15.0 }, &a));
    }

    #[test]
    fn transforms_turn_and_stretch_boxes() {
        let b = building();
        let r = transform_object(&b, &Affine::rotate_about([100.0, 50.0], std::f64::consts::FRAC_PI_2));
        let MapObject::Building(r) = r else { panic!() };
        assert_eq!((r.rotation, r.w, r.h), (90.0, 20.0, 10.0));
        let s = transform_object(&b, &Affine::scale_about([90.0, 50.0], 2.0, 1.0));
        let MapObject::Building(s) = s else { panic!() };
        assert!((s.x - 110.0).abs() < 1e-9 && (s.w - 40.0).abs() < 1e-9 && (s.h - 10.0).abs() < 1e-9);
        let mut t = building();
        translate(&mut t, 5.0, -5.0);
        assert_eq!(t.boxed().unwrap(), (105.0, 45.0, 0.0));
    }
}
