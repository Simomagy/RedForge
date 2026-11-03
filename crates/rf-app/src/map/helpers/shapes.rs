//! Helper meshes: the C#'s part shapes (box, sphere, capsule, cylinder) and
//! meshes of triangles, made on the CPU ([`Geometry`]) and uploaded, with the
//! unit box and sphere shared by every part that's one ([`Shapes`]).

use std::f32::consts::{PI, TAU};
use std::sync::Arc;

use glam::{Mat4, Quat, Vec3};
use rf_render::{HelperMesh, HelperVertex};

use crate::viewers::Gpu;

/// The unit shapes, white, opaque and see-through: parts tint them.
#[derive(Default)]
pub struct Shapes {
    boxes: [Option<Arc<HelperMesh>>; 2],
    spheres: [Option<Arc<HelperMesh>>; 2],
}

impl Shapes {
    /// A box from -0.5 to 0.5.
    pub fn unit_box(&mut self, gpu: &Gpu, translucent: bool) -> Arc<HelperMesh> {
        self.boxes[translucent as usize]
            .get_or_insert_with(|| {
                let mut g = Geometry::default();
                g.add_box(Mat4::IDENTITY, [255; 4]);
                g.upload(gpu, translucent).expect("a box")
            })
            .clone()
    }

    /// A sphere of radius 1 about the origin.
    pub fn unit_sphere(&mut self, gpu: &Gpu, translucent: bool) -> Arc<HelperMesh> {
        self.spheres[translucent as usize]
            .get_or_insert_with(|| {
                let mut g = Geometry::default();
                g.add_sphere(Vec3::ZERO, 1.0, [255; 4]);
                g.upload(gpu, translucent).expect("a sphere")
            })
            .clone()
    }
}

/// Triangles (or lines) on their way to a [`HelperMesh`].
#[derive(Default)]
pub struct Geometry {
    pub vertices: Vec<HelperVertex>,
    pub indices: Vec<u32>,
}

/// Around the sphere and its stacks pole to pole.
const SLICES: u32 = 16;
const STACKS: u32 = 8;

impl Geometry {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// On the GPU (`None` when there's nothing): `translucent` draws it after
    /// what's opaque, without hiding what's behind it.
    pub fn upload(&self, gpu: &Gpu, translucent: bool) -> Option<Arc<HelperMesh>> {
        if self.is_empty() {
            return None;
        }
        let mut mesh = gpu.renderer.create_helper_mesh(&gpu.ctx, &self.vertices, &self.indices, false);
        mesh.translucent |= translucent;
        Some(Arc::new(mesh))
    }

    fn vertex(&mut self, p: Vec3, n: Vec3, colour: [u8; 4]) -> u32 {
        self.vertices.push(HelperVertex::new(p, n, colour));
        self.vertices.len() as u32 - 1
    }

    /// A triangle, with its face's normal.
    pub fn add_triangle(&mut self, a: Vec3, b: Vec3, c: Vec3, colour: [u8; 4]) {
        let n = (b - a).cross(c - a).normalize_or_zero();
        let i = self.vertex(a, n, colour);
        self.vertex(b, n, colour);
        self.vertex(c, n, colour);
        self.indices.extend([i, i + 1, i + 2]);
    }

    /// A quad (corners in order around it), as two triangles.
    pub fn add_quad(&mut self, corners: [Vec3; 4], colour: [u8; 4]) {
        self.add_triangle(corners[0], corners[1], corners[2], colour);
        self.add_triangle(corners[0], corners[2], corners[3], colour);
    }

    /// A box from -0.5 to 0.5 through `m`.
    pub fn add_box(&mut self, m: Mat4, colour: [u8; 4]) {
        let p = |x: f32, y: f32, z: f32| m.transform_point3(Vec3::new(x, y, z) * 0.5);
        let c: [Vec3; 8] = std::array::from_fn(|i| {
            p(
                if i & 1 != 0 { 1.0 } else { -1.0 },
                if i & 2 != 0 { 1.0 } else { -1.0 },
                if i & 4 != 0 { 1.0 } else { -1.0 },
            )
        });
        for f in [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]] {
            self.add_quad(f.map(|i| c[i]), colour);
        }
    }

    /// A UV sphere.
    pub fn add_sphere(&mut self, centre: Vec3, radius: f32, colour: [u8; 4]) {
        self.add_cap(centre, radius, Quat::IDENTITY, 0.0, PI, colour);
    }

    /// Part of a sphere about `centre`, turned by `q`, from the polar angle
    /// `from` to `to` (0 is +Z).
    fn add_cap(&mut self, centre: Vec3, radius: f32, q: Quat, from: f32, to: f32, colour: [u8; 4]) {
        let first = self.vertices.len() as u32;
        for s in 0..=STACKS {
            let theta = from + (to - from) * s as f32 / STACKS as f32;
            for k in 0..=SLICES {
                let phi = TAU * k as f32 / SLICES as f32;
                let n = q * Vec3::new(theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos());
                self.vertex(centre + n * radius, n, colour);
            }
        }
        let row = SLICES + 1;
        for s in 0..STACKS {
            for k in 0..SLICES {
                let a = first + s * row + k;
                self.indices.extend([a, a + row, a + 1, a + 1, a + row, a + row + 1]);
            }
        }
    }

    /// A cylinder from `a` to `b`, open (`ends` closes it).
    fn add_tube(&mut self, a: Vec3, b: Vec3, radius: f32, ends: bool, colour: [u8; 4]) {
        let axis = (b - a).normalize_or(Vec3::Z);
        let q = Quat::from_rotation_arc(Vec3::Z, axis);
        let first = self.vertices.len() as u32;
        for k in 0..=SLICES {
            let phi = TAU * k as f32 / SLICES as f32;
            let n = q * Vec3::new(phi.cos(), phi.sin(), 0.0);
            self.vertex(a + n * radius, n, colour);
            self.vertex(b + n * radius, n, colour);
        }
        for k in 0..SLICES {
            let i = first + k * 2;
            self.indices.extend([i, i + 2, i + 1, i + 1, i + 2, i + 3]);
        }
        if ends {
            for (centre, n) in [(a, -axis), (b, axis)] {
                let mid = self.vertex(centre, n, colour);
                for k in 0..SLICES {
                    let edge = |k: u32| {
                        let phi = TAU * k as f32 / SLICES as f32;
                        centre + q * Vec3::new(phi.cos(), phi.sin(), 0.0) * radius
                    };
                    let (p, r) = (edge(k), edge(k + 1));
                    let i = self.vertex(p, n, colour);
                    self.vertex(r, n, colour);
                    self.indices.extend([mid, i, i + 1]);
                }
            }
        }
    }

    pub fn add_cylinder(&mut self, a: Vec3, b: Vec3, radius: f32, colour: [u8; 4]) {
        self.add_tube(a, b, radius, true, colour);
    }

    /// A cylinder from `a` to `b` with half spheres on its ends.
    pub fn add_capsule(&mut self, a: Vec3, b: Vec3, radius: f32, colour: [u8; 4]) {
        let axis = (b - a).normalize_or(Vec3::Z);
        let q = Quat::from_rotation_arc(Vec3::Z, axis);
        self.add_tube(a, b, radius, false, colour);
        self.add_cap(b, radius, q, 0.0, PI * 0.5, colour);
        self.add_cap(a, radius, q, PI * 0.5, PI, colour);
    }
}
