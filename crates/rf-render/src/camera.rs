use glam::camera::rh::{proj::directx, view};
use glam::{Mat4, Vec3, Vec4};

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
