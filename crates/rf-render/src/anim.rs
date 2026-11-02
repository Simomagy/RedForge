//! Skeletons posed by clips: the bones' local transforms from a clip at a time,
//! composed into model-space transforms and the matrices that skin a model.
//! From the C#'s `Rsc8ClipAnimator`, `Skeleton.UpdateBoneTransforms` and the
//! skeleton parts of `RDR2Ped` (`AttachSkeleton`).
//!
//! Conventions: glam's column vectors, so a bone's model-space transform is
//! `parent * local` and its skinning matrix `model * bind_inverse` (the C#'s
//! row-vector `BindTransformInv * AnimTransform`).
//!
//! Differences from the C#, on purpose:
//! - A bone attached from another skeleton takes its parent by tag in the
//!   combined skeleton. The C# keeps the parent object of the skeleton it came
//!   from, which the animation doesn't move when that bone was a duplicate.
//! - Each evaluation starts from the rest pose, so a time can be shown on its
//!   own (scrubbing); the C# resets only when the clip changes.

use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};
use rf_formats::formats::ycd::{Anim, Clip, ClipKind};
use rf_formats::formats::ydr::{DrawableBase, Skeleton};

/// A bone of a [`Pose`].
#[derive(Clone, Debug)]
pub struct PoseBone {
    pub tag: u16,
    pub name: Option<String>,
    pub parent: Option<usize>,
    pub rest_rotation: Quat,
    pub rest_translation: Vec3,
    pub rest_scale: Vec3,
    /// The local transform being shown (`AnimRotation`...), relative to the parent.
    pub rotation: Quat,
    pub translation: Vec3,
    pub scale: Vec3,
    /// The inverse of the bone's model-space transform in the bind pose.
    pub bind_inverse: Mat4,
}

/// A skeleton's bones and their current transforms.
#[derive(Clone, Debug, Default)]
pub struct Pose {
    bones: Vec<PoseBone>,
    by_tag: HashMap<u16, usize>,
    /// Bone indices with parents before their children.
    order: Vec<usize>,
    /// Model-space transforms (`AnimTransform`).
    model: Vec<Mat4>,
    /// Skinning matrices (`SkinTransform`).
    skin: Vec<Mat4>,
}

/// A skeleton's bind pose inverse as glam reads it: the C#'s row-vector matrix,
/// row by row, is glam's column-vector one column by column. The game stores
/// other values in the projective column, which the C# leaves out (`M44 = 1`
/// and the 3x4 bone matrices).
fn bind_inverse(m: &[f32; 16]) -> Mat4 {
    let mut b = Mat4::from_cols_array(m);
    b.x_axis.w = 0.0;
    b.y_axis.w = 0.0;
    b.z_axis.w = 0.0;
    b.w_axis.w = 1.0;
    b
}

impl Pose {
    /// A pose of one skeleton.
    pub fn from_skeleton(skeleton: &Skeleton) -> Self {
        let mut p = Self::default();
        p.attach(skeleton);
        p
    }

    /// `AttachSkeleton`: adds a skeleton's bones that aren't here yet (by tag).
    /// A skeleton with a parent bone tag hangs off that bone: its roots become
    /// the bone's children and its bind poses are moved into the bone's space.
    pub fn attach(&mut self, skeleton: &Skeleton) {
        let Some(bones) = skeleton.bones.as_deref() else { return };
        let parents: Vec<Option<usize>> = (0..bones.len())
            .map(|i| {
                let p = match skeleton.mapping_block.as_ref().and_then(|m| m.indices.as_ref()) {
                    Some(ix) => ix.get(i).copied().unwrap_or(-1),
                    None => bones[i].parent_index,
                };
                usize::try_from(p).ok().filter(|&p| p < bones.len())
            })
            .collect();
        let local = |i: usize| {
            let b = &bones[i];
            let [x, y, z, w] = b.rotation;
            Mat4::from_scale_rotation_translation(b.scale.into(), Quat::from_xyzw(x, y, z, w), b.position.into())
        };
        // Model-space rest transforms, for bones without a stored bind pose.
        let mut rest: Vec<Option<Mat4>> = vec![None; bones.len()];
        fn rest_of(
            i: usize,
            parents: &[Option<usize>],
            rest: &mut [Option<Mat4>],
            local: &dyn Fn(usize) -> Mat4,
        ) -> Mat4 {
            if let Some(m) = rest[i] {
                return m;
            }
            rest[i] = Some(Mat4::IDENTITY); // (a parent loop ends here)
            let m = match parents[i] {
                Some(p) => rest_of(p, parents, rest, local) * local(i),
                None => local(i),
            };
            rest[i] = Some(m);
            m
        }
        let attach_to = (skeleton.parent_bone_tag != 0).then(|| self.bone_index(skeleton.parent_bone_tag)).flatten();
        let first = self.bones.len();
        let mut parent_tags = Vec::new();
        for (i, b) in bones.iter().enumerate() {
            if self.by_tag.contains_key(&b.tag) {
                continue;
            }
            let mut inv = match skeleton.bind_pose_inv.as_ref().and_then(|p| p.get(i)) {
                Some(m) => bind_inverse(m),
                None => rest_of(i, &parents, &mut rest, &local).inverse(),
            };
            if let Some(p) = attach_to {
                inv *= self.bones[p].bind_inverse;
            }
            let [x, y, z, w] = b.rotation;
            let rotation = Quat::from_xyzw(x, y, z, w);
            self.by_tag.insert(b.tag, self.bones.len());
            parent_tags.push(parents[i].map(|p| bones[p].tag));
            self.bones.push(PoseBone {
                tag: b.tag,
                name: b.name.clone(),
                parent: None,
                rest_rotation: rotation,
                rest_translation: b.position.into(),
                rest_scale: b.scale.into(),
                rotation,
                translation: b.position.into(),
                scale: b.scale.into(),
                bind_inverse: inv,
            });
        }
        for (i, tag) in parent_tags.into_iter().enumerate() {
            let parent = match tag {
                Some(t) => self.by_tag.get(&t).copied(),
                None => attach_to,
            };
            self.bones[first + i].parent = parent.filter(|&p| p != first + i);
        }
        self.build_order();
        self.update();
    }

    /// `AttachDrawable`'s skeletons: a drawable's own, then its extra ones.
    pub fn attach_drawable(&mut self, d: &DrawableBase) {
        for s in d.skeleton.iter().chain(d.extra_skeletons.iter().flatten().flatten()) {
            self.attach(s);
        }
    }

    /// Parents first, so one pass computes the model-space transforms.
    fn build_order(&mut self) {
        let n = self.bones.len();
        let mut state = vec![0u8; n]; // 0 new, 1 being placed, 2 placed
        self.order.clear();
        for i in 0..n {
            let mut chain = Vec::new();
            let mut at = Some(i);
            while let Some(b) = at {
                if state[b] != 0 {
                    break;
                }
                state[b] = 1;
                chain.push(b);
                at = self.bones[b].parent;
            }
            for &b in chain.iter().rev() {
                state[b] = 2;
                self.order.push(b);
            }
        }
    }

    pub fn bones(&self) -> &[PoseBone] {
        &self.bones
    }

    pub fn bone_index(&self, tag: u16) -> Option<usize> {
        self.by_tag.get(&tag).copied()
    }

    /// Puts every bone back in its rest pose (`ResetBoneTransforms`, without
    /// [`Pose::update`]).
    pub fn reset(&mut self) {
        for b in &mut self.bones {
            (b.rotation, b.translation, b.scale) = (b.rest_rotation, b.rest_translation, b.rest_scale);
        }
    }

    /// `UpdateBoneTransforms`: the model-space and skinning matrices of the
    /// bones' current local transforms.
    pub fn update(&mut self) {
        let n = self.bones.len();
        self.model.resize(n, Mat4::IDENTITY);
        self.skin.resize(n, Mat4::IDENTITY);
        for &i in &self.order {
            let b = &self.bones[i];
            let local = Mat4::from_scale_rotation_translation(b.scale, b.rotation, b.translation);
            let m = match b.parent {
                Some(p) => self.model[p] * local,
                None => local,
            };
            self.model[i] = m;
            self.skin[i] = m * b.bind_inverse;
        }
    }

    /// A bone's model-space transform (`AnimTransform`).
    pub fn bone_transform(&self, tag: u16) -> Option<Mat4> {
        self.model.get(self.bone_index(tag)?).copied()
    }

    /// The skinning matrix of each bone of a rig, as 3 rows of a 3x4 matrix (the
    /// shader's layout). A tag the skeleton hasn't got gets its first bone's, as
    /// the C# does (`boneinds[i] = 0`).
    pub fn skin_rows<'a>(&'a self, tags: &'a [u16]) -> impl Iterator<Item = [[f32; 4]; 3]> + 'a {
        tags.iter().map(|t| {
            let m =
                self.bone_index(*t).or((!self.skin.is_empty()).then_some(0)).map_or(Mat4::IDENTITY, |i| self.skin[i]);
            let r = m.transpose();
            [r.x_axis.to_array(), r.y_axis.to_array(), r.z_axis.to_array()]
        })
    }

    /// The bounds of the bones' positions (`GetAnimatedBounds`).
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut it = self.model.iter().map(|m| m.w_axis.truncate());
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p))))
    }

    /// `Rsc8ClipAnimator.Update`: poses the bones as a clip has them at a time
    /// (looping, or held at the end), from the rest pose, then
    /// [`Pose::update`]s. `anim` finds the animations the clip plays by hash
    /// (in [`rf_formats::formats::ycd::ClipDictionary::anims`]).
    ///
    /// Only the bones' own tracks are shown (position and rotation); root
    /// motion, scale, cameras and facial values aren't: the model plays in place.
    pub fn apply_clip<'a>(&mut self, clip: &Clip, anim: impl Fn(u32) -> Option<&'a Anim>, time: f32, looping: bool) {
        self.reset();
        let duration = clip.duration();
        if duration > 0.0 {
            let mut time = if looping { time.rem_euclid(duration) } else { time.clamp(0.0, duration) };
            match &clip.kind {
                ClipKind::Single(s) => {
                    if let Some(a) = anim(s.animation_hash) {
                        self.apply_anim(a, s.start_time, s.speed, time);
                    }
                }
                ClipKind::Multi(m) => {
                    // The parts play one after the other, each with its layers.
                    let parts = m.parts.as_deref().unwrap_or(&[]);
                    for (i, part) in parts.iter().enumerate() {
                        if time > part.duration && i + 1 < parts.len() {
                            time -= part.duration;
                            continue;
                        }
                        for layer in part.clips.iter().flatten() {
                            if let Some(a) = anim(layer.animation_hash) {
                                self.apply_anim(a, layer.start_time, layer.speed, time);
                            }
                        }
                        break;
                    }
                }
            }
        }
        self.update();
    }

    /// `Apply`: one animation's bone tracks at a time of the clip playing it.
    fn apply_anim(&mut self, anim: &Anim, start: f32, speed: f32, time: f32) {
        let Some(ids) = anim.bone_ids.as_deref() else { return };
        if anim.duration <= 0.0 || anim.sequence_size == 0 {
            return;
        }
        let frame = anim.frame_position((start + time * speed).clamp(0.0, anim.duration));
        for (i, id) in ids.iter().enumerate() {
            // 0 position (a vector), 1 rotation (a quaternion).
            if id.track_id > 1 || id.track_id != id.type_id {
                continue;
            }
            let Some(&b) = self.by_tag.get(&id.bone_id) else { continue };
            let [x, y, z, w] = anim.evaluate(&frame, i);
            let bone = &mut self.bones[b];
            if id.track_id == 0 {
                bone.translation = Vec3::new(x, y, z);
            } else if [x, y, z, w] != [0.0; 4] {
                bone.rotation = Quat::from_xyzw(x, y, z, w).normalize();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rf_formats::formats::ydr::Bone;

    use super::*;

    fn bone(tag: u16, parent: i16, position: [f32; 3]) -> Bone {
        Bone { tag, parent_index: parent, position, ..Default::default() }
    }

    #[test]
    fn rest_pose_skins_to_identity_and_attached_skeletons_follow_their_bone() {
        let base =
            Skeleton { bones: Some(vec![bone(0, -1, [0.0; 3]), bone(10, 0, [0.0, 0.0, 1.0])]), ..Default::default() };
        let mut pose = Pose::from_skeleton(&base);
        // An attached skeleton: its root under bone 10, with a bone already there.
        let extra = Skeleton {
            bones: Some(vec![bone(20, -1, [1.0, 0.0, 0.0]), bone(10, -1, [5.0, 5.0, 5.0])]),
            parent_bone_tag: 10,
            ..Default::default()
        };
        pose.attach(&extra);
        assert_eq!(pose.bones().len(), 3);
        assert_eq!(pose.bones()[2].parent, Some(1));
        let at = |p: &Pose, tag| p.bone_transform(tag).unwrap().w_axis.truncate();
        assert!(at(&pose, 20).abs_diff_eq(Vec3::new(1.0, 0.0, 1.0), 1e-6));
        for rows in pose.skin_rows(&[0, 10, 20, 99]) {
            let m = Mat4::from_cols_array_2d(&[rows[0], rows[1], rows[2], [0.0, 0.0, 0.0, 1.0]]).transpose();
            assert!(m.abs_diff_eq(Mat4::IDENTITY, 1e-6), "{m}");
        }

        // Turning bone 10 a quarter round Z carries bone 20 with it.
        pose.bones[1].rotation = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        pose.update();
        assert!(at(&pose, 20).abs_diff_eq(Vec3::new(0.0, 1.0, 1.0), 1e-6));
        pose.reset();
        pose.update();
        assert!(at(&pose, 20).abs_diff_eq(Vec3::new(1.0, 0.0, 1.0), 1e-6));
    }
}
