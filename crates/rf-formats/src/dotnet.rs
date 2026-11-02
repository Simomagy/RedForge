//! Bit-exact reproductions of .NET behaviour that game data was built with.

/// .NET's `List<T>.Sort(Comparison<T>)`, whose unstable order of equal items
/// the C# RedForge's results depend on.
pub use crate::formats::ybn::math::dotnet_sort;

/// `System.Random` constructed with a seed (the legacy Knuth subtractive
/// generator that .NET keeps for seeded instances).
///
/// `magic.dat` was obfuscated with byte streams from this generator, so it has to
/// match .NET exactly.
pub struct DotNetRandom {
    seed_array: [i32; 56],
    inext: usize,
    inextp: usize,
}

impl DotNetRandom {
    const MBIG: i32 = i32::MAX;
    const MSEED: i32 = 161_803_398;

    pub fn new(seed: i32) -> Self {
        let mut seed_array = [0i32; 56];
        let subtraction = if seed == i32::MIN { i32::MAX } else { seed.abs() };
        let mut mj = Self::MSEED.wrapping_sub(subtraction);
        seed_array[55] = mj;
        let mut mk: i32 = 1;
        let mut ii = 0usize;
        for _ in 1..55 {
            ii += 21;
            if ii >= 55 {
                ii -= 55;
            }
            seed_array[ii] = mk;
            mk = mj.wrapping_sub(mk);
            if mk < 0 {
                mk = mk.wrapping_add(Self::MBIG);
            }
            mj = seed_array[ii];
        }
        for _ in 1..5 {
            for i in 1..56 {
                let mut n = i + 30;
                if n >= 55 {
                    n -= 55;
                }
                seed_array[i] = seed_array[i].wrapping_sub(seed_array[1 + n]);
                if seed_array[i] < 0 {
                    seed_array[i] = seed_array[i].wrapping_add(Self::MBIG);
                }
            }
        }
        Self { seed_array, inext: 0, inextp: 21 }
    }

    fn internal_sample(&mut self) -> i32 {
        let mut inext = self.inext + 1;
        if inext >= 56 {
            inext = 1;
        }
        let mut inextp = self.inextp + 1;
        if inextp >= 56 {
            inextp = 1;
        }
        let mut ret = self.seed_array[inext].wrapping_sub(self.seed_array[inextp]);
        if ret == Self::MBIG {
            ret -= 1;
        }
        if ret < 0 {
            ret = ret.wrapping_add(Self::MBIG);
        }
        self.seed_array[inext] = ret;
        self.inext = inext;
        self.inextp = inextp;
        ret
    }

    /// `Random.Next()`: a value in `[0, i32::MAX)`.
    pub fn next_int(&mut self) -> i32 {
        self.internal_sample()
    }

    /// `Random.NextBytes`: each byte is the low byte of one sample.
    pub fn next_bytes(&mut self, buf: &mut [u8]) {
        for b in buf {
            *b = self.internal_sample() as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_in_range() {
        let mut a = DotNetRandom::new(12345);
        let mut b = DotNetRandom::new(12345);
        for _ in 0..1000 {
            let x = a.next_int();
            assert_eq!(x, b.next_int());
            assert!((0..i32::MAX).contains(&x));
        }
        // i32::MIN must not overflow in abs().
        let _ = DotNetRandom::new(i32::MIN).next_int();
    }
}
