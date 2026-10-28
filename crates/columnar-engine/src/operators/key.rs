//! The keys a join and a sort read.
//!
//! Both operators need one thing the filter does not: a column's value, one
//! row at a time, at an arbitrary row. The filter always walks a column in
//! order, so it can work on the encoding directly; a join probes a row the
//! hash table points at, and a sort compares rows that are nowhere near each
//! other.
//!
//! The values are read **where they lie**. A column nobody asked to be
//! materialised is never copied, so a join's key costs a lookup rather than
//! a copy of the column -- and a dictionary-encoded column is read through
//! its dictionary, exactly as the gather would have copied it out of it.

use arrow::array::{
    Array, DictionaryArray, Float64Array, Int32Array, Int64Array, LargeStringArray, StringArray,
};
use arrow::datatypes::Int32Type;
use columnar_core::{BatchColumn, BatchData, ColumnChunk, Encoding, NullBitmap};

/// Where a probe's keys come from.
///
/// The probe reads a key a row at a time, and it reads it from one of two
/// places: the engine's own column, when the join runs over a materialised
/// dataset, and the reader's Arrow array, when the join runs **inside** the
/// scan. The lookup is written once, against this interface, so that the two
/// paths cannot answer differently.
///
/// See [ADR-0030](../../../docs/adr/0030-the-scan-that-feeds-the-join.md).
pub trait KeySource {
    /// How many rows the column holds.
    ///
    /// The length is what makes "the row is in range" a question with an
    /// answer. Without it, the two sources cannot say whether a row past the
    /// end holds a value, and one of them would **panic** on it while the
    /// other would not.
    fn len(&self) -> usize;

    /// Whether the column holds no rows at all.
    #[inline]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the column holds a NULL at all.
    ///
    /// A column with no NULLs is read without touching its validity bitmap.
    fn has_nulls(&self) -> bool;

    /// The column's plain integers, if it holds integers plainly.
    ///
    /// It is what opens the join's fast path: an integer key read straight
    /// out of the column, with no enum and no validity bitmap.
    fn integer_values(&self) -> Option<&[i64]>;

    /// The value at a row **in range**, if it holds one.
    ///
    /// A row that holds no value has no key: a NULL never matches anything,
    /// and it has no place in an order either.
    ///
    /// The row must be in range. Use [`KeySource::value_at`], which checks.
    fn value_at_in_range(&self, row: usize) -> Option<KeyValue<'_>>;

    /// The value at `row`, if it holds one.
    ///
    /// A row that holds no value has no key: a NULL never matches anything,
    /// and it has no place in an order either. A row **past the end** of the
    /// column has no value either.
    ///
    /// The range check is made **here**, once, for every source: the two
    /// paths cannot answer differently about a row that is not there.
    #[inline]
    fn value_at(&self, row: usize) -> Option<KeyValue<'_>> {
        if row >= self.len() {
            return None;
        }
        self.value_at_in_range(row)
    }
}

impl KeySource for KeyColumn<'_> {
    fn len(&self) -> usize {
        self.validity.len()
    }

    fn has_nulls(&self) -> bool {
        self.nulls
    }

    fn integer_values(&self) -> Option<&[i64]> {
        KeyColumn::integer_values(self)
    }

    fn value_at_in_range(&self, row: usize) -> Option<KeyValue<'_>> {
        KeyColumn::value_at_in_range(self, row)
    }
}

/// A key column read out of the reader's own Arrow array.
///
/// The values are read **where they lie**: a dictionary-encoded column is read
/// through its dictionary, exactly as the engine's own key column is. The
/// reader's array is never copied into the engine's own encoding to be probed
/// -- the copy is the thing this increment is about.
#[derive(Debug, Clone, Copy)]
pub enum ArrowKeys<'a> {
    /// Plain integers.
    Int64(&'a Int64Array),
    /// Plain doubles.
    Float64(&'a Float64Array),
    /// Plain strings.
    Utf8(&'a StringArray),
    /// Plain strings, in the wide encoding.
    LargeUtf8(&'a LargeStringArray),
    /// Dictionary-encoded strings.
    Dictionary {
        /// One index into the dictionary per row.
        keys: &'a Int32Array,
        /// The distinct values.
        values: &'a StringArray,
    },
    /// Dictionary-encoded strings, in the wide encoding.
    LargeDictionary {
        /// One index into the dictionary per row.
        keys: &'a Int32Array,
        /// The distinct values.
        values: &'a LargeStringArray,
    },
}

impl<'a> ArrowKeys<'a> {
    /// The key column in `array`, if the probe can read it.
    ///
    /// The types are the ones the pushdown reads exactly: integers, doubles
    /// and strings, plain or dictionary-encoded. A column of any other type
    /// is declined, and the caller takes the other path.
    #[must_use]
    pub fn new(array: &'a dyn Array) -> Option<Self> {
        if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
            return Some(Self::Int64(values));
        }
        if let Some(values) = array.as_any().downcast_ref::<Float64Array>() {
            return Some(Self::Float64(values));
        }
        if let Some(values) = array.as_any().downcast_ref::<StringArray>() {
            return Some(Self::Utf8(values));
        }
        if let Some(values) = array.as_any().downcast_ref::<LargeStringArray>() {
            return Some(Self::LargeUtf8(values));
        }
        let dictionary = array.as_any().downcast_ref::<DictionaryArray<Int32Type>>()?;
        let values = dictionary.values();
        if let Some(values) = values.as_any().downcast_ref::<StringArray>() {
            return Some(Self::Dictionary { keys: dictionary.keys(), values });
        }
        let values = values.as_any().downcast_ref::<LargeStringArray>()?;
        Some(Self::LargeDictionary { keys: dictionary.keys(), values })
    }
}

impl KeySource for ArrowKeys<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Int64(values) => values.len(),
            Self::Float64(values) => values.len(),
            Self::Utf8(values) => values.len(),
            Self::LargeUtf8(values) => values.len(),
            Self::Dictionary { keys, .. } | Self::LargeDictionary { keys, .. } => keys.len(),
        }
    }

    fn has_nulls(&self) -> bool {
        match self {
            Self::Int64(values) => values.null_count() != 0,
            Self::Float64(values) => values.null_count() != 0,
            Self::Utf8(values) => values.null_count() != 0,
            Self::LargeUtf8(values) => values.null_count() != 0,
            Self::Dictionary { keys, .. } | Self::LargeDictionary { keys, .. } => {
                keys.null_count() != 0
            }
        }
    }

    fn integer_values(&self) -> Option<&[i64]> {
        match self {
            Self::Int64(values) => Some(values.values()),
            _ => None,
        }
    }

    fn value_at_in_range(&self, row: usize) -> Option<KeyValue<'_>> {
        match self {
            Self::Int64(values) => {
                if values.is_null(row) {
                    return None;
                }
                Some(KeyValue::Int(values.value(row)))
            }
            Self::Float64(values) => {
                if values.is_null(row) {
                    return None;
                }
                Some(KeyValue::Float(values.value(row)))
            }
            Self::Utf8(values) => {
                if values.is_null(row) {
                    return None;
                }
                Some(KeyValue::Text(values.value(row)))
            }
            Self::LargeUtf8(values) => {
                if values.is_null(row) {
                    return None;
                }
                Some(KeyValue::Text(values.value(row)))
            }
            Self::Dictionary { keys, values } => {
                if keys.is_null(row) {
                    return None;
                }
                let index = usize::try_from(keys.value(row)).ok()?;
                Some(KeyValue::Text(values.value(index)))
            }
            Self::LargeDictionary { keys, values } => {
                if keys.is_null(row) {
                    return None;
                }
                let index = usize::try_from(keys.value(row)).ok()?;
                Some(KeyValue::Text(values.value(index)))
            }
        }
    }
}

/// A value a join or a sort can key on.
///
/// The value borrows from the column it was read out of, so reading a key
/// costs a lookup rather than a copy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyValue<'a> {
    /// A 64-bit signed integer.
    Int(i64),
    /// A 64-bit double.
    Float(f64),
    /// A string.
    Text(&'a str),
}

/// A column read one row at a time, where it lies.
#[derive(Debug, Clone)]
pub struct KeyColumn<'a> {
    /// Where the values are.
    values: KeyValues<'a>,
    /// Which rows hold a value.
    validity: &'a NullBitmap,
    /// Whether the column holds a NULL at all.
    ///
    /// A column with no NULLs does not have to have its validity bitmap
    /// read, once a row, in a hot loop -- and the bitmap of a column with no
    /// NULLs says nothing. The filter operator already asks this question of
    /// a column; the key reader did not, and it paid for the bitmap on every
    /// row of every join and every sort.
    nulls: bool,
}

/// A key column's values, resolved once.
///
/// The operators that read a key a row at a time resolve the column's
/// encoding **once**, before the loop, and read the values out of the slice
/// they are given. The encoding is a property of the column, not of the row,
/// and a row at a time the column's own value is read -- with no enum built
/// for it and no `Option` wrapped around it.
#[derive(Debug, Clone)]
pub enum KeySlice<'a> {
    /// Plain integers.
    Int64(&'a [i64]),
    /// Plain doubles.
    Float64(&'a [f64]),
    /// Plain strings.
    Utf8(&'a [String]),
    /// Dictionary-encoded integers.
    Int64Dict {
        /// The distinct values.
        dict: &'a [i64],
        /// One index into the dictionary per row.
        indices: &'a [i32],
    },
    /// Dictionary-encoded doubles.
    Float64Dict {
        /// The distinct values.
        dict: &'a [f64],
        /// One index into the dictionary per row.
        indices: &'a [i32],
    },
    /// Dictionary-encoded strings.
    Utf8Dict {
        /// The distinct values.
        dict: &'a [String],
        /// One index into the dictionary per row.
        indices: &'a [i32],
    },
}

impl<'a> KeySlice<'a> {
    /// The plain integers, when the column holds them plainly.
    #[must_use]
    pub fn integers(&self) -> Option<&'a [i64]> {
        match self {
            Self::Int64(values) => Some(values),
            _ => None,
        }
    }

    /// The plain doubles, when the column holds them plainly.
    #[must_use]
    pub fn doubles(&self) -> Option<&'a [f64]> {
        match self {
            Self::Float64(values) => Some(values),
            _ => None,
        }
    }
}

/// Where a key column's values are.
#[derive(Debug, Clone)]
enum KeyValues<'a> {
    /// Plain integers.
    Int64(&'a [i64]),
    /// Plain doubles.
    Float64(&'a [f64]),
    /// Plain strings.
    Utf8(&'a [String]),
    /// Dictionary-encoded integers.
    Int64Dict {
        /// The distinct values.
        dict: &'a [i64],
        /// One index into the dictionary per row.
        indices: &'a [i32],
    },
    /// Dictionary-encoded doubles.
    Float64Dict {
        /// The distinct values.
        dict: &'a [f64],
        /// One index into the dictionary per row.
        indices: &'a [i32],
    },
    /// Dictionary-encoded strings.
    Utf8Dict {
        /// The distinct values.
        dict: &'a [String],
        /// One index into the dictionary per row.
        indices: &'a [i32],
    },
    /// A decoded column.
    ///
    /// A run-length-encoded column holds runs rather than values, so reading
    /// one of its rows means finding the run the row falls in. The column is
    /// decoded once instead, and the rows are read out of the copy.
    Decoded(BatchData),
}

impl<'a> KeyColumn<'a> {
    /// The column in `chunk`.
    pub fn from_chunk(chunk: &'a ColumnChunk) -> Self {
        let values = match &chunk.encoding {
            Encoding::Int64 { values } => KeyValues::Int64(values),
            Encoding::Float64 { values } => KeyValues::Float64(values),
            Encoding::Utf8 { values } => KeyValues::Utf8(values),
            Encoding::Int64Dict { dict, indices } => KeyValues::Int64Dict { dict, indices },
            Encoding::Float64Dict { dict, indices } => KeyValues::Float64Dict { dict, indices },
            Encoding::Utf8Dict { dict, indices } => KeyValues::Utf8Dict { dict, indices },
            Encoding::BoolRle { .. } => KeyValues::Decoded(chunk.decode().data),
        };
        Self::new(values, &chunk.validity)
    }

    /// The column in an already-decoded column.
    pub fn from_decoded(column: &'a BatchColumn) -> Self {
        let values = match &column.data {
            BatchData::Int64(values) => KeyValues::Int64(values),
            BatchData::Float64(values) => KeyValues::Float64(values),
            BatchData::Utf8(values) => KeyValues::Utf8(values),
        };
        Self::new(values, &column.validity)
    }

    /// A column over `values`, with `validity` saying which rows hold one.
    fn new(values: KeyValues<'a>, validity: &'a NullBitmap) -> Self {
        let nulls = validity.null_count() != 0;
        Self { values, validity, nulls }
    }

    /// Whether the column holds a NULL at all.
    ///
    /// A column that does not is read without touching its validity bitmap.
    #[must_use]
    pub fn has_nulls(&self) -> bool {
        self.nulls
    }

    /// The column's values, resolved once.
    #[must_use]
    pub fn slice(&self) -> KeySlice<'_> {
        match &self.values {
            KeyValues::Int64(values) => KeySlice::Int64(values),
            KeyValues::Float64(values) => KeySlice::Float64(values),
            KeyValues::Utf8(values) => KeySlice::Utf8(values),
            KeyValues::Int64Dict { dict, indices } => KeySlice::Int64Dict { dict, indices },
            KeyValues::Float64Dict { dict, indices } => KeySlice::Float64Dict { dict, indices },
            KeyValues::Utf8Dict { dict, indices } => KeySlice::Utf8Dict { dict, indices },
            KeyValues::Decoded(data) => match data {
                BatchData::Int64(values) => KeySlice::Int64(values),
                BatchData::Float64(values) => KeySlice::Float64(values),
                BatchData::Utf8(values) => KeySlice::Utf8(values),
            },
        }
    }

    /// The column's integer values, if it holds integers.
    ///
    /// The values are the column's own: a dictionary-encoded column is read
    /// through its dictionary, and this is the plain column only. It is what
    /// opens the join's fast path -- an integer key read straight out of the
    /// column, with no enum and no validity bitmap.
    #[must_use]
    pub fn integer_values(&self) -> Option<&'a [i64]> {
        match &self.values {
            KeyValues::Int64(values) => Some(values),
            _ => None,
        }
    }

    /// The value of `row`, if it holds one.
    ///
    /// A row that holds no value has no key: a NULL never matches anything,
    /// and it has no place in an order either. A row **past the end** of the
    /// column has no value either.
    ///
    /// The range check is made by [`KeySource::value_at`], where it lives for
    /// every source: the two paths cannot answer differently about a row that
    /// is not there.
    pub fn value_at(&self, row: usize) -> Option<KeyValue<'_>> {
        <Self as KeySource>::value_at(self, row)
    }

    /// The value of a row **in range**, if it holds one.
    ///
    /// # Panics
    ///
    /// Panics if `row` is out of range for the column. The rows come from a
    /// selection built over this very column, so they are in range by
    /// construction.
    pub fn value_at_in_range(&self, row: usize) -> Option<KeyValue<'_>> {
        if self.nulls && !self.validity.is_valid(row) {
            return None;
        }
        Some(match &self.values {
            KeyValues::Int64(values) => KeyValue::Int(values[row]),
            KeyValues::Float64(values) => KeyValue::Float(values[row]),
            KeyValues::Utf8(values) => KeyValue::Text(values[row].as_str()),
            KeyValues::Int64Dict { dict, indices } => KeyValue::Int(dict[dictionary(indices, row)]),
            KeyValues::Float64Dict { dict, indices } => {
                KeyValue::Float(dict[dictionary(indices, row)])
            }
            KeyValues::Utf8Dict { dict, indices } => {
                KeyValue::Text(dict[dictionary(indices, row)].as_str())
            }
            KeyValues::Decoded(data) => match data {
                BatchData::Int64(values) => KeyValue::Int(values[row]),
                BatchData::Float64(values) => KeyValue::Float(values[row]),
                BatchData::Utf8(values) => KeyValue::Text(values[row].as_str()),
            },
        })
    }
}

/// The dictionary index of `row`.
///
/// # Panics
///
/// Panics if the index is negative, which a dictionary index never is.
fn dictionary(indices: &[i32], row: usize) -> usize {
    let index = indices[row];
    usize::try_from(index).expect("a dictionary index is not negative")
}
