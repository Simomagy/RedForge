//! Exact Euclidean distance transform (Felzenszwalb and Huttenlocher), in
//! squared distances.

fn pass(f: &[f64], n: usize, d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    let parabola = |q: usize, p: usize| {
        let (qf, pf) = (q as f64, p as f64);
        (f[q] + qf * qf - (f[p] + pf * pf)) / (2.0 * qf - 2.0 * pf)
    };
    for q in 1..n {
        let mut s = parabola(q, v[k]);
        while s <= z[k] {
            k -= 1;
            s = parabola(q, v[k]);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for (q, out) in d.iter_mut().enumerate().take(n) {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let dq = q as f64 - v[k] as f64;
        *out = dq * dq + f[v[k]];
    }
}

/// Squared distances to the nearest pixel with `seed` set (0 on those).
pub fn edt(seed: &[u8], w: usize, h: usize) -> Vec<f32> {
    let mut out: Vec<f32> = seed.iter().map(|&s| if s != 0 { 0.0 } else { 1e20 }).collect();
    let n = w.max(h);
    let (mut f, mut d, mut v, mut z) = (vec![0.0; n], vec![0.0; n], vec![0usize; n], vec![0.0; n + 1]);
    for x in 0..w {
        for y in 0..h {
            f[y] = out[y * w + x] as f64;
        }
        pass(&f, h, &mut d, &mut v, &mut z);
        for y in 0..h {
            out[y * w + x] = d[y] as f32;
        }
    }
    for y in 0..h {
        for x in 0..w {
            f[x] = out[y * w + x] as f64;
        }
        pass(&f, w, &mut d, &mut v, &mut z);
        for x in 0..w {
            out[y * w + x] = d[x] as f32;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_brute_force() {
        let (w, h) = (23, 17);
        let seed: Vec<u8> = (0..w * h).map(|i| ((i * 7919) % 31 == 0) as u8).collect();
        let d = edt(&seed, w, h);
        for y in 0..h {
            for x in 0..w {
                let mut best = f64::MAX;
                for sy in 0..h {
                    for sx in 0..w {
                        if seed[sy * w + sx] != 0 {
                            let (dx, dy) = (x as f64 - sx as f64, y as f64 - sy as f64);
                            best = best.min(dx * dx + dy * dy);
                        }
                    }
                }
                assert_eq!(d[y * w + x] as f64, best, "{x},{y}");
            }
        }
    }
}
