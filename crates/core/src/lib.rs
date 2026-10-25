//! Deterministic primitives shared by every Skywave crate.
//!
//! Everything here is `no_std`, allocation-free and bit-identical on every platform:
//! integer hashing, a seeded PCG32 generator and the fixed-timestep accumulator that
//! drives the 120 Hz simulation.
#![no_std]

mod hash;
mod rng;
mod step;

pub use hash::{hash64, hash_bytes, mix64};
pub use rng::Rng;
pub use step::{FixedStep, Steps, MAX_STEPS_PER_FRAME, SIM_DT, SIM_HZ};

/// The root seed of a generated place or session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seed(pub u64);

impl Seed {
    /// Derives an independent child seed for a named sub-system, e.g. `seed.derive(&[ROADS])`.
    pub fn derive(self, key: &[u64]) -> Seed {
        Seed(hash64(self.0, key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_seeds_are_stable_and_distinct() {
        let root = Seed(123);
        assert_eq!(root.derive(&[1]), Seed(hash64(123, &[1])));
        assert_ne!(root.derive(&[1]), root.derive(&[2]));
        assert_ne!(root.derive(&[1]), Seed(124).derive(&[1]));
        assert_ne!(root.derive(&[1, 2]), root.derive(&[1]).derive(&[2]));
    }
}
