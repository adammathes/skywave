//! PCG32 (XSH-RR, 64-bit state) after O'Neill, "PCG: A Family of Simple Fast Space-Efficient
//! Statistically Good Algorithms for Random Number Generation", 2014.

use crate::{hash64, Seed};

const MUL: u64 = 6_364_136_223_846_793_005;

/// A small, fast, deterministic random number generator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng {
    state: u64,
    inc: u64,
}

impl Rng {
    /// The reference `pcg32_srandom_r(initstate, initseq)` construction.
    pub const fn from_pcg_state(initstate: u64, initseq: u64) -> Rng {
        let mut rng = Rng {
            state: 0,
            inc: (initseq << 1) | 1,
        };
        rng.step();
        rng.state = rng.state.wrapping_add(initstate);
        rng.step();
        rng
    }

    /// A generator for the sub-system named by `key` under `seed`. Distinct keys give
    /// independent streams.
    pub fn new(seed: Seed, key: &[u64]) -> Rng {
        let state = hash64(seed.0, key);
        let stream = hash64(state, &[0x5354_5245_414D]);
        Rng::from_pcg_state(state, stream)
    }

    const fn step(&mut self) {
        self.state = self.state.wrapping_mul(MUL).wrapping_add(self.inc);
    }

    /// Uniform over all `u32`.
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.step();
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Uniform in `[0, 1)` with 24 bits of precision.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (1.0 / 16_777_216.0)
    }

    /// Uniform integer in `[lo, hi)`, without modulo bias. Returns `lo` if the range is empty.
    pub fn range(&mut self, lo: u32, hi: u32) -> u32 {
        if hi <= lo {
            return lo;
        }
        let span = hi - lo;
        // Lemire's multiply-shift with rejection of the biased low region.
        let threshold = span.wrapping_neg() % span;
        loop {
            let m = u64::from(self.next_u32()) * u64::from(span);
            if (m as u32) >= threshold {
                return lo + (m >> 32) as u32;
            }
        }
    }

    /// Uniform float in `[lo, hi)`.
    pub fn range_f32(&mut self, lo: f32, hi: f32) -> f32 {
        scale_below(lo, hi, self.next_f32())
    }
}

/// `lo + (hi - lo) * t` for `t` in `[0, 1)`, kept below `hi`: with `t` near 1 the product can
/// round up to exactly `hi`, and then `lo` is returned instead.
fn scale_below(lo: f32, hi: f32, t: f32) -> f32 {
    let x = lo + (hi - lo) * t;
    if x < hi || hi <= lo {
        x
    } else {
        lo
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_pcg32_reference_demo() {
        // pcg32-demo from pcg-c-basic: pcg32_srandom_r(&rng, 42u, 54u).
        let mut rng = Rng::from_pcg_state(42, 54);
        let expected = [
            0xa15c_02b7,
            0x7b47_f409,
            0xba1d_3330,
            0x83d2_f293,
            0xbfa4_784b,
            0xcbed_606e,
        ];
        for e in expected {
            assert_eq!(rng.next_u32(), e);
        }
    }

    #[test]
    fn same_seed_same_sequence() {
        let mut a = Rng::new(Seed(7), &[1]);
        let mut b = Rng::new(Seed(7), &[1]);
        for _ in 0..1000 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
    }

    #[test]
    fn different_seeds_or_keys_differ() {
        let first = |seed, key| {
            let mut r = Rng::new(Seed(seed), &[key]);
            [r.next_u32(), r.next_u32(), r.next_u32(), r.next_u32()]
        };
        assert_ne!(first(7, 1), first(8, 1));
        assert_ne!(first(7, 1), first(7, 2));
    }

    #[test]
    fn seeded_sequence_is_pinned() {
        let mut r = Rng::new(Seed(1), &[]);
        let got = [r.next_u32(), r.next_u32(), r.next_u32()];
        assert_eq!(got, [0xb135_c5c5, 0x41b6_ffa2, 0x6c97_5116]);
    }

    #[test]
    fn f32_and_ranges_stay_in_bounds() {
        let mut r = Rng::new(Seed(3), &[]);
        let mut seen = [false; 6];
        for _ in 0..10_000 {
            let f = r.next_f32();
            assert!((0.0..1.0).contains(&f));
            let g = r.range_f32(-2.0, 5.0);
            assert!((-2.0..5.0).contains(&g));
            let i = r.range(10, 16);
            assert!((10..16).contains(&i));
            seen[(i - 10) as usize] = true;
        }
        assert!(seen.iter().all(|&s| s));
        assert_eq!(r.range(5, 5), 5);
        assert_eq!(r.range(9, 3), 9);
    }

    #[test]
    fn range_f32_never_rounds_up_to_hi() {
        // The largest `next_f32`: 1 - 2^-24.
        let t_max = 16_777_215.0 / 16_777_216.0;
        // 8 + 12 * t_max and 0.9 + 0.2 * t_max round to exactly `hi` in f32.
        assert_eq!(8.0f32 + (20.0f32 - 8.0) * t_max, 20.0);
        assert_eq!(scale_below(8.0, 20.0, t_max), 8.0);
        assert_eq!(0.9f32 + (1.1f32 - 0.9) * t_max, 1.1);
        assert_eq!(scale_below(0.9, 1.1, t_max), 0.9);
        assert!(scale_below(-2.0, 5.0, t_max) < 5.0);
        assert_eq!(scale_below(2.0, 2.0, t_max), 2.0);
        assert_eq!(scale_below(3.0, 1.0, 0.5), 2.0);
    }
}
