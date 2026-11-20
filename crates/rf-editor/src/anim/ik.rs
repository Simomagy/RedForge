//! Reaching: where two bones go for the end of the second to be at a point (an
//! arm for its hand, a leg for its foot).
//!
//! Everything here is in the model's space: where the joints are and how the
//! bones are turned in the model, not relative to their parents.
//!
//! The joint in the middle (elbow, knee) opens or closes in the plane it's
//! bent in now, so the limb keeps facing the way it did; then the whole limb
//! swings from its top to put the end on the point. A point too far is
//! reached for with the limb almost straight; one too near, with it folded as
//! far as its lengths let it.

use glam::{Quat, Vec3};

/// How straight a limb gets, as a share of its length: a fully straight one
/// has no plane to bend back in.
const STRAIGHTEST: f32 = 0.9995;

/// Two bones reaching for a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reach {
    /// The first bone's new rotation in the model (upper arm, thigh).
    pub upper: Quat,
    /// The second bone's (forearm, calf).
    pub lower: Quat,
    /// Where the middle joint and the end are then.
    pub middle: Vec3,
    pub end: Vec3,
}

/// Where a limb goes for its end to be at `target`.
///
/// `top`, `middle` and `end` are the joints now (shoulder, elbow, wrist);
/// `upper` and `lower` how the two bones are turned now, in the model. `None`
/// when a bone has no length.
pub fn reach(top: Vec3, middle: Vec3, end: Vec3, upper: Quat, lower: Quat, target: Vec3) -> Option<Reach> {
    let (first, second) = (middle - top, end - middle);
    let (l1, l2) = (first.length(), second.length());
    if l1 < 1e-5 || l2 < 1e-5 {
        return None;
    }
    // How far the end can be from the top.
    let (least, most) = ((l1 - l2).abs() / STRAIGHTEST, (l1 + l2) * STRAIGHTEST);
    let wanted = (target - top).length().clamp(least.min(most), most);

    // The middle joint's angle (between the two bones, at the joint) for the
    // end to be that far: the law of cosines.
    let angle = |far: f32| ((l1 * l1 + l2 * l2 - far * far) / (2.0 * l1 * l2)).clamp(-1.0, 1.0).acos();
    let now = (-first).angle_between(second);
    // The plane it bends in: the one it's bent in, or for a straight limb any
    // one through it (the one nearest the bone's own side).
    let mut axis = first.cross(second);
    if axis.length_squared() < 1e-10 {
        axis = first.cross(lower * Vec3::Y);
        if axis.length_squared() < 1e-10 {
            axis = first.cross(lower * Vec3::Z);
        }
    }
    let axis = axis.normalize();
    // Turning the second bone about `axis` by an angle closes the joint by it.
    let bend = Quat::from_axis_angle(axis, now - angle(wanted));
    let bent_end = middle + bend * second;

    // The whole limb, from its top, onto the point.
    let from = (bent_end - top).normalize_or_zero();
    let to = (target - top).normalize_or_zero();
    let swing = if from == Vec3::ZERO || to == Vec3::ZERO { Quat::IDENTITY } else { Quat::from_rotation_arc(from, to) };
    Some(Reach {
        upper: (swing * upper).normalize(),
        lower: (swing * bend * lower).normalize(),
        middle: top + swing * first,
        end: top + swing * (bent_end - top),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An arm along x, bent a quarter at the elbow towards y.
    fn arm() -> (Vec3, Vec3, Vec3) {
        (Vec3::ZERO, Vec3::new(0.3, 0.0, 0.0), Vec3::new(0.3, 0.25, 0.0))
    }

    fn check(target: Vec3, expect_at_target: bool) -> Reach {
        let (top, middle, end) = arm();
        let r = reach(top, middle, end, Quat::IDENTITY, Quat::IDENTITY, target).unwrap();
        // The bones keep their lengths.
        assert!(((r.middle - top).length() - 0.3).abs() < 1e-5);
        assert!(((r.end - r.middle).length() - 0.25).abs() < 1e-5);
        // The rotations given are the ones that put the joints there.
        assert!((r.upper * (middle - top)).abs_diff_eq(r.middle - top, 1e-5));
        assert!((r.lower * (end - middle)).abs_diff_eq(r.end - r.middle, 1e-5));
        if expect_at_target {
            assert!(r.end.abs_diff_eq(target, 1e-4), "{:?} for {target:?}", r.end);
        }
        r
    }

    #[test]
    fn reaches_points_within_its_length() {
        for target in [
            Vec3::new(0.4, 0.1, 0.0),
            Vec3::new(0.2, 0.2, 0.3),
            Vec3::new(-0.3, 0.1, -0.2),
            Vec3::new(0.0, 0.0, 0.5),
            Vec3::new(0.1, -0.2, 0.0),
        ] {
            check(target, true);
        }
    }

    #[test]
    fn stays_where_it_is_for_where_it_is() {
        let (_, middle, end) = arm();
        let r = check(end, true);
        assert!(r.middle.abs_diff_eq(middle, 1e-4));
        assert!(r.upper.abs_diff_eq(Quat::IDENTITY, 1e-4) && r.lower.abs_diff_eq(Quat::IDENTITY, 1e-4));
    }

    #[test]
    fn keeps_its_bending_plane() {
        // A point in the plane the arm is bent in: the elbow stays in it.
        let r = check(Vec3::new(0.2, 0.3, 0.0), true);
        assert!(r.middle.z.abs() < 1e-5);
    }

    #[test]
    fn too_far_is_reached_for_almost_straight() {
        let target = Vec3::new(2.0, 1.0, 0.5);
        let r = check(target, false);
        let towards = target.normalize();
        assert!(r.end.normalize().abs_diff_eq(towards, 1e-4));
        assert!((r.end.length() - 0.55 * STRAIGHTEST).abs() < 1e-4);
    }

    #[test]
    fn too_near_folds_as_far_as_it_can() {
        let r = check(Vec3::new(0.01, 0.0, 0.0), false);
        assert!((r.end.length() - 0.05 / STRAIGHTEST).abs() < 1e-3);
    }

    #[test]
    fn a_straight_limb_bends() {
        let (top, middle, end) = (Vec3::ZERO, Vec3::new(0.3, 0.0, 0.0), Vec3::new(0.55, 0.0, 0.0));
        let target = Vec3::new(0.4, 0.0, 0.0);
        let r = reach(top, middle, end, Quat::IDENTITY, Quat::IDENTITY, target).unwrap();
        assert!(r.end.abs_diff_eq(target, 1e-4), "{:?}", r.end);
        assert!((r.middle.length() - 0.3).abs() < 1e-5);
    }

    #[test]
    fn a_bone_without_length_doesnt_reach() {
        assert!(reach(Vec3::ZERO, Vec3::ZERO, Vec3::X, Quat::IDENTITY, Quat::IDENTITY, Vec3::Y).is_none());
    }
}
