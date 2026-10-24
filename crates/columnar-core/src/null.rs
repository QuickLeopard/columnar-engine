//! Validity masks.
//!
//! A NULL in this engine is recorded by the *absence of a bit*, never by a
//! sentinel value inside the data array. This is the whole reason the
//! comparison kernels can be branchless on the null path: the value array is
//! dense and always holds a real value for every row, and the validity mask
//! says which of those values the predicate is allowed to look at.
//!
//! ## Bit layout
//!
//! Row `i` lives in word `i / 64` at bit `i % 64`, least-significant-bit
//! first. This is the layout `DuckDB`, `Arrow` and the Parquet validity encoding
//! all use, and matching it means the mask can be handed to a comparator
//! without a repack. The layout is part of the crate's contract: changing it
//! changes the bytes a published hash covers.

/// A validity mask over `len` rows.
///
/// A set bit means "this row has a value". A clear bit means NULL.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NullBitmap {
    /// Bit-packed validity, LSB-first within each word.
    words: Vec<u64>,
    /// Number of rows this mask describes.
    len: usize,
    /// Number of rows whose bit is clear.
    null_count: usize,
}

impl NullBitmap {
    /// Number of 64-bit words needed to hold `len` bits.
    #[must_use]
    pub const fn words_for(len: usize) -> usize {
        len.div_ceil(64)
    }

    /// A mask in which every row is valid.
    #[must_use]
    pub fn all_valid(len: usize) -> Self {
        let mut words = vec![u64::MAX; Self::words_for(len)];
        // Clear the padding bits in the last word so that `words` is a
        // canonical representation: two masks describing the same rows always
        // compare equal, byte for byte.
        let rem = len % 64;
        if rem != 0 {
            if let Some(last) = words.last_mut() {
                *last = (1u64 << rem) - 1;
            }
        }
        Self { words, len, null_count: 0 }
    }

    /// A mask in which the listed rows are NULL and every other row is valid.
    ///
    /// # Panics
    ///
    /// Panics if any index is out of range for `len`.
    #[must_use]
    pub fn with_nulls(len: usize, null_rows: &[usize]) -> Self {
        let mut bitmap = Self::all_valid(len);
        for &row in null_rows {
            assert!(row < len, "null row {row} out of range for len {len}");
            bitmap.clear(row);
        }
        bitmap
    }

    /// Builds a mask from raw bit-packed words.
    ///
    /// `null_count` is computed rather than trusted, so a caller cannot
    /// construct an internally inconsistent mask. Words beyond `len` are
    /// masked off, so the resulting mask is always canonical.
    #[must_use]
    pub fn from_words(words: &[u64], len: usize) -> Self {
        let mut owned = words.to_vec();
        owned.resize(Self::words_for(len), 0);
        let rem = len % 64;
        if rem != 0 {
            if let Some(last) = owned.last_mut() {
                *last &= (1u64 << rem) - 1;
            }
        }
        let null_count = (0..len).filter(|&i| !Self::bit_is_set(&owned, i)).count();
        Self { words: owned, len, null_count }
    }

    /// A mask in which every row is NULL.
    #[must_use]
    pub fn all_null(len: usize) -> Self {
        Self { words: vec![0; Self::words_for(len)], len, null_count: len }
    }

    /// Number of rows this mask describes.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// True when the mask describes no rows.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Number of rows that are NULL.
    #[must_use]
    pub const fn null_count(&self) -> usize {
        self.null_count
    }

    /// True when the row at `row` holds a value.
    ///
    /// # Panics
    ///
    /// Panics if `row` is out of range.
    #[must_use]
    pub fn is_valid(&self, row: usize) -> bool {
        assert!(row < self.len, "row {row} out of range for len {}", self.len);
        Self::bit_is_set(&self.words, row)
    }

    /// Sets the validity bit for `row`.
    ///
    /// # Panics
    ///
    /// Panics if `row` is out of range.
    pub fn set_valid(&mut self, row: usize) {
        assert!(row < self.len, "row {row} out of range for len {}", self.len);
        let word = row / 64;
        let bit = row % 64;
        let was_null = self.words[word] & (1u64 << bit) == 0;
        self.words[word] |= 1u64 << bit;
        if was_null {
            self.null_count -= 1;
        }
    }

    /// Clears the validity bit for `row`, marking it NULL.
    ///
    /// # Panics
    ///
    /// Panics if `row` is out of range.
    pub fn clear(&mut self, row: usize) {
        assert!(row < self.len, "row {row} out of range for len {}", self.len);
        let word = row / 64;
        let bit = row % 64;
        let was_valid = self.words[word] & (1u64 << bit) != 0;
        self.words[word] &= !(1u64 << bit);
        if was_valid {
            self.null_count += 1;
        }
    }

    /// The raw bit-packed words.
    ///
    /// The slice is exactly `words_for(len)` long and the padding bits of the
    /// final word are zero, so the slice is a canonical encoding of the mask.
    #[must_use]
    pub fn words(&self) -> &[u64] {
        &self.words
    }

    /// Iterates over the rows that hold a value.
    pub fn valid_rows(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.len).filter(|&i| Self::bit_is_set(&self.words, i))
    }

    /// Reads a bit out of a raw word slice.
    #[must_use]
    pub(crate) fn bit_is_set(words: &[u64], row: usize) -> bool {
        words[row / 64] & (1u64 << (row % 64)) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::NullBitmap;

    #[test]
    fn all_valid_has_no_nulls() {
        let bitmap = NullBitmap::all_valid(130);
        assert_eq!(bitmap.len(), 130);
        assert_eq!(bitmap.null_count(), 0);
        assert!(bitmap.is_valid(0));
        assert!(bitmap.is_valid(129));
    }

    #[test]
    fn with_nulls_marks_exactly_the_listed_rows() {
        let bitmap = NullBitmap::with_nulls(130, &[0, 63, 64, 129]);
        assert_eq!(bitmap.null_count(), 4);
        assert!(!bitmap.is_valid(0));
        assert!(bitmap.is_valid(1));
        assert!(!bitmap.is_valid(63));
        assert!(!bitmap.is_valid(64));
        assert!(bitmap.is_valid(128));
        assert!(!bitmap.is_valid(129));
    }

    #[test]
    fn padding_bits_are_zeroed_so_masks_are_canonical() {
        let bitmap = NullBitmap::all_valid(3);
        assert_eq!(bitmap.words(), &[0b111]);
    }

    #[test]
    fn from_words_recomputes_the_null_count() {
        // A caller claiming zero nulls while passing a mask with a clear bit
        // must not be believed.
        let bitmap = NullBitmap::from_words(&[0b0101], 3);
        assert_eq!(bitmap.null_count(), 1);
        assert!(bitmap.is_valid(0));
        assert!(!bitmap.is_valid(1));
        assert!(bitmap.is_valid(2));
    }

    #[test]
    fn set_and_clear_keep_the_count_consistent() {
        let mut bitmap = NullBitmap::all_valid(10);
        bitmap.clear(4);
        assert_eq!(bitmap.null_count(), 1);
        bitmap.set_valid(4);
        assert_eq!(bitmap.null_count(), 0);
    }

    #[test]
    fn valid_rows_yields_exactly_the_valid_rows() {
        let bitmap = NullBitmap::with_nulls(8, &[2, 5]);
        let rows: Vec<usize> = bitmap.valid_rows().collect();
        assert_eq!(rows, vec![0, 1, 3, 4, 6, 7]);
    }
}
