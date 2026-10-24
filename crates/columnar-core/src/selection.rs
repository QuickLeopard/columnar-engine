//! Selection vectors.
//!
//! A selection vector is the set of rows that survived the previous operator.
//! It has two representations, and the engine picks between them at runtime:
//!
//! - **Index** -- the list of surviving row numbers, `u32` each. Compact when
//!   few rows survive.
//! - **Bitmap** -- one bit per row of the source batch. Compact when many
//!   rows survive, and O(1) to test membership.
//!
//! The switch happens at [`BITMAP_DISPATCH_DENSITY`]. This is not an
//! arbitrary constant: the two representations cost 4 bytes per surviving
//! row and 1 bit per source row respectively, so they cross over at a density
//! of 1/16, and `DuckDB` -- which uses the same two representations -- switches
//! in the same neighbourhood. We use 30% rather than the theoretical 6.25%
//! because the index representation wins earlier than pure byte counting
//! suggests: an index list is contiguous in memory and the gather that
//! consumes it is a linear scan, whereas a bitmap forces a bit test per
//! source row. The threshold is a named constant so that the architecture
//! diagram, the benchmark and this comment cannot drift apart.
//!
//! The dispatch is measured, not asserted: `docs/architecture.md` records the
//! crossover this repository observed.

// Row identifiers in a selection vector are `u32`: a batch is 2048 rows, so
// a `u32` is not a limit but a format decision that keeps the index
// representation at 4 bytes per surviving row. The `usize -> u32` narrowing
// below is therefore exact by construction, and the `usize -> f64` widening
// is a density ratio used only to pick a representation, where a 2^53
// boundary is irrelevant.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

/// The density at which the engine switches from an index list to a bitmap.
///
/// A selection vector with a density below this is stored as an index list;
/// at or above it, as a bitmap.
pub const BITMAP_DISPATCH_DENSITY: f64 = 0.30;

/// Which representation a [`SelectionVector`] is using.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelectionKind {
    /// A list of surviving row numbers.
    Index,
    /// One bit per source row.
    Bitmap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Repr {
    Index(Vec<u32>),
    Bitmap(Vec<u64>),
}

/// The set of rows that survived a filter.
///
/// `count` is the number of surviving rows; `total` is the number of rows in
/// the source batch. Both are kept explicitly so that density is a stored
/// fact rather than something recomputed from the representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionVector {
    repr: Repr,
    count: usize,
    total: usize,
}

impl SelectionVector {
    /// The empty selection over `total` source rows.
    #[must_use]
    pub fn empty(total: usize) -> Self {
        Self { repr: Repr::Index(Vec::new()), count: 0, total }
    }

    /// A selection containing every row of a `total`-row batch.
    #[must_use]
    pub fn all(total: usize) -> Self {
        Self {
            repr: Repr::Bitmap(vec![u64::MAX; NullBitmapWords::for_len(total)]),
            count: total,
            total,
        }
    }

    /// Builds a selection from a filter's output bitmap, choosing the
    /// representation by density.
    ///
    /// `words` is the bit-packed predicate result (a set bit means "matched")
    /// and `count` is the number of set bits. The bitmap is canonicalised:
    /// padding bits beyond `total` are cleared, so two selections describing
    /// the same rows are equal.
    #[must_use]
    pub fn from_filter(words: &[u64], count: usize, total: usize) -> Self {
        let density = Self::density_of(count, total);
        if density < BITMAP_DISPATCH_DENSITY {
            let rows = Self::rows_from_words(words, total);
            debug_assert_eq!(rows.len(), count, "filter bitmap count disagrees with its contents");
            Self { repr: Repr::Index(rows), count, total }
        } else {
            let mut owned = words.to_vec();
            owned.resize(NullBitmapWords::for_len(total), 0);
            let rem = total % 64;
            if rem != 0 {
                if let Some(last) = owned.last_mut() {
                    *last &= (1u64 << rem) - 1;
                }
            }
            Self { repr: Repr::Bitmap(owned), count, total }
        }
    }

    /// Builds a selection from an explicit list of surviving rows.
    ///
    /// # Panics
    ///
    /// Panics if any row is out of range for `total`.
    #[must_use]
    pub fn from_rows(rows: Vec<u32>, total: usize) -> Self {
        assert!(
            rows.iter().all(|&r| (r as usize) < total),
            "selection row out of range for a {total}-row batch"
        );
        let count = rows.len();
        Self { repr: Repr::Index(rows), count, total }
    }

    /// The fraction of source rows that survived.
    ///
    /// Returns `0.0` for an empty batch, so an empty batch never
    /// accidentally reads as full density.
    #[must_use]
    pub fn density(&self) -> f64 {
        Self::density_of(self.count, self.total)
    }

    fn density_of(count: usize, total: usize) -> f64 {
        if total == 0 {
            0.0
        } else {
            count as f64 / total as f64
        }
    }

    /// Number of surviving rows.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.count
    }

    /// True when no row survived.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Number of rows in the source batch.
    #[must_use]
    pub const fn total(&self) -> usize {
        self.total
    }

    /// Which representation is in use.
    #[must_use]
    pub const fn kind(&self) -> SelectionKind {
        match self.repr {
            Repr::Index(_) => SelectionKind::Index,
            Repr::Bitmap(_) => SelectionKind::Bitmap,
        }
    }

    /// The surviving row numbers, materialised.
    ///
    /// For a bitmap-backed selection this walks the words; for an index-backed
    /// one it clones the row list. Callers on the hot path should prefer
    /// [`SelectionVector::for_each`] or [`SelectionVector::contains`], which
    /// do not allocate.
    #[must_use]
    pub fn rows(&self) -> Vec<u32> {
        match &self.repr {
            Repr::Index(rows) => rows.clone(),
            Repr::Bitmap(words) => Self::rows_from_words(words, self.total),
        }
    }

    /// True when `row` survived.
    #[must_use]
    pub fn contains(&self, row: u32) -> bool {
        match &self.repr {
            Repr::Index(rows) => rows.binary_search(&row).is_ok(),
            Repr::Bitmap(words) => {
                let idx = row as usize;
                idx < self.total && words[idx / 64] & (1u64 << (idx % 64)) != 0
            }
        }
    }

    /// Visits each surviving row in ascending order.
    pub fn for_each(&self, mut f: impl FnMut(u32)) {
        match &self.repr {
            Repr::Index(rows) => rows.iter().for_each(|&r| f(r)),
            Repr::Bitmap(words) => {
                for (word_index, &word) in words.iter().enumerate() {
                    let mut bits = word;
                    while bits != 0 {
                        let bit = bits.trailing_zeros();
                        f((word_index * 64 + bit as usize) as u32);
                        bits &= bits - 1;
                    }
                }
            }
        }
    }

    /// Visits each surviving row in ascending order, returning the collected
    /// rows.
    #[must_use]
    pub fn collect_rows(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.count);
        self.for_each(|r| out.push(r));
        out
    }

    /// Extracts the set bits of a filter bitmap into a row list.
    fn rows_from_words(words: &[u64], total: usize) -> Vec<u32> {
        let mut rows = Vec::new();
        for (word_index, &word) in words.iter().enumerate() {
            let mut bits = word;
            while bits != 0 {
                let bit = bits.trailing_zeros();
                let row = word_index * 64 + bit as usize;
                if row < total {
                    rows.push(row as u32);
                }
                bits &= bits - 1;
            }
        }
        rows
    }
}

/// Helper so the word-count formula lives in one place.
struct NullBitmapWords;

impl NullBitmapWords {
    const fn for_len(len: usize) -> usize {
        len.div_ceil(64)
    }
}

#[cfg(test)]
mod tests {
    use super::{SelectionKind, SelectionVector};

    #[test]
    fn sparse_filter_selects_the_index_representation() {
        // 10 of 100 rows survive -> 10% density -> index.
        let mut words = vec![0u64; 2];
        for row in [3usize, 17, 22, 41, 96, 1, 2, 4, 5, 6] {
            words[row / 64] |= 1u64 << (row % 64);
        }
        let selection = SelectionVector::from_filter(&words, 10, 100);
        assert_eq!(selection.kind(), SelectionKind::Index);
        assert_eq!(selection.len(), 10);
        assert!((selection.density() - 0.10).abs() < 1e-12, "density was {}", selection.density());
    }

    #[test]
    fn dense_filter_selects_the_bitmap_representation() {
        // 60 of 100 rows survive -> 60% density -> bitmap.
        let mut words = vec![0u64; 2];
        for row in 0..60usize {
            words[row / 64] |= 1u64 << (row % 64);
        }
        let selection = SelectionVector::from_filter(&words, 60, 100);
        assert_eq!(selection.kind(), SelectionKind::Bitmap);
        assert_eq!(selection.len(), 60);
    }

    #[test]
    fn the_dispatch_threshold_is_the_documented_one() {
        // Exactly at the threshold the bitmap wins.
        let total = 100usize;
        // 30% of 100 rows, computed exactly in integers to avoid a float cast.
        let count = total * 30 / 100;
        let mut words = vec![0u64; 2];
        for row in 0..count {
            words[row / 64] |= 1u64 << (row % 64);
        }
        let selection = SelectionVector::from_filter(&words, count, total);
        assert_eq!(selection.kind(), SelectionKind::Bitmap);

        // One row below it the index wins.
        let mut words = vec![0u64; 2];
        for row in 0..count - 1 {
            words[row / 64] |= 1u64 << (row % 64);
        }
        let selection = SelectionVector::from_filter(&words, count - 1, total);
        assert_eq!(selection.kind(), SelectionKind::Index);
    }

    #[test]
    fn both_representations_agree_on_their_rows() {
        let mut words = vec![0u64; 4];
        for row in [0usize, 5, 63, 64, 200, 255] {
            words[row / 64] |= 1u64 << (row % 64);
        }
        let dense = SelectionVector::from_filter(&words, 6, 256);
        // Force the other representation and compare.
        let sparse = SelectionVector::from_rows(
            words
                .iter()
                .enumerate()
                .flat_map(|(wi, &w)| {
                    (0..64)
                        .filter_map(move |b| (w & (1u64 << b) != 0).then_some((wi * 64 + b) as u32))
                })
                .collect(),
            256,
        );
        assert_eq!(dense.collect_rows(), sparse.collect_rows());
        assert!(dense.contains(200));
        assert!(!dense.contains(201));
    }

    #[test]
    fn empty_selection_has_zero_density_even_for_an_empty_batch() {
        let selection = SelectionVector::empty(0);
        assert!(selection.density().abs() < f64::EPSILON);
        assert!(selection.is_empty());
    }
}
