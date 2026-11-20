//! The bones an animation is made on ([`Rig`]): a ped's skeleton as the game
//! has it, with each bone's place at rest.
//!
//! A ycd's values are a bone's rotation and position relative to its parent,
//! in the skeleton's own units and axes, so the rest pose read here is what a
//! key starts from: a bone keyed at its rest rotation doesn't move.

use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};
use rf_formats::formats::ydr::Skeleton;
use serde::Serialize;

use super::BoneKeys;

/// A bone of a [`Rig`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RigBone {
    /// The bone's tag: what a ycd's bone ids and [`BoneKeys::bone`] are.
    pub tag: u16,
    pub name: String,
    /// The parent's index in [`Rig::bones`]; parents come before their children.
    pub parent: Option<usize>,
    /// At rest, relative to the parent: a quaternion (x, y, z, w).
    pub rotation: [f32; 4],
    /// At rest, relative to the parent, in metres.
    pub position: [f32; 3],
}

/// A skeleton's bones, in the skeleton's order.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Rig {
    pub bones: Vec<RigBone>,
    #[serde(skip)]
    by_tag: HashMap<u16, usize>,
    #[serde(skip)]
    by_name: HashMap<String, usize>,
}

impl Rig {
    /// The bones of a model's skeleton (a ped's is its fragment's).
    pub fn from_skeleton(skeleton: &Skeleton) -> Self {
        let mut rig = Rig::default();
        for (i, b) in skeleton.bones.iter().flatten().enumerate() {
            // A parent that isn't before its child would be a file gone wrong:
            // the bone is then a root, and nothing later loops.
            let parent = usize::try_from(b.parent_index).ok().filter(|p| *p < i);
            let name = b.name.clone().unwrap_or_else(|| format!("bone_{}", b.tag));
            // Tags and names are found first come, as the game finds them.
            rig.by_tag.entry(b.tag).or_insert(i);
            rig.by_name.entry(name.to_lowercase()).or_insert(i);
            rig.bones.push(RigBone { tag: b.tag, name, parent, rotation: b.rotation, position: b.position });
        }
        rig
    }

    /// A bone by name, whatever the case (`SKEL_R_Hand`).
    pub fn bone(&self, name: &str) -> Option<&RigBone> {
        self.index(name).map(|i| &self.bones[i])
    }

    pub fn index(&self, name: &str) -> Option<usize> {
        self.by_name.get(&name.to_lowercase()).copied()
    }

    pub fn by_tag(&self, tag: u16) -> Option<&RigBone> {
        self.by_tag.get(&tag).map(|i| &self.bones[*i])
    }

    /// A bone with nothing keyed yet, on its rest pose: where posing it starts.
    pub fn keys_at_rest(&self, name: &str) -> Option<BoneKeys> {
        let b = self.bone(name)?;
        Some(BoneKeys::new(b.tag, &b.name, Quat::from_array(b.rotation), Vec3::from_array(b.position)))
    }

    /// Each bone's place in the model at rest (not relative to its parent),
    /// in the bones' order.
    pub fn model_transforms(&self) -> Vec<Mat4> {
        let mut out: Vec<Mat4> = Vec::with_capacity(self.bones.len());
        for b in &self.bones {
            let local =
                Mat4::from_rotation_translation(Quat::from_array(b.rotation).normalize(), Vec3::from_array(b.position));
            out.push(match b.parent {
                Some(p) => out[p] * local,
                None => local,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use rf_formats::formats::ydr::Bone;

    use super::*;

    fn bone(name: &str, tag: u16, parent: i16, rotation: [f32; 4], position: [f32; 3]) -> Bone {
        Bone { name: Some(name.into()), tag, parent_index: parent, rotation, position, ..Default::default() }
    }

    fn arm() -> Rig {
        let quarter = Quat::from_rotation_z(90f32.to_radians()).to_array();
        Rig::from_skeleton(&Skeleton {
            bones: Some(vec![
                bone("SKEL_ROOT", 0, -1, [0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0]),
                bone("SKEL_R_UpperArm", 46065, 0, quarter, [0.2, 0.0, 0.5]),
                bone("SKEL_R_Forearm", 54187, 1, [0.0, 0.0, 0.0, 1.0], [0.3, 0.0, 0.0]),
            ]),
            ..Default::default()
        })
    }

    #[test]
    fn bones_by_name_and_tag() {
        let rig = arm();
        assert_eq!(rig.bones.len(), 3);
        assert_eq!(rig.bone("skel_r_forearm").unwrap().tag, 54187);
        assert_eq!(rig.by_tag(46065).unwrap().name, "SKEL_R_UpperArm");
        assert_eq!(rig.bones[2].parent, Some(1));
        assert_eq!(rig.bones[0].parent, None);
        assert!(rig.bone("SKEL_L_Hand").is_none());
    }

    #[test]
    fn keys_at_rest_are_the_rest_pose() {
        let rig = arm();
        let keys = rig.keys_at_rest("SKEL_R_UpperArm").unwrap();
        assert_eq!(keys.bone, 46065);
        assert!(!keys.is_keyed() && keys.rotation(40.0).is_none());
        assert_eq!(keys.rest_rotation, rig.bones[1].rotation);
        // Keyed as it is at rest, it's a rotation of nothing.
        let mut keys = keys;
        keys.key_rotation(0.0, Quat::from_array(rig.bones[1].rotation));
        assert!((keys.value(crate::anim::Path::RotationQuaternion, 0, 0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn places_in_the_model() {
        // The upper arm is turned a quarter about z, so the forearm's 0.3 along
        // x in the arm is 0.3 along y in the model.
        let m = arm().model_transforms();
        let forearm = m[2].transform_point3(Vec3::ZERO);
        assert!(forearm.abs_diff_eq(Vec3::new(0.2, 0.3, 1.5), 1e-6), "{forearm:?}");
    }
}
