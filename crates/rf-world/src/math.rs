//! The bits of `System.Numerics` the C# uses for entity transforms, with the
//! same formulas (quaternions are `[x, y, z, w]`).

pub type Vec3 = [f32; 3];
pub type Quat = [f32; 4];

pub const IDENTITY: Quat = [0.0, 0.0, 0.0, 1.0];

/// `Quaternion.Inverse`: the conjugate over the squared length.
pub fn quat_inverse(q: Quat) -> Quat {
    let ls = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
    let inv = 1.0 / ls;
    [-q[0] * inv, -q[1] * inv, -q[2] * inv, q[3] * inv]
}

/// `Quaternion.Normalize`.
pub fn quat_normalize(q: Quat) -> Quat {
    let inv = 1.0 / (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    [q[0] * inv, q[1] * inv, q[2] * inv, q[3] * inv]
}

/// `a * b` (`Quaternion.Multiply`): the Hamilton product, `b`'s rotation then `a`'s.
pub fn quat_mul(a: Quat, b: Quat) -> Quat {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    let cx = ay * bz - az * by;
    let cy = az * bx - ax * bz;
    let cz = ax * by - ay * bx;
    let dot = ax * bx + ay * by + az * bz;
    [ax * bw + bx * aw + cx, ay * bw + by * aw + cy, az * bw + bz * aw + cz, aw * bw - dot]
}

/// `QuaternionExtensions.Multiply`: `v` rotated by `q`.
pub fn quat_rotate(a: Quat, b: Vec3) -> Vec3 {
    let (axx, ayy, azz) = (a[0] * 2.0, a[1] * 2.0, a[2] * 2.0);
    let (awxx, awyy, awzz) = (a[3] * axx, a[3] * ayy, a[3] * azz);
    let (axxx, axyy, axzz) = (a[0] * axx, a[0] * ayy, a[0] * azz);
    let (ayyy, ayzz, azzz) = (a[1] * ayy, a[1] * azz, a[2] * azz);
    [
        b[0] * ((1.0 - ayyy) - azzz) + b[1] * (axyy - awzz) + b[2] * (axzz + awyy),
        b[0] * (axyy + awzz) + b[1] * ((1.0 - axxx) - azzz) + b[2] * (ayzz - awxx),
        b[0] * (axzz - awyy) + b[1] * (ayzz + awxx) + b[2] * ((1.0 - axxx) - ayyy),
    ]
}

pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn distance(a: Vec3, b: Vec3) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// `Matrix4x4Ext.Transformation(scale, rotation, translation)`: scale, then
/// rotate, then translate. `System.Numerics` matrices are row-major for row
/// vectors, so their memory layout is this transform column-major for column
/// vectors (`glam::Mat4::from_cols_array`).
pub fn transformation(scale: Vec3, q: Quat, t: Vec3) -> [f32; 16] {
    let [x, y, z, w] = q;
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, wz, xz, wy, yz, wx) = (x * y, w * z, x * z, w * y, y * z, w * x);
    let [sx, sy, sz] = scale;
    [
        sx * (1.0 - 2.0 * (yy + zz)),
        sx * (2.0 * (xy + wz)),
        sx * (2.0 * (xz - wy)),
        0.0,
        sy * (2.0 * (xy - wz)),
        sy * (1.0 - 2.0 * (zz + xx)),
        sy * (2.0 * (yz + wx)),
        0.0,
        sz * (2.0 * (xz + wy)),
        sz * (2.0 * (yz - wx)),
        sz * (1.0 - 2.0 * (yy + xx)),
        0.0,
        t[0],
        t[1],
        t[2],
        1.0,
    ]
}

/// A point through a column-major transform.
pub fn transform_point(m: &[f32; 16], p: Vec3) -> Vec3 {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

/// An axis-aligned box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    /// A box nothing is in (`BoundingBox`'s default is a point at the origin,
    /// which the C# entities without a model have; this is used instead).
    pub const EMPTY: Aabb = Aabb { min: [f32::MAX; 3], max: [f32::MIN; 3] };

    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    /// `ContainmentType.Contains` for a point: inside or on the faces.
    pub fn contains(&self, p: Vec3) -> bool {
        (0..3).all(|i| p[i] >= self.min[i] && p[i] <= self.max[i])
    }

    pub fn centre(&self) -> Vec3 {
        [0, 1, 2].map(|i| (self.min[i] + self.max[i]) * 0.5)
    }

    pub fn extend(&mut self, p: Vec3) {
        self.min = [0, 1, 2].map(|i| self.min[i].min(p[i]));
        self.max = [0, 1, 2].map(|i| self.max[i].max(p[i]));
    }

    /// `Entity.UpdateBoundsFromLocal`: a local box through an entity's transform.
    /// Unrotated, the box is scaled and moved; else its corners are transformed.
    pub fn transformed(&self, scale: Vec3, q: Quat, m: &[f32; 16]) -> Aabb {
        let pos = [m[12], m[13], m[14]];
        if q == IDENTITY {
            let lo = [0, 1, 2].map(|i| self.min[i].min(self.max[i]) * scale[i] + pos[i]);
            let hi = [0, 1, 2].map(|i| self.min[i].max(self.max[i]) * scale[i] + pos[i]);
            return Aabb::new(lo, hi);
        }
        let mut out = Aabb::EMPTY;
        for c in 0..8 {
            let p = [0, 1, 2].map(|i| if c & (1 << i) != 0 { self.max[i] } else { self.min[i] });
            out.extend(transform_point(m, p));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &[f32], b: &[f32]) -> bool {
        a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-5)
    }

    /// A rotation of `deg` degrees about Z.
    fn about_z(deg: f32) -> Quat {
        let h = deg.to_radians() * 0.5;
        [0.0, 0.0, h.sin(), h.cos()]
    }

    #[test]
    fn rotations() {
        // 90° about Z takes X to Y, by the vector formula and by the matrix.
        let q = about_z(90.0);
        assert!(close(&quat_rotate(q, [1.0, 0.0, 0.0]), &[0.0, 1.0, 0.0]));
        let m = transformation([1.0; 3], q, [0.0; 3]);
        assert!(close(&transform_point(&m, [1.0, 0.0, 0.0]), &[0.0, 1.0, 0.0]));
        // The inverse rotates back; products compose (b then a).
        assert!(close(&quat_rotate(quat_inverse(q), [0.0, 1.0, 0.0]), &[1.0, 0.0, 0.0]));
        assert!(close(&quat_mul(q, q), &about_z(180.0)));
        let a = about_z(30.0);
        let b = [0.5f32.sqrt(), 0.0, 0.0, 0.5f32.sqrt()]; // 90° about X
        let v = [0.3, -1.2, 2.0];
        assert!(close(&quat_rotate(quat_mul(a, b), v), &quat_rotate(a, quat_rotate(b, v))));
        assert!(close(&quat_normalize([0.0, 0.0, 2.0, 0.0]), &[0.0, 0.0, 1.0, 0.0]));
    }

    #[test]
    fn transforms_scale_then_rotate_then_translate() {
        let m = transformation([2.0, 2.0, 3.0], about_z(90.0), [10.0, 20.0, 30.0]);
        assert!(close(&transform_point(&m, [1.0, 0.0, 1.0]), &[10.0, 22.0, 33.0]));
        // Column-major: the translation is the last column.
        assert_eq!(&m[12..], &[10.0, 20.0, 30.0, 1.0]);
    }

    #[test]
    fn boxes() {
        let b = Aabb::new([-1.0, -2.0, 0.0], [1.0, 2.0, 4.0]);
        let m = transformation([1.0; 3], IDENTITY, [5.0, 0.0, 0.0]);
        assert_eq!(b.transformed([1.0; 3], IDENTITY, &m), Aabb::new([4.0, -2.0, 0.0], [6.0, 2.0, 4.0]));
        let q = about_z(90.0);
        let m = transformation([1.0; 3], q, [0.0; 3]);
        let r = b.transformed([1.0; 3], q, &m);
        assert!(close(&r.min, &[-2.0, -1.0, 0.0]) && close(&r.max, &[2.0, 1.0, 4.0]));
        assert!(r.contains([0.0, 0.0, 4.0]) && !r.contains([0.0, 1.5, 1.0]));
        assert!(!Aabb::EMPTY.contains([0.0; 3]));
    }
}
