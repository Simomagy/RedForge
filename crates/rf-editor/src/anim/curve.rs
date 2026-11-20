//! F-curves: one number over the frames, from keyframes with Bézier handles,
//! as Blender has them (its `FCurve` and `BezTriple`, and the way it works out
//! the handles and the value between two keys).
//!
//! A keyframe is a point (frame, value) with a handle on each side, each a
//! point too. Between two keys the curve is the cubic Bézier of the first
//! key, its right handle, the second's left handle and the second key, read
//! as value over frame; or a straight line, or the first key's value held,
//! by the first key's interpolation. Before the first key and after the last
//! the curve keeps their values.
//!
//! Handles have a type each: `Auto` and `AutoClamped` are worked out from the
//! neighbouring keys (the clamped one never lets the curve go past them),
//! `Vector` points at the neighbour, `Aligned` is placed by hand and keeps
//! the two in a line, `Free` is placed by hand.

use serde::{Deserialize, Serialize};

/// How the curve goes from a key to the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Interpolation {
    /// The key's value, until the next key.
    Constant,
    /// A straight line.
    Linear,
    /// The curve the handles make.
    #[default]
    Bezier,
}

impl Interpolation {
    pub const ALL: [Interpolation; 3] = [Interpolation::Constant, Interpolation::Linear, Interpolation::Bezier];

    pub fn label(self) -> &'static str {
        match self {
            Interpolation::Constant => "Constant",
            Interpolation::Linear => "Linear",
            Interpolation::Bezier => "Bézier",
        }
    }
}

/// How a handle is placed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Handle {
    Free,
    Aligned,
    Vector,
    Auto,
    #[default]
    AutoClamped,
}

impl Handle {
    pub const ALL: [Handle; 5] = [Handle::Free, Handle::Aligned, Handle::Vector, Handle::Auto, Handle::AutoClamped];

    pub fn label(self) -> &'static str {
        match self {
            Handle::Free => "Free",
            Handle::Aligned => "Aligned",
            Handle::Vector => "Vector",
            Handle::Auto => "Automatic",
            Handle::AutoClamped => "Auto Clamped",
        }
    }

    fn auto(self) -> bool {
        matches!(self, Handle::Auto | Handle::AutoClamped)
    }
}

/// A keyframe: Blender's `BezTriple`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    pub frame: f32,
    pub value: f32,
    #[serde(default)]
    pub interpolation: Interpolation,
    /// The handles, as (frame, value).
    #[serde(default)]
    pub left: [f32; 2],
    #[serde(default)]
    pub right: [f32; 2],
    #[serde(default)]
    pub left_type: Handle,
    #[serde(default)]
    pub right_type: Handle,
    /// Selected in the editors (not saved).
    #[serde(skip)]
    pub selected: bool,
}

impl Keyframe {
    pub fn new(frame: f32, value: f32) -> Self {
        Keyframe {
            frame,
            value,
            interpolation: Interpolation::default(),
            left: [frame - 1.0, value],
            right: [frame + 1.0, value],
            left_type: Handle::default(),
            right_type: Handle::default(),
            selected: false,
        }
    }

    /// Moves the key with its handles.
    pub fn shift(&mut self, frames: f32, value: f32) {
        self.frame += frames;
        self.value += value;
        for h in [&mut self.left, &mut self.right] {
            h[0] += frames;
            h[1] += value;
        }
    }
}

/// One number over the frames.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FCurve {
    /// In frame order.
    pub keys: Vec<Keyframe>,
}

/// Frames this near are the same frame.
const SAME_FRAME: f32 = 1e-3;

impl FCurve {
    /// The key at a frame.
    pub fn key_at(&self, frame: f32) -> Option<usize> {
        self.keys.iter().position(|k| (k.frame - frame).abs() < SAME_FRAME)
    }

    /// A key at a frame with a value: the one there changed, or a new one
    /// (interpolated and handled as its neighbour before it, as Blender's
    /// insert does). Returns its index.
    pub fn insert(&mut self, frame: f32, value: f32) -> usize {
        let i = match self.key_at(frame) {
            Some(i) => {
                let by = value - self.keys[i].value;
                self.keys[i].shift(0.0, by);
                i
            }
            None => {
                let i = self.keys.partition_point(|k| k.frame < frame);
                let mut key = Keyframe::new(frame, value);
                if let Some(before) = i.checked_sub(1).and_then(|b| self.keys.get(b)) {
                    key.interpolation = before.interpolation;
                }
                self.keys.insert(i, key);
                i
            }
        };
        self.recalc();
        i
    }

    /// Puts the keys back in frame order (after moving some) and the handles
    /// where their types say. Two keys on a frame: the selected one stays
    /// (the one moved there), as when keys are dropped on others in Blender.
    pub fn tidy(&mut self) {
        self.keys.sort_by(|a, b| a.frame.total_cmp(&b.frame));
        let mut i = 0;
        while i + 1 < self.keys.len() {
            if (self.keys[i + 1].frame - self.keys[i].frame).abs() < SAME_FRAME {
                let gone = if self.keys[i].selected && !self.keys[i + 1].selected { i + 1 } else { i };
                self.keys.remove(gone);
            } else {
                i += 1;
            }
        }
        self.recalc();
    }

    /// Works the handles out again (`BKE_fcurve_handles_recalc`).
    pub fn recalc(&mut self) {
        let n = self.keys.len();
        for i in 0..n {
            let prev = i.checked_sub(1).map(|p| [self.keys[p].frame, self.keys[p].value]);
            let next = self.keys.get(i + 1).map(|k| [k.frame, k.value]);
            let key = &mut self.keys[i];
            // A handle is never on the wrong side of its key.
            key.left[0] = key.left[0].min(key.frame);
            key.right[0] = key.right[0].max(key.frame);
            calc_handles(key, prev, next);
            // The first and last keys ease in and out: flat automatic handles.
            if (i == 0 || i == n - 1) && key.left_type.auto() && key.right_type.auto() {
                key.left[1] = key.value;
                key.right[1] = key.value;
            }
        }
    }

    /// The value at a frame. `None` without keys.
    pub fn evaluate(&self, frame: f32) -> Option<f32> {
        let (first, last) = (self.keys.first()?, self.keys.last()?);
        if frame <= first.frame {
            return Some(first.value);
        }
        if frame >= last.frame {
            return Some(last.value);
        }
        let b = self.keys.partition_point(|k| k.frame <= frame);
        let (a, b) = (&self.keys[b - 1], &self.keys[b]);
        if (frame - a.frame).abs() < SAME_FRAME {
            return Some(a.value);
        }
        Some(match a.interpolation {
            Interpolation::Constant => a.value,
            Interpolation::Linear => {
                let t = (frame - a.frame) / (b.frame - a.frame);
                a.value + (b.value - a.value) * t
            }
            Interpolation::Bezier => bezier(a, b, frame),
        })
    }
}

/// `BKE_nurb_handle_calc` for an F-curve's key: the automatic and vector
/// handles from the neighbouring keys (the missing one mirrored).
fn calc_handles(key: &mut Keyframe, prev: Option<[f32; 2]>, next: Option<[f32; 2]>) {
    let p2 = [key.frame, key.value];
    let (p1, p3) = match (prev, next) {
        (Some(p), Some(n)) => (p, n),
        (Some(p), None) => (p, [2.0 * p2[0] - p[0], 2.0 * p2[1] - p[1]]),
        (None, Some(n)) => ([2.0 * p2[0] - n[0], 2.0 * p2[1] - n[1]], n),
        (None, None) => ([p2[0] - 1.0, p2[1]], [p2[0] + 1.0, p2[1]]),
    };
    let da = [p2[0] - p1[0], p2[1] - p1[1]];
    let db = [p3[0] - p2[0], p3[1] - p2[1]];
    let mut len_a = if da[0] == 0.0 { 1.0 } else { da[0] };
    let mut len_b = if db[0] == 0.0 { 1.0 } else { db[0] };

    if key.left_type.auto() || key.right_type.auto() {
        let t = [db[0] / len_b + da[0] / len_a, db[1] / len_b + da[1] / len_a];
        let len = t[0] * 2.5614;
        if len != 0.0 {
            len_a = len_a.min(5.0 * len_b);
            len_b = len_b.min(5.0 * len_a);
            let (mut left_hit, mut right_hit) = (false, false);
            let both = prev.is_some() && next.is_some();
            if key.left_type.auto() {
                let l = len_a / len;
                key.left = [p2[0] - t[0] * l, p2[1] - t[1] * l];
                if key.left_type == Handle::AutoClamped && both {
                    let (y1, y2) = (p1[1] - p2[1], p3[1] - p2[1]);
                    if (y1 <= 0.0 && y2 <= 0.0) || (y1 >= 0.0 && y2 >= 0.0) {
                        key.left[1] = p2[1];
                    } else if (y1 <= 0.0 && p1[1] > key.left[1]) || (y1 > 0.0 && p1[1] < key.left[1]) {
                        key.left[1] = p1[1];
                        left_hit = true;
                    }
                }
            }
            if key.right_type.auto() {
                let l = len_b / len;
                key.right = [p2[0] + t[0] * l, p2[1] + t[1] * l];
                if key.right_type == Handle::AutoClamped && both {
                    let (y1, y2) = (p1[1] - p2[1], p3[1] - p2[1]);
                    if (y1 <= 0.0 && y2 <= 0.0) || (y1 >= 0.0 && y2 >= 0.0) {
                        key.right[1] = p2[1];
                    } else if (y1 <= 0.0 && p3[1] < key.right[1]) || (y1 > 0.0 && p3[1] > key.right[1]) {
                        key.right[1] = p3[1];
                        right_hit = true;
                    }
                }
            }
            // One side stopped at its neighbour: the other keeps in line with it.
            if (left_hit || right_hit) && key.left_type.auto() && key.right_type.auto() {
                let (h1, h2) = (p2[0] - key.left[0], p2[0] - key.right[0]);
                if left_hit && h1 != 0.0 {
                    key.right[1] = p2[1] + (p2[1] - key.left[1]) * (h2 / h1).abs();
                } else if h2 != 0.0 {
                    key.left[1] = p2[1] + (p2[1] - key.right[1]) * (h1 / h2).abs();
                }
            }
        }
    }
    if key.left_type == Handle::Vector {
        key.left = [p2[0] - da[0] / 3.0, p2[1] - da[1] / 3.0];
    }
    if key.right_type == Handle::Vector {
        key.right = [p2[0] + db[0] / 3.0, p2[1] + db[1] / 3.0];
    }
}

/// The value at `frame` of the Bézier between two keys
/// (`fcurve_eval_keyframes_interpolate`): the handles shortened so that
/// together they aren't longer than the gap (`BKE_fcurve_correct_bezpart`),
/// then the curve's point whose frame is the one asked.
fn bezier(a: &Keyframe, b: &Keyframe, frame: f32) -> f32 {
    let (v1, mut v2, mut v3, v4) = ([a.frame, a.value], a.right, b.left, [b.frame, b.value]);
    let (h1, h2) = ([v1[0] - v2[0], v1[1] - v2[1]], [v4[0] - v3[0], v4[1] - v3[1]]);
    let len = v4[0] - v1[0];
    let (len1, len2) = (h1[0].abs(), h2[0].abs());
    if len1 + len2 > len && len1 + len2 > 0.0 {
        let fac = len / (len1 + len2);
        v2 = [v1[0] - fac * h1[0], v1[1] - fac * h1[1]];
        v3 = [v4[0] - fac * h2[0], v4[1] - fac * h2[1]];
    }
    let at = |t: f32, i: usize| {
        let u = 1.0 - t;
        u * u * u * v1[i] + 3.0 * u * u * t * v2[i] + 3.0 * u * t * t * v3[i] + t * t * t * v4[i]
    };
    // The frame only grows along the curve now: halving finds it.
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..32 {
        let mid = 0.5 * (lo + hi);
        if at(mid, 0) < frame {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    at(0.5 * (lo + hi), 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(keys: &[(f32, f32)]) -> FCurve {
        let mut c = FCurve::default();
        for (f, v) in keys {
            c.insert(*f, *v);
        }
        c
    }

    #[test]
    fn holds_before_and_after_its_keys() {
        let c = curve(&[(10.0, 1.0), (20.0, 3.0)]);
        assert_eq!(c.evaluate(0.0), Some(1.0));
        assert_eq!(c.evaluate(10.0), Some(1.0));
        assert_eq!(c.evaluate(20.0), Some(3.0));
        assert_eq!(c.evaluate(99.0), Some(3.0));
        assert_eq!(FCurve::default().evaluate(1.0), None);
    }

    #[test]
    fn two_keys_ease_in_and_out() {
        // Blender's default between two keys: flat handles, an S.
        let c = curve(&[(0.0, 0.0), (10.0, 1.0)]);
        assert_eq!(c.keys[0].right[1], 0.0);
        assert_eq!(c.keys[1].left[1], 1.0);
        let mid = c.evaluate(5.0).unwrap();
        assert!((mid - 0.5).abs() < 1e-4, "{mid}");
        let early = c.evaluate(2.0).unwrap();
        assert!(early < 0.2 && early > 0.0, "{early}");
        // It only goes up.
        let mut last = 0.0;
        for f in 0..=10 {
            let v = c.evaluate(f as f32).unwrap();
            assert!(v >= last - 1e-6);
            last = v;
        }
    }

    #[test]
    fn linear_and_constant() {
        let mut c = curve(&[(0.0, 0.0), (10.0, 1.0)]);
        c.keys[0].interpolation = Interpolation::Linear;
        assert!((c.evaluate(2.5).unwrap() - 0.25).abs() < 1e-6);
        c.keys[0].interpolation = Interpolation::Constant;
        assert_eq!(c.evaluate(9.9), Some(0.0));
        assert_eq!(c.evaluate(10.0), Some(1.0));
    }

    #[test]
    fn clamped_handles_dont_go_past_the_keys() {
        // A peak in the middle: its handles are flat, so the curve never goes above it.
        let c = curve(&[(0.0, 0.0), (10.0, 1.0), (20.0, 0.0)]);
        let peak = c.keys[1];
        assert_eq!((peak.left[1], peak.right[1]), (1.0, 1.0));
        for f in 0..=20 {
            let v = c.evaluate(f as f32).unwrap();
            assert!((-1e-5..=1.0 + 1e-5).contains(&v), "{v} at {f}");
        }
        // On a slope the handles lean with it, and are in a line.
        let c = curve(&[(0.0, 0.0), (10.0, 1.0), (20.0, 2.0)]);
        let k = c.keys[1];
        assert!(k.left[1] < 1.0 && k.right[1] > 1.0);
        let (l, r) = ((k.value - k.left[1]) / (k.frame - k.left[0]), (k.right[1] - k.value) / (k.right[0] - k.frame));
        assert!((l - r).abs() < 1e-5 && (l - 0.1).abs() < 1e-5, "{l} {r}");
    }

    #[test]
    fn vector_handles_point_at_the_neighbours() {
        let mut c = curve(&[(0.0, 0.0), (9.0, 3.0), (18.0, 0.0)]);
        (c.keys[1].left_type, c.keys[1].right_type) = (Handle::Vector, Handle::Vector);
        c.recalc();
        assert_eq!(c.keys[1].left, [6.0, 2.0]);
        assert_eq!(c.keys[1].right, [12.0, 2.0]);
    }

    #[test]
    fn inserting_changes_the_key_there_and_keeps_order() {
        let mut c = curve(&[(10.0, 1.0), (0.0, 0.0), (5.0, 2.0)]);
        assert_eq!(c.keys.iter().map(|k| k.frame).collect::<Vec<_>>(), [0.0, 5.0, 10.0]);
        assert_eq!(c.insert(5.0, 4.0), 1);
        assert_eq!(c.keys.len(), 3);
        assert_eq!(c.keys[1].value, 4.0);
        // A new key takes the interpolation of the one before it.
        c.keys[0].interpolation = Interpolation::Linear;
        let i = c.insert(2.0, 1.0);
        assert_eq!(c.keys[i].interpolation, Interpolation::Linear);
    }

    #[test]
    fn tidying_after_a_move() {
        let mut c = curve(&[(0.0, 0.0), (5.0, 1.0), (10.0, 2.0)]);
        // The first key moved onto the second: it stays, the other goes.
        c.keys[0].selected = true;
        c.keys[0].shift(5.0, 0.0);
        c.tidy();
        assert_eq!(c.keys.iter().map(|k| (k.frame, k.value)).collect::<Vec<_>>(), [(5.0, 0.0), (10.0, 2.0)]);
        // And past the last: the order follows.
        c.keys[0].shift(10.0, 0.0);
        c.tidy();
        assert_eq!(c.keys.iter().map(|k| k.frame).collect::<Vec<_>>(), [10.0, 15.0]);
    }

    #[test]
    fn long_handles_are_shortened_to_fit() {
        let mut c = curve(&[(0.0, 0.0), (10.0, 1.0)]);
        (c.keys[0].right_type, c.keys[1].left_type) = (Handle::Free, Handle::Free);
        c.keys[0].right = [30.0, 0.0];
        c.keys[1].left = [-20.0, 1.0];
        // Still one value a frame, between the keys' values.
        let mut last = 0.0;
        for f in 0..=10 {
            let v = c.evaluate(f as f32).unwrap();
            assert!(v >= last - 1e-5 && v <= 1.0 + 1e-5, "{v} at {f}");
            last = v;
        }
    }
}
