//! Selection vectors.
//!
//! A selection vector is the set of rows that survived the previous operator.
//! It has three representations, and the engine picks between them at runtime:
//!
//! - **Index** -- the list of surviving row numbers, `u32` each. Compact when
//!   few rows survive.
//! - **Bitmap** -- one bit per row of the source batch. Compact when many
//!   rows survive, and O(1) to test membership.
//! - **All** -- every row of the batch. A batch of a million rows is 128 KiB
//!   as a bitmap and nothing at all as a range, and the range is what the
//!   rows actually are.
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
//!
//! ## The rows are read, not copied
//!
//! A selection that holds every row of a batch is **the range**, and reading
//! its rows costs nothing at all: there is no bitmap to walk and no vector of
//! row numbers to build. A selection that holds some of them is read through
//! [`SelectionVector::iter`], which walks the index list or the bitmap
//! directly -- a row at a time, and never as a vector of them.

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
    /// Every row of the source batch.
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Repr {
    Index(Vec<u32>),
    Bitmap(Vec<u64>),
    /// Every row of a batch of `usize` rows.
    All(usize),
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
    ///
    /// The rows are the range `0..total`, and the selection says so: there
    /// is no bitmap to walk and no vector of row numbers to build. A
    /// selection that holds every row is the cheapest one there is, and it
    /// is the one a plan with no predicate is handed.
    #[must_use]
    pub fn all(total: usize) -> Self {
        Self { repr: Repr::All(total), count: total, total }
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
            Repr::All(_) => SelectionKind::All,
        }
    }

    /// The surviving row numbers, materialised.
    ///
    /// For a bitmap-backed selection this walks the words; for an index-backed
    /// one it clones the row list. Callers on the hot path should prefer
    /// [`SelectionVector::iter`] or [`SelectionVector::for_each`], which do
    /// not allocate.
    #[must_use]
    pub fn rows(&self) -> Vec<u32> {
        match &self.repr {
            Repr::Index(rows) => rows.clone(),
            Repr::Bitmap(words) => Self::rows_from_words(words, self.total),
            Repr::All(total) => (0..u32::try_from(*total).unwrap_or(u32::MAX)).collect(),
        }
    }

    /// The rows, as a range, when the selection holds every row.
    ///
    /// This is the selection that holds them all, and its rows are the
    /// range -- there is nothing to walk and nothing to build. A consumer
    /// that reads the rows in a hot loop takes the range rather than an
    /// iterator, so that the loop's own state stays where it can be kept.
    #[must_use]
    pub fn all_rows(&self) -> Option<std::ops::Range<u32>> {
        match self.repr {
            Repr::All(total) => Some(0..u32::try_from(total).unwrap_or(u32::MAX)),
            _ => None,
        }
    }

    /// The surviving rows, one at a time.
    ///
    /// This is how a selection's rows are read when they are read once: the
    /// index list is walked directly, the bitmap is walked a set bit at a
    /// time, and the selection that holds every row is the range. Nothing is
    /// allocated, and no vector of row numbers is built.
    ///
    /// The rows come in ascending order, as everywhere else.
    #[must_use]
    pub fn iter(&self) -> SelectionIter<'_> {
        match &self.repr {
            Repr::Index(rows) => SelectionIter { inner: SelectionIterInner::Index(rows.iter()) },
            Repr::Bitmap(words) => SelectionIter {
                inner: SelectionIterInner::Bitmap(BitmapWalk {
                    words,
                    next_word: 0,
                    base: 0,
                    bits: 0,
                    total: self.total,
                }),
            },
            Repr::All(total) => SelectionIter {
                inner: SelectionIterInner::All(0..u32::try_from(*total).unwrap_or(u32::MAX)),
            },
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
            Repr::All(total) => usize::try_from(row).is_ok_and(|row| row < *total),
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
            Repr::All(total) => (0..u32::try_from(*total).unwrap_or(u32::MAX)).for_each(f),
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

    /// Visits each surviving row whose number is in `start..end`.
    ///
    /// This is what lets a gather be split across threads: each worker takes
    /// a contiguous range of the **source** rows and gathers the survivors in
    /// it. The ranges are disjoint and cover the source, so every survivor is
    /// visited exactly once and the workers' outputs concatenate into the
    /// same rows the serial gather would have produced.
    ///
    /// The rows are visited in ascending order, as everywhere else.
    pub fn for_each_in_range(&self, start: usize, end: usize, mut f: impl FnMut(u32)) {
        if start >= end {
            return;
        }
        match &self.repr {
            Repr::Index(rows) => {
                // The rows are sorted, so the first one in range is found by a
                // binary search rather than by a scan.
                let first = rows.partition_point(|&row| (row as usize) < start);
                for &row in rows.get(first..).unwrap_or(&[]) {
                    if (row as usize) >= end {
                        break;
                    }
                    f(row);
                }
            }
            Repr::Bitmap(words) => {
                let first_word = start / 64;
                let last_word = end.div_ceil(64);
                for word_index in first_word..last_word {
                    let Some(&word) = words.get(word_index) else { break };
                    let mut bits = word;
                    while bits != 0 {
                        let bit = bits.trailing_zeros();
                        let row = word_index * 64 + bit as usize;
                        if row >= end {
                            break;
                        }
                        if row >= start {
                            f(row as u32);
                        }
                        bits &= bits - 1;
                    }
                }
            }
            Repr::All(total) => {
                let end = end.min(*total);
                let start = start.min(end);
                for row in start..end {
                    f(row as u32);
                }
            }
        }
    }

    /// How many surviving rows are in the source-row range `start..end`.
    #[must_use]
    pub fn count_in_range(&self, start: usize, end: usize) -> usize {
        let mut count = 0usize;
        self.for_each_in_range(start, end, |_| count += 1);
        count
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

/// An iterator over the rows of a [`SelectionVector`].
///
/// The rows are read as the selection holds them: an index list is walked
/// directly, a bitmap is walked a set bit at a time, and the selection that
/// holds every row is the range. Nothing is allocated along the way, and no
/// vector of row numbers is built.
#[derive(Debug, Clone)]
pub struct SelectionIter<'a> {
    /// Which representation the walk is over.
    inner: SelectionIterInner<'a>,
}

/// The representation a [`SelectionIter`] walks.
#[derive(Debug, Clone)]
enum SelectionIterInner<'a> {
    /// The index list, walked directly.
    Index(std::slice::Iter<'a, u32>),
    /// The bitmap, walked a set bit at a time.
    Bitmap(BitmapWalk<'a>),
    /// Every row of the batch.
    All(std::ops::Range<u32>),
}

/// A walk over a bitmap's set bits.
#[derive(Debug, Clone)]
struct BitmapWalk<'a> {
    /// The bitmap's words.
    words: &'a [u64],
    /// The word the walk reads next.
    next_word: usize,
    /// The row number of the word being walked.
    base: usize,
    /// The bits of that word that are still to come.
    bits: u64,
    /// The number of rows in the batch.
    total: usize,
}

impl BitmapWalk<'_> {
    /// The next set row, if there is one.
    fn next_row(&mut self) -> Option<u32> {
        loop {
            if self.bits == 0 {
                let index = self.next_word;
                let word = self.words.get(index)?;
                self.next_word = index + 1;
                self.base = index * 64;
                self.bits = *word;
                continue;
            }
            let bit = self.bits.trailing_zeros() as usize;
            self.bits &= self.bits - 1;
            let row = self.base + bit;
            if row >= self.total {
                // The padding bits of the last word are not rows.
                self.bits = 0;
                return None;
            }
            return Some(row as u32);
        }
    }
}

impl Iterator for SelectionIter<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            SelectionIterInner::Index(rows) => rows.next().copied(),
            SelectionIterInner::Bitmap(walk) => walk.next_row(),
            SelectionIterInner::All(range) => range.next(),
        }
    }
}

impl<'a> IntoIterator for &'a SelectionVector {
    type Item = u32;
    type IntoIter = SelectionIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::{SelectionKind, SelectionVector};

    #[test]
    fn the_selection_that_holds_every_row_is_the_range() {
        let selection = SelectionVector::all(100);
        assert_eq!(selection.kind(), SelectionKind::All);
        assert_eq!(selection.len(), 100);
        assert!((selection.density() - 1.0).abs() < 1e-12, "density was {}", selection.density());
        assert_eq!(selection.iter().collect::<Vec<u32>>(), (0..100).collect::<Vec<u32>>());
        assert_eq!(selection.rows(), (0..100).collect::<Vec<u32>>());
    }

    #[test]
    fn the_iterator_agrees_with_the_rows() {
        // The index representation: a list of the surviving rows.
        let mut words = vec![0u64; 2];
        for row in [3usize, 17, 22, 41, 96] {
            words[row / 64] |= 1u64 << (row % 64);
        }
        let index = SelectionVector::from_filter(&words, 5, 100);
        assert_eq!(index.kind(), SelectionKind::Index);
        assert_eq!(index.iter().collect::<Vec<u32>>(), index.rows());

        // The bitmap representation: one bit per row.
        let mut words = vec![0u64; 2];
        for row in 0..60usize {
            words[row / 64] |= 1u64 << (row % 64);
        }
        let bitmap = SelectionVector::from_filter(&words, 60, 100);
        assert_eq!(bitmap.kind(), SelectionKind::Bitmap);
        assert_eq!(bitmap.iter().collect::<Vec<u32>>(), bitmap.rows());
    }

    #[test]
    fn the_iterator_stops_at_the_end_of_the_batch() {
        // The padding bits of the last word are set, and they are not rows.
        let words = vec![u64::MAX; 2];
        let selection = SelectionVector::from_filter(&words, 100, 100);
        let rows = selection.iter().collect::<Vec<u32>>();
        assert_eq!(rows.len(), 100);
        assert_eq!(rows[99], 99);
    }

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
    fn a_range_visits_exactly_the_rows_in_it() {
        let mask: Vec<u64> = vec![0b1010_1010, 0b1111_0000, 0b0000_0011];
        let selection = SelectionVector::from_filter(&mask, 10, 192);

        let mut all = Vec::new();
        selection.for_each_in_range(0, 192, |row| all.push(row));
        assert_eq!(all, selection.collect_rows());

        for (start, end) in [(0usize, 192usize), (0, 1), (1, 2), (63, 65), (64, 128), (100, 150)] {
            let mut rows = Vec::new();
            selection.for_each_in_range(start, end, |row| rows.push(row));
            let expected: Vec<u32> = selection
                .collect_rows()
                .into_iter()
                .filter(|&row| (row as usize) >= start && (row as usize) < end)
                .collect();
            assert_eq!(rows, expected, "the range {start}..{end} is wrong");
            assert_eq!(selection.count_in_range(start, end), rows.len());
        }
    }

    #[test]
    fn the_ranges_cover_every_row_exactly_once() {
        let mask: Vec<u64> = vec![0xFFFF_FFFF_FFFF_FFFF, 0x0000_0000_FFFF_0000];
        let selection = SelectionVector::from_filter(&mask, 80, 128);
        let mut seen = Vec::new();
        for (start, end) in [(0usize, 32usize), (32, 64), (64, 96), (96, 128)] {
            selection.for_each_in_range(start, end, |row| seen.push(row));
        }
        assert_eq!(seen, selection.collect_rows());
    }

    #[test]
    fn an_empty_range_visits_nothing() {
        let selection = SelectionVector::from_filter(&[0b1010], 2, 8);
        let mut rows = Vec::new();
        selection.for_each_in_range(3, 3, |row| rows.push(row));
        assert_eq!(rows.len(), 0);
        assert_eq!(selection.count_in_range(3, 3), 0);
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
        assert_eq!(selection.len(), 0);
    }
}
