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

    /// The conjunction of two masks.
    ///
    /// A row is valid in the result when it is valid in **both** masks. This is
    /// what an expression's operands combine to: a row whose expression reads
    /// a NULL is a NULL row, so the validity of the result is the validity of
    /// the left operand `AND`ed with the validity of the right one.
    ///
    /// The masks must describe the same number of rows. Two masks of different
    /// lengths would have to be combined over some third length, and there is
    /// no third length that is obviously right — so this panics rather than
    /// guessing.
    ///
    /// # Panics
    ///
    /// Panics if the two masks have different lengths.
    #[must_use]
    pub fn and(&self, other: &Self) -> Self {
        assert_eq!(
            self.len, other.len,
            "two validity masks of different lengths cannot be conjoined"
        );
        let words: Vec<u64> =
            self.words.iter().zip(other.words.iter()).map(|(left, right)| left & right).collect();
        Self::from_words(&words, self.len)
    }

    /// Concatenates two masks.
    ///
    /// The second mask's rows continue the first's, so a first mask that does
    /// not end on a word boundary has the second one's bits **shifted** into
    /// place. That shift is the whole reason this is a method and not two
    /// `extend` calls: the bits are packed, not aligned, and a mask
    /// concatenated at the wrong offset describes different rows.
    ///
    /// The result is canonical: the padding bits of the last word are clear,
    /// so two concatenations that describe the same rows compare equal.
    #[must_use]
    pub fn concat(&self, other: &Self) -> Self {
        let total = self.len + other.len();
        let mut words = vec![0u64; Self::words_for(total)];

        // The first mask's own words, **in full**. The last of them may be a
        // partial word, and it is exactly the word the second mask's bits
        // land in -- so copying only the complete words would drop it, and
        // the second mask's rows would land on the wrong ones.
        words[..self.words.len()].copy_from_slice(&self.words);

        let base = self.len / 64;
        let shift = self.len % 64;
        if shift == 0 {
            for (index, &word) in other.words.iter().enumerate() {
                words[base + index] |= word;
            }
        } else {
            // The second mask starts mid-word: each of its words splits
            // across two of the result's.
            for (index, &word) in other.words.iter().enumerate() {
                let low = word & ((1u64 << (64 - shift)) - 1);
                let high = word >> (64 - shift);
                words[base + index] |= low << shift;
                if let Some(slot) = words.get_mut(base + index + 1) {
                    *slot |= high;
                }
            }
        }

        Self::from_words(&words, total)
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

    #[test]
    fn and_keeps_only_the_rows_both_masks_hold() {
        let left = NullBitmap::with_nulls(8, &[1, 2]);
        let right = NullBitmap::with_nulls(8, &[2, 3]);
        let joined = left.and(&right);
        assert_eq!(joined.null_count(), 3);
        assert!(joined.is_valid(0));
        assert!(!joined.is_valid(1));
        assert!(!joined.is_valid(2));
        assert!(!joined.is_valid(3));
        assert!(joined.is_valid(4));
    }

    #[test]
    fn and_of_a_mask_with_itself_is_itself() {
        let mask = NullBitmap::with_nulls(70, &[0, 69]);
        assert_eq!(mask.and(&mask), mask);
    }

    #[test]
    fn and_of_two_all_valid_masks_is_all_valid() {
        let left = NullBitmap::all_valid(65);
        let right = NullBitmap::all_valid(65);
        assert_eq!(left.and(&right), left);
    }

    #[test]
    #[should_panic(expected = "different lengths")]
    fn and_of_two_masks_of_different_lengths_is_refused() {
        let left = NullBitmap::all_valid(4);
        let right = NullBitmap::all_valid(5);
        let _ = left.and(&right);
    }

    #[test]
    fn concat_of_two_empty_masks_is_empty() {
        let empty = NullBitmap::all_valid(0);
        let joined = empty.concat(&empty);
        assert_eq!(joined.len(), 0);
        assert_eq!(joined.null_count(), 0);
        assert_eq!(joined.words(), &[] as &[u64]);
    }

    #[test]
    fn concat_puts_the_second_masks_rows_after_the_firsts() {
        // The first mask ends mid-word, so the second mask's bits have to be
        // shifted into place rather than simply appended.
        let left = NullBitmap::with_nulls(5, &[1]);
        let right = NullBitmap::with_nulls(3, &[0]);
        let joined = left.concat(&right);

        assert_eq!(joined.len(), 8);
        assert_eq!(joined.null_count(), 2);
        assert!(!joined.is_valid(1), "the first mask's NULL moved");
        assert!(!joined.is_valid(5), "the second mask's NULL did not land on row 5");
        assert!(joined.is_valid(0));
        assert!(joined.is_valid(2));
        assert!(joined.is_valid(4));
        assert!(joined.is_valid(6));
        assert!(joined.is_valid(7));
    }

    #[test]
    fn concat_at_a_word_boundary_needs_no_shift() {
        let left = NullBitmap::with_nulls(64, &[63]);
        let right = NullBitmap::with_nulls(64, &[0]);
        let joined = left.concat(&right);

        assert_eq!(joined.len(), 128);
        assert_eq!(joined.null_count(), 2);
        assert!(!joined.is_valid(63));
        assert!(!joined.is_valid(64));
        assert!(joined.is_valid(65));
        assert!(joined.is_valid(127));
    }

    #[test]
    fn concat_is_the_same_as_one_mask_over_the_same_rows() {
        // The property the parallel merge relies on: concatenating the parts
        // must describe exactly the rows the single mask describes.
        for left_len in [0usize, 1, 63, 64, 65, 127, 128, 129] {
            for right_len in [0usize, 1, 63, 64, 65] {
                let left_nulls: Vec<usize> = (0..left_len).filter(|row| row % 3 == 0).collect();
                let right_nulls: Vec<usize> = (0..right_len).filter(|row| row % 5 == 0).collect();
                let left = NullBitmap::with_nulls(left_len, &left_nulls);
                let right = NullBitmap::with_nulls(right_len, &right_nulls);

                let joined = left.concat(&right);
                assert_eq!(joined.len(), left_len + right_len);

                let mut expected = left_nulls.clone();
                expected.extend(right_nulls.iter().map(|row| row + left_len));
                assert_eq!(
                    joined.null_count(),
                    expected.len(),
                    "left {left_len}, right {right_len}"
                );
                for row in 0..(left_len + right_len) {
                    assert_eq!(
                        joined.is_valid(row),
                        !expected.contains(&row),
                        "row {row} differs, left {left_len}, right {right_len}"
                    );
                }
            }
        }
    }

    #[test]
    fn concat_is_associative() {
        let a = NullBitmap::with_nulls(37, &[0, 36]);
        let b = NullBitmap::with_nulls(11, &[10]);
        let c = NullBitmap::with_nulls(80, &[79, 0]);

        assert_eq!(a.concat(&b).concat(&c), a.concat(&b.concat(&c)));
    }

    #[test]
    fn concat_produces_a_canonical_mask() {
        // The padding bits of the last word must be clear, so two masks
        // describing the same rows compare equal. Here the result is 63 rows
        // long, so bit 63 is padding and has to be clear.
        let left = NullBitmap::with_nulls(3, &[1]);
        let right = NullBitmap::with_nulls(60, &[59]);
        let joined = left.concat(&right);

        assert_eq!(joined.len(), 63);
        assert_eq!(joined.words().len(), 1);
        assert_eq!(joined.words()[0] & (1u64 << 63), 0, "the padding bits are not clear");
        assert!(!joined.is_valid(1), "the first mask's NULL moved");
        assert!(!joined.is_valid(62), "the second mask's NULL did not land on row 62");
    }
}
