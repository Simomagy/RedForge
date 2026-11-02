use glam::camera::rh::{proj::directx, view};
use glam::{Mat4, Vec2, Vec3, Vec4};

/// What the renderer needs of a camera.
#[derive(Clone, Copy, Debug)]
pub struct CameraView {
    pub view: Mat4,
    pub projection: Mat4,
    pub eye: Vec3,
}

impl CameraView {
    pub fn frustum(&self) -> Frustum {
        Frustum::from_matrix(self.projection * self.view)
    }

    /// The ray from the eye through a point of the view, given in normalised
    /// device coordinates (-1..1, y up).
    pub fn ray(&self, ndc: Vec2) -> Ray {
        let inv = (self.projection * self.view).inverse();
        // Reversed Z: depth 1 is the near plane.
        let near = inv.project_point3(ndc.extend(1.0));
        let mid = inv.project_point3(ndc.extend(0.5));
        Ray { origin: self.eye, dir: (mid - near).normalize_or(Vec3::Y) }
    }

    /// The world point at a depth (as the depth buffer holds it) behind a point
    /// of the view.
    pub fn unproject(&self, ndc: Vec2, depth: f32) -> Vec3 {
        (self.projection * self.view).inverse().project_point3(ndc.extend(depth))
    }

    /// Where a point shows in the view, in normalised device coordinates;
    /// `None` behind the eye.
    pub fn project(&self, p: Vec3) -> Option<Vec2> {
        let clip = self.projection * self.view * p.extend(1.0);
        (clip.w > 1e-6).then(|| Vec2::new(clip.x, clip.y) / clip.w)
    }
}

/// A half-line, for picking.
#[derive(Clone, Copy, Debug)]
pub struct Ray {
    pub origin: Vec3,
    /// Unit length.
    pub dir: Vec3,
}

impl Ray {
    pub fn at(&self, t: f32) -> Vec3 {
        self.origin + self.dir * t
    }

    /// The distance along the ray to a box, 0 from inside it.
    pub fn hit_box(&self, min: Vec3, max: Vec3) -> Option<f32> {
        let inv = self.dir.recip();
        let (a, b) = ((min - self.origin) * inv, (max - self.origin) * inv);
        let near = a.min(b).max_element().max(0.0);
        let far = a.max(b).min_element();
        (near <= far).then_some(near)
    }

    /// The distance along the ray to a triangle, either side (Möller–Trumbore).
    pub fn hit_triangle(&self, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
        let (e1, e2) = (b - a, c - a);
        let p = self.dir.cross(e2);
        let det = e1.dot(p);
        if det.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / det;
        let s = self.origin - a;
        let u = s.dot(p) * inv;
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let q = s.cross(e1);
        let v = self.dir.dot(q) * inv;
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        let t = e2.dot(q) * inv;
        (t > 0.0).then_some(t)
    }

    /// The distance along the ray to a plane through `point`; `None` when
    /// parallel or behind.
    pub fn hit_plane(&self, point: Vec3, normal: Vec3) -> Option<f32> {
        let d = self.dir.dot(normal);
        if d.abs() < 1e-8 {
            return None;
        }
        let t = (point - self.origin).dot(normal) / d;
        (t >= 0.0).then_some(t)
    }
}

/// The planes of a view (normals pointing in), for culling.
#[derive(Clone, Copy, Debug)]
pub struct Frustum {
    planes: [Vec4; 5],
}

impl Frustum {
    /// From a view-projection matrix with wgpu's 0..1 depth (reversed and
    /// infinite: there's no far plane).
    pub fn from_matrix(m: Mat4) -> Self {
        let r = |i| m.row(i);
        let planes = [r(3) + r(0), r(3) - r(0), r(3) + r(1), r(3) - r(1), r(3) - r(2)]
            .map(|p| p / p.truncate().length().max(1e-12));
        Self { planes }
    }

    pub fn sphere_visible(&self, centre: Vec3, radius: f32) -> bool {
        self.planes.iter().all(|p| p.truncate().dot(centre) + p.w >= -radius)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centre_ray_looks_forward() {
        let cam = FlyCamera { position: Vec3::new(1.0, 2.0, 3.0), yaw: 0.7, pitch: -0.3, ..Default::default() };
        let view = cam.camera_view(1.5);
        let ray = view.ray(Vec2::ZERO);
        assert!(ray.dir.distance(cam.forward()) < 1e-4, "{:?} vs {:?}", ray.dir, cam.forward());
        let p = ray.at(50.0);
        assert!(view.project(p).unwrap().length() < 1e-4);
        let off = view.ray(Vec2::new(0.5, -0.25));
        let back = view.project(off.at(10.0)).unwrap();
        assert!(back.distance(Vec2::new(0.5, -0.25)) < 1e-4);
    }

    #[test]
    fn ray_hits() {
        let r = Ray { origin: Vec3::ZERO, dir: Vec3::X };
        assert_eq!(r.hit_box(Vec3::new(2.0, -1.0, -1.0), Vec3::new(3.0, 1.0, 1.0)), Some(2.0));
        assert_eq!(r.hit_box(Vec3::splat(-1.0), Vec3::splat(1.0)), Some(0.0));
        assert_eq!(r.hit_box(Vec3::new(2.0, 1.5, -1.0), Vec3::new(3.0, 2.0, 1.0)), None);
        let t = r.hit_triangle(Vec3::new(5.0, -1.0, -1.0), Vec3::new(5.0, 1.0, -1.0), Vec3::new(5.0, 0.0, 1.0));
        assert!((t.unwrap() - 5.0).abs() < 1e-5);
        assert_eq!(r.hit_plane(Vec3::new(4.0, 0.0, 0.0), Vec3::X), Some(4.0));
    }
}

/// A free camera for flying around the map.
#[derive(Clone, Debug)]
pub struct FlyCamera {
    pub position: Vec3,
    /// Around Z, radians; 0 looks along +Y.
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
}

impl Default for FlyCamera {
    fn default() -> Self {
        Self { position: Vec3::new(0.0, 0.0, 150.0), yaw: 0.0, pitch: -0.2, fov_y: 60f32.to_radians() }
    }
}

impl FlyCamera {
    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(-sy * cp, cy * cp, sp)
    }

    pub fn right(&self) -> Vec3 {
        self.forward().cross(Vec3::Z).normalize_or(Vec3::X)
    }

    pub fn look(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx;
        self.pitch = (self.pitch - dy).clamp(-1.55, 1.55);
    }

    pub fn camera_view(&self, aspect: f32) -> CameraView {
        CameraView {
            view: view::look_to_mat4(self.position, self.forward(), Vec3::Z),
            projection: directx::perspective_infinite_reverse(self.fov_y, aspect.max(0.01), 0.1),
            eye: self.position,
        }
    }
}

/// A camera orbiting a point. The game's world is Z-up.
#[derive(Clone, Debug)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub distance: f32,
    /// Around Z, radians.
    pub yaw: f32,
    /// Above the horizon, radians.
    pub pitch: f32,
    /// Vertical field of view, radians.
    pub fov_y: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self { target: Vec3::ZERO, distance: 5.0, yaw: -0.6, pitch: 0.35, fov_y: 50f32.to_radians() }
    }
}

impl OrbitCamera {
    /// Frames a box: looks at its centre from far enough to see all of it.
    pub fn frame(&mut self, min: Vec3, max: Vec3) {
        self.target = (min + max) * 0.5;
        let radius = ((max - min).length() * 0.5).max(0.1);
        self.distance = radius / (self.fov_y * 0.5).sin() * 0.9;
    }

    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + Vec3::new(cy * cp, sy * cp, sp) * self.distance
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx;
        self.pitch = (self.pitch + dy).clamp(-1.55, 1.55);
    }

    /// Moves the target across the view, `dx`/`dy` in fractions of the distance.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let fwd = (self.target - self.eye()).normalize();
        let right = fwd.cross(Vec3::Z).normalize_or(Vec3::X);
        let up = right.cross(fwd);
        self.target += (-right * dx + up * dy) * self.distance;
    }

    pub fn zoom(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(0.01, 100_000.0);
    }

    pub fn view(&self) -> Mat4 {
        view::look_at_mat4(self.eye(), self.target, Vec3::Z)
    }

    pub fn camera_view(&self, aspect: f32) -> CameraView {
        CameraView { view: self.view(), projection: self.projection(aspect), eye: self.eye() }
    }

    /// Reversed-Z, infinite far plane; wgpu's clip space is DirectX's.
    pub fn projection(&self, aspect: f32) -> Mat4 {
        let near = (self.distance * 0.001).clamp(0.001, 1.0);
        directx::perspective_infinite_reverse(self.fov_y, aspect.max(0.01), near)
    }
}
