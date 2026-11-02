//! Where an entity is in the world, and how that's stored in its definition.
//!
//! As in the C# RedForge (`RDR2Entity`): a ymap `CEntityDef` stores the
//! inverse of its orientation, a `CMloInstanceDef` the orientation itself,
//! and scale is `(scaleXY, scaleXY, scaleZ)`.

use glam::{Mat4, Quat, Vec3};
use rf_formats::formats::databag::DataBag;

/// `CMloInstanceDef`'s class name hash.
pub const MLO_INSTANCE_DEF: u32 = 0x09CC_28BE;

/// An entity's position, orientation and scale in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub position: Vec3,
    pub orientation: Quat,
    /// X and Y are always the same (`scaleXY`).
    pub scale: Vec3,
}

impl Default for Placement {
    fn default() -> Self {
        Self { position: Vec3::ZERO, orientation: Quat::IDENTITY, scale: Vec3::ONE }
    }
}

impl Placement {
    /// From a `CEntityDef` or `CMloInstanceDef` of a ymap.
    pub fn of_entity(def: &DataBag) -> Self {
        let r = def.vec4("rotation");
        let stored = Quat::from_xyzw(r[0], r[1], r[2], r[3]);
        let stored = if stored.length_squared() > 0.0 { stored.normalize() } else { Quat::IDENTITY };
        let orientation = if is_mlo_instance(def) { stored } else { stored.inverse() };
        let (xy, z) = (def.f32("scaleXY"), def.f32("scaleZ"));
        Self { position: def.vec3("position").into(), orientation, scale: Vec3::new(xy, xy, z) }
    }

    /// Writes it into a `CEntityDef` or `CMloInstanceDef` of a ymap.
    pub fn write_entity(&self, def: &mut DataBag) {
        let q = self.orientation.normalize();
        let stored = if is_mlo_instance(def) { q } else { q.inverse() };
        def.set_vec3("position", self.position.into());
        def.set_vec4("rotation", stored.to_array());
        def.set_f32("scaleXY", self.scale.x);
        def.set_f32("scaleZ", self.scale.z);
    }

    /// Model to world.
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.orientation, self.position)
    }

    /// With X and Y scale made equal again after a change to one of them: the
    /// one that moved away from `before` wins.
    pub fn lock_scale_xy(mut self, before: &Placement) -> Self {
        let dx = (self.scale.x - before.scale.x).abs();
        let dy = (self.scale.y - before.scale.y).abs();
        let xy = if dy > dx { self.scale.y } else { self.scale.x };
        self.scale.x = xy;
        self.scale.y = xy;
        self
    }
}

pub fn is_mlo_instance(def: &DataBag) -> bool {
    def.class_name() == MLO_INSTANCE_DEF
}

#[cfg(test)]
mod tests {
    use rf_formats::formats::meta::Meta;

    use super::*;

    #[test]
    fn stored_rotation_is_inverted_for_entities_only() {
        let turn = Quat::from_rotation_z(0.7);
        for (class, stored) in [("CEntityDef", turn.inverse()), ("CMloInstanceDef", turn)] {
            let mut def = Meta::create(class).unwrap();
            let p =
                Placement { position: Vec3::new(1.0, 2.0, 3.0), orientation: turn, scale: Vec3::new(2.0, 2.0, 3.0) };
            p.write_entity(&mut def);
            let r = def.vec4("rotation");
            assert!(Quat::from_array(r).dot(stored).abs() > 0.99999, "{class}");
            let back = Placement::of_entity(&def);
            assert!(back.orientation.dot(turn).abs() > 0.99999);
            assert_eq!((back.position, back.scale), (p.position, p.scale));
        }
    }

    #[test]
    fn scale_xy_follows_the_axis_that_moved() {
        let before = Placement::default();
        let mut p = before;
        p.scale = Vec3::new(1.0, 3.0, 1.0);
        assert_eq!(p.lock_scale_xy(&before).scale, Vec3::new(3.0, 3.0, 1.0));
        p.scale = Vec3::new(2.0, 1.0, 1.5);
        assert_eq!(p.lock_scale_xy(&before).scale, Vec3::new(2.0, 2.0, 1.5));
    }
}
