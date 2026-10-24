//! A hand-written pseudo-random number generator.
//!
//! The `rand` crate is banned workspace-wide, so the generator is written
//! here: `splitmix64` expands a single seed into four words of state, and
//! `xoshiro256**` produces the stream.
//!
//! Both are small, well-documented algorithms with no dependencies and no
//! platform-dependent behaviour, which is the whole point: a dataset must be
//! reproducible from its seed alone, on any machine, forever. A generator
//! whose output depends on the version of a crate in the dependency graph is
//! not reproducible, it is merely repeatable.
//!
//! ```
//! use columnar_testdata::prng::Xoshiro256;
//!
//! let mut a = Xoshiro256::seed_from_u64(0x5EED);
//! let mut b = Xoshiro256::seed_from_u64(0x5EED);
//! assert_eq!(a.next_u64(), b.next_u64());
//! ```

/// Expands `state` with the `splitmix64` mixing function.
///
/// This is used only to turn one seed into the four words of state that
/// `xoshiro256**` needs. It is not part of the generated stream.
#[must_use]
pub fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The `xoshiro256**` generator: 256 bits of state, one `u64` out per step.
///
/// Deterministic and reproducible from a seed alone. The `**` variant is
/// used rather than `+` because it passes `BigCrush` and has the better
/// `xoshiro256**` score; the difference is irrelevant at the scale of a
/// dataset, and reproducibility is what matters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Xoshiro256 {
    state: [u64; 4],
}

impl Xoshiro256 {
    /// Builds a generator from a single seed.
    ///
    /// The seed is expanded with `splitmix64` into the four words of state,
    /// so that adjacent seeds do not produce correlated streams. A zero seed
    /// is valid: `splitmix64` never leaves the state all-zero.
    #[must_use]
    pub fn seed_from_u64(seed: u64) -> Self {
        let mut mixer = seed;
        let mut state = [0u64; 4];
        for word in &mut state {
            *word = splitmix64(&mut mixer);
        }
        Self { state }
    }

    /// Returns the next 64 bits of the stream.
    pub fn next_u64(&mut self) -> u64 {
        let result = self.state[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.state[1] << 17;

        self.state[2] ^= self.state[0];
        self.state[3] ^= self.state[1];
        self.state[1] ^= self.state[2];
        self.state[0] ^= self.state[3];
        self.state[2] ^= t;
        self.state[3] = self.state[3].rotate_left(45);

        result
    }

    /// Returns the next 32 bits of the stream.
    pub fn next_u32(&mut self) -> u32 {
        // The high half of a `u64` always fits in a `u32`.
        u32::try_from(self.next_u64() >> 32).expect("the high 32 bits fit in a u32")
    }

    /// Returns the next value as a double in `[0.0, 1.0)`.
    ///
    /// The 53 random bits are placed in the mantissa of `1.0 + m * 2^-53` and
    /// `1.0` is subtracted. Both steps are exact in IEEE-754, so the result
    /// is a pure function of the seed and the whole range is covered
    /// uniformly. Building the double from its bits avoids a `u64`-to-`f64`
    /// conversion, which would lose precision on the way in.
    pub fn next_f64(&mut self) -> f64 {
        let mantissa = self.next_u64() >> 11;
        f64::from_bits(mantissa | 0x3FF0_0000_0000_0000) - 1.0
    }

    /// Returns a value in `[0, bound)`, uniformly and without modulo bias.
    ///
    /// # Panics
    ///
    /// Panics if `bound` is zero.
    pub fn below(&mut self, bound: u64) -> u64 {
        assert!(bound > 0, "bound must be positive");

        // The largest multiple of `bound` that fits in a `u64`. Values below
        // it map onto `[0, bound)` with every residue class equally likely;
        // values at or above it are rejected.
        let threshold = u64::MAX - (u64::MAX % bound);
        loop {
            let candidate = self.next_u64();
            if candidate < threshold {
                return candidate % bound;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Xoshiro256;

    #[test]
    fn same_seed_gives_same_stream() {
        let mut a = Xoshiro256::seed_from_u64(0x5EED);
        let mut b = Xoshiro256::seed_from_u64(0x5EED);
        for _ in 0..64 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_give_different_streams() {
        let mut a = Xoshiro256::seed_from_u64(1);
        let mut b = Xoshiro256::seed_from_u64(2);
        let left: Vec<u64> = (0..8).map(|_| a.next_u64()).collect();
        let right: Vec<u64> = (0..8).map(|_| b.next_u64()).collect();
        assert_ne!(left, right);
    }

    #[test]
    fn next_f64_stays_in_the_unit_interval() {
        let mut rng = Xoshiro256::seed_from_u64(7);
        for _ in 0..10_000 {
            let value = rng.next_f64();
            assert!((0.0..1.0).contains(&value), "{value} out of range");
        }
    }

    #[test]
    fn below_stays_in_range() {
        let mut rng = Xoshiro256::seed_from_u64(9);
        for _ in 0..10_000 {
            assert!(rng.below(7) < 7);
            assert!(rng.below(1) < 1);
        }
    }

    #[test]
    fn below_covers_its_range() {
        let mut rng = Xoshiro256::seed_from_u64(11);
        let mut seen = [false; 7];
        for _ in 0..1_000 {
            let value = rng.below(7);
            let index = usize::try_from(value).expect("below(7) fits in a usize");
            *seen.get_mut(index).expect("the index is below the array") = true;
        }
        assert!(seen.iter().all(|hit| *hit), "every residue must appear");
    }
}
