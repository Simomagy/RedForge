//! `f32` vector maths that rounds exactly like the `System.Numerics` of the .NET
//! the C# runs on (checked against .NET 10 on x64):
//! - `Vector3.Transform` and `TransformNormal` are fused multiply-adds;
//! - `Vector3.Cross` and `Dot` (so `Length`) are not;
//! - division by a scalar is a true division, not a multiplication by the
//!   reciprocal;
//! - `Vector3.Min`/`Max` are IEEE 754-2019 `minimum`/`maximum` (NaN propagates,
//!   -0 < +0).

pub type Vec3 = [f32; 3];
/// A `System.Numerics.Matrix4x4`: M11, M12, M13, M14, M21, ... M44.
pub type Mat4 = [f32; 16];

pub const IDENTITY: Mat4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

#[inline]
pub fn splat(v: f32) -> Vec3 {
    [v, v, v]
}
#[inline]
pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
pub fn mul(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}
#[inline]
pub fn div(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] / b[0], a[1] / b[1], a[2] / b[2]]
}
#[inline]
pub fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
#[inline]
pub fn div_scalar(a: Vec3, s: f32) -> Vec3 {
    [a[0] / s, a[1] / s, a[2] / s]
}
#[inline]
pub fn abs(a: Vec3) -> Vec3 {
    a.map(f32::abs)
}

/// IEEE 754-2019 `minimum`, as .NET's `Vector3.Min` and `Math.Min`.
#[inline]
pub fn fmin(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        a
    } else if b.is_nan() {
        b
    } else if a == b {
        if a.is_sign_negative() { a } else { b }
    } else if a < b {
        a
    } else {
        b
    }
}

/// IEEE 754-2019 `maximum`, as .NET's `Vector3.Max` and `Math.Max`.
#[inline]
pub fn fmax(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        a
    } else if b.is_nan() {
        b
    } else if a == b {
        if a.is_sign_positive() { a } else { b }
    } else if a > b {
        a
    } else {
        b
    }
}

#[inline]
pub fn min(a: Vec3, b: Vec3) -> Vec3 {
    [fmin(a[0], b[0]), fmin(a[1], b[1]), fmin(a[2], b[2])]
}
#[inline]
pub fn max(a: Vec3, b: Vec3) -> Vec3 {
    [fmax(a[0], b[0]), fmax(a[1], b[1]), fmax(a[2], b[2])]
}

#[inline]
pub fn dot(a: Vec3, b: Vec3) -> f32 {
    (a[0] * b[0] + a[1] * b[1]) + a[2] * b[2]
}
#[inline]
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// RedForge's `GetSafeNormal`.
pub fn safe_normal(v: Vec3) -> Vec3 {
    let ls = dot(v, v);
    if ls == 1.0 {
        return v;
    }
    if ls < 1e-8 {
        return [0.0; 3];
    }
    let s = 1.0 / ls.sqrt();
    scale(v, s)
}

/// `Vector3.Transform(position, matrix)`.
pub fn transform(p: Vec3, m: &Mat4) -> Vec3 {
    std::array::from_fn(|j| {
        let r = m[j] * p[0];
        let r = m[4 + j].mul_add(p[1], r);
        let r = m[8 + j].mul_add(p[2], r);
        r + m[12 + j]
    })
}

/// `Vector3.TransformNormal(normal, matrix)`.
pub fn transform_normal(n: Vec3, m: &Mat4) -> Vec3 {
    std::array::from_fn(|j| {
        let r = m[j] * n[0];
        let r = m[4 + j].mul_add(n[1], r);
        m[8 + j].mul_add(n[2], r)
    })
}

/// RedForge's `Matrix4x4Ext.TransformCoordinate` (plain scalar code, with the
/// division by W).
pub fn transform_coordinate(c: Vec3, m: &Mat4) -> Vec3 {
    let x = c[0] * m[0] + c[1] * m[4] + c[2] * m[8] + m[12];
    let y = c[0] * m[1] + c[1] * m[5] + c[2] * m[9] + m[13];
    let z = c[0] * m[2] + c[1] * m[6] + c[2] * m[10] + m[14];
    let w = 1.0 / (c[0] * m[3] + c[1] * m[7] + c[2] * m[11] + m[15]);
    [x * w, y * w, z * w]
}

/// `float.CompareTo`: NaN sorts first, and equals NaN.
pub fn compare_f32(a: f32, b: f32) -> i32 {
    if a < b {
        -1
    } else if a > b {
        1
    } else if a == b {
        0
    } else if a.is_nan() {
        if b.is_nan() { 0 } else { -1 }
    } else {
        1
    }
}

/// RedForge's `Vector3.CompareTo` extension: X, then Y, then Z.
pub fn compare_vec3(a: Vec3, b: Vec3) -> i32 {
    for i in 0..3 {
        let c = compare_f32(a[i], b[i]);
        if c != 0 {
            return c;
        }
    }
    0
}

/// .NET's `List<T>.Sort(Comparison<T>)`: the unstable introsort of
/// `ArraySortHelper<T>`, so equal items end up in the same order as there.
pub fn dotnet_sort<T: Copy>(keys: &mut [T], cmp: &impl Fn(&T, &T) -> i32) {
    if keys.len() > 1 {
        let depth = 2 * (keys.len().ilog2() as i32 + 1);
        intro_sort(keys, depth, cmp);
    }
}

fn swap_if_greater<T: Copy>(keys: &mut [T], cmp: &impl Fn(&T, &T) -> i32, i: usize, j: usize) {
    if cmp(&keys[i], &keys[j]) > 0 {
        keys.swap(i, j);
    }
}

fn intro_sort<T: Copy>(keys: &mut [T], mut depth: i32, cmp: &impl Fn(&T, &T) -> i32) {
    let mut size = keys.len();
    while size > 1 {
        if size <= 16 {
            if size == 2 {
                swap_if_greater(keys, cmp, 0, 1);
                return;
            }
            if size == 3 {
                swap_if_greater(keys, cmp, 0, 1);
                swap_if_greater(keys, cmp, 0, 2);
                swap_if_greater(keys, cmp, 1, 2);
                return;
            }
            insertion_sort(&mut keys[..size], cmp);
            return;
        }
        if depth == 0 {
            heap_sort(&mut keys[..size], cmp);
            return;
        }
        depth -= 1;
        let p = pick_pivot_and_partition(&mut keys[..size], cmp);
        intro_sort(&mut keys[p + 1..size], depth, cmp);
        size = p;
    }
}

fn pick_pivot_and_partition<T: Copy>(keys: &mut [T], cmp: &impl Fn(&T, &T) -> i32) -> usize {
    let hi = keys.len() - 1;
    let middle = hi >> 1;
    swap_if_greater(keys, cmp, 0, middle);
    swap_if_greater(keys, cmp, 0, hi);
    swap_if_greater(keys, cmp, middle, hi);
    let pivot = keys[middle];
    keys.swap(middle, hi - 1);
    let (mut left, mut right) = (0usize, hi - 1);
    while left < right {
        loop {
            left += 1;
            if cmp(&keys[left], &pivot) >= 0 {
                break;
            }
        }
        loop {
            right -= 1;
            if cmp(&pivot, &keys[right]) >= 0 {
                break;
            }
        }
        if left >= right {
            break;
        }
        keys.swap(left, right);
    }
    if left != hi - 1 {
        keys.swap(left, hi - 1);
    }
    left
}

fn heap_sort<T: Copy>(keys: &mut [T], cmp: &impl Fn(&T, &T) -> i32) {
    let n = keys.len();
    for i in (1..=n >> 1).rev() {
        down_heap(keys, i, n, cmp);
    }
    for i in (2..=n).rev() {
        keys.swap(0, i - 1);
        down_heap(keys, 1, i - 1, cmp);
    }
}

fn down_heap<T: Copy>(keys: &mut [T], mut i: usize, n: usize, cmp: &impl Fn(&T, &T) -> i32) {
    let d = keys[i - 1];
    while i <= n >> 1 {
        let mut child = 2 * i;
        if child < n && cmp(&keys[child - 1], &keys[child]) < 0 {
            child += 1;
        }
        if cmp(&d, &keys[child - 1]) >= 0 {
            break;
        }
        keys[i - 1] = keys[child - 1];
        i = child;
    }
    keys[i - 1] = d;
}

fn insertion_sort<T: Copy>(keys: &mut [T], cmp: &impl Fn(&T, &T) -> i32) {
    for i in 0..keys.len() - 1 {
        let t = keys[i + 1];
        let mut j = i as isize;
        while j >= 0 && cmp(&t, &keys[j as usize]) < 0 {
            keys[j as usize + 1] = keys[j as usize];
            j -= 1;
        }
        keys[(j + 1) as usize] = t;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_sorts() {
        let mut v: Vec<i32> = (0..300).map(|i| (i * 7919) % 101).collect();
        dotnet_sort(&mut v, &|a, b| a.cmp(b) as i32);
        assert!(v.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn min_max_like_dotnet() {
        assert!(fmin(-0.0, 0.0).is_sign_negative());
        assert!(fmin(0.0, -0.0).is_sign_negative());
        assert!(fmax(-0.0, 0.0).is_sign_positive());
        assert!(fmin(f32::NAN, 1.0).is_nan());
        assert!(fmax(1.0, f32::NAN).is_nan());
    }
}
