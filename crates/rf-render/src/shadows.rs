//! The sun's cascaded shadow maps, fitted as the C# RedForge's
//! `CSMShadowView.Update` fits them: each cascade covers the view from the eye
//! to its distance, padded to the slice's diagonal so it keeps its size as the
//! camera turns, and snapped to whole texels so shadows don't shimmer.

use glam::camera::rh::{proj::directx, view};
use glam::{Mat4, Vec3, Vec3Swizzles};

use crate::CameraView;

pub const CASCADES: usize = 4;
/// Each cascade's shadow map is this many texels square.
pub const SHADOW_SIZE: u32 = 2048;
/// How far each cascade reaches from the eye (the C#'s `Render.CSM.CascadeInterval0-3`).
const INTERVALS: [f32; CASCADES] = [7.0, 20.0, 65.0, 500.0];
/// How much further towards the light than its slice a cascade reaches, for
/// what casts shadows into it from outside the view.
const TOWARDS_LIGHT: f32 = 300.0;
/// And past it, away from the light.
const AWAY_FROM_LIGHT: f32 = 20.0;

/// One cascade: its box in the light's view space and its projection.
#[derive(Clone, Copy, Debug)]
pub struct Cascade {
    /// World to the shadow map's clip space (depth 0 nearest the light).
    pub view_proj: Mat4,
    /// The box it covers in the light's view (looking down -Z).
    pub min: Vec3,
    pub max: Vec3,
    /// The size of a texel, in metres.
    pub texel: f32,
}

/// The cascades of one view for one light direction.
#[derive(Clone, Debug)]
pub struct Cascades {
    pub light_view: Mat4,
    pub cascades: [Cascade; CASCADES],
}

impl Cascades {
    /// Fits the cascades to a camera's view, for a light shining from
    /// `to_light` (towards it).
    pub fn new(camera: &CameraView, to_light: Vec3) -> Self {
        let travel = -to_light.normalize_or(Vec3::Z);
        let up = if travel.z.abs() > 0.99 { Vec3::Y } else { Vec3::Z };
        let light_view = view::look_to_mat4(Vec3::ZERO, travel, up);
        let view_proj = camera.projection * camera.view;
        let inv = view_proj.inverse();
        // The depth a point at a distance ahead of the eye gets.
        let depth_at = |d: f32| {
            let clip = camera.projection * glam::Vec4::new(0.0, 0.0, -d, 1.0);
            clip.z / clip.w
        };
        let corners = |depth: f32| {
            [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
                .map(|(x, y)| inv.project_point3(Vec3::new(x, y, depth)))
        };
        // Reversed Z: the near plane is at depth 1.
        let near = corners(1.0);
        let cascades = INTERVALS.map(|far| {
            let far = corners(depth_at(far));
            let points: Vec<Vec3> = near.iter().chain(&far).map(|p| light_view.transform_point3(*p)).collect();
            let mut min = points.iter().fold(Vec3::splat(f32::MAX), |a, p| a.min(*p));
            let mut max = points.iter().fold(Vec3::splat(f32::MIN), |a, p| a.max(*p));
            // Padded to the slice's diagonal (near corner to the far corner
            // opposite), so its size doesn't change as the view turns.
            let bound = (near[0] - far[2]).length();
            let pad = ((Vec3::splat(bound) - (max - min)) * 0.5).max(Vec3::ZERO);
            min -= pad.with_z(0.0);
            max += pad.with_z(0.0);
            let texel = bound / SHADOW_SIZE as f32;
            min = ((min / texel).floor() * texel).with_z(min.z);
            max = ((max / texel).floor() * texel).with_z(max.z);
            // Looking down -Z: towards the light is +Z.
            max.z += TOWARDS_LIGHT;
            min.z -= AWAY_FROM_LIGHT;
            let proj = directx::orthographic(min.x, max.x, min.y, max.y, -max.z, -min.z);
            Cascade { view_proj: proj * light_view, min, max, texel }
        });
        Self { light_view, cascades }
    }

    /// Whether something with this bounding sphere should be drawn into a
    /// cascade: it's in the cascade's box and big enough to show in it (the
    /// C#'s `MinEntitySize`, larger in the further cascades; it grows with the
    /// cascade's square there, which leaves houses past 65 m without shadows).
    pub fn casts(&self, cascade: usize, centre: Vec3, radius: f32) -> bool {
        let c = &self.cascades[cascade];
        let min_radius = c.texel * 4.0 * cascade.max(1) as f32;
        if radius < min_radius {
            return false;
        }
        let p = self.light_view.transform_point3(centre);
        let (lo, hi) = (c.min - radius, c.max + radius);
        p.xy().cmpge(lo.xy()).all() && p.xy().cmple(hi.xy()).all() && p.z >= lo.z && p.z <= hi.z
    }

    /// How far each cascade reaches from the eye.
    pub fn distances() -> [f32; CASCADES] {
        INTERVALS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FlyCamera;

    #[test]
    fn cascades_cover_their_slices() {
        let cam = FlyCamera { position: Vec3::new(100.0, 200.0, 50.0), ..Default::default() };
        let view = cam.camera_view(16.0 / 9.0);
        let c = Cascades::new(&view, Vec3::new(0.3, -0.4, 0.8).normalize());
        // A point 5 m ahead is in the first cascade's map; 300 m ahead only in the last.
        let ahead = |d: f32| cam.position + cam.forward() * d;
        let inside = |i: usize, p: Vec3| {
            let q = c.cascades[i].view_proj.project_point3(p);
            q.x.abs() < 1.0 && q.y.abs() < 1.0 && (0.0..1.0).contains(&q.z)
        };
        assert!(inside(0, ahead(5.0)));
        assert!(!inside(0, ahead(300.0)));
        assert!(inside(3, ahead(300.0)));
        // Something between the light and the view casts into it.
        assert!(c.casts(3, ahead(100.0) + Vec3::new(0.3, -0.4, 0.8).normalize() * 150.0, 8.0));
        // Small things don't count in the far cascades.
        assert!(c.casts(0, ahead(3.0), 0.1));
        assert!(!c.casts(3, ahead(100.0), 0.1));
    }
}
