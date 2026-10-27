//! The dictionary path, in one place.
//!
//! A dictionary-encoded column holds its distinct values once, and each row holds an index into
//! them. So a predicate over such a column is two pieces of work: the **distinct values** are
//! compared once each, and the answer is then **mapped back** through the indices.
//!
//! The pushdown — over the reader's arrays — and the filter — over the engine's own columns —
//! both do exactly this, and they did it in two places, with two copies of the same string
//! comparison and two copies of the same map. This module is the one implementation, so the two
//! paths cannot drift apart.
//!
//! The measurement behind it is spec 022's instrument
//! (`crates/columnar-engine/examples/dictionary_cost_probe.rs`).

use columnar_core::ComparisonOp;

/// The match of every distinct value, as one boolean a value.
///
/// The bounds are resolved **before** the loop over the values: a comparison per distinct value
/// must not match on an enum per distinct value. That match was the engine's own overhead on the
/// real Q6's predicate set, and it is the one thing this path did that the numeric paths do not.
pub fn distinct_matches<'a, I>(values: I, comparisons: &[(ComparisonOp, &str)]) -> Vec<bool>
where
    I: IntoIterator<Item = Option<&'a str>>,
    I::IntoIter: ExactSizeIterator,
{
    let values = values.into_iter();
    let mut matched = vec![true; values.len()];
    for (index, value) in values.enumerate() {
        let Some(value) = value else {
            matched[index] = false;
            continue;
        };
        matched[index] = comparisons.iter().all(|(op, bound)| compare_str(value, *op, bound));
    }
    matched
}

/// The dictionary's answer, as one boolean a dictionary entry.
///
/// The dictionary's answer is a bit mask, and the map wants a table: one boolean a entry, small
/// enough to live in the first-level cache, so the per-row work is a plain index.
pub fn mask_to_bools(mask: &[u64]) -> Vec<bool> {
    let entries = mask.len() * 64;
    (0..entries)
        .map(|entry| mask.get(entry / 64).is_some_and(|word| word & (1u64 << (entry % 64)) != 0))
        .collect()
}

/// The rows' mask, from the dictionary's answer.
///
/// Row `r` matches when the dictionary entry its index points at matched.
///
/// The map is branchless and accumulates a word at a time: a date column's dictionary indices run
/// in long stretches, so a conditional bit set is a mispredicted branch every time a run ends. The
/// engine's own path was rewritten for that reason and cost 41 ms before it was fixed.
pub fn map_mask(entry_matches: &[bool], indices: &[i32], rows: usize) -> Vec<u64> {
    let mut mask = vec![0u64; rows.div_ceil(64)];
    for (chunk_index, chunk) in indices[..rows.min(indices.len())].chunks(64).enumerate() {
        let mut word = 0u64;
        for (bit, &index) in chunk.iter().enumerate() {
            let entry = usize::try_from(index).unwrap_or(usize::MAX);
            let matched = u64::from(entry_matches.get(entry).copied().unwrap_or(false));
            word |= matched << bit;
        }
        if let Some(slot) = mask.get_mut(chunk_index) {
            *slot = word;
        }
    }
    mask
}

/// Compares two strings, byte-wise, as `DuckDB`'s binary collation does.
///
/// Rust's `str` ordering is byte-wise lexicographic, so the engine and `DuckDB` agree on the same
/// bytes.
pub fn compare_str(value: &str, op: ComparisonOp, threshold: &str) -> bool {
    match op {
        ComparisonOp::Lt => value < threshold,
        ComparisonOp::LtEq => value <= threshold,
        ComparisonOp::Gt => value > threshold,
        ComparisonOp::GtEq => value >= threshold,
        ComparisonOp::Eq => value == threshold,
        ComparisonOp::Ne => value != threshold,
    }
}

#[cfg(test)]
mod tests {
    use super::{compare_str, distinct_matches, map_mask, mask_to_bools};
    use columnar_core::ComparisonOp;

    #[test]
    fn the_distinct_values_are_compared_once_each() {
        let values = vec![Some("1994-01-01"), Some("1995-01-01"), None];
        let matched = distinct_matches(
            values,
            &[(ComparisonOp::GtEq, "1994-01-01"), (ComparisonOp::Lt, "1995-01-01")],
        );
        assert_eq!(matched, vec![true, false, false]);
    }

    #[test]
    fn the_map_follows_the_indices() {
        let matched = vec![true, false, true];
        let indices = [0i32, 1, 1, 2, 0];
        let mask = map_mask(&matched, &indices, indices.len());
        let rows: Vec<bool> =
            (0..5).map(|row| mask[row / 64] & (1u64 << (row % 64)) != 0).collect();
        assert_eq!(rows, vec![true, false, false, true, true]);
    }

    #[test]
    fn an_index_outside_the_dictionary_matches_nothing() {
        let matched = vec![true];
        let indices = [0i32, 7];
        let mask = map_mask(&matched, &indices, indices.len());
        assert_eq!(mask[0] & 0b11, 0b01);
    }

    #[test]
    fn a_bit_mask_becomes_a_table() {
        let mask = vec![0b1010u64];
        let table = mask_to_bools(&mask);
        assert_eq!(table.first(), Some(&false));
        assert_eq!(table.get(1), Some(&true));
        assert_eq!(table.get(2), Some(&false));
        assert_eq!(table.get(3), Some(&true));
        // The table is a word at a time, so it is never shorter than the mask.
        assert_eq!(table.len(), 64);
    }

    #[test]
    fn the_strings_compare_byte_wise() {
        assert!(compare_str("1994-01-02", ComparisonOp::Gt, "1994-01-01"));
        assert!(compare_str("1994-01-01", ComparisonOp::Lt, "1995-01-01"));
        assert!(compare_str("1994-01-01", ComparisonOp::Eq, "1994-01-01"));
        assert!(compare_str("1994-01-01", ComparisonOp::Ne, "1995-01-01"));
    }
}
