//! Bézier paths, the Illustrator way: anchors with optional in and out
//! handles, stored relative to their anchor so that moving an anchor carries
//! them along.

use serde::{Deserialize, Serialize};

pub type Point = [f64; 2];

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathNode {
    pub p: Point,
    /// The handle towards the previous anchor (relative); `None` is straight.
    #[serde(rename = "in")]
    pub inn: Option<Point>,
    /// The handle towards the next anchor (relative).
    pub out: Option<Point>,
}

impl PathNode {
    /// A corner (no handles).
    pub fn corner(p: Point) -> Self {
        PathNode { p, inn: None, out: None }
    }
    /// A smooth anchor: `out`, and the opposite as `in`.
    pub fn smooth(p: Point, out: Point) -> Self {
        PathNode { p, inn: Some([-out[0], -out[1]]), out: Some(out) }
    }
}

pub type Segment = [Point; 4];

fn add(a: Point, b: Option<Point>) -> Point {
    b.map_or(a, |b| [a[0] + b[0], a[1] + b[1]])
}

/// The path's segments as cubic control points; a closed path adds last to
/// first.
pub fn segments(nodes: &[PathNode], closed: bool) -> Vec<Segment> {
    let n = nodes.len();
    let count = if closed { n } else { n.saturating_sub(1) };
    (0..count)
        .map(|i| {
            let (a, b) = (&nodes[i], &nodes[(i + 1) % n]);
            [a.p, add(a.p, a.out), add(b.p, b.inn), b.p]
        })
        .collect()
}

pub fn bezier_at([p0, c1, c2, p3]: &Segment, t: f64) -> Point {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    [a * p0[0] + b * c1[0] + c * c2[0] + d * p3[0], a * p0[1] + b * c1[1] + c * c2[1] + d * p3[1]]
}

fn straight([p0, c1, c2, p3]: &Segment) -> bool {
    c1 == p0 && c2 == p3
}

fn dist(a: Point, b: Point) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// A polyline through the path, fine enough that no chord strays from the
/// curve.
pub fn flatten(nodes: &[PathNode], closed: bool) -> Vec<Point> {
    let Some(first) = nodes.first() else { return Vec::new() };
    let mut pts = vec![first.p];
    for seg in segments(nodes, closed) {
        if straight(&seg) {
            pts.push(seg[3]);
            continue;
        }
        let hull = dist(seg[1], seg[0]) + dist(seg[2], seg[1]) + dist(seg[3], seg[2]);
        let steps = ((hull / 2.0).ceil() as usize).clamp(4, 96);
        for s in 1..=steps {
            pts.push(bezier_at(&seg, s as f64 / steps as f64));
        }
    }
    pts
}

/// The old "smooth" polylines (centripetal Catmull-Rom through the points)
/// as the same curve in Bézier handles, so that saved projects keep their
/// shape.
pub fn from_points(points: &[Point], smooth: bool) -> Vec<PathNode> {
    let mut nodes: Vec<PathNode> = points.iter().map(|&p| PathNode::corner(p)).collect();
    let n = points.len();
    if !smooth || n < 3 {
        return nodes;
    }
    let at = |i: isize| points[i.clamp(0, n as isize - 1) as usize];
    for i in 0..n - 1 {
        let i = i as isize;
        let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
        let t01 = dist(p1, p0).sqrt().max(1e-4);
        let t12 = dist(p2, p1).sqrt().max(1e-4);
        let t23 = dist(p3, p2).sqrt().max(1e-4);
        let (mut m1, mut m2) = ([0.0; 2], [0.0; 2]);
        for k in 0..2 {
            m1[k] = ((p1[k] - p0[k]) / t01 - (p2[k] - p0[k]) / (t01 + t12) + (p2[k] - p1[k]) / t12) * t12;
            m2[k] = ((p2[k] - p1[k]) / t12 - (p3[k] - p1[k]) / (t12 + t23) + (p3[k] - p2[k]) / t23) * t12;
        }
        // A Hermite span with tangents m1, m2 is the Bézier with handles m1/3
        // and -m2/3.
        nodes[i as usize].out = Some([m1[0] / 3.0, m1[1] / 3.0]);
        nodes[i as usize + 1].inn = Some([-m2[0] / 3.0, -m2[1] / 3.0]);
    }
    nodes
}

/// Splits segment `i` at `t` (de Casteljau), the curve unchanged.
pub fn insert_node(nodes: &[PathNode], closed: bool, i: usize, t: f64) -> Vec<PathNode> {
    let seg = segments(nodes, closed)[i];
    let [p0, c1, c2, p3] = seg;
    let lerp = |a: Point, b: Point| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    let (a, b, c) = (lerp(p0, c1), lerp(c1, c2), lerp(c2, p3));
    let (d, e) = (lerp(a, b), lerp(b, c));
    let m = lerp(d, e);
    let rel = |q: Point| [q[0] - m[0], q[1] - m[1]];
    let mut out = nodes.to_vec();
    let j = (i + 1) % nodes.len();
    if out[i].out.is_some() {
        out[i].out = Some([a[0] - p0[0], a[1] - p0[1]]);
    }
    if out[j].inn.is_some() {
        out[j].inn = Some([c[0] - p3[0], c[1] - p3[1]]);
    }
    let curved = !straight(&seg);
    let node = PathNode { p: m, inn: curved.then(|| rel(d)), out: curved.then(|| rel(e)) };
    out.insert(i + 1, node);
    out
}

/// The nearest point of the path: segment, parameter and distance.
#[derive(Clone, Copy, Debug)]
pub struct Nearest {
    pub i: usize,
    pub t: f64,
    pub d: f64,
}

pub fn nearest_on_path(nodes: &[PathNode], closed: bool, q: Point) -> Nearest {
    let mut best = Nearest { i: 0, t: 0.0, d: f64::INFINITY };
    for (i, seg) in segments(nodes, closed).iter().enumerate() {
        let st = straight(seg);
        let steps = if st { 1 } else { 32 };
        let mut prev = seg[0];
        for s in 1..=steps {
            let cur = if st { seg[3] } else { bezier_at(seg, s as f64 / steps as f64) };
            let (vx, vy) = (cur[0] - prev[0], cur[1] - prev[1]);
            let len2 = vx * vx + vy * vy;
            let u =
                if len2 > 0.0 { (((q[0] - prev[0]) * vx + (q[1] - prev[1]) * vy) / len2).clamp(0.0, 1.0) } else { 0.0 };
            let d = (q[0] - prev[0] - vx * u).hypot(q[1] - prev[1] - vy * u);
            if d < best.d {
                best = Nearest { i, t: (s as f64 - 1.0 + u) / steps as f64, d };
            }
            prev = cur;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Point, b: Point) -> bool {
        dist(a, b) < 1e-9
    }

    // The web app's path tests.
    #[test]
    fn straight_paths_stay_polylines() {
        let nodes = from_points(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]], false);
        assert_eq!(flatten(&nodes, false), vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]);
        assert_eq!(flatten(&nodes, true).last(), Some(&[0.0, 0.0]));
    }

    #[test]
    fn smooth_points_pass_through_their_points() {
        let pts = [[0.0, 0.0], [20.0, 10.0], [40.0, 0.0], [60.0, 15.0]];
        let nodes = from_points(&pts, true);
        let segs = segments(&nodes, false);
        for (i, s) in segs.iter().enumerate() {
            assert!(close(bezier_at(s, 0.0), pts[i]));
            assert!(close(bezier_at(s, 1.0), pts[i + 1]));
        }
        assert!(nodes[1].inn.is_some() && nodes[1].out.is_some());
    }

    #[test]
    fn inserting_keeps_the_curve() {
        let nodes = vec![PathNode::smooth([0.0, 0.0], [10.0, 20.0]), PathNode::smooth([50.0, 0.0], [10.0, -20.0])];
        let before = segments(&nodes, false)[0];
        let split = insert_node(&nodes, false, 0, 0.3);
        assert_eq!(split.len(), 3);
        let segs = segments(&split, false);
        for k in 0..=10 {
            let t = k as f64 / 10.0;
            let p = bezier_at(&segs[0], t);
            // A point of the first half is on the original curve at 0.3 t.
            assert!(close(p, bezier_at(&before, 0.3 * t)), "{t}");
        }
        assert!(close(split[1].p, bezier_at(&before, 0.3)));
    }

    #[test]
    fn nearest_point() {
        let nodes = from_points(&[[0.0, 0.0], [10.0, 0.0]], false);
        let n = nearest_on_path(&nodes, false, [4.0, 3.0]);
        assert_eq!(n.i, 0);
        assert!((n.t - 0.4).abs() < 1e-9 && (n.d - 3.0).abs() < 1e-9);
    }

    #[test]
    fn json_like_the_web_app() {
        let n = PathNode { p: [1.0, 2.0], inn: None, out: Some([3.0, 4.0]) };
        assert_eq!(serde_json::to_string(&n).unwrap(), r#"{"p":[1.0,2.0],"in":null,"out":[3.0,4.0]}"#);
        let back: PathNode = serde_json::from_str(r#"{"p":[1,2],"in":null,"out":[3,4]}"#).unwrap();
        assert_eq!(back, n);
    }
}
