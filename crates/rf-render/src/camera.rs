use glam::camera::rh::{proj::directx, view};
use glam::{Mat4, Vec3};

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

    /// Reversed-Z, infinite far plane; wgpu's clip space is DirectX's.
    pub fn projection(&self, aspect: f32) -> Mat4 {
        let near = (self.distance * 0.001).clamp(0.001, 1.0);
        directx::perspective_infinite_reverse(self.fov_y, aspect.max(0.01), near)
    }
}
