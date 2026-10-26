//! Rebuilding the column in the browser.
//!
//! The column is the same **multiset** as the native benchmark's, in a
//! deterministic order that is **not** the native one's -- the browser has no
//! Parquet reader, so it cannot know the file's row order, and pretending
//! otherwise would be a lie about what is being measured.
//!
//! What matters for the number the browser publishes is the multiset: the
//! predicate is applied to each row independently, so the count of matching
//! rows depends only on which values are present and how many of each.
//!
//! The **order** does matter for one thing: the oracle is a row-at-a-time
//! loop with a branch, and its speed depends on how predictable that branch
//! is. Filling the column in value order would give the oracle long runs of
//! the same value, which is the best case for a branch predictor and not a
//! representative one. So the values are **interleaved** by a deterministic
//! generator: the multiset is exact and the order is not sorted.
//!
//! ## The generator
//!
//! `SplitMix64`. It is written out here rather than pulled in, because the
//! column the browser measures must be **the same column** on every machine
//! and every run: a reader who reloads the page has to get the same numbers.

use crate::histogram::{SEGMENTS, VALUES};

/// A `SplitMix64` generator.
///
/// Small, fast, and fully determined by its seed — which is what the column
/// needs.
#[derive(Debug, Clone)]
struct SplitMix64(u64);

impl SplitMix64 {
    /// The generator's seed.
    const SEED: u64 = 0x5EED_C0DE_BA5E_1234;

    /// A generator at the fixed seed.
    const fn new() -> Self {
        Self(Self::SEED)
    }

    /// The next 64 bits.
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Builds a column of `rows` values with the canonical multiset.
///
/// The first `rows` values of the canonical 20,000,000-row column, in a
/// deterministic interleaved order. A request for more rows than the
/// canonical column has is truncated to it, because the multiset past that
/// point is not defined.
pub fn build(rows: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(rows);
    let mut rng = SplitMix64::new();
    for (counts, segment_rows) in SEGMENTS {
        if out.len() >= rows {
            break;
        }
        let take = segment_rows.min(rows - out.len());
        fill(&mut out, counts, take, &mut rng);
    }
    out
}

/// Appends `take` values drawn from `counts`, in a deterministic order.
///
/// Each value is drawn with a probability proportional to the number of times
/// it is still to be placed, so the multiset is exact: the counts are a
/// budget, not a weight.
fn fill(out: &mut Vec<f64>, counts: &[u32; 11], take: usize, rng: &mut SplitMix64) {
    let mut remaining = *counts;
    let mut total: u64 = remaining.iter().map(|&count| u64::from(count)).sum();
    for _ in 0..take {
        // A value is chosen by picking a point in the remaining budget and
        // finding which value's share it falls in.
        let mut point = rng.next_u64() % total;
        let mut chosen = 0usize;
        for (index, count) in remaining.iter().enumerate() {
            let share = u64::from(*count);
            if point < share {
                chosen = index;
                break;
            }
            point -= share;
        }
        // `chosen` is an index into `remaining`, which has the same length as
        // `VALUES`, so both lookups are in bounds by construction. They are
        // written as lookups anyway, because a panic here would be a silent
        // truncation of the column rather than a wrong number.
        if let Some(slot) = remaining.get_mut(chosen) {
            *slot -= 1;
        }
        if let Some(value) = VALUES.get(chosen) {
            out.push(*value);
        }
        total -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::histogram::{BENCH_MATCHES, COLUMN_ROWS, FULL_MATCHES, PREFIX_ROWS, THRESHOLD};

    /// Counts how many of the column's values are at least the threshold.
    fn matches(values: &[f64]) -> u64 {
        values.iter().filter(|&&value| value >= THRESHOLD).count() as u64
    }

    #[test]
    fn the_full_column_matches_the_native_column() {
        let column = build(COLUMN_ROWS);
        assert_eq!(column.len(), COLUMN_ROWS);
        assert_eq!(matches(&column), FULL_MATCHES);
    }

    /// The number the browser publishes is the native's, exactly.
    #[test]
    fn the_benchmark_column_matches_the_native_benchmark() {
        let column = build(20_000_000);
        assert_eq!(column.len(), 20_000_000);
        assert_eq!(matches(&column), BENCH_MATCHES);
    }

    /// The multiset is exact: every value occurs exactly as many times as
    /// the histogram says, in the canonical column.
    #[test]
    fn the_multiset_is_exact() {
        let column = build(20_000_000);
        for ((value, full), prefix) in
            VALUES.iter().zip(crate::histogram::FULL.iter()).zip(crate::histogram::PREFIX.iter())
        {
            // The values are compared **bit for bit**: the multiset is
            // exact, so a value that came out one bit different would be a
            // different value, and the counts would not add up.
            let count = column.iter().filter(|v| v.to_bits() == value.to_bits()).count();
            // 3 x the whole column plus the prefix.
            let expected = 3 * u64::from(*full) + u64::from(*prefix);
            assert_eq!(count as u64, expected, "value {value}");
        }
    }

    /// The order is not sorted: a column filled in value order would give
    /// the oracle's branch an unrealistically easy time.
    #[test]
    fn the_order_is_not_sorted() {
        let column = build(PREFIX_ROWS);
        let sorted = {
            let mut copy = column.clone();
            copy.sort_by(|a, b| a.partial_cmp(b).expect("no NaN in the column"));
            copy
        };
        assert_ne!(column, sorted, "the column came out in value order");
    }

    /// The column is the same on every run, on every machine.
    #[test]
    fn the_column_is_deterministic() {
        assert_eq!(build(10_000), build(10_000));
    }
}
